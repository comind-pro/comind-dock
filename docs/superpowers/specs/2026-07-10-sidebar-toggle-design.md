# Sidebar collapse/expand via mouse

Date: 2026-07-10. Status: approved.

## Problem

`Action::ToggleSidebar` (prefix+`b`) already hides/shows the sidebar, but there
is no mouse affordance: nothing to click to collapse it, and once hidden no way
to bring it back without the keyboard.

## Design

1. **Collapse** — `ui/sidebar.rs`: the `≡ menu` row gets a `«` pinned to the
   right edge (`rows()` takes a `width` param for padding). `hit()` also takes
   the column: a click in the last 3 cells of the menu row returns the new
   `Target::CollapseSidebar`; the rest of the row stays `AppMenu`.
2. **Expand** — `ui/tabbar.rs`: when `!state.sidebar_visible`, `segments()`
   prepends a `" ≡  "` segment with the new `Hit::ShowSidebar`. Render and hit
   share `segments()`, so the click works with no extra wiring.
3. **Handling** — `input/mouse.rs`: two new match arms flip
   `rt.state.sidebar_visible` — the same flag the keybinding uses; no new state.

## Known limits

- `hide_tab_bar_when_single_tab = true` (non-default) + collapsed sidebar → no
  `≡` visible; fallback is prefix+`b`.
- A narrow window auto-hides the sidebar while `sidebar_visible` is still true;
  the `≡` is gated on the flag only, so it does not appear there. Keyboard covers it.

## Tests

- Menu-row hit: click on `«` column → `CollapseSidebar`, click on text → `AppMenu`.
- Tab bar hit: `≡` present and clickable only when sidebar hidden.
