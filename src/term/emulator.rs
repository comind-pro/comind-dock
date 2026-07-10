//! The only module (with ui/pane_widget.rs) that touches alacritty_terminal types.

use alacritty_terminal::event::{Event as TermEvent, EventListener};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{self, Term, TermMode};
use alacritty_terminal::vte::ansi::Processor;
use tokio::sync::mpsc::UnboundedSender;

use crate::runtime::event::AppEvent;
use crate::state::ids::PaneId;

/// Forwards emulator events (Wakeup, Title, PtyWrite, …) into the app event loop.
#[derive(Clone)]
pub struct EventProxy {
    pane: PaneId,
    tx: UnboundedSender<AppEvent>,
}

impl EventListener for EventProxy {
    fn send_event(&self, event: TermEvent) {
        let _ = self.tx.send(AppEvent::Term(self.pane, event));
    }
}

pub struct Emulator {
    pub term: Term<EventProxy>,
    processor: Processor,
}

impl Emulator {
    pub fn new(
        cols: u16,
        rows: u16,
        pane: PaneId,
        tx: UnboundedSender<AppEvent>,
        scrollback_lines: usize,
    ) -> Self {
        let config = term::Config { scrolling_history: scrollback_lines, ..Default::default() };
        let size = TermSize::new(cols.max(1) as usize, rows.max(1) as usize);
        let term = Term::new(config, &size, EventProxy { pane, tx });
        Self { term, processor: Processor::new() }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.processor.advance(&mut self.term, bytes);
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.term.resize(TermSize::new(cols.max(1) as usize, rows.max(1) as usize));
    }

    /// Pane-local viewport cell → buffer point (accounts for scrollback offset).
    fn buffer_point(&self, col: u16, row: u16) -> Point {
        let offset = self.term.grid().display_offset() as i32;
        let line = Line(row as i32 - offset);
        let max_col = self.term.grid().columns().saturating_sub(1);
        Point::new(line, Column((col as usize).min(max_col)))
    }

    pub fn start_selection(&mut self, col: u16, row: u16, semantic: bool) {
        let ty = if semantic { SelectionType::Semantic } else { SelectionType::Simple };
        let point = self.buffer_point(col, row);
        self.term.selection = Some(Selection::new(ty, point, Side::Left));
    }

    pub fn update_selection(&mut self, col: u16, row: u16) {
        let point = self.buffer_point(col, row);
        if let Some(sel) = self.term.selection.as_mut() {
            sel.update(point, Side::Left);
        }
    }

    pub fn selection_text(&self) -> Option<String> {
        self.term.selection_to_string()
    }

    pub fn clear_selection(&mut self) {
        self.term.selection = None;
    }

    /// Scroll the viewport; positive = up into scrollback.
    pub fn scroll_display(&mut self, delta: i32) {
        self.term.scroll_display(Scroll::Delta(delta));
    }

    /// The application in this pane requested mouse reporting.
    pub fn wants_mouse(&self) -> bool {
        self.term.mode().intersects(TermMode::MOUSE_MODE)
    }

    /// Alt screen without mouse reporting: wheel becomes arrow keys.
    pub fn alternate_scroll(&self) -> bool {
        let mode = self.term.mode();
        mode.contains(TermMode::ALT_SCREEN) && mode.contains(TermMode::ALTERNATE_SCROLL)
    }

    /// Color for an OSC query (OSC 4/10/11/12): OSC-set palette entry if any,
    /// else the standard xterm value. 256 = foreground, 257 = background.
    pub fn palette_color(&self, idx: usize) -> alacritty_terminal::vte::ansi::Rgb {
        use alacritty_terminal::vte::ansi::Rgb;
        if let Some(rgb) = self.term.colors()[idx] {
            return rgb;
        }
        match idx {
            // Standard ANSI 16 (xterm defaults).
            0 => Rgb { r: 0, g: 0, b: 0 },
            1 => Rgb { r: 205, g: 0, b: 0 },
            2 => Rgb { r: 0, g: 205, b: 0 },
            3 => Rgb { r: 205, g: 205, b: 0 },
            4 => Rgb { r: 0, g: 0, b: 238 },
            5 => Rgb { r: 205, g: 0, b: 205 },
            6 => Rgb { r: 0, g: 205, b: 205 },
            7 => Rgb { r: 229, g: 229, b: 229 },
            8 => Rgb { r: 127, g: 127, b: 127 },
            9 => Rgb { r: 255, g: 0, b: 0 },
            10 => Rgb { r: 0, g: 255, b: 0 },
            11 => Rgb { r: 255, g: 255, b: 0 },
            12 => Rgb { r: 92, g: 92, b: 255 },
            13 => Rgb { r: 255, g: 0, b: 255 },
            14 => Rgb { r: 0, g: 255, b: 255 },
            15 => Rgb { r: 255, g: 255, b: 255 },
            // 6×6×6 color cube.
            16..=231 => {
                let i = idx - 16;
                let level = |n: usize| if n == 0 { 0 } else { (n * 40 + 55) as u8 };
                Rgb { r: level(i / 36), g: level(i / 6 % 6), b: level(i % 6) }
            }
            // Grayscale ramp.
            232..=255 => {
                let v = ((idx - 232) * 10 + 8) as u8;
                Rgb { r: v, g: v, b: v }
            }
            // Foreground / background / cursor: we don't own the host palette,
            // so report a dark theme baseline apps can use for light/dark detection.
            257 => Rgb { r: 30, g: 30, b: 30 },
            _ => Rgb { r: 229, g: 229, b: 229 },
        }
    }
}
