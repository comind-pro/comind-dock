use crate::state::ids::PaneId;

/// Everything the main event loop reacts to, funneled through one channel.
pub enum AppEvent {
    /// Raw output bytes from a pane's PTY.
    PtyBytes(PaneId, Vec<u8>),
    /// A pane's PTY hit EOF (process exited).
    PtyExit(PaneId),
    /// Event emitted by a pane's terminal emulator.
    Term(PaneId, alacritty_terminal::event::Event),
    /// Host terminal input (keys, mouse, resize, paste).
    Input(crossterm::event::Event),
}
