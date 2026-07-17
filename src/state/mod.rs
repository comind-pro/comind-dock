//! Pure application state: plain data, constructible in unit tests without
//! PTYs, async, or emulator types. The runtime wraps this.

pub mod ids;
pub mod layout;
pub mod snapshot;
pub mod workspace;

use ids::{IdGen, PaneId};
use layout::{Dir, Node, Side};
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
    /// Tab-bar context menu: new tab / rename / close (the ✕ stays too).
    RenameTab(ids::TabId),
    CloseTab(ids::TabId),
    NewTab,
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
    /// Stop the dock: save the session, every agent goes with it.
    Quit,
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

/// Where a dragged pane lands on the tab bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabTarget {
    Existing(ids::TabId),
    New,
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
        self.workspaces.iter().flat_map(|w| w.tabs.iter()).flat_map(|t| t.layout.panes()).collect()
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
        let wi =
            self.workspaces.iter().position(|w| w.tabs.iter().any(|t| t.layout.contains(pane)));
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
        tab.zoomed =
            if tab.zoomed == Some(tab.focused_pane) { None } else { Some(tab.focused_pane) };
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
            && !ws.custom_name
            && ws.name != name
        {
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

    /// Drag-drop: dissolve tab `src` into the tab holding `target`, grafting
    /// its whole layout on `side` of that pane. Same-workspace only.
    pub fn move_tab_into_pane(&mut self, src: ids::TabId, target: PaneId, side: Side) -> bool {
        let Some((wi, tti)) = self.locate_pane(target) else { return false };
        let Some(sti) = self.workspaces[wi].tabs.iter().position(|t| t.id == src) else {
            return false;
        };
        if sti == tti {
            return false; // target lives inside the dragged tab
        }
        let ws = &mut self.workspaces[wi];
        let moved = ws.tabs.remove(sti);
        let tti = if sti < tti { tti - 1 } else { tti };
        if sti < ws.active_tab {
            ws.active_tab -= 1;
        }
        ws.active_tab = ws.active_tab.min(ws.tabs.len() - 1);
        let focus = moved.focused_pane;
        let grafted = ws.tabs[tti].layout.graft(target, moved.layout, side);
        debug_assert!(grafted, "graft target vanished mid-op");
        ws.tabs[tti].zoomed = None;
        self.focus_pane(focus);
        debug_assert!(self.check_invariants());
        true
    }

    /// Drag-drop: move `pane` out of its tab — into an existing tab (grafted
    /// at the root, right side) or a brand-new one. An emptied source tab
    /// closes. Same-workspace only.
    pub fn move_pane_to_tab(&mut self, pane: PaneId, dest: TabTarget) -> bool {
        let Some((wi, ti)) = self.locate_pane(pane) else { return false };
        let src_id = self.workspaces[wi].tabs[ti].id;
        match dest {
            TabTarget::Existing(id) if id == src_id => return false,
            TabTarget::Existing(id)
                if !self.workspaces[wi].tabs.iter().any(|t| t.id == id) =>
            {
                return false;
            }
            _ => {}
        }
        let tab = &mut self.workspaces[wi].tabs[ti];
        let single = matches!(tab.layout, Node::Leaf(_));
        if single && dest == TabTarget::New {
            return false; // already alone in its own tab
        }
        let emptied = !tab.layout.remove(pane);
        if !emptied {
            if tab.focused_pane == pane {
                tab.focused_pane = *tab.layout.panes().first().expect("non-empty after remove");
            }
            if tab.zoomed == Some(pane) {
                tab.zoomed = None;
            }
        }
        match dest {
            TabTarget::Existing(id) => {
                let t = self.workspaces[wi]
                    .tabs
                    .iter_mut()
                    .find(|t| t.id == id)
                    .expect("checked above");
                let old = std::mem::replace(&mut t.layout, Node::Leaf(PaneId(u64::MAX)));
                t.layout = Node::Split {
                    dir: Dir::Right,
                    ratio: 0.5,
                    a: Box::new(old),
                    b: Box::new(Node::Leaf(pane)),
                };
                t.zoomed = None;
            }
            TabTarget::New => {
                let id = self.ids.tab();
                let ws = &mut self.workspaces[wi];
                let name = (ws.tabs.len() + 1).to_string();
                ws.tabs.push(Tab::new(id, name, pane));
            }
        }
        if emptied {
            let ws = &mut self.workspaces[wi];
            let sti = ws.tabs.iter().position(|t| t.id == src_id).expect("still present");
            ws.tabs.remove(sti);
            if sti < ws.active_tab {
                ws.active_tab -= 1;
            }
            ws.active_tab = ws.active_tab.min(ws.tabs.len() - 1);
        }
        self.focus_pane(pane);
        debug_assert!(self.check_invariants());
        true
    }

    /// Drag-drop: detach `pane` and graft it on `side` of `target`. Works
    /// across tabs in one workspace; an emptied source tab closes.
    pub fn move_pane_onto_pane(&mut self, pane: PaneId, target: PaneId, side: Side) -> bool {
        if pane == target {
            return false;
        }
        let Some((swi, sti)) = self.locate_pane(pane) else { return false };
        let Some((twi, tti)) = self.locate_pane(target) else { return false };
        if swi != twi {
            return false;
        }
        let src_id = self.workspaces[swi].tabs[sti].id;
        let tab = &mut self.workspaces[swi].tabs[sti];
        let emptied = !tab.layout.remove(pane);
        if !emptied {
            if tab.focused_pane == pane {
                tab.focused_pane = *tab.layout.panes().first().expect("non-empty after remove");
            }
            if tab.zoomed == Some(pane) {
                tab.zoomed = None;
            }
        }
        let ttab = &mut self.workspaces[twi].tabs[tti];
        let grafted = ttab.layout.graft(target, Node::Leaf(pane), side);
        debug_assert!(grafted, "graft target vanished mid-op");
        // A drop must never land hidden behind another pane's zoom.
        ttab.zoomed = None;
        if emptied {
            // The bare-leaf source tab still holds a stale copy — drop it
            // BEFORE focus_pane, which scans tabs front to back.
            let ws = &mut self.workspaces[swi];
            let i = ws.tabs.iter().position(|t| t.id == src_id).expect("still present");
            ws.tabs.remove(i);
            if i < ws.active_tab {
                ws.active_tab -= 1;
            }
            ws.active_tab = ws.active_tab.min(ws.tabs.len() - 1);
        }
        self.focus_pane(pane);
        debug_assert!(self.check_invariants());
        true
    }

    /// Drag-drop center zone: exchange two panes' positions. Shape of both
    /// trees is untouched; focus/zoom references follow the ids.
    pub fn swap_panes(&mut self, a: PaneId, b: PaneId) -> bool {
        if a == b {
            return false;
        }
        let Some((awi, ati)) = self.locate_pane(a) else { return false };
        let Some((bwi, bti)) = self.locate_pane(b) else { return false };
        self.workspaces[awi].tabs[ati].layout.swap(a, b);
        if (awi, ati) != (bwi, bti) {
            self.workspaces[bwi].tabs[bti].layout.swap(a, b);
            for (wi, ti, from, to) in [(awi, ati, a, b), (bwi, bti, b, a)] {
                let t = &mut self.workspaces[wi].tabs[ti];
                if t.focused_pane == from {
                    t.focused_pane = to;
                }
                if t.zoomed == Some(from) {
                    t.zoomed = Some(to);
                }
            }
        }
        debug_assert!(self.check_invariants());
        true
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

    /// Back to the auto name (its index): the tab label then follows the
    /// pane's own name/title again.
    pub fn reset_tab_name(&mut self, id: ids::TabId) {
        for ws in &mut self.workspaces {
            if let Some(i) = ws.tabs.iter().position(|t| t.id == id) {
                ws.tabs[i].name = (i + 1).to_string();
                return;
            }
        }
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
            ws.tabs.iter().position(|t| t.layout.contains(pane)).map(|ti| (wi, ti))
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

    /// `LastClosed` leaves `workspaces` empty — `active_workspace()` /
    /// `focused_pane()` must NOT be called until a new workspace is pushed.
    /// Regression test for the underflow at active_workspace(): calling
    /// `new_workspace` right after `LastClosed` (the only documented-safe
    /// recovery) must restore a normally queryable state.
    #[test]
    fn last_closed_recovers_via_new_workspace() {
        let mut s = AppState::new("main".into(), std::path::PathBuf::from("/tmp"));
        let only = s.focused_pane();
        assert_eq!(s.close_pane(only), CloseOutcome::LastClosed);
        assert!(s.workspaces.is_empty(), "LastClosed empties workspaces");

        let pane = s.new_workspace("main".into(), std::path::PathBuf::from("/tmp"), None);
        assert!(s.check_invariants());
        assert_eq!(s.focused_pane(), pane);
        assert_eq!(s.active_workspace().tabs.len(), 1);
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
        assert!(
            s.attach_scope(std::path::PathBuf::from("/proj/b")).is_none(),
            "prefix match reused"
        );
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

    #[test]
    fn move_tab_into_pane_grafts_whole_tree() {
        let mut s = AppState::new("main".into(), std::path::PathBuf::from("/tmp"));
        let first = s.focused_pane();
        // Tab 2: two panes.
        let t2a = s.new_tab();
        let t2b = s.split_focused(Dir::Right, false);
        let src = s.active_tab().id;

        assert!(s.move_tab_into_pane(src, first, Side::Down));
        let ws = s.active_workspace();
        assert_eq!(ws.tabs.len(), 1, "source tab is gone");
        let panes = ws.tabs[0].layout.panes();
        assert_eq!(panes, vec![first, t2a, t2b], "subtree grafted below");
        assert_eq!(s.focused_pane(), t2b, "focus follows the moved tab's focus");
        assert!(s.check_invariants());
    }

    #[test]
    fn move_tab_into_own_pane_rejected() {
        let mut s = AppState::new("main".into(), std::path::PathBuf::from("/tmp"));
        let pane = s.focused_pane();
        let src = s.active_tab().id;
        assert!(!s.move_tab_into_pane(src, pane, Side::Left));
        assert_eq!(s.active_workspace().tabs.len(), 1);
        assert!(s.check_invariants());
    }

    #[test]
    fn move_tab_fixes_active_tab_index() {
        let mut s = AppState::new("main".into(), std::path::PathBuf::from("/tmp"));
        let first = s.focused_pane();
        s.new_tab();
        s.new_tab();
        // Active = tab 3 (index 2). Move tab 1 (index 0) into tab 3's pane —
        // wait: tab 1 holds `first`. Move tab 2 instead.
        let src = s.active_workspace().tabs[1].id;
        let target = s.active_workspace().tabs[2].layout.panes()[0];
        assert!(s.move_tab_into_pane(src, target, Side::Right));
        assert_eq!(s.active_workspace().tabs.len(), 2);
        assert!(s.active_workspace().tabs.iter().any(|t| t.layout.contains(first)));
        assert!(s.check_invariants());
    }

    #[test]
    fn move_pane_to_new_tab() {
        let mut s = AppState::new("main".into(), std::path::PathBuf::from("/tmp"));
        let first = s.focused_pane();
        let second = s.split_focused(Dir::Right, false);
        assert!(s.move_pane_to_tab(second, TabTarget::New));
        let ws = s.active_workspace();
        assert_eq!(ws.tabs.len(), 2);
        assert_eq!(ws.tabs[0].layout.panes(), vec![first]);
        assert_eq!(ws.tabs[1].layout.panes(), vec![second]);
        assert_eq!(s.focused_pane(), second, "moved pane is focused in its new tab");
        assert!(s.check_invariants());
    }

    #[test]
    fn move_only_pane_to_new_tab_is_noop() {
        let mut s = AppState::new("main".into(), std::path::PathBuf::from("/tmp"));
        let pane = s.focused_pane();
        assert!(!s.move_pane_to_tab(pane, TabTarget::New));
        assert_eq!(s.active_workspace().tabs.len(), 1);
        assert!(s.check_invariants());
    }

    #[test]
    fn move_pane_to_existing_tab_and_source_tab_closes() {
        let mut s = AppState::new("main".into(), std::path::PathBuf::from("/tmp"));
        let first = s.focused_pane();
        let dest = s.active_tab().id;
        let second = s.new_tab(); // its own single-pane tab
        assert!(s.move_pane_to_tab(second, TabTarget::Existing(dest)));
        let ws = s.active_workspace();
        assert_eq!(ws.tabs.len(), 1, "emptied source tab closed");
        assert_eq!(ws.tabs[0].layout.panes(), vec![first, second]);
        assert_eq!(s.focused_pane(), second);
        assert!(s.check_invariants());
    }

    #[test]
    fn move_pane_to_its_own_tab_rejected() {
        let mut s = AppState::new("main".into(), std::path::PathBuf::from("/tmp"));
        let second = s.split_focused(Dir::Right, false);
        let own = s.active_tab().id;
        assert!(!s.move_pane_to_tab(second, TabTarget::Existing(own)));
        assert_eq!(s.active_workspace().tabs.len(), 1);
        assert!(s.check_invariants());
    }

    #[test]
    fn move_pane_onto_pane_same_tab() {
        let mut s = AppState::new("main".into(), std::path::PathBuf::from("/tmp"));
        let first = s.focused_pane();
        let second = s.split_focused(Dir::Right, false);
        // [1|2] → drop 2 above 1 → vertical [2/1].
        assert!(s.move_pane_onto_pane(second, first, Side::Up));
        assert_eq!(s.active_tab().layout.panes(), vec![second, first]);
        let Node::Split { dir, .. } = &s.active_tab().layout else { panic!() };
        assert_eq!(*dir, Dir::Down);
        assert_eq!(s.focused_pane(), second);
        assert!(s.check_invariants());
    }

    #[test]
    fn move_pane_onto_pane_cross_tab_closes_empty_source() {
        let mut s = AppState::new("main".into(), std::path::PathBuf::from("/tmp"));
        let first = s.focused_pane();
        let second = s.new_tab();
        assert!(s.move_pane_onto_pane(second, first, Side::Left));
        let ws = s.active_workspace();
        assert_eq!(ws.tabs.len(), 1);
        assert_eq!(ws.tabs[0].layout.panes(), vec![second, first]);
        assert!(s.check_invariants());
    }

    #[test]
    fn move_pane_onto_itself_rejected() {
        let mut s = AppState::new("main".into(), std::path::PathBuf::from("/tmp"));
        let p = s.focused_pane();
        assert!(!s.move_pane_onto_pane(p, p, Side::Left));
        assert!(s.check_invariants());
    }

    #[test]
    fn swap_panes_same_and_cross_tab() {
        let mut s = AppState::new("main".into(), std::path::PathBuf::from("/tmp"));
        let a = s.focused_pane();
        let b = s.split_focused(Dir::Right, false);
        assert!(s.swap_panes(a, b));
        assert_eq!(s.active_tab().layout.panes(), vec![b, a]);

        // Cross-tab: c in its own tab; focused/zoomed follow the ids.
        let c = s.new_tab();
        s.toggle_zoom(); // zoom c in tab 2
        assert!(s.swap_panes(c, a));
        let ws = s.active_workspace();
        assert!(ws.tabs[0].layout.contains(c));
        assert!(ws.tabs[1].layout.contains(a));
        assert_eq!(ws.tabs[1].focused_pane, a);
        assert_eq!(ws.tabs[1].zoomed, Some(a));
        assert!(!s.swap_panes(a, a));
        assert!(s.check_invariants());
    }

    #[test]
    fn move_pane_to_existing_tab_source_survives() {
        let mut s = AppState::new("main".into(), std::path::PathBuf::from("/tmp"));
        let first = s.focused_pane();
        let dest = s.active_tab().id;
        let a = s.new_tab();
        let b = s.split_focused(Dir::Right, false);
        s.toggle_zoom(); // zoom b in the source tab
        assert!(s.move_pane_to_tab(b, TabTarget::Existing(dest)));
        let ws = s.active_workspace();
        assert_eq!(ws.tabs.len(), 2, "source tab survives");
        assert_eq!(ws.tabs[1].layout.panes(), vec![a], "source keeps the other pane");
        assert_eq!(ws.tabs[1].focused_pane, a, "source focus fell back");
        assert_eq!(ws.tabs[1].zoomed, None, "source zoom cleared");
        assert_eq!(ws.tabs[0].layout.panes(), vec![first, b]);
        assert_eq!(s.focused_pane(), b);
        assert!(s.check_invariants());
    }

    #[test]
    fn move_ops_reject_missing_and_cross_workspace() {
        let mut s = AppState::new("main".into(), std::path::PathBuf::from("/tmp"));
        let a = s.focused_pane();
        let ghost = PaneId(9999);
        assert!(!s.move_pane_onto_pane(a, ghost, Side::Left));
        assert!(!s.move_pane_onto_pane(ghost, a, Side::Left));
        assert!(!s.swap_panes(a, ghost));
        // Cross-workspace move rejected.
        let b = s.new_workspace("w2".into(), std::path::PathBuf::from("/tmp"), None);
        assert!(!s.move_pane_onto_pane(b, a, Side::Left));
        assert!(s.check_invariants());
    }
}
