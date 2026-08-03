# Cline Agent Support Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Recognize the Cline CLI (`cline`) as an agent in comind-dock with screen-based working/blocked/idle status and a hook that captures the session id for `cline --id` resume.

**Architecture:** Two independent layers. Status comes from a bundled screen-detection manifest (`cline.toml`), like codex/opencode — because Cline's hooks have no approval event and a hook `working` report would mask a screen-detected blocked. A single `TaskStart`/`TaskResume` hook (installed by `cdock integration install cline`) forwards the event payload to `cdock hook cline-session`, which records the Cline session id on the pane for exact resume.

**Tech Stack:** Rust (edition 2024), the existing `detect` substring manifest engine, the existing `report-agent`/`ReportAgentSession` runtime API, clap CLI.

## Global Constraints

- Toolchain 1.96.1, edition 2024. Version floor v0.5.1.
- `cargo clippy --all-targets` clean AND `cargo test` green before every commit.
- ALL runtime/server tests use `cdock-dev` (isolated namespace) — NEVER the production session. Build the symlink once: `cargo build && ln -sf cdock target/debug/cdock-dev`.
- Hook/install tests point `HOME` at a throwaway dir — never touch the real `~/.cline` or `~/.claude`.
- Cline reference facts (from `cline/cline` source): file hooks live in `~/.cline/hooks/`, named by exact base name (`TaskStart`, `TaskResume`, …), executable, discovered run-all. Payload is JSON on stdin; discriminator `hookName`; session id at `sessionContext.rootSessionId` (fallback top-level `taskId`). Hooks are DISABLED under `--yolo`. Approval prompt on screen: `Approve tool "<tool>" with input <preview>? [y/N]`. Cline runs as `node` (npm), so identity is via the OSC title, not the exe path.

---

### Task 1: Identity — register `cline` and its resume command

**Files:**
- Modify: `src/agents.rs` (the `KNOWN` array; `resume_command`)
- Test: `src/agents.rs` `#[cfg(test)] mod tests`

**Interfaces:**
- Produces: `KNOWN` contains `"cline"`; `resume_command("cline:<id>") == "cline --id <id>"`; `resume_command("cline") == "cline"`.

- [ ] **Step 1: Write the failing test** — add to `mod tests` in `src/agents.rs`:

```rust
#[test]
fn cline_is_known_and_resumes_by_id() {
    assert_eq!(detect("Cline", "node"), Some("cline"));
    assert_eq!(resume_command("cline:ses_42"), "cline --id ses_42");
    assert_eq!(resume_command("cline"), "cline");
}
```

- [ ] **Step 2: Run it, verify it fails**

Run: `cargo test cline_is_known_and_resumes_by_id`
Expected: FAIL (`detect` returns `None`; `resume_command("cline:ses_42")` returns `"cline:ses_42".to_string()` via the `other` arm).

- [ ] **Step 3: Implement** — in `src/agents.rs`:

Add `"cline"` to the `KNOWN` array (append to the list on line 7-10).

In `resume_command`, inside the `split_once(':')` match, add the arm before `other`:

```rust
            "cline" => format!("cline --id {session}"),
```

(The bare `"cline"` case needs no arm — the final `match ident { … other => other.to_string() }` already returns `"cline"`.)

- [ ] **Step 4: Run tests, verify pass**

Run: `cargo test cline_is_known_and_resumes_by_id && cargo test resume_uses_reported_session_id detects_agents_not_shells`
Expected: PASS (existing agent tests still green).

- [ ] **Step 5: Commit**

```bash
git add src/agents.rs
git commit -m "feat(agents): recognize cline; resume via cline --id"
```

---

### Task 2: `Req::ReportAgentSession` carries the agent id

**Files:**
- Modify: `src/api.rs` (the `ReportAgentSession` request variant ~line 61; its handler ~line 352-367)
- Modify: `src/main.rs` (the `HookCmd::ClaudeSession` caller ~line 1124-1131)

**Interfaces:**
- Produces: `Req::ReportAgentSession { pane, session_id, agent, pid }`; the server stores `format!("{agent}:{session_id}")` in `rt.agent_sessions`. Later tasks pass `agent: "cline"`.

**Why:** the handler currently hardcodes `format!("claude:{session_id}")`, which would mislabel a cline session as claude and resume the wrong CLI. The pane's detected `agent` can be `None` when the hook races the first detection poll, so the caller (which knows which CLI it is) supplies the agent, not the server.

- [ ] **Step 1: Write the failing test** — add to `src/api.rs` tests (or the nearest server-request test module); if none exists for this handler, add a focused unit test that builds a runtime with one pane and applies the request. If that harness is heavy, SKIP the unit test here and rely on Task 3's `cline_session_id` test plus the Task 6 E2E — note the skip in the commit. (Do not invent a harness that doesn't exist.)

- [ ] **Step 2: Add the field** — in `src/api.rs`, the `ReportAgentSession` variant, add `agent: String` next to `session_id`.

- [ ] **Step 3: Use it in the handler** — replace line 365:

```rust
            rt.agent_sessions.insert(pane, format!("{agent}:{session_id}"));
```

- [ ] **Step 4: Update the claude caller** — in `src/main.rs` `HookCmd::ClaudeSession` handler, the `Req::ReportAgentSession { … }` construction, add:

```rust
                    agent: "claude".to_string(),
```

- [ ] **Step 5: Build + existing tests**

Run: `cargo build && cargo test`
Expected: compiles; all green (claude path unchanged: `agent="claude"` reproduces `"claude:<id>"`).

- [ ] **Step 6: Commit**

```bash
git add src/api.rs src/main.rs
git commit -m "refactor(api): ReportAgentSession carries the agent id (was hardcoded claude)"
```

---

### Task 3: `cdock hook cline-session` — extract Cline's session id

**Files:**
- Modify: `src/main.rs` (`HookCmd` enum ~line 219; the `Cmd::Hook { … }` match ~line 1103; a new pure helper `cline_session_id`)
- Test: `src/main.rs` tests module

**Interfaces:**
- Consumes: `Req::ReportAgentSession { …, agent, … }` (Task 2).
- Produces: `HookCmd::ClineSession { pid: Option<u32> }`; helper `fn cline_session_id(v: &serde_json::Value) -> Option<String>`.

- [ ] **Step 1: Write the failing test** — add to `src/main.rs` `mod tests`:

```rust
#[test]
fn cline_session_id_reads_root_then_task() {
    use serde_json::json;
    let a = json!({"hookName":"agent_start","taskId":"conv-1",
                   "sessionContext":{"rootSessionId":"ses_9"}});
    assert_eq!(super::cline_session_id(&a).as_deref(), Some("ses_9"));
    // no sessionContext → fall back to taskId
    let b = json!({"hookName":"agent_start","taskId":"conv-2"});
    assert_eq!(super::cline_session_id(&b).as_deref(), Some("conv-2"));
    // neither → None, no panic
    assert_eq!(super::cline_session_id(&json!({})), None);
}
```

- [ ] **Step 2: Run it, verify it fails**

Run: `cargo test cline_session_id_reads_root_then_task`
Expected: FAIL (`cline_session_id` undefined).

- [ ] **Step 3: Implement the helper** — add near the other hook helpers in `src/main.rs`:

```rust
/// Cline file-hook payload → resumable session id. Prefer the stable
/// rootSessionId; fall back to the per-task id.
fn cline_session_id(v: &serde_json::Value) -> Option<String> {
    v["sessionContext"]["rootSessionId"]
        .as_str()
        .or_else(|| v["taskId"].as_str())
        .map(str::to_string)
}
```

- [ ] **Step 4: Add the CLI variant** — in `HookCmd`:

```rust
    /// Cline TaskStart/TaskResume hook: stdin JSON → report session id.
    /// `pid` is the wrapping shell's $PPID (the cline that fired the hook).
    ClineSession { #[arg(long)] pid: Option<u32> },
```

- [ ] **Step 5: Add the handler** — in the `Cmd::Hook { … }` match, alongside `ClaudeSession`:

```rust
        Cmd::Hook { sub: HookCmd::ClineSession { pid } } => {
            let Some(pane) = std::env::var("CDOCK_PANE_ID").ok().and_then(|p| parse_pane(&p).ok())
            else {
                return Ok(true);
            };
            let mut input = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut input)
                .map_err(|e| e.to_string())?;
            let v: serde_json::Value =
                serde_json::from_str(&input).map_err(|e| format!("bad hook input: {e}"))?;
            let Some(session_id) = cline_session_id(&v) else {
                return Ok(true);
            };
            let _ = api::request_with_timeout(
                &Req::ReportAgentSession {
                    pane,
                    session_id,
                    agent: "cline".to_string(),
                    pid: pid.or_else(|| Some(std::os::unix::process::parent_id())),
                },
                Duration::from_secs(3),
            );
            return Ok(true);
        }
```

- [ ] **Step 6: Run tests, verify pass**

Run: `cargo test cline_session_id_reads_root_then_task && cargo build`
Expected: PASS; compiles.

- [ ] **Step 7: Commit**

```bash
git add src/main.rs
git commit -m "feat(hook): cdock hook cline-session records the cline session id"
```

---

### Task 4: `cdock integration install cline` — write the hook scripts

**Files:**
- Modify: `src/main.rs` (`IntegrationCmd::Install` match ~line 1067; new `install_cline_hook`)
- Test: `src/main.rs` tests module

**Interfaces:**
- Consumes: `HookCmd::ClineSession` (Task 3) — the scripts call `cdock hook cline-session`.
- Produces: `fn install_cline_hook() -> Result<bool, String>`.

- [ ] **Step 1: Write the failing test** — add to `src/main.rs` `mod tests`:

```rust
#[test]
fn install_cline_writes_executable_hooks() {
    use std::os::unix::fs::PermissionsExt;
    let home = std::env::temp_dir().join(format!("cdock-cline-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    // SAFETY: single test, serialized via unique temp path; HOME restored after.
    let prev = std::env::var("HOME").ok();
    unsafe { std::env::set_var("HOME", &home) };
    assert!(super::install_cline_hook().unwrap(), "first install writes");
    assert!(!super::install_cline_hook().unwrap(), "re-install is a no-op");
    for name in ["TaskStart", "TaskResume"] {
        let p = home.join(".cline/hooks").join(name);
        let body = std::fs::read_to_string(&p).unwrap();
        assert!(body.contains("hook cline-session"), "{name}: {body}");
        let mode = std::fs::metadata(&p).unwrap().permissions().mode();
        assert_eq!(mode & 0o111, 0o111, "{name} is executable");
    }
    match prev { Some(v) => unsafe { std::env::set_var("HOME", v) }, None => unsafe { std::env::remove_var("HOME") } }
    std::fs::remove_dir_all(&home).unwrap();
}
```

- [ ] **Step 2: Run it, verify it fails**

Run: `cargo test install_cline_writes_executable_hooks`
Expected: FAIL (`install_cline_hook` undefined).

- [ ] **Step 3: Implement** — add to `src/main.rs`:

```rust
/// Install cline file hooks that report the session id to cdock. Cline
/// discovers hooks by exact base name in ~/.cline/hooks and runs them with
/// the event JSON on stdin. Idempotent (write only when changed). Note:
/// cline disables hooks under --yolo — run with --act/--plan/interactive.
fn install_cline_hook() -> Result<bool, String> {
    use std::os::unix::fs::PermissionsExt;
    let home = std::env::var("HOME").map_err(|_| "HOME unset".to_string())?;
    let dir = std::path::PathBuf::from(home).join(".cline/hooks");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    // $PPID is the cline that fired the hook; our own parent is the sh wrapper.
    const SCRIPT: &str = "#!/bin/sh\n[ -z \"$CDOCK_PANE_ID\" ] || \"$CDOCK_BIN\" hook cline-session --pid \"$PPID\" || true\n";
    let mut wrote = false;
    for name in ["TaskStart", "TaskResume"] {
        let path = dir.join(name);
        if std::fs::read_to_string(&path).is_ok_and(|c| c == SCRIPT) {
            continue;
        }
        std::fs::write(&path, SCRIPT).map_err(|e| e.to_string())?;
        let mut perms = std::fs::metadata(&path).map_err(|e| e.to_string())?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).map_err(|e| e.to_string())?;
        wrote = true;
    }
    if wrote {
        println!("installed cline hooks into {} (run cline with --act/--plan, not --yolo)", dir.display());
    }
    Ok(wrote)
}
```

- [ ] **Step 4: Wire the subcommand** — in the `IntegrationCmd::Install` match (~line 1067), change the arm:

```rust
                "claude" => install_claude_hook(),
                "cline" => install_cline_hook(),
                other => Err(format!("no integration for {other:?} yet (claude, cline)")),
```

- [ ] **Step 5: Run tests, verify pass**

Run: `cargo test install_cline_writes_executable_hooks && cargo clippy --all-targets`
Expected: PASS; clippy clean.

- [ ] **Step 6: Commit**

```bash
git add src/main.rs
git commit -m "feat(integration): cdock integration install cline (session-id hooks)"
```

---

### Task 5: Evidence capture — real Cline screens (BLOCKS Task 6)

**Files:** none (discovery task). Records concrete substrings for Task 6.

**Prereq (user):** `npm i -g cline`, then in a **cdock-dev** pane run `cline -i` (interactive; NOT `--yolo`).

- [ ] **Step 1:** `./target/debug/cdock-dev agent list` → note the cline pane id. Confirm its `agent` field is `"cline"`. If it is NOT recognized, the OSC-title identity assumption failed (Risk 1) — capture the pane title with `./target/debug/cdock-dev agent explain <pane>` (the `title` field) and STOP; the design needs an identity fix before continuing.

- [ ] **Step 2:** With Cline mid-response, run `./target/debug/cdock-dev agent explain <pane>` and record the exact **working** line (the live spinner/streaming indicator) from `bottom_lines`.

- [ ] **Step 3:** With Cline at rest (input prompt shown), record the exact **idle** line(s).

- [ ] **Step 4:** Ask Cline to run a shell command so the `Approve tool "…"? [y/N]` prompt appears; record the exact **blocked** line(s). Confirm `approve tool` (case-insensitive) is present.

- [ ] **Step 5 (integration validation):** with `cdock integration install cline` applied to the throwaway/dev HOME and Cline restarted, start a task and confirm `~/.cline/hooks/TaskStart` fired and the pane got a `cline:<id>` resume ident: `./target/debug/cdock-dev agent list` → the pane resumes as cline. If the payload lacked `sessionContext.rootSessionId`, confirm `taskId` was used.

- [ ] **Step 6:** Write the three captured substrings into Task 6 (working / idle / blocked). No commit.

---

### Task 6: Status manifest — `cline.toml` + register in `bundled()`

**Files:**
- Create: `src/detect/manifests/cline.toml`
- Modify: `src/detect/mod.rs` (`bundled()` ~line 196-211; add to the `#[cfg(test)]` count if asserted)
- Test: `src/detect/mod.rs` tests (mirror `claude_states`)

**Interfaces:**
- Consumes: the captured substrings from Task 5 (working `<WORKING>`, idle `<IDLE>`, blocked confirmed as `approve tool`).

- [ ] **Step 1: Create the manifest** — `src/detect/manifests/cline.toml`. The blocked rule is known; replace `<WORKING>` / `<IDLE>` with the exact Task 5 captures:

```toml
# Cline CLI detection rules. Bottom-of-buffer text, case-insensitive.
id = "cline"

# Tool-approval prompt: `Approve tool "…" with input …? [y/N]`.
[[rule]]
priority = 100
state = "blocked"
any_of = ["approve tool"]
none_of = ["esc to interrupt", "esc interrupt"]

[[rule]]
priority = 90
state = "working"
any_of = ["<WORKING>"]

[[rule]]
priority = 50
state = "idle"
any_of = ["<IDLE>"]
```

- [ ] **Step 2: Register it** — in `src/detect/mod.rs` `bundled()`, add to the `include_str!` array:

```rust
        include_str!("manifests/cline.toml"),
```

If `bundled_manifests_parse` asserts `m.len() >= 3`, bump to `>= 4`.

- [ ] **Step 3: Write the test** — add to `src/detect/mod.rs` tests (fill `<WORKING>`/`<IDLE>` with the captures):

```rust
#[test]
fn cline_states() {
    let m: Manifest =
        toml::from_str(include_str!("manifests/cline.toml")).expect("cline manifest parses");
    assert_eq!(
        classify(&m, "", &lines(&["Approve tool \"execute_command\" with input …? [y/N]"])),
        Some(Status::Blocked)
    );
    assert_eq!(classify(&m, "", &lines(&["<WORKING>"])), Some(Status::Working));
    assert_eq!(classify(&m, "", &lines(&["<IDLE>"])), Some(Status::Idle));
    assert_eq!(classify(&m, "", &lines(&["random text"])), None);
}
```

- [ ] **Step 4: Run tests, verify pass**

Run: `cargo test cline_states bundled_manifests_parse && cargo clippy --all-targets`
Expected: PASS; clippy clean.

- [ ] **Step 5: Commit**

```bash
git add src/detect/manifests/cline.toml src/detect/mod.rs
git commit -m "feat(detect): cline status manifest (approve-tool blocked, spinner, idle)"
```

---

### Task 7: Sandboxed end-to-end + docs

**Files:**
- Modify: `README`/agent docs if they enumerate supported agents (grep first: `rtk grep -rn "opencode" README* docs/ 2>/dev/null`)

- [ ] **Step 1:** In cdock-dev with `cline -i` running, drive a full cycle and confirm the sidebar/`agent list` status transitions: idle → working (mid-response) → blocked (on an approval prompt) → working (after approving) → idle. Record the observed statuses.

- [ ] **Step 2:** Confirm resume: kill/restart the pane (dev only) and confirm it relaunches as `cline --id <id>` (integration installed) or `cline` (not installed).

- [ ] **Step 3:** If any doc lists supported agents, add `cline`. Commit docs only if changed.

- [ ] **Step 4: Final gate**

Run: `cargo clippy --all-targets && cargo test`
Expected: clean + green.

- [ ] **Step 5: Commit (if docs changed)**

```bash
git add -A && git commit -m "docs: list cline as a supported agent"
```

---

## Self-Review

**Spec coverage:**
- Identity (KNOWN, resume) → Task 1. ✓
- Status manifest (blocked/working/idle) → Task 6 (+ Task 5 evidence). ✓
- Integration install + `hook cline-session` + session-id resume → Tasks 2-4. ✓
- Evidence-capture protocol → Task 5. ✓
- Risks: OSC-title identity → Task 5 Step 1 gate; `--yolo` → documented in Task 4 script comment + install message; agent_pid None → guard already permits (Task 2 note). ✓
- Out of scope (status hooks, project hooks, cline-hook auto-refresh, history picker) → not planned. ✓

**Placeholder scan:** The only intentional gaps are `<WORKING>`/`<IDLE>` in Task 6, which are runtime-discovered values produced by Task 5 (not lazy TODOs). Every other step has concrete code. The Task 2 unit-test may be skipped if no server-request harness exists — that contingency is stated explicitly with the fallback coverage.

**Type consistency:** `Req::ReportAgentSession { pane, session_id, agent, pid }` defined in Task 2, consumed in Task 3 with `agent: "cline".to_string()` and in the claude caller with `agent: "claude".to_string()`. `cline_session_id` defined and used in Task 3. `install_cline_hook` defined in Task 4 and wired in the same task. `ClineSession { pid }` defined in Task 3, invoked by the script written in Task 4 (`hook cline-session --pid`). Consistent.
