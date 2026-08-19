# Process Monitor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** An in-app overlay listing the processes cdock agents spawned — grouped by the pane/agent that launched them (via inherited `CDOCK_PANE_ID`, so orphans still show), with uptime, CPU/mem, and a protected kill.

**Architecture:** A new platform call `cdock_processes()` scans all pids, reads `CDOCK_PANE_ID` from each process's env to attribute it to a pane, and returns `ProcInfo` rows; `system_load()` returns host CPU/mem. The runtime polls these only while a `ProcessMonitor` overlay is open, computes CPU% from two samples, flags orphans, and the overlay renders per-pane groups with ↑/↓ + Enter-to-kill (the pane's own pty child is protected).

**Tech Stack:** Rust (edition 2024), `libc` (already a dep) for macOS `proc_*` / signals, `/proc` on Linux, ratatui overlay.

## Global Constraints

- Toolchain 1.96.1, edition 2024. `cargo clippy --all-targets` clean AND `cargo test` green before every commit.
- ARCHITECTURE rule: core modules contain NO `#[cfg(target_os)]`; all OS specifics live in `src/platform/{unix,mod}.rs` behind shared signatures. A `#[cfg(not(unix))]` stub returns empty.
- Runtime/visual tests use `cdock-dev` (isolated) ONLY, never production. `cargo build && ln -sf cdock target/debug/cdock-dev` (dev symlink present).
- Ownership marker: cdock sets `CDOCK_PANE_ID` in each pane's PTY env (`term/pty.rs:63`); it is inherited by all descendants and survives reparenting. Attribution reads it back via the env-read path (`process_env_var`, `src/platform/unix.rs:149`).
- Kill safety: a pid equal to any pane's `pty.child_pid` (shell/agent) is NEVER selectable or killed.

## File / symbol map

- `src/platform/unix.rs` — new `ProcInfo`, `SystemLoad`, `cdock_processes()`, `system_load()` (macOS + Linux bodies).
- `src/platform/mod.rs` — re-export + `#[cfg(not(unix))]` stubs; the `ProcInfo`/`SystemLoad` types live here (OS-neutral) so core can name them.
- `src/runtime/mod.rs` — monitor snapshot state, on-demand poll, cpu-delta, orphan flag, kill.
- `src/state/mod.rs` — `InputMode::ProcessMonitor { selected }`, `MenuAction::OpenProcessMonitor`.
- `src/ui/procmon.rs` (new) — overlay render.
- `src/ui/mod.rs` — call `procmon::render`; `src/ui/menu.rs` — the menu item; input handler wires the mode + keys.

---

### Task 1: `ProcInfo`/`SystemLoad` types + macOS `cdock_processes()`/`system_load()`

**Files:**
- Modify: `src/platform/mod.rs` (add the two structs, OS-neutral), `src/platform/unix.rs` (macOS bodies + re-export)

**Interfaces:**
- Produces:
  ```rust
  pub struct ProcInfo {
      pub pid: u32,
      pub ppid: u32,
      pub pane: crate::state::ids::PaneId,
      pub cmd: String,
      pub start: std::time::SystemTime,
      pub cpu_ns: u64,
      pub rss: u64,
  }
  pub struct SystemLoad { pub cpu_pct: f32, pub mem_used: u64, pub mem_total: u64 }
  pub fn cdock_processes() -> Vec<ProcInfo>;
  pub fn system_load() -> SystemLoad;
  ```

- [ ] **Step 1: Add the types** in `src/platform/mod.rs` (near the other shared decls, OS-neutral):

```rust
#[derive(Debug, Clone)]
pub struct ProcInfo {
    pub pid: u32,
    pub ppid: u32,
    pub pane: crate::state::ids::PaneId,
    pub cmd: String,
    pub start: std::time::SystemTime,
    pub cpu_ns: u64,
    pub rss: u64,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SystemLoad {
    pub cpu_pct: f32,
    pub mem_used: u64,
    pub mem_total: u64,
}
```

Add `#[cfg(not(unix))]` stubs returning `Vec::new()` / `SystemLoad::default()`, and re-export `cdock_processes`, `system_load` from `unix` in the `#[cfg(unix)] pub use` list.

- [ ] **Step 2: Write the failing test** — add to `src/platform/unix.rs` `#[cfg(all(test, target_os = "macos"))] mod tests`:

```rust
#[test]
fn cdock_processes_sees_a_child_tagged_with_pane_env() {
    // A child that inherits CDOCK_PANE_ID must be attributed to that pane.
    let mut child = std::process::Command::new("sleep")
        .arg("5")
        .env("CDOCK_PANE_ID", "7")
        .spawn()
        .expect("spawn sleep");
    std::thread::sleep(std::time::Duration::from_millis(80));
    let procs = super::cdock_processes();
    let _ = child.kill();
    let _ = child.wait();
    let mine = procs.iter().find(|p| p.pid == child.id());
    let p = mine.expect("tagged child is listed");
    assert_eq!(p.pane.0, 7, "attributed to CDOCK_PANE_ID");
    assert!(p.cmd.contains("sleep"), "cmd captured: {}", p.cmd);
    assert!(p.rss > 0, "rss captured");
}

#[test]
fn system_load_is_populated() {
    let l = super::system_load();
    assert!(l.mem_total > 0, "mem_total read");
    assert!(l.mem_used <= l.mem_total);
}
```

- [ ] **Step 3: Run it, verify it fails** — `cargo test cdock_processes_sees_a_child` (undefined).

- [ ] **Step 4: Implement macOS `cdock_processes()`.** In `src/platform/unix.rs`, `#[cfg(target_os = "macos")]`. Follow the existing FFI style (`child_process_idents` declares `proc_listpids`/`proc_pidpath` inline; `process_env_var` already parses KERN_PROCARGS2 for env). Steps:
  - `proc_listpids(PROC_ALL_PIDS = 1, 0, buf, size)` → all pids (size the buffer from a first `proc_listpids(..., null, 0)` call ×2 for headroom).
  - For each pid: read `CDOCK_PANE_ID` via the existing env path (`process_env_var(pid, "CDOCK_PANE_ID")`); `continue` if `None`; parse `u32` → `PaneId`.
  - `proc_pidinfo(pid, PROC_PIDTASKINFO = 4, 0, &mut ti, size)` where `ti: libc::proc_taskinfo` → `cpu_ns = ti.pti_total_user + ti.pti_total_system` (already ns), `rss = ti.pti_resident_size`.
  - `proc_pidinfo(pid, PROC_PIDTBSDINFO = 3, 0, &mut bi, size)` where `bi: libc::proc_bsdinfo` → `ppid = bi.pbi_ppid`, `start = UNIX_EPOCH + Duration::new(bi.pbi_start_tvsec, bi.pbi_start_tvusec*1000)`.
  - `cmd`: reuse KERN_PROCARGS2 argv (extend the env-reading helper to also hand back argv[0..], joined by space) or fall back to `proc_pidpath`.
  - `libc::proc_taskinfo`, `libc::proc_bsdinfo`, `libc::proc_pidinfo`, `libc::proc_listpids`, `libc::PROC_PIDTASKINFO` etc. are in the `libc` crate for macOS — prefer them over hand-declared externs where present.
  - Skip a pid gracefully (`continue`) on any failed call (zombie/race).

- [ ] **Step 5: Implement macOS `system_load()`.** `host_statistics64(HOST_VM_INFO64)` for mem (`used = (active+wire+compressed)*page_size`, `total` from `sysctl HW_MEMSIZE`); cpu% from two `host_processor_info(PROCESSOR_CPU_LOAD_INFO)` samples ~50 ms apart (ticks delta busy/total). `libc` exposes `host_statistics64`, `mach_host_self`, `sysctl`. If the cpu sample is disproportionate, a single `sysctl(KERN_LOADAVG)`-derived estimate is an acceptable first cut — but prefer the tick delta.

- [ ] **Step 6: Run tests, verify pass** — `cargo test cdock_processes_sees_a_child system_load_is_populated && cargo clippy --all-targets`. (Two filters: run separately if the harness rejects two names.)

- [ ] **Step 7: Commit**

```bash
git add src/platform/unix.rs src/platform/mod.rs
git commit -m "feat(platform): cdock_processes + system_load (macOS)"
```

---

### Task 2: Linux `cdock_processes()`/`system_load()`

**Files:**
- Modify: `src/platform/unix.rs` (`#[cfg(target_os = "linux")]` bodies)

**Interfaces:**
- Consumes/Produces: same `cdock_processes()` / `system_load()` signatures as Task 1.

- [ ] **Step 1: Write the failing test** — add under `#[cfg(all(test, target_os = "linux"))] mod tests` the SAME two tests as Task 1 Step 2 (copy them verbatim — they are OS-agnostic in intent, only the impl differs).

- [ ] **Step 2: Run it, verify it fails** (Linux only).

- [ ] **Step 3: Implement Linux `cdock_processes()`.** Iterate `std::fs::read_dir("/proc")`, numeric names = pids. For each:
  - `/proc/<pid>/environ` (NUL-separated `KEY=VALUE`): find `CDOCK_PANE_ID`; `continue` if absent; parse → `PaneId`.
  - `/proc/<pid>/stat`: fields (space-split, but comm is in parens — split on last `')'`): field 4 = ppid; 14 = utime, 15 = stime (clock ticks → ns via `sysconf(_SC_CLK_TCK)`); 22 = starttime (ticks since boot; `start = boot_time + starttime/tck`; boot_time from `/proc/stat` `btime`).
  - `/proc/<pid>/statm` field 2 = resident pages → `rss = pages * page_size` (`sysconf(_SC_PAGESIZE)`).
  - `/proc/<pid>/cmdline` (NUL-separated) → `cmd` (join with space); fall back to `stat` comm.
  - Skip a pid on any read error.

- [ ] **Step 4: Implement Linux `system_load()`.** mem from `/proc/meminfo` (`MemTotal`, `MemAvailable` → `used = total - available`); cpu% from two `/proc/stat` `cpu ` line samples ~50 ms apart (busy = total - idle - iowait; `pct = Δbusy/Δtotal`).

- [ ] **Step 5: Run tests, verify pass** (Linux) — `cargo test cdock_processes_sees_a_child system_load_is_populated && cargo clippy --all-targets`.

- [ ] **Step 6: Commit**

```bash
git add src/platform/unix.rs
git commit -m "feat(platform): cdock_processes + system_load (Linux /proc)"
```

---

### Task 3: runtime monitor state (poll, cpu-delta, orphan, kill)

**Files:**
- Modify: `src/runtime/mod.rs` (new monitor fields + methods)

**Interfaces:**
- Consumes: `crate::platform::{cdock_processes, system_load, ProcInfo, SystemLoad}`.
- Produces:
  ```rust
  pub struct MonitorRow { pub info: ProcInfo, pub cpu_pct: Option<f32>, pub orphan: bool, pub protected: bool }
  pub struct MonitorSnapshot { pub load: SystemLoad, pub rows: Vec<MonitorRow> }
  impl Runtime {
      pub fn refresh_monitor(&mut self);          // called on poll while overlay open
      pub fn monitor(&self) -> Option<&MonitorSnapshot>;
      pub fn clear_monitor(&mut self);            // on overlay close
      pub fn kill_process(&self, pid: u32) -> bool; // false if protected
  }
  fn cpu_pct(prev_ns: u64, now_ns: u64, dt: std::time::Duration) -> f32; // pure
  ```

- [ ] **Step 1: Write the failing tests** — in `src/runtime/mod.rs` tests:

```rust
#[test]
fn cpu_pct_from_two_samples() {
    // 1s of CPU time over a 2s interval = 50%.
    let p = super::cpu_pct(0, 1_000_000_000, std::time::Duration::from_secs(2));
    assert!((p - 50.0).abs() < 0.5, "got {p}");
    // No progress = 0.
    assert_eq!(super::cpu_pct(5, 5, std::time::Duration::from_secs(1)), 0.0);
}
```

- [ ] **Step 2: Run it, verify it fails** — `cargo test cpu_pct_from_two_samples`.

- [ ] **Step 3: Implement the pure helper + state.** In `src/runtime/mod.rs`:

```rust
/// CPU percent from cumulative CPU-ns between two samples over dt.
fn cpu_pct(prev_ns: u64, now_ns: u64, dt: std::time::Duration) -> f32 {
    let dt_ns = dt.as_nanos() as f64;
    if dt_ns <= 0.0 { return 0.0; }
    ((now_ns.saturating_sub(prev_ns)) as f64 / dt_ns * 100.0) as f32
}

pub struct MonitorRow { pub info: crate::platform::ProcInfo, pub cpu_pct: Option<f32>, pub orphan: bool, pub protected: bool }
pub struct MonitorSnapshot { pub load: crate::platform::SystemLoad, pub rows: Vec<MonitorRow> }
```

Add to `Runtime`: `monitor: Option<MonitorSnapshot>`, `mon_prev: std::collections::HashMap<u32, (u64, std::time::Instant)>` (pid → last cpu_ns + when). Init both empty in every `Runtime { … }` literal.

- [ ] **Step 4: Implement the methods.**

```rust
pub fn refresh_monitor(&mut self) {
    let protected: std::collections::HashSet<u32> =
        self.panes.values().filter_map(|p| p.pty.child_pid).collect();
    let live_panes: std::collections::HashSet<crate::state::ids::PaneId> =
        self.state.workspaces.iter().flat_map(|w| w.tabs.iter())
            .flat_map(|t| t.layout.panes()).collect();
    let load = crate::platform::system_load();
    let now = std::time::Instant::now();
    let mut rows = Vec::new();
    let mut next_prev = std::collections::HashMap::new();
    for info in crate::platform::cdock_processes() {
        let cpu_pct = self.mon_prev.get(&info.pid).map(|&(prev_ns, t)| {
            cpu_pct(prev_ns, info.cpu_ns, now.duration_since(t))
        });
        next_prev.insert(info.pid, (info.cpu_ns, now));
        let orphan = info.ppid <= 1 || !live_panes.contains(&info.pane);
        let protected = protected.contains(&info.pid);
        rows.push(MonitorRow { info, cpu_pct, orphan, protected });
    }
    self.mon_prev = next_prev;
    self.monitor = Some(MonitorSnapshot { load, rows });
}
pub fn monitor(&self) -> Option<&MonitorSnapshot> { self.monitor.as_ref() }
pub fn clear_monitor(&mut self) { self.monitor = None; self.mon_prev.clear(); }
pub fn kill_process(&self, pid: u32) -> bool {
    let protected = self.panes.values().any(|p| p.pty.child_pid == Some(pid));
    if protected { return false; }
    // SAFETY: SIGTERM to a pid; failure (already gone) is ignored.
    unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) == 0 }
}
```

- [ ] **Step 5: Run tests + build** — `cargo test cpu_pct_from_two_samples && cargo build && cargo clippy --all-targets`.

- [ ] **Step 6: Commit**

```bash
git add src/runtime/mod.rs
git commit -m "feat(runtime): process-monitor snapshot, cpu-delta, orphan flag, protected kill"
```

---

### Task 4: `InputMode::ProcessMonitor` + overlay render

**Files:**
- Modify: `src/state/mod.rs` (`InputMode::ProcessMonitor { selected: usize }`)
- Create: `src/ui/procmon.rs`
- Modify: `src/ui/mod.rs` (module decl + render call)

**Interfaces:**
- Consumes: `rt.monitor()` (Task 3), `InputMode::ProcessMonitor`.
- Produces: `pub fn render(rt: &Runtime, selected: usize, area: Rect, frame: &mut Frame)`; `pub fn killable_rows(snap: &MonitorSnapshot) -> Vec<usize>` (indices of non-protected rows, for selection bounds).

- [ ] **Step 1: Add the mode.** In `src/state/mod.rs` `InputMode`, add `ProcessMonitor { selected: usize }`. (Grep the `match &mode`/`match rt.state.input_mode` sites and add an arm where the compiler flags exhaustiveness; overlays are handled in `ui/mod.rs::render` and the input handler.)

- [ ] **Step 2: Write the failing test** — in `src/ui/procmon.rs` tests, test the pure `killable_rows`:

```rust
#[test]
fn killable_rows_excludes_protected() {
    use crate::runtime::{MonitorRow, MonitorSnapshot};
    let row = |pid, protected| MonitorRow {
        info: crate::platform::ProcInfo { pid, ppid: 100, pane: crate::state::ids::PaneId(1),
            cmd: "x".into(), start: std::time::SystemTime::now(), cpu_ns: 0, rss: 0 },
        cpu_pct: None, orphan: false, protected,
    };
    let snap = MonitorSnapshot { load: Default::default(),
        rows: vec![row(10, true), row(11, false), row(12, false)] };
    assert_eq!(super::killable_rows(&snap), vec![1, 2]);
}
```

- [ ] **Step 3: Run it, verify it fails.**

- [ ] **Step 4: Implement `procmon.rs`.** `killable_rows` filters `!protected`. `render` draws (Clear + Paragraph/Block like `ui/help.rs`): a `SYSTEM  cpu {load.cpu_pct:.0}%  mem {used}/{total}` header; rows grouped by `info.pane` (look up the pane/agent name from `rt`); each row `{cmd:trunc} {uptime} {cpu%|—} {mem} {orphan?}`; protected rows dimmed with no marker; the `selected`-th killable row highlighted (REVERSED). Format uptime from `now - info.start`, mem human (`KiB/MiB/GiB`).

- [ ] **Step 5: Wire render.** In `src/ui/mod.rs::render`, add a `mod procmon;` decl and, in the overlay match, `InputMode::ProcessMonitor { selected } => procmon::render(rt, *selected, full, frame)`.

- [ ] **Step 6: Run tests + build** — `cargo test killable_rows_excludes_protected && cargo build && cargo clippy --all-targets`.

- [ ] **Step 7: Commit**

```bash
git add src/state/mod.rs src/ui/procmon.rs src/ui/mod.rs
git commit -m "feat(ui): process-monitor overlay (system load + per-pane processes)"
```

---

### Task 5: input — open, navigate, kill, close

**Files:**
- Modify: the input handler (grep `InputMode::Help =>` / `InputMode::Menu` handling in `src/input/` or `src/runtime/`), `src/runtime/mod.rs` poll site.

**Interfaces:**
- Consumes: `rt.refresh_monitor()`, `rt.clear_monitor()`, `rt.kill_process()`, `procmon::killable_rows`.

- [ ] **Step 1: Poll while open.** At the server/runtime poll site that already refreshes agents (grep `effective_status`/the ~2 s poll in `src/runtime/mod.rs` or `src/server.rs` ws_poll), add: if `matches!(rt.state.input_mode, InputMode::ProcessMonitor{..})` → `rt.refresh_monitor()` (throttled to ~1.5 s). On the FIRST open, refresh immediately so the overlay isn't empty.

- [ ] **Step 2: Key handling.** In the input handler, add an `InputMode::ProcessMonitor { selected }` arm:
  - `Up`/`Down` → move `selected` within `0..killable_rows(snap).len()` (saturating).
  - `Enter` → `let idx = killable_rows(snap)[selected]; let pid = snap.rows[idx].info.pid; rt.kill_process(pid);` then refresh.
  - `q` / `Esc` → set `input_mode = Terminal`, `rt.clear_monitor()`.
  (Model the arm on the existing `InputMode::Help`/`Menu` handling; a kill-confirm can be a second Enter or a `y` prompt — keep it to a single confirm line if cheap, else kill on Enter directly and note it.)

- [ ] **Step 3: Build + manual reasoning check** — `cargo build && cargo clippy --all-targets && cargo test`. (No unit test for the wiring; covered by the Task 6 E2E.)

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(input): process-monitor open/navigate/kill/close"
```

---

### Task 6: menu entry + end-to-end

**Files:**
- Modify: `src/state/mod.rs` (`MenuAction::OpenProcessMonitor`), `src/ui/menu.rs` (`app_items`), the `MenuAction` dispatch site.

- [ ] **Step 1: Add the action + item.** `MenuAction::OpenProcessMonitor` in `src/state/mod.rs`. In `src/ui/menu.rs::app_items`, add `("process monitor…", MenuAction::OpenProcessMonitor)`. In the `MenuAction` dispatch (grep `MenuAction::` handling), add an arm setting `input_mode = InputMode::ProcessMonitor { selected: 0 }` and calling `rt.refresh_monitor()`.

- [ ] **Step 2: Build + full gate** — `cargo build && cargo clippy --all-targets && cargo test`.

- [ ] **Step 3: Sandbox E2E (cdock-dev).** Controller-run (chrome/2-client not needed, but a live server is): in a cdock-dev pane, start a background script (`python3 -c 'import time; time.sleep(300)' &` — NOT `sleep`, an Apple platform-binary whose env macOS redacts); open menu → "process monitor…"; confirm the sleep is listed under that pane with uptime/mem; select it, Enter → confirm it dies and drops off; confirm the pane's shell/agent row is dimmed and not selectable.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat: process-monitor menu entry + wiring"
```

---

## Self-Review

**Spec coverage:**
- Ownership via `CDOCK_PANE_ID` env → Task 1/2 (env-read attribution). ✓
- `cdock_processes()` + `system_load()` macOS + Linux → Tasks 1, 2. ✓
- On-demand poll + cpu-delta + orphan → Task 3 (`refresh_monitor`, `cpu_pct`, orphan rule ppid≤1 or dead pane). ✓
- Overlay per-pane + system header → Task 4. ✓
- Protected kill (pty child) → Task 3 `kill_process` guard + Task 4 `killable_rows` + Task 5 Enter. ✓
- Menu entry → Task 6. ✓
- Testing: platform child-with-env test, cpu_pct unit, killable_rows unit, E2E → Tasks 1/2/3/4/6. ✓

**Placeholder scan:** The FFI bodies (Task 1 Steps 4-5, Task 2 Steps 3-4) are described as concrete call sequences with exact constants/struct fields rather than full `unsafe` transcriptions — deliberate: the struct layouts are `libc`-documented and a verbatim 200-line FFI dump in the plan is more error-prone than the field-level recipe. Every non-FFI task carries complete code. The kill-confirm UX (Task 5) is left as "single confirm line or direct Enter" — an explicitly bounded choice, not a vague TODO.

**Type consistency:** `ProcInfo`/`SystemLoad` (Task 1) consumed by `refresh_monitor` (Task 3) and `procmon` test (Task 4). `MonitorRow`/`MonitorSnapshot` (Task 3) consumed by `procmon` (Task 4) and input (Task 5). `cpu_pct`, `kill_process`, `refresh_monitor`, `clear_monitor`, `killable_rows` names consistent across Tasks 3-6. `InputMode::ProcessMonitor { selected }` (Task 4) used in Tasks 5-6. `MenuAction::OpenProcessMonitor` (Task 6).
