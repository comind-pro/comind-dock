pub mod event;

use std::collections::HashMap;
use std::io::{self, Write};
use std::time::Duration;

use alacritty_terminal::event::Event as TermEvent;
use ratatui::DefaultTerminal;
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
use crate::ui;
use event::{AppEvent, PtyData};

/// Max PTY bytes fed to emulators between renders, so `cat bigfile`
/// cannot starve input handling and the render tick.
const DRAIN_BUDGET: usize = 256 * 1024;

pub struct PaneRuntime {
    pub emu: Emulator,
    pub pty: Pty,
    /// Program shown in the agents sidebar (command or shell basename).
    pub program: String,
    /// Last PTY output — drives the working/idle activity heuristic
    /// (real agent detection is Phase 3).
    pub last_output: std::time::Instant,
    last_size: (u16, u16),
}

impl PaneRuntime {
    pub fn working(&self) -> bool {
        self.last_output.elapsed() < Duration::from_secs(3)
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
    pub drag: Option<MouseDrag>,
    pub last_click: Option<(std::time::Instant, u16, u16)>,
    tx: mpsc::UnboundedSender<AppEvent>,
    data_tx: mpsc::Sender<PtyData>,
    dirty: bool,
}

impl Runtime {
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
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
                program,
                last_output: std::time::Instant::now(),
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
            let title = self.titles.get(id).map(String::as_str).unwrap_or("");
            if let Some(agent) = crate::agents::detect(title, &p.program) {
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

pub async fn run(terminal: &mut DefaultTerminal, cfg: Config) -> io::Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel::<AppEvent>();
    // Small capacity on purpose: caps in-flight PTY output (~16×64 KB) so a
    // `cat bigfile` can never queue seconds of work in front of a keystroke.
    let (data_tx, mut data_rx) = mpsc::channel::<PtyData>(16);
    let area = terminal.get_frame().area();

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
        drag: None,
        last_click: None,
        tx,
        data_tx,
        dirty: true,
    };
    for (pane, agent) in initial_panes {
        let resume = agent.as_deref().map(crate::agents::resume_command);
        rt.spawn_pane_cmd(pane, area.width, area.height, resume)?;
    }

    spawn_input_thread(rt.tx.clone());

    let mut tick = tokio::time::interval(Duration::from_millis(16));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut ws_poll = tokio::time::interval(Duration::from_millis(2000));
    ws_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            // Input and control events always win over PTY output.
            biased;
            maybe = rx.recv() => {
                let Some(first) = maybe else { return Ok(()) };
                let mut next = Some(first);
                while let Some(ev) = next.take() {
                    match ev {
                        AppEvent::PtyExit(id) => {
                            if handle_pane_exit(&mut rt, id) {
                                // The user closed everything on purpose.
                                crate::state::snapshot::delete();
                                return Ok(());
                            }
                        }
                        AppEvent::Term(id, tev) => handle_term_event(&mut rt, id, tev),
                        AppEvent::Input(iev) => {
                            if handle_input(&mut rt, iev, terminal)? {
                                rt.save_session();
                                return Ok(());
                            }
                        }
                    }
                    next = rx.try_recv().ok();
                }
            }
            maybe = data_rx.recv() => {
                let Some(first) = maybe else { return Ok(()) };
                let mut budget = DRAIN_BUDGET;
                let mut next = Some(first);
                while let Some((id, bytes)) = next.take() {
                    budget = budget.saturating_sub(bytes.len());
                    if let Some(p) = rt.panes.get_mut(&id) {
                        p.emu.feed(&bytes);
                        p.last_output = std::time::Instant::now();
                        rt.dirty = true;
                    }
                    if budget > 0 {
                        next = data_rx.try_recv().ok();
                    }
                }
            }
            _ = ws_poll.tick() => rt.poll_workspaces(),
            _ = tick.tick() => {
                if rt.dirty {
                    let area = terminal.get_frame().area();
                    let view = ui::compute_view(&mut rt, area);
                    terminal.draw(|f| ui::render(&view, &rt, f))?;
                    rt.dirty = false;
                }
            }
        }
    }
}

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

/// A pane's process exited: drop its runtime, cascade the close. True → quit app.
fn handle_pane_exit(rt: &mut Runtime, id: PaneId) -> bool {
    let Some(mut p) = rt.panes.remove(&id) else { return false };
    p.pty.kill();
    rt.titles.remove(&id);
    rt.dirty = true;
    matches!(rt.state.close_pane(id), CloseOutcome::LastClosed)
}

fn handle_term_event(rt: &mut Runtime, id: PaneId, ev: TermEvent) {
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
        TermEvent::ClipboardStore(_, data) => osc52_copy(&data),
        TermEvent::ColorRequest(idx, format) => {
            if let Some(p) = rt.panes.get_mut(&id) {
                let rgb = p.emu.palette_color(idx);
                p.pty.write(format(rgb).as_bytes());
            }
        }
        _ => {}
    }
}

/// Forward a clipboard write (app OSC 52 or mouse selection) to the host terminal.
pub fn osc52_copy(data: &str) {
    let mut out = io::stdout();
    let _ = write!(out, "\x1b]52;c;{}\x07", base64_engine::encode(data.as_bytes()));
    let _ = out.flush();
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

/// Host input. Returns Ok(true) to quit the app.
fn handle_input(
    rt: &mut Runtime,
    ev: crossterm::event::Event,
    terminal: &mut DefaultTerminal,
) -> io::Result<bool> {
    use alacritty_terminal::term::TermMode;
    use crossterm::event::{Event, KeyEventKind};

    match ev {
        Event::Key(key) if key.kind != KeyEventKind::Release => {
            let area = terminal.get_frame().area();
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
        Event::Mouse(m) => {
            let area = terminal.get_frame().area();
            return Ok(input::mouse::handle(rt, m, area));
        }
        _ => {}
    }
    Ok(false)
}

/// crossterm's blocking event reader on a std thread; the tokio loop consumes
/// via the same channel as PTY bytes. ponytail: simpler than the event-stream
/// feature + futures dependency.
fn spawn_input_thread(tx: mpsc::UnboundedSender<AppEvent>) {
    std::thread::spawn(move || {
        loop {
            match crossterm::event::read() {
                Ok(ev) => {
                    if tx.send(AppEvent::Input(ev)).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "input read failed");
                    break;
                }
            }
        }
    });
}
