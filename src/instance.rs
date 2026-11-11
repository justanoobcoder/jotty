//! Single instance: a jotty started with `--single-instance` hands its window
//! over to the jotty already running for the same Wayland display and config
//! file, through a Unix socket in `$XDG_RUNTIME_DIR`, and exits.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Bumped whenever the request format changes, so that a jotty of another
/// version opens a window of its own instead of misreading the request.
const VERSION: u32 = 1;
const MAGIC: &[u8; 4] = b"JOTY";
/// Magic, version and body length.
const HEADER_LEN: usize = 12;
/// The largest request body accepted; the environment is most of it.
const MAX_REQUEST: usize = 1 << 20;
/// How long a client waits for the running jotty to answer.
const REPLY_TIMEOUT: Duration = Duration::from_secs(5);
/// How long the running jotty waits for a client to take its answer.
const REPLY_WRITE_TIMEOUT: Duration = Duration::from_secs(1);
/// The longest answer read; it is a status byte and maybe an error message.
const MAX_REPLY: u64 = 64 << 10;

const OPENED: u8 = 0;
const FAILED: u8 = 1;
const VERSION_MISMATCH: u8 = 2;

/// A window another jotty asks for: what it would have opened itself.
#[derive(Debug, PartialEq, Eq)]
pub struct Request {
    pub cwd: PathBuf,
    pub env: Vec<(OsString, OsString)>,
    /// Program and arguments to run instead of the shell.
    pub command: Option<Vec<OsString>>,
}

impl Request {
    /// The request on the wire: magic, version and body length, then the
    /// body as length-prefixed byte strings.
    #[must_use]
    fn encode(&self) -> Vec<u8> {
        let mut body = Writer::default();
        body.bytes(self.cwd.as_os_str().as_bytes());
        body.u32(self.env.len() as u32);
        for (name, value) in &self.env {
            body.bytes(name.as_bytes());
            body.bytes(value.as_bytes());
        }
        match &self.command {
            None => body.u32(0),
            Some(command) => {
                body.u32(1);
                body.u32(command.len() as u32);
                for arg in command {
                    body.bytes(arg.as_bytes());
                }
            }
        }
        let mut out = Vec::with_capacity(HEADER_LEN + body.0.len());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&VERSION.to_le_bytes());
        out.extend_from_slice(&(body.0.len() as u32).to_le_bytes());
        out.extend_from_slice(&body.0);
        out
    }
}

/// Why a request couldn't be read.
#[derive(Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// Not a jotty request.
    BadMagic,
    /// From a jotty with another request format.
    Version(u32),
    TooLarge(usize),
    Malformed,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::BadMagic => write!(f, "not a jotty request"),
            DecodeError::Version(v) => write!(f, "request format {v}, expected {VERSION}"),
            DecodeError::TooLarge(len) => write!(f, "request of {len} bytes is too large"),
            DecodeError::Malformed => write!(f, "malformed request"),
        }
    }
}

impl std::error::Error for DecodeError {}

/// Decodes a request from the bytes received so far: `None` until all of it
/// has arrived.
fn decode(buf: &[u8]) -> Result<Option<Request>, DecodeError> {
    if buf.len() < HEADER_LEN {
        // A wrong magic is reported as soon as it can be seen.
        let seen = buf.len().min(MAGIC.len());
        if buf[..seen] != MAGIC[..seen] {
            return Err(DecodeError::BadMagic);
        }
        return Ok(None);
    }
    if &buf[..4] != MAGIC {
        return Err(DecodeError::BadMagic);
    }
    let version = u32::from_le_bytes(buf[4..8].try_into().expect("4 bytes"));
    if version != VERSION {
        return Err(DecodeError::Version(version));
    }
    let len = u32::from_le_bytes(buf[8..12].try_into().expect("4 bytes")) as usize;
    if len > MAX_REQUEST {
        return Err(DecodeError::TooLarge(len));
    }
    let Some(body) = buf.get(HEADER_LEN..HEADER_LEN + len) else {
        return Ok(None);
    };
    if buf.len() > HEADER_LEN + len {
        return Err(DecodeError::Malformed);
    }
    let mut r = Reader { buf: body, pos: 0 };
    let cwd = PathBuf::from(r.os_string()?);
    let mut env = Vec::new();
    for _ in 0..r.u32()? {
        env.push((r.os_string()?, r.os_string()?));
    }
    let command = match r.u32()? {
        0 => None,
        1 => {
            let count = r.u32()?;
            // A command has at least the program.
            if count == 0 {
                return Err(DecodeError::Malformed);
            }
            let mut command = Vec::new();
            for _ in 0..count {
                command.push(r.os_string()?);
            }
            Some(command)
        }
        _ => return Err(DecodeError::Malformed),
    };
    if r.pos != body.len() {
        return Err(DecodeError::Malformed);
    }
    Ok(Some(Request { cwd, env, command }))
}

#[derive(Default)]
struct Writer(Vec<u8>);

impl Writer {
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }

    fn bytes(&mut self, b: &[u8]) {
        self.u32(b.len() as u32);
        self.0.extend_from_slice(b);
    }
}

struct Reader<'buf> {
    buf: &'buf [u8],
    pos: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], DecodeError> {
        let end = self.pos.checked_add(n).ok_or(DecodeError::Malformed)?;
        let bytes = self.buf.get(self.pos..end).ok_or(DecodeError::Malformed)?;
        self.pos = end;
        Ok(bytes)
    }

    fn u32(&mut self) -> Result<u32, DecodeError> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes(bytes.try_into().expect("4 bytes")))
    }

    fn os_string(&mut self) -> Result<OsString, DecodeError> {
        let len = self.u32()? as usize;
        Ok(OsString::from_vec(self.take(len)?.to_vec()))
    }
}

/// The running jotty's answer to a request.
#[derive(Debug, PartialEq, Eq)]
pub enum Reply {
    Opened,
    /// The window couldn't be opened, and why.
    Failed(String),
    VersionMismatch,
}

impl Reply {
    #[must_use]
    fn encode(&self) -> Vec<u8> {
        match self {
            Reply::Opened => vec![OPENED],
            Reply::Failed(message) => {
                let mut out = vec![FAILED];
                out.extend_from_slice(message.as_bytes());
                out
            }
            Reply::VersionMismatch => vec![VERSION_MISMATCH],
        }
    }

    /// The reply in `buf`, the whole answer; `None` if there was none.
    fn decode(buf: &[u8]) -> Option<Reply> {
        match buf.split_first()? {
            (&OPENED, _) => Some(Reply::Opened),
            (&FAILED, message) => {
                Some(Reply::Failed(String::from_utf8_lossy(message).into_owned()))
            }
            (&VERSION_MISMATCH, _) => Some(Reply::VersionMismatch),
            _ => None,
        }
    }
}

/// The socket of the jotty serving `display` with the config file `config`
/// (or the default one), so that another config gets an instance of its own.
#[must_use]
fn socket_path(runtime_dir: &Path, display: &OsStr, config: Option<&Path>) -> PathBuf {
    // WAYLAND_DISPLAY may be a path.
    let display = Path::new(display)
        .file_name()
        .unwrap_or(display)
        .to_string_lossy();
    let key = match config {
        Some(path) => {
            let path = fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
            format!("{:016x}", fnv1a(path.as_os_str().as_bytes()))
        }
        None => "default".to_owned(),
    };
    runtime_dir.join(format!("jotty-{display}-{key}.sock"))
}

/// 64-bit FNV-1a: a stable hash, the same in every build.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &b| {
        (hash ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// What starting with `--single-instance` came to.
pub enum Start {
    /// The running jotty opened the window.
    HandedOver,
    /// The running jotty couldn't open the window, and why.
    Failed(String),
    /// No jotty was running: this one opens its window and serves the
    /// others.
    Serve(Server),
    /// This one opens its window on its own, without serving.
    Standalone,
}

/// Hands `request` over to the jotty running for this Wayland display and
/// `config`, or becomes that jotty. Whatever goes wrong, a window still
/// opens: this one's own.
#[must_use]
pub fn start(config: Option<&Path>, request: &Request) -> Start {
    let Some(runtime_dir) = std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty()) else {
        log::warn!("single instance needs XDG_RUNTIME_DIR; opening a window of its own");
        return Start::Standalone;
    };
    // The display libwayland connects to without WAYLAND_DISPLAY.
    let display = std::env::var_os("WAYLAND_DISPLAY")
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| "wayland-0".into());
    let path = socket_path(Path::new(&runtime_dir), &display, config);
    start_at(&path, request).unwrap_or_else(|e| {
        log::warn!(
            "single instance at {}: {e}; opening a window of its own",
            path.display()
        );
        Start::Standalone
    })
}

fn start_at(path: &Path, request: &Request) -> io::Result<Start> {
    // Two jottys starting at once take turns, so only one binds the socket
    // and the other connects to it.
    let lock = lock(&path.with_extension("lock"))?;
    match UnixStream::connect(path) {
        Ok(stream) => {
            drop(lock);
            return hand_over(stream, request);
        }
        // No socket, or one left behind by a jotty that is gone.
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
            ) => {}
        Err(e) => return Err(e),
    }
    match fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    let listener = UnixListener::bind(path)?;
    listener.set_nonblocking(true)?;
    let meta = fs::metadata(path)?;
    log::info!("serving other jottys at {}", path.display());
    Ok(Start::Serve(Server {
        listener,
        path: path.to_owned(),
        id: (meta.dev(), meta.ino()),
    }))
}

/// Takes the lock file at `path`, until the returned file is dropped.
fn lock(path: &Path) -> io::Result<File> {
    let file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)?;
    // SAFETY: flock on a descriptor the file owns for the duration of the
    // call; the lock goes away when the file is closed.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(file)
}

fn hand_over(mut stream: UnixStream, request: &Request) -> io::Result<Start> {
    stream.set_read_timeout(Some(REPLY_TIMEOUT))?;
    stream.set_write_timeout(Some(REPLY_TIMEOUT))?;
    stream.write_all(&request.encode())?;
    let mut reply = Vec::new();
    stream.take(MAX_REPLY).read_to_end(&mut reply)?;
    Ok(match Reply::decode(&reply) {
        Some(Reply::Opened) => Start::HandedOver,
        Some(Reply::Failed(message)) => Start::Failed(message),
        Some(Reply::VersionMismatch) => {
            log::info!("the running jotty is another version; opening a window of its own");
            Start::Standalone
        }
        None => {
            // It was exiting, as its last window closed.
            log::info!("the running jotty didn't answer; opening a window of its own");
            Start::Standalone
        }
    })
}

/// The listening socket of the jotty that serves the others. Dropping it
/// removes the socket file, unless another jotty has replaced it since.
pub struct Server {
    listener: UnixListener,
    path: PathBuf,
    /// Device and inode of the socket file this jotty created.
    id: (u64, u64),
}

impl Server {
    /// The listening socket, non-blocking, for the event loop to watch and
    /// accept from.
    pub fn listener(&self) -> io::Result<UnixListener> {
        self.listener.try_clone()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let ours = fs::metadata(&self.path).is_ok_and(|m| (m.dev(), m.ino()) == self.id);
        if ours && let Err(e) = fs::remove_file(&self.path) {
            log::debug!("cannot remove {}: {e}", self.path.display());
        }
    }
}

/// What a client has sent so far.
pub enum Received {
    Incomplete,
    Request(Request),
    Invalid(DecodeError),
}

/// A connection from a jotty handing its window over.
pub struct Client {
    stream: UnixStream,
    received: Vec<u8>,
}

impl Client {
    pub fn new(stream: UnixStream) -> io::Result<Self> {
        stream.set_nonblocking(true)?;
        Ok(Client {
            stream,
            received: Vec::new(),
        })
    }

    /// The socket, for the event loop to watch.
    pub fn socket(&self) -> io::Result<UnixStream> {
        self.stream.try_clone()
    }

    /// Reads what the client sent, when the socket is readable. An error
    /// means the client is gone.
    pub fn read(&mut self) -> io::Result<Received> {
        let mut chunk = [0; 1 << 14];
        loop {
            match self.stream.read(&mut chunk) {
                Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
                Ok(n) => self.received.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    return Ok(Received::Incomplete);
                }
                Err(e) => return Err(e),
            }
            // Decoded after every chunk, so that a client that keeps
            // sending is cut off at the size limit.
            match decode(&self.received) {
                Ok(None) => {}
                Ok(Some(request)) => return Ok(Received::Request(request)),
                Err(e) => return Ok(Received::Invalid(e)),
            }
        }
    }

    /// Answers the client, which then goes away.
    pub fn reply(&mut self, reply: &Reply) -> io::Result<()> {
        self.stream.set_nonblocking(false)?;
        self.stream.set_write_timeout(Some(REPLY_WRITE_TIMEOUT))?;
        self.stream.write_all(&reply.encode())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    fn request(command: Option<&[&[u8]]>) -> Request {
        Request {
            cwd: PathBuf::from(OsStr::from_bytes(b"/home/caf\xe9")),
            env: vec![
                ("PATH".into(), "/bin".into()),
                (OsString::from_vec(b"X\xff".to_vec()), OsString::new()),
            ],
            command: command.map(|c| c.iter().map(|a| OsString::from_vec(a.to_vec())).collect()),
        }
    }

    #[test]
    fn requests_round_trip_whatever_their_bytes() {
        for command in [None, Some(&[b"vim" as &[u8], b"caf\xe9.txt"][..])] {
            let request = request(command);
            assert_eq!(decode(&request.encode()), Ok(Some(request)));
        }
    }

    #[test]
    fn a_request_is_incomplete_until_its_last_byte() {
        let bytes = request(Some(&[b"ls"])).encode();
        for len in 0..bytes.len() {
            assert_eq!(decode(&bytes[..len]), Ok(None), "at {len} bytes");
        }
    }

    #[test]
    fn bad_requests_are_errors() {
        let bytes = request(None).encode();
        assert_eq!(decode(b"GET / HTTP/1.1\r\n"), Err(DecodeError::BadMagic));
        assert_eq!(decode(b"GE"), Err(DecodeError::BadMagic));

        let mut other_version = bytes.clone();
        other_version[4..8].copy_from_slice(&2u32.to_le_bytes());
        assert_eq!(decode(&other_version), Err(DecodeError::Version(2)));

        let mut huge = bytes.clone();
        huge[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(decode(&huge), Err(DecodeError::TooLarge(_))));

        let empty_command = request(Some(&[])).encode();
        assert_eq!(decode(&empty_command), Err(DecodeError::Malformed));

        let mut trailing = bytes.clone();
        trailing.push(0);
        assert_eq!(decode(&trailing), Err(DecodeError::Malformed));

        // A string longer than the body.
        let mut body = Writer::default();
        body.u32(1000);
        let mut short = MAGIC.to_vec();
        short.extend(VERSION.to_le_bytes());
        short.extend((body.0.len() as u32).to_le_bytes());
        short.extend(&body.0);
        assert_eq!(decode(&short), Err(DecodeError::Malformed));
    }

    #[test]
    fn replies_round_trip() {
        for reply in [
            Reply::Opened,
            Reply::Failed("cannot run vim".into()),
            Reply::VersionMismatch,
        ] {
            assert_eq!(Reply::decode(&reply.encode()), Some(reply));
        }
        assert_eq!(Reply::decode(b""), None);
        assert_eq!(Reply::decode(b"\x09"), None);
    }

    #[test]
    fn each_display_and_config_has_its_own_socket() {
        let dir = Path::new("/run/user/1000");
        let default = socket_path(dir, "wayland-1".as_ref(), None);
        assert_eq!(default, dir.join("jotty-wayland-1-default.sock"));
        assert_eq!(
            socket_path(dir, "/run/user/1000/wayland-1".as_ref(), None),
            default
        );
        assert_ne!(socket_path(dir, "wayland-0".as_ref(), None), default);
        let a = socket_path(dir, "wayland-1".as_ref(), Some(Path::new("/nonexistent/a")));
        let b = socket_path(dir, "wayland-1".as_ref(), Some(Path::new("/nonexistent/b")));
        assert_ne!(a, b);
        assert_ne!(a, default);
        assert_eq!(
            a,
            socket_path(dir, "wayland-1".as_ref(), Some(Path::new("/nonexistent/a")))
        );
    }

    /// A directory of its own for a test, removed when dropped.
    struct TestDir(PathBuf);

    impl TestDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("jotty-{}-{name}", std::process::id()));
            fs::create_dir_all(&dir).unwrap();
            TestDir(dir)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Accepts one client on `server` and answers its request with `reply`.
    fn serve_one(server: &Server, reply: Reply) -> Request {
        let listener = server.listener().unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(5))
                }
                Err(e) => panic!("{e}"),
            }
        };
        let mut client = Client::new(stream).unwrap();
        loop {
            match client.read() {
                Ok(Received::Incomplete) => std::thread::sleep(Duration::from_millis(5)),
                Ok(Received::Request(request)) => {
                    client.reply(&reply).unwrap();
                    return request;
                }
                Ok(Received::Invalid(e)) => panic!("{e}"),
                Err(e) => panic!("{e}"),
            }
        }
    }

    #[test]
    fn a_client_that_keeps_sending_is_cut_off() {
        let (mut peer, stream) = UnixStream::pair().unwrap();
        let mut client = Client::new(stream).unwrap();
        let mut header = MAGIC.to_vec();
        header.extend(VERSION.to_le_bytes());
        header.extend((MAX_REQUEST as u32).to_le_bytes());
        peer.write_all(&header).unwrap();
        peer.set_nonblocking(true).unwrap();
        let junk = vec![0; 1 << 16];
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            // Fills the socket, as a client faster than jotty would.
            while peer.write(&junk).is_ok() {}
            match client.read().unwrap() {
                Received::Incomplete => assert!(Instant::now() < deadline),
                Received::Invalid(DecodeError::Malformed) => break,
                Received::Invalid(e) => panic!("{e}"),
                Received::Request(_) => panic!("junk isn't a request"),
            }
        }
        assert!(client.received.len() <= HEADER_LEN + MAX_REQUEST + junk.len());
    }

    #[test]
    fn the_second_jotty_hands_its_window_to_the_first() {
        let dir = TestDir::new("hand-over");
        let path = dir.0.join("jotty.sock");
        let Ok(Start::Serve(server)) = start_at(&path, &request(None)) else {
            panic!("the first jotty should serve");
        };

        let second = {
            let path = path.clone();
            std::thread::spawn(move || start_at(&path, &request(Some(&[b"htop"]))))
        };
        assert_eq!(serve_one(&server, Reply::Opened), request(Some(&[b"htop"])));
        assert!(matches!(second.join().unwrap(), Ok(Start::HandedOver)));

        let third = {
            let path = path.clone();
            std::thread::spawn(move || start_at(&path, &request(None)))
        };
        serve_one(&server, Reply::Failed("no such program".into()));
        assert!(matches!(third.join().unwrap(), Ok(Start::Failed(m)) if m == "no such program"));

        drop(server);
        assert!(!path.exists(), "the socket goes with the server");
    }

    #[test]
    fn a_socket_left_behind_is_replaced() {
        let dir = TestDir::new("stale");
        let path = dir.0.join("jotty.sock");
        drop(UnixListener::bind(&path).unwrap());
        assert!(path.exists());
        assert!(matches!(
            start_at(&path, &request(None)),
            Ok(Start::Serve(_))
        ));
    }

    #[test]
    fn a_server_leaves_a_socket_it_no_longer_owns() {
        let dir = TestDir::new("replaced");
        let path = dir.0.join("jotty.sock");
        let Ok(Start::Serve(server)) = start_at(&path, &request(None)) else {
            panic!("should serve");
        };
        fs::remove_file(&path).unwrap();
        let other = UnixListener::bind(&path).unwrap();
        drop(server);
        assert!(path.exists());
        drop(other);
    }
}
