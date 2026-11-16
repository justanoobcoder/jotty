//! Opening text in the user's editor: the text goes to a temporary file,
//! which the editor gets as its last argument in a new window, and which is
//! removed when that window closes.

use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::{env, process};

/// The editor to run: `terminal.editor` when set (an empty program name
/// counts as unset), else `$VISUAL`, then `$EDITOR` (split at whitespace,
/// as in `EDITOR="nvim -R"`), else `less`.
#[must_use]
pub fn command(configured: &[String]) -> Vec<OsString> {
    if configured
        .first()
        .is_some_and(|program| !program.is_empty())
    {
        return configured.iter().map(OsString::from).collect();
    }
    from_env(env::var("VISUAL").ok(), env::var("EDITOR").ok())
}

fn from_env(visual: Option<String>, editor: Option<String>) -> Vec<OsString> {
    let words = |value: Option<String>| -> Vec<OsString> {
        value
            .as_deref()
            .unwrap_or("")
            .split_whitespace()
            .map(OsString::from)
            .collect()
    };
    [words(visual), words(editor)]
        .into_iter()
        .find(|command| !command.is_empty())
        .unwrap_or_else(|| vec!["less".into()])
}

/// A file only its owner can read, in `$XDG_RUNTIME_DIR` (or the
/// system's temporary directory), removed when dropped.
#[derive(Debug)]
pub struct TempFile {
    path: PathBuf,
}

impl TempFile {
    /// Writes `text` to a new file.
    pub fn create(text: &str) -> io::Result<TempFile> {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = env::var_os("XDG_RUNTIME_DIR")
            .filter(|d| !d.is_empty())
            .map_or_else(env::temp_dir, PathBuf::from);
        // A jotty that crashed with the same process id may have left one.
        let mut tries = 0;
        let (path, mut file) = loop {
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = dir.join(format!("jotty-output-{}-{n}", process::id()));
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
            {
                Ok(file) => break (path, file),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists && tries < 100 => tries += 1,
                Err(e) => return Err(e),
            }
        };
        // From here on, dropping the `TempFile` removes a file left half
        // written.
        let temp = TempFile { path };
        file.write_all(text.as_bytes())?;
        Ok(temp)
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        match fs::remove_file(&self.path) {
            // The editor may have removed or moved it.
            Err(e) if e.kind() != io::ErrorKind::NotFound => {
                log::warn!("cannot remove {}: {e}", self.path.display());
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn some(s: &str) -> Option<String> {
        Some(s.to_owned())
    }

    #[test]
    fn configured_editor_wins() {
        assert_eq!(command(&["hx".into()]), ["hx"]);
    }

    #[test]
    fn empty_program_name_means_unset() {
        assert_ne!(command(&[String::new(), "-R".into()])[0], "");
    }

    #[test]
    fn visual_then_editor_then_less() {
        assert_eq!(from_env(some("nvim -R"), some("vi")), ["nvim", "-R"]);
        assert_eq!(from_env(some(" "), some("vi")), ["vi"]);
        assert_eq!(from_env(None, some("vi")), ["vi"]);
        assert_eq!(from_env(None, some("")), ["less"]);
        assert_eq!(from_env(None, None), ["less"]);
    }

    #[test]
    fn temp_file_holds_the_text_until_dropped() {
        let file = TempFile::create("one\ntwo\n").unwrap();
        let path = file.path().to_owned();
        assert_eq!(fs::read_to_string(&path).unwrap(), "one\ntwo\n");
        let other = TempFile::create("").unwrap();
        assert_ne!(other.path(), path);
        drop(file);
        assert!(!path.exists());
    }
}
