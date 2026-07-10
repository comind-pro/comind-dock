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
        let scrollback = self.cfg.advanced.scrollback_lines();
        let emu = Emulator::new(cols, rows, pane, self.tx.clone(), scrollback);
        let opts = self.spawn_opts(command);
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
    pub fn poll_agent_status(&mut self, manifests: &[crate::detect::Manifest]) -> Vec<Notice> {
        use crate::detect::Status;
        let mut notices = Vec::new();
        let ids: Vec<PaneId> = self.panes.keys().copied().collect();
        for id in ids {
            let title = self.titles.get(&id).cloned().unwrap_or_default();
            let Some(p) = self.panes.get(&id) else { continue };
            // Spawn command and title first; else look at what actually runs
            // inside the shell — `claude` typed into a zsh pane must count.
            let agent = crate::agents::detect(&title, &p.program).or_else(|| {
                p.pty
                    .child_pid
                    .map(crate::platform::child_process_names)
                    .unwrap_or_default()
                    .iter()
                    .find_map(|name| crate::agents::detect("", name))
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
        notices
    }

    /// Track each space's folder from its focused pane's shell: update cwd,
    /// auto-rename (unless renamed manually), refresh the git branch.
    pub fn poll_workspaces(&mut self) {
        for wi in 0..self.state.workspaces.len() {
            let ws = &self.state.workspaces[wi];
            let ws_id = ws.id;
            let pane = ws.active_tab().focused_pane;
            let pid = self.panes.get(&pane).and_then(|p| p.pty.child_pid);
            if let Some(cwd) = pid.and_then(crate::platform::process_cwd) {
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
        let mut agents = HashMap::new();
        for (id, p) in &self.panes {
            if let Some(agent) = p.agent {
                agents.insert(*id, agent.to_string());
            }
        }
        crate::state::snapshot::save(&self.state, &agents);
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
            (st, vec![(first, None)])
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
        last_view: None,
        sidebar_scroll: 0,
        drag: None,
        last_click: None,
        tx,
        data_tx,
        raw_out,
        dirty: true,
    };
    for (pane, agent) in initial_panes {
        let resume = agent.as_deref().map(crate::agents::resume_command);
        rt.spawn_pane_cmd(pane, area.width, area.height, resume)?;
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
    let Some(mut p) = rt.panes.remove(&id) else { return };
    p.pty.kill();
    rt.titles.remove(&id);
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
