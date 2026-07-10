mod logging;

use std::process::ExitCode;

use clap::Parser;

/// comind-dock: terminal-native runtime and multiplexer for AI coding agents.
#[derive(Parser, Debug)]
#[command(name = "cdock", version, about)]
struct Cli {
    /// Print the annotated default configuration and exit.
    #[arg(long)]
    default_config: bool,

    /// Path to the config file (overrides CDOCK_CONFIG_PATH and the default location).
    #[arg(long, value_name = "PATH")]
    config: Option<std::path::PathBuf>,
}

const DEFAULT_CONFIG: &str = include_str!("default_config.toml");

fn main() -> ExitCode {
    let cli = Cli::parse();

    if cli.default_config {
        print!("{DEFAULT_CONFIG}");
        return ExitCode::SUCCESS;
    }

    // Nested-launch guard: panes get CDOCK_ENV=1; running cdock inside cdock
    // is almost always a mistake. ponytail: [experimental].allow_nested lands with config in M6.
    if std::env::var_os("CDOCK_ENV").is_some() {
        eprintln!("cdock: already running inside a cdock pane (CDOCK_ENV is set); refusing to nest");
        return ExitCode::FAILURE;
    }

    let _log_guard = match logging::init() {
        Ok(guard) => guard,
        Err(e) => {
            eprintln!("cdock: failed to initialize logging: {e}");
            return ExitCode::FAILURE;
        }
    };
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "cdock starting");

    // M1: TUI event loop lands here.
    println!("cdock {}: TUI not implemented yet (M0 skeleton)", env!("CARGO_PKG_VERSION"));
    ExitCode::SUCCESS
}
