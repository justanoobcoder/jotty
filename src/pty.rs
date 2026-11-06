//! Pseudo-terminal: spawns the child process and exchanges bytes with it.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, ExitStatus, Stdio};

use crate::terminfo::Term;

/// Terminal size in cells and pixels, as reported to the child.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowSize {
    pub cols: u16,
    pub rows: u16,
    pub width: u16,
    pub height: u16,
}

impl WindowSize {
    fn to_winsize(self) -> libc::winsize {
        libc::winsize {
            ws_row: self.rows,
            ws_col: self.cols,
            ws_xpixel: self.width,
            ws_ypixel: self.height,
        }
    }
}

pub struct Pty {
    master: File,
    child: Child,
    /// Input not yet accepted by the (non-blocking) master.
    pending: VecDeque<u8>,
}

fn check(ret: libc::c_int) -> io::Result<libc::c_int> {
    if ret < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(ret)
    }
}

fn set_flag(
    fd: BorrowedFd<'_>,
    get: libc::c_int,
    set: libc::c_int,
    flag: libc::c_int,
) -> io::Result<()> {
    let fd = fd.as_raw_fd();
    // SAFETY: fcntl on a valid, owned descriptor.
    unsafe {
        let flags = check(libc::fcntl(fd, get))?;
        check(libc::fcntl(fd, set, flags | flag))?;
    }
    Ok(())
}

/// How much queued input leaves no room for more terminal replies.
const MAX_QUEUED_RESPONSES: usize = 1 << 20;

impl Pty {
    /// Spawns `program` with `args` on a new pseudo-terminal.
    pub fn spawn(program: &str, args: &[String], term: &Term, size: WindowSize) -> io::Result<Pty> {
        let (mut master, mut slave) = (0, 0);
        let winsize = size.to_winsize();
        // SAFETY: openpty writes two descriptors into the provided pointers.
        check(unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null(),
                &winsize,
            )
        })?;
        // SAFETY: openpty succeeded, so both descriptors are valid and ours.
        let (master, slave) =
            unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
        set_flag(
            master.as_fd(),
            libc::F_GETFD,
            libc::F_SETFD,
            libc::FD_CLOEXEC,
        )?;
        set_flag(
            slave.as_fd(),
            libc::F_GETFD,
            libc::F_SETFD,
            libc::FD_CLOEXEC,
        )?;

        let mut command = Command::new(program);
        command
            .args(args)
            .stdin(Stdio::from(slave.try_clone()?))
            .stdout(Stdio::from(slave.try_clone()?))
            .stderr(Stdio::from(slave))
            .env("TERM", &term.name)
            .env("COLORTERM", "truecolor")
            .env("TERM_PROGRAM", "jotty")
            .env("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION"))
            .env_remove("LINES")
            .env_remove("COLUMNS");
        // SAFETY: only async-signal-safe calls between fork and exec.
        unsafe {
            command.pre_exec(|| {
                // Become a session leader with the pty as controlling terminal.
                check(libc::setsid())?;
                check(libc::ioctl(0, libc::TIOCSCTTY, 0))?;
                // Undo signal dispositions a parent may have changed.
                for signal in [
                    libc::SIGCHLD,
                    libc::SIGHUP,
                    libc::SIGINT,
                    libc::SIGQUIT,
                    libc::SIGTERM,
                    libc::SIGPIPE,
                ] {
                    libc::signal(signal, libc::SIG_DFL);
                }
                Ok(())
            });
        }
        if let Some(dir) = &term.terminfo {
            command.env("TERMINFO", dir);
        }
        // Before spawning: failing after it would leave the child behind.
        set_flag(
            master.as_fd(),
            libc::F_GETFL,
            libc::F_SETFL,
            libc::O_NONBLOCK,
        )?;
        let child = command.spawn()?;
        Ok(Pty {
            master: File::from(master),
            child,
            pending: VecDeque::new(),
        })
    }

    pub fn fd(&self) -> BorrowedFd<'_> {
        self.master.as_fd()
    }

    /// Reads child output. Returns `WouldBlock` when nothing is available.
    /// Once the child has exited, Linux reports `EIO`.
    pub fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.master.read(buf)
    }

    /// Queues input for the child and writes as much as possible right away.
    pub fn write(&mut self, data: &[u8]) -> io::Result<()> {
        self.pending.extend(data);
        self.flush()
    }

    /// Queues a reply to a query in the program's output, unless too much
    /// is waiting already: a program that floods queries without reading
    /// its input would otherwise grow the queue forever. What the user
    /// types and pastes goes through [`Pty::write`] and is never dropped.
    pub fn write_response(&mut self, data: &[u8]) -> io::Result<()> {
        if self.pending.len() + data.len() > MAX_QUEUED_RESPONSES {
            log::debug!("the program isn't reading; dropping a terminal reply");
            return Ok(());
        }
        self.write(data)
    }

    /// Forgets queued input, when the pty can't take it any more.
    pub fn discard_pending(&mut self) {
        self.pending.clear();
    }

    /// Writes queued input. Leaves the rest queued if the pty is full.
    pub fn flush(&mut self) -> io::Result<()> {
        while !self.pending.is_empty() {
            let (chunk, _) = self.pending.as_slices();
            match self.master.write(chunk) {
                Ok(n) => {
                    self.pending.drain(..n);
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    pub fn resize(&self, size: WindowSize) -> io::Result<()> {
        let winsize = size.to_winsize();
        // SAFETY: TIOCSWINSZ reads a winsize struct.
        check(unsafe { libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ, &winsize) })?;
        Ok(())
    }

    /// The current directory of the program in the foreground (the shell,
    /// or what it runs), for resolving relative paths it printed.
    pub fn foreground_cwd(&self) -> Option<std::path::PathBuf> {
        // SAFETY: tcgetpgrp only reads the terminal's foreground group.
        let group = unsafe { libc::tcgetpgrp(self.master.as_raw_fd()) };
        // The group's leader may be gone while the group lives on.
        let cwd = |pid: u32| std::fs::read_link(format!("/proc/{pid}/cwd")).ok();
        (group > 0)
            .then(|| cwd(group as u32))
            .flatten()
            .or_else(|| cwd(self.child.id()))
    }

    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        // A child already reaped has no pid of its own any more: it may
        // belong to another process by now.
        if !matches!(self.child.try_wait(), Ok(None)) {
            return;
        }
        // Closing the master sends SIGHUP to the session; make sure it goes away.
        // SAFETY: plain kill(2) on our own child, which hasn't been reaped.
        unsafe {
            libc::kill(self.child.id() as libc::pid_t, libc::SIGHUP);
        }
        // Not waiting for it: the window must close at once. jotty exits
        // right after this, and init reaps whatever is left.
        if let Err(e) = self.child.try_wait() {
            log::debug!("cannot reap the child: {e}");
        }
    }
}

/// The program to run when none is given: `$SHELL`, then `/bin/sh`.
pub fn default_shell() -> String {
    std::env::var("SHELL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/bin/sh".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    const SIZE: WindowSize = WindowSize {
        cols: 80,
        rows: 24,
        width: 800,
        height: 480,
    };

    /// Reads everything until the child closes the pty.
    fn read_to_end(pty: &mut Pty) -> String {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut out = Vec::new();
        let mut buf = [0; 4096];
        while Instant::now() < deadline {
            match pty.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => out.extend_from_slice(&buf[..n]),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5))
                }
                Err(_) => break, // EIO: the child hung up.
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    fn xterm() -> Term {
        Term {
            name: "xterm-256color".into(),
            terminfo: None,
        }
    }

    fn sh(script: &str) -> Pty {
        sh_with(script, &xterm())
    }

    fn sh_with(script: &str, term: &Term) -> Pty {
        Pty::spawn("/bin/sh", &["-c".into(), script.into()], term, SIZE).unwrap()
    }

    #[test]
    fn replies_are_dropped_when_the_program_doesnt_read() {
        let mut pty = sh("sleep 5");
        let reply = [b'x'; 4096];
        for _ in 0..1000 {
            pty.write_response(&reply).unwrap();
        }
        assert!(pty.pending.len() <= MAX_QUEUED_RESPONSES);
        // What the user types still queues up, past the limit. It is more
        // than the kernel's buffer can take, however much of that is free.
        pty.write(&vec![b'x'; 2 * MAX_QUEUED_RESPONSES]).unwrap();
        assert!(pty.pending.len() > MAX_QUEUED_RESPONSES);
    }

    #[test]
    fn exports_terminfo_location() {
        let term = Term {
            name: "jotty".into(),
            terminfo: Some("/some/terminfo".into()),
        };
        let mut pty = sh_with("echo \"$TERM:$TERMINFO\"", &term);
        assert!(read_to_end(&mut pty).contains("jotty:/some/terminfo"));
    }

    #[test]
    fn runs_child_with_environment() {
        let mut pty = sh("echo \"$TERM $COLORTERM\"");
        assert!(read_to_end(&mut pty).contains("xterm-256color truecolor"));
    }

    #[test]
    fn child_sees_window_size() {
        let mut pty = sh("stty size");
        assert!(read_to_end(&mut pty).contains("24 80"));
    }

    #[test]
    fn resize_is_visible_to_child() {
        let mut pty = sh("read _; stty size");
        pty.resize(WindowSize {
            cols: 100,
            rows: 30,
            ..SIZE
        })
        .unwrap();
        pty.write(b"\n").unwrap();
        assert!(read_to_end(&mut pty).contains("30 100"));
    }

    #[test]
    fn input_reaches_child() {
        let mut pty = sh("read line; echo \"got:$line\"");
        pty.write(b"hello\n").unwrap();
        assert!(!pty.has_pending());
        assert!(read_to_end(&mut pty).contains("got:hello"));
    }

    #[test]
    fn child_has_controlling_terminal() {
        let mut pty = sh("tty");
        assert!(read_to_end(&mut pty).contains("/dev/pts/"));
    }

    #[test]
    fn reports_exit_status() {
        let mut pty = sh("exit 3");
        read_to_end(&mut pty);
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = pty.try_wait().unwrap() {
                assert_eq!(status.code(), Some(3));
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn spawn_failure_is_an_error() {
        assert!(Pty::spawn("/nonexistent/program", &[], &xterm(), SIZE).is_err());
    }
}
