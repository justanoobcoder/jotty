//! `jotty config`: the overrides kept in `$XDG_STATE_HOME/jotty/overrides.toml`,
//! which take precedence over the config file and are written by the TUI
//! instead of the (often read-only, home-manager-managed) config file.

mod nix;
mod schema;
mod tui;

use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::cli::ConfigCommand;
use crate::config::{self, Config};

/// The config file and the overrides, as TOML tables.
pub struct Layers {
    /// The config file's table, empty when it is missing or broken.
    pub base: toml::Table,
    /// Why the config file couldn't be read, when it couldn't.
    pub base_error: Option<String>,
    pub overrides_path: PathBuf,
    pub overrides: toml::Table,
}

impl Layers {
    /// Reads both layers. A broken config file is reported in `base_error`,
    /// since the overrides can still be edited over the defaults; broken
    /// overrides are an error, so they are never overwritten unseen.
    pub fn load(config_path: Option<&Path>) -> Result<Layers, String> {
        let overrides_path =
            config::overrides_path().ok_or("neither $XDG_STATE_HOME nor $HOME is set")?;
        let base_path = config_path
            .map(Path::to_path_buf)
            .or_else(config::default_path);
        let (base, base_error) = match &base_path {
            Some(path) => match read_table(path) {
                Ok(Some(table)) => (table, None),
                Ok(None) if config_path.is_none() => (toml::Table::new(), None),
                Ok(None) => (
                    toml::Table::new(),
                    Some(format!("{}: no such file", path.display())),
                ),
                Err(e) => (toml::Table::new(), Some(e)),
            },
            None => (toml::Table::new(), None),
        };
        let overrides = read_table(&overrides_path)
            .map_err(|e| format!("{e}\nFix it, or remove it with: jotty config reset"))?
            .unwrap_or_default();
        Ok(Layers {
            base,
            base_error,
            overrides_path,
            overrides,
        })
    }

    /// The config file with the overrides merged over it.
    pub fn merged(&self) -> toml::Table {
        let mut merged = self.base.clone();
        config::merge(&mut merged, self.overrides.clone());
        merged
    }
}

/// A TOML file as a table, `None` when missing. Its values are checked
/// against the config's schema too, so errors show where they are.
pub fn read_table(path: &Path) -> Result<Option<toml::Table>, String> {
    let error = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
    let Some(text) = config::read_optional(path).map_err(|e| error(&e))? else {
        return Ok(None);
    };
    Config::parse(&text).map_err(|e| error(&e))?;
    toml::from_str(&text).map(Some).map_err(|e| error(&e))
}

/// Writes the overrides, replacing the file in one step so a running
/// jotty never reads half of it. No overrides remove the file.
pub fn save(path: &Path, overrides: &toml::Table) -> io::Result<()> {
    if overrides.is_empty() {
        return match std::fs::remove_file(path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    let text = toml::to_string(overrides).map_err(io::Error::other)?;
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    // A name of its own per process, so two writers never share one.
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    let written = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp)?;
        io::Write::write_all(&mut file, text.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if written.is_err() {
        // Best effort: the error that matters is the one returned.
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

/// The value at `path` in nested tables.
pub fn get_path<'a>(table: &'a toml::Table, path: &[String]) -> Option<&'a toml::Value> {
    let (last, tables) = path.split_last()?;
    let mut table = table;
    for name in tables {
        table = table.get(name)?.as_table()?;
    }
    table.get(last)
}

/// Sets the value at `path`, creating the tables on the way.
pub fn set_path(table: &mut toml::Table, path: &[String], value: toml::Value) {
    let Some((last, tables)) = path.split_last() else {
        return;
    };
    let mut table = table;
    for name in tables {
        let entry = table
            .entry(name.clone())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        if !entry.is_table() {
            *entry = toml::Value::Table(toml::Table::new());
        }
        table = entry.as_table_mut().expect("made a table above");
    }
    table.insert(last.clone(), value);
}

/// Removes the value at `path`, and the tables it leaves empty.
pub fn remove_path(table: &mut toml::Table, path: &[String]) {
    match path {
        [] => {}
        [last] => {
            table.remove(last);
        }
        [first, rest @ ..] => {
            if let Some(toml::Value::Table(inner)) = table.get_mut(first) {
                remove_path(inner, rest);
                if inner.is_empty() {
                    table.remove(first);
                }
            }
        }
    }
}

/// Runs a `jotty config` subcommand.
pub fn run(command: ConfigCommand, config_path: Option<&Path>) -> ExitCode {
    match run_command(command, config_path) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("jotty: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run_command(command: ConfigCommand, config_path: Option<&Path>) -> Result<(), String> {
    // Before reading anything: reset is how a broken file is recovered from.
    if command == ConfigCommand::Reset {
        let path = config::overrides_path().ok_or("neither $XDG_STATE_HOME nor $HOME is set")?;
        return save(&path, &toml::Table::new()).map_err(|e| format!("{}: {e}", path.display()));
    }
    let layers = Layers::load(config_path)?;
    match command {
        ConfigCommand::Edit => {
            if !std::io::IsTerminal::is_terminal(&std::io::stdout()) {
                return Err("jotty config needs a terminal; see jotty config export".into());
            }
            tui::run(layers).map_err(|e| e.to_string())
        }
        ConfigCommand::Export { merged, nix } => {
            if merged && let Some(e) = &layers.base_error {
                return Err(e.clone());
            }
            let table = if merged {
                layers.merged()
            } else {
                layers.overrides
            };
            let text = if nix {
                nix::to_nix(&toml::Value::Table(table))?
            } else {
                toml::to_string(&table).map_err(|e| e.to_string())?
            };
            print!("{text}");
            Ok(())
        }
        ConfigCommand::Reset => unreachable!("handled before loading"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_create_and_prune_tables() {
        let path = |p: &str| p.split('.').map(String::from).collect::<Vec<_>>();
        let mut table = toml::Table::new();
        set_path(
            &mut table,
            &path("files.pdf.launcher"),
            toml::Value::Integer(1),
        );
        set_path(
            &mut table,
            &path("files.image.launcher"),
            toml::Value::Integer(2),
        );
        assert_eq!(
            get_path(&table, &path("files.pdf.launcher")),
            Some(&toml::Value::Integer(1))
        );
        assert_eq!(get_path(&table, &path("files.pdf.launcher.x")), None);
        remove_path(&mut table, &path("files.pdf.launcher"));
        assert_eq!(
            table,
            toml::from_str("[files.image]\nlauncher = 2").unwrap()
        );
        remove_path(&mut table, &path("files.image.launcher"));
        assert!(table.is_empty());
    }

    #[test]
    fn saving_nothing_removes_the_file() {
        let dir = std::env::temp_dir().join(format!("jotty-settings-{}", std::process::id()));
        let path = dir.join("sub").join("overrides.toml");
        let overrides: toml::Table = toml::from_str("[font]\nsize = 12.0").unwrap();
        save(&path, &overrides).unwrap();
        assert_eq!(read_table(&path).unwrap(), Some(overrides));
        save(&path, &toml::Table::new()).unwrap();
        assert!(!path.exists());
        // Already gone is fine.
        save(&path, &toml::Table::new()).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn invalid_tables_are_errors() {
        let dir = std::env::temp_dir().join(format!("jotty-settings-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("overrides.toml");
        std::fs::write(&path, "[font]\nsiez = 1").unwrap();
        let err = read_table(&path).unwrap_err();
        assert!(err.contains("siez"), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
