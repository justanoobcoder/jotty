//! Reasons to reload the config, as file descriptors for the event loop:
//! the config file changing (inotify) and SIGUSR1.

use std::ffi::{CString, OsStr};
use std::fs::File;
use std::io::{self, Read};
use std::os::fd::FromRawFd;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::sync::atomic::{AtomicI32, Ordering};

fn check(ret: libc::c_int) -> io::Result<libc::c_int> {
    if ret < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(ret)
    }
}

/// Watches the directory holding `path`, since editors save by replacing
/// the file and home-manager by replacing a symlink. The returned
/// descriptor is non-blocking and becomes readable on any change there;
/// [`changed`] tells whether it was about the file.
pub fn watch_file(path: &Path) -> io::Result<File> {
    let dir = path.parent().ok_or(io::ErrorKind::InvalidInput)?;
    let dir = CString::new(dir.as_os_str().as_bytes())?;
    // SAFETY: inotify_init1 returns a new descriptor (or -1), which the
    // File then owns; inotify_add_watch reads a NUL-terminated path.
    unsafe {
        let fd = check(libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC))?;
        let file = File::from_raw_fd(fd);
        let mask = libc::IN_CLOSE_WRITE
            | libc::IN_MOVED_TO
            | libc::IN_CREATE
            | libc::IN_DELETE
            | libc::IN_MOVED_FROM;
        check(libc::inotify_add_watch(fd, dir.as_ptr(), mask))?;
        Ok(file)
    }
}

/// Reads the pending inotify events; whether one was about the file
/// `name` in the watched directory.
pub fn changed(file: &mut File, name: &OsStr) -> bool {
    let mut buf = [0u8; 4096];
    let mut found = false;
    while let Ok(n) = file.read(&mut buf) {
        if n == 0 {
            break;
        }
        found |= event_names(&buf[..n]).any(|event| event == name.as_bytes());
    }
    found
}

/// The file names in a buffer of `struct inotify_event`s: a 16-byte header
/// whose last field is the length of the NUL-padded name that follows.
fn event_names(mut buf: &[u8]) -> impl Iterator<Item = &[u8]> {
    std::iter::from_fn(move || {
        let header = buf.get(..16)?;
        let len = u32::from_ne_bytes(header[12..16].try_into().unwrap()) as usize;
        let name = buf.get(16..16 + len)?;
        buf = &buf[16 + len..];
        let end = name.iter().position(|&b| b == 0).unwrap_or(len);
        Some(&name[..end])
    })
}

/// The write end of the SIGUSR1 pipe, for the signal handler.
static SIGNAL_PIPE: AtomicI32 = AtomicI32::new(-1);

extern "C" fn on_signal(_: libc::c_int) {
    let fd = SIGNAL_PIPE.load(Ordering::Relaxed);
    if fd >= 0 {
        // SAFETY: write(2) is async-signal-safe; a full pipe just drops the
        // byte, and one pending byte is enough.
        unsafe { libc::write(fd, [1u8].as_ptr().cast(), 1) };
    }
}

/// Returns a non-blocking descriptor that becomes readable when the process
/// receives SIGUSR1 (read it empty with [`drain`]). Only the first call
/// installs the handler.
pub fn sigusr1() -> io::Result<File> {
    let mut fds = [0; 2];
    // SAFETY: pipe2 fills two new descriptors; the read end goes to the
    // File, the write end lives for the rest of the process in SIGNAL_PIPE.
    // The handler only calls write(2). Programs started later get the
    // default disposition back on exec.
    unsafe {
        check(libc::pipe2(
            fds.as_mut_ptr(),
            libc::O_NONBLOCK | libc::O_CLOEXEC,
        ))?;
        SIGNAL_PIPE.store(fds[1], Ordering::Relaxed);
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = on_signal as *const () as usize;
        action.sa_flags = libc::SA_RESTART;
        libc::sigemptyset(&mut action.sa_mask);
        check(libc::sigaction(
            libc::SIGUSR1,
            &action,
            std::ptr::null_mut(),
        ))?;
        Ok(File::from_raw_fd(fds[0]))
    }
}

/// Reads everything available from a non-blocking descriptor.
pub fn drain(file: &mut File) {
    let mut buf = [0u8; 64];
    while matches!(file.read(&mut buf), Ok(n) if n > 0) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Waits up to a second for `ready`.
    fn eventually(mut ready: impl FnMut() -> bool) -> bool {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(1) {
            if ready() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    #[test]
    fn notices_the_file_being_replaced() {
        let dir = std::env::temp_dir().join(format!("jotty-watch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let mut file = watch_file(&path).unwrap();
        // Other files don't count.
        std::fs::write(dir.join("other"), "x").unwrap();
        std::thread::sleep(Duration::from_millis(50));
        assert!(!changed(&mut file, OsStr::new("config.toml")));
        // Saving by renaming over it, as editors do.
        std::fs::write(dir.join(".config.toml.tmp"), "x").unwrap();
        std::fs::rename(dir.join(".config.toml.tmp"), &path).unwrap();
        assert!(eventually(|| changed(&mut file, OsStr::new("config.toml"))));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn parses_event_names() {
        let mut buf = vec![0u8; 12];
        buf.extend(8u32.to_ne_bytes());
        buf.extend(b"a.toml\0\0");
        buf.extend([0u8; 12]);
        buf.extend(0u32.to_ne_bytes());
        let names: Vec<&[u8]> = event_names(&buf).collect();
        assert_eq!(names, [b"a.toml".as_slice(), b""]);
    }
}
