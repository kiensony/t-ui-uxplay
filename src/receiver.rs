use crate::{dacp::Credentials, state::Track};
use anyhow::{Context, Result};
use image::{DynamicImage, ImageFormat, ImageReader, Limits};
use std::{
    fs::{File, Metadata, Permissions},
    io::{self, BufRead, BufReader, Cursor, Read},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::mpsc::{self, Receiver as Inbox, Sender},
    thread,
    time::{Duration, Instant, SystemTime},
};
use tempfile::TempDir;

const MAX_ART_BYTES: u64 = 16 * 1024 * 1024;

pub struct Receiver {
    child: Child,
    pub logs: Inbox<LogRecord>,
    pub exports: Exports,
    _directory: TempDir,
}

impl Receiver {
    pub fn spawn(executable: &Path, name: &str) -> Result<Self> {
        let directory = tempfile::Builder::new()
            .prefix("t-ui-uxplay-")
            .permissions(Permissions::from_mode(0o700))
            .tempdir()?;
        let exports = Exports::new(directory.path());
        let (reader, writer) = io::pipe()?;
        let mut child = Command::new("stdbuf")
            .args(["-oL", "-eL", "--"])
            .arg(executable)
            .args(["-rc", "/dev/null", "-n", name, "-nh", "-vs", "0", "-d", "1"])
            .arg("-md")
            .arg(&exports.metadata.path)
            .arg("-ca")
            .arg(&exports.artwork.path)
            .arg("-dacp")
            .arg(&exports.dacp.path)
            .stdin(Stdio::null())
            .stdout(writer.try_clone()?)
            .stderr(writer)
            .spawn()
            .with_context(|| format!("Could not start {} via stdbuf", executable.display()))?;

        let (send, logs) = mpsc::channel();
        let result = thread::Builder::new()
            .name("uxplay-log".into())
            .spawn(move || {
                let _ = read_log(reader, send);
            });
        if let Err(error) = result {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error.into());
        }
        Ok(Self {
            child,
            logs,
            exports,
            _directory: directory,
        })
    }

    pub fn exit_status(&mut self) -> io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }
}

impl Drop for Receiver {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            // The process is still our child and remains unreaped until try_wait or wait.
            unsafe { libc::kill(self.child.id() as libc::pid_t, libc::SIGTERM) };
            let deadline = Instant::now() + Duration::from_millis(600);
            while Instant::now() < deadline {
                if !matches!(self.child.try_wait(), Ok(None)) {
                    return;
                }
                thread::sleep(Duration::from_millis(20));
            }
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum LogEvent {
    Ready,
    Connected,
    StreamStarted,
    Disconnected,
    Progress(u32, u32),
    Diagnostic(String),
}

pub struct LogRecord {
    pub event: LogEvent,
    pub received_at: Instant,
    pub wall_time: SystemTime,
}

pub fn read_log(input: impl Read, send: Sender<LogRecord>) -> io::Result<()> {
    let mut input = BufReader::new(input);
    let mut record = Vec::new();
    loop {
        let bytes = input.fill_buf()?;
        if bytes.is_empty() {
            return Ok(());
        }
        for &byte in bytes {
            match byte {
                b'\r' | b'\n' => {
                    if let Some(event) = parse_log(&String::from_utf8_lossy(&record))
                        && send
                            .send(LogRecord {
                                event,
                                received_at: Instant::now(),
                                wall_time: SystemTime::now(),
                            })
                            .is_err()
                    {
                        return Ok(());
                    }
                    record.clear();
                }
                _ if record.len() < 8192 => record.push(byte),
                _ => {}
            }
        }
        let len = bytes.len();
        input.consume(len);
    }
}

pub fn parse_log(line: &str) -> Option<LogEvent> {
    if let Some((_, position)) = line.split_once("audio progress (min:sec):") {
        let (position, rest) = position.split_once("; remaining:")?;
        let (_, length) = rest.split_once("; track length")?;
        let (position, length) = (seconds(position)?, seconds(length)?);
        return (length > 0 && position <= length).then_some(LogEvent::Progress(position, length));
    }

    // UxPlay can prefix debug output with a timestamp.
    if let Some((_, count)) = line.split_once("Open connections: ") {
        return match count.trim().parse::<u32>().ok()? {
            0 => Some(LogEvent::Disconnected),
            _ => Some(LogEvent::Connected),
        };
    }
    if line.contains("TEARDOWN request,  96=1")
        || line.contains("TEARDOWN request,  96=0, 110=0")
        || line.contains("lost connection with client")
    {
        return Some(LogEvent::Disconnected);
    }
    if line.contains(" spf=") && line.contains(" usingScreen=") && line.contains("audioFormat=") {
        return Some(LogEvent::StreamStarted);
    }
    if line.contains("register_dnssd: advertised AirPlay service") {
        return Some(LogEvent::Ready);
    }
    if line.contains("dnssd_register_raop failed")
        || line.contains("dnssd_register_airplay failed")
        || line.contains("failed to initialize http daemon")
        || line.contains("GStreamer gst_parse_launch failed")
        || line.contains("stdbuf: failed to run command")
        || line.contains("failed to open configuration file")
    {
        return Some(LogEvent::Diagnostic(clean_text(line)));
    }
    None
}

fn seconds(time: &str) -> Option<u32> {
    let (minutes, seconds) = time.trim().split_once(':')?;
    let minutes: u32 = minutes.parse().ok()?;
    let seconds: u32 = seconds.parse().ok()?;
    (seconds < 60).then_some(())?;
    minutes.checked_mul(60)?.checked_add(seconds)
}

pub fn metadata(data: &[u8]) -> Option<Option<Track>> {
    // Stock UxPlay writes strlen + 1, so the NUL is the record's commit marker.
    let text = std::str::from_utf8(data.strip_suffix(b"\0")?).ok()?;
    let mut track = Track::default();
    for (tag, value) in text.lines().filter_map(|line| line.split_once(": ")) {
        match tag {
            "Title" => track.title = clean_text(value),
            "Artist" => track.artist = clean_text(value),
            "Album" => track.album = clean_text(value),
            _ => {}
        }
    }
    Some((!track.title.is_empty()).then_some(track))
}

pub fn clean_text(text: &str) -> String {
    text.chars()
        .filter(|character| !character.is_control())
        .collect::<String>()
        .trim()
        .to_owned()
}

pub fn decode_cover(data: &[u8]) -> Option<Option<DynamicImage>> {
    let format = match image::guess_format(data).ok()? {
        ImageFormat::Png if data.ends_with(b"\0\0\0\0IEND\xae\x42\x60\x82") => ImageFormat::Png,
        ImageFormat::Jpeg if data.ends_with(b"\xff\xd9") => ImageFormat::Jpeg,
        _ => return None,
    };
    let mut reader = ImageReader::with_format(Cursor::new(data), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(96 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode().ok()?;
    // UxPlay writes a 1×1 placeholder on receiver and audio stream startup.
    if image.width() == 1 && image.height() == 1 {
        return Some(None);
    }
    Some(Some(image.thumbnail(320, 320)))
}

pub struct Exports {
    metadata: ChangedFile,
    artwork: ChangedFile,
    dacp: ChangedFile,
}

impl Exports {
    fn new(directory: &Path) -> Self {
        Self {
            metadata: ChangedFile::new(directory.join("metadata")),
            artwork: ChangedFile::new(directory.join("artwork")),
            dacp: ChangedFile::new(directory.join("dacp")),
        }
    }

    pub fn track(&mut self) -> Option<Option<Track>> {
        self.metadata
            .read(256 * 1024)
            .flatten()
            .and_then(|data| metadata(&data))
    }

    pub fn artwork(&mut self) -> Option<Option<DynamicImage>> {
        self.artwork
            .read(MAX_ART_BYTES)?
            .as_deref()
            .and_then(decode_cover)
    }

    pub fn credentials(&mut self) -> Option<Option<Credentials>> {
        let update = self.dacp.read(1024)?;
        match update {
            Some(bytes) => Credentials::parse(&bytes).map(Some),
            None => Some(None),
        }
    }
}

#[derive(PartialEq, Eq)]
struct FileStamp {
    inode: u64,
    size: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

impl From<Metadata> for FileStamp {
    fn from(metadata: Metadata) -> Self {
        Self {
            inode: metadata.ino(),
            size: metadata.len(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
        }
    }
}

struct ChangedFile {
    path: PathBuf,
    stamp: Option<FileStamp>,
}

impl ChangedFile {
    fn new(path: PathBuf) -> Self {
        Self { path, stamp: None }
    }

    // No outer value means the file was unchanged or currently being written.
    fn read(&mut self, limit: u64) -> Option<Option<Vec<u8>>> {
        let file = match File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return self.stamp.take().map(|_| None);
            }
            Err(_) => return None,
        };
        let before = FileStamp::from(file.metadata().ok()?);
        if self.stamp.as_ref() == Some(&before) {
            return None;
        }
        if before.size > limit {
            self.stamp = Some(before);
            return None;
        }
        let mut data = Vec::new();
        let mut read = file.take(limit + 1);
        read.read_to_end(&mut data).ok()?;
        let after = FileStamp::from(read.get_ref().metadata().ok()?);
        if before != after || data.len() as u64 != after.size {
            return None;
        }
        self.stamp = Some(after);
        Some(Some(data))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn receiver_export_directory_is_owner_only_and_removed_on_drop() {
        let receiver = Receiver::spawn(Path::new("/usr/bin/true"), "Test").unwrap();
        let path = receiver._directory.path().to_owned();
        let permissions = fs::metadata(&path).unwrap().permissions();
        assert_eq!(permissions.mode() & 0o077, 0);
        drop(receiver);
        assert!(!path.exists());
    }

    #[test]
    fn oversized_export_is_skipped_until_it_changes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("export");
        let mut export = ChangedFile::new(path.clone());
        fs::write(&path, b"oversized").unwrap();
        assert!(export.read(4).is_none());
        assert_eq!(export.stamp.as_ref().map(|stamp| stamp.size), Some(9));
        fs::write(&path, b"fit").unwrap();
        assert_eq!(export.read(4).unwrap().unwrap(), b"fit");
    }

    #[test]
    fn reads_the_finished_file_after_observing_a_partial_write() {
        let directory = tempfile::tempdir().unwrap();
        let mut exports = Exports::new(directory.path());
        let path = directory.path().join("metadata");
        fs::write(&path, b"Title: A").unwrap();
        assert!(exports.track().is_none());
        fs::write(&path, b"Title: A Track\nArtist: An Artist\n\0").unwrap();
        assert_eq!(exports.track().unwrap().unwrap().title, "A Track");
        assert!(exports.track().is_none());
    }
}
