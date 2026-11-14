//! jotty: an opinionated, Wayland-native terminal emulator.

mod app;
mod cli;
mod color;
mod config;
mod emoji;
mod file_marks;
mod font;
mod ime;
mod input;
mod instance;
mod keybinds;
mod links;
mod notify;
mod pty;
mod renderer;
mod search;
mod selection;
mod settings;
mod terminal;
mod terminfo;
mod text;
mod vi;
mod watch;

use std::process::ExitCode;

use cli::Parsed;
use config::Config;

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    let options = match cli::parse(std::env::args_os().skip(1)) {
        Ok(Parsed::Run(options)) => options,
        Ok(Parsed::Config(config, command)) => return settings::run(command, config.as_deref()),
        Ok(Parsed::Help) => {
            print!("{}", cli::USAGE);
            return ExitCode::SUCCESS;
        }
        Ok(Parsed::Version) => {
            println!("jotty {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("jotty: {e}\n\n{}", cli::USAGE);
            return ExitCode::from(2);
        }
    };

    let config = match Config::load(options.config.as_deref()) {
        Ok(config) => config,
        // An explicitly requested config must work; a broken default one
        // should not leave the user without a terminal.
        Err(e) if options.config.is_some() => {
            eprintln!("jotty: {e}");
            return ExitCode::FAILURE;
        }
        Err(e) => {
            log::error!("{e}; using the default configuration");
            Config::default()
        }
    };

    let mut command = options.command;
    let mut server = None;
    if options.single_instance || config.window.single_instance {
        match std::env::current_dir() {
            Ok(cwd) => {
                let request = instance::Request {
                    cwd,
                    env: std::env::vars_os().collect(),
                    command,
                };
                match instance::start(options.config.as_deref(), &request) {
                    instance::Start::HandedOver => return ExitCode::SUCCESS,
                    instance::Start::Failed(message) => {
                        eprintln!("jotty: {message}");
                        return ExitCode::FAILURE;
                    }
                    instance::Start::Serve(s) => server = Some(s),
                    instance::Start::Standalone => {}
                }
                command = request.command;
            }
            Err(e) => log::warn!("no current directory for single instance ({e}); not using it"),
        }
    }

    let spawn = app::Spawn {
        command,
        activation_token: std::env::var("XDG_ACTIVATION_TOKEN")
            .ok()
            .filter(|t| !t.is_empty()),
        ..app::Spawn::default()
    };
    match app::run(config, options.config, spawn, server) {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("jotty: {e}");
            ExitCode::FAILURE
        }
    }
}
