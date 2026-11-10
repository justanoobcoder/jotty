//! Command-line arguments.

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

pub const USAGE: &str = "\
Usage: jotty [OPTIONS] [-e COMMAND [ARGS...]]
       jotty [-c PATH] config [export [--merged] [--nix]]
       jotty config reset

Options:
  -c, --config PATH    Use this config file instead of the default
                       ($XDG_CONFIG_HOME/jotty/config.toml); the
                       overrides still apply over it
  -e, --command CMD..  Run CMD with its arguments instead of the shell
  -h, --help           Print this help
  -V, --version        Print the version

Config commands (overrides in $XDG_STATE_HOME/jotty/overrides.toml):
  config               Edit the overrides in a TUI
  config export        Print the overrides as TOML
         --merged      Print the config file merged with the overrides
         --nix         Print a Nix attrset for programs.jotty.settings
  config reset         Remove all overrides
";

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Options {
    pub config: Option<PathBuf>,
    /// Program and arguments to run instead of the shell.
    pub command: Option<Vec<OsString>>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Parsed {
    Run(Options),
    /// `jotty config ...`, with the `--config` file given before it.
    Config(Option<PathBuf>, ConfigCommand),
    Help,
    Version,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ConfigCommand {
    /// Edit the overrides in the TUI.
    Edit,
    /// Print the overrides, or with `merged` the config file merged with
    /// them, as TOML or as Nix.
    Export { merged: bool, nix: bool },
    /// Remove the overrides.
    Reset,
}

fn parse_config_command(args: impl Iterator<Item = OsString>) -> Result<ConfigCommand, String> {
    let args: Vec<OsString> = args.collect();
    let args: Vec<&[u8]> = args.iter().map(|a| a.as_bytes()).collect();
    match args.split_first() {
        Some((&b"export", flags)) => {
            let (mut merged, mut nix) = (false, false);
            for flag in flags {
                match *flag {
                    b"--merged" => merged = true,
                    b"--nix" => nix = true,
                    _ => {
                        let flag = String::from_utf8_lossy(flag);
                        return Err(format!("unknown argument {flag:?} for config export"));
                    }
                }
            }
            Ok(ConfigCommand::Export { merged, nix })
        }
        Some((&b"reset", [])) => Ok(ConfigCommand::Reset),
        Some((&b"reset", _)) => Err("config reset takes no arguments".into()),
        Some((other, _)) => Err(format!(
            "unknown config command {:?}",
            String::from_utf8_lossy(other)
        )),
        None => Ok(ConfigCommand::Edit),
    }
}

/// Parses the arguments, which needn't be UTF-8: paths and commands are
/// passed on as they are.
pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Parsed, String> {
    let mut options = Options::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_bytes() {
            b"-h" | b"--help" => return Ok(Parsed::Help),
            b"-V" | b"--version" => return Ok(Parsed::Version),
            b"-c" | b"--config" => {
                let path = args.next().ok_or("--config needs a path")?;
                options.config = Some(path.into());
            }
            b"config" => {
                let command = parse_config_command(args)?;
                return Ok(Parsed::Config(options.config, command));
            }
            b"-e" | b"--command" => {
                let command: Vec<OsString> = args.by_ref().collect();
                if command.is_empty() {
                    return Err("--command needs a program".into());
                }
                options.command = Some(command);
            }
            _ => {
                if let Some(path) = arg.as_bytes().strip_prefix(b"--config=") {
                    options.config = Some(OsStr::from_bytes(path).into());
                } else {
                    return Err(format!("unknown argument {arg:?}"));
                }
            }
        }
    }
    Ok(Parsed::Run(options))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(args: &[&str]) -> Result<Parsed, String> {
        parse(args.iter().map(OsString::from))
    }

    #[test]
    fn arguments_need_not_be_utf8() {
        let name = OsStr::from_bytes(b"caf\xe9.txt").to_os_string();
        let args = ["-e".into(), "vim".into(), name.clone()];
        let Ok(Parsed::Run(options)) = parse(args) else {
            panic!()
        };
        assert_eq!(options.command.unwrap(), ["vim".into(), name]);
    }

    #[test]
    fn no_arguments() {
        assert_eq!(run(&[]), Ok(Parsed::Run(Options::default())));
    }

    #[test]
    fn help_and_version() {
        assert_eq!(run(&["--help"]), Ok(Parsed::Help));
        assert_eq!(run(&["-V"]), Ok(Parsed::Version));
    }

    #[test]
    fn config_path() {
        let expected = Parsed::Run(Options {
            config: Some("/tmp/x.toml".into()),
            command: None,
        });
        assert_eq!(run(&["-c", "/tmp/x.toml"]), Ok(expected));
        assert!(
            matches!(run(&["--config=/a"]), Ok(Parsed::Run(o)) if o.config == Some("/a".into()))
        );
        assert!(run(&["--config"]).is_err());
    }

    #[test]
    fn command_takes_the_rest() {
        let parsed = run(&["-c", "c.toml", "-e", "vim", "-c", "q"]).unwrap();
        let Parsed::Run(options) = parsed else {
            panic!()
        };
        assert_eq!(options.command.unwrap(), ["vim", "-c", "q"]);
        assert_eq!(options.config, Some("c.toml".into()));
        assert!(run(&["-e"]).is_err());
    }

    #[test]
    fn unknown_argument() {
        assert!(run(&["--bogus"]).is_err());
    }

    #[test]
    fn config_commands() {
        let export = |merged, nix| Ok(Parsed::Config(None, ConfigCommand::Export { merged, nix }));
        assert_eq!(
            run(&["config", "export"]),
            export(/* merged= */ false, /* nix= */ false)
        );
        assert_eq!(
            run(&["config", "export", "--nix", "--merged"]),
            export(/* merged= */ true, /* nix= */ true)
        );
        assert_eq!(
            run(&["-c", "/a.toml", "config", "reset"]),
            Ok(Parsed::Config(Some("/a.toml".into()), ConfigCommand::Reset))
        );
        assert_eq!(
            run(&["config"]),
            Ok(Parsed::Config(None, ConfigCommand::Edit))
        );
        assert!(run(&["config", "reset", "now"]).is_err());
        assert!(run(&["config", "export", "--json"]).is_err());
        assert!(run(&["config", "frobnicate"]).is_err());
        // After -e, `config` is the program's.
        let Ok(Parsed::Run(options)) = run(&["-e", "config"]) else {
            panic!()
        };
        assert_eq!(options.command.unwrap(), ["config"]);
    }
}
