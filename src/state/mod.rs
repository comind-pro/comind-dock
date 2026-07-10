//! Pure application state: plain data, constructible in unit tests without
//! PTYs, async, or emulator types. The runtime wraps this.

pub mod ids;
pub mod layout;
pub mod snapshot;
pub mod workspace;

use ids::{IdGen, PaneId};
use layout::{Dir, Side};
use workspace::{Tab, Workspace};

/// What a rename prompt is renaming.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    RenameTab,
    RenameWorkspace,
}

/// Input-mode state machine (Terminal ↔ Prefix ↔ Resize, plus modal overlays).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum InputMode {
    #[default]
    Terminal,
    Prefix,
    Resize,
    Help,
    Prompt {
        kind: PromptKind,
        buffer: String,
    },
    /// y/n confirmation before killing a pane ([ui].confirm_close).
    ConfirmClose(ids::PaneId),
    /// Right-click context menu anchored at a screen cell.
    Menu {
        pane: ids::PaneId,
        x: u16,
        y: u16,
    },
}

/// What a pane close did to the surrounding structure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseOutcome {
    /// Pane removed; its tab lives on.
    PaneRemoved,
    /// The pane was its tab's last — the tab closed too.
    TabClosed,
    /// Cascaded up: the workspace closed.
    WorkspaceClosed,
    /// Nothing left; the app should exit.
    LastClosed,
}

#[derive(Debug)]
pub struct AppState {
    pub workspaces: Vec<Workspace>,
    pub active_workspace: usize,
    pub sidebar_visible: bool,
    pub input_mode: InputMode,
    ids: IdGen,
}

impl AppState {
    /// State with one workspace, one tab, one pane (the initial pane id is returned
    /// via `focused_pane`). Workspaces are named after their folder.
    pub fn new(workspace_name: String) -> Self {
        let mut ids = IdGen::default();
        let pane = ids.pane();
        let tab = Tab::new(ids.tab(), "1".to_string(), pane);
        let ws = Workspace::new(ids.workspace(), workspace_name, tab);
        Self {
            workspaces: vec![ws],
            active_workspace: 0,
            sidebar_visible: true,
            input_mode: InputMode::Terminal,
            ids,
        }
    }

    pub fn active_workspace(&self) -> &Workspace {
        &self.workspaces[self.active_workspace.min(self.workspaces.len() - 1)]
    }

    pub fn active_workspace_mut(&mut self) -> &mut Workspace {
        let i = self.active_workspace.min(self.workspaces.len() - 1);
        &mut self.workspaces[i]
    }

    pub fn active_tab(&self) -> &Tab {
        self.active_workspace().active_tab()
    }

    pub fn active_tab_mut(&mut self) -> &mut Tab {
        self.active_workspace_mut().active_tab_mut()
    }

    pub fn focused_pane(&self) -> PaneId {
        self.active_tab().focused_pane
    }

    /// All pane ids across every workspace and tab.
    #[cfg(test)]
    pub fn all_panes(&self) -> Vec<PaneId> {
        self.workspaces
            .iter()
            .flat_map(|w| w.tabs.iter())
            .flat_map(|t| t.layout.panes())
            .collect()
    }

    /// Split the focused pane; the new pane becomes focused. `before` puts
    /// it on the left/top. Returns its id so the runtime can spawn a PTY.
    pub fn split_focused(&mut self, dir: Dir, before: bool) -> PaneId {
        let new = self.ids.pane();
        let tab = self.active_tab_mut();
        let target = tab.focused_pane;
        tab.layout.split(target, new, dir, before);
        tab.focused_pane = new;
        tab.zoomed = None;
        debug_assert!(self.check_invariants());
        new
    }

    /// Move focus to the geometric neighbor, given current pane rects.
    pub fn focus_neighbor(
        &mut self,
        rects: &[(PaneId, ratatui::layout::Rect)],
        side: Side,
    ) -> bool {
        let tab = self.active_tab_mut();
        match layout::neighbor(rects, tab.focused_pane, side) {
            Some(id) => {
                tab.focused_pane = id;
                true
            }
            None => false,
        }
    }

    /// Close a pane, cascading tab → workspace → app.
    pub fn close_pane(&mut self, pane: PaneId) -> CloseOutcome {
        let wi = self
            .workspaces
            .iter()
            .position(|w| w.tabs.iter().any(|t| t.layout.contains(pane)));
        let Some(wi) = wi else { return CloseOutcome::PaneRemoved };
        let ws = &mut self.workspaces[wi];
        let ti = ws
            .tabs
            .iter()
            .position(|t| t.layout.contains(pane))
            .expect("workspace was just matched");
        let tab = &mut ws.tabs[ti];

        if tab.layout.remove(pane) {
            if tab.focused_pane == pane {
                tab.focused_pane = *tab.layout.panes().first().expect("non-empty after remove");
            }
            if tab.zoomed == Some(pane) {
                tab.zoomed = None;
            }
            debug_assert!(self.check_invariants());
            return CloseOutcome::PaneRemoved;
        }

        // Bare root leaf → the tab closes.
        ws.tabs.remove(ti);
        if ws.tabs.is_empty() {
            self.workspaces.remove(wi);
            if self.workspaces.is_empty() {
                return CloseOutcome::LastClosed;
            }
            self.active_workspace = self.active_workspace.min(self.workspaces.len() - 1);
            debug_assert!(self.check_invariants());
            return CloseOutcome::WorkspaceClosed;
        }
        ws.active_tab = ws.active_tab.min(ws.tabs.len() - 1);
        debug_assert!(self.check_invariants());
        CloseOutcome::TabClosed
    }

    pub fn toggle_zoom(&mut self) {
        let tab = self.active_tab_mut();
        tab.zoomed = if tab.zoomed == Some(tab.focused_pane) {
            None
        } else {
            Some(tab.focused_pane)
        };
    }

    /// New tab in the active workspace; returns the new pane id to spawn.
    pub fn new_tab(&mut self) -> PaneId {
        let pane = self.ids.pane();
        let id = self.ids.tab();
        let ws = self.active_workspace_mut();
        let name = (ws.tabs.len() + 1).to_string();
        ws.tabs.push(Tab::new(id, name, pane));
        ws.active_tab = ws.tabs.len() - 1;
        debug_assert!(self.check_invariants());
        pane
    }

    pub fn next_tab(&mut self) {
        let ws = self.active_workspace_mut();
        ws.active_tab = (ws.active_tab + 1) % ws.tabs.len();
    }

    pub fn prev_tab(&mut self) {
        let ws = self.active_workspace_mut();
        ws.active_tab = (ws.active_tab + ws.tabs.len() - 1) % ws.tabs.len();
    }

    /// New workspace with one tab/pane; becomes active. Returns the pane to spawn.
    pub fn new_workspace(&mut self, name: String) -> PaneId {
        let pane = self.ids.pane();
        let tab = Tab::new(self.ids.tab(), "1".to_string(), pane);
        self.workspaces.push(Workspace::new(self.ids.workspace(), name, tab));
        self.active_workspace = self.workspaces.len() - 1;
        debug_assert!(self.check_invariants());
        pane
    }

    pub fn cycle_workspace(&mut self) {
        self.active_workspace = (self.active_workspace + 1) % self.workspaces.len();
    }

    pub fn jump_tab(&mut self, index: usize) {
        let ws = self.active_workspace_mut();
        if index < ws.tabs.len() {
            ws.active_tab = index;
        }
    }

    /// Resize the focused pane along `axis`; positive delta grows it.
    pub fn resize_focused(&mut self, axis: Dir, delta: f32) -> bool {
        let tab = self.active_tab_mut();
        let target = tab.focused_pane;
        tab.layout.resize(target, axis, delta)
    }

    /// Swap the focused pane with its geometric neighbor, keeping focus on the
    /// moved pane (which now sits in the neighbor's slot).
    pub fn swap_with_neighbor(
        &mut self,
        rects: &[(PaneId, ratatui::layout::Rect)],
        side: Side,
    ) -> bool {
        let tab = self.active_tab_mut();
        let focused = tab.focused_pane;
        match layout::neighbor(rects, focused, side) {
            Some(other) => {
                tab.layout.swap(focused, other);
                true
            }
            None => false,
        }
    }

    pub fn rename_active_tab(&mut self, name: String) {
        self.active_tab_mut().name = name;
    }

    pub fn rename_active_workspace(&mut self, name: String) {
        self.active_workspace_mut().name = name;
    }

    /// Panes of the active tab (for close-tab: kill each, PtyExit cascades).
    pub fn active_tab_panes(&self) -> Vec<PaneId> {
        self.active_tab().layout.panes()
    }

    /// Panes of the active workspace (for close-workspace).
    pub fn active_workspace_panes(&self) -> Vec<PaneId> {
        self.active_workspace().tabs.iter().flat_map(|t| t.layout.panes()).collect()
    }

    /// Workspace/tab indices containing a pane.
    pub fn locate_pane(&self, pane: PaneId) -> Option<(usize, usize)> {
        self.workspaces.iter().enumerate().find_map(|(wi, ws)| {
            ws.tabs
                .iter()
                .position(|t| t.layout.contains(pane))
                .map(|ti| (wi, ti))
        })
    }

    /// Jump straight to a pane wherever it lives (sidebar agent click).
    pub fn focus_pane(&mut self, pane: PaneId) -> bool {
        let Some((wi, ti)) = self.locate_pane(pane) else { return false };
        self.active_workspace = wi;
        self.workspaces[wi].active_tab = ti;
        self.workspaces[wi].tabs[ti].focused_pane = pane;
        true
    }

    /// Invariants, checked in tests and debug builds after every mutation.
    pub fn check_invariants(&self) -> bool {
        let mut seen = std::collections::HashSet::new();
        for ws in &self.workspaces {
            assert!(!ws.tabs.is_empty(), "workspace {} has no tabs", ws.id);
            assert!(ws.active_tab < ws.tabs.len(), "active_tab out of range");
            for tab in &ws.tabs {
                let panes = tab.layout.panes();
                assert!(!panes.is_empty(), "tab {} has no panes", tab.id);
                for p in &panes {
                    assert!(seen.insert(*p), "pane {p} appears twice");
                    assert_ne!(p.0, u64::MAX, "placeholder pane id leaked");
                }
                assert!(
                    panes.contains(&tab.focused_pane),
                    "focused pane {} not in tab {}",
                    tab.focused_pane,
                    tab.id
                );
                if let Some(z) = tab.zoomed {
                    assert!(panes.contains(&z), "zoomed pane {z} not in tab");
                }
            }
        }
        assert!(!self.workspaces.is_empty(), "no workspaces");
        assert!(self.active_workspace < self.workspaces.len());
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_state_is_valid() {
        let s = AppState::new("main".into());
        assert!(s.check_invariants());
        assert_eq!(s.all_panes().len(), 1);
    }

    #[test]
    fn split_focus_close_cycle() {
        let mut s = AppState::new("main".into());
        let first = s.focused_pane();
        let second = s.split_focused(Dir::Right, false);
        assert_eq!(s.focused_pane(), second);
        let third = s.split_focused(Dir::Down, false);
        assert_eq!(s.all_panes().len(), 3);

        assert_eq!(s.close_pane(third), CloseOutcome::PaneRemoved);
        assert_eq!(s.focused_pane(), first, "focus falls back to first pane");
        assert_eq!(s.close_pane(second), CloseOutcome::PaneRemoved);
        assert_eq!(s.close_pane(first), CloseOutcome::LastClosed);
    }

    #[test]
    fn tab_close_cascades() {
        let mut s = AppState::new("main".into());
        let first = s.focused_pane();
        let in_tab2 = s.new_tab();
        assert_eq!(s.active_workspace().tabs.len(), 2);
        assert_eq!(s.close_pane(in_tab2), CloseOutcome::TabClosed);
        assert_eq!(s.active_workspace().tabs.len(), 1);
        assert_eq!(s.close_pane(first), CloseOutcome::LastClosed);
    }

    #[test]
    fn zoom_toggles_and_clears_on_split() {
        let mut s = AppState::new("main".into());
        s.split_focused(Dir::Right, false);
        s.toggle_zoom();
        assert_eq!(s.active_tab().zoomed, Some(s.focused_pane()));
        s.toggle_zoom();
        assert_eq!(s.active_tab().zoomed, None);
        s.toggle_zoom();
        s.split_focused(Dir::Down, false);
        assert_eq!(s.active_tab().zoomed, None, "split un-zooms");
    }

    #[test]
    fn tab_and_workspace_navigation() {
        let mut s = AppState::new("main".into());
        s.new_tab();
        s.new_tab();
        assert_eq!(s.active_workspace().active_tab, 2);
        s.next_tab();
        assert_eq!(s.active_workspace().active_tab, 0);
        s.prev_tab();
        assert_eq!(s.active_workspace().active_tab, 2);

        let p = s.new_workspace("proj".into());
        assert_eq!(s.active_workspace, 1);
        assert_eq!(s.focused_pane(), p);
        s.cycle_workspace();
        assert_eq!(s.active_workspace, 0);
        assert!(s.check_invariants());
    }

    #[test]
    fn close_zoomed_pane_clears_zoom() {
        let mut s = AppState::new("main".into());
        let second = s.split_focused(Dir::Right, false);
        s.toggle_zoom();
        assert_eq!(s.close_pane(second), CloseOutcome::PaneRemoved);
        assert_eq!(s.active_tab().zoomed, None);
    }
}
