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
    /// Pinned on top of the sidebar list (★), whatever the sort.
    pub favorite: bool,
    /// Unix millis a client last looked at this space — the "recent" sort.
    pub last_used: u64,
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
            favorite: false,
            last_used: now_ms(),
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

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Sidebar order of spaces (clicked from the "spaces" header).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpaceSort {
    /// Creation order.
    #[default]
    Manual,
    /// Most recently looked-at first.
    Recent,
    Name,
}

impl SpaceSort {
    pub fn next(self) -> Self {
        match self {
            SpaceSort::Manual => SpaceSort::Recent,
            SpaceSort::Recent => SpaceSort::Name,
            SpaceSort::Name => SpaceSort::Manual,
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            SpaceSort::Manual => "manual",
            SpaceSort::Recent => "recent",
            SpaceSort::Name => "a–z",
        }
    }
}

/// Sidebar order: indices of the `shown` spaces — favorites first, then by
/// `sort`; worktree children follow their parent (a child whose parent is
/// not shown stands on its own).
pub fn space_order(ws: &[Workspace], shown: &[usize], sort: SpaceSort) -> Vec<usize> {
    let has_parent =
        |i: usize| ws[i].parent.is_some_and(|p| shown.iter().any(|&j| j != i && ws[j].id == p));
    let sorted = |mut v: Vec<usize>| {
        v.sort_by(|&a, &b| {
            let (a, b) = (&ws[a], &ws[b]);
            b.favorite.cmp(&a.favorite).then_with(|| match sort {
                SpaceSort::Manual => std::cmp::Ordering::Equal,
                SpaceSort::Recent => b.last_used.cmp(&a.last_used),
                SpaceSort::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            })
        }); // stable: ties keep creation order
        v
    };
    let mut out = Vec::new();
    for top in sorted(shown.iter().copied().filter(|&i| !has_parent(i)).collect()) {
        out.push(top);
        let kids =
            shown.iter().copied().filter(|&i| ws[i].parent == Some(ws[top].id) && has_parent(i));
        out.extend(sorted(kids.collect()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::ids::TabId;

    fn ws(id: u64, name: &str, used: u64) -> Workspace {
        let mut w = Workspace::new(
            WorkspaceId(id),
            name.into(),
            "/".into(),
            Tab::new(TabId(id), String::new(), PaneId(id)),
        );
        w.last_used = used;
        w
    }

    #[test]
    fn favorites_first_children_follow_parent() {
        let mut v = vec![ws(0, "b", 10), ws(1, "a", 30), ws(2, "c", 20), ws(3, "b-wt", 40)];
        v[3].parent = Some(WorkspaceId(0));
        let all = [0, 1, 2, 3];
        assert_eq!(space_order(&v, &all, SpaceSort::Manual), [0, 3, 1, 2]);
        assert_eq!(space_order(&v, &all, SpaceSort::Recent), [1, 2, 0, 3]);
        assert_eq!(space_order(&v, &all, SpaceSort::Name), [1, 0, 3, 2]);
        v[2].favorite = true;
        assert_eq!(space_order(&v, &all, SpaceSort::Name), [2, 1, 0, 3]);
        // Parent filtered out: the worktree stands alone, sorted with the rest.
        assert_eq!(space_order(&v, &[1, 3], SpaceSort::Recent), [3, 1]);
    }
}

/// A closed space, kept so the "+ new space" menu can reopen it. Only what
/// reopening needs — the layout, tabs and agents are gone with the panes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentSpace {
    pub name: String,
    pub cwd: PathBuf,
    pub profile: Option<String>,
}
