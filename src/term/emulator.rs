//! The only module (with ui/pane_widget.rs) that touches alacritty_terminal types.

use alacritty_terminal::event::{Event as TermEvent, EventListener};
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{self, Term};
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
}
