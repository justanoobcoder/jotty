//! Command-line arguments.

use std::path::PathBuf;

pub const USAGE: &str = "\
Usage: jotty [OPTIONS] [-e COMMAND [ARGS...]]

Options:
  -c, --config PATH    Use this config file instead of the default
                       ($XDG_CONFIG_HOME/jotty/config.toml)
  -e, --command CMD..  Run CMD with its arguments instead of the shell
  -h, --help           Print this help
  -V, --version        Print the version
";

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Options {
    pub config: Option<PathBuf>,
    /// Program and arguments to run instead of the shell.
    pub command: Option<Vec<String>>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Parsed {
    Run(Options),
    Help,
    Version,
}

pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Parsed, String> {
    let mut options = Options::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(Parsed::Help),
            "-V" | "--version" => return Ok(Parsed::Version),
            "-c" | "--config" => {
                let path = args.next().ok_or("--config needs a path")?;
                options.config = Some(path.into());
            }
            "-e" | "--command" => {
                let command: Vec<String> = args.by_ref().collect();
                if command.is_empty() {
                    return Err("--command needs a program".into());
                }
                options.command = Some(command);
            }
            _ => {
                if let Some(path) = arg.strip_prefix("--config=") {
                    options.config = Some(path.into());
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
        parse(args.iter().map(|s| s.to_string()))
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
}
