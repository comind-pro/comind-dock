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
    /// A manual "check for update" finished: Ok(Some(tag)) = newer found,
    /// Ok(None) = already current, Err = the check itself failed.
    UpdateCheckDone(Result<Option<String>, String>),
    /// Press Enter in a pane as its OWN late keystroke: a CR in the same
    /// burst as a bracketed paste is sometimes folded into the paste by
    /// agent TUIs (claude's paste debounce) — the message then sits in the
    /// input box unsubmitted. Carries a normalized tail of the injected
    /// message for the later delivery check.
    SubmitEnter(PaneId, String),
    /// Delivery check ~1s after SubmitEnter: if the message tail is STILL
    /// on the pane's screen bottom (input box), the Enter was swallowed —
    /// press it once more.
    VerifySubmit(PaneId, String),
}

/// PTY output travels on its own BOUNDED channel: when the main loop falls
/// behind, reader threads block, and the kernel pty buffer throttles the
/// child — backpressure instead of an unbounded backlog in front of input.
pub type PtyData = (PaneId, Vec<u8>);
