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
    pub fn new(cols: u16, rows: u16, pane: PaneId, tx: UnboundedSender<AppEvent>) -> Self {
        let size = TermSize::new(cols.max(1) as usize, rows.max(1) as usize);
        let term = Term::new(term::Config::default(), &size, EventProxy { pane, tx });
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
}
