# Drag-and-drop: tabs ↔ panes

2026-07-17. Approved in brainstorming session.

## Goal

Mouse drag-and-drop between the tab bar and the pane area, with live
drop-zone highlighting:

1. **Tab → pane**: drag a tab off the tab bar and drop it onto a pane.
   The tab's entire layout tree is grafted into the drop position (the
   tab disappears). Works for multi-pane tabs — the whole subtree moves.
2. **Pane → tab bar**: drag a pane (by its border) onto the tab bar.
   Drop on empty space or the `+` button → the pane becomes a new tab in
   the current workspace. Drop on an existing tab → the pane moves into
   that tab (grafted at the root, right side).
3. **Pane → pane**: drag a pane onto another pane. Edge zones split
   (left/right/up/down); the center zone swaps the two panes. The
   gesture is same-tab by construction (only one tab is visible);
   cross-tab pane moves go through a tab-bar drop. The state ops are
   still written target-agnostic — they take ids, not "the visible
   tab" — so they work cross-tab if a future gesture needs it.

## Interaction design

### Initiating a drag

- **Tab drag**: mouse Down on a tab segment only arms `MouseDrag::Tab`;
  it does not switch tabs. The switch (`jump_tab`) happens on Up, but
  only when no drop occurred — i.e. a plain click (or an aborted drag)
  still switches on release, preserving click semantics. Down must not
  switch: if it did, the dragged tab would become active immediately
  and every pane visible during the drag would belong to it, so
  tab-onto-pane drops could never find a valid target.
- **Pane drag**: mouse Down on a pane's **border ring** (its rect minus
  the 1-cell-inset content area) arms `MouseDrag::Pane`. Dividers are
  hit-tested first, so divider-resize is unaffected. Down inside the
  content area keeps today's behavior (focus + text selection).

### Drop zones

On a hovered pane, the pointer position relative to the pane rect picks
a zone:

- Center box (middle 50% × 50%): **Center** zone.
- Otherwise: nearest edge by normalized distance → **Left / Right / Up /
  Down**.

Zone meaning:

| Source | Edge zone | Center zone |
|---|---|---|
| Tab | graft tab's tree on that side (split 0.5) | inactive (no highlight) |
| Pane | detach + graft on that side | swap the two panes |

On the tab bar (pane drag only): hovering a tab segment targets that
tab; hovering empty space or `+` targets "new tab".

### Highlighting

Rendered as an overlay in `ui::render`, driven by the hover target
stored in the drag state:

- Edge zone: the corresponding half of the target pane tinted with the
  accent background (`set_style` over the area — glyphs stay readable).
- Center zone: accent border around the whole target pane.
- Tab bar: hovered segment rendered `REVERSED`; "new tab" target
  highlights the `+` segment.
- Invalid target (see Rules) → no highlight, drop is a no-op.

### Completing / canceling

- Up over a valid target: re-validate ids against current state (the
  hit-test view is one frame stale; panes can die mid-drag), apply the
  state operation, clear the drag, mark dirty.
- Up over nothing / an invalid target: cancel silently.
- `Esc` key during a drag: cancel.

## Rules (validity)

- A tab cannot be dropped onto a pane belonging to itself (covers the
  single-tab workspace automatically: the only visible panes are its
  own).
- A pane cannot be dropped onto itself, onto its own tab-bar segment
  when it is the only pane of that tab (no-op move), or split against
  itself.
- Dropping the only pane of a tab onto the tab bar is allowed — the
  source tab closes (net effect: tab move/merge).
- Zoomed panes: targets are whatever is visible; dragging a zoomed pane
  is allowed.

## Architecture

Follows the existing compute-view / hit-test / drag-state patterns. No
new modules; four files change meaningfully.

### 1. Tree ops — `src/state/layout.rs`

- `Node::graft(&mut self, at: PaneId, subtree: Node, side: Side) -> bool`
  — replaces `Leaf(at)` with `Split { dir, ratio: 0.5 }`; `side` maps to
  `(dir, order)`: Left/Up put the subtree first, Right/Down second.
  `split()` becomes the `Leaf` special case (or stays as-is; graft is
  additive).
- Detach = existing `remove()` (caller captures the leaf first); for a
  whole tab the tree is simply taken from the `Tab` being deleted.

### 2. State ops — `src/state/mod.rs`

- `move_tab_into_pane(&mut self, src_tab: TabId, target: PaneId, side: Side) -> bool`
  — locate both; reject if `target` is inside `src_tab`; take the src
  tab's `layout`, delete the tab (fix `active_tab`), graft into the
  target tab's tree; focus follows the moved subtree's `focused_pane`.
- `move_pane_to_tab(&mut self, pane: PaneId, dest: TabTarget) -> bool`
  where `TabTarget = Existing(TabId) | New` — detach the leaf from its
  tab (source tab with no panes left closes, reusing `close_pane`'s
  cascade rules but *without* killing the pane); `Existing`: graft at
  the dest root, `Side::Right`; `New`: push `Tab::new(id, name, pane)`
  into the active workspace and activate it.
- `move_pane_onto_pane(&mut self, pane: PaneId, target: PaneId, side: Side) -> bool`
  and center-zone `swap_panes(&mut self, a: PaneId, b: PaneId)` —
  cross-tab swap swaps the ids in both trees (shape untouched) and fixes
  both tabs' `focused_pane`/`zoomed` references.
- All ops keep `check_invariants` green and are pure state (no PTY
  work) — panes keep their PTYs; only tree membership changes.

### 3. Drag state — `src/runtime/mod.rs`

```rust
pub enum MouseDrag {
    Divider { .. },            // existing
    Select { .. },             // existing
    Tab  { id: TabId, hover: Option<DropTarget> },
    Pane { pane: PaneId, hover: Option<DropTarget> },
}
pub enum DropTarget {
    Zone { pane: PaneId, zone: Zone },   // Zone: Left/Right/Up/Down/Center
    TabBar(TabDrop),                     // TabDrop: Tab(TabId) | NewTab
}
```

### 4. Mouse handling — `src/input/mouse.rs`

- Down/tab-bar branch: arm `MouseDrag::Tab` after `jump_tab`.
- Down/pane branch: border-ring hit → arm `MouseDrag::Pane`; else
  existing focus/select path.
- Drag branch: compute hover target from `last_view` (`pane_at` +
  zone math; `tabbar::hit` for the bar), store it, `mark_dirty()`.
- Up branch: validate + dispatch to the state ops above.
- Key handler: `Esc` clears an active `Tab`/`Pane` drag before its
  normal meaning.

### 5. Overlay — `src/ui/mod.rs`

After panes and before mode overlays: if `rt.drag` carries a hover
target, draw the highlight (zone fill / center border / tab-bar
reverse). Geometry comes from the same `View` used for hit-testing.

## Error handling

- Every drop re-validates via `locate_pane` / tab lookup at Up time;
  stale or dead ids → silent cancel.
- Tree ops return `bool`; a `false` mid-operation must not leave a
  half-moved state — ops validate all preconditions before mutating.

## Testing

- Unit, `state/layout.rs`: `graft` on all four sides, graft of a
  multi-pane subtree, shape after graft+remove round-trip.
- Unit, `state/mod.rs`: each move op — same-tab, cross-tab, last-pane
  source tab closes, invalid targets rejected, `check_invariants` after
  every op, `focused_pane`/`zoomed`/`active_tab` fixed up.
- Unit: zone hit-test math (center box vs nearest edge).
- Manual E2E via `cdock-dev` only (per repo rules).

## Non-goals (v1)

- Dragging tabs/panes across **workspaces**.
- Reordering tabs within the tab bar by drag (separate feature).
- Hovering over a tab mid-pane-drag does not switch tabs (dwell-switch);
  cross-tab pane moves go through a tab-bar drop instead.
- Drag "ghost" following the cursor — the highlight is the only visual.
