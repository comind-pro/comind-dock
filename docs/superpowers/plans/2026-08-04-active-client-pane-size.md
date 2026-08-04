# Active-Client Pane Size Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Size a pane shared by several attached clients to the most-recently-active client, not the smallest, so a large terminal isn't capped to a small one's size.

**Architecture:** Track `last_active: Instant` per `Client`; in `render_clients`, fold each shared pane's size to the client with the greatest `last_active` (extracted into a pure, testable `fold_pane_sizes` helper) instead of the min.

**Tech Stack:** Rust (edition 2024), tokio server loop, ratatui `TestBackend` clients.

## Global Constraints

- Toolchain 1.96.1, edition 2024.
- `cargo clippy --all-targets` clean AND `cargo test` green before commit.
- Runtime/server tests use `cdock-dev` (isolated namespace) ONLY — NEVER the production session. `cargo build && ln -sf cdock target/debug/cdock-dev` (dev symlink already present).
- Behavior invariants: a single client always gets its own size; clients on different tabs/workspaces keep independent sizes (they don't share the pane).

---

### Task 1: Extract the pane-size fold + make it last-active-wins

**Files:**
- Modify: `src/server.rs` — `Client` struct (~47), `ClientCtl::New` init (~251), `ClientMsg::Event` arm (~317), `render_clients` (~715); new `fold_pane_sizes` + its test
- Modify: `src/ui/mod.rs` — the `shared_pane_takes_the_smallest_viewers_size` test (now stale)

**Interfaces:**
- Produces: `fn fold_pane_sizes(clients: &[(std::time::Instant, Vec<(PaneId, (u16, u16))>)]) -> HashMap<PaneId, (u16, u16)>` — per-pane size from the client with the max `Instant`.

- [ ] **Step 1: Write the failing fold test.** In `src/server.rs`, add a `#[cfg(test)] mod tests` (or extend one if present):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::ids::PaneId;
    use std::time::{Duration, Instant};

    #[test]
    fn fold_takes_the_most_recently_active_client() {
        let t0 = Instant::now();
        let older = t0;
        let newer = t0 + Duration::from_millis(10);
        let pane = PaneId(1);
        // older client wants it big, newer client wants it small → newer wins.
        let folded = fold_pane_sizes(&[
            (older, vec![(pane, (120, 40))]),
            (newer, vec![(pane, (80, 24))]),
        ]);
        assert_eq!(folded[&pane], (80, 24), "the active (newer) client sizes the pane");
        // Order of the input slice must not matter — newer still wins.
        let folded = fold_pane_sizes(&[
            (newer, vec![(pane, (80, 24))]),
            (older, vec![(pane, (120, 40))]),
        ]);
        assert_eq!(folded[&pane], (80, 24), "newer wins regardless of slice order");
    }
}
```

- [ ] **Step 2: Run it, verify it fails** — `cargo test fold_takes_the_most_recently_active_client` (undefined).

- [ ] **Step 3: Add `fold_pane_sizes`.** In `src/server.rs`, near `render_clients`:

```rust
/// Per-pane pty size when several clients view it: the most-recently-active
/// client wins (typing in a big terminal sizes the pane big; switching to a
/// small one sizes it small). One client → its own size, trivially.
fn fold_pane_sizes(
    clients: &[(std::time::Instant, Vec<(crate::state::ids::PaneId, (u16, u16))>)],
) -> HashMap<crate::state::ids::PaneId, (u16, u16)> {
    // Ascending activity: the newest client is applied last and overwrites.
    let mut order: Vec<usize> = (0..clients.len()).collect();
    order.sort_by_key(|&i| clients[i].0);
    let mut wanted = HashMap::new();
    for &i in &order {
        for (pane, size) in &clients[i].1 {
            wanted.insert(*pane, *size);
        }
    }
    wanted
}
```

- [ ] **Step 4: Run it, verify it passes** — `cargo test fold_takes_the_most_recently_active_client`.

- [ ] **Step 5: Add `last_active` to `Client`.** In the `Client` struct (~line 47), add:

```rust
    /// When this client last had real input — the shared-pane size follows
    /// the most-recently-active viewer.
    last_active: std::time::Instant,
```

In `ClientCtl::New` (~line 251), add to the struct literal (a fresh client is active — it wins on attach):

```rust
                            last_active: std::time::Instant::now(),
```

- [ ] **Step 6: Stamp activity on real input.** In the `ClientMsg::Event(ev)` arm (~line 317), before the `clients.remove(&id)` line, mark this client active for Key/Mouse/Paste (not Resize):

```rust
                        if matches!(
                            ev,
                            crossterm::event::Event::Key(_)
                                | crossterm::event::Event::Mouse(_)
                                | crossterm::event::Event::Paste(_)
                        ) && let Some(c) = clients.get_mut(&id)
                        {
                            c.last_active = std::time::Instant::now();
                        }
```

- [ ] **Step 7: Use the fold in `render_clients`.** Replace Pass 1's inline min-merge (~lines 718-731). Collect `(last_active, pane_sizes)` per client, keep the `views` for Pass 3, and call `fold_pane_sizes`:

```rust
    let mut per_client: Vec<(std::time::Instant, Vec<(crate::state::ids::PaneId, (u16, u16))>)> =
        Vec::new();
    let mut views: Vec<(ClientId, crate::ui::view::View)> = Vec::new();
    for (id, c) in clients.iter_mut() {
        enter(rt, c);
        let view = ui::compute_view(rt, c.area());
        per_client.push((c.last_active, ui::pane_sizes(&view)));
        views.push((*id, view));
        leave(rt, c);
    }
    let wanted = fold_pane_sizes(&per_client);
    // Pass 2 — one pty resize per pane, at the active client's size.
    rt.apply_pane_sizes(&wanted);
```

(Pass 3 draw loop is unchanged.)

- [ ] **Step 8: Fix the stale `ui/mod.rs` test.** `shared_pane_takes_the_smallest_viewers_size` asserted the old min-fold, which no longer lives in `ui/mod.rs`. Repoint it at what `ui/mod.rs` still owns — `pane_sizes` computes each view's size from its own area — and drop the min-fold assertion:

```rust
    /// pane_sizes reports each client's own view size (the cross-client fold
    /// that used to live here now lives in server::fold_pane_sizes).
    #[test]
    fn pane_sizes_are_per_view() {
        use crate::state::ids::PaneId;
        use crate::ui::view::View;
        let view = |w: u16, h: u16| View {
            tab_bar: Rect::new(0, 0, w, 1),
            sidebar: None,
            pane_rects: vec![(PaneId(1), Rect::new(0, 1, w, h))],
            dividers: Vec::new(),
            focused: PaneId(1),
        };
        let wide = pane_sizes(&view(120, 40))[0].1;
        let narrow = pane_sizes(&view(80, 24))[0].1;
        assert!(narrow.0 < wide.0, "each view sizes from its own area");
    }
```

Delete the old test body (the `HashMap` min-fold and `use std::collections::HashMap`) it replaces.

- [ ] **Step 9: Build + regression**

Run: `cargo build && cargo clippy --all-targets && cargo test`
Expected: compiles, clippy clean, all green (incl. the two new/updated tests).

- [ ] **Step 10: Sandbox visual (cdock-dev).** Controller-driven (needs two attached terminals of different sizes — an agent can't drive two real client attaches). Report DONE; the controller attaches a big and a small client to the same pane and confirms: typing in the big one grows the pane (small crops top-left), typing in the small one shrinks it.

- [ ] **Step 11: Commit**

```bash
git add src/server.rs src/ui/mod.rs
git commit -m "feat(server): shared pane follows the active client's size, not the smallest"
```

---

## Self-Review

**Spec coverage:**
- `Client.last_active` field + init + input stamping → Steps 5-6. ✓
- Fold to most-recently-active (was min) → Steps 3, 7. ✓
- Cropping for background clients → inherent in Pass 3 (unchanged draw into each client's rect); no code needed. ✓
- Single-client / different-tab invariants → fold with one entry returns its size; non-shared panes appear in only one client's list. ✓
- Testing: pure `fold_pane_sizes` test + repointed `ui/mod.rs` test + sandbox E2E → Steps 1-4, 8, 10. ✓
- Key/Mouse/Paste only, not Resize → Step 6. ✓

**Placeholder scan:** No TBDs. The visual E2E (Step 10) is controller-run because two live client attaches can't be scripted by a subagent — stated explicitly, not a vague deferral.

**Type consistency:** `fold_pane_sizes(&[(Instant, Vec<(PaneId,(u16,u16))>)]) -> HashMap<PaneId,(u16,u16)>` defined in Step 3, its input built in Step 7, tested in Step 1. `last_active: Instant` defined Step 5, set Steps 5-6, read Step 7. `PaneId`, `HashMap`, `Instant` all already in `server.rs` scope (HashMap is used by `render_clients`; PaneId via `crate::state::ids`).
