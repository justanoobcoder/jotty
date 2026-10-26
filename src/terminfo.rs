//! Choosing `$TERM` for the child: jotty's own terminfo entry when it is
//! installed, `xterm-256color` otherwise.

use std::path::{Path, PathBuf};

/// Name of jotty's terminfo entry (`extra/jotty.terminfo`).
pub const NAME: &str = "jotty";
/// Used when the entry isn't installed; works everywhere.
pub const FALLBACK: &str = "xterm-256color";

/// The `$TERM` value and, for jotty's entry, the directory holding it
/// (exported as `$TERMINFO` so the child finds it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Term {
    pub name: String,
    pub terminfo: Option<PathBuf>,
}

/// Resolves the configured `terminal.term`. `"auto"` picks jotty's entry if
/// one of `dirs` contains it.
pub fn resolve(configured: &str, dirs: &[PathBuf]) -> Term {
    if configured != "auto" {
        return Term {
            name: configured.to_string(),
            terminfo: None,
        };
    }
    match dirs.iter().find(|dir| contains_entry(dir, NAME)) {
        Some(dir) => Term {
            name: NAME.to_string(),
            terminfo: Some(dir.clone()),
        },
        None => Term {
            name: FALLBACK.to_string(),
            terminfo: None,
        },
    }
}

/// Whether the compiled terminfo directory `dir` has an entry for `name`,
/// in either the letter (`j/jotty`) or hex (`6a/jotty`) layout.
fn contains_entry(dir: &Path, name: &str) -> bool {
    let Some(first) = name.bytes().next() else {
        return false;
    };
    [(first as char).to_string(), format!("{first:02x}")]
        .iter()
        .any(|sub| dir.join(sub).join(name).is_file())
}

/// Where to look for jotty's entry, in order: next to the executable
/// (`<prefix>/bin/jotty` → `<prefix>/share/terminfo`, as installed by the
/// Nix package or the dev shell), then the places ncurses searches.
pub fn search_dirs() -> Vec<PathBuf> {
    let env_dir = |var: &str| {
        std::env::var_os(var)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let mut dirs = Vec::new();
    if let Some(prefix) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent()?.parent().map(Path::to_path_buf))
    {
        dirs.push(prefix.join("share/terminfo"));
    }
    dirs.extend(env_dir("TERMINFO"));
    if let Some(home) = env_dir("HOME") {
        dirs.push(home.join(".terminfo"));
    }
    if let Some(list) = std::env::var_os("TERMINFO_DIRS") {
        dirs.extend(std::env::split_paths(&list).filter(|p| !p.as_os_str().is_empty()));
    }
    dirs.extend(["/etc/terminfo", "/lib/terminfo", "/usr/share/terminfo"].map(PathBuf::from));
    dirs
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A temporary terminfo directory, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> TempDir {
            let dir =
                std::env::temp_dir().join(format!("jotty-terminfo-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }

        fn add(&self, sub: &str, name: &str) {
            std::fs::create_dir_all(self.0.join(sub)).unwrap();
            std::fs::write(self.0.join(sub).join(name), b"").unwrap();
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn explicit_term_is_kept() {
        let term = resolve("xterm-256color", &[]);
        assert_eq!(term.name, "xterm-256color");
        assert_eq!(term.terminfo, None);
    }

    #[test]
    fn auto_without_entry_falls_back() {
        let empty = TempDir::new("empty");
        let term = resolve("auto", std::slice::from_ref(&empty.0));
        assert_eq!(term.name, FALLBACK);
        assert_eq!(term.terminfo, None);
    }

    #[test]
    fn auto_finds_entry_in_either_layout() {
        let other = TempDir::new("other");
        other.add("x", "xterm-256color");
        let letter = TempDir::new("letter");
        letter.add("j", "jotty");
        let term = resolve("auto", &[other.0.clone(), letter.0.clone()]);
        assert_eq!(term.name, NAME);
        assert_eq!(term.terminfo.as_deref(), Some(letter.0.as_path()));

        let hex = TempDir::new("hex");
        hex.add("6a", "jotty");
        assert_eq!(
            resolve("auto", std::slice::from_ref(&hex.0))
                .terminfo
                .as_deref(),
            Some(hex.0.as_path())
        );
    }

    #[test]
    fn search_starts_next_to_the_executable() {
        let dirs = search_dirs();
        assert!(dirs[0].ends_with("share/terminfo"));
        assert!(dirs.contains(&PathBuf::from("/usr/share/terminfo")));
    }
}
