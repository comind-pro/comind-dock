# Cline agent support — design

**Date:** 2026-08-03
**Status:** approved (design), pending evidence capture
**Scope:** add the Cline CLI (`npm i -g cline`, binary `cline`, "Cline CLI 2.0") as a
recognized agent in comind-dock, with screen-based status detection and a
lightweight hook that captures the session id for exact resume.

## Goal

A `cline` pane should:
- be recognized as an agent (sidebar row, agent id `cline`);
- report **working / blocked / idle** status;
- resume its own conversation on restart (`cline --id <session>`).

## Key finding that shaped the design

Cline's hook system (confirmed from the `cline/cline` source, not docs) has **no
approval/permission event**. Its ten file-hook events are `agent_start`,
`agent_resume`, `prompt_submit`, `tool_call` (PreToolUse), `tool_result`
(PostToolUse), `agent_end` (Stop/done), `agent_error`, `agent_abort`,
`session_shutdown`, `pre_compact` (not wired). Its internal session-status model
has no "blocked" state at all (`idle|running|pending|completed|failed|cancelled`).
"Approval" exists only as a hook *output* (`review:true` — the hook telling Cline
to pause), never as an event Cline emits when it pauses for the user.

Consequence: **blocked-waiting-for-approval cannot come from a hook.** Worse, a
hook that reports `working` (long TTL) would *mask* a screen-detected blocked,
because `reported` outranks screen detection in `effective_status` — the exact
masking bug we just fixed for stale Claude installs. Therefore Cline status is
driven by **screen detection**, and hooks are used only for the session id (which
does not touch status and cannot mask anything).

The only Cline-side signal about an approval is on screen:
`Approve tool "<tool>" with input <preview>? [y/N]`.

## Architecture — two independent layers

1. **Status = screen detection** (the codex/opencode model): a bundled
   `cline.toml` manifest. No status hooks.
2. **Session id = one hook**: `cdock integration install cline` drops an
   executable `TaskStart`/`TaskResume` script into Cline's hooks dir; the script
   forwards the event payload to a new `cdock hook cline-session`, which records
   `cline:<rootSessionId>` for the pane. Used only by resume.

The layers are independent: status works with or without the integration
installed; the integration only improves resume fidelity.

## Component 1 — identity (`src/agents.rs`)

- Add `"cline"` to `KNOWN`.
- `resume_command`:
  - `"cline:<id>"` → `cline --id <id>` (exact conversation)
  - bare `"cline"` → `cline` (fresh; Cline has no `--continue`-style picker we
    rely on, and `cline history` is interactive-only)
- Cline installs via npm and runs as `node`, so `detect_process` (exe-path
  component match) will not see `cline`. Identity falls to the interpreter-hosted
  branch (`detect(&title, "")`, OSC-title word match). **This requires Cline to
  set a terminal title containing the word `cline`.** Verified during evidence
  capture (Risk 1). `agent_pid` is left `None` on that branch — same as npm
  Claude — which turns the report-guard off (guard only rejects when both
  `agent_pid` and reporter pid are `Some` and differ), so the session hook's
  report is still accepted.

## Component 2 — status manifest (`src/detect/manifests/cline.toml` + `bundled()`)

Substring rules in the existing engine, priority-ordered like codex/opencode:

- `blocked@100`: `any_of = ["approve tool"]`, with `none_of` spinner markers so a
  stale approval line doesn't re-block a resumed agent. (`approve tool` is the
  distinctive stem of `Approve tool "…"? [y/N]`.) Known from docs; re-confirmed on
  a real screen during capture.
- `working@90`: Cline's live spinner text — **captured from a real screen.**
- `idle@50`: Cline's resting input prompt — **captured from a real screen.**

Register the manifest in `detect::bundled()` via `include_str!`.

## Component 3 — integration (`cdock integration install cline`)

- Add `Cline` handling to the `integration install` command (currently claude-only).
- Write an executable hook script (no extension, `chmod 0755`) named by Cline's
  file-hook convention into `~/.cline/hooks/` (the CLI's default global hooks dir,
  also `--hooks-dir` default):
  - `TaskStart` and `TaskResume`, each:
    ```sh
    [ -z "$CDOCK_PANE_ID" ] || "$CDOCK_BIN" hook cline-session "$CDOCK_PANE_ID" --pid "$PPID" || true
    ```
  - The script inherits the event JSON on its stdin; it execs `cdock`, which reads
    that stdin.
- New subcommand `cdock hook cline-session <pane> --pid <pid>`: read JSON from
  stdin, extract `sessionContext.rootSessionId` (fallback `taskId`), and record
  `cline:<id>` on the pane via the same pane resume-identity field Claude's
  `hook claude-session` writes (Claude derives its id from `$PPID`; Cline's id
  comes from the stdin payload — same destination, different source). `--pid` is
  carried for parity/guarding; with `agent_pid == None` the report is accepted
  regardless.
- Hooks are **disabled by Cline in `--yolo` mode**. Document: to get cdock resume
  fidelity, run Cline interactively or with `--act`/`--plan`, not `--yolo`.
  (Status detection is unaffected — it's screen-based.)
- Idempotent + quiet-refresh: reuse the write-only-if-changed behaviour added in
  v0.5.1 so a re-install / server-start refresh is a no-op when unchanged. The
  0.5.1 startup `refresh_claude_hooks` stays Claude-specific for now; a Cline
  equivalent is out of scope for this iteration (the Cline hook set is tiny and
  rarely changes).

## Evidence capture protocol (blocking for the manifest)

User installs Cline and runs `cline -i` in a **cdock-dev** pane. Agent then:
1. `cdock-dev agent list` → find the cline pane id.
2. `cdock-dev agent explain <pane>` at three moments — mid-response (working),
   at rest (idle), and while an `Approve tool …? [y/N]` prompt is up (blocked) —
   to read the real `bottom_text` and pick invariant substrings.
3. Confirm the OSC title carries `cline` (Risk 1). If not, identity needs a
   different signal and the design returns to that question.
4. With the integration installed, trigger a task and confirm `TaskStart` fires
   and the stdin payload carries `sessionContext.rootSessionId`.

All done against `cdock-dev` / a throwaway `~/.cline` — never the live session.

## Testing

- `detect` unit tests in `cline.toml`'s harness: `Approve tool "x"? [y/N]` →
  `Blocked`; captured working line → `Working`; captured idle line → `Idle`;
  random text → `None`.
- `agents.rs`: `resume_command("cline:ses_1")` → `cline --id ses_1`;
  `resume_command("cline")` → `cline`; `detect("Cline …","node")`/title match.
- `main.rs`: install writes an executable `~/.cline/hooks/TaskStart` with the
  expected command; idempotent second run writes nothing.
- `cdock hook cline-session`: feed a sample JSON payload on stdin → records
  `cline:<rootSessionId>`; malformed/absent id → no-op, no panic.
- Sandboxed E2E: `cline -i` in cdock-dev shows correct status transitions.

## Risks

1. **OSC-title identity.** node-hosted identity depends on Cline setting a title
   containing `cline`. Verified in capture. If absent, fall back to matching the
   node process argv for `cline` (larger change) — decide then, not now.
2. **`--yolo` disables hooks.** Session-id resume degrades to bare `cline`.
   Documented; status still works.
3. **agent_pid None → report guard off.** Matches existing npm-Claude behaviour;
   acceptable.

## Out of scope (YAGNI)

- Status hooks (proven counterproductive for Cline).
- Cline-side `--hooks-dir`/project `.cline/hooks` install (global `~/.cline/hooks`
  is enough).
- Server-start auto-refresh for Cline hooks (tiny, stable set).
- `cline history` picker integration.

## Implementation order

1. Identity (`agents.rs`) + tests — no evidence needed.
2. Capture real screens (user runs `cline -i` in cdock-dev).
3. Manifest (`cline.toml`) + `bundled()` + tests.
4. Integration (`cdock integration install cline` + `cdock hook cline-session`)
   + tests.
5. Sandboxed E2E; then commit.
