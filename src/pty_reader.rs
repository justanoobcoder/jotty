//! Reads a pty's output on a thread of its own.
//!
//! The kernel moves a program's output to the pty in small pieces, and
//! the program waits whenever the little the kernel holds is full. Reading
//! on the window's thread stops while it draws or handles input, and the
//! program with it. Here a thread reads all the time, into a queue the
//! window takes from whenever it can. While output streams in, the window
//! takes it without waiting to be woken: see [`PtyReader::streaming`].

use std::fs::File;
use std::io::{self, PipeReader, PipeWriter, Read};
use std::mem;
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use smithay_client_toolkit::reexports::calloop::ping::Ping;

/// The most bytes one read takes.
const READ_SIZE: usize = 1 << 16;
/// The output queued for the window past which the thread waits, and so
/// does the program. The queue ends up at most a read larger, which is
/// about how much the window takes at most before it draws.
const MAX_QUEUED: usize = 1 << 20;
/// While output streams in, the window is woken at most this often: a
/// wake-up costs it about as much as a few kilobytes of output, and the
/// kernel hands output over a few hundred bytes at a time. Also the
/// interval over which [`Rate`] counts the output the window takes.
const STREAM_WAKE_INTERVAL: Duration = Duration::from_millis(1);
/// Output since the last wake-up that makes it a stream. Less, such as a
/// program's query and its reply going back and forth, wakes the window
/// at once. Taken within one interval, it makes the window poll (see
/// [`Rate`]).
const STREAM_BYTES: usize = 4096;

/// What the thread hands the window.
#[derive(Debug, Default)]
struct Queue {
    output: Vec<u8>,
    /// The pty hung up: everything holding its other side is gone. Comes
    /// after the last output.
    closed: bool,
    /// The window is closing; the thread stops.
    stop: bool,
}

#[derive(Debug, Default)]
struct Shared {
    queue: Mutex<Queue>,
    /// Signalled when the window takes the output or closes.
    taken: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Queue> {
        // The lock only guards plain data, which a panic on the other
        // thread leaves whole.
        self.queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// The output read so far, taken by [`PtyReader::take`].
#[derive(Debug)]
pub struct Output {
    /// The new output; empty when there is none.
    pub bytes: Vec<u8>,
    /// Whether the pty hung up after it.
    pub closed: bool,
}

/// A thread reading a pty and queueing the output for the window.
#[derive(Debug)]
pub struct PtyReader {
    shared: Arc<Shared>,
    /// Dropped to wake the thread when the window closes.
    stop: Option<PipeWriter>,
    thread: Option<JoinHandle<()>>,
    /// Keeps the window's loop source when the thread ends: the loop
    /// drops a ping source once every ping is gone.
    _wake: Ping,
    /// How fast the window takes output, for [`PtyReader::streaming`].
    rate: Rate,
}

impl PtyReader {
    /// Starts reading `pty`, a non-blocking pty master, and pinging `wake`
    /// when output arrives with none queued before it.
    pub fn spawn(pty: OwnedFd, wake: Ping) -> io::Result<PtyReader> {
        let shared = Arc::new(Shared::default());
        let (stop_reader, stop) = io::pipe()?;
        let thread = thread::Builder::new().name("pty reader".into()).spawn({
            let shared = Arc::clone(&shared);
            let wake = wake.clone();
            move || read_loop(File::from(pty), &stop_reader, &shared, &wake)
        })?;
        Ok(PtyReader {
            shared,
            stop: Some(stop),
            thread: Some(thread),
            _wake: wake,
            rate: Rate::new(Instant::now()),
        })
    }

    /// Takes the output queued so far. `previous` is the last output
    /// taken, done with, whose buffer the thread fills next.
    pub fn take(&mut self, mut previous: Vec<u8>) -> Output {
        // A buffer a burst of output grew goes once output slows down.
        if previous.len() <= READ_SIZE {
            previous.shrink_to(READ_SIZE);
        }
        previous.clear();
        let mut queue = self.shared.lock();
        let bytes = mem::replace(&mut queue.output, previous);
        let closed = queue.closed;
        drop(queue);
        self.shared.taken.notify_one();
        if !bytes.is_empty() {
            self.rate.taken(bytes.len(), Instant::now());
        }
        Output { bytes, closed }
    }

    /// Whether output streams in, so that the window should take it
    /// without sleeping until it is woken: a few kilobytes came within the
    /// last millisecond or so. A window that stays busy makes the kernel
    /// hand output over faster, where a reader that sleeps and is woken
    /// over and over slows the program writing it down.
    pub fn streaming(&self) -> bool {
        self.rate.streaming(Instant::now())
    }
}

/// How fast the window takes output.
#[derive(Debug)]
struct Rate {
    /// When the interval being counted began.
    since: Instant,
    /// The bytes taken since then.
    bytes: usize,
    /// Until when output counts as streaming in.
    streaming_until: Option<Instant>,
}

impl Rate {
    fn new(now: Instant) -> Self {
        Rate {
            since: now,
            bytes: 0,
            streaming_until: None,
        }
    }

    /// `n` bytes were taken at `now`. `STREAM_BYTES` within one
    /// `STREAM_WAKE_INTERVAL` make a stream, which lasts until output
    /// stops coming that fast for an interval.
    fn taken(&mut self, n: usize, now: Instant) {
        if now - self.since >= STREAM_WAKE_INTERVAL {
            self.since = now;
            self.bytes = 0;
        }
        self.bytes += n;
        if self.bytes >= STREAM_BYTES {
            self.streaming_until = Some(now + STREAM_WAKE_INTERVAL);
        }
    }

    fn streaming(&self, now: Instant) -> bool {
        self.streaming_until.is_some_and(|until| now < until)
    }
}

impl Drop for PtyReader {
    fn drop(&mut self) {
        self.shared.lock().stop = true;
        self.shared.taken.notify_one();
        // Wakes the thread from waiting for the pty.
        drop(self.stop.take());
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            log::error!("the pty reader panicked");
        }
    }
}

/// Reads `pty` into the queue until it hangs up or `stop` closes.
fn read_loop(mut pty: File, stop: &PipeReader, shared: &Shared, wake: &Ping) {
    let mut buf = vec![0; READ_SIZE];
    let mut waker = Waker::new(wake);
    loop {
        match pty.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => match queue(shared, &buf[..n], &mut waker) {
                Some(was_empty) => waker.output(n, was_empty, Instant::now()),
                None => return,
            },
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                if !wait_readable(&pty, stop, &mut waker) {
                    return;
                }
            }
            Err(e) => {
                // EIO: every process holding the pty has exited.
                log::debug!("pty closed: {e}");
                break;
            }
        }
        waker.wake_if_due(Instant::now());
    }
    let mut queue = shared.lock();
    queue.closed = true;
    if !queue.stop {
        drop(queue);
        wake.ping();
    }
}

/// Wakes the window for new output: at once, or while output streams
/// in, `STREAM_WAKE_INTERVAL` after the last time.
struct Waker<'a> {
    ping: &'a Ping,
    /// When the window was last woken.
    woken: Option<Instant>,
    /// When output last came.
    last_output: Option<Instant>,
    /// Bytes that came without a pause as long as `STREAM_WAKE_INTERVAL`.
    stream: usize,
    /// When to wake the window for output it wasn't woken for yet.
    due: Option<Instant>,
}

impl<'a> Waker<'a> {
    fn new(ping: &'a Ping) -> Self {
        Waker {
            ping,
            woken: None,
            last_output: None,
            stream: 0,
            due: None,
        }
    }

    /// `n` bytes were queued at `now`. With `was_empty`, the window has
    /// taken all it was woken for, and has to be woken for these.
    fn output(&mut self, n: usize, was_empty: bool, now: Instant) {
        if self
            .last_output
            .is_none_or(|last| now - last >= STREAM_WAKE_INTERVAL)
        {
            self.stream = 0;
        }
        self.stream += n;
        self.last_output = Some(now);
        if !was_empty {
            return;
        }
        match self.woken {
            Some(woken) if self.stream >= STREAM_BYTES && now - woken < STREAM_WAKE_INTERVAL => {
                self.due.get_or_insert(woken + STREAM_WAKE_INTERVAL);
            }
            _ => self.wake(now),
        }
    }

    fn wake_if_due(&mut self, now: Instant) {
        if self.due.is_some_and(|due| now >= due) {
            self.wake(now);
        }
    }

    fn wake(&mut self, now: Instant) {
        self.ping.ping();
        self.woken = Some(now);
        self.due = None;
    }

    /// How long until a wake-up is due, if one is.
    fn timeout(&self) -> Option<Duration> {
        self.due
            .map(|due| due.saturating_duration_since(Instant::now()))
    }
}

/// Adds `bytes` to the queue, waiting while it is full. Returns whether
/// the queue was empty, or `None` when the window is closing.
fn queue(shared: &Shared, bytes: &[u8], waker: &mut Waker<'_>) -> Option<bool> {
    let mut queue = shared.lock();
    if queue.output.len() >= MAX_QUEUED && waker.due.is_some() {
        // Not to wait for a window never woken for the output.
        waker.wake(Instant::now());
    }
    while queue.output.len() >= MAX_QUEUED && !queue.stop {
        queue = shared
            .taken
            .wait(queue)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
    }
    if queue.stop {
        return None;
    }
    let was_empty = queue.output.is_empty();
    queue.output.extend_from_slice(bytes);
    Some(was_empty)
}

/// Waits until `pty` can be read, waking the window meanwhile when that
/// is due. Returns false once `stop` closes.
fn wait_readable(pty: &File, stop: &PipeReader, waker: &mut Waker<'_>) -> bool {
    let mut fds = [
        libc::pollfd {
            fd: pty.as_fd().as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        },
        libc::pollfd {
            fd: stop.as_fd().as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        },
    ];
    loop {
        // Rounded up, not to wake before it is due.
        let timeout = waker.timeout().map_or(-1, |t| {
            t.as_micros().div_ceil(1000).min(libc::c_int::MAX as u128) as libc::c_int
        });
        // SAFETY: `fds` is a valid array of two pollfd, and both
        // descriptors stay open for the call.
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout) };
        if ready == 0 {
            waker.wake(Instant::now());
            continue;
        }
        if ready > 0 {
            // The pty hanging up shows as readable too: the next read
            // reports it.
            return fds[1].revents == 0;
        }
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::Interrupted {
            log::error!("cannot wait for the pty: {e}");
            return false;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::time::{Duration, Instant};

    use smithay_client_toolkit::reexports::calloop::EventLoop;
    use smithay_client_toolkit::reexports::calloop::ping::make_ping;

    use super::*;

    /// A pipe standing in for a pty: non-blocking, like the pty master.
    fn pipe() -> (OwnedFd, PipeWriter) {
        let (reader, writer) = io::pipe().expect("a pipe");
        let fd = OwnedFd::from(reader);
        // SAFETY: fcntl on a descriptor we own.
        unsafe {
            let flags = libc::fcntl(fd.as_raw_fd(), libc::F_GETFL);
            libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
        (fd, writer)
    }

    /// Takes output until `done` says it is all there.
    fn take_until(reader: &mut PtyReader, done: impl Fn(&[u8], bool) -> bool) -> (Vec<u8>, bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut all = Vec::new();
        loop {
            let output = reader.take(Vec::new());
            all.extend_from_slice(&output.bytes);
            if done(&all, output.closed) {
                return (all, output.closed);
            }
            assert!(
                Instant::now() < deadline,
                "timed out with {} bytes",
                all.len()
            );
            thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn hands_over_all_output_then_the_hang_up() {
        let (fd, mut writer) = pipe();
        let (ping, _source) = make_ping().expect("a ping");
        let mut reader = PtyReader::spawn(fd, ping).expect("a reader");
        // More than the queue holds: the thread waits for it to be taken.
        let data: Vec<u8> = (0..3 * MAX_QUEUED).map(|i| (i % 251) as u8).collect();
        let writing = thread::spawn(move || {
            writer.write_all(&data).expect("writing");
            data
        });
        let (got, _) = take_until(&mut reader, |all, _| all.len() >= 3 * MAX_QUEUED);
        let data = writing.join().expect("the writer");
        assert_eq!(got, data);
        // The writer is gone: the "pty" hangs up.
        let (rest, closed) = take_until(&mut reader, |_, closed| closed);
        assert!(rest.is_empty());
        assert!(closed);
    }

    /// Pings that reached `source` since the last call.
    fn pings(event_loop: &mut EventLoop<'_, usize>) -> usize {
        let mut count = 0;
        event_loop
            .dispatch(Duration::ZERO, &mut count)
            .expect("dispatching");
        count
    }

    fn ping_loop() -> (Ping, EventLoop<'static, usize>) {
        let (ping, source) = make_ping().expect("a ping");
        let event_loop: EventLoop<'_, usize> = EventLoop::try_new().expect("a loop");
        event_loop
            .handle()
            .insert_source(source, |(), &mut (), count| *count += 1)
            .expect("the ping source");
        (ping, event_loop)
    }

    #[test]
    fn a_stream_wakes_the_window_once_an_interval() {
        let (ping, mut event_loop) = ping_loop();
        let mut waker = Waker::new(&ping);
        let start = Instant::now();
        let at = |us: u64| start + Duration::from_micros(us);
        // Output after a pause wakes the window at once.
        waker.output(100, true, at(0));
        assert_eq!(pings(&mut event_loop), 1);
        // Small exchanges go on waking it at once.
        waker.output(100, true, at(100));
        assert_eq!(pings(&mut event_loop), 1);
        // A stream: woken again only an interval after the last time.
        waker.output(STREAM_BYTES, true, at(200));
        assert_eq!(pings(&mut event_loop), 0);
        waker.output(300, false, at(300));
        waker.wake_if_due(at(1000));
        assert_eq!(pings(&mut event_loop), 0);
        waker.wake_if_due(at(1100));
        assert_eq!(pings(&mut event_loop), 1);
        // The stream pauses: what comes next wakes it at once.
        waker.output(10, true, at(1100 + 1000));
        assert_eq!(pings(&mut event_loop), 1);
    }

    #[test]
    fn wakes_for_output_that_follows_a_stream() {
        let (fd, mut writer) = pipe();
        let (ping, source) = make_ping().expect("a ping");
        let mut event_loop: EventLoop<'_, usize> = EventLoop::try_new().expect("a loop");
        event_loop
            .handle()
            .insert_source(source, |(), &mut (), wakes| *wakes += 1)
            .expect("the ping source");
        let mut reader = PtyReader::spawn(fd, ping).expect("a reader");
        let mut wakes = 0;
        let mut take_on_wake = |event_loop: &mut EventLoop<'_, usize>, wakes: &mut usize| {
            let deadline = Instant::now() + Duration::from_secs(5);
            let before = *wakes;
            while *wakes == before {
                assert!(Instant::now() < deadline, "never woken");
                event_loop
                    .dispatch(Duration::from_millis(10), wakes)
                    .expect("dispatching");
            }
            reader.take(Vec::new()).bytes.len()
        };
        // A stream, then a little more before the window may be woken
        // again: the wake-up for it is put off, not dropped.
        writer
            .write_all(&[b'x'; 2 * STREAM_BYTES])
            .expect("writing");
        let mut got = 0;
        while got < 2 * STREAM_BYTES {
            got += take_on_wake(&mut event_loop, &mut wakes);
        }
        writer.write_all(b"tail").expect("writing");
        assert_eq!(take_on_wake(&mut event_loop, &mut wakes), 4);
    }

    #[test]
    fn a_stream_is_taken_until_output_slows_down() {
        let start = Instant::now();
        let at = |us: u64| start + Duration::from_micros(us);
        let mut rate = Rate::new(start);
        assert!(!rate.streaming(at(0)));
        rate.taken(STREAM_BYTES / 2, at(100));
        assert!(!rate.streaming(at(100)));
        rate.taken(STREAM_BYTES / 2, at(200));
        assert!(rate.streaming(at(200)));
        // The stream ends an interval after the last take that made it.
        assert!(rate.streaming(at(1100)));
        assert!(!rate.streaming(at(1200)));
    }

    #[test]
    fn trickling_output_is_not_a_stream() {
        let start = Instant::now();
        let mut rate = Rate::new(start);
        // A line every half millisecond, for a second.
        for i in 0..2000 {
            let now = start + Duration::from_micros(500 * i);
            rate.taken(40, now);
            assert!(!rate.streaming(now), "streaming after {i} lines");
        }
    }

    #[test]
    fn stops_when_dropped_while_waiting() {
        let (fd, _writer) = pipe();
        let (ping, _source) = make_ping().expect("a ping");
        let reader = PtyReader::spawn(fd, ping).expect("a reader");
        // Waiting for output that never comes; dropping must not hang.
        drop(reader);
    }

    #[test]
    fn stops_when_dropped_with_a_full_queue() {
        let (fd, mut writer) = pipe();
        let (ping, _source) = make_ping().expect("a ping");
        let reader = PtyReader::spawn(fd, ping).expect("a reader");
        let writing = thread::spawn(move || {
            let chunk = vec![b'x'; READ_SIZE];
            // Fails once the reader is gone.
            while writer.write_all(&chunk).is_ok() {}
        });
        // Wait until the queue is full and the thread waits on it.
        let deadline = Instant::now() + Duration::from_secs(5);
        while reader.shared.lock().output.len() < MAX_QUEUED {
            assert!(Instant::now() < deadline, "the queue never filled");
            thread::sleep(Duration::from_millis(1));
        }
        drop(reader);
        writing.join().expect("the writer");
    }
}
