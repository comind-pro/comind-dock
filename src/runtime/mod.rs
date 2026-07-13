pub mod event;

use std::collections::HashMap;
use std::io;
use std::time::Duration;

use alacritty_terminal::event::Event as TermEvent;
use ratatui::layout::Rect;
use tokio::sync::mpsc;

use crate::config::keys::Keymap;
use crate::config::theme::Theme;
use crate::config::{CommandKind, Config, CustomCommand, ShellMode};
use crate::input;
use crate::state::ids::PaneId;
use crate::state::layout::{Dir, Divider};
use crate::state::{AppState, CloseOutcome};
use crate::term::emulator::Emulator;
use crate::term::pty::{self, Pty};
use event::{AppEvent, PtyData};

/// Max PTY bytes fed to emulators between renders, so `cat bigfile`
/// cannot starve input handling and the render tick.
const DRAIN_BUDGET: usize = 256 * 1024;

/// Whether a rename should also rename the agent's conversation. Claude is
/// the only CLI with a `/rename` command; a busy agent would take the text
/// as a queued message instead of a command, so we leave it alone. An empty
/// name only clears our own override — there is nothing to un-name.
fn should_rename_conversation(
    agent: Option<&str>,
    status: crate::detect::Status,
    name: &str,
) -> bool {
    use crate::detect::Status;
    agent == Some("claude")
        && !name.is_empty()
        && !matches!(status, Status::Working | Status::Blocked)
}

pub struct PaneRuntime {
    pub emu: Emulator,
    pub pty: Pty,
    /// Program shown in the agents sidebar (command or shell basename).
    pub program: String,
    /// Last PTY output — the routine "working" signal and detection fallback.
    pub last_output: std::time::Instant,
    /// Recognized agent CLI in this pane (spawn command, title, or a child
    /// process of the shell) — refreshed by the agent poll.
    pub agent: Option<&'static str>,
    /// Pid of the agent process itself (may be a shell's child).
    pub agent_pid: Option<u32>,
    /// CLAUDE_CONFIG_DIR of the agent process — which profile it runs as.
    pub agent_config_dir: Option<String>,
    /// Exe path of the agent process — resume by absolute path survives a
    /// server started with a PATH that can't find the launcher.
    pub agent_bin: Option<String>,
    /// Hook-reported state override (report-agent API): wins over screen
    /// detection until it expires or the reporter clears it.
    pub reported: Option<Reported>,
    /// An agent event the user has not looked at yet (finished / blocked).
    /// The status itself decays back to idle within seconds — this does not:
    /// it survives until the pane is actually on someone's screen, so the
    /// sidebar can say WHICH agent the sound was about.
    pub unseen: Option<NoticeKind>,
    /// When the current agent process first appeared — the `since` cutoff
    /// for codex/opencode session-file discovery (they have no hooks).
    pub agent_since: Option<std::time::SystemTime>,
    /// Attached behavior profile ident ("global:<name>" | "ws:<name>") —
    /// injected into the live session, re-applied as system prompt on resume.
    pub behavior: Option<String>,
    /// Consecutive polls with no agent — releases the session mapping.
    agent_gone_polls: u8,
    /// Detection-engine result for agent panes.
    pub status: crate::detect::Status,
    /// Last status the UI showed — drives redraws and notifications.
    pub last_shown: crate::detect::Status,
    /// When `last_shown` last changed.
    pub status_since: std::time::Instant,
    last_size: (u16, u16),
}

impl PaneRuntime {
    pub fn working(&self) -> bool {
        self.last_output.elapsed() < Duration::from_secs(3)
    }

    /// Status with the activity fallback when no manifest rule matched.
    /// A live hook report (report-agent API) outranks screen detection.
    pub fn effective_status(&self) -> crate::detect::Status {
        if let Some(r) = &self.reported
            && r.until > std::time::Instant::now()
        {
            return r.status;
        }
        match self.status {
            crate::detect::Status::Unknown => {
                if self.working() {
                    crate::detect::Status::Working
                } else {
                    crate::detect::Status::Idle
                }
            }
            s => s,
        }
    }

    /// Reporter-supplied status label ("running tests"), while the report lives.
    pub fn reported_label(&self) -> Option<&str> {
        self.reported
            .as_ref()
            .filter(|r| r.until > std::time::Instant::now())
            .and_then(|r| r.label.as_deref())
    }
}

/// An in-progress mouse drag gesture.
#[derive(Debug, Clone, Copy)]
pub enum MouseDrag {
    Divider { before: PaneId, after: PaneId, dir: Dir, extent: u16, last_pos: u16 },
    Select { pane: PaneId },
}

pub struct Runtime {
    pub state: AppState,
    /// false for --no-session (mono) servers: an ephemeral session that
    /// exits with its client never resumes, so persisting it only litters
    /// the state dir with snapshots and screen tails.
    pub persist: bool,
    pub panes: HashMap<PaneId, PaneRuntime>,
    pub cfg: Config,
    pub keymap: Keymap,
    pub theme: Theme,
    /// OSC window titles reported by pane applications.
    pub titles: HashMap<PaneId, String>,
    /// Git branch per workspace (polled with cwd tracking).
    pub branches: HashMap<crate::state::ids::WorkspaceId, String>,
    /// Agent conversation ids reported by SessionStart integration hooks —
    /// lets restore resume each pane's own conversation.
    pub agent_sessions: HashMap<PaneId, String>,
    /// In-app notification toasts (top-right overlay, click jumps to pane).
    pub toasts: Vec<Toast>,
    /// A newer release tag found by the background check ("update ready").
    pub update_available: Option<String>,
    /// The last computed view — neighbor focus and mouse hit testing.
    pub last_view: Option<crate::ui::view::View>,
    /// Sidebar scroll offset in rows (mouse wheel over the sidebar).
    pub sidebar_scroll: u16,
    pub drag: Option<MouseDrag>,
    pub last_click: Option<(std::time::Instant, u16, u16)>,
    tx: mpsc::UnboundedSender<AppEvent>,
    data_tx: mpsc::Sender<PtyData>,
    /// Bytes for the host terminal(s) outside the frame pipeline (OSC 52).
    raw_out: mpsc::UnboundedSender<Vec<u8>>,
    dirty: bool,
}

impl Runtime {
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    /// Send raw bytes to every attached host terminal (e.g. OSC 52 copy).
    pub fn host_write(&self, bytes: Vec<u8>) {
        let _ = self.raw_out.send(bytes);
    }

    /// Encode a key for the focused pane's modes and write it to its PTY.
    pub fn send_key(&mut self, key: &crossterm::event::KeyEvent) {
        let focused = self.state.focused_pane();
        if let Some(p) = self.panes.get_mut(&focused)
            && let Some(bytes) = input::encode::encode_key(key, p.emu.term.mode())
        {
            p.pty.write(&bytes);
        }
    }

    /// Kill a pane's child; PtyExit drives the state change (single close path).
    pub fn kill_pane(&mut self, pane: PaneId) {
        if let Some(p) = self.panes.get_mut(&pane) {
            p.pty.kill();
        }
    }

    pub fn split_focused(&mut self, dir: Dir, before: bool, area: Rect) -> io::Result<()> {
        let pane = self.state.split_focused(dir, before);
        // Provisional size; compute_view corrects it before the next frame.
        let r = self.spawn_pane(pane, area.width.max(2) / 2, area.height.max(2) / 2);
        if r.is_err() {
            self.state.close_pane(pane); // a leaf without a PTY is unclosable
        }
        r
    }

    pub fn spawn_pane(&mut self, pane: PaneId, cols: u16, rows: u16) -> io::Result<()> {
        self.spawn_pane_cmd(pane, cols, rows, None)
    }

    pub fn spawn_pane_cmd(
        &mut self,
        pane: PaneId,
        cols: u16,
        rows: u16,
        command: Option<String>,
    ) -> io::Result<()> {
        self.spawn_pane_env(pane, cols, rows, command, Vec::new())
    }

    pub fn spawn_pane_env(
        &mut self,
        pane: PaneId,
        cols: u16,
        rows: u16,
        command: Option<String>,
        env: Vec<(String, String)>,
    ) -> io::Result<()> {
        self.spawn_pane_full(pane, cols, rows, command, env, None)
    }

    pub fn spawn_pane_full(
        &mut self,
        pane: PaneId,
        cols: u16,
        rows: u16,
        command: Option<String>,
        env: Vec<(String, String)>,
        cwd: Option<std::path::PathBuf>,
    ) -> io::Result<()> {
        let scrollback = self.cfg.advanced.scrollback_lines();
        let emu = Emulator::new(cols, rows, pane, self.tx.clone(), scrollback);
        let mut opts = self.spawn_opts(pane, command);
        opts.env = env;
        if let Some(cwd) = cwd.filter(|c| c.is_dir()) {
            opts.cwd = cwd;
        }
        let program = opts
            .command
            .as_deref()
            .unwrap_or(&opts.shell)
            .split_whitespace()
            .next()
            .map(|w| w.rsplit('/').next().unwrap_or(w).to_string())
            .unwrap_or_else(|| "shell".to_string());
        let pty = pty::spawn_shell(pane, cols, rows, self.tx.clone(), self.data_tx.clone(), &opts)?;
        self.panes.insert(
            pane,
            PaneRuntime {
                emu,
                pty,
                agent: crate::agents::detect("", &program),
                agent_pid: None,
                agent_config_dir: None,
                agent_bin: None,
                reported: None,
                unseen: None,
                agent_since: None,
                behavior: None,
                agent_gone_polls: 0,
                program,
                last_output: std::time::Instant::now(),
                status: crate::detect::Status::Unknown,
                last_shown: crate::detect::Status::Unknown,
                status_since: std::time::Instant::now(),
                last_size: (cols, rows),
            },
        );
        Ok(())
    }

    /// Rename an agent session: ONE name for the sidebar, the tab bar and
    /// the "+ continue" picker. An empty name clears the override (back to
    /// the agent's own title). Persisted by conversation id, so it outlives
    /// the pane. The pane's tab drops any custom name of its own — the
    /// label follows this one, and two names for the same thing is the bug
    /// we are fixing.
    pub fn rename_pane(&mut self, pane: PaneId, name: String) {
        let name = name.trim().to_string();
        self.state.rename_pane(pane, name.clone());
        if let Some(ident) = self.agent_sessions.get(&pane) {
            crate::agents::set_session_name(ident, Some(name.as_str()).filter(|n| !n.is_empty()));
        }
        if let Some(tab) = self
            .state
            .locate_pane(pane)
            .and_then(|(wi, ti)| self.state.workspaces.get(wi).and_then(|w| w.tabs.get(ti)))
            .map(|t| t.id)
        {
            self.state.reset_tab_name(tab);
        }
        self.dirty = true;
        self.rename_conversation(pane, &name);
    }

    /// Carry the name into the agent's OWN conversation, so `/resume` lists
    /// it under the same label the tab shows. Typed like a human types it —
    /// bracketed paste would land as a "[Pasted text]" chunk, not a command.
    fn rename_conversation(&mut self, pane: PaneId, name: &str) {
        let Some(p) = self.panes.get_mut(&pane) else { return };
        let agent = p.agent;
        let status = p.effective_status();
        if should_rename_conversation(agent, status, name) {
            p.pty.write(format!("/rename {name}\r").as_bytes());
        } else if agent == Some("claude") && !name.is_empty() {
            self.add_plain_toast("agent busy — conversation not renamed".into(), 6);
        }
    }

    /// $EDITOR on a path, in a fresh tab.
    pub fn open_in_editor(&mut self, path: &std::path::Path, area: Rect) -> io::Result<()> {
        let editor = self.cfg.terminal.editor_cmd();
        let pane = self.state.new_tab();
        self.spawn_pane_cmd(
            pane,
            area.width,
            area.height,
            Some(format!("{editor} {}", path.display())),
        )
    }

    /// Inject a behavior profile into a pane's LIVE session: the staged
    /// prompt is pasted (bracketed when the app asked) and submitted. The
    /// ident is remembered so a cold restore resumes with the same role as
    /// system prompt.
    pub fn apply_behavior(&mut self, pane: PaneId, ident: Option<String>) -> Result<(), String> {
        let Some(ident_ref) = ident.as_deref() else {
            if let Some(p) = self.panes.get_mut(&pane) {
                p.behavior = None;
                self.dirty = true;
            }
            return Ok(());
        };
        let ws_cwd = self
            .state
            .locate_pane(pane)
            .and_then(|(wi, _)| self.state.workspaces.get(wi))
            .map(|w| w.cwd.clone())
            .ok_or("pane not in any workspace")?;
        let profile = crate::profile::load_behavior(ident_ref, &ws_cwd)?;
        let text = profile
            .prompt_text_with(Some(&ws_cwd))
            .ok_or("behavior profile has an empty prompt")?;
        let p = self.panes.get_mut(&pane).ok_or("no such pane")?;
        use alacritty_terminal::term::TermMode;
        if p.emu.term.mode().contains(TermMode::BRACKETED_PASTE) {
            p.pty.write(b"\x1b[200~");
            p.pty.write(text.as_bytes());
            p.pty.write(b"\x1b[201~");
        } else {
            p.pty.write(text.as_bytes());
        }
        p.pty.write(b"\r");
        p.behavior = ident;
        self.dirty = true;
        Ok(())
    }

    fn spawn_opts(&self, pane: PaneId, command: Option<String>) -> pty::SpawnOpts {
        let t = &self.cfg.terminal;
        // Command panes run under /bin/sh: our generated command lines
        // (hold_on_failure, resume) are POSIX — a fish/nushell $SHELL would
        // reject them outright. Interactive panes keep the user's shell.
        let shell = if command.is_some() {
            "/bin/sh".to_string()
        } else if t.default_shell.is_empty() {
            std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
        } else {
            t.default_shell.clone()
        };
        let login = match t.shell_mode {
            ShellMode::Auto => cfg!(target_os = "macos"),
            ShellMode::Login => true,
            ShellMode::NonLogin => false,
        };
        // Panes spawn in their OWNING workspace's folder (the pane is in
        // the tree before spawn) — background API spawns into another space
        // must not inherit the user's current folder, or the anchor poll
        // rewrites the new space to the active one's cwd within seconds.
        // With [terminal].new_cwd = "follow", follow that tab's focused
        // pane. A vanished folder (deleted worktree in a snapshot) must not
        // brick the spawn — and with it the whole restore: fall back home.
        let (wi, ti) = self.state.locate_pane(pane).unwrap_or((self.state.active_workspace, 0));
        let ws = self.state.workspaces.get(wi).unwrap_or_else(|| self.state.active_workspace());
        let tab = ws.tabs.get(ti).unwrap_or_else(|| ws.active_tab());
        let follow = (t.new_cwd == "follow")
            .then(|| {
                self.panes
                    .get(&tab.focused_pane)
                    .and_then(|p| p.pty.child_pid)
                    .and_then(crate::platform::process_cwd)
                    .filter(|c| c.is_dir())
            })
            .flatten();
        let cwd = if let Some(f) = follow {
            f
        } else if ws.cwd.is_dir() {
            ws.cwd.clone()
        } else {
            std::env::var_os("HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| std::path::PathBuf::from("/"))
        };
        pty::SpawnOpts {
            shell,
            login,
            cwd,
            command,
            env: Vec::new(),
            tab_id: tab.id.to_string(),
            workspace_id: ws.id.to_string(),
        }
    }

    /// Run the detection engine over agent panes (bottom buffer + title).
    /// Called every ~500ms by the server; cheap — a few strings per agent.
    /// Run the detection engine over agent panes (bottom buffer + title).
    /// Called every ~500ms by the server. Marks dirty whenever the DISPLAYED
    /// status changes (including activity-fallback flips, so the sidebar is
    /// reactive without input) and returns the transitions worth notifying.
    pub fn poll_agent_status(
        &mut self,
        manifests: &[crate::detect::Manifest],
    ) -> (Vec<Notice>, Vec<StatusChange>) {
        use crate::detect::Status;
        let mut notices = Vec::new();
        let mut changes = Vec::new();
        let ids: Vec<PaneId> = self.panes.keys().copied().collect();
        for id in ids {
            let title = self.titles.get(&id).cloned().unwrap_or_default();
            let Some(p) = self.panes.get(&id) else { continue };
            // Live processes are the source of truth: the pane's child
            // itself (direct spawn) or any of its children (agent typed into
            // a shell). Exe-path components are matched exactly — no title
            // or spawn-command matching, which pinned the SHELL's pid as the
            // agent (wrong profile) and kept "phantom agents" alive after
            // the process died behind a stale title.
            let mut agent_pid = None;
            let mut agent_bin = None;
            let agent = p
                .pty
                .child_pid
                .and_then(|child| {
                    crate::platform::process_ident(child)
                        .and_then(|ident| {
                            crate::agents::detect_process(&ident).inspect(|_| {
                                agent_pid = Some(child);
                                agent_bin = Some(ident.clone());
                            })
                        })
                        .or_else(|| {
                            crate::platform::child_process_idents(child).iter().find_map(
                                |(pid, ident)| {
                                    crate::agents::detect_process(ident).inspect(|_| {
                                        agent_pid = Some(*pid);
                                        agent_bin = Some(ident.clone());
                                    })
                                },
                            )
                        })
                })
                // Interpreter-hosted installs (npm/bun claude runs as
                // "node") have no agent-named exe path — the OSC title is
                // the only signal. Identity only, and ONLY while the shell
                // still hosts a child: shells never reset the OSC title, so
                // an unguarded fallback would resurrect phantom agents (and
                // pin their session ids) forever after the agent exits.
                .or_else(|| {
                    let hosts_child = p
                        .pty
                        .child_pid
                        .is_some_and(|c| !crate::platform::child_process_idents(c).is_empty());
                    hosts_child.then(|| crate::agents::detect(&title, "")).flatten()
                });
            let status = agent
                .and_then(|a| crate::detect::manifest_for(manifests, a))
                .and_then(|m| {
                    let lines = self.panes.get(&id).map(|p| p.emu.bottom_text(15))?;
                    crate::detect::classify(m, &title, &lines)
                })
                .unwrap_or(Status::Unknown);

            let Some(p) = self.panes.get_mut(&id) else { continue };
            if p.agent != agent {
                p.agent = agent;
                p.agent_since = None;
                self.dirty = true; // agent row appears/leaves the sidebar
            }
            // Stamp on presence, not transition: a direct-spawned agent is
            // already Some on the first poll (spawn seeds p.agent), and a
            // handoff heir re-derives it from the program — a transition
            // stamp would leave both at None forever.
            if p.agent.is_some() && p.agent_since.is_none() {
                p.agent_since = Some(std::time::SystemTime::now());
            }
            p.agent_pid = agent_pid;
            if agent_bin.is_some() && p.agent_bin != agent_bin {
                p.agent_bin = agent_bin;
            }
            // Which profile: the agent process's own CLAUDE_CONFIG_DIR.
            // Re-read every poll (one cheap sysctl) — caching on the pid
            // froze transient read failures and survived agent crashes.
            let dir = agent_pid
                .and_then(|pid| crate::platform::process_env_var(pid, "CLAUDE_CONFIG_DIR"));
            if p.agent_config_dir != dir {
                p.agent_config_dir = dir;
                self.dirty = true;
            }
            // An agent that stays gone releases its conversation: without
            // this, a pane the user turned back into a shell resurrects
            // claude on restore, and the picker hides the conversation.
            // Grace of a few polls tolerates transient process-scan misses.
            if p.agent.is_none() && self.agent_sessions.contains_key(&id) {
                p.agent_gone_polls = p.agent_gone_polls.saturating_add(1);
                if p.agent_gone_polls >= 6 {
                    self.agent_sessions.remove(&id);
                }
            } else {
                p.agent_gone_polls = 0;
            }
            p.status = status;
            let eff = p.effective_status();
            if eff == p.last_shown {
                continue;
            }
            let prev = p.last_shown;
            let prev_lasted = p.status_since.elapsed();
            p.last_shown = eff;
            p.status_since = std::time::Instant::now();
            self.dirty = true;
            changes.push(StatusChange { pane: id, agent, from: prev, to: eff });

            let Some(agent) = agent else { continue };
            // Same precedence as the sidebar: user name > OSC title > agent.
            let name = match self.state.pane_name(id) {
                Some(n) => crate::agents::truncate_clean(n, 24),
                None if title.trim().is_empty() => agent.to_string(),
                None => crate::agents::truncate_clean(&title, 24),
            };
            if eff == Status::Blocked {
                if let Some(p) = self.panes.get_mut(&id) {
                    p.unseen = Some(NoticeKind::Blocked);
                }
                notices.push(Notice { pane: id, kind: NoticeKind::Blocked, name });
            } else if prev == Status::Working
                && matches!(eff, Status::Idle | Status::Done)
                // Manifest-confirmed or hook-reported — not the activity
                // fallback, which flips on every 3s output pause and would
                // spam "finished" for manifest-less agents.
                && (p.status != Status::Unknown
                    || p.reported.as_ref().is_some_and(|r| r.until > std::time::Instant::now()))
                && prev_lasted >= Duration::from_secs(5)
            {
                // Finished a real stretch of work — not spinner flicker.
                if let Some(p) = self.panes.get_mut(&id) {
                    p.unseen = Some(NoticeKind::Done);
                }
                notices.push(Notice { pane: id, kind: NoticeKind::Done, name });
            }
        }
        (notices, changes)
    }

    /// Track each space's folder. The anchor is sticky: as long as at least
    /// one pane (terminal or agent) still lives in the anchor folder — same
    /// project, see `anchor_holds` — the space stays put, so tabs exploring
    /// the repo never drag it. Only when every pane has left does the space
    /// move (to the folder most panes are in now). Also auto-rename (unless
    /// renamed manually) and refresh the git branch.
    pub fn poll_workspaces(&mut self) {
        // Prune branch entries of closed workspaces (worktree churn leaks).
        let live: std::collections::HashSet<_> =
            self.state.workspaces.iter().map(|w| w.id).collect();
        self.branches.retain(|id, _| live.contains(id));
        for wi in 0..self.state.workspaces.len() {
            let ws = &self.state.workspaces[wi];
            let ws_id = ws.id;
            let current = ws.cwd.clone();
            let mut votes: HashMap<std::path::PathBuf, usize> = HashMap::new();
            let mut anchor_alive = false;
            for pane in ws.tabs.iter().flat_map(|t| t.layout.panes()) {
                if let Some(cwd) = self
                    .panes
                    .get(&pane)
                    .and_then(|p| p.pty.child_pid)
                    .and_then(crate::platform::process_cwd)
                {
                    anchor_alive |= anchor_holds(&current, &cwd);
                    *votes.entry(cwd).or_default() += 1;
                }
            }
            let winner = if anchor_alive {
                None // somebody is still home — the space stays
            } else {
                votes.into_iter().max_by_key(|(_, n)| *n).map(|(cwd, _)| cwd)
            };
            if let Some(cwd) = winner {
                let ws = &mut self.state.workspaces[wi];
                if ws.cwd != cwd {
                    ws.cwd = cwd.clone();
                    self.dirty = true;
                }
                let name = folder_name(&cwd);
                if !ws.custom_name && ws.name != name {
                    self.state.auto_rename_workspace(wi, name);
                    self.dirty = true;
                }
            }
            let branch = crate::git::branch(&self.state.workspaces[wi].cwd);
            let old = self.branches.get(&ws_id);
            if branch.as_ref() != old {
                match branch {
                    Some(b) => {
                        self.branches.insert(ws_id, b);
                    }
                    None => {
                        self.branches.remove(&ws_id);
                    }
                }
                self.dirty = true;
            }
        }
    }

    /// Re-read config, keymap, and theme from disk and repaint. A --config
    /// launch override is honored via CDOCK_CONFIG_PATH (pinned in main).
    pub fn reload_config(&mut self) {
        let (cfg, warnings) = crate::config::load(None);
        let (keymap, kw) = crate::config::keys::build_keymap(&cfg.keys);
        let (theme, tw) = crate::config::theme::resolve(&cfg.theme);
        for w in warnings.iter().chain(&kw).chain(&tw) {
            tracing::warn!("reload: {w}");
        }
        self.cfg = cfg;
        self.keymap = keymap;
        self.theme = theme;
        self.dirty = true;
        tracing::info!("config reloaded");
    }

    /// Create a git worktree for workspace `wi` and open it as a child space.
    pub fn create_worktree(&mut self, wi: usize, branch: &str, area: Rect) {
        let Some(ws) = self.state.workspaces.get(wi) else { return };
        let (repo_cwd, parent_id) = (ws.cwd.clone(), ws.id);
        let root = self.cfg.worktrees.root();
        match crate::git::worktree_add(&repo_cwd, branch, &root) {
            Ok(path) => self.open_worktree(parent_id, path, area, true),
            Err(e) => tracing::warn!(error = %e, branch, "worktree add failed"),
        }
    }

    /// Open an existing worktree path as a child space of `parent_id`.
    /// `activate=false` for API callers — background automation must not
    /// yank the user's view.
    pub fn open_worktree(
        &mut self,
        parent_id: crate::state::ids::WorkspaceId,
        path: std::path::PathBuf,
        area: Rect,
        activate: bool,
    ) {
        let name = folder_name(&path);
        let pane = self.state.new_workspace_full(name, path, Some(parent_id), activate);
        if let Err(e) = self.spawn_pane(pane, area.width.max(4), area.height.max(4)) {
            tracing::warn!(error = %e, "worktree space spawn failed");
            self.state.close_pane(pane); // a leaf without a PTY is unclosable
        }
    }

    /// Snapshot the session and write it out before returning. For the
    /// paths that must not outlive the write: shutdown, handoff.
    pub fn save_session(&mut self) {
        if let Some(p) = self.stage_session() {
            crate::state::snapshot::persist(p);
        }
    }

    /// Autosave: stage on the loop, write on a blocking thread — the pane
    /// walk and serialization stay here, fsync and file churn move off the
    /// event loop so PTY output never waits on the disk.
    pub fn save_session_bg(&mut self) {
        if let Some(p) = self.stage_session() {
            tokio::task::spawn_blocking(move || crate::state::snapshot::persist(p));
        }
    }

    /// Collect everything a snapshot needs, remembering which agent ran in
    /// which pane.
    fn stage_session(&mut self) -> Option<crate::state::snapshot::Pending> {
        if !self.persist {
            return None;
        }
        // agent_sessions holds FULL "agent:id" idents. Pre-0.4 servers and
        // handoffs stored bare claude uuids — normalize on read.
        fn full_ident(s: &str) -> String {
            if s.contains(':') { s.to_string() } else { format!("claude:{s}") }
        }
        // Ids already bound to a pane: two same-cwd codex panes must not
        // both claim the newest session file.
        let mut claimed: std::collections::HashSet<String> =
            self.agent_sessions.values().map(|s| full_ident(s)).collect();
        let mut discovered: Vec<(PaneId, String)> = Vec::new();
        let mut metas = HashMap::new();
        for (id, p) in &self.panes {
            // "agent:session-id" when the integration hook reported one.
            // The hook can land before the 500ms detection poll notices the
            // agent — the ident must not wait for detection.
            // The pane's own folder — an agent must resume where its
            // conversation lives, wherever the workspace anchor drifted.
            let cwd = p.pty.child_pid.and_then(crate::platform::process_cwd);
            let agent = match (p.agent, self.agent_sessions.get(id)) {
                (_, Some(s)) => Some(full_ident(s)),
                // codex/opencode have no SessionStart hook — match their
                // session files by cwd + when the agent appeared (minus a
                // grace: the file usually predates the first detection poll).
                (Some(a), None) => Some(
                    cwd.as_deref()
                        .zip(p.agent_since)
                        .and_then(|(c, t)| {
                            let since = t - Duration::from_secs(5);
                            crate::agents::newest_agent_session(a, std::path::Path::new(c), since)
                        })
                        .map(|s| format!("{a}:{s}"))
                        .filter(|ident| !claimed.contains(ident))
                        .inspect(|ident| {
                            // Cache: later saves skip the fs walk, and the
                            // binding survives handoff and quiet mtimes.
                            claimed.insert(ident.clone());
                            discovered.push((*id, ident.clone()));
                        })
                        .unwrap_or_else(|| a.to_string()),
                ),
                (None, None) => None,
            };
            // Profile env rides along so restore resumes under the same
            // CLAUDE_CONFIG_DIR, and the cdock profile NAME so restore can
            // re-merge that profile's [env] block.
            let mut env = p
                .agent_config_dir
                .as_ref()
                .map(|d| vec![("CLAUDE_CONFIG_DIR".to_string(), d.clone())])
                .unwrap_or_default();
            if let Some(name) = p
                .agent_pid
                .and_then(|pid| crate::platform::process_env_var(pid, "CDOCK_AGENT_PROFILE"))
            {
                env.push(("CDOCK_AGENT_PROFILE".to_string(), name));
            }
            let name = self.state.pane_name(*id).map(str::to_string);
            if agent.is_some() || cwd.is_some() || name.is_some() {
                metas.insert(
                    *id,
                    crate::state::snapshot::PaneMeta {
                        agent,
                        cwd,
                        env,
                        agent_bin: p.agent_bin.clone(),
                        behavior: p.behavior.clone(),
                        name,
                        saved_pane: None, // save-side: the layout leaf carries the id
                    },
                );
            }
        }
        for (id, ident) in discovered {
            self.agent_sessions.insert(id, ident);
        }
        // Screen history: a text tail per pane, replayed on cold restore.
        // Alt-screen panes are skipped — scrollback_text is just the TUI
        // frame there and replaying it is garbage.
        let enabled = self.cfg.restore.screen_history;
        let screens = if enabled { self.screen_tails() } else { Vec::new() };
        Some(crate::state::snapshot::stage(&self.state, &metas, enabled, screens))
    }

    /// Per-pane screen tails. Alt-screen panes report None — the visible TUI
    /// frame is garbage to replay, the earlier primary tail stays on disk.
    fn screen_tails(&self) -> Vec<(u64, Option<String>)> {
        self.panes
            .iter()
            .map(|(id, p)| {
                let text = (!p.emu.on_alt_screen())
                    .then(|| p.emu.scrollback_tail(crate::state::snapshot::SCREEN_MAX_LINES));
                (id.0, text)
            })
            .collect()
    }

    /// Screen tails for a live handoff — written regardless of the
    /// `screen_history` setting: the heir deletes them on adoption (one-shot
    /// transfer, not history), and if the handoff never happens the next
    /// autosave purges them when the feature is off.
    pub fn save_handoff_screens(&self) {
        if let Some(dir) = crate::state::snapshot::screens_dir() {
            crate::state::snapshot::save_screens(&dir, &self.screen_tails());
        }
    }

    /// Default workspace name: the folder new panes spawn in.
    pub fn workspace_name(&self) -> String {
        folder_name(&resolve_cwd(&self.cfg.terminal))
    }

    /// Folder for a brand-new space (per [terminal].new_cwd). "follow"
    /// means the focused pane's live process cwd — same rule as new panes.
    pub fn new_space_cwd(&self) -> std::path::PathBuf {
        if self.cfg.terminal.new_cwd == "follow"
            && let Some(cwd) = self
                .panes
                .get(&self.state.focused_pane())
                .and_then(|p| p.pty.child_pid)
                .and_then(crate::platform::process_cwd)
                .filter(|c| c.is_dir())
        {
            return cwd;
        }
        resolve_cwd(&self.cfg.terminal)
    }

    /// `[[keys.command]]`: pane → run in a new tab; shell → silent background run.
    pub fn run_custom_command(&mut self, cmd: &CustomCommand, area: Rect) -> io::Result<()> {
        match cmd.kind {
            CommandKind::Pane => {
                let pane = self.state.new_tab();
                self.spawn_pane_cmd(pane, area.width, area.height, Some(cmd.command.clone()))
            }
            CommandKind::Shell => {
                let focused = self.state.focused_pane();
                let result = std::process::Command::new("/bin/sh")
                    .arg("-c")
                    .arg(&cmd.command)
                    .env("CDOCK_ENV", "1")
                    .env("CDOCK_PANE_ID", focused.to_string())
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn();
                match result {
                    // Reap — an unwaited child is a zombie for the server's life.
                    Ok(mut child) => {
                        std::thread::spawn(move || {
                            let _ = child.wait();
                        });
                    }
                    Err(e) => {
                        tracing::warn!(command = %cmd.command, error = %e, "shell command failed");
                    }
                }
                Ok(())
            }
        }
    }

    /// Geometry phase: compute pane rects for the active tab and propagate
    /// size changes to emulators and PTYs. Mutation happens here, never in render.
    /// The user is looking at this pane now — the sidebar's unseen marker
    /// has done its job. Called per client, per frame, for the pane it has
    /// focused (see server::render_clients).
    pub fn mark_seen(&mut self, pane: PaneId) {
        if let Some(p) = self.panes.get_mut(&pane)
            && p.unseen.take().is_some()
        {
            self.dirty = true;
        }
    }

    /// Pane rectangles for the active tab — pure geometry, no resizing:
    /// each attached client lays out at ITS own size, and a pane shown in
    /// several clients must end up at ONE pty size (see `apply_pane_sizes`).
    pub fn layout_panes(&self, area: Rect) -> (Vec<(PaneId, Rect)>, Vec<Divider>) {
        let tab = self.state.active_tab();
        match tab.zoomed {
            Some(z) if tab.layout.contains(z) => (vec![(z, area)], Vec::new()),
            _ => tab.layout.layout(area),
        }
    }

    /// Resize emulators and PTYs to the agreed per-pane sizes. A pane two
    /// clients both display gets the SMALLEST of their sizes (tmux's rule):
    /// then everyone can see all of it, and the pty is not resized twice per
    /// tick by clients fighting over it.
    pub fn apply_pane_sizes(&mut self, sizes: &HashMap<PaneId, (u16, u16)>) {
        for (id, size) in sizes {
            if let Some(p) = self.panes.get_mut(id)
                && p.last_size != *size
            {
                p.emu.resize(size.0, size.1);
                p.pty.resize(size.0, size.1);
                p.last_size = *size;
            }
        }
    }
}

/// Build the runtime: config resolution, snapshot restore (or a fresh
/// state), and the initial pane spawns. `area` is the first client's size.
pub fn build(
    cfg: Config,
    tx: mpsc::UnboundedSender<AppEvent>,
    data_tx: mpsc::Sender<PtyData>,
    raw_out: mpsc::UnboundedSender<Vec<u8>>,
    area: Rect,
) -> io::Result<Runtime> {
    let (keymap, key_warnings) = crate::config::keys::build_keymap(&cfg.keys);
    let (theme, theme_warnings) = crate::config::theme::resolve(&cfg.theme);
    for w in key_warnings.iter().chain(&theme_warnings) {
        tracing::warn!("{w}");
    }

    // Restore the last session's structure if a snapshot exists.
    let (state, initial_panes) = match crate::state::snapshot::load().and_then(|s| s.restore()) {
        Some((st, panes)) => (st, panes),
        None => {
            let cwd = resolve_cwd(&cfg.terminal);
            let st = AppState::new(folder_name(&cwd), cwd);
            let first = st.focused_pane();
            (st, vec![(first, crate::state::snapshot::PaneMeta::default())])
        }
    };
    let mut rt = Runtime {
        state,
        panes: HashMap::new(),
        cfg,
        keymap,
        theme,
        titles: HashMap::new(),
        branches: HashMap::new(),
        persist: true,
        agent_sessions: HashMap::new(),
        toasts: Vec::new(),
        update_available: None,
        last_view: None,
        sidebar_scroll: 0,
        drag: None,
        last_click: None,
        tx,
        data_tx,
        raw_out,
        dirty: true,
    };
    for (pane, meta) in initial_panes {
        // A failed resume must degrade into a shell, not close the pane —
        // an instant exit cascades into killing the tab and the space.
        let resume = meta.agent.as_deref().map(|a| {
            let mut cmd = crate::agents::resume_command(a);
            // /bin/sh has no shell-rc PATH: prefer the recorded absolute
            // binary when it still exists (agent updates replace the path —
            // then the bare name + inherited PATH is the fallback).
            if let Some(bin) = meta
                .agent_bin
                .as_deref()
                .filter(|b| std::path::Path::new(b).is_file() && !b.contains('\''))
                && let Some(rest) = cmd.split_once(' ').map(|(_, r)| r.to_string())
            {
                // Quote-hostile paths fall back to the bare name + PATH.
                cmd = format!("'{bin}' {rest}");
            }
            // An attached behavior rides back in as system prompt (claude
            // adapter only — other CLIs got it as a chat message live and
            // their conversation history already carries it).
            if a.starts_with("claude")
                && let Some(ident) = meta.behavior.as_deref()
                && let Some(cwd) = meta.cwd.as_deref()
                && let Ok(profile) = crate::profile::load_behavior(ident, cwd)
                && let Some(staged) = profile.stage_prompt()
                && !staged.display().to_string().contains('\'')
            {
                cmd.push_str(&format!(" --append-system-prompt \"$(cat '{}')\"", staged.display()));
            }
            crate::agents::hold_on_failure(&cmd)
        });
        // Seed the session mapping now: the first autosave fires before the
        // agent's hook re-reports (claude) or before the file walk can
        // rediscover (codex/opencode), and must not strip the ident. The
        // map holds full "agent:id" idents.
        if let Some(ident) = meta.agent.as_deref().filter(|a| a.contains(':')) {
            rt.agent_sessions.insert(pane, ident.to_string());
            // The name follows the CONVERSATION: a snapshot without one
            // (older server, or the pane was renamed elsewhere) still gets
            // the user's name back.
            if meta.name.is_none()
                && let Some(id) = ident.split_once(':').map(|(_, i)| i)
                && let Some(name) = crate::agents::session_names().get(id)
            {
                rt.state.rename_pane(pane, name.clone());
            }
        }
        // The pane's own saved folder wins over the workspace anchor —
        // agent conversations are folder-bound; env carries the profile.
        // Snapshots from before profile tracking miss the env: recover the
        // profile by finding which ~/.claude* owns the conversation.
        let mut env = meta.env;
        if !env.iter().any(|(k, _)| k == "CLAUDE_CONFIG_DIR")
            && let Some(id) = meta.agent.as_deref().and_then(|a| a.strip_prefix("claude:"))
            && let Some(dir) = crate::agents::find_session_profile(id)
        {
            env.push(("CLAUDE_CONFIG_DIR".to_string(), dir.display().to_string()));
        }
        // A pane that ran a cdock profile gets that profile's [env] block
        // re-merged (saved env keys win — they reflect what actually ran).
        if let Some(name) =
            env.iter().find(|(k, _)| k == "CDOCK_AGENT_PROFILE").map(|(_, v)| v.clone())
            && let Ok(profile) = crate::profile::load(&name)
        {
            let (_, profile_env) = profile.resolve();
            for (k, v) in profile_env {
                if !env.iter().any(|(ek, _)| *ek == k) {
                    env.push((k, v));
                }
            }
        }
        // No per-pane cwd (old snapshot) → the pane's OWN workspace folder;
        // spawn_opts would otherwise use whichever workspace is active.
        let cwd = meta.cwd.or_else(|| {
            rt.state
                .workspaces
                .iter()
                .find(|w| w.tabs.iter().any(|t| t.layout.contains(pane)))
                .map(|w| w.cwd.clone())
        });
        rt.spawn_pane_full(pane, area.width, area.height, resume, env, cwd)?;
        if let Some(p) = rt.panes.get_mut(&pane) {
            p.behavior = meta.behavior.clone();
        }
        // Replay the pane's saved screen tail into the EMULATOR only, before
        // any child output lands (PTY reads drain later, via the channel).
        // Never write it to the PTY — the shell would eat it as input.
        if let Some(text) = meta
            .saved_pane
            .zip(crate::state::snapshot::screens_dir())
            .and_then(|(old, dir)| {
                crate::state::snapshot::restore_screen(rt.cfg.restore.screen_history, &dir, old)
            })
            && let Some(p) = rt.panes.get_mut(&pane)
        {
            p.emu.feed(&crate::state::snapshot::screen_replay(&text));
        }
    }
    // Branches known before the first frame — the sidebar subtitle must not
    // repaint from counts to branch a poll-tick later.
    rt.poll_workspaces();
    Ok(rt)
}

/// Feed a batch of PTY output within the drain budget.
pub fn feed_pty(rt: &mut Runtime, id: PaneId, bytes: &[u8]) {
    if let Some(p) = rt.panes.get_mut(&id) {
        p.emu.feed(bytes);
        p.last_output = std::time::Instant::now();
        rt.dirty = true;
    }
}

pub const PTY_DRAIN_BUDGET: usize = DRAIN_BUDGET;

/// Where new panes spawn, per [terminal].new_cwd.
fn resolve_cwd(t: &crate::config::TerminalCfg) -> std::path::PathBuf {
    match t.new_cwd.as_str() {
        "home" => std::env::var_os("HOME").map(std::path::PathBuf::from),
        p if p.starts_with('/') => Some(std::path::PathBuf::from(p)),
        // "follow"/"current" without a live pane to follow (boot, or a
        // dead process): the server's own cwd.
        _ => None,
    }
    .or_else(|| std::env::current_dir().ok())
    .unwrap_or_else(|| std::path::PathBuf::from("/"))
}

fn folder_name(p: &std::path::Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "/".to_string())
}

/// Does a pane sitting in `cwd` still hold the space at `anchor`? Only if it
/// is in the same project: inside the anchor's subtree AND in the same repo
/// (a folder outside any repo is its own project). Without the repo test a
/// container folder like ~/projects is a black hole — every project under it
/// counts as "still home", so `cd repo` never moves the space down.
fn anchor_holds(anchor: &std::path::Path, cwd: &std::path::Path) -> bool {
    let project = |p: &std::path::Path| crate::git::root(p).unwrap_or_else(|| p.to_path_buf());
    cwd.starts_with(anchor) && project(cwd) == project(anchor)
}

/// A pane's process exited: drop its runtime, cascade the close. Closing the
/// last tab of the last space does NOT quit — a fresh root space opens so the
/// runtime always has a terminal (quit stays on the tab-bar ✕ / prefix keys).
pub fn handle_pane_exit(rt: &mut Runtime, id: PaneId, area: Rect) {
    if let Some(mut p) = rt.panes.remove(&id) {
        p.pty.kill();
    }
    rt.titles.remove(&id);
    rt.agent_sessions.remove(&id);
    rt.dirty = true;
    if matches!(rt.state.close_pane(id), CloseOutcome::LastClosed) {
        let name = rt.workspace_name();
        let cwd = rt.new_space_cwd();
        let pane = rt.state.new_workspace(name, cwd, None);
        if let Err(e) = rt.spawn_pane(pane, area.width.max(4), area.height.max(4)) {
            tracing::warn!(error = %e, "root space spawn failed");
        }
    }
}

pub fn handle_term_event(rt: &mut Runtime, id: PaneId, ev: TermEvent) {
    match ev {
        TermEvent::Wakeup | TermEvent::MouseCursorDirty | TermEvent::CursorBlinkingChange => {
            rt.dirty = true;
        }
        TermEvent::PtyWrite(text) => {
            if let Some(p) = rt.panes.get_mut(&id) {
                p.pty.write(text.as_bytes());
            }
        }
        TermEvent::Title(title) => {
            rt.titles.insert(id, title);
            rt.dirty = true;
        }
        TermEvent::ResetTitle => {
            rt.titles.remove(&id);
            rt.dirty = true;
        }
        TermEvent::ClipboardStore(_, data) => {
            rt.host_write(osc52_bytes(&data));
        }
        TermEvent::ColorRequest(idx, format) => {
            if let Some(p) = rt.panes.get_mut(&id) {
                let rgb = p.emu.palette_color(idx);
                p.pty.write(format(rgb).as_bytes());
            }
        }
        _ => {}
    }
}

/// OSC 52 clipboard-write escape for the host terminal.
pub fn osc52_bytes(data: &str) -> Vec<u8> {
    format!("\x1b]52;c;{}\x07", base64_engine::encode(data.as_bytes())).into_bytes()
}

/// ponytail: minimal base64 (RFC 4648) — only needed for OSC 52; not worth a dependency.
mod base64_engine {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    pub fn encode(input: &[u8]) -> String {
        let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
        for chunk in input.chunks(3) {
            let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
            let n = u32::from_be_bytes([0, b[0], b[1], b[2]]);
            out.push(TABLE[(n >> 18 & 63) as usize] as char);
            out.push(TABLE[(n >> 12 & 63) as usize] as char);
            out.push(if chunk.len() > 1 { TABLE[(n >> 6 & 63) as usize] as char } else { '=' });
            out.push(if chunk.len() > 2 { TABLE[(n & 63) as usize] as char } else { '=' });
        }
        out
    }
}

/// Everything a live handoff carries across exec(): pure state as JSON plus
/// raw master fds, which survive exec once CLOEXEC is cleared. Children keep
/// their pids — exec does not change ours, so they remain our children.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Handoff {
    /// Guard: only honored by the process with this pid (i.e. after exec).
    pub pid: u32,
    pub area: (u16, u16),
    pub state: crate::state::AppState,
    pub titles: Vec<(PaneId, String)>,
    pub agent_sessions: Vec<(PaneId, String)>,
    pub panes: Vec<HandoffPane>,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct HandoffPane {
    pub id: PaneId,
    pub fd: i32,
    pub pid: Option<u32>,
    pub program: String,
    pub size: (u16, u16),
    /// Attached behavior ident — survives the exec (metadata only).
    #[serde(default)]
    pub behavior: Option<String>,
}

/// Where the exec-handoff state file lives.
pub fn handoff_path() -> Option<std::path::PathBuf> {
    let name = std::env::var("CDOCK_SESSION").unwrap_or_else(|_| "default".to_string());
    crate::logging::state_dir().map(|d| d.join(format!("handoff-{name}.json")))
}

impl Default for Handoff {
    fn default() -> Self {
        Handoff {
            pid: 0,
            area: (0, 0),
            state: crate::state::AppState::new(String::new(), std::path::PathBuf::from("/")),
            titles: Vec::new(),
            agent_sessions: Vec::new(),
            panes: Vec::new(),
        }
    }
}

impl Default for HandoffPane {
    fn default() -> Self {
        HandoffPane {
            id: PaneId(0),
            fd: -1,
            pid: None,
            program: String::new(),
            size: (0, 0),
            behavior: None,
        }
    }
}

/// Snapshot the runtime for exec-handoff. Clears CLOEXEC on every master fd;
/// panes whose pty cannot expose an fd are dropped (they die with us).
pub fn capture_handoff(rt: &Runtime, area: Rect) -> Handoff {
    let panes = rt
        .panes
        .iter()
        .filter_map(|(id, p)| {
            let fd = p.pty.handoff_fd();
            if fd.is_none() {
                tracing::warn!(pane = %id, "handoff: no master fd — pane will not survive");
            }
            let fd = fd?;
            Some(HandoffPane {
                id: *id,
                fd,
                pid: p.pty.child_pid,
                program: p.program.clone(),
                size: p.last_size,
                behavior: p.behavior.clone(),
            })
        })
        .collect();
    Handoff {
        pid: std::process::id(),
        area: (area.width, area.height),
        state: serde_json::from_str(&serde_json::to_string(&rt.state).expect("state serializes"))
            .expect("state round-trips"),
        titles: rt.titles.iter().map(|(k, v)| (*k, v.clone())).collect(),
        agent_sessions: rt.agent_sessions.iter().map(|(k, v)| (*k, v.clone())).collect(),
        panes,
    }
}

/// Rebuild the runtime on the far side of an exec-handoff: same state, same
/// children, fresh emulators. Screens are blank until apps repaint — each
/// pane is nudged one column narrower so the next compute_panes resize is a
/// real change and delivers SIGWINCH.
pub fn build_from_handoff(
    cfg: Config,
    h: Handoff,
    tx: mpsc::UnboundedSender<AppEvent>,
    data_tx: mpsc::Sender<PtyData>,
    raw_out: mpsc::UnboundedSender<Vec<u8>>,
) -> io::Result<Runtime> {
    let (keymap, kw) = crate::config::keys::build_keymap(&cfg.keys);
    let (theme, tw) = crate::config::theme::resolve(&cfg.theme);
    for w in kw.iter().chain(&tw) {
        tracing::warn!("{w}");
    }
    let scrollback = cfg.advanced.scrollback_lines();
    let mut rt = Runtime {
        state: h.state,
        persist: true,
        panes: HashMap::new(),
        cfg,
        keymap,
        theme,
        titles: h.titles.into_iter().collect(),
        branches: HashMap::new(),
        agent_sessions: h.agent_sessions.into_iter().collect(),
        toasts: Vec::new(),
        update_available: None,
        last_view: None,
        sidebar_scroll: 0,
        drag: None,
        last_click: None,
        tx: tx.clone(),
        data_tx: data_tx.clone(),
        raw_out,
        dirty: true,
    };
    for hp in h.panes {
        let (cols, rows) = hp.size;
        match crate::term::pty::adopt(hp.id, hp.fd, hp.pid, tx.clone(), data_tx.clone()) {
            Ok(pty) => {
                let nudged = (cols.max(3) - 1, rows);
                pty.resize(nudged.0, nudged.1);
                rt.panes.insert(
                    hp.id,
                    PaneRuntime {
                        emu: Emulator::new(nudged.0, nudged.1, hp.id, tx.clone(), scrollback),
                        pty,
                        agent: crate::agents::detect("", &hp.program),
                        agent_pid: None,
                        agent_config_dir: None,
                        agent_bin: None,
                        reported: None,
                        unseen: None,
                        agent_since: None,
                        behavior: hp.behavior.clone(),
                        agent_gone_polls: 0,
                        program: hp.program,
                        last_output: std::time::Instant::now(),
                        status: crate::detect::Status::Unknown,
                        last_shown: crate::detect::Status::Unknown,
                        status_since: std::time::Instant::now(),
                        last_size: nudged,
                    },
                );
            }
            Err(e) => {
                tracing::warn!(pane = %hp.id, error = %e, "handoff adopt failed");
                // The dup'd master (CLOEXEC cleared) would otherwise leak
                // into every future child, holding the slave open forever.
                unsafe { libc::close(hp.fd) };
            }
        }
        // The fresh emulator is blank until the app writes again — replay
        // the screen tail saved just before the handoff (same pane ids).
        if let Some(text) = crate::state::snapshot::screens_dir()
            .and_then(|dir| crate::state::snapshot::take_screen(&dir, hp.id.0))
            && let Some(p) = rt.panes.get_mut(&hp.id)
        {
            p.emu.feed(&crate::state::snapshot::screen_replay(&text));
        }
    }
    // Panes that did not make it across close out of the tree now.
    let dead: Vec<PaneId> = rt
        .state
        .workspaces
        .iter()
        .flat_map(|w| w.tabs.iter())
        .flat_map(|t| t.layout.panes())
        .filter(|id| !rt.panes.contains_key(id))
        .collect();
    for id in dead {
        let area = Rect::new(0, 0, h.area.0, h.area.1);
        handle_pane_exit(&mut rt, id, area);
    }
    rt.poll_workspaces();
    Ok(rt)
}

/// An in-app toast: one overlay line; click focuses the pane when set.
#[derive(Debug, Clone)]
pub struct Toast {
    pub pane: Option<PaneId>,
    pub kind: NoticeKind,
    pub text: String,
    pub until: std::time::Instant,
}

impl Runtime {
    pub fn add_toast(&mut self, notice: &Notice) {
        let text = match notice.kind {
            NoticeKind::Blocked => format!("● {} needs input", notice.name),
            NoticeKind::Done => format!("✓ {} finished", notice.name),
        };
        self.push_toast(Some(notice.pane), notice.kind, text, 6);
    }

    /// Free-form toast (boot warnings etc.) — no jump target.
    pub fn add_plain_toast(&mut self, text: String, secs: u64) {
        self.push_toast(None, NoticeKind::Blocked, text, secs);
    }

    fn push_toast(&mut self, pane: Option<PaneId>, kind: NoticeKind, text: String, secs: u64) {
        self.toasts.push(Toast {
            pane,
            kind,
            text,
            until: std::time::Instant::now() + Duration::from_secs(secs),
        });
        if self.toasts.len() > 4 {
            self.toasts.remove(0);
        }
        self.dirty = true;
    }

    /// Drop expired toasts; true when the screen needs a repaint.
    pub fn expire_toasts(&mut self) -> bool {
        let now = std::time::Instant::now();
        let before = self.toasts.len();
        self.toasts.retain(|t| t.until > now);
        let changed = self.toasts.len() != before;
        if changed {
            self.dirty = true;
        }
        changed
    }
}

/// Any displayed-status transition — the event-subscription feed.
#[derive(Debug, Clone, Copy)]
pub struct StatusChange {
    pub pane: PaneId,
    pub agent: Option<&'static str>,
    pub from: crate::detect::Status,
    pub to: crate::detect::Status,
}

/// A hook-reported agent state (report-agent API).
#[derive(Debug, Clone)]
pub struct Reported {
    pub status: crate::detect::Status,
    /// Free-form label shown instead of the status word (e.g. "reviewing").
    pub label: Option<String>,
    pub until: std::time::Instant,
}

/// A status transition worth telling the user about.
#[derive(Debug, Clone)]
pub struct Notice {
    pub pane: PaneId,
    pub kind: NoticeKind,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeKind {
    /// The agent waits on the user.
    Blocked,
    /// The agent finished a stretch of work.
    Done,
}

/// What handling one input event asks of the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputOutcome {
    Continue,
    /// Detach clients; the server keeps running (prefix+q).
    Detach,
    /// Save and stop the server (tab-bar ✕).
    Shutdown,
}

/// Host input from a client, applied at that client's screen size.
pub fn handle_input(
    rt: &mut Runtime,
    ev: crossterm::event::Event,
    area: Rect,
) -> io::Result<InputOutcome> {
    use alacritty_terminal::term::TermMode;
    use crossterm::event::{Event, KeyEventKind};

    match ev {
        // Releases skip keybind matching (a binding must not fire twice)
        // and go straight to the pane — encode_key forwards them only when
        // the pane's kitty protocol asked for event types.
        Event::Key(key) if key.kind == KeyEventKind::Release => {
            rt.send_key(&key);
            return Ok(InputOutcome::Continue);
        }
        Event::Key(key) => {
            return input::handle_key(rt, key, area);
        }
        Event::Paste(text) if text.trim().is_empty() => {
            // Cmd+V with an IMAGE on the clipboard: the terminal pastes the
            // clipboard's text — and an image has none, so it sends an empty
            // paste. The image never travels through the pty at all; the agent
            // reads the system clipboard itself when it sees Ctrl+V. So: turn
            // the empty paste into that keypress, and the standard macOS
            // shortcut pastes pictures like it pastes text.
            let focused = rt.state.focused_pane();
            if crate::platform::clipboard_has_image()
                && let Some(p) = rt.panes.get_mut(&focused)
            {
                p.pty.write(&[0x16]); // Ctrl+V
            }
        }
        Event::Paste(text) => {
            let focused = rt.state.focused_pane();
            if let Some(p) = rt.panes.get_mut(&focused) {
                if p.emu.term.mode().contains(TermMode::BRACKETED_PASTE) {
                    p.pty.write(b"\x1b[200~");
                    p.pty.write(text.as_bytes());
                    p.pty.write(b"\x1b[201~");
                } else {
                    p.pty.write(text.as_bytes());
                }
            }
        }
        Event::Resize(..) => rt.dirty = true, // compute_view picks up the new size
        Event::Mouse(m) => return Ok(input::mouse::handle(rt, m, area)),
        _ => {}
    }
    Ok(InputOutcome::Continue)
}

#[cfg(test)]
mod tests {
    use crate::detect::Status;

    #[test]
    fn only_an_idle_claude_gets_its_conversation_renamed() {
        let go = super::should_rename_conversation;
        assert!(go(Some("claude"), Status::Idle, "auth work"));
        assert!(go(Some("claude"), Status::Done, "auth work"));
        // Typing into a busy agent queues a message, it does not run a command.
        assert!(!go(Some("claude"), Status::Working, "auth work"));
        assert!(!go(Some("claude"), Status::Blocked, "auth work"));
        // Clearing our own name override has no conversation-side meaning.
        assert!(!go(Some("claude"), Status::Idle, ""));
        // No /rename in the other CLIs, and a plain shell would just run it.
        assert!(!go(Some("codex"), Status::Idle, "auth work"));
        assert!(!go(None, Status::Idle, "auth work"));
    }

    #[test]
    fn a_container_folder_does_not_hold_the_space_against_a_project_under_it() {
        let holds = super::anchor_holds;
        let tmp = std::env::temp_dir().join(format!("cdk-anchor-{}", std::process::id()));
        let container = tmp.join("projects");
        let repo = container.join("repo");
        let plain = container.join("notes");
        std::fs::create_dir_all(repo.join("src")).unwrap();
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(&plain).unwrap();

        // Exploring inside the repo never drags the space out of the repo.
        assert!(holds(&repo, &repo));
        assert!(holds(&repo, &repo.join("src")));
        assert!(!holds(&repo, &plain));
        // A space anchored on the container yields to the folder panes moved
        // into — repo or plain folder alike — instead of sticking forever.
        assert!(holds(&container, &container));
        assert!(!holds(&container, &repo));
        assert!(!holds(&container, &plain));

        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn base64_rfc4648_vectors() {
        let e = |s: &str| super::base64_engine::encode(s.as_bytes());
        assert_eq!(e(""), "");
        assert_eq!(e("f"), "Zg==");
        assert_eq!(e("fo"), "Zm8=");
        assert_eq!(e("foo"), "Zm9v");
        assert_eq!(e("foobar"), "Zm9vYmFy");
        assert_eq!(super::base64_engine::encode(&[0xff, 0x00, 0xfe]), "/wD+");
    }
}
