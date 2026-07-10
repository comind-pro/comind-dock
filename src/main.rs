mod agents;
mod config;
mod input;
mod logging;
mod runtime;
mod state;
mod term;
mod ui;

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

    let (cfg, warnings) = config::load(cli.config);
    for w in &warnings {
        eprintln!("cdock: warning: {w}");
    }

    // Nested-launch guard: panes get CDOCK_ENV=1; running cdock inside cdock
    // is almost always a mistake.
    if std::env::var_os("CDOCK_ENV").is_some() && !cfg.experimental.allow_nested {
        eprintln!("cdock: already running inside a cdock pane (CDOCK_ENV is set); refusing to nest");
        eprintln!("cdock: set [experimental].allow_nested = true to override");
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
    for w in &warnings {
        tracing::warn!("{w}");
    }

    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("cdock: failed to start async runtime: {e}");
            return ExitCode::FAILURE;
        }
    };

    // ratatui::init installs panic hooks that restore the host terminal.
    let mouse = cfg.ui.mouse_capture;
    let mut terminal = ratatui::init();
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::EnableBracketedPaste);
    if mouse {
        let _ = crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture);
    }

    let result = rt.block_on(runtime::run(&mut terminal, cfg));

    if mouse {
        let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture);
    }
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableBracketedPaste);
    ratatui::restore();

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("cdock: {e}");
            tracing::error!(error = %e, "runtime exited with error");
            ExitCode::FAILURE
        }
    }
}
