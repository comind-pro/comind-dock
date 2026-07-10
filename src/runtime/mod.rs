pub mod event;

use std::collections::HashMap;
use std::io::{self, Write};
use std::time::Duration;

use alacritty_terminal::event::Event as TermEvent;
use ratatui::DefaultTerminal;
use ratatui::layout::Rect;
use tokio::sync::mpsc;

use crate::input;
use crate::state::layout::{Dir, Divider, Side};
use crate::state::{AppState, CloseOutcome};
use crate::state::ids::PaneId;
use crate::term::emulator::Emulator;
use crate::term::pty::{self, Pty};
use crate::ui;
use event::AppEvent;

/// Max PTY bytes fed to emulators between renders, so `cat bigfile`
/// cannot starve input handling and the render tick.
const DRAIN_BUDGET: usize = 256 * 1024;

pub struct PaneRuntime {
    pub emu: Emulator,
    pub pty: Pty,
    last_size: (u16, u16),
}

pub struct Runtime {
    pub state: AppState,
    pub panes: HashMap<PaneId, PaneRuntime>,
    /// Pane rects from the last computed view — for neighbor focus and (M5) mouse hit testing.
    pub last_pane_rects: Vec<(PaneId, Rect)>,
    tx: mpsc::UnboundedSender<AppEvent>,
    dirty: bool,
    /// ponytail: minimal prefix chord state; the real input-mode machine lands in M4.
    prefix_pending: bool,
}

impl Runtime {
    fn spawn_pane(&mut self, pane: PaneId, cols: u16, rows: u16) -> io::Result<()> {
        let emu = Emulator::new(cols, rows, pane, self.tx.clone());
        let pty = pty::spawn_shell(pane, cols, rows, self.tx.clone())?;
        self.panes.insert(pane, PaneRuntime { emu, pty, last_size: (cols, rows) });
        Ok(())
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
                let size = (rect.width, rect.height);
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

pub async fn run(terminal: &mut DefaultTerminal) -> io::Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel::<AppEvent>();
    let area = terminal.get_frame().area();

    let state = AppState::new();
    let first = state.focused_pane();
    let mut rt = Runtime {
        state,
        panes: HashMap::new(),
        last_pane_rects: Vec::new(),
        tx,
        dirty: true,
        prefix_pending: false,
    };
    rt.spawn_pane(first, area.width, area.height)?;

    spawn_input_thread(rt.tx.clone());

    let mut tick = tokio::time::interval(Duration::from_millis(16));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            maybe = rx.recv() => {
                let Some(first) = maybe else { return Ok(()) };
                let mut budget = DRAIN_BUDGET;
                let mut next = Some(first);
                while let Some(ev) = next.take() {
                    match ev {
                        AppEvent::PtyBytes(id, bytes) => {
                            budget = budget.saturating_sub(bytes.len());
                            if let Some(p) = rt.panes.get_mut(&id) {
                                p.emu.feed(&bytes);
                                rt.dirty = true;
                            }
                        }
                        AppEvent::PtyExit(id) => {
                            if handle_pane_exit(&mut rt, id) {
                                return Ok(());
                            }
                        }
                        AppEvent::Term(id, tev) => handle_term_event(&mut rt, id, tev),
                        AppEvent::Input(iev) => {
                            if handle_input(&mut rt, iev, terminal)? {
                                return Ok(());
                            }
                        }
                    }
                    if budget > 0 {
                        next = rx.try_recv().ok();
                    }
                }
            }
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

/// A pane's process exited: drop its runtime, cascade the close. True → quit app.
fn handle_pane_exit(rt: &mut Runtime, id: PaneId) -> bool {
    let Some(mut p) = rt.panes.remove(&id) else { return false };
    p.pty.kill();
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
        TermEvent::Title(title) => tracing::debug!(pane = %id, %title, "pane title"),
        TermEvent::ClipboardStore(_, data) => osc52_copy(&data),
        // ponytail: OSC color queries answered with real palette values in M7
        _ => {}
    }
}

/// Forward an application's OSC 52 clipboard write to the host terminal.
fn osc52_copy(data: &str) {
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
    use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};

    match ev {
        Event::Key(key) if key.kind != KeyEventKind::Release => {
            let is_prefix = key.code == KeyCode::Char('b')
                && key.modifiers.contains(KeyModifiers::CONTROL);

            if rt.prefix_pending {
                rt.prefix_pending = false;
                rt.dirty = true;
                let area = terminal.get_frame().area();
                let rects = rt.last_pane_rects.clone();
                match (key.code, is_prefix) {
                    (_, true) => send_key(rt, &key), // double prefix → literal
                    (KeyCode::Char('v'), _) => split(rt, Dir::Right, area)?,
                    (KeyCode::Char('-'), _) => split(rt, Dir::Down, area)?,
                    (KeyCode::Char('h'), _) => { rt.state.focus_neighbor(&rects, Side::Left); }
                    (KeyCode::Char('j'), _) => { rt.state.focus_neighbor(&rects, Side::Down); }
                    (KeyCode::Char('k'), _) => { rt.state.focus_neighbor(&rects, Side::Up); }
                    (KeyCode::Char('l'), _) => { rt.state.focus_neighbor(&rects, Side::Right); }
                    (KeyCode::Char('x'), _) => {
                        // Kill only; PtyExit drives the state change (single close path).
                        let focused = rt.state.focused_pane();
                        if let Some(p) = rt.panes.get_mut(&focused) {
                            p.pty.kill();
                        }
                    }
                    (KeyCode::Char('z'), _) => rt.state.toggle_zoom(),
                    (KeyCode::Char('c'), _) => {
                        let pane = rt.state.new_tab();
                        rt.spawn_pane(pane, area.width, area.height)?;
                    }
                    (KeyCode::Char('n'), _) => rt.state.next_tab(),
                    (KeyCode::Char('p'), _) => rt.state.prev_tab(),
                    (KeyCode::Char('N'), _) => {
                        let pane = rt.state.new_workspace();
                        rt.spawn_pane(pane, area.width, area.height)?;
                    }
                    // ponytail: temp workspace cycling until M4 bindings + M5 sidebar clicks
                    (KeyCode::Char('o'), _) => rt.state.cycle_workspace(),
                    (KeyCode::Char('b'), _) => {
                        rt.state.sidebar_visible = !rt.state.sidebar_visible;
                    }
                    (KeyCode::Char('q'), _) => return Ok(true),
                    _ => {} // unknown chord: swallow
                }
            } else if is_prefix {
                rt.prefix_pending = true;
            } else {
                send_key(rt, &key);
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
        // Mouse lands in M5.
        _ => {}
    }
    Ok(false)
}

fn split(rt: &mut Runtime, dir: Dir, area: Rect) -> io::Result<()> {
    let pane = rt.state.split_focused(dir);
    // Spawned at a provisional size; compute_view corrects it before the next frame.
    rt.spawn_pane(pane, area.width.max(2) / 2, area.height.max(2) / 2)
}

fn send_key(rt: &mut Runtime, key: &crossterm::event::KeyEvent) {
    let focused = rt.state.focused_pane();
    if let Some(p) = rt.panes.get_mut(&focused) {
        if let Some(bytes) = input::encode::encode_key(key, p.emu.term.mode()) {
            p.pty.write(&bytes);
        }
    }
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
