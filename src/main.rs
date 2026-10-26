//! jotty: a minimal, Wayland-native, GPU-accelerated terminal emulator.

mod app;
mod cli;
mod config;
mod font;
mod ime;
mod input;
mod pty;
mod renderer;
mod selection;
mod terminal;

use std::process::ExitCode;

use cli::Parsed;
use config::Config;

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    let options = match cli::parse(std::env::args().skip(1)) {
        Ok(Parsed::Run(options)) => options,
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

    match app::run(config, options.command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("jotty: {e}");
            ExitCode::FAILURE
        }
    }
}
