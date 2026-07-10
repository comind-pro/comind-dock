mod agents;
mod api;
mod client;
mod config;
mod detect;
mod git;
mod input;
mod logging;
mod platform;
mod proto;
mod runtime;
mod server;
mod state;
mod term;
mod ui;

use std::process::ExitCode;
use std::time::Duration;

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

    /// Run everything in one process (no background server).
    #[arg(long)]
    no_session: bool,

    /// Run the headless session server (internal; started automatically).
    #[arg(long, hide = true)]
    server: bool,

    #[command(subcommand)]
    cmd: Option<Cmd>,
}

/// Automation commands: thin wrappers over the JSON socket API. They print
/// the server's JSON reply verbatim; exit 0 when `ok` is true, 1 otherwise.
#[derive(clap::Subcommand, Debug)]
enum Cmd {
    /// Pane inspection and control.
    Pane {
        #[command(subcommand)]
        sub: PaneCmd,
    },
    /// Agent panes (recognized agent CLIs only).
    Agent {
        #[command(subcommand)]
        sub: AgentCmd,
    },
    /// Block until a pane reaches a condition.
    Wait {
        #[command(subcommand)]
        sub: WaitCmd,
    },
}

#[derive(clap::Subcommand, Debug)]
enum PaneCmd {
    /// List every pane with workspace/tab, program, agent and status.
    List,
    /// Split a pane and spawn a shell or command in the new half.
    Split {
        /// Pane to split (default: the focused pane). Accepts 3 or %3.
        pane: Option<String>,
        #[arg(long, default_value = "right")]
        direction: String,
        /// Command to run instead of the default shell.
        #[arg(long)]
        command: Option<String>,
    },
    /// Write text + Enter to a pane's PTY.
    Run { pane: String, command: String },
    /// Write literal text (no Enter).
    SendText { pane: String, text: String },
    /// Read the last non-empty screen lines.
    Read {
        pane: String,
        #[arg(long)]
        lines: Option<usize>,
    },
    Focus { pane: String },
}

#[derive(clap::Subcommand, Debug)]
enum AgentCmd {
    /// List agent panes only.
    List,
}

#[derive(clap::Subcommand, Debug)]
enum WaitCmd {
    /// Wait until a pane's screen contains the given text. Exit 1 on timeout.
    Output {
        pane: String,
        #[arg(long = "match")]
        pattern: String,
        #[arg(long)]
        timeout: Option<u64>,
    },
    /// Wait until an agent pane reaches a status. Exit 1 on timeout.
    AgentStatus {
        pane: String,
        #[arg(long)]
        status: String,
        #[arg(long)]
        timeout: Option<u64>,
    },
}

/// "%3" or "3" → 3.
fn parse_pane(s: &str) -> Result<u64, String> {
    s.trim_start_matches('%').parse().map_err(|_| format!("bad pane id {s:?}"))
}

/// Send one API request, print the JSON reply. Ok(true) when the server said ok.
fn run_cmd(cmd: Cmd) -> Result<bool, String> {
    use api::Req;
    let req = match cmd {
        Cmd::Pane { sub } => match sub {
            PaneCmd::List => Req::PaneList,
            PaneCmd::Split { pane, direction, command } => Req::Split {
                pane: pane.as_deref().map(parse_pane).transpose()?,
                direction: Some(direction),
                command,
            },
            PaneCmd::Run { pane, command } => Req::Run { pane: parse_pane(&pane)?, command },
            PaneCmd::SendText { pane, text } => Req::SendText { pane: parse_pane(&pane)?, text },
            PaneCmd::Read { pane, lines } => Req::Read { pane: parse_pane(&pane)?, lines },
            PaneCmd::Focus { pane } => Req::Focus { pane: parse_pane(&pane)? },
        },
        Cmd::Agent { sub: AgentCmd::List } => {
            // Agent list = pane list filtered to recognized agents.
            let mut v = api::request(&Req::PaneList).map_err(|e| e.to_string())?;
            if let Some(panes) = v.get_mut("panes").and_then(|p| p.as_array_mut()) {
                panes.retain(|p| !p["agent"].is_null());
            }
            println!("{v}");
            return Ok(v["ok"].as_bool().unwrap_or(false));
        }
        Cmd::Wait { sub } => match sub {
            WaitCmd::Output { pane, pattern, timeout } => {
                Req::WaitOutput { pane: parse_pane(&pane)?, needle: pattern, timeout_ms: timeout }
            }
            WaitCmd::AgentStatus { pane, status, timeout } => {
                Req::WaitAgentStatus { pane: parse_pane(&pane)?, status, timeout_ms: timeout }
            }
        },
    };
    let v = api::request(&req).map_err(|e| e.to_string())?;
    println!("{v}");
    Ok(v["ok"].as_bool().unwrap_or(false))
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

    // Automation subcommands talk to the socket API and are the whole point
    // of CDOCK_ENV panes — never blocked by the nested-launch guard, no UI.
    if let Some(cmd) = cli.cmd {
        return match run_cmd(cmd) {
            Ok(true) => ExitCode::SUCCESS,
            Ok(false) => ExitCode::FAILURE,
            Err(e) => {
                eprintln!("cdock: {e}");
                ExitCode::FAILURE
            }
        };
    }

    // Nested-launch guard: panes get CDOCK_ENV=1; running cdock inside cdock
    // is almost always a mistake.
    if !cli.server && std::env::var_os("CDOCK_ENV").is_some() && !cfg.experimental.allow_nested {
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
    tracing::info!(version = env!("CARGO_PKG_VERSION"), server = cli.server, "cdock starting");
    for w in &warnings {
        tracing::warn!("{w}");
    }

    let result = if cli.server {
        run_server(cfg)
    } else if cli.no_session {
        run_monolithic(cfg)
    } else {
        attach_or_spawn()
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("cdock: {e}");
            tracing::error!(error = %e, "exited with error");
            ExitCode::FAILURE
        }
    }
}

/// Headless daemon: bind the session socket, serve until shutdown.
fn run_server(cfg: config::Config) -> std::io::Result<()> {
    let sock =
        proto::socket_path().ok_or_else(|| std::io::Error::other("cannot determine state dir"))?;
    let api_sock =
        api::socket_path().ok_or_else(|| std::io::Error::other("cannot determine state dir"))?;
    // A stale socket file from a dead server blocks bind; if nobody answers
    // on it, it is safe to remove.
    for s in [&sock, &api_sock] {
        if s.exists() && std::os::unix::net::UnixStream::connect(s).is_err() {
            let _ = std::fs::remove_file(s);
        }
    }
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let listener = tokio::net::UnixListener::bind(&sock)?;
        let api_listener = tokio::net::UnixListener::bind(&api_sock)?;
        let result = server::run(
            cfg,
            Some(listener),
            Some(api_listener),
            Vec::new(),
            server::ServerOpts { exit_when_no_clients: false },
        )
        .await;
        let _ = std::fs::remove_file(&sock);
        let _ = std::fs::remove_file(&api_sock);
        result
    })
}

/// --no-session: server task and thin client in one process over a socketpair.
fn run_monolithic(cfg: config::Config) -> std::io::Result<()> {
    let (client_side, server_side) = std::os::unix::net::UnixStream::pair()?;
    let rt = tokio::runtime::Runtime::new()?;
    let server_thread = std::thread::spawn(move || {
        rt.block_on(async {
            server_side.set_nonblocking(true)?;
            let stream = tokio::net::UnixStream::from_std(server_side)?;
            // ponytail: no API socket in --no-session escape-hatch mode.
            server::run(cfg, None, None, vec![stream], server::ServerOpts { exit_when_no_clients: true })
                .await
        })
    });
    let client_result = client::run(client_side);
    match server_thread.join() {
        Ok(server_result) => client_result.and(server_result),
        Err(_) => Err(std::io::Error::other("server thread panicked")),
    }
}

/// Default flow: attach to a live server, or start one and attach.
fn attach_or_spawn() -> std::io::Result<()> {
    let sock =
        proto::socket_path().ok_or_else(|| std::io::Error::other("cannot determine state dir"))?;

    if let Ok(stream) = std::os::unix::net::UnixStream::connect(&sock) {
        return client::run(stream);
    }

    // No server: start a detached daemon seeded with our cwd, wait for its
    // socket (bounded), then attach (ARCHITECTURE §1 launch flow).
    let exe = std::env::current_exe()?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--server")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // Own process group: the tty's SIGHUP on client death must not reach
    // the server — that is the whole point of the split.
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    cmd.spawn()?;

    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        if let Ok(stream) = std::os::unix::net::UnixStream::connect(&sock) {
            return client::run(stream);
        }
        if std::time::Instant::now() > deadline {
            return Err(std::io::Error::other("server did not start within 15s"));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
