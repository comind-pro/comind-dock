# Sidebar Grouped Agents Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Render each agent nested under its own space in the sidebar, replacing the two flat sections (`spaces` + a flat `agents` list) with one unified hierarchy.

**Architecture:** A single change to `rows()` in `src/ui/sidebar.rs`: merge the two loops (a spaces loop, then a flat agents loop) into one — emit each space's row + subtitle, then that space's agent rows nested one indent deeper, before moving to the next space. `render`/`max_scroll`/`hit`/`clamped_scroll` all derive from `rows()` and are untouched.

**Tech Stack:** Rust (edition 2024), ratatui `Line`/`Span`, the existing `Runtime`/`AppState`/`Theme`.

## Global Constraints

- Toolchain 1.96.1, edition 2024.
- `cargo clippy --all-targets` clean AND `cargo test` green before commit.
- Testing/visual checks use `cdock-dev` (isolated namespace) ONLY — NEVER the production session. Build once: `cargo build && ln -sf cdock target/debug/cdock-dev` (the dev symlink already exists in this repo).
- `Target` enum stays exactly as-is: `Workspace(usize)`, `Pane(PaneId)`, `NewWorkspace`, `AppMenu`, `ContinueAgent`, `CollapseSidebar`. No new variants.
- Footers order: `+ new space` then `+ continue`, both at the very bottom.

---

### Task 1: Merge the spaces + agents loops in `rows()`

**Files:**
- Modify: `src/ui/sidebar.rs` — `rows()` (~line 80-253); its doc comment (~line 77-79)

**Interfaces:**
- Produces: the same `Vec<Row>` shape (`Row { line, target }`), just reordered — agent rows now interleave under their space instead of in a trailing section. `render`/`hit`/`max_scroll` consume it unchanged.

**Current structure (what to change):**
`rows()` builds: menu row → blank → `" spaces"` header → [per in-scope space: name row + subtitle row] → `"+ new space"` → blank → `" agents"` header → [per in-scope space, per tab, per pane: if agent → name row + `status·agent·profile` row + space row] → `"none yet"` if empty → `"+ continue"`.

**Target structure:**
menu row → blank → `" spaces"` header → [per in-scope space: name row + subtitle row + [per agent pane in that space: marker+name row + `status·agent @profile` row]] → `"+ new space"` → `"+ continue"`.

- [ ] **Step 1: Extract an agent-row helper (keeps the merged loop readable).** In `src/ui/sidebar.rs`, add a helper that emits the 2 rows for one agent pane. Move the existing per-pane rendering logic (from the second loop, ~line 166-238) into it, but DROP the third `space` row. `indent` is the agent indent string for that space (space indent + 2 spaces).

```rust
/// The two rows for one agent pane: marker + name, then the
/// `status · agent @profile` detail line indented under it. `indent` is the
/// leading whitespace for this agent (its space's indent + 2).
fn agent_rows(rt: &Runtime, theme: &Theme, pane: PaneId, indent: &str, out: &mut Vec<Row>) {
    let Some(p) = rt.panes.get(&pane) else { return };
    let Some(agent) = p.agent else { return };
    let state = &rt.state;
    let title = rt.titles.get(&pane).map(String::as_str).unwrap_or("");
    let status = p.effective_status();
    let (dot, dot_style) = match p.unseen {
        Some(crate::runtime::NoticeKind::Done) => {
            ("★ ", Style::new().fg(Color::LightGreen).add_modifier(Modifier::BOLD))
        }
        Some(crate::runtime::NoticeKind::Blocked) => {
            ("★ ", Style::new().fg(Color::LightRed).add_modifier(Modifier::BOLD))
        }
        None => status_marker(status, theme),
    };
    let status = match (p.unseen, p.reported_label()) {
        (_, Some(label)) => label,
        (Some(crate::runtime::NoticeKind::Done), _) => "finished",
        (Some(crate::runtime::NoticeKind::Blocked), _) => "blocked",
        (None, None) => status.word(),
    };
    let name = match state.pane_name(pane) {
        Some(n) => crate::agents::truncate_clean(n, 16),
        None if title.trim().is_empty() => agent.to_string(),
        None => crate::agents::truncate_clean(title, 16),
    };
    let focused = pane == state.focused_pane();
    let name_style = if focused {
        Style::new().fg(theme.accent).add_modifier(Modifier::BOLD)
    } else {
        Style::new().add_modifier(Modifier::BOLD)
    };
    out.push(Row {
        line: Line::from(vec![
            Span::raw(indent.to_string()),
            Span::styled(dot, dot_style),
            Span::styled(name, name_style),
        ]),
        target: Some(Target::Pane(pane)),
    });
    let profile = p
        .agent_config_dir
        .as_deref()
        .and_then(crate::agents::profile_label_from_dir)
        .map(|l| format!(" @{l}"))
        .unwrap_or_default();
    out.push(Row {
        line: Line::from(Span::styled(
            format!("{indent}  {status} · {agent}{profile}"),
            Style::new().fg(theme.muted),
        )),
        target: Some(Target::Pane(pane)),
    });
}
```

- [ ] **Step 2: Emit agents inside the spaces loop.** In the spaces loop (~line 104-144), right AFTER the subtitle-row `out.push(...)`, add the agent emission for that space. `indent` there is `"  "` (top-level) or `"    "` (worktree child); the agent indent is that + `"  "`:

```rust
        let agent_indent = format!("{indent}  ");
        for tab in &ws.tabs {
            for pane in tab.layout.panes() {
                agent_rows(rt, theme, pane, &agent_indent, &mut out);
            }
        }
```

(`agent_rows` early-returns for non-agent panes, so plain shells emit nothing — an empty space shows just its name + subtitle.)

- [ ] **Step 3: Delete the old agents section.** Remove the now-dead block: the blank row + `" agents"` header (~line 150-157), the entire second `for (wi, ws)` agents loop (~line 159-241), the `any_agent` / `"none yet"` block (~line 242-247), and the `let mut any_agent = false;`. KEEP the `"+ new space"` push (it stays after the spaces loop) and the final `"+ continue"` push — reorder so it reads: spaces loop → `"+ new space"` → `"+ continue"` → `out`.

- [ ] **Step 4: Update the `rows()` doc comment** (~line 77-79) to describe the unified structure, e.g.:

```rust
/// Sidebar (mockup): one "spaces" section — each workspace shows a status dot,
/// a git-branch/counts subtitle, and its agent panes nested one indent deeper
/// (marker + name, then a `status · agent @profile` line). Worktree children
/// indent under their parent; their agents indent deeper still.
```

- [ ] **Step 5: Build + regression**

Run: `cargo build && cargo clippy --all-targets && cargo test`
Expected: compiles, clippy clean, all existing tests green (no sidebar unit tests exist; this is a presentation reorg — coverage is the visual check in Step 6).

- [ ] **Step 6: Sandbox visual verification (cdock-dev).** The controller drives this (agents cannot see the TUI chrome via `pane read` — the sidebar is chrome, not pane content). Hand back to the controller: build, `cdock-dev server handoff`, and confirm in the dev window that agents render nested under their spaces, empty spaces show just their row, and `+ new space` / `+ continue` sit at the bottom. Do NOT run the handoff yourself if you are a subagent — report DONE and let the controller do the visual check.

- [ ] **Step 7: Commit**

```bash
git add src/ui/sidebar.rs
git commit -m "feat(sidebar): nest agents under their spaces (one unified hierarchy)"
```

---

## Self-Review

**Spec coverage:**
- Agents nested under spaces, one section → Steps 2-3. ✓
- Agent = 2 rows, space line dropped → Step 1 (`agent_rows` omits the space row). ✓
- Space row + subtitle unchanged, empty spaces always shown → Step 2 (emission is additive; empty spaces emit no agent rows). ✓
- Indent: space +2 for agents; worktree child deeper → Step 2 (`format!("{indent}  ")`). ✓
- Markers (`space_dot`, `status_marker`, unseen ★) unchanged → Step 1 copies the logic verbatim. ✓
- Footers `+ new space` then `+ continue` → Step 3. ✓
- Scope (`in_scope`) unchanged → the spaces loop already guards it; agents inherit it by nesting. ✓
- `hit`/scroll/`Target` unchanged → not touched. ✓

**Placeholder scan:** No TBDs. The test is intentionally a visual check (Step 6), not a unit test — justified by: no Runtime unit harness exists, sidebar has never had unit tests, and this is a pure presentation reorder. The spec's testing section pre-approved this fallback.

**Type consistency:** `agent_rows(rt, theme, pane, indent, out)` defined in Step 1, called in Step 2 with `&agent_indent`. `Row`/`Target`/`Style`/`Span` all already imported in sidebar.rs. `truncate_clean`, `profile_label_from_dir`, `pane_name`, `effective_status`, `reported_label` — all already used by the code being moved.
