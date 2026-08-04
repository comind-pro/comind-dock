# Shared pane size: the active client wins — design

**Date:** 2026-08-04
**Status:** approved (design)
**Scope:** when several terminals are attached to one cdock session and look at
the SAME pane, size that pane to the most-recently-active client instead of the
smallest one, so a big terminal is no longer capped to a small one's size.

## Problem

cdock is a session server with multiple attached clients (terminals). Each
client renders at its own size, in its own scope/workspace. But a pane is one
PTY with one size — the same process can't be two sizes at once. Today
`render_clients` folds each shared pane to the **smallest** viewer's size
(`server.rs:726`, `s.0.min(size.0)`) so nobody sees a cropped agent. Result: a
large terminal viewing the same pane as a small one is stuck at the small
size and wastes its space.

## Design

Size a shared pane to the client that **last interacted** (key / mouse / paste),
not the smallest. Working in the big terminal makes the pane big; switching to
the small one makes it small. A background (smaller) client renders the larger
emulator into its smaller rect — ratatui crops it (top-left visible) — until it
becomes active again.

A single client is trivially its own "most recent", so it always gets its own
size (no regression). Clients on different tabs/workspaces don't share the pane
at all, so they keep independent sizes exactly as today.

## Components

1. **`Client.last_active: std::time::Instant`** (new field, `server.rs` Client
   struct ~line 47). Initialized to `Instant::now()` when the client attaches
   (`ClientCtl::New`, ~line 244-261 — a fresh client is "active", so it wins on
   first attach). Updated to `Instant::now()` on real input in the
   `ClientMsg::Event` arm (~line 317), for `Key`/`Mouse`/`Paste` events only —
   NOT `Resize` (an OS resize isn't the user working in that client).

2. **`render_clients` fold** (`server.rs:715-733`). Replace the min-merge with
   "last-active wins": iterate clients in ascending `last_active` order and
   `wanted.insert(pane, size)` (overwrite), so the most-recently-active client
   processed last sets each pane's size. Equivalent: for each pane, take the
   size from the client with the greatest `last_active` among those that see it.

3. **Cropping.** `apply_pane_sizes` resizes the PTY to the winner's size; a
   background client whose rect is smaller than that pane renders the larger
   emulator clipped to its rect (ratatui clamps — top-left shown). No panic, no
   new scroll. When it becomes active, its next input flips the winner and the
   PTY resizes to it.

## Data flow

`ClientMsg::Event(Key|Mouse|Paste)` → set that client's `last_active = now()` →
next `render_clients`: Pass 1 folds pane sizes by `last_active` (winner = newest)
→ Pass 2 `apply_pane_sizes` resizes each shared PTY to its winner → Pass 3 each
client draws its own view (background clients clip the oversized emulator).

## Testing

- Rewrite `src/ui/mod.rs::shared_pane_takes_the_smallest_viewers_size` (it
  currently asserts the min). New `last_active`-wins test can't live in
  `ui/mod.rs` (the fold is in `server.rs`, and `pane_sizes` itself is unchanged)
  — instead:
  - Keep a `pane_sizes` unit test in `ui/mod.rs` asserting the per-view sizes are
    computed from each client's own area (unchanged behavior).
  - Add a focused test of the FOLD in `server.rs`: given two `(area, last_active)`
    inputs for the same pane, the folded size equals the newer client's. Extract
    the fold into a small pure helper `fold_pane_sizes(inputs) -> HashMap` so it's
    testable without a live `Client`/PTY (the current inline fold isn't). If
    extracting is disproportionate, cover it with the sandboxed cdock-dev visual
    check below and note the skip.
- Sandboxed E2E (cdock-dev): attach two clients of different sizes to the same
  pane; confirm typing in the big one sizes the pane big (small one crops), and
  typing in the small one sizes it small.

## Risks

- **PTY resize churn.** Switching the active client between different-sized
  terminals resizes the shared PTY each time; agents (claude/cline) reflow on
  resize. Acceptable — it only happens when differently-sized clients view the
  same pane, which is exactly the case the user wants fixed. (A debounce could
  layer on later; out of scope.)
- **Tie on `last_active`.** Two clients with the same instant (extremely
  unlikely at `Instant` resolution) — iteration order breaks the tie
  arbitrarily; harmless.
- **Instant availability.** This is server runtime code (not a workflow script),
  so `Instant::now()` is fine.

## Out of scope (YAGNI)

- Config option for smallest/largest/active policy (active is the chosen default;
  a config knob can layer on later if anyone wants smallest back).
- Resize debounce.
- Per-client independent reflow of the same PTY (impossible — one PTY, one size).
