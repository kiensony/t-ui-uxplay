#[cfg(windows)]
use self::windows::Pipe as Stream;
use crate::state::{PresenceStatus, Timing, Track, format_time};
use serde_json::{Value, json};
#[cfg(unix)]
use std::{
    env,
    os::{fd::AsRawFd, unix::net::UnixStream as Stream},
};
use std::{
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, RecvTimeoutError, Sender},
    thread::{self, JoinHandle},
    time::{Duration, Instant, UNIX_EPOCH},
};

#[cfg(windows)]
mod windows;

const RETRY: Duration = Duration::from_secs(2);
const TICK: Duration = Duration::from_millis(250);
const IO_TIMEOUT: Duration = Duration::from_millis(750);
const MAX_FRAME: usize = 1024 * 1024;

const HANDSHAKE: u32 = 0;
const FRAME: u32 = 1;
const CLOSE: u32 = 2;
const PING: u32 = 3;
const PONG: u32 = 4;

pub struct Worker {
    desired: Option<Sender<(u64, Desired)>>,
    pub status: Receiver<(u64, PresenceStatus)>,
    thread: Option<JoinHandle<()>>,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct Desired {
    pub enabled: bool,
    pub application_id: Option<String>,
    pub activity: Option<Listening>,
}

impl Worker {
    pub fn spawn() -> io::Result<Self> {
        Self::start(socket_paths())
    }

    fn start(paths: Vec<PathBuf>) -> io::Result<Self> {
        let (desired, inbox) = mpsc::channel();
        let (statuses, status) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("discord-ipc".into())
            .spawn(move || {
                run(&paths, inbox, statuses);
            })?;
        Ok(Self {
            desired: Some(desired),
            status,
            thread: Some(thread),
        })
    }

    pub fn update(&self, revision: u64, desired: Desired) {
        if let Some(sender) = &self.desired {
            let _ = sender.send((revision, desired));
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        drop(self.desired.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run(paths: &[PathBuf], inbox: Receiver<(u64, Desired)>, report: Sender<(u64, PresenceStatus)>) {
    let mut desired = Desired::default();
    let mut revision = 0;
    let mut last_status = None;
    let mut connection: Option<Ipc> = None;
    let mut published = None;
    let mut next_retry = Instant::now();

    loop {
        match inbox.recv_timeout(TICK) {
            Ok(update) => {
                let (next_revision, update) = inbox.try_iter().last().unwrap_or(update);
                let id_changed = desired.application_id != update.application_id;
                if id_changed {
                    clear(&mut connection, &mut published);
                }
                if id_changed || (!desired.enabled && update.enabled) {
                    next_retry = Instant::now();
                }
                desired = update;
                revision = next_revision;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }

        let status = if !desired.enabled {
            clear(&mut connection, &mut published);
            PresenceStatus::Off
        } else if let Some(id) = &desired.application_id {
            if connection.is_none() && Instant::now() >= next_retry {
                connection = paths.iter().find_map(|path| Ipc::connect(path, id).ok());
                next_retry = Instant::now() + RETRY;
            }

            if let Some(ipc) = &mut connection {
                let result = if desired.activity == published {
                    ipc.poll()
                } else {
                    ipc.set_activity(desired.activity.as_ref())
                };
                if result.is_ok() {
                    published.clone_from(&desired.activity);
                } else {
                    connection = None;
                    published = None;
                    next_retry = Instant::now() + RETRY;
                }
            }
            if connection.is_some() {
                PresenceStatus::Sharing
            } else {
                PresenceStatus::Unavailable
            }
        } else {
            PresenceStatus::Unavailable
        };

        let update = (revision, status);
        if last_status != Some(update) {
            if report.send(update).is_err() {
                break;
            }
            last_status = Some(update);
        }
    }
    clear(&mut connection, &mut published);
}

fn clear(connection: &mut Option<Ipc>, published: &mut Option<Listening>) {
    if let Some(mut ipc) = connection.take()
        && published.take().is_some()
    {
        let _ = ipc.set_activity(None);
    }
}

#[cfg(unix)]
fn socket_paths() -> Vec<PathBuf> {
    let mut directories = Vec::new();
    for key in ["XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"] {
        if let Some(value) = env::var_os(key).filter(|value| !value.is_empty()) {
            let path = PathBuf::from(value);
            if path.is_absolute() && !directories.contains(&path) {
                directories.push(path);
            }
        }
    }
    let tmp = PathBuf::from("/tmp");
    if !directories.contains(&tmp) {
        directories.push(tmp);
    }
    directories
        .iter()
        .flat_map(|directory| {
            [
                directory.clone(),
                directory.join("app/com.discordapp.Discord"),
            ]
            .into_iter()
            .flat_map(|path| (0..10).map(move |index| path.join(format!("discord-ipc-{index}"))))
        })
        .collect()
}

#[cfg(windows)]
fn socket_paths() -> Vec<PathBuf> {
    (0..10)
        .map(|index| PathBuf::from(format!(r"\\.\pipe\discord-ipc-{index}")))
        .collect()
}

#[derive(Clone, PartialEq, Eq)]
pub struct Listening {
    track: Track,
    progress: Option<Progress>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Progress {
    Advancing { start: u64, end: u64 },
    Still { position: u32, duration: u32 },
}

impl Listening {
    pub fn new(track: &Track, timing: Option<Timing>, now: Instant) -> Self {
        let progress = timing.map(|timing| {
            // Discord's local SET_ACTIVITY request uses Unix seconds.
            if timing.is_advancing(now)
                && let Some(start) = timing
                    .started_at
                    .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                    .map(|elapsed| elapsed.as_secs())
                && let Some(end) = start.checked_add(u64::from(timing.duration))
            {
                Progress::Advancing { start, end }
            } else {
                Progress::Still {
                    position: timing.position,
                    duration: timing.duration,
                }
            }
        });
        Self {
            track: track.clone(),
            progress,
        }
    }

    pub fn payload(&self) -> Value {
        const TEXT_LIMIT: usize = 128;
        let title = self
            .track
            .title
            .chars()
            .take(TEXT_LIMIT)
            .collect::<String>();
        let mut activity = json!({
            "type": 2,
            "status_display_type": 2,
            "name": title,
            "details": title,
        });

        let mut state: String = self.track.artist.chars().take(TEXT_LIMIT).collect();
        match self.progress {
            Some(Progress::Advancing { start, end }) => {
                activity["timestamps"] = json!({ "start": start, "end": end });
            }
            Some(Progress::Still { position, duration }) => {
                let time = format!("{} / {}", format_time(position), format_time(duration));
                if state.is_empty() {
                    state = time;
                } else {
                    let artist: String = state.chars().take(TEXT_LIMIT - time.len() - 3).collect();
                    state = format!("{artist} · {time}");
                }
            }
            None => {}
        }
        if !state.is_empty() {
            activity["state"] = Value::String(state);
        }
        activity
    }
}

struct Ipc {
    stream: Stream,
    nonce: u64,
}

impl Ipc {
    fn connect(path: &Path, client_id: &str) -> io::Result<Self> {
        let stream = Stream::connect(path)?;
        #[cfg(unix)]
        {
            stream.set_read_timeout(Some(IO_TIMEOUT))?;
            stream.set_write_timeout(Some(IO_TIMEOUT))?;
        }
        let mut ipc = Self { stream, nonce: 0 };
        ipc.send_json(HANDSHAKE, &json!({ "v": 1, "client_id": client_id }))?;
        ipc.wait_for(|data| data["cmd"] == "DISPATCH" && data["evt"] == "READY")?;
        Ok(ipc)
    }

    fn set_activity(&mut self, activity: Option<&Listening>) -> io::Result<()> {
        self.nonce += 1;
        let nonce = self.nonce.to_string();
        self.send_json(
            FRAME,
            &json!({
                "cmd": "SET_ACTIVITY",
                "nonce": nonce,
                "args": { "pid": std::process::id(), "activity": activity.map(Listening::payload) },
            }),
        )?;
        self.wait_for(|data| data["cmd"] == "SET_ACTIVITY" && data["nonce"] == nonce)
    }

    fn send_json(&mut self, opcode: u32, data: &Value) -> io::Result<()> {
        write_frame(&mut self.stream, opcode, &serde_json::to_vec(data)?)
    }

    #[cfg(unix)]
    fn poll(&mut self) -> io::Result<()> {
        let mut poll = libc::pollfd {
            fd: self.stream.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // The file descriptor is borrowed from the stream for the duration of poll.
        match unsafe { libc::poll(&mut poll, 1, 0) } {
            -1 => Err(io::Error::last_os_error()),
            0 => Ok(()),
            _ => self.receive().map(|_| ()),
        }
    }

    #[cfg(windows)]
    fn poll(&mut self) -> io::Result<()> {
        if self.stream.has_data()? {
            self.receive()?;
        }
        Ok(())
    }

    fn wait_for(&mut self, matches: impl Fn(&Value) -> bool) -> io::Result<()> {
        let deadline = Instant::now() + IO_TIMEOUT;
        loop {
            if self.receive()?.is_some_and(|value| matches(&value)) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Discord did not acknowledge the request",
                ));
            }
        }
    }

    fn receive(&mut self) -> io::Result<Option<Value>> {
        let (opcode, data) = read_frame(&mut self.stream)?;
        match opcode {
            CLOSE => Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "Discord closed IPC",
            )),
            PING => {
                write_frame(&mut self.stream, PONG, &data)?;
                Ok(None)
            }
            FRAME => {
                let value: Value = serde_json::from_slice(&data)?;
                if value["evt"] == "ERROR" {
                    return Err(io::Error::other("Discord rejected the activity"));
                }
                Ok(Some(value))
            }
            _ => Ok(None),
        }
    }
}

fn read_frame(stream: &mut impl Read) -> io::Result<(u32, Vec<u8>)> {
    let mut header = [0; 8];
    stream.read_exact(&mut header)?;
    let opcode = u32::from_le_bytes(header[..4].try_into().unwrap());
    let len = u32::from_le_bytes(header[4..].try_into().unwrap()) as usize;
    if len > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Discord IPC frame is too large",
        ));
    }
    let mut data = vec![0; len];
    stream.read_exact(&mut data)?;
    Ok((opcode, data))
}

fn write_frame(stream: &mut impl Write, opcode: u32, data: &[u8]) -> io::Result<()> {
    stream.write_all(&opcode.to_le_bytes())?;
    stream.write_all(&(data.len() as u32).to_le_bytes())?;
    stream.write_all(data)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{
        io::ErrorKind,
        os::unix::net::{UnixListener, UnixStream},
    };

    #[test]
    fn reconnects_and_clears_on_disconnect_toggle_and_shutdown() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("discord-ipc-0");
        let worker = Worker::start(vec![path.clone()]).unwrap();
        let update = |revision, enabled, id: Option<&str>, activity| {
            worker.update(
                revision,
                Desired {
                    enabled,
                    application_id: id.map(str::to_owned),
                    activity,
                },
            );
        };
        let first_id = Some("12345");
        let next_id = Some("67890");

        let one = Track {
            title: "One".into(),
            artist: "Artist".into(),
            ..Track::default()
        };
        let two = Track {
            title: "Two".into(),
            ..one.clone()
        };
        let now = Instant::now();
        let one = Listening::new(&one, None, now);
        let two = Listening::new(&two, None, now);
        update(1, true, first_id, Some(one.clone()));
        wait_for(&worker, 1, PresenceStatus::Unavailable);

        let listener = UnixListener::bind(path).unwrap();
        listener.set_nonblocking(true).unwrap();
        let mut initial = accept(&listener, "12345");
        assert_eq!(activity_request(&mut initial), one.payload());
        wait_for(&worker, 1, PresenceStatus::Sharing);
        drop(initial);

        wait_for(&worker, 1, PresenceStatus::Unavailable);
        update(2, true, first_id, Some(two.clone()));
        let mut reconnected = accept(&listener, "12345");
        assert_eq!(activity_request(&mut reconnected), two.payload());
        wait_for(&worker, 2, PresenceStatus::Sharing);

        let now = Instant::now();
        let mut player = crate::state::Player::default();
        let wall_time = UNIX_EPOCH + Duration::from_secs(1_790_000_000);
        player.progress(0, 210, now, wall_time);
        let timed = Listening::new(&two.track, player.timing, now);
        update(3, true, next_id, Some(timed));
        assert_eq!(activity_request(&mut reconnected), Value::Null);
        let mut reconnected = accept(&listener, "67890");
        let request = activity_request(&mut reconnected);
        assert_eq!(request["name"], "Two");
        assert_eq!(
            request["timestamps"],
            json!({ "start": 1_790_000_000_u64, "end": 1_790_000_210_u64 })
        );
        wait_for(&worker, 3, PresenceStatus::Sharing);

        update(4, true, next_id, None);
        assert_eq!(activity_request(&mut reconnected), Value::Null);
        wait_for(&worker, 4, PresenceStatus::Sharing);
        update(5, true, next_id, Some(one.clone()));
        assert_eq!(activity_request(&mut reconnected), one.payload());
        wait_for(&worker, 5, PresenceStatus::Sharing);

        update(6, true, None, None);
        assert_eq!(activity_request(&mut reconnected), Value::Null);
        wait_for(&worker, 6, PresenceStatus::Unavailable);

        update(7, true, next_id, Some(two.clone()));
        let mut reconnected = accept(&listener, "67890");
        assert_eq!(activity_request(&mut reconnected), two.payload());
        wait_for(&worker, 7, PresenceStatus::Sharing);

        update(8, false, None, None);
        assert_eq!(activity_request(&mut reconnected), Value::Null);
        wait_for(&worker, 8, PresenceStatus::Off);

        update(9, true, next_id, Some(two.clone()));
        let mut final_connection = accept(&listener, "67890");
        assert_eq!(activity_request(&mut final_connection), two.payload());
        wait_for(&worker, 9, PresenceStatus::Sharing);
        let shutdown = thread::spawn(move || drop(worker));
        assert_eq!(activity_request(&mut final_connection), Value::Null);
        shutdown.join().unwrap();
    }

    fn wait_for(worker: &Worker, revision: u64, expected: PresenceStatus) {
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            let status = worker
                .status
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap();
            if status == (revision, expected) {
                return;
            }
        }
    }

    fn accept(listener: &UnixListener, application_id: &str) -> UnixStream {
        let deadline = Instant::now() + Duration::from_secs(4);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error)
                    if error.kind() == ErrorKind::WouldBlock && Instant::now() < deadline =>
                {
                    thread::sleep(Duration::from_millis(10))
                }
                Err(error) => panic!("IPC mock accept: {error}"),
            }
        };
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let (opcode, data) = read_frame(&mut stream).unwrap();
        assert_eq!(opcode, HANDSHAKE);
        assert_eq!(
            serde_json::from_slice::<Value>(&data).unwrap(),
            json!({"v": 1, "client_id": application_id})
        );
        write_frame(&mut stream, PING, b"ping").unwrap();
        assert_eq!(read_frame(&mut stream).unwrap(), (PONG, b"ping".to_vec()));
        reply(&mut stream, json!({"cmd": "DISPATCH", "evt": "READY"}));
        stream
    }

    fn activity_request(stream: &mut UnixStream) -> Value {
        let (opcode, data) = read_frame(stream).unwrap();
        assert_eq!(opcode, FRAME);
        let data: Value = serde_json::from_slice(&data).unwrap();
        assert_eq!(data["cmd"], "SET_ACTIVITY");
        assert_eq!(data["args"]["pid"], std::process::id());
        reply(
            stream,
            json!({"cmd": "SET_ACTIVITY", "nonce": data["nonce"], "data": null}),
        );
        data["args"]["activity"].clone()
    }

    fn reply(stream: &mut UnixStream, data: Value) {
        write_frame(stream, FRAME, &serde_json::to_vec(&data).unwrap()).unwrap();
    }
}
