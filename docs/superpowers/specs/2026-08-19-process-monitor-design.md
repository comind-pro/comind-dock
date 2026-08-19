# Process monitor — design

**Date:** 2026-08-19
**Status:** approved (design)
**Scope:** an in-app overlay that lists the processes cdock's agents spawned —
grouped by the pane/agent that launched them, including orphans the agent lost
track of — with who/when, uptime, CPU/mem load, and the ability to kill a hung
one. macOS and Linux.

## Problem

Agents (claude/cline in panes) fork scripts, dev servers, tests. Some go
**orphan**: the agent finishes or forgets, the process reparents to
launchd/init and keeps running. The user loses track of what's running where and
what it's costing, and wants to stop the dead ones. `htop` shows the processes
but not which agent/pane launched them — the exact link the user needs.

## Key idea: ownership via inherited env

cdock sets `CDOCK_PANE_ID` (+ `CDOCK_TAB_ID`, `CDOCK_WORKSPACE_ID`) in every
pane's PTY env (`term/pty.rs`). Every descendant inherits it, and it **survives
reparenting** (env is a snapshot taken at exec, not tied to ppid). So: scan all
system processes, read `CDOCK_PANE_ID` from each, and group by pane — even when
the ppid link is gone. That's what catches the hung orphans, and it's why a
generic process viewer can't do this.

`process_env_var` already reads this (KERN_PROCARGS2 on macOS,
`/proc/<pid>/environ` on Linux).

## Components

### 1. platform: `cdock_processes()` + `system_load()`

```
struct ProcInfo {
    pid: u32,
    ppid: u32,
    pane: PaneId,          // from CDOCK_PANE_ID
    cmd: String,           // argv joined, or exe path
    start: SystemTime,     // process start → uptime = now - start
    cpu_ns: u64,           // cumulative CPU time (user+sys); % is a delta
    rss: u64,              // resident memory bytes
}
```

- `cdock_processes() -> Vec<ProcInfo>`: enumerate all pids, read `CDOCK_PANE_ID`;
  skip any without it (not ours). For the rest, collect cmd/start/cpu/rss/ppid.
  - **macOS:** `proc_listpids(PROC_ALL_PIDS)`; `proc_pidinfo(PROC_PIDTASKINFO)`
    for cpu+rss; `proc_pidinfo(PROC_PIDTBSDINFO)` for start time + ppid;
    KERN_PROCARGS2 for argv + env (extend the existing `process_env_var` path to
    also return argv, or a sibling fn).
  - **Linux:** iterate `/proc/<pid>`; `environ` for the env, `stat` for
    ppid/utime/stime/starttime, `statm` (or `status` VmRSS) for rss, `cmdline`
    for argv.
- `system_load() -> SystemLoad { cpu_pct: f32, mem_used: u64, mem_total: u64 }`:
  - macOS: `host_processor_info`/`host_statistics64` (or a cheap `sysctl`);
  - Linux: `/proc/stat` (cpu delta), `/proc/meminfo`.

Both live behind `src/platform/` with the same signatures; a `#[cfg(not(unix))]`
stub returns empty (core modules stay OS-free).

### 2. runtime: monitor state

- On-demand: only while the overlay is open does the server poll
  `cdock_processes()` + `system_load()` (~1.5 s). Closed → no scanning, no cost.
- **CPU %:** keep the previous sample's `pid → cpu_ns`; percent =
  `(cpu_ns - prev) / interval_ns`. First frame after opening shows `—`.
- **Orphan flag:** a process whose `ppid == 1` (reparented) OR whose pane's agent
  pane is gone. Rendered with an `orphan` badge.

### 3. ui: overlay (`InputMode::ProcessMonitor`)

- New `InputMode::ProcessMonitor { selected: usize }` (selection index over the
  killable rows). Rendered like `help` — a centered/full scrollable panel drawn
  last in `render`.
- Layout: a `SYSTEM  cpu NN%  mem U/T` header; then per-pane/agent sections
  (pane name + agent), each listing its processes: `cmd` (truncated), uptime,
  cpu%, mem, orphan badge.
- Keys: ↑/↓ move `selected` over killable rows; Enter → kill-confirm; `q`/Esc
  close. The pane's own pty-child (shell/agent) is shown dimmed and is NOT a
  selectable/killable row (see kill safety).

### 4. kill (protected)

- Enter on a selected process → a confirm line (`kill <cmd>? y/n`); `y` sends
  `SIGTERM`. A follow-up option escalates to `SIGKILL` if it's still there.
- **Safety guard:** any pid that equals a pane's `pty.child_pid` (the shell or
  agent itself) is protected — never selectable, never killed. Closing the pane
  is the way to stop those. Descendants and orphans are killable.

### 5. entry

- `app_items` (`ui/menu.rs`) gains `("process monitor…", MenuAction::OpenProcessMonitor)`.
- New `MenuAction::OpenProcessMonitor` sets `InputMode::ProcessMonitor`. Optional
  keybind can follow.

## Data flow

menu → `OpenProcessMonitor` → `InputMode::ProcessMonitor` → server, while in that
mode, polls `cdock_processes()`/`system_load()` every 1.5 s → runtime holds the
snapshot + previous cpu sample → ui renders per-pane groups → ↑/↓ select → Enter
→ confirm → `kill(pid)` unless protected.

## Testing

- **platform (macOS + Linux):** spawn a child process with `CDOCK_PANE_ID` set in
  its env, call `cdock_processes()`, assert it appears with the right pane and
  non-zero rss / a start time. (`sleep 5` child, like the existing
  `children_visible_by_path` test.)
- **cpu delta:** unit-test the percent-from-two-samples helper (pure: two
  `cpu_ns` + interval → %).
- **orphan flag + kill guard:** unit-test the pure predicates (ppid==1 → orphan;
  pid ∈ pty child_pids → protected).
- **E2E (cdock-dev):** start a background script in a pane, open the monitor,
  confirm it's listed under that pane with load; kill it and confirm it's gone;
  confirm the agent/shell row is not killable.

## Risks

- **Scan cost.** All-pids × env-read (KERN_PROCARGS2 / `/proc/environ`) is the
  expensive part on a busy box (100s of processes). Mitigated by on-demand +
  1.5 s throttle. If still heavy, a later optimization can read env only for
  pids not already in a known pane subtree; out of scope now.
- **First-frame CPU %** is unknown (no previous sample) → shown as `—`.
- **Permissions.** All cdock descendants share the user's uid, so reading their
  info and killing them needs no privilege; env-read failures (zombies, races)
  skip that pid gracefully.

## Out of scope (YAGNI)

- Net/disk I/O, per-core breakdown, historical graphs.
- A nested process-tree view (flat list per pane is enough to start).
- A keybind (menu entry first; keybind can follow).
- Env-read optimization (subtree pre-filter).

## Implementation order (plan tasks)

1. platform: `ProcInfo` + `cdock_processes()` + `system_load()` — macOS.
2. platform: the same for Linux (`/proc`); shared `#[cfg(not(unix))]` stub.
3. runtime: on-demand monitor poll + cpu-delta + orphan flag.
4. ui: `InputMode::ProcessMonitor` overlay render + navigation.
5. kill with the protected-pid guard + confirm.
6. entry: `MenuAction::OpenProcessMonitor` + menu item + E2E.
