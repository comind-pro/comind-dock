# Sidebar: agents grouped under their spaces — design

**Date:** 2026-08-04
**Status:** approved (design)
**Scope:** restructure the sidebar so each agent renders nested under its own
space, replacing the current two flat sections ("spaces" then a flat "agents"
list that repeats each agent's space as a third dim line).

## Problem

The sidebar has two sections: `spaces` (workspace + branch·tabs·panes subtitle)
and `agents` (a flat list of every in-scope agent pane, each 3 lines:
name / status·agent·profile / space). With ~11 agents the flat list is long and
the space label repeats on every agent, making it hard to see which agents
belong to which space.

## Design (option A — unified hierarchy)

One `spaces` section. Each space renders as today (dot + name row, then a
branch·tabs·panes subtitle row), and immediately under it come **its** agents,
indented one level deeper. The separate `agents` section is removed, and with
it the per-agent space line (nesting now conveys the space).

```
 ≡ menu                    «

 spaces
 ● comind-dock   master · 2·2
    ○ драг і дроп    idle · claude
 ○ db_service    CP-3171_ · 2·2
    ○ Додати новий  idle · claude @oleh
 ● neryba        2026-07-17
    ○ Write complete  idle · claude
    ⊙ gravity-field   working · claude
 ○ sbu
    ○ у нас зараз    idle · cline
    ○ Заповнити      idle · claude
 ○ projects
 ○ cpgps         4·4
    ⊙ consolidate    working · claude @oleh
    ○ Документувати  idle · claude @oleh
 + new space
 + continue
```

## Decisions (locked)

- **Agent = 2 rows.** Row 1: marker + name (user name → OSC title → agent id,
  `truncate_clean` 16). Row 2 (indented under it): `status · agent @profile`.
  The old third line (space name) is dropped.
- **Space row unchanged.** `space_dot` + name, then the branch·tabs·panes
  subtitle row exactly as today. Shown for **every** in-scope space, including
  empty ones (e.g. `projects`) — always visible.
- **Nesting/indent.** Top-level space indent `  ` (2), its agents `    ` (4).
  Worktree-child space indent `    ` (4), its agents `      ` (6). The existing
  `child = ws.parent.is_some()` indent logic extends by +2 for agents.
- **Markers unchanged.** Space `space_dot` (dim/green/working/blocked). Agent
  row `status_marker` / unseen `★` logic — carried over verbatim.
- **Footers.** `+ new space` then `+ continue`, both at the very bottom (the
  two "+" actions together). `Target::ContinueAgent` is kept.
- **Scope unchanged.** `state.in_scope(wi)` gates both spaces and their agents.
- **Order.** Spaces in `state.workspaces` order; agents within a space in the
  current `tabs → layout.panes()` traversal order.

## Architecture

Only `rows()` in `src/ui/sidebar.rs` changes: the two separate loops (a spaces
loop, then a flat agents loop over all workspaces) merge into one — the spaces
loop, with the agent emission (currently the body of the second loop) moved
inside it, right after each space's subtitle row, iterating only that space's
`tabs → panes`. `render`, `max_scroll`, `hit`, and `clamped_scroll` are
unchanged — they all derive from `rows()`, so nesting, scroll, and click
targeting follow automatically. `Target` enum is unchanged (`Workspace`,
`Pane`, `NewWorkspace`, `AppMenu`, `ContinueAgent`, `CollapseSidebar`).

The `any_agent` / "none yet" branch is removed — there is no standalone agents
section to be empty; a space with no agents simply shows no nested rows.

## Testing

`sidebar.rs` has no tests today; add a focused `rows()` test:
- Build an `AppState` (test constructor) with two spaces, the second holding one
  agent pane.
- Assert the emitted `Row`s put the agent's `Target::Pane` row *after* that
  space's `Target::Workspace` row and *before* the next space's `Workspace` row
  (nesting/ordering), and that its indent is deeper than the space row's.
- Assert an empty space still emits its `Workspace` row (always-visible).
- Assert the flat old "agents" header and per-agent space line are gone.

If constructing agent `PaneRuntime`s in a unit test is too heavy (needs PTY),
fall back to asserting the space/footer structure (`+ new space`, `+ continue`,
no "agents" header) and cover the nesting via a sandboxed cdock-dev visual
check. Decide when writing the test, not now.

## Risks

- **Long spaces push agents off-screen.** With many spaces the list grows; the
  existing scroll handles it. Collapsible spaces (option C) can layer on later
  if needed — out of scope now.
- **`space_dot` already scans agents.** No new cost — the merged loop reuses the
  same `ws.tabs` walk `space_dot` does.

## Out of scope (YAGNI)

- Collapsible / expandable spaces (option C).
- Reordering, filtering, or hiding empty spaces.
- Any change to `hit`, scroll, or `Target`.
