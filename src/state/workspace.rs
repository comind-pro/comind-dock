use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::ids::{PaneId, TabId, WorkspaceId};
use super::layout::Node;

// Field defaults: these structs cross the exec-handoff boundary between
// VERSIONS — a missing field must never abort an upgrade.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Tab {
    pub id: TabId,
    pub name: String,
    pub layout: Node,
    pub zoomed: Option<PaneId>,
    pub focused_pane: PaneId,
}

impl Default for Tab {
    fn default() -> Self {
        Tab {
            id: TabId(0),
            name: String::new(),
            layout: Node::Leaf(PaneId(0)),
            zoomed: None,
            focused_pane: PaneId(0),
        }
    }
}

impl Tab {
    pub fn new(id: TabId, name: String, pane: PaneId) -> Self {
        Self { id, name, layout: Node::Leaf(pane), zoomed: None, focused_pane: pane }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub name: String,
    /// Folder this space lives in; new panes spawn here. Tracked from the
    /// focused pane's shell, so `cd` moves the space.
    pub cwd: PathBuf,
    /// The user renamed it — stop auto-renaming after the folder.
    pub custom_name: bool,
    /// Worktree spaces group under their parent in the sidebar.
    pub parent: Option<WorkspaceId>,
    /// Agent profile associated with this space — the picker's default.
    pub profile: Option<String>,
    pub tabs: Vec<Tab>,
    pub active_tab: usize,
}

impl Default for Workspace {
    fn default() -> Self {
        Workspace::new(WorkspaceId(0), String::new(), std::path::PathBuf::from("/"), Tab::default())
    }
}

impl Workspace {
    pub fn new(id: WorkspaceId, name: String, cwd: PathBuf, tab: Tab) -> Self {
        Self {
            id,
            name,
            cwd,
            custom_name: false,
            parent: None,
            profile: None,
            tabs: vec![tab],
            active_tab: 0,
        }
    }

    pub fn active_tab(&self) -> &Tab {
        &self.tabs[self.active_tab.min(self.tabs.len() - 1)]
    }

    pub fn active_tab_mut(&mut self) -> &mut Tab {
        let i = self.active_tab.min(self.tabs.len() - 1);
        &mut self.tabs[i]
    }
}
