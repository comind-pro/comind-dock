//! Pure application state: plain data, constructible in unit tests without
//! PTYs, async, or emulator types. The runtime wraps this.

pub mod ids;
pub mod layout;
pub mod snapshot;
pub mod workspace;

use ids::{IdGen, PaneId};
use layout::{Dir, Side};
use workspace::{Tab, Workspace};

/// What a text prompt is naming.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptKind {
    RenameTab(ids::TabId),
    RenameWorkspace(ids::WorkspaceId),
    /// Branch name for a new worktree of the workspace with this id.
    WorktreeBranch(ids::WorkspaceId),
    /// Name for a new skill (scaffold + catalog + editor).
    NewSkill,
    /// Name for a new profile: None = global, Some(cwd) = workspace-scoped.
    NewProfile(Option<std::path::PathBuf>),
    /// Custom name for a pane / agent session (empty clears it).
    RenamePane(ids::PaneId),
}

/// One context-menu entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuItem {
    pub label: String,
    pub action: MenuAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuAction {
    SplitRight(ids::PaneId),
    SplitLeft(ids::PaneId),
    SplitDown(ids::PaneId),
    SplitUp(ids::PaneId),
    ClosePane(ids::PaneId),
    RenameSpace(ids::WorkspaceId),
    CloseSpace(ids::WorkspaceId),
    NewWorktree(ids::WorkspaceId),
    ListWorktrees(ids::WorkspaceId),
    OpenWorktree(ids::WorkspaceId, std::path::PathBuf),
    /// Open the config file in $EDITOR in a new tab.
    OpenSettings,
    /// Submenu listing agent profiles; None → spawn in a new tab,
    /// Some(pane) → split that pane.
    AgentPicker(Option<ids::PaneId>),
    /// Submenu to pick this space's default agent profile.
    SpaceProfilePicker(ids::WorkspaceId),
    SetSpaceProfile(ids::WorkspaceId, Option<String>),
    StartProfile(String, Option<ids::PaneId>),
    /// Submenu of recent Claude Code sessions on the system.
    ContinuePicker,
    /// Resume conversation `id` in a space anchored at its folder;
    /// the third field is the CLAUDE_CONFIG_DIR profile (None = default).
    ResumeClaudeSession(String, std::path::PathBuf, Option<std::path::PathBuf>),
    /// Open the profiles directory in $EDITOR.
    EditProfiles,
    /// Modal browser: profiles list → per-profile actions.
    ProfileBrowser,
    ProfileMenu(String),
    /// Open one profile file in $EDITOR (a new tab): (name, file).
    ProfileEdit(String, &'static str),
    /// Toggle-assign catalog skills to an agent profile.
    ProfileSkills(String),
    ToggleProfileSkill(String, String),
    /// Toast with the resolved launch command.
    ProfileInfo(String),
    /// Modal browser: skill catalog (name + source, read-only).
    SkillBrowser,
    /// Open a skill's source path in $EDITOR (a new tab).
    SkillEdit(String),
    /// Prompt for a new skill name.
    SkillNew,
    /// Prompt for a new profile name (None = global, Some = for that space).
    ProfileNew(Option<std::path::PathBuf>),
    /// Options menu for an agent pane (sidebar right-click).
    AgentOptions(ids::PaneId),
    /// Jump to a pane wherever it lives.
    FocusPane(ids::PaneId),
    /// Prompt for a custom pane/agent name.
    RenamePane(ids::PaneId),
    /// Pick a behavior (global or space-scoped profile) for an agent pane.
    BehaviorPicker(ids::PaneId),
    /// Inject the behavior into the running session; None clears the mark.
    SetBehavior(ids::PaneId, Option<String>),
    /// Submenu to pick the default editor (persisted into config.toml).
    EditorPicker,
    SetEditor(String),
    /// The prefix+? keybinding overlay.
    ShowKeybinds,
    ReloadConfig,
    /// Download the new release and live-handoff into it (a visible tab).
    RunUpdate,
    Detach,
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
    /// Scrollback search: typing the query.
    Search {
        buffer: String,
    },
    /// Scrollback search: hopping between matches (n / N).
    SearchNav,
    /// y/n confirmation before killing a pane ([ui].confirm_close).
    ConfirmClose(ids::PaneId),
    /// Context menu anchored at a screen cell.
    Menu {
        x: u16,
        y: u16,
        items: Vec<MenuItem>,
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

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct AppState {
    // Every field defaults: this struct crosses the exec-handoff boundary
    // between VERSIONS — a missing field must never abort an upgrade.
    #[serde(default)]
    pub workspaces: Vec<Workspace>,
    #[serde(default)]
    pub active_workspace: usize,
    #[serde(default = "default_true")]
    pub sidebar_visible: bool,
    /// Folder-scoped attach (`cdock -f`): only workspaces under this folder
    /// show. Owned by the CLIENT, not the session — the server swaps the
    /// attached client's scope in before it renders or handles its input
    /// (server::enter/leave), so a second plain attach no longer widens a
    /// scoped view. Never persisted; the client re-sends its folder in Hello
    /// after a reconnect.
    #[serde(skip)]
    pub scope: Option<std::path::PathBuf>,
    /// Modal UI state — never persisted (handoff/restore resets to Terminal).
    #[serde(skip)]
    pub input_mode: InputMode,
    /// User-given names for panes (agent sessions). A custom name always
    /// wins over the agent's own OSC title. Crosses the handoff with the
    /// state; persisted per pane in the snapshot.
    #[serde(default)]
    pub pane_names: std::collections::HashMap<PaneId, String>,
    #[serde(default)]
    ids: IdGen,
}

fn default_true() -> bool {
    true
}

impl AppState {
    /// State with one workspace, one tab, one pane (the initial pane id is returned
    /// via `focused_pane`). Workspaces are named after their folder.
    pub fn new(workspace_name: String, cwd: std::path::PathBuf) -> Self {
        let mut ids = IdGen::default();
        let pane = ids.pane();
        let tab = Tab::new(ids.tab(), "1".to_string(), pane);
        let ws = Workspace::new(ids.workspace(), workspace_name, cwd, tab);
        Self {
            pane_names: std::collections::HashMap::new(),
            workspaces: vec![ws],
            active_workspace: 0,
            sidebar_visible: true,
            scope: None,
            input_mode: InputMode::Terminal,
            ids,
        }
    }

    /// Scope filter: a workspace shows when no scope is set, its folder is
    /// under the scope, or its parent's is (worktree children live outside
    /// the repo folder).
    pub fn in_scope(&self, wi: usize) -> bool {
        let Some(scope) = &self.scope else { return true };
        let ws = &self.workspaces[wi];
        if ws.cwd.starts_with(scope) {
            return true;
        }
        ws.parent
            .and_then(|pid| self.workspaces.iter().find(|w| w.id == pid))
            .is_some_and(|p| p.cwd.starts_with(scope))
    }

    /// Folder-scoped attach (`cdock -f`): set the scope, land on a workspace
    /// under the folder (exact cwd match preferred; the active one keeps
    /// focus if it already qualifies), or create one. Returns the new
    /// workspace's pane when one was created — the caller spawns its PTY.
    pub fn attach_scope(&mut self, folder: std::path::PathBuf) -> Option<PaneId> {
        self.scope = Some(folder.clone());
        if self.in_scope(self.active_workspace) {
            return None;
        }
        let found = self
            .workspaces
            .iter()
            .position(|w| w.cwd == folder)
            .or_else(|| (0..self.workspaces.len()).find(|&wi| self.in_scope(wi)));
        if let Some(wi) = found {
            self.active_workspace = wi;
            return None;
        }
        let name = folder
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "/".to_string());
        Some(self.new_workspace(name, folder, None))
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

    /// Split `target` in place WITHOUT moving the user's focus or active
    /// workspace/tab — the automation API splits background panes while the
    /// user types elsewhere. None if the pane is gone.
    pub fn split_pane(&mut self, target: PaneId, dir: Dir) -> Option<PaneId> {
        let (wi, ti) = self.locate_pane(target)?;
        let new = self.ids.pane();
        let tab = &mut self.workspaces[wi].tabs[ti];
        tab.layout.split(target, new, dir, false);
        if tab.zoomed == Some(target) {
            tab.zoomed = None;
        }
        debug_assert!(self.check_invariants());
        Some(new)
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
        self.pane_names.remove(&pane);
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
            // Removing a lower-index workspace shifts everything down —
            // keep the user's active workspace, don't jump to a neighbor.
            if wi < self.active_workspace {
                self.active_workspace -= 1;
            }
            self.active_workspace = self.active_workspace.min(self.workspaces.len() - 1);
            // Never leave the user on a scope-hidden workspace.
            if !self.in_scope(self.active_workspace) {
                match (0..self.workspaces.len()).find(|&i| self.in_scope(i)) {
                    Some(vis) => self.active_workspace = vis,
                    // The last in-scope space closed: widen to everything
                    // rather than strand the user on hidden panes.
                    None => self.scope = None,
                }
            }
            debug_assert!(self.check_invariants());
            return CloseOutcome::WorkspaceClosed;
        }
        // Same shift logic one level down for tabs.
        if ti < ws.active_tab {
            ws.active_tab -= 1;
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
        let wi = self.active_workspace.min(self.workspaces.len() - 1);
        self.new_tab_in(wi, true)
    }

    /// New tab in workspace `wi`; `activate` = also switch the user's view
    /// to it (background API calls must NOT yank the screen mid-typing).
    pub fn new_tab_in(&mut self, wi: usize, activate: bool) -> PaneId {
        let pane = self.ids.pane();
        let id = self.ids.tab();
        let ws = &mut self.workspaces[wi];
        let name = (ws.tabs.len() + 1).to_string();
        ws.tabs.push(Tab::new(id, name, pane));
        if activate {
            ws.active_tab = ws.tabs.len() - 1;
        }
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
    pub fn new_workspace(
        &mut self,
        name: String,
        cwd: std::path::PathBuf,
        parent: Option<ids::WorkspaceId>,
    ) -> PaneId {
        self.new_workspace_full(name, cwd, parent, true)
    }

    /// `activate: false` keeps the user's current view (background API).
    pub fn new_workspace_full(
        &mut self,
        name: String,
        cwd: std::path::PathBuf,
        parent: Option<ids::WorkspaceId>,
        activate: bool,
    ) -> PaneId {
        let pane = self.ids.pane();
        let tab = Tab::new(self.ids.tab(), "1".to_string(), pane);
        let mut ws = Workspace::new(self.ids.workspace(), name, cwd, tab);
        ws.parent = parent;
        // Children group right after their parent; plain spaces go last.
        let insert_at = match parent {
            Some(pid) => self
                .workspaces
                .iter()
                .position(|w| w.id == pid)
                .map(|i| {
                    let mut end = i + 1;
                    while end < self.workspaces.len() && self.workspaces[end].parent == Some(pid) {
                        end += 1;
                    }
                    end
                })
                .unwrap_or(self.workspaces.len()),
            None => self.workspaces.len(),
        };
        self.workspaces.insert(insert_at, ws);
        if activate {
            self.active_workspace = insert_at;
        } else if insert_at <= self.active_workspace && self.workspaces.len() > 1 {
            // Keep the user's view pinned despite the insertion shift.
            self.active_workspace = (self.active_workspace + 1).min(self.workspaces.len() - 1);
        }
        debug_assert!(self.check_invariants());
        pane
    }

    /// Rename by the folder — only while the user hasn't renamed manually.
    pub fn auto_rename_workspace(&mut self, wi: usize, name: String) {
        if let Some(ws) = self.workspaces.get_mut(wi)
            && !ws.custom_name && ws.name != name {
                ws.name = name;
            }
    }

    /// Panes of any workspace by index (close-space from the menu).
    /// Workspace index by public id — menu/prompt actions hold ids because
    /// indexes shift whenever a workspace closes underneath an open modal.
    pub fn workspace_index(&self, id: ids::WorkspaceId) -> Option<usize> {
        self.workspaces.iter().position(|w| w.id == id)
    }

    pub fn workspace_panes(&self, wi: usize) -> Vec<PaneId> {
        self.workspaces
            .get(wi)
            .map(|w| w.tabs.iter().flat_map(|t| t.layout.panes()).collect())
            .unwrap_or_default()
    }

    /// Next workspace, skipping any outside the attach scope.
    pub fn cycle_workspace(&mut self) {
        let n = self.workspaces.len();
        for step in 1..=n {
            let wi = (self.active_workspace + step) % n;
            if self.in_scope(wi) {
                self.active_workspace = wi;
                return;
            }
        }
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

    /// Rename by id — prompts resolve at Enter time, indexes may shift.
    /// Set (or clear, when empty) a pane's user-given name.
    pub fn rename_pane(&mut self, pane: PaneId, name: String) {
        let name = name.trim().to_string();
        if name.is_empty() {
            self.pane_names.remove(&pane);
        } else {
            self.pane_names.insert(pane, name);
        }
    }

    /// The user's name for a pane, if any.
    pub fn pane_name(&self, pane: PaneId) -> Option<&str> {
        self.pane_names.get(&pane).map(String::as_str)
    }

    pub fn rename_tab_by_id(&mut self, id: ids::TabId, name: String) {
        for ws in &mut self.workspaces {
            if let Some(t) = ws.tabs.iter_mut().find(|t| t.id == id) {
                t.name = name;
                return;
            }
        }
    }

    pub fn rename_workspace_by_id(&mut self, id: ids::WorkspaceId, name: String) {
        if let Some(ws) = self.workspaces.iter_mut().find(|w| w.id == id) {
            ws.name = name;
            ws.custom_name = true;
        }
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
        // Landing behind another pane's zoom would leave the user typing
        // into an invisible pane (toast/sidebar jumps).
        if self.workspaces[wi].tabs[ti].zoomed.is_some_and(|z| z != pane) {
            self.workspaces[wi].tabs[ti].zoomed = None;
        }
        // An explicit jump to a scope-hidden pane wins over the -f filter:
        // otherwise the sidebar shows no active space and cycling can't
        // ever reach it back.
        if !self.in_scope(wi) {
            self.scope = None;
        }
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
        let s = AppState::new("main".into(), std::path::PathBuf::from("/tmp"));
        assert!(s.check_invariants());
        assert_eq!(s.all_panes().len(), 1);
    }

    #[test]
    fn split_focus_close_cycle() {
        let mut s = AppState::new("main".into(), std::path::PathBuf::from("/tmp"));
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
        let mut s = AppState::new("main".into(), std::path::PathBuf::from("/tmp"));
        let first = s.focused_pane();
        let in_tab2 = s.new_tab();
        assert_eq!(s.active_workspace().tabs.len(), 2);
        assert_eq!(s.close_pane(in_tab2), CloseOutcome::TabClosed);
        assert_eq!(s.active_workspace().tabs.len(), 1);
        assert_eq!(s.close_pane(first), CloseOutcome::LastClosed);
    }

    #[test]
    fn zoom_toggles_and_clears_on_split() {
        let mut s = AppState::new("main".into(), std::path::PathBuf::from("/tmp"));
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
        let mut s = AppState::new("main".into(), std::path::PathBuf::from("/tmp"));
        s.new_tab();
        s.new_tab();
        assert_eq!(s.active_workspace().active_tab, 2);
        s.next_tab();
        assert_eq!(s.active_workspace().active_tab, 0);
        s.prev_tab();
        assert_eq!(s.active_workspace().active_tab, 2);

        let p = s.new_workspace("proj".into(), std::path::PathBuf::from("/tmp"), None);
        assert_eq!(s.active_workspace, 1);
        assert_eq!(s.focused_pane(), p);
        s.cycle_workspace();
        assert_eq!(s.active_workspace, 0);
        assert!(s.check_invariants());
    }

    #[test]
    fn folder_scope_attach_filters_and_creates() {
        let mut s = AppState::new("a".into(), std::path::PathBuf::from("/proj/a"));
        assert!(s.in_scope(0), "no scope: everything visible");

        // Scope to a folder with no matching workspace: one gets created.
        let pane = s.attach_scope(std::path::PathBuf::from("/proj/b"));
        assert!(pane.is_some());
        assert_eq!(s.active_workspace().cwd, std::path::PathBuf::from("/proj/b"));
        assert!(!s.in_scope(0));
        assert!(s.in_scope(1));

        // Re-attach with the same scope: active already in scope, no new space.
        assert!(s.attach_scope(std::path::PathBuf::from("/proj/b")).is_none());
        assert_eq!(s.workspaces.len(), 2);

        // Cycling skips out-of-scope spaces.
        s.cycle_workspace();
        assert_eq!(s.active_workspace().cwd, std::path::PathBuf::from("/proj/b"));

        // Worktree child outside the folder follows its in-scope parent.
        let parent = s.active_workspace().id;
        s.new_workspace("wt".into(), std::path::PathBuf::from("/worktrees/x"), Some(parent));
        let wt = s.active_workspace;
        assert!(s.in_scope(wt));

        // Plain attach widens back.
        s.scope = None;
        assert!(s.in_scope(0));
    }

    #[test]
    fn scope_attach_focuses_existing_workspace() {
        let mut s = AppState::new("a".into(), std::path::PathBuf::from("/proj/a"));
        s.new_workspace("b".into(), std::path::PathBuf::from("/proj/b/sub"), None);
        s.attach_scope(std::path::PathBuf::from("/proj/a"));
        assert!(s.attach_scope(std::path::PathBuf::from("/proj/b")).is_none(), "prefix match reused");
        assert_eq!(s.active_workspace().cwd, std::path::PathBuf::from("/proj/b/sub"));
    }

    #[test]
    fn close_zoomed_pane_clears_zoom() {
        let mut s = AppState::new("main".into(), std::path::PathBuf::from("/tmp"));
        let second = s.split_focused(Dir::Right, false);
        s.toggle_zoom();
        assert_eq!(s.close_pane(second), CloseOutcome::PaneRemoved);
        assert_eq!(s.active_tab().zoomed, None);
    }
}
