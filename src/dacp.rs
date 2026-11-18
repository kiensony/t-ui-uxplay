use std::{
    ffi::CString,
    io::{self, BufRead, BufReader, Read, Write},
    net::{IpAddr, SocketAddr, SocketAddrV6, TcpStream},
    process::{Command as Process, Stdio},
    sync::mpsc::{self, Receiver, RecvTimeoutError, Sender},
    thread,
    time::{Duration, Instant},
};

const DISCOVERY_INTERVAL: Duration = Duration::from_secs(5);
const IO_TIMEOUT: Duration = Duration::from_millis(900);
const SERVICE_TYPE: &str = "_dacp._tcp";

#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    id: String,
    token: String,
}

impl Credentials {
    pub fn parse(data: &[u8]) -> Option<Self> {
        // UxPlay exports DACP-ID on the first line and Active-Remote on the second.
        let data = std::str::from_utf8(data.strip_suffix(b"\n")?).ok()?;
        let (id, token) = data.split_once('\n')?;
        let (id, token) = (id.trim(), token.trim());
        if id.is_empty()
            || token.is_empty()
            || !id.bytes().all(|byte| byte.is_ascii_hexdigit())
            || !token.bytes().all(|byte| byte.is_ascii_digit())
        {
            return None;
        }
        Some(Self {
            id: id.to_owned(),
            token: token.to_owned(),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Previous,
    PlayPause,
    Next,
}

impl Command {
    fn path(self) -> &'static str {
        match self {
            Self::Previous => "previtem",
            Self::PlayPause => "playpause",
            Self::Next => "nextitem",
        }
    }
}

fn discover(credentials: &Credentials) -> Option<SocketAddr> {
    let output = Process::new("timeout")
        .args([
            "--kill-after=1s",
            "3s",
            "avahi-browse",
            "-rptk",
            SERVICE_TYPE,
        ])
        .env("LC_ALL", "C")
        .stderr(Stdio::null())
        .output()
        .ok()?;
    resolve(&String::from_utf8_lossy(&output.stdout), &credentials.id)
}

fn resolve(output: &str, id: &str) -> Option<SocketAddr> {
    output
        .lines()
        .filter_map(|line| {
            let fields: Vec<_> = line.splitn(10, ';').collect();
            let [
                "=",
                interface,
                _,
                instance,
                SERVICE_TYPE,
                _,
                _,
                address,
                port,
                ..,
            ] = fields.as_slice()
            else {
                return None;
            };
            let suffix = instance.strip_prefix("iTunes_Ctrl_")?;
            if !suffix
                .trim_start_matches('0')
                .eq_ignore_ascii_case(id.trim_start_matches('0'))
            {
                return None;
            }
            let port = port.parse::<u16>().ok().filter(|number| *number > 0)?;
            let ip = address.parse::<IpAddr>().ok()?;
            if let IpAddr::V6(address) = ip
                && address.is_unicast_link_local()
            {
                let interface = CString::new(*interface).ok()?;
                // Avahi provides the interface name needed for IPv6 link-local addresses.
                let scope = unsafe { libc::if_nametoindex(interface.as_ptr()) };
                return (scope != 0).then(|| SocketAddrV6::new(address, port, 0, scope).into());
            }
            Some(SocketAddr::new(ip, port))
        })
        .min_by_key(|address| address.is_ipv6())
}

fn control(address: SocketAddr, credentials: &Credentials, command: Command) -> io::Result<()> {
    let stream = TcpStream::connect_timeout(&address, IO_TIMEOUT)?;
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;

    let port = address.port();
    let host = match address.ip() {
        IpAddr::V4(ip) => format!("{ip}:{port}"),
        IpAddr::V6(ip) => format!("[{ip}]:{port}"),
    };
    request(stream, &host, credentials, command)
}

fn request(
    mut stream: impl Read + Write,
    host: &str,
    credentials: &Credentials,
    command: Command,
) -> io::Result<()> {
    write!(
        stream,
        "GET /ctrl-int/1/{} HTTP/1.1\r\nHost: {host}\r\nActive-Remote: {}\r\nConnection: close\r\n\r\n",
        command.path(),
        credentials.token,
    )?;

    let mut status = String::new();
    BufReader::new(stream).take(4096).read_line(&mut status)?;
    let mut fields = status.split_whitespace();
    let version = fields.next().unwrap_or_default();
    let code = fields.next().and_then(|code| code.parse::<u16>().ok());
    if version.starts_with("HTTP/") && code.is_some_and(|code| (200..300).contains(&code)) {
        Ok(())
    } else {
        Err(io::Error::other("Sender rejected the DACP command"))
    }
}

enum Message {
    Session(u64, Option<Credentials>),
    Control(u64, Command),
}

pub struct Worker {
    send: Sender<Message>,
    pub availability: Receiver<(u64, bool)>,
}

impl Worker {
    pub fn spawn() -> io::Result<Self> {
        let (send, inbox) = mpsc::channel();
        let (report, availability) = mpsc::channel();
        thread::Builder::new().name("dacp".into()).spawn(move || {
            let mut session = 0;
            let mut credentials: Option<Credentials> = None;
            let mut endpoint = None;
            let mut next_discovery = Instant::now();

            loop {
                let message = if credentials.is_some() {
                    inbox.recv_timeout(next_discovery.saturating_duration_since(Instant::now()))
                } else {
                    inbox.recv().map_err(|_| RecvTimeoutError::Disconnected)
                };
                match message {
                    Err(RecvTimeoutError::Disconnected) => break,
                    Err(RecvTimeoutError::Timeout) => {
                        endpoint = credentials.as_ref().and_then(discover);
                        next_discovery = Instant::now() + DISCOVERY_INTERVAL;
                        let _ = report.send((session, endpoint.is_some()));
                    }
                    Ok(Message::Session(revision, new_credentials)) => {
                        session = revision;
                        credentials = new_credentials;
                        endpoint = None;
                        next_discovery = Instant::now();
                    }
                    Ok(Message::Control(revision, command)) if revision == session => {
                        if let (Some(address), Some(credentials)) = (endpoint, credentials.as_ref())
                            && control(address, credentials, command).is_err()
                        {
                            endpoint = None;
                            next_discovery = Instant::now();
                            let _ = report.send((session, false));
                        }
                    }
                    Ok(Message::Control(..)) => {}
                }
            }
        })?;
        Ok(Self { send, availability })
    }

    pub fn session(&self, revision: u64, credentials: Option<Credentials>) {
        let _ = self.send.send(Message::Session(revision, credentials));
    }

    pub fn control(&self, revision: u64, command: Command) {
        let _ = self.send.send(Message::Control(revision, command));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;

    #[test]
    fn resolves_only_the_current_senders_service() {
        let results = concat!(
            "=;eth0;IPv4;iTunes_Ctrl_AA;_dacp._tcp;local;x.local;192.168.0.3;3768;\n",
            "=;eth0;IPv4;iTunes_Ctrl_000B1;_dacp._tcp;local;y.local;192.168.0.4;3641;\n",
        );
        assert_eq!(
            resolve(results, "b1"),
            Some("192.168.0.4:3641".parse().unwrap())
        );
        assert_eq!(resolve(results, "CC"), None);
    }

    #[test]
    fn sends_only_the_requested_command_with_the_active_remote_header() {
        let (client, server) = UnixStream::pair().unwrap();
        server
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let service = thread::spawn(move || {
            let mut server = BufReader::new(server);
            assert_eq!(
                read_line(&mut server),
                "GET /ctrl-int/1/previtem HTTP/1.1\r\n"
            );
            assert_eq!(read_line(&mut server), "Host: 127.0.0.1:9000\r\n");
            assert_eq!(read_line(&mut server), "Active-Remote: 1234\r\n");
            assert_eq!(read_line(&mut server), "Connection: close\r\n");
            assert_eq!(read_line(&mut server), "\r\n");
            server
                .get_mut()
                .write_all(b"HTTP/1.1 204 No Content\r\n\r\n")
                .unwrap();
        });
        let credentials = Credentials::parse(b"ABC\n1234\n").unwrap();
        request(client, "127.0.0.1:9000", &credentials, Command::Previous).unwrap();
        service.join().unwrap();
    }

    fn read_line(reader: &mut impl BufRead) -> String {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        line
    }
}
