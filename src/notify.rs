//! Desktop notifications through `org.freedesktop.Notifications`, with a
//! small D-Bus client of its own: it authenticates to the session bus,
//! calls `Notify`, and reads the signals that say a notification was
//! clicked, so the window can be focused with the activation token that
//! came with the click.
//!
//! [`Bus::connect`] blocks, so it runs on a thread of its own. The connected
//! bus is non-blocking and belongs to the event loop: what the socket can't
//! take yet stays in a bounded queue, so a hung bus or daemon can't freeze
//! the window.

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::os::linux::net::SocketAddrExt;
use std::os::unix::net::{SocketAddr, UnixStream};
use std::path::PathBuf;
use std::time::{Duration, Instant};

const NOTIFICATIONS: &str = "org.freedesktop.Notifications";
const NOTIFICATIONS_PATH: &str = "/org/freedesktop/Notifications";
const BUS: &str = "org.freedesktop.DBus";
const BUS_PATH: &str = "/org/freedesktop/DBus";

/// How long a write to the bus, or the authentication, may take.
const TIMEOUT: Duration = Duration::from_secs(1);
/// Larger messages are a broken bus; the messages jotty receives are tiny.
const MAX_MESSAGE: usize = 1 << 20;
/// Notifications remembered for their clicks. A daemon that never reports
/// closing them would otherwise grow the list forever.
const MAX_SHOWN: usize = 32;
/// Messages waiting for the socket, before notifications get dropped.
const MAX_OUTGOING: usize = 1 << 16;
/// Notifications that can be shown at once after a quiet time...
const NOTIFICATION_BURST: f64 = 4.0;
/// ...and how fast that allowance comes back.
const NOTIFICATIONS_PER_SECOND: f64 = 0.5;

const METHOD_CALL: u8 = 1;
const METHOD_RETURN: u8 = 2;
const ERROR: u8 = 3;
const SIGNAL: u8 = 4;
const NO_REPLY_EXPECTED: u8 = 1;

/// Something the window should do after reading the bus.
#[derive(Debug, PartialEq, Eq)]
pub enum Event {
    /// A notification was clicked: focus the window, with the activation
    /// token the daemon sent, if it sent one.
    Activate(Option<String>),
}

struct Shown {
    id: u32,
    /// The daemon's unique bus name; only it may report on the notification.
    sender: String,
    token: Option<String>,
}

/// Limits how many notifications a program can put on the desktop over
/// time, however it splits them across reads of the pty.
#[derive(Debug)]
pub struct RateLimit {
    allowance: f64,
    updated: Instant,
}

impl RateLimit {
    pub fn new(now: Instant) -> Self {
        RateLimit {
            allowance: NOTIFICATION_BURST,
            updated: now,
        }
    }

    /// Whether one more notification may be shown at `now`.
    pub fn allow(&mut self, now: Instant) -> bool {
        let elapsed = now.saturating_duration_since(self.updated).as_secs_f64();
        self.allowance =
            (self.allowance + elapsed * NOTIFICATIONS_PER_SECOND).min(NOTIFICATION_BURST);
        self.updated = now;
        if self.allowance < 1.0 {
            return false;
        }
        self.allowance -= 1.0;
        true
    }
}

pub struct Bus {
    stream: UnixStream,
    serial: u32,
    received: Vec<u8>,
    /// Messages the socket couldn't take yet.
    outgoing: VecDeque<u8>,
    /// Serials of `Notify` calls whose reply (the notification's id) is due.
    pending: VecDeque<u32>,
    shown: VecDeque<Shown>,
}

impl Bus {
    /// Connects to the session bus and subscribes to the notification
    /// daemon's signals.
    pub fn connect() -> io::Result<Bus> {
        let mut stream = connect_session()?;
        stream.set_read_timeout(Some(TIMEOUT))?;
        stream.set_write_timeout(Some(TIMEOUT))?;
        authenticate(&mut stream)?;
        let mut bus = Bus {
            stream,
            serial: 0,
            received: Vec::new(),
            outgoing: VecDeque::new(),
            pending: VecDeque::new(),
            shown: VecDeque::new(),
        };
        // The bus requires Hello first; its reply (our unique name) and the
        // NameAcquired signal that follows are ignored.
        bus.call(BUS, BUS_PATH, BUS, "Hello", "", &[], NO_REPLY_EXPECTED)?;
        let rule = format!("type='signal',interface='{NOTIFICATIONS}',path='{NOTIFICATIONS_PATH}'");
        let mut body = Writer::default();
        body.str(&rule);
        bus.call(
            BUS,
            BUS_PATH,
            BUS,
            "AddMatch",
            "s",
            &body.buf,
            NO_REPLY_EXPECTED,
        )?;
        // Everything is written: the timeouts above let these block.
        bus.stream.set_nonblocking(true)?;
        Ok(bus)
    }

    /// Writes queued messages, as far as the socket takes them. An error
    /// means the connection is gone.
    pub fn flush(&mut self) -> io::Result<()> {
        while !self.outgoing.is_empty() {
            let (chunk, _) = self.outgoing.as_slices();
            match self.stream.write(chunk) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(n) => {
                    self.outgoing.drain(..n);
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    /// Whether messages wait for the socket to take them.
    pub fn has_outgoing(&self) -> bool {
        !self.outgoing.is_empty()
    }

    /// The socket, for the event loop to watch.
    pub fn socket(&self) -> io::Result<UnixStream> {
        self.stream.try_clone()
    }

    /// Shows a notification that focuses the window when clicked. It is
    /// dropped when the bus isn't keeping up.
    pub fn notify(&mut self, app_id: &str, summary: &str, body: &str) -> io::Result<()> {
        if self.outgoing.len() >= MAX_OUTGOING {
            log::warn!("the session bus isn't reading; dropping a notification");
            return Ok(());
        }
        let message = notify_body(app_id, summary, &escape_markup(body));
        let serial = self.call(
            NOTIFICATIONS,
            NOTIFICATIONS_PATH,
            NOTIFICATIONS,
            "Notify",
            "susssasa{sv}i",
            &message,
            0,
        )?;
        if self.pending.len() == MAX_SHOWN {
            self.pending.pop_front();
        }
        self.pending.push_back(serial);
        Ok(())
    }

    /// Reads what the bus sent, when the socket is readable. An error means
    /// the connection is gone.
    pub fn read(&mut self) -> io::Result<Vec<Event>> {
        let mut chunk = [0; 4096];
        match self.stream.read(&mut chunk) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => self.received.extend_from_slice(&chunk[..n]),
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
                ) => {}
            Err(e) => return Err(e),
        }
        let mut events = Vec::new();
        while let Some(len) = message_len(&self.received)?
            && len <= self.received.len()
        {
            let message: Vec<u8> = self.received.drain(..len).collect();
            if let Some(message) = Message::parse(&message) {
                self.handle(message, &mut events);
            }
        }
        Ok(events)
    }

    fn handle(&mut self, message: Message, events: &mut Vec<Event>) {
        match message.kind {
            METHOD_RETURN | ERROR => {
                let Some(i) = self
                    .pending
                    .iter()
                    .position(|&s| Some(s) == message.reply_serial)
                else {
                    return;
                };
                self.pending.remove(i);
                let mut body = message.body();
                if message.kind == ERROR {
                    let text = if message.signature.starts_with('s') {
                        body.str()
                    } else {
                        None
                    };
                    log::warn!(
                        "cannot show a notification: {} {}",
                        message.error_name.unwrap_or_default(),
                        text.unwrap_or_default()
                    );
                } else if message.signature == "u"
                    && let Some(id) = body.u32()
                {
                    if self.shown.len() == MAX_SHOWN {
                        self.shown.pop_front();
                    }
                    let sender = message.sender.unwrap_or_default();
                    self.shown.push_back(Shown {
                        id,
                        sender,
                        token: None,
                    });
                }
            }
            SIGNAL if message.interface.as_deref() == Some(NOTIFICATIONS) => {
                let mut body = message.body();
                let Some(id) = body.u32() else { return };
                let Some(i) = self
                    .shown
                    .iter()
                    .position(|s| s.id == id && Some(&s.sender) == message.sender.as_ref())
                else {
                    return;
                };
                match (message.member.as_deref(), message.signature.as_str()) {
                    // Sent just before ActionInvoked, by daemons following
                    // version 1.2 of the specification.
                    (Some("ActivationToken"), "us") => self.shown[i].token = body.str(),
                    (Some("ActionInvoked"), "us") if body.str().as_deref() == Some("default") => {
                        events.push(Event::Activate(self.shown[i].token.take()));
                    }
                    (Some("NotificationClosed"), _) => {
                        self.shown.remove(i);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn call(
        &mut self,
        destination: &str,
        path: &str,
        interface: &str,
        member: &str,
        signature: &str,
        body: &[u8],
        flags: u8,
    ) -> io::Result<u32> {
        self.serial = self.serial.checked_add(1).unwrap_or(1);
        let message = method_call(
            self.serial,
            flags,
            destination,
            path,
            interface,
            member,
            signature,
            body,
        );
        self.outgoing.extend(message);
        self.flush()?;
        Ok(self.serial)
    }
}

/// A place a D-Bus server listens on.
#[derive(Debug, PartialEq, Eq)]
enum Address {
    Path(PathBuf),
    Abstract(Vec<u8>),
}

fn connect_session() -> io::Result<UnixStream> {
    let mut addresses = std::env::var("DBUS_SESSION_BUS_ADDRESS")
        .map(|value| parse_addresses(&value))
        .unwrap_or_default();
    // Also when the variable only names transports jotty can't use.
    if addresses.is_empty()
        && let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR")
    {
        addresses.push(Address::Path(PathBuf::from(dir).join("bus")));
    }
    let mut last_error = io::Error::new(io::ErrorKind::NotFound, "no session bus address");
    for address in addresses {
        let connected = match &address {
            Address::Path(path) => UnixStream::connect(path),
            Address::Abstract(name) => {
                SocketAddr::from_abstract_name(name).and_then(|a| UnixStream::connect_addr(&a))
            }
        };
        match connected {
            Ok(stream) => return Ok(stream),
            Err(e) => last_error = e,
        }
    }
    Err(last_error)
}

/// The Unix socket addresses in a D-Bus address list, such as
/// `unix:path=/run/user/1000/bus;unix:abstract=/tmp/dbus-x`.
fn parse_addresses(list: &str) -> Vec<Address> {
    list.split(';')
        .filter_map(|entry| {
            let pairs = entry.strip_prefix("unix:")?;
            pairs.split(',').find_map(|pair| {
                let (key, value) = pair.split_once('=')?;
                let value = unescape(value)?;
                match key {
                    "path" => Some(Address::Path(PathBuf::from(String::from_utf8(value).ok()?))),
                    "abstract" => Some(Address::Abstract(value)),
                    _ => None,
                }
            })
        })
        .collect()
}

/// Decodes the `%XX` escapes of an address value.
fn unescape(value: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(value.len());
    let mut bytes = value.bytes();
    while let Some(b) = bytes.next() {
        if b == b'%' {
            let hex = [bytes.next()?, bytes.next()?];
            // from_str_radix alone would accept a sign.
            if !hex.iter().all(u8::is_ascii_hexdigit) {
                return None;
            }
            out.push(u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?);
        } else {
            out.push(b);
        }
    }
    Some(out)
}

/// SASL EXTERNAL: the bus checks our uid on the socket.
fn authenticate(stream: &mut UnixStream) -> io::Result<()> {
    // SAFETY: getuid has no preconditions and cannot fail.
    let uid = unsafe { libc::getuid() };
    let hex: String = uid
        .to_string()
        .bytes()
        .map(|b| format!("{b:02x}"))
        .collect();
    stream.write_all(format!("\0AUTH EXTERNAL {hex}\r\n").as_bytes())?;
    // Read byte by byte, so nothing after the line is consumed.
    let mut line = Vec::new();
    while !line.ends_with(b"\r\n") {
        let mut byte = [0];
        if stream.read(&mut byte)? == 0 || line.len() > 512 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        line.push(byte[0]);
    }
    if !line.starts_with(b"OK ") {
        let reply = String::from_utf8_lossy(&line);
        return Err(io::Error::other(format!(
            "the bus refused authentication: {}",
            reply.trim_end()
        )));
    }
    stream.write_all(b"BEGIN\r\n")
}

/// The body markup of the specification is a subset of HTML; a daemon that
/// supports it would otherwise read `<` and `&` as markup.
fn escape_markup(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c => out.push(c),
        }
    }
    out
}

/// The arguments of `Notify` (signature `susssasa{sv}i`).
fn notify_body(app_id: &str, summary: &str, body: &str) -> Vec<u8> {
    let mut w = Writer::default();
    w.str(app_id); // app_name
    w.u32(0); // replaces_id
    w.str(app_id); // app_icon
    w.str(summary);
    w.str(body);
    // Action keys and labels; "default" is clicking the notification.
    w.array(4, |w| {
        w.str("default");
        w.str("Show");
    });
    w.array(8, |w| {
        w.pad(8);
        w.str("desktop-entry");
        w.signature("s");
        w.str(app_id);
    });
    w.i32(-1); // expire_timeout: the daemon's default
    w.buf
}

#[allow(clippy::too_many_arguments)]
fn method_call(
    serial: u32,
    flags: u8,
    destination: &str,
    path: &str,
    interface: &str,
    member: &str,
    signature: &str,
    body: &[u8],
) -> Vec<u8> {
    let mut w = Writer::default();
    w.buf.extend_from_slice(&[b'l', METHOD_CALL, flags, 1]);
    w.u32(body.len() as u32);
    w.u32(serial);
    w.array(8, |w| {
        let field = |w: &mut Writer, code: u8, kind: &str, value: &str| {
            w.pad(8);
            w.buf.push(code);
            w.signature(kind);
            if kind == "g" {
                w.signature(value);
            } else {
                w.str(value);
            }
        };
        field(w, 1, "o", path);
        field(w, 2, "s", interface);
        field(w, 3, "s", member);
        field(w, 6, "s", destination);
        if !signature.is_empty() {
            field(w, 8, "g", signature);
        }
    });
    w.pad(8);
    w.buf.extend_from_slice(body);
    w.buf
}

/// Marshals values in little-endian D-Bus wire format. Alignment is
/// relative to the start of the buffer, which is fine for a body too: it
/// starts at a multiple of 8 in its message.
#[derive(Default)]
struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    fn pad(&mut self, align: usize) {
        let len = self.buf.len().next_multiple_of(align);
        self.buf.resize(len, 0);
    }

    fn u32(&mut self, value: u32) {
        self.pad(4);
        self.buf.extend_from_slice(&value.to_le_bytes());
    }

    fn i32(&mut self, value: i32) {
        self.pad(4);
        self.buf.extend_from_slice(&value.to_le_bytes());
    }

    /// A string or an object path.
    fn str(&mut self, value: &str) {
        self.u32(value.len() as u32);
        self.buf.extend_from_slice(value.as_bytes());
        self.buf.push(0);
    }

    fn signature(&mut self, value: &str) {
        self.buf.push(value.len() as u8);
        self.buf.extend_from_slice(value.as_bytes());
        self.buf.push(0);
    }

    /// An array whose elements `elements` writes; its length counts the
    /// bytes after the padding to the first element.
    fn array(&mut self, element_align: usize, elements: impl FnOnce(&mut Writer)) {
        self.u32(0);
        let at = self.buf.len() - 4;
        self.pad(element_align);
        let start = self.buf.len();
        elements(self);
        let len = (self.buf.len() - start) as u32;
        self.buf[at..at + 4].copy_from_slice(&len.to_le_bytes());
    }
}

/// The length of the message at the start of `buf`, once its fixed header
/// has arrived.
fn message_len(buf: &[u8]) -> io::Result<Option<usize>> {
    if buf.len() < 16 {
        return Ok(None);
    }
    let big = match buf[0] {
        b'l' => false,
        b'B' => true,
        _ => return Err(io::Error::other("bad D-Bus message")),
    };
    let mut r = Reader { buf, pos: 4, big };
    let body = r.u32().unwrap_or(0) as usize;
    r.pos = 12;
    let fields = r.u32().unwrap_or(0) as usize;
    let len = (16 + fields).next_multiple_of(8) + body;
    if len > MAX_MESSAGE {
        return Err(io::Error::other("D-Bus message too large"));
    }
    Ok(Some(len))
}

/// The parts of a received message jotty looks at.
struct Message<'a> {
    buf: &'a [u8],
    big: bool,
    kind: u8,
    body_start: usize,
    reply_serial: Option<u32>,
    sender: Option<String>,
    interface: Option<String>,
    member: Option<String>,
    error_name: Option<String>,
    signature: String,
}

impl<'a> Message<'a> {
    /// Parses one whole message; `None` when it has header fields of a type
    /// no message to jotty uses, or is cut short.
    fn parse(buf: &'a [u8]) -> Option<Message<'a>> {
        let big = buf.first()? == &b'B';
        let mut r = Reader { buf, pos: 1, big };
        let kind = r.u8()?;
        r.pos = 12;
        let end = 16 + r.u32()? as usize;
        let mut m = Message {
            buf,
            big,
            kind,
            body_start: end.next_multiple_of(8),
            reply_serial: None,
            sender: None,
            interface: None,
            member: None,
            error_name: None,
            signature: String::new(),
        };
        while r.pos < end {
            r.align(8)?;
            let code = r.u8()?;
            match (code, r.signature()?.as_str()) {
                (5, "u") => m.reply_serial = Some(r.u32()?),
                (2, "s") => m.interface = Some(r.str()?),
                (3, "s") => m.member = Some(r.str()?),
                (4, "s") => m.error_name = Some(r.str()?),
                (7, "s") => m.sender = Some(r.str()?),
                (8, "g") => m.signature = r.signature()?,
                (_, "s" | "o") => _ = r.str()?,
                (_, "u") => _ = r.u32()?,
                (_, "g") => _ = r.signature()?,
                _ => return None,
            }
        }
        (m.body_start <= buf.len()).then_some(m)
    }

    fn body(&self) -> Reader<'a> {
        Reader {
            buf: self.buf,
            pos: self.body_start,
            big: self.big,
        }
    }
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
    big: bool,
}

impl Reader<'_> {
    fn align(&mut self, align: usize) -> Option<()> {
        self.pos = self.pos.next_multiple_of(align);
        (self.pos <= self.buf.len()).then_some(())
    }

    fn bytes(&mut self, n: usize) -> Option<&[u8]> {
        let bytes = self.buf.get(self.pos..self.pos.checked_add(n)?)?;
        self.pos += n;
        Some(bytes)
    }

    fn u8(&mut self) -> Option<u8> {
        Some(self.bytes(1)?[0])
    }

    fn u32(&mut self) -> Option<u32> {
        self.align(4)?;
        let big = self.big;
        let bytes: [u8; 4] = self.bytes(4)?.try_into().ok()?;
        Some(if big {
            u32::from_be_bytes(bytes)
        } else {
            u32::from_le_bytes(bytes)
        })
    }

    fn str(&mut self) -> Option<String> {
        let len = self.u32()? as usize;
        let text = String::from_utf8(self.bytes(len)?.to_vec()).ok()?;
        self.u8()?;
        Some(text)
    }

    fn signature(&mut self) -> Option<String> {
        let len = self.u8()? as usize;
        let text = String::from_utf8(self.bytes(len)?.to_vec()).ok()?;
        self.u8()?;
        Some(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A message the daemon could send, built with the same writer.
    fn incoming(kind: u8, fields: &[(u8, &str, &str)], signature: &str, body: &[u8]) -> Vec<u8> {
        let mut w = Writer::default();
        w.buf.extend_from_slice(&[b'l', kind, 0, 1]);
        w.u32(body.len() as u32);
        w.u32(77);
        w.array(8, |w| {
            for &(code, kind, value) in fields {
                w.pad(8);
                w.buf.push(code);
                w.signature(kind);
                match kind {
                    "u" => w.u32(value.parse().unwrap()),
                    "g" => w.signature(value),
                    _ => w.str(value),
                }
            }
            if !signature.is_empty() {
                w.pad(8);
                w.buf.push(8);
                w.signature("g");
                w.signature(signature);
            }
        });
        w.pad(8);
        w.buf.extend_from_slice(body);
        w.buf
    }

    fn signal(member: &str, sender: &str, id: u32, text: &str) -> Vec<u8> {
        let mut body = Writer::default();
        body.u32(id);
        body.str(text);
        incoming(
            SIGNAL,
            &[
                (1, "o", NOTIFICATIONS_PATH),
                (2, "s", NOTIFICATIONS),
                (3, "s", member),
                (7, "s", sender),
            ],
            "us",
            &body.buf,
        )
    }

    fn reply(serial: u32, sender: &str, id: u32) -> Vec<u8> {
        let mut body = Writer::default();
        body.u32(id);
        incoming(
            METHOD_RETURN,
            &[(5, "u", &serial.to_string()), (7, "s", sender)],
            "u",
            &body.buf,
        )
    }

    /// Runs messages through `handle`, as `read` does.
    fn feed(state: &mut (VecDeque<u32>, VecDeque<Shown>), messages: &[Vec<u8>]) -> Vec<Event> {
        let (a, _) = UnixStream::pair().unwrap();
        let mut bus = Bus {
            stream: a,
            serial: 0,
            received: Vec::new(),
            outgoing: VecDeque::new(),
            pending: std::mem::take(&mut state.0),
            shown: std::mem::take(&mut state.1),
        };
        let mut events = Vec::new();
        for m in messages {
            assert_eq!(message_len(m).unwrap(), Some(m.len()));
            bus.handle(Message::parse(m).unwrap(), &mut events);
        }
        *state = (bus.pending, bus.shown);
        events
    }

    #[test]
    fn a_bus_that_doesnt_read_neither_blocks_nor_grows_the_queue() {
        let (a, _peer) = UnixStream::pair().unwrap();
        a.set_nonblocking(true).unwrap();
        let mut bus = Bus {
            stream: a,
            serial: 0,
            received: Vec::new(),
            outgoing: VecDeque::new(),
            pending: VecDeque::new(),
            shown: VecDeque::new(),
        };
        let body = "x".repeat(1024);
        for _ in 0..2000 {
            bus.notify("jotty", "title", &body).unwrap();
        }
        assert!(bus.has_outgoing());
        assert!(bus.outgoing.len() < MAX_OUTGOING + 2048);
    }

    #[test]
    fn the_rate_limit_holds_across_time() {
        let start = Instant::now();
        let mut limit = RateLimit::new(start);
        let allowed = (0..100).filter(|_| limit.allow(start)).count();
        assert_eq!(allowed, NOTIFICATION_BURST as usize);
        assert!(!limit.allow(start + Duration::from_millis(100)));
        assert!(limit.allow(start + Duration::from_secs(3)));
        // Idle time doesn't save up more than a burst.
        let later = start + Duration::from_secs(3600);
        let allowed = (0..100).filter(|_| limit.allow(later)).count();
        assert_eq!(allowed, NOTIFICATION_BURST as usize);
    }

    #[test]
    fn address_escapes_must_be_hexadecimal() {
        assert_eq!(unescape("a%2fb"), Some(b"a/b".to_vec()));
        assert_eq!(unescape("%+f"), None);
    }

    #[test]
    fn notify_message_layout() {
        let body = notify_body("jotty", "Title", "Body");
        let m = method_call(
            3,
            0,
            NOTIFICATIONS,
            NOTIFICATIONS_PATH,
            NOTIFICATIONS,
            "Notify",
            "susssasa{sv}i",
            &body,
        );
        assert_eq!(message_len(&m).unwrap(), Some(m.len()));
        let parsed = Message::parse(&m).unwrap();
        assert_eq!(parsed.kind, METHOD_CALL);
        assert_eq!(parsed.interface.as_deref(), Some(NOTIFICATIONS));
        assert_eq!(parsed.member.as_deref(), Some("Notify"));
        assert_eq!(parsed.signature, "susssasa{sv}i");
        assert_eq!(parsed.body_start % 8, 0);
        // The fixed header: little-endian, method call, protocol 1, the
        // body length and the serial.
        assert_eq!(&m[..4], b"l\x01\x00\x01");
        assert_eq!(
            u32::from_le_bytes(m[4..8].try_into().unwrap()) as usize,
            body.len()
        );
        assert_eq!(u32::from_le_bytes(m[8..12].try_into().unwrap()), 3);

        let mut r = parsed.body();
        assert_eq!(r.str().as_deref(), Some("jotty"));
        assert_eq!(r.u32(), Some(0));
        assert_eq!(r.str().as_deref(), Some("jotty"));
        assert_eq!(r.str().as_deref(), Some("Title"));
        assert_eq!(r.str().as_deref(), Some("Body"));
        // as: its byte length, then the strings.
        assert_eq!(r.u32(), Some(4 + 8 + 4 + 5));
        assert_eq!(r.str().as_deref(), Some("default"));
        assert_eq!(r.str().as_deref(), Some("Show"));
        // a{sv}: the length excludes the padding to the dict entry.
        let len = r.u32().unwrap() as usize;
        r.align(8).unwrap();
        let start = r.pos;
        assert_eq!(r.str().as_deref(), Some("desktop-entry"));
        assert_eq!(r.signature().as_deref(), Some("s"));
        assert_eq!(r.str().as_deref(), Some("jotty"));
        assert_eq!(r.pos - start, len);
        assert_eq!(r.u32(), Some(u32::MAX)); // -1
        assert_eq!(r.pos, r.buf.len());
    }

    #[test]
    fn click_activates_with_token() {
        let mut state = (VecDeque::from([5]), VecDeque::new());
        let events = feed(
            &mut state,
            &[
                reply(5, ":1.9", 42),
                signal("ActivationToken", ":1.9", 42, "tok"),
                signal("ActionInvoked", ":1.9", 42, "default"),
            ],
        );
        assert_eq!(events, [Event::Activate(Some("tok".into()))]);
        assert!(state.0.is_empty());

        let events = feed(
            &mut state,
            &[signal("ActionInvoked", ":1.9", 42, "default")],
        );
        assert_eq!(events, [Event::Activate(None)]);

        let mut body = Writer::default();
        body.u32(42);
        body.u32(2);
        let closed = incoming(
            SIGNAL,
            &[
                (2, "s", NOTIFICATIONS),
                (3, "s", "NotificationClosed"),
                (7, "s", ":1.9"),
            ],
            "uu",
            &body.buf,
        );
        assert!(feed(&mut state, &[closed]).is_empty());
        assert!(state.1.is_empty());
    }

    #[test]
    fn ignores_other_notifications_and_senders() {
        let mut state = (VecDeque::from([5]), VecDeque::new());
        let events = feed(
            &mut state,
            &[
                reply(6, ":1.9", 41),
                reply(5, ":1.9", 42),
                signal("ActionInvoked", ":1.9", 41, "default"),
                signal("ActionInvoked", ":1.66", 42, "default"),
                signal("ActionInvoked", ":1.9", 42, "other"),
            ],
        );
        assert!(events.is_empty());
        assert_eq!(state.1.len(), 1);
    }

    #[test]
    fn big_endian_messages() {
        // A method return with reply serial 5 and body u32 42, in big endian.
        let mut m = vec![b'B', METHOD_RETURN, 0, 1];
        m.extend(4u32.to_be_bytes()); // body length
        m.extend(9u32.to_be_bytes()); // serial
        m.extend(15u32.to_be_bytes()); // header fields length
        m.extend([5, 1, b'u', 0]);
        m.extend(5u32.to_be_bytes());
        m.extend([8, 1, b'g', 0, 1, b'u', 0]);
        m.push(0); // padding to 8
        m.extend(42u32.to_be_bytes());
        assert_eq!(message_len(&m).unwrap(), Some(m.len()));
        let parsed = Message::parse(&m).unwrap();
        assert_eq!(parsed.reply_serial, Some(5));
        assert_eq!(parsed.signature, "u");
        assert_eq!(parsed.body().u32(), Some(42));
    }

    #[test]
    fn partial_and_bad_headers() {
        assert_eq!(message_len(b"l\x02\x00\x01").unwrap(), None);
        assert!(message_len(&[b'x'; 16]).is_err());
        let mut huge = vec![b'l', 4, 0, 1];
        huge.extend(u32::MAX.to_le_bytes());
        huge.extend([0; 8]);
        assert!(message_len(&huge).is_err());
    }

    #[test]
    fn addresses() {
        assert_eq!(
            parse_addresses("unix:path=/run/user/1000/bus"),
            [Address::Path("/run/user/1000/bus".into())]
        );
        assert_eq!(
            parse_addresses(
                "tcp:host=localhost,port=1;unix:guid=ab,abstract=/tmp/dbus-%41b;unix:path=/a%2cb"
            ),
            [
                Address::Abstract(b"/tmp/dbus-Ab".to_vec()),
                Address::Path("/a,b".into())
            ]
        );
        assert!(parse_addresses("unix:path=%4").is_empty());
    }

    #[test]
    fn markup_is_escaped() {
        assert_eq!(
            escape_markup("a < b && c > d"),
            "a &lt; b &amp;&amp; c &gt; d"
        );
    }
}
