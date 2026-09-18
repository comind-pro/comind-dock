//! Plain-data output of compute_view, consumed immutably by render.

use ratatui::layout::Rect;

use crate::state::ids::PaneId;
use crate::state::layout::Divider;

#[derive(Debug, Clone)]
pub struct View {
    pub tab_bar: Rect,
    pub sidebar: Option<Rect>,
    pub pane_rects: Vec<(PaneId, Rect)>,
    pub dividers: Vec<Divider>,
    pub focused: PaneId,
    /// The active tab's orchestrator pane and its always-open team panel
    /// (right column) — present only on tabs that hold an orchestrator.
    pub team_panel: Option<(PaneId, Rect)>,
}
