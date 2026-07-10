pub mod event;

use std::io::{self, Write};
use std::time::Duration;

use alacritty_terminal::event::Event as TermEvent;
use ratatui::DefaultTerminal;
use tokio::sync::mpsc;

use crate::input;
use crate::state::ids::PaneId;
use crate::term::emulator::Emulator;
use crate::term::pty;
use crate::ui;
use event::AppEvent;

/// Max PTY bytes fed to emulators between renders, so `cat bigfile`
/// cannot starve input handling and the render tick.
const DRAIN_BUDGET: usize = 256 * 1024;

/// M1 runtime: a single fullscreen pane. Grows into the full
/// Runtime{state, panes} structure in M2/M3.
pub async fn run(terminal: &mut DefaultTerminal) -> io::Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel::<AppEvent>();

    let area = terminal.get_frame().area();
    let pane = PaneId(1);
    let mut emu = Emulator::new(area.width, area.height, pane, tx.clone());
    let mut pty = pty::spawn_shell(pane, area.width, area.height, tx.clone())?;

    spawn_input_thread(tx);

    let mut tick = tokio::time::interval(Duration::from_millis(16));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut dirty = true;

    loop {
        tokio::select! {
            maybe = rx.recv() => {
                let Some(first) = maybe else { return Ok(()) };
                let mut budget = DRAIN_BUDGET;
                let mut next = Some(first);
                while let Some(ev) = next.take() {
                    match ev {
                        AppEvent::PtyBytes(_, bytes) => {
                            budget = budget.saturating_sub(bytes.len());
                            emu.feed(&bytes);
                            dirty = true;
                        }
                        AppEvent::PtyExit(_) => {
                            pty.kill();
                            return Ok(());
                        }
                        AppEvent::Term(_, tev) => handle_term_event(tev, &mut pty, &mut dirty),
                        AppEvent::Input(iev) => {
                            handle_input(iev, &mut emu, &mut pty, &mut dirty);
                        }
                    }
                    if budget > 0 {
                        next = rx.try_recv().ok();
                    }
                }
            }
            _ = tick.tick() => {
                if dirty {
                    terminal.draw(|f| ui::pane_widget::render(&emu.term, f.area(), f))?;
                    dirty = false;
                }
            }
        }
    }
}

fn handle_term_event(ev: TermEvent, pty: &mut pty::Pty, dirty: &mut bool) {
    match ev {
        TermEvent::Wakeup | TermEvent::MouseCursorDirty | TermEvent::CursorBlinkingChange => {
            *dirty = true;
        }
        TermEvent::PtyWrite(text) => pty.write(text.as_bytes()),
        TermEvent::Title(title) => tracing::debug!(%title, "pane title"),
        TermEvent::ResetTitle => {}
        TermEvent::ClipboardStore(_, data) => osc52_copy(&data),
        // ponytail: OSC color queries answered with real palette values in M7
        TermEvent::ClipboardLoad(..) | TermEvent::ColorRequest(..) => {}
        TermEvent::TextAreaSizeRequest(_) => {}
        TermEvent::Bell => {}
        TermEvent::Exit | TermEvent::ChildExit(_) => {}
    }
}

/// Forward an application's OSC 52 clipboard write to the host terminal.
fn osc52_copy(data: &str) {
    use base64_engine::encode;
    let mut out = io::stdout();
    let _ = write!(out, "\x1b]52;c;{}\x07", encode(data.as_bytes()));
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

fn handle_input(
    ev: crossterm::event::Event,
    emu: &mut Emulator,
    pty: &mut pty::Pty,
    dirty: &mut bool,
) {
    use alacritty_terminal::term::TermMode;
    use crossterm::event::{Event, KeyEventKind};

    match ev {
        Event::Key(key) if key.kind != KeyEventKind::Release => {
            if let Some(bytes) = input::encode::encode_key(&key, emu.term.mode()) {
                pty.write(&bytes);
            }
        }
        Event::Paste(text) => {
            if emu.term.mode().contains(TermMode::BRACKETED_PASTE) {
                pty.write(b"\x1b[200~");
                pty.write(text.as_bytes());
                pty.write(b"\x1b[201~");
            } else {
                pty.write(text.as_bytes());
            }
        }
        Event::Resize(cols, rows) => {
            emu.resize(cols, rows);
            pty.resize(cols, rows);
            *dirty = true;
        }
        // Mouse lands in M5.
        _ => {}
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
