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
use crate::state::layout::{Dir, Divider};
use crate::state::{AppState, CloseOutcome};
use crate::state::ids::PaneId;
use crate::term::emulator::Emulator;
use crate::term::pty::{self, Pty};
use event::{AppEvent, PtyData};

/// Max PTY bytes fed to emulators between renders, so `cat bigfile`
/// cannot starve input handling and the render tick.
const DRAIN_BUDGET: usize = 256 * 1024;

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
    pub fn effective_status(&self) -> crate::detect::Status {
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
}

/// An in-progress mouse drag gesture.
#[derive(Debug, Clone, Copy)]
pub enum MouseDrag {
    Divider { before: PaneId, after: PaneId, dir: Dir, extent: u16, last_pos: u16 },
    Select { pane: PaneId },
}

pub struct Runtime {
    pub state: AppState,
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
            && let Some(bytes) = input::encode::encode_key(key, p.emu.term.mode()) {
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
        self.spawn_pane(pane, area.width.max(2) / 2, area.height.max(2) / 2)
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
        let mut opts = self.spawn_opts(command);
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

    fn spawn_opts(&self, command: Option<String>) -> pty::SpawnOpts {
        let t = &self.cfg.terminal;
        let shell = if t.default_shell.is_empty() {
            std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
        } else {
            t.default_shell.clone()
        };
        let login = match t.shell_mode {
            ShellMode::Auto => cfg!(target_os = "macos"),
            ShellMode::Login => true,
            ShellMode::NonLogin => false,
        };
        // Panes spawn in their workspace's folder (worktree spaces have their own).
        let ws = self.state.active_workspace();
        let tab = ws.active_tab();
        pty::SpawnOpts {
            shell,
            login,
            cwd: ws.cwd.clone(),
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
            // Spawn command and title first; else look at what actually runs
            // inside the shell — `claude` typed into a zsh pane must count.
            // Idents are exe paths, word-matched like titles: Claude Code's
            // process is literally named "2.1.206", only its path says claude.
            let agent = crate::agents::detect(&title, &p.program).or_else(|| {
                p.pty
                    .child_pid
                    .map(crate::platform::child_process_idents)
                    .unwrap_or_default()
                    .iter()
                    .find_map(|ident| crate::agents::detect(ident, ""))
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
                self.dirty = true; // agent row appears/leaves the sidebar
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
            let name = if title.trim().is_empty() {
                agent.to_string()
            } else {
                title.chars().take(24).collect()
            };
            if eff == Status::Blocked {
                notices.push(Notice { pane: id, kind: NoticeKind::Blocked, name });
            } else if prev == Status::Working
                && matches!(eff, Status::Idle | Status::Done)
                && prev_lasted >= Duration::from_secs(5)
            {
                // Finished a real stretch of work — not spinner flicker.
                notices.push(Notice { pane: id, kind: NoticeKind::Done, name });
            }
        }
        (notices, changes)
    }

    /// Track each space's folder. The anchor is sticky: as long as at least
    /// one pane (terminal or agent) still lives in the anchor folder or its
    /// subtree, the space stays put — tabs exploring elsewhere never drag it.
    /// Only when every pane has left does the space move (to the folder most
    /// panes are in now). Also auto-rename (unless renamed manually) and
    /// refresh the git branch.
    pub fn poll_workspaces(&mut self) {
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
                    anchor_alive |= cwd.starts_with(&current);
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

    /// Re-read config, keymap, and theme from disk and repaint.
    /// ponytail: always the default path — a --config override on the
    /// original launch is not remembered by the server.
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
            Ok(path) => self.open_worktree(parent_id, path, area),
            Err(e) => tracing::warn!(error = %e, branch, "worktree add failed"),
        }
    }

    /// Open an existing worktree path as a child space of `parent_id`.
    pub fn open_worktree(
        &mut self,
        parent_id: crate::state::ids::WorkspaceId,
        path: std::path::PathBuf,
        area: Rect,
    ) {
        let name = folder_name(&path);
        let pane = self.state.new_workspace(name, path, Some(parent_id));
        if let Err(e) = self.spawn_pane(pane, area.width.max(4), area.height.max(4)) {
            tracing::warn!(error = %e, "worktree space spawn failed");
        }
    }

    /// Snapshot the session, remembering which agent ran in which pane.
    pub fn save_session(&self) {
        let mut metas = HashMap::new();
        for (id, p) in &self.panes {
            // "agent:session-id" when the integration hook reported one.
            let agent = p.agent.map(|agent| match self.agent_sessions.get(id) {
                Some(s) => format!("{agent}:{s}"),
                None => agent.to_string(),
            });
            // The pane's own folder — an agent must resume where its
            // conversation lives, wherever the workspace anchor drifted.
            let cwd = p.pty.child_pid.and_then(crate::platform::process_cwd);
            if agent.is_some() || cwd.is_some() {
                metas.insert(*id, crate::state::snapshot::PaneMeta { agent, cwd });
            }
        }
        crate::state::snapshot::save(&self.state, &metas);
    }

    /// Default workspace name: the folder new panes spawn in.
    pub fn workspace_name(&self) -> String {
        folder_name(&resolve_cwd(&self.cfg.terminal))
    }

    /// Folder for a brand-new space (per [terminal].new_cwd).
    pub fn new_space_cwd(&self) -> std::path::PathBuf {
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
                if let Err(e) = result {
                    tracing::warn!(command = %cmd.command, error = %e, "shell command failed");
                }
                Ok(())
            }
        }
    }

    /// Geometry phase: compute pane rects for the active tab and propagate
    /// size changes to emulators and PTYs. Mutation happens here, never in render.
    pub fn compute_panes(&mut self, area: Rect) -> (Vec<(PaneId, Rect)>, Vec<Divider>) {
        let tab = self.state.active_tab();
        let (rects, dividers) = match tab.zoomed {
            Some(z) if tab.layout.contains(z) => (vec![(z, area)], Vec::new()),
            _ => tab.layout.layout(area),
        };
        for (id, rect) in &rects {
            if let Some(p) = self.panes.get_mut(id) {
                let inner = crate::ui::content_rect(*rect);
                let size = (inner.width, inner.height);
                if p.last_size != size {
                    p.emu.resize(size.0, size.1);
                    p.pty.resize(size.0, size.1);
                    p.last_size = size;
                }
            }
        }
        (rects, dividers)
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
        agent_sessions: HashMap::new(),
        toasts: Vec::new(),
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
        let resume = meta.agent.as_deref().map(crate::agents::resume_command);
        // The pane's own saved folder wins over the workspace anchor —
        // agent conversations are folder-bound.
        rt.spawn_pane_full(pane, area.width, area.height, resume, Vec::new(), meta.cwd)?;
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
        // ponytail: "follow" = launch cwd until per-pane cwd tracking
        // (platform process info) lands; "current" is the same today.
        _ => None,
    }
    .or_else(|| std::env::current_dir().ok())
    .unwrap_or_else(|| std::path::PathBuf::from("/"))
}

fn folder_name(p: &std::path::Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "/".to_string())
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
pub struct HandoffPane {
    pub id: PaneId,
    pub fd: i32,
    pub pid: Option<u32>,
    pub program: String,
    pub size: (u16, u16),
}

/// Snapshot the runtime for exec-handoff. Clears CLOEXEC on every master fd;
/// panes whose pty cannot expose an fd are dropped (they die with us).
pub fn capture_handoff(rt: &Runtime, area: Rect) -> Handoff {
    let panes = rt
        .panes
        .iter()
        .filter_map(|(id, p)| {
            let fd = p.pty.handoff_fd()?;
            Some(HandoffPane {
                id: *id,
                fd,
                pid: p.pty.child_pid,
                program: p.program.clone(),
                size: p.last_size,
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
        panes: HashMap::new(),
        cfg,
        keymap,
        theme,
        titles: h.titles.into_iter().collect(),
        branches: HashMap::new(),
        agent_sessions: h.agent_sessions.into_iter().collect(),
        toasts: Vec::new(),
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
                        program: hp.program,
                        last_output: std::time::Instant::now(),
                        status: crate::detect::Status::Unknown,
                        last_shown: crate::detect::Status::Unknown,
                        status_since: std::time::Instant::now(),
                        last_size: nudged,
                    },
                );
            }
            Err(e) => tracing::warn!(pane = %hp.id, error = %e, "handoff adopt failed"),
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

/// An in-app toast: one overlay line, click focuses the pane.
#[derive(Debug, Clone)]
pub struct Toast {
    pub pane: PaneId,
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
        self.toasts.push(Toast {
            pane: notice.pane,
            kind: notice.kind,
            text,
            until: std::time::Instant::now() + Duration::from_secs(6),
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
        Event::Key(key) if key.kind != KeyEventKind::Release => {
            return input::handle_key(rt, key, area);
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
