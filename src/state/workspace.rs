use super::ids::{PaneId, TabId, WorkspaceId};
use super::layout::Node;

#[derive(Debug, Clone)]
pub struct Tab {
    pub id: TabId,
    pub name: String,
    pub layout: Node,
    pub zoomed: Option<PaneId>,
    pub focused_pane: PaneId,
}

impl Tab {
    pub fn new(id: TabId, name: String, pane: PaneId) -> Self {
        Self { id, name, layout: Node::Leaf(pane), zoomed: None, focused_pane: pane }
    }
}

#[derive(Debug, Clone)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub name: String,
    pub tabs: Vec<Tab>,
    pub active_tab: usize,
}

impl Workspace {
    pub fn new(id: WorkspaceId, name: String, tab: Tab) -> Self {
        Self { id, name, tabs: vec![tab], active_tab: 0 }
    }

    pub fn active_tab(&self) -> &Tab {
        &self.tabs[self.active_tab.min(self.tabs.len() - 1)]
    }

    pub fn active_tab_mut(&mut self) -> &mut Tab {
        let i = self.active_tab.min(self.tabs.len() - 1);
        &mut self.tabs[i]
    }
}
