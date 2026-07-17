# Drag-and-drop: tabs ↔ panes — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Mouse drag-and-drop: drag a tab onto a pane (grafts the tab's whole layout), drag a pane onto another pane (edge = split, center = swap) or onto the tab bar (existing tab = move into it, `+`/empty = new tab), with live drop-zone highlighting.

**Architecture:** Pure-state tree/move operations in `state/` (TDD-able without PTYs), two new `MouseDrag` variants carrying a hover target in `runtime/`, gesture wiring in `input/mouse.rs`, highlight overlay in `ui/`. Follows the existing compute-view → hit-test-last-view → drag-state patterns exactly.

**Tech Stack:** Rust, ratatui, crossterm. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-07-17-drag-drop-tabs-panes-design.md`

## Global Constraints

- NEVER touch the user's live session. All manual testing via `cdock-dev` only (see CLAUDE.md). Kill only dev-server pids: `pgrep -f "cdock-dev --server"`.
- `cargo clippy --all-targets` clean and `cargo test` green before every commit.
- All state mutations must keep `AppState::check_invariants` passing (it runs in `debug_assert!` after every op).
- All ops validate preconditions before mutating — a `false` return must leave state unchanged (exception: internally a pane may transiently exist in two trees mid-op, but never at return).
- v1 scope: same-workspace only. No cross-workspace moves, no tab reordering, no drag ghost.
- Known limitation (accepted): with `hide_tab_bar_when_single_tab = true` and one tab, the bar is hidden, so pane→new-tab by drag has no drop target.

---

### Task 1: `Node::graft` — insert a subtree at a leaf

**Files:**
- Modify: `src/state/layout.rs` (impl block near `split`, ~line 92; tests module at bottom)

**Interfaces:**
- Consumes: existing `Node`, `Dir`, `Side`, `PaneId`.
- Produces: `pub fn graft(&mut self, at: PaneId, subtree: Node, side: Side) -> bool` on `Node`. Side mapping: `Left → (Dir::Right, subtree first)`, `Right → (Dir::Right, subtree second)`, `Up → (Dir::Down, first)`, `Down → (Dir::Down, second)`. Ratio 0.5. Returns false (tree untouched) if `at` is absent.

- [ ] **Step 1: Write the failing tests** — append to the `tests` module in `src/state/layout.rs`:

```rust
    #[test]
    fn graft_inserts_subtree_on_each_side() {
        // Subtree [3|4] grafted left of leaf 1 in [1|2]:
        // expected pane order (in-order) becomes [3,4], 1, 2.
        let mut n = Node::Leaf(p(1));
        n.split(p(1), p(2), Dir::Right, false);
        let mut sub = Node::Leaf(p(3));
        sub.split(p(3), p(4), Dir::Right, false);
        assert!(n.graft(p(1), sub, Side::Left));
        assert_eq!(n.panes(), vec![p(3), p(4), p(1), p(2)]);

        // Down puts the subtree second.
        let mut n = Node::Leaf(p(1));
        assert!(n.graft(p(1), Node::Leaf(p(2)), Side::Down));
        assert_eq!(n.panes(), vec![p(1), p(2)]);
        let Node::Split { dir, ratio, .. } = &n else { panic!() };
        assert_eq!(*dir, Dir::Down);
        assert!((ratio - 0.5).abs() < 1e-6);

        // Up puts it first.
        let mut n = Node::Leaf(p(1));
        assert!(n.graft(p(1), Node::Leaf(p(2)), Side::Up));
        assert_eq!(n.panes(), vec![p(2), p(1)]);
    }

    #[test]
    fn graft_missing_target_leaves_tree_untouched() {
        let mut n = Node::Leaf(p(1));
        n.split(p(1), p(2), Dir::Right, false);
        let before = n.clone();
        assert!(!n.graft(p(99), Node::Leaf(p(3)), Side::Right));
        assert_eq!(n, before);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test graft`
Expected: FAIL — "no method named `graft`" (compile error).

- [ ] **Step 3: Implement** — in `src/state/layout.rs`, after `split` (~line 92):

```rust
    /// Replace `Leaf(at)` with a 0.5 split of (at, subtree); `side` says
    /// where the subtree lands. Returns false (tree untouched) if absent.
    pub fn graft(&mut self, at: PaneId, subtree: Node, side: Side) -> bool {
        if !self.contains(at) {
            return false;
        }
        self.graft_inner(at, subtree, side);
        true
    }

    fn graft_inner(&mut self, at: PaneId, subtree: Node, side: Side) {
        match self {
            Node::Leaf(_) => {
                let old = std::mem::replace(self, Node::Leaf(PaneId(u64::MAX)));
                let (dir, before) = match side {
                    Side::Left => (Dir::Right, true),
                    Side::Right => (Dir::Right, false),
                    Side::Up => (Dir::Down, true),
                    Side::Down => (Dir::Down, false),
                };
                let (a, b) = if before { (subtree, old) } else { (old, subtree) };
                *self = Node::Split { dir, ratio: 0.5, a: Box::new(a), b: Box::new(b) };
            }
            Node::Split { a, b, .. } => {
                if a.contains(at) {
                    a.graft_inner(at, subtree, side);
                } else {
                    b.graft_inner(at, subtree, side);
                }
            }
        }
    }
```

- [ ] **Step 4: Run tests**

Run: `cargo test graft`
Expected: both tests PASS.

- [ ] **Step 5: Commit**

```bash
git add src/state/layout.rs
git commit -m "feat(layout): Node::graft inserts a subtree at a leaf"
```

---

### Task 2: `AppState::move_tab_into_pane`

**Files:**
- Modify: `src/state/mod.rs` (impl block near `swap_with_neighbor` ~line 509; tests at bottom)

**Interfaces:**
- Consumes: `Node::graft` (Task 1), existing `locate_pane`, `focus_pane`, `check_invariants`.
- Produces: `pub fn move_tab_into_pane(&mut self, src: ids::TabId, target: PaneId, side: Side) -> bool`. Same-workspace only. The src tab's whole layout is grafted at `target`; the src tab disappears; focus lands on the moved tab's `focused_pane`. False + no change when: target pane missing, src tab missing from that workspace, or target lives inside the src tab.

- [ ] **Step 1: Write the failing tests** — append to `tests` in `src/state/mod.rs`:

```rust
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test move_tab`
Expected: FAIL — "no method named `move_tab_into_pane`".

- [ ] **Step 3: Implement** — in `src/state/mod.rs`, after `swap_with_neighbor`:

```rust
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
        ws.tabs[tti].layout.graft(target, moved.layout, side);
        ws.tabs[tti].zoomed = None;
        self.focus_pane(focus);
        debug_assert!(self.check_invariants());
        true
    }
```

- [ ] **Step 4: Run tests**

Run: `cargo test move_tab`
Expected: 3 tests PASS. Also run `cargo test` — full suite green.

- [ ] **Step 5: Commit**

```bash
git add src/state/mod.rs
git commit -m "feat(state): move_tab_into_pane dissolves a tab into a pane split"
```

---

### Task 3: `AppState::move_pane_to_tab` (+ `TabTarget`)

**Files:**
- Modify: `src/state/mod.rs` (types near `CloseOutcome` ~line 137; impl near `move_tab_into_pane`; tests at bottom)

**Interfaces:**
- Consumes: existing `Node::remove`, `Tab::new`, `locate_pane`, `focus_pane`, `self.ids.tab()`.
- Produces:
  - `pub enum TabTarget { Existing(ids::TabId), New }` (derive `Debug, Clone, Copy, PartialEq, Eq`).
  - `pub fn move_pane_to_tab(&mut self, pane: PaneId, dest: TabTarget) -> bool`. `Existing`: pane grafted at the dest tab's root, right side. `New`: pane becomes a fresh tab at the end of its workspace. A source tab left empty closes. No-ops (false): pane missing, dest tab is the pane's own tab, dest tab not in the pane's workspace, or `New` when the pane is already its tab's only pane.

- [ ] **Step 1: Write the failing tests**:

```rust
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test move_pane_to`
Expected: FAIL — `TabTarget` / method not found.

- [ ] **Step 3: Implement** — type after `CloseOutcome` (~line 147):

```rust
/// Where a dragged pane lands on the tab bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabTarget {
    Existing(ids::TabId),
    New,
}
```

Method after `move_tab_into_pane`:

```rust
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
```

Imports: `Node` is already in scope via `layout::Node` (check the file head — `use` items exist for `Tab`, `Node`, `Dir`; add any missing to the existing `use super::` / `use` lines rather than fully qualifying).

- [ ] **Step 4: Run tests**

Run: `cargo test move_pane_to`
Expected: 4 PASS. `cargo test` — full suite green.

- [ ] **Step 5: Commit**

```bash
git add src/state/mod.rs
git commit -m "feat(state): move_pane_to_tab moves a pane into an existing or new tab"
```

---

### Task 4: `AppState::move_pane_onto_pane` + `swap_panes`

**Files:**
- Modify: `src/state/mod.rs` (impl + tests)

**Interfaces:**
- Consumes: `Node::graft` (Task 1), `Node::remove`, `Node::swap`, `locate_pane`, `focus_pane`.
- Produces:
  - `pub fn move_pane_onto_pane(&mut self, pane: PaneId, target: PaneId, side: Side) -> bool` — detach `pane`, graft it on `side` of `target`. Same-workspace only; an emptied source tab closes. False: same pane, either missing, different workspaces.
  - `pub fn swap_panes(&mut self, a: PaneId, b: PaneId) -> bool` — exchange two panes' positions; shape untouched; works same-tab and cross-tab (fixes `focused_pane`/`zoomed` per tab). False when equal or missing.

- [ ] **Step 1: Write the failing tests**:

```rust
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test 'move_pane_onto' && cargo test swap_panes`
Expected: FAIL — methods not found.

- [ ] **Step 3: Implement**:

```rust
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
        ttab.layout.graft(target, Node::Leaf(pane), side);
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
```

- [ ] **Step 4: Run tests**

Run: `cargo test move_pane && cargo test swap_panes`
Expected: PASS. `cargo test` — full suite green.

- [ ] **Step 5: Commit**

```bash
git add src/state/mod.rs
git commit -m "feat(state): move_pane_onto_pane and swap_panes for drag-drop"
```

---

### Task 5: Drag types + zone geometry

**Files:**
- Modify: `src/runtime/mod.rs` (the `MouseDrag` enum, line ~118)
- Modify: `src/ui/mod.rs` (geometry helpers + tests)

**Interfaces:**
- Consumes: `TabId`, `PaneId`, ratatui `Rect`/`Position`.
- Produces, in `src/runtime/mod.rs`:

```rust
pub enum MouseDrag {
    Divider { .. },                                  // unchanged
    Select { pane: PaneId },                         // unchanged
    Tab { id: crate::state::ids::TabId, hover: Option<DropTarget> },
    Pane { pane: PaneId, hover: Option<DropTarget> },
}
pub enum Zone { Left, Right, Up, Down, Center }      // Copy, PartialEq
pub enum TabDrop { Tab(crate::state::ids::TabId), NewTab }
pub enum DropTarget { Zone { pane: PaneId, zone: Zone }, TabBar(TabDrop) }
```

Amended in review (3ee7173): `MouseDrag::Tab`/`Pane` gained an
`origin: (u16, u16)` field (the cell of the arming Down-event) — a 1-cell
trackpad slip during a click must not arm a drop.

- Produces, in `src/ui/mod.rs`: `pub fn zone_at(rect: Rect, pos: Position) -> Zone` (middle 50%×50% = Center, else nearest edge) and `pub fn zone_rect(rect: Rect, zone: Zone) -> Rect` (the half of `rect` to highlight; Center → whole rect).

- [ ] **Step 1: Write the failing tests** — append to `tests` in `src/ui/mod.rs`:

```rust
    #[test]
    fn zone_at_center_and_edges() {
        use crate::runtime::Zone;
        use ratatui::layout::Position;
        let r = Rect::new(10, 10, 40, 20);
        assert_eq!(zone_at(r, Position::new(30, 20)), Zone::Center);
        assert_eq!(zone_at(r, Position::new(11, 20)), Zone::Left);
        assert_eq!(zone_at(r, Position::new(48, 20)), Zone::Right);
        assert_eq!(zone_at(r, Position::new(30, 10)), Zone::Up);
        assert_eq!(zone_at(r, Position::new(30, 29)), Zone::Down);
    }

    #[test]
    fn zone_rect_halves() {
        use crate::runtime::Zone;
        let r = Rect::new(0, 0, 41, 20);
        assert_eq!(zone_rect(r, Zone::Left), Rect::new(0, 0, 20, 20));
        assert_eq!(zone_rect(r, Zone::Right), Rect::new(20, 0, 21, 20));
        assert_eq!(zone_rect(r, Zone::Up), Rect::new(0, 0, 41, 10));
        assert_eq!(zone_rect(r, Zone::Down), Rect::new(0, 10, 41, 10));
        assert_eq!(zone_rect(r, Zone::Center), r);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test zone_`
Expected: FAIL — `Zone` / `zone_at` not found.

- [ ] **Step 3: Implement types** — in `src/runtime/mod.rs`, replace the `MouseDrag` block (~line 117-122) with:

```rust
/// An in-progress mouse drag gesture.
#[derive(Debug, Clone, Copy)]
pub enum MouseDrag {
    Divider { before: PaneId, after: PaneId, dir: Dir, extent: u16, last_pos: u16 },
    Select { pane: PaneId },
    /// A tab being dragged off the bar toward a pane.
    Tab { id: crate::state::ids::TabId, hover: Option<DropTarget> },
    /// A pane grabbed by its border.
    Pane { pane: PaneId, hover: Option<DropTarget> },
}

/// Region of a hovered pane during a drag: four edges plus the center box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zone {
    Left,
    Right,
    Up,
    Down,
    Center,
}

/// Tab-bar landing spot for a dragged pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabDrop {
    Tab(crate::state::ids::TabId),
    NewTab,
}

/// Where a drag would drop right now — drives the highlight and the commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropTarget {
    Zone { pane: PaneId, zone: Zone },
    TabBar(TabDrop),
}
```

Implement geometry — in `src/ui/mod.rs`, after `content_rect`:

```rust
/// Drop zone under `pos` inside `rect`: the middle 50%×50% box is Center,
/// otherwise the nearest edge (normalized distance, ties prefer horizontal).
pub fn zone_at(rect: Rect, pos: ratatui::layout::Position) -> crate::runtime::Zone {
    use crate::runtime::Zone;
    let rx = (pos.x.saturating_sub(rect.x)) as f32 / rect.width.max(1) as f32;
    let ry = (pos.y.saturating_sub(rect.y)) as f32 / rect.height.max(1) as f32;
    if (0.25..0.75).contains(&rx) && (0.25..0.75).contains(&ry) {
        return Zone::Center;
    }
    let (dx, hz) = if rx < 0.5 { (rx, Zone::Left) } else { (1.0 - rx, Zone::Right) };
    let (dy, vz) = if ry < 0.5 { (ry, Zone::Up) } else { (1.0 - ry, Zone::Down) };
    if dx <= dy { hz } else { vz }
}

/// The half of `rect` a zone highlights (Center → the whole rect).
pub fn zone_rect(rect: Rect, zone: crate::runtime::Zone) -> Rect {
    use crate::runtime::Zone;
    let (hw, hh) = (rect.width / 2, rect.height / 2);
    match zone {
        Zone::Left => Rect { width: hw, ..rect },
        Zone::Right => Rect { x: rect.x + hw, width: rect.width - hw, ..rect },
        Zone::Up => Rect { height: hh, ..rect },
        Zone::Down => Rect { y: rect.y + hh, height: rect.height - hh, ..rect },
        Zone::Center => rect,
    }
}
```

- [ ] **Step 4: Run tests + clippy**

Run: `cargo test zone_ && cargo clippy --all-targets`
Expected: tests PASS; clippy clean (the new `MouseDrag` variants are not yet constructed — if clippy/rustc warns dead_code on them, proceed: Task 6 uses them; suppress nothing).

Note: if `cargo build` fails because existing `match rt.drag` arms are non-exhaustive (mouse.rs matches on `Some(...)` patterns with a `None` arm — check), add temporary `Some(MouseDrag::Tab { .. }) | Some(MouseDrag::Pane { .. }) => {}` arms in `src/input/mouse.rs` Drag/Up matches; Task 6 replaces them.

- [ ] **Step 5: Commit**

```bash
git add src/runtime/mod.rs src/ui/mod.rs src/input/mouse.rs
git commit -m "feat(runtime,ui): drag drop-target types and zone geometry"
```

---

### Task 6: Mouse gestures — arm, hover, commit, cancel

**Files:**
- Modify: `src/input/mouse.rs` (Down ~line 71 and ~line 192; Drag ~line 215; Up ~line 262; helpers at bottom)
- Modify: `src/input/mod.rs` (`handle_key`, ~line 132)

**Interfaces:**
- Consumes: state ops from Tasks 2–4, types from Task 5, `ui::zone_at`, existing `pane_at`, `tabbar::hit`, `focus_pane`.
- Produces: working gestures; no new public API beyond two private helpers `drop_target_for_tab` / `drop_target_for_pane` in `mouse.rs`.

- [ ] **Step 1: Update imports** — in `src/input/mouse.rs` line 12:

```rust
use crate::runtime::{DropTarget, InputOutcome, MouseDrag, Runtime, TabDrop, Zone, osc52_bytes};
```

Add to the `use crate::state::...` lines: `TabTarget` (from `crate::state`) and `Side` (extend `use crate::state::layout::Dir;` → `use crate::state::layout::{Dir, Side};`).

- [ ] **Step 2: Arm the tab drag** — in the Down/Left tab-bar branch, replace:

```rust
                    Some(tabbar::Hit::Tab(ti)) => rt.state.jump_tab(ti),
```

with:

```rust
                    Some(tabbar::Hit::Tab(ti)) => {
                        rt.state.jump_tab(ti);
                        // A click stays a click; movement turns it into a drag.
                        if let Some(t) = rt.state.active_workspace().tabs.get(ti) {
                            rt.drag = Some(MouseDrag::Tab { id: t.id, hover: None });
                        }
                    }
```

Amended in review: jump_tab moved from Down to the Up/no-hover arm — Down-switching made tab→pane drops unreachable.

- [ ] **Step 3: Arm the pane drag** — in the Down/Left pane branch, replace:

```rust
                let inner = crate::ui::content_rect(rect);
                if !inner.contains(pos) {
                    return InputOutcome::Continue; // border click: focus only
                }
```

with:

```rust
                let inner = crate::ui::content_rect(rect);
                if !inner.contains(pos) {
                    // Border grab: focus now, and arm a pane drag.
                    rt.drag = Some(MouseDrag::Pane { pane: id, hover: None });
                    return InputOutcome::Continue;
                }
```

- [ ] **Step 4: Hover tracking** — in the `MouseEventKind::Drag(MouseButton::Left)` match, add arms before `None`:

```rust
            Some(MouseDrag::Tab { id, hover }) => {
                let new = drop_target_for_tab(rt, &view, pos, id);
                if new != hover {
                    rt.drag = Some(MouseDrag::Tab { id, hover: new });
                    rt.mark_dirty();
                }
            }
            Some(MouseDrag::Pane { pane, hover }) => {
                let new = drop_target_for_pane(rt, &view, pos, pane);
                if new != hover {
                    rt.drag = Some(MouseDrag::Pane { pane, hover: new });
                    rt.mark_dirty();
                }
            }
```

Amended in review (3ee7173): hover is only computed once the drag has
moved ≥2 cells from `origin` (Chebyshev distance) — below that threshold
`hover` stays `None`, so a click-sized wiggle can't arm a graft/move.

- [ ] **Step 5: Commit on Up** — in the `MouseEventKind::Up(MouseButton::Left)` match, add arms before `None` (drop the temporary arms from Task 5 if added):

```rust
            Some(MouseDrag::Tab { id, hover }) => {
                // Ids re-validate inside the state op — the view is a frame
                // old and the tab/pane may be gone.
                if let Some(DropTarget::Zone { pane, zone }) = hover
                    && let Some(side) = zone_side(zone)
                {
                    rt.state.move_tab_into_pane(id, pane, side);
                }
                rt.mark_dirty();
            }
            Some(MouseDrag::Pane { pane, hover }) => {
                match hover {
                    Some(DropTarget::Zone { pane: target, zone: Zone::Center }) => {
                        rt.state.swap_panes(pane, target);
                    }
                    Some(DropTarget::Zone { pane: target, zone }) => {
                        if let Some(side) = zone_side(zone) {
                            rt.state.move_pane_onto_pane(pane, target, side);
                        }
                    }
                    Some(DropTarget::TabBar(TabDrop::Tab(id))) => {
                        rt.state.move_pane_to_tab(pane, TabTarget::Existing(id));
                    }
                    Some(DropTarget::TabBar(TabDrop::NewTab)) => {
                        rt.state.move_pane_to_tab(pane, TabTarget::New);
                    }
                    None => {}
                }
                rt.mark_dirty();
            }
```

- [ ] **Step 6: Helpers** — at the bottom of `mouse.rs`, after `pane_at`:

```rust
/// Side a zone splits toward; Center has none.
fn zone_side(zone: Zone) -> Option<Side> {
    match zone {
        Zone::Left => Some(Side::Left),
        Zone::Right => Some(Side::Right),
        Zone::Up => Some(Side::Up),
        Zone::Down => Some(Side::Down),
        Zone::Center => None,
    }
}

/// Tab id of the tab holding `pane`.
fn tab_of(rt: &Runtime, pane: PaneId) -> Option<crate::state::ids::TabId> {
    rt.state.locate_pane(pane).map(|(wi, ti)| rt.state.workspaces[wi].tabs[ti].id)
}

/// Valid landing spot for a dragged TAB at `pos`, if any. Center and the
/// tab's own panes are inert.
fn drop_target_for_tab(
    rt: &Runtime,
    view: &crate::ui::view::View,
    pos: Position,
    src: crate::state::ids::TabId,
) -> Option<DropTarget> {
    let (id, rect) = pane_at(&view.pane_rects, pos)?;
    if tab_of(rt, id) == Some(src) {
        return None;
    }
    let zone = crate::ui::zone_at(rect, pos);
    if zone == Zone::Center {
        return None;
    }
    Some(DropTarget::Zone { pane: id, zone })
}

/// Valid landing spot for a dragged PANE at `pos`: another pane's zone, an
/// existing tab, or the new-tab button / empty bar space.
fn drop_target_for_pane(
    rt: &Runtime,
    view: &crate::ui::view::View,
    pos: Position,
    src: PaneId,
) -> Option<DropTarget> {
    if view.tab_bar.contains(pos) {
        let single = rt
            .state
            .locate_pane(src)
            .map(|(wi, ti)| &rt.state.workspaces[wi].tabs[ti].layout)
            .is_some_and(|l| matches!(l, crate::state::layout::Node::Leaf(_)));
        return match tabbar::hit(rt, pos.x - view.tab_bar.x, view.tab_bar.width) {
            Some(tabbar::Hit::Tab(ti) | tabbar::Hit::CloseTab(ti)) => {
                let t = rt.state.active_workspace().tabs.get(ti)?;
                // Dropping a pane onto its own tab's segment is a no-op.
                if t.layout.contains(src) {
                    return None;
                }
                Some(DropTarget::TabBar(TabDrop::Tab(t.id)))
            }
            // A lone pane moving to "a new tab" would recreate its own tab.
            Some(tabbar::Hit::NewTab) | None if !single => {
                Some(DropTarget::TabBar(TabDrop::NewTab))
            }
            _ => None, // CloseApp / ShowSidebar / lone-pane new-tab
        };
    }
    let (id, rect) = pane_at(&view.pane_rects, pos)?;
    if id == src {
        return None;
    }
    Some(DropTarget::Zone { pane: id, zone: crate::ui::zone_at(rect, pos) })
}
```

- [ ] **Step 7: Esc cancels a drag** — in `src/input/mod.rs`, top of `handle_key` (before the `match rt.state.input_mode.clone()`):

```rust
    // Esc aborts an in-flight tab/pane drag before any modal handling.
    if key.code == KeyCode::Esc
        && matches!(
            rt.drag,
            Some(crate::runtime::MouseDrag::Tab { .. } | crate::runtime::MouseDrag::Pane { .. })
        )
    {
        rt.drag = None;
        rt.mark_dirty();
        return Ok(InputOutcome::Continue);
    }
```

- [ ] **Step 8: Build + full test suite**

Run: `cargo clippy --all-targets && cargo test`
Expected: clean, green. (Gesture paths have no unit tests — the state ops they call are covered by Tasks 1–4; end-to-end check comes in Task 8.)

- [ ] **Step 9: Commit**

```bash
git add src/input/mouse.rs src/input/mod.rs
git commit -m "feat(input): tab and pane drag gestures with drop targets"
```

---

### Task 7: Drop highlighting

**Files:**
- Modify: `src/ui/mod.rs` (`render`, after the toast overlay ~line 84)
- Modify: `src/ui/tabbar.rs` (new `drop_rect` helper next to `hit`)

**Interfaces:**
- Consumes: `MouseDrag`/`DropTarget`/`Zone`/`TabDrop` (Task 5), `zone_rect` (Task 5), `tabbar::segments` (existing, private — `drop_rect` lives in the same file), `rt.theme.accent`.
- Produces: `pub fn drop_rect(rt: &Runtime, target: TabDrop, bar: Rect) -> Option<Rect>` in `tabbar.rs`; highlight drawing inside `ui::render` (no signature change).

- [ ] **Step 1: `tabbar::drop_rect`** — in `src/ui/tabbar.rs`, after `hit` (imports: add `use crate::runtime::TabDrop;`):

```rust
/// Screen rect of the segment a pane drag would drop on — same segment walk
/// as render() and hit(), so the highlight matches the click.
pub fn drop_rect(rt: &Runtime, target: TabDrop, bar: Rect) -> Option<Rect> {
    use unicode_width::UnicodeWidthStr as _;
    let ws = rt.state.active_workspace();
    let mut cursor: u16 = 0;
    for s in segments(rt) {
        let w = s.text.width() as u16;
        let matched = match (target, s.hit) {
            (TabDrop::Tab(id), Some(Hit::Tab(ti))) => {
                ws.tabs.get(ti).is_some_and(|t| t.id == id)
            }
            (TabDrop::NewTab, Some(Hit::NewTab)) => true,
            _ => false,
        };
        if matched {
            let x = bar.x + cursor.min(bar.width);
            let width = w.min(bar.width.saturating_sub(cursor));
            return Some(Rect { x, width, ..bar });
        }
        cursor += w;
    }
    None
}
```

- [ ] **Step 2: Overlay in `ui::render`** — in `src/ui/mod.rs`, after `toast::render(rt, full, frame);` and before the mode overlays, insert:

```rust
    // Drag-drop hover highlight: above panes, below modals.
    if let Some(
        crate::runtime::MouseDrag::Tab { hover: Some(target), .. }
        | crate::runtime::MouseDrag::Pane { hover: Some(target), .. },
    ) = rt.drag
    {
        render_drop_highlight(view, rt, target, frame);
    }
```

And add at the bottom of the file (before the tests module):

```rust
/// Paint the current drop target: an accent-tinted half-pane for edge zones,
/// an accent border for a center swap, reverse-video for a tab-bar segment.
/// set_style keeps the glyphs underneath readable.
fn render_drop_highlight(
    view: &View,
    rt: &Runtime,
    target: crate::runtime::DropTarget,
    frame: &mut Frame,
) {
    use crate::runtime::{DropTarget, Zone};
    use ratatui::style::{Modifier, Style};
    match target {
        DropTarget::Zone { pane, zone } => {
            let Some((_, r)) = view.pane_rects.iter().find(|(id, _)| *id == pane) else {
                return;
            };
            match zone {
                Zone::Center => {
                    let block = ratatui::widgets::Block::bordered()
                        .border_type(ratatui::widgets::BorderType::Rounded)
                        .border_style(Style::new().fg(rt.theme.accent));
                    frame.render_widget(block, *r);
                }
                z => {
                    let zr = zone_rect(*r, z);
                    frame.buffer_mut().set_style(zr, Style::new().bg(rt.theme.accent));
                }
            }
        }
        DropTarget::TabBar(td) => {
            if let Some(zr) = tabbar::drop_rect(rt, td, view.tab_bar) {
                frame.buffer_mut().set_style(zr, Style::new().add_modifier(Modifier::REVERSED));
            }
        }
    }
}
```

Note: if the pinned ratatui version lacks `frame.buffer_mut()`, use `frame.render_widget(ratatui::widgets::Block::new().style(...), zr)` instead — `Block` applies its style to the whole area without erasing glyphs.

- [ ] **Step 3: Build + clippy + tests**

Run: `cargo clippy --all-targets && cargo test`
Expected: clean, green.

- [ ] **Step 4: Commit**

```bash
git add src/ui/mod.rs src/ui/tabbar.rs
git commit -m "feat(ui): drop-zone and tab-bar highlights during drag"
```

---

### Task 8: End-to-end verification (cdock-dev)

**Files:** none (verification only).

**Interfaces:** consumes the whole feature.

- [ ] **Step 1: Build and start an isolated dev server**

```bash
cargo build && ln -sf cdock target/debug/cdock-dev
./target/debug/cdock-dev --server & echo "DEV_PID=$!"
```

- [ ] **Step 2: Prepare a layout** — attach in a real terminal (`./target/debug/cdock-dev`), or script via the automation CLI:

```bash
./target/debug/cdock-dev pane list
./target/debug/cdock-dev pane split --dir right   # adjust to actual CLI syntax (see `cdock-dev --help`)
```

Manual checklist (needs a real mouse in the attached terminal):

1. Two tabs, tab 2 with 2 panes. Drag tab 2's segment down onto a tab-1 pane: edge halves highlight in accent while hovering; center shows nothing; dropping on "down" grafts both panes below; tab 2 disappears; focus lands in the moved subtree.
2. Split a pane, grab one by its **border** (not the divider), drag over its sibling: 4 edge zones highlight as halves; center draws an accent border; drop on center swaps; drop on an edge re-splits.
3. Drag a pane onto the tab bar: hovering another tab reverses its segment — drop moves the pane there and closes the emptied source tab; hovering `+`/empty space reverses `+` — drop creates a new tab with the pane.
4. Cancels: press `Esc` mid-drag (highlight clears, nothing moves); release over a dead zone (own pane, own tab segment, sidebar) — nothing moves.
5. Regressions: plain tab click still switches; divider drag still resizes; text selection inside a pane still works; border click still focuses.

- [ ] **Step 3: Kill ONLY the dev server**

```bash
kill $DEV_PID   # the pid recorded in Step 1 — never pgrep bare "cdock --server"
```

- [ ] **Step 4: Final gate**

Run: `cargo clippy --all-targets && cargo test`
Expected: clean, green.

- [ ] **Step 5: Commit any fixes found during E2E**

```bash
git add -A && git commit -m "fix: drag-drop e2e polish"
```
