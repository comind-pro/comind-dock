use crate::state::ids::PaneId;

/// Control events (input, emulator events, exits) — unbounded channel,
/// always drained before PTY output so input stays responsive under load.
pub enum AppEvent {
    /// A pane's child process exited.
    PtyExit(PaneId),
    /// Event emitted by a pane's terminal emulator.
    Term(PaneId, alacritty_terminal::event::Event),
    /// The background release check found a newer version tag.
    UpdateAvailable(String),
}

/// PTY output travels on its own BOUNDED channel: when the main loop falls
/// behind, reader threads block, and the kernel pty buffer throttles the
/// child — backpressure instead of an unbounded backlog in front of input.
pub type PtyData = (PaneId, Vec<u8>);
