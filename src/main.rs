mod agents;
mod api;
mod client;
mod config;
mod detect;
mod git;
mod input;
mod logging;
mod platform;
mod plugin;
mod profile;
mod proto;
mod runtime;
mod server;
mod state;
mod term;
mod ui;
mod update;

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

    /// Folder-scoped attach: show only workspaces under this folder
    /// (default: current directory); creates one there if none matches.
    #[arg(short = 'f', long, value_name = "PATH", num_args = 0..=1, default_missing_value = ".")]
    folder: Option<std::path::PathBuf>,

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
    /// Install per-agent integration hooks (session identity).
    Integration {
        #[command(subcommand)]
        sub: IntegrationCmd,
    },
    /// Raw API access.
    Api {
        #[command(subcommand)]
        sub: ApiCmd,
    },
    /// Git worktrees as child spaces.
    Worktree {
        #[command(subcommand)]
        sub: WorktreeCmd,
    },
    /// Control the running session server.
    Server {
        #[command(subcommand)]
        sub: ServerCmd,
    },
    /// Agent profiles: role + command + env per directory.
    Profile {
        #[command(subcommand)]
        sub: ProfileCmd,
    },
    /// Skill catalog: named skill sources assignable to profiles.
    Skill {
        #[command(subcommand)]
        sub: SkillCmd,
    },
    /// Plugins: out-of-process actions from linked directories.
    Plugin {
        #[command(subcommand)]
        sub: PluginCmd,
    },
    /// Stream server events as JSON lines (agent-status, output).
    Events {
        /// Only events for this pane.
        #[arg(long)]
        pane: Option<String>,
        /// Comma-separated kinds: agent-status,output (default: all).
        #[arg(long)]
        only: Option<String>,
    },
    /// Workspaces: focus, create, close (list via `api snapshot`).
    Workspace {
        #[command(subcommand)]
        sub: WorkspaceCmd,
    },
    /// Tabs: focus, create, close.
    Tab {
        #[command(subcommand)]
        sub: TabCmd,
    },
    /// Self-update from the latest GitHub release.
    Update {
        /// After replacing the binary, live-handoff the running server
        /// into it (panes survive).
        #[arg(long)]
        handoff: bool,
    },
    /// Internal hook entrypoints (called by agent CLIs, not by hand).
    #[command(hide = true)]
    Hook {
        #[command(subcommand)]
        sub: HookCmd,
    },
}

#[derive(clap::Subcommand, Debug)]
enum IntegrationCmd {
    /// Add the cdock SessionStart hook to the agent's settings.
    Install { agent: String },
}

#[derive(clap::Subcommand, Debug)]
enum ApiCmd {
    /// Full runtime state: workspaces → tabs → panes, one JSON tree.
    Snapshot,
    /// The socket API reference: one example request per command.
    Reference,
}

#[derive(clap::Subcommand, Debug)]
enum ServerCmd {
    /// Re-read detection manifests (bundled + ~/.config/comind-dock/manifests).
    ReloadManifests,
    /// Re-read config, keymap, and theme.
    ReloadConfig,
    /// Replace the running server with the current binary in place —
    /// panes and agents keep running, clients reconnect.
    Handoff,
}

#[derive(clap::Subcommand, Debug)]
enum WorktreeCmd {
    /// Worktrees of a workspace's repo (default: active workspace).
    List {
        #[arg(long)]
        workspace: Option<u64>,
    },
    /// Create branch + worktree, open as a child space.
    Create {
        branch: String,
        #[arg(long)]
        workspace: Option<u64>,
    },
    /// Open an existing worktree by branch as a child space.
    Open {
        branch: String,
        #[arg(long)]
        workspace: Option<u64>,
    },
    /// git worktree remove + close the child space.
    Remove {
        #[arg(long)]
        workspace: u64,
        #[arg(long)]
        force: bool,
    },
}

#[derive(clap::Subcommand, Debug)]
enum HookCmd {
    /// Claude Code SessionStart hook: stdin JSON → report session id.
    ClaudeSession,
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
    /// Stream a pane's raw output to stdout until Ctrl-C.
    Observe { pane: String },
}

#[derive(clap::Subcommand, Debug)]
enum AgentCmd {
    /// List agent panes only.
    List,
    /// Spawn an agent in a new tab (or a split of the focused pane).
    Start {
        /// Command to run, e.g. "claude" or "codex --model o3".
        #[arg(required_unless_present = "profile")]
        command: Option<String>,
        /// Launch by profile (~/.config/comind-dock/agents/<name>/).
        #[arg(long, conflicts_with = "command")]
        profile: Option<String>,
        /// right | down — split instead of a new tab.
        #[arg(long)]
        split: Option<String>,
        #[arg(long)]
        workspace: Option<u64>,
    },
}

#[derive(clap::Subcommand, Debug)]
enum PluginCmd {
    /// Installed plugins with their actions.
    List,
    /// Symlink a local plugin directory (must contain plugin.toml).
    Link { path: String },
    /// Remove a linked plugin (refuses to delete real directories).
    Unlink { id: String },
    /// Run a plugin action in the foreground.
    Action {
        plugin: String,
        action: String,
    },
}

#[derive(clap::Subcommand, Debug)]
enum SkillCmd {
    /// Catalog with sources and descriptions.
    List,
    /// Register a skill directory (must contain SKILL.md).
    Add {
        name: String,
        #[arg(long)]
        source: String,
        #[arg(long, default_value = "")]
        description: String,
    },
    /// Unregister (profiles referencing it just skip it with a warning).
    Remove { name: String },
}

#[derive(clap::Subcommand, Debug)]
enum WorkspaceCmd {
    Focus { workspace: u64 },
    /// New workspace ([terminal].new_cwd unless --cwd).
    Create {
        #[arg(long)]
        cwd: Option<String>,
    },
    /// Kill every pane in the workspace.
    Close { workspace: u64 },
}

#[derive(clap::Subcommand, Debug)]
enum TabCmd {
    Focus { tab: u64 },
    Create {
        #[arg(long)]
        workspace: Option<u64>,
    },
    Close { tab: u64 },
}

#[derive(clap::Subcommand, Debug)]
enum ProfileCmd {
    /// All profiles.
    List,
    /// Resolved profile: the exact command and env a launch would use.
    Show { name: String },
    /// Scaffold a new profile directory (optionally copying another).
    New {
        name: String,
        #[arg(long)]
        from: Option<String>,
    },
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

/// Merge the cdock SessionStart hook into EVERY Claude profile's
/// settings.json (~/.claude and ~/.claude-*): a profile without the hook
/// never reports its session id, and its panes silently lose their
/// conversation on restore. Idempotent; backs each file up first. The
/// command guards on CDOCK_PANE_ID, so the hook is inert outside panes.
fn install_claude_hook() -> Result<bool, String> {
    let home = std::env::var("HOME").map_err(|_| "HOME unset".to_string())?;
    let home = std::path::PathBuf::from(home);
    let mut profiles: Vec<std::path::PathBuf> = std::fs::read_dir(&home)
        .map_err(|e| e.to_string())?
        .flatten()
        .filter(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            (n == ".claude" || n.starts_with(".claude-")) && e.path().is_dir()
        })
        .map(|e| e.path())
        .collect();
    if profiles.is_empty() {
        profiles.push(home.join(".claude"));
    }
    profiles.sort();
    for dir in profiles {
        install_hook_into(&dir)?;
        install_skill_into(&dir)?;
    }
    println!("restart running claude panes to activate the hook");
    Ok(true)
}

fn install_hook_into(profile_dir: &std::path::Path) -> Result<(), String> {
    const MARKER: &str = "hook claude-session";
    let path = profile_dir.join("settings.json");
    let mut root: serde_json::Value = match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|e| format!("{} is not valid JSON ({e}); not touching it", path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
        Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
    };

    let starts = root
        .as_object_mut()
        .ok_or("settings.json root is not an object")?
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or("settings.json \"hooks\" is not an object")?
        .entry("SessionStart")
        .or_insert_with(|| serde_json::json!([]));
    let arr = starts.as_array_mut().ok_or("\"SessionStart\" is not an array")?;
    if arr.iter().any(|e| e.to_string().contains(MARKER)) {
        println!("{}: hook already installed", path.display());
        return Ok(());
    }
    arr.push(serde_json::json!({
        "hooks": [{
            "type": "command",
            "command": "[ -z \"$CDOCK_PANE_ID\" ] || \"$CDOCK_BIN\" hook claude-session"
        }]
    }));

    if path.exists() {
        let _ = std::fs::copy(&path, path.with_extension("json.bak"));
    } else if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let pretty = serde_json::to_string_pretty(&root).map_err(|e| e.to_string())?;
    std::fs::write(&path, pretty).map_err(|e| e.to_string())?;
    println!("installed SessionStart hook into {} (backup: .json.bak)", path.display());
    Ok(())
}

/// Materialize the cdock skill in a profile so claude agents inside panes
/// know how to drive the runtime. Overwrites — cdock's copy is canonical.
fn install_skill_into(profile_dir: &std::path::Path) -> Result<(), String> {
    let dir = profile_dir.join("skills/cdock");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("SKILL.md");
    std::fs::write(&path, include_str!("integration/cdock_skill.md"))
        .map_err(|e| e.to_string())?;
    println!("installed agent skill at {}", path.display());
    Ok(())
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
            PaneCmd::Observe { pane } => {
                let pane = parse_pane(&pane)?;
                let spec =
                    api::SubSpec { events: vec!["output".to_string()], pane: Some(pane) };
                api::subscribe(&spec, |v| {
                    if let Some(data) = v["data"].as_str() {
                        print!("{data}");
                        let _ = std::io::Write::flush(&mut std::io::stdout());
                    }
                })
                .map_err(|e| e.to_string())?;
                return Ok(true);
            }
        },
        Cmd::Agent { sub: AgentCmd::Start { command, profile, split, workspace } } => {
            let (command, env) = match profile {
                Some(name) => profile::load(&name)?.resolve(),
                None => (command.expect("clap: command or profile"), Vec::new()),
            };
            Req::AgentStart { command, split, workspace, env }
        }
        Cmd::Plugin { sub } => {
            return match sub {
                PluginCmd::List => {
                    for p in plugin::list() {
                        println!("{}\t{}\t{}", p.manifest.id, p.manifest.name, p.dir.display());
                        for a in &p.manifest.actions {
                            println!("  {}:{}\t{}", p.manifest.id, a.id, a.label);
                        }
                    }
                    Ok(true)
                }
                PluginCmd::Link { path } => {
                    let id = plugin::link(&path)?;
                    println!("linked {id}");
                    Ok(true)
                }
                PluginCmd::Unlink { id } => {
                    plugin::unlink(&id)?;
                    println!("unlinked {id}");
                    Ok(true)
                }
                PluginCmd::Action { plugin, action } => plugin::invoke(&plugin, &action),
            };
        }
        Cmd::Skill { sub } => {
            return match sub {
                SkillCmd::List => {
                    for (name, e) in profile::skill_catalog() {
                        println!("{name}\t{}\t{}", e.source, e.description);
                    }
                    Ok(true)
                }
                SkillCmd::Add { name, source, description } => {
                    profile::skill_add(&name, &source, &description)?;
                    println!("added {name}");
                    Ok(true)
                }
                SkillCmd::Remove { name } => {
                    profile::skill_remove(&name)?;
                    println!("removed {name}");
                    Ok(true)
                }
            };
        }
        Cmd::Events { pane, only } => {
            let spec = api::SubSpec {
                events: only
                    .map(|o| o.split(',').map(|s| s.trim().to_string()).collect())
                    .unwrap_or_default(),
                pane: pane.as_deref().map(parse_pane).transpose()?,
            };
            api::subscribe(&spec, |v| println!("{v}")).map_err(|e| e.to_string())?;
            return Ok(true);
        }
        Cmd::Workspace { sub } => match sub {
            WorkspaceCmd::Focus { workspace } => Req::WorkspaceFocus { workspace },
            WorkspaceCmd::Create { cwd } => Req::WorkspaceCreate { cwd },
            WorkspaceCmd::Close { workspace } => Req::WorkspaceClose { workspace },
        },
        Cmd::Tab { sub } => match sub {
            TabCmd::Focus { tab } => Req::TabFocus { tab },
            TabCmd::Create { workspace } => Req::TabCreate { workspace },
            TabCmd::Close { tab } => Req::TabClose { tab },
        },
        Cmd::Profile { sub } => {
            return match sub {
                ProfileCmd::List => {
                    for name in profile::list() {
                        println!("{name}");
                    }
                    Ok(true)
                }
                ProfileCmd::Show { name } => {
                    let p = profile::load(&name)?;
                    let (command, env) = p.resolve();
                    let files: Vec<String> = ["profile.toml", "agent.md", "memory.md"]
                        .iter()
                        .filter(|f| p.dir.join(f).exists())
                        .map(|f| f.to_string())
                        .collect();
                    println!(
                        "{}",
                        serde_json::json!({
                            "name": p.name,
                            "dir": p.dir.display().to_string(),
                            "files": files,
                            "command": command,
                            "env": env.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>(),
                        })
                    );
                    Ok(true)
                }
                ProfileCmd::New { name, from } => {
                    let dir = profile::scaffold(&name, from.as_deref())?;
                    println!("created {} — edit profile.toml and agent.md", dir.display());
                    Ok(true)
                }
            };
        }
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
        Cmd::Api { sub: ApiCmd::Snapshot } => Req::Snapshot,
        Cmd::Api { sub: ApiCmd::Reference } => {
            println!("{}", api::REFERENCE.trim());
            return Ok(true);
        }
        Cmd::Server { sub } => match sub {
            ServerCmd::ReloadManifests => Req::ReloadManifests,
            ServerCmd::ReloadConfig => Req::ReloadConfig,
            ServerCmd::Handoff => Req::Handoff,
        },
        Cmd::Worktree { sub } => match sub {
            WorktreeCmd::List { workspace } => Req::WorktreeList { workspace },
            WorktreeCmd::Create { branch, workspace } => {
                Req::WorktreeCreate { workspace, branch }
            }
            WorktreeCmd::Open { branch, workspace } => Req::WorktreeOpen { workspace, branch },
            WorktreeCmd::Remove { workspace, force } => {
                Req::WorktreeRemove { workspace, force }
            }
        },
        Cmd::Integration { sub: IntegrationCmd::Install { agent } } => {
            return match agent.as_str() {
                "claude" => install_claude_hook(),
                other => Err(format!("no integration for {other:?} yet (only claude)")),
            };
        }
        Cmd::Update { handoff } => {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            println!("current: v{}", update::CURRENT);
            return match update::self_update(&exe)? {
                Some(tag) => {
                    println!("updated to {tag} at {}", exe.display());
                    if handoff {
                        println!("handing the running server off to the new binary…");
                        match api::request(&Req::Handoff) {
                            Ok(v) if v["ok"] == true => println!("handoff requested — reconnecting"),
                            Ok(v) => println!("handoff refused: {v}"),
                            Err(e) => println!("no running server to hand off ({e})"),
                        }
                    } else {
                        println!("apply to the running session with: cdock server handoff");
                    }
                    Ok(true)
                }
                None => {
                    println!("already up to date");
                    Ok(true)
                }
            };
        }
        Cmd::Hook { sub: HookCmd::ClaudeSession } => {
            // Quietly a no-op outside a cdock pane — the hook is installed
            // globally but only means something here.
            let Some(pane) = std::env::var("CDOCK_PANE_ID").ok().and_then(|p| parse_pane(&p).ok())
            else {
                return Ok(true);
            };
            let mut input = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut input)
                .map_err(|e| e.to_string())?;
            let v: serde_json::Value =
                serde_json::from_str(&input).map_err(|e| format!("bad hook input: {e}"))?;
            let Some(session_id) = v["session_id"].as_str() else {
                return Ok(true); // nothing to report
            };
            api::request_with_timeout(
                &Req::ReportAgentSession { pane, session_id: session_id.to_string() },
                Duration::from_secs(3),
            )
            .map_err(|e| e.to_string())?;
            return Ok(true);
        }
    };
    let v = api::request(&req).map_err(|e| e.to_string())?;
    println!("{v}");
    Ok(v["ok"].as_bool().unwrap_or(false))
}

use config::DEFAULT_CONFIG;

fn main() -> ExitCode {
    let _ = startup_exe(); // capture before any self-update can rename us
    // Invoked as cdock-dev → pin the dev namespace into the environment so
    // the auto-spawned server and every pane child inherit it (current_exe
    // resolves the symlink, argv[0] does not survive respawns).
    if logging::dev_mode() {
        // Safety: single-threaded this early in main.
        unsafe { std::env::set_var("CDOCK_DEV", "1") };
    }
    let cli = Cli::parse();

    if cli.default_config {
        print!("{DEFAULT_CONFIG}");
        return ExitCode::SUCCESS;
    }

    let (cfg, warnings) = config::load(cli.config);
    for w in &warnings {
        eprintln!("cdock: warning: {w}");
    }

    // Scope folder resolves client-side — the server has its own cwd.
    let folder = match cli.folder {
        Some(p) => match std::fs::canonicalize(&p) {
            Ok(p) => Some(p),
            Err(e) => {
                eprintln!("cdock: bad folder {}: {e}", p.display());
                return ExitCode::FAILURE;
            }
        },
        None => None,
    };

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
    // is almost always a mistake — EXCEPT the dev binary, whose whole point
    // is being developed and launched from inside the production dock (its
    // namespace is isolated, so nesting is harmless).
    if !cli.server
        && !logging::dev_mode()
        && std::env::var_os("CDOCK_ENV").is_some()
        && !cfg.experimental.allow_nested
    {
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
        run_monolithic(cfg, folder)
    } else {
        attach_or_spawn(folder)
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

/// Headless daemon: bind the session socket, serve until shutdown. A live
/// handoff replaces this process image via exec — same pid, panes keep
/// running on their inherited master fds.
fn run_server(cfg: config::Config) -> std::io::Result<()> {
    let sock =
        proto::socket_path().ok_or_else(|| std::io::Error::other("cannot determine state dir"))?;
    let api_sock =
        api::socket_path().ok_or_else(|| std::io::Error::other("cannot determine state dir"))?;
    // A stale socket file from a dead server blocks bind; if nobody answers
    // on it, it is safe to remove. After exec-handoff OUR OWN sockets look
    // stale (fds closed at exec) — same cleanup covers that.
    for s in [&sock, &api_sock] {
        if s.exists() && std::os::unix::net::UnixStream::connect(s).is_err() {
            let _ = std::fs::remove_file(s);
        }
    }
    let handoff = take_handoff();
    // Session snapshot as of this boot — one restore point if a bad attach
    // or runaway automation mangles the live session and autosave persists
    // it. NOT on handoff: that would overwrite the rollback point with the
    // very state the user may want to roll back from.
    if handoff.is_none()
        && let Some(snap) = crate::state::snapshot::path()
        && snap.exists()
    {
        let _ = std::fs::copy(&snap, snap.with_extension("json.boot-bak"));
    }
    let rt = tokio::runtime::Runtime::new()?;
    let heir = handoff.is_some();
    let outcome = rt.block_on(async {
        // The heir of a live handoff must not die on a transient bind race
        // with a rival auto-spawned server — losing here kills every pane.
        let bind = |path: std::path::PathBuf| async move {
            for _ in 0..10 {
                if let Ok(l) = tokio::net::UnixListener::bind(&path) {
                    return Ok(l);
                }
                if std::os::unix::net::UnixStream::connect(&path).is_err() {
                    let _ = std::fs::remove_file(&path);
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            tokio::net::UnixListener::bind(&path)
        };
        let (listener, api_listener) = if heir {
            (bind(sock.clone()).await?, bind(api_sock.clone()).await?)
        } else {
            (
                tokio::net::UnixListener::bind(&sock)?,
                tokio::net::UnixListener::bind(&api_sock)?,
            )
        };
        let result = server::run(
            cfg,
            Some(listener),
            Some(api_listener),
            Vec::new(),
            handoff,
            server::ServerOpts { exit_when_no_clients: false },
        )
        .await;
        let _ = std::fs::remove_file(&sock);
        let _ = std::fs::remove_file(&api_sock);
        result
    })?;
    drop(rt); // no tokio left behind the exec
    match outcome {
        server::RunOutcome::Exit => Ok(()),
        server::RunOutcome::Handoff(h) => {
            let path = handoff_path()
                .ok_or_else(|| std::io::Error::other("cannot determine state dir"))?;
            std::fs::write(&path, serde_json::to_vec(&*h)?)?;
            tracing::info!("exec-ing replacement server");
            // exec only returns on failure. Use the path captured at startup:
            // after a self-update renamed over us, Linux current_exe() reads
            // "/…/cdock (deleted)" and the exec would kill the session.
            let err = std::os::unix::process::CommandExt::exec(
                std::process::Command::new(startup_exe()).arg("--server"),
            );
            let _ = std::fs::remove_file(&path);
            Err(err)
        }
    }
}

/// The executable path as it was at process start (see exec-handoff note).
fn startup_exe() -> &'static std::path::Path {
    static EXE: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    EXE.get_or_init(|| {
        let p = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("cdock"));
        // Strip Linux's " (deleted)" suffix if we raced a self-update.
        match p.to_str().and_then(|s| s.strip_suffix(" (deleted)")) {
            Some(clean) => std::path::PathBuf::from(clean),
            None => p,
        }
    })
}

fn handoff_path() -> Option<std::path::PathBuf> {
    logging::state_dir().map(|d| d.join("handoff.json"))
}

/// Handoff state left by the previous process image, if it was really ours:
/// the pid guard rejects stale files from crashed handoffs, whose fds would
/// be garbage.
fn take_handoff() -> Option<runtime::Handoff> {
    let path = handoff_path()?;
    let text = std::fs::read_to_string(&path).ok()?;
    match serde_json::from_str::<runtime::Handoff>(&text) {
        Ok(h) if h.pid == std::process::id() => {
            let _ = std::fs::remove_file(&path);
            tracing::info!("resuming from live handoff");
            Some(h)
        }
        // NOT ours — a concurrently spawned server must leave the heir's
        // handoff file alone (deleting it here dropped every live pane).
        Ok(h) => {
            tracing::warn!(their_pid = h.pid, "ignoring another process's handoff file");
            None
        }
        Err(e) => {
            let _ = std::fs::remove_file(&path);
            tracing::warn!(error = %e, "removed corrupt handoff file");
            None
        }
    }
}

/// --no-session: server task and thin client in one process over a socketpair.
fn run_monolithic(cfg: config::Config, folder: Option<std::path::PathBuf>) -> std::io::Result<()> {
    let (client_side, server_side) = std::os::unix::net::UnixStream::pair()?;
    let rt = tokio::runtime::Runtime::new()?;
    let server_thread = std::thread::spawn(move || {
        rt.block_on(async {
            server_side.set_nonblocking(true)?;
            let stream = tokio::net::UnixStream::from_std(server_side)?;
            // ponytail: no API socket / handoff in --no-session mode.
            server::run(
                cfg,
                None,
                None,
                vec![stream],
                None,
                server::ServerOpts { exit_when_no_clients: true },
            )
            .await
            .map(|_| ())
        })
    });
    let client_result = client::run(client_side, folder);
    match server_thread.join() {
        Ok(server_result) => client_result.and(server_result),
        Err(_) => Err(std::io::Error::other("server thread panicked")),
    }
}

/// Default flow: attach to a live server, or start one and attach.
fn attach_or_spawn(folder: Option<std::path::PathBuf>) -> std::io::Result<()> {
    let sock =
        proto::socket_path().ok_or_else(|| std::io::Error::other("cannot determine state dir"))?;

    if let Ok(stream) = std::os::unix::net::UnixStream::connect(&sock) {
        return client::run(stream, folder);
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
            return client::run(stream, folder);
        }
        if std::time::Instant::now() > deadline {
            return Err(std::io::Error::other("server did not start within 15s"));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
