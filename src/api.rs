//! JSON automation API (Phase 4): newline-delimited JSON request/response on
//! a second unix socket (`api-<session>.sock`). One line in → one line out;
//! `wait-*` requests hold the line until the condition or timeout.
//! Subscriptions (`{"cmd":"subscribe"}`) stream events on the same socket;
//! `cdock api reference`/`schema` prints the machine-readable catalog.

use std::io::{BufRead, Write};
use std::time::{Duration, Instant};

use ratatui::layout::Rect;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};

use crate::runtime::Runtime;
use crate::state::ids::PaneId;
use crate::state::layout::Dir;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "kebab-case")]
pub enum Req {
    PaneList,
    /// Full runtime state in one reply: workspaces → tabs → panes.
    Snapshot,
    /// Split a pane (default: focused) and spawn a shell or command in it.
    Split { pane: Option<u64>, direction: Option<String>, command: Option<String> },
    /// Write literal text to a pane's PTY (no Enter).
    SendText { pane: u64, text: String },
    /// Write text + Enter.
    Run { pane: u64, command: String },
    /// Read the last non-empty screen lines of a pane.
    Read { pane: u64, lines: Option<usize> },
    Focus { pane: u64 },
    /// Spawn an agent (or any command) in a new tab, or a split of the
    /// focused pane when `split` is given. `env` rides into the pane
    /// (profiles resolve to command + env on the CLI side).
    AgentStart {
        command: String,
        split: Option<String>,
        workspace: Option<u64>,
        #[serde(default)]
        env: Vec<(String, String)>,
    },
    /// From the agent's SessionStart integration hook: which conversation
    /// runs in this pane (restore resumes exactly it).
    ReportAgentSession {
        pane: u64,
        session_id: String,
        /// The reporting agent's pid (the hook's parent). Rejected when it
        /// isn't the pane's tracked agent — a nested claude (agent's Bash
        /// tool, subshell) must not clobber the pane's own conversation.
        #[serde(default)]
        pid: Option<u32>,
    },
    /// Integration hooks push an authoritative agent state: overrides
    /// screen detection until ttl_ms (default 30s) or an explicit clear.
    ReportAgent {
        pane: u64,
        /// working | blocked | done | idle — or "clear" to drop the report.
        state: String,
        label: Option<String>,
        ttl_ms: Option<u64>,
        /// The reporting agent's pid. Rejected when it isn't the pane's
        /// tracked agent — a nested claude (the agent's own Bash tool) would
        /// otherwise report ITS state onto the parent's pane.
        #[serde(default)]
        pid: Option<u32>,
    },
    /// Presentation metadata from hooks: title override for the pane.
    ReportMetadata { pane: u64, title: Option<String> },
    /// User-given pane name (empty clears it): wins over the agent's own
    /// OSC title in the sidebar and notifications.
    RenamePane { pane: u64, name: String },
    /// Re-read detection manifests from disk (bundled + overrides).
    /// Handled directly by the server loop, which owns the manifest set.
    ReloadManifests,
    /// Full detection trace for a pane (server loop owns the manifests).
    AgentExplain { pane: u64 },
    /// Attach a behavior profile ("global:<name>" | "ws:<name>") to an
    /// agent pane: injected into the live session, resumed as system
    /// prompt. Null behavior clears the mark.
    AgentBehavior { pane: u64, behavior: Option<String> },
    /// Re-read config/keymap/theme. Also on the sidebar app menu.
    ReloadConfig,
    /// exec() the current binary in place: same pid, panes survive.
    Handoff,
    /// Save and stop the whole session server (`cdock session stop`).
    Shutdown,
    /// Focus a workspace / tab by public id.
    WorkspaceFocus { workspace: u64 },
    /// Close a workspace: kill every pane in it (cascade does the rest).
    WorkspaceClose { workspace: u64 },
    /// New workspace (cwd defaults to [terminal].new_cwd policy).
    WorkspaceCreate { cwd: Option<String> },
    TabFocus { tab: u64 },
    TabClose { tab: u64 },
    /// New tab in a workspace (default: the active one).
    TabCreate { workspace: Option<u64> },
    /// Worktrees of a workspace's repo (default: the active workspace).
    WorktreeList { workspace: Option<u64> },
    /// Create branch + worktree and open it as a child space.
    WorktreeCreate { workspace: Option<u64>, branch: String },
    /// Open an existing worktree (by branch) as a child space.
    WorktreeOpen { workspace: Option<u64>, branch: String },
    /// Remove a worktree child space: git worktree remove + close its panes.
    WorktreeRemove { workspace: u64, #[serde(default)] force: bool },
    WaitAgentStatus { pane: u64, status: String, timeout_ms: Option<u64> },
    WaitOutput {
        pane: u64,
        #[serde(rename = "match")]
        needle: String,
        timeout_ms: Option<u64>,
    },
}

pub enum WaitCond {
    Status(crate::detect::Status),
    /// Needle plus a rolling tail of raw pane output — fast output can
    /// scroll past between 500ms polls, so the PTY-data path feeds chunks
    /// here and matching never misses.
    Output(String, String),
}

pub struct PendingWait {
    pub pane: PaneId,
    pub cond: WaitCond,
    pub deadline: Option<Instant>,
}

pub type Replier = oneshot::Sender<Value>;

/// What a subscription wants: which event kinds ("agent-status", "output";
/// empty = all) and optionally a single pane.
#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SubSpec {
    pub events: Vec<String>,
    pub pane: Option<u64>,
}

impl SubSpec {
    pub fn wants(&self, kind: &str, pane: Option<u64>) -> bool {
        (self.events.is_empty() || self.events.iter().any(|e| e == kind))
            && (self.pane.is_none() || self.pane == pane)
    }
}

/// One message from an API connection to the server loop.
pub enum ConnMsg {
    Req(Req, Replier),
    /// `{"cmd":"subscribe",...}`: the connection becomes an event stream.
    Sub(SubSpec, mpsc::UnboundedSender<Value>),
}

/// Push `event` to every live subscriber that wants it; prunes dead ones.
pub fn emit(
    subs: &mut Vec<(SubSpec, mpsc::UnboundedSender<Value>)>,
    kind: &str,
    pane: Option<u64>,
    event: &Value,
) {
    subs.retain(|(spec, tx)| {
        if spec.wants(kind, pane) { tx.send(event.clone()).is_ok() } else { !tx.is_closed() }
    });
}

/// True when at least one subscriber wants this kind (skip building events
/// nobody reads — output events fire on every PTY chunk).
pub fn wanted(subs: &[(SubSpec, mpsc::UnboundedSender<Value>)], kind: &str) -> bool {
    subs.iter().any(|(spec, _)| spec.events.is_empty() || spec.events.iter().any(|e| e == kind))
}

fn err(msg: impl std::fmt::Display) -> Value {
    json!({"ok": false, "error": msg.to_string()})
}

fn parse_status(s: &str) -> Option<crate::detect::Status> {
    use crate::detect::Status::*;
    Some(match s {
        "working" => Working,
        "blocked" => Blocked,
        "done" => Done,
        "idle" => Idle,
        "unknown" => Unknown,
        _ => return None,
    })
}

/// Handle one request against the runtime. Ok → reply now (errors included);
/// Err → park the connection until `check_waiters` resolves it.
pub fn handle(rt: &mut Runtime, area: Rect, req: Req) -> Result<Value, PendingWait> {
    match req {
        Req::PaneList => Ok(pane_list(rt)),
        Req::Snapshot => Ok(snapshot(rt)),
        Req::Split { pane, direction, command } => {
            let target = pane.map(PaneId).unwrap_or_else(|| rt.state.focused_pane());
            let dir = match direction.as_deref() {
                Some("down") => Dir::Down,
                _ => Dir::Right,
            };
            // In place: background API calls must never yank the user's
            // focus — their keystrokes would land in the new shell.
            let Some(new) = rt.state.split_pane(target, dir) else {
                return Ok(err(format!("no such pane {target}")));
            };
            // Provisional size; compute_view corrects it before the next frame.
            match rt.spawn_pane_cmd(new, area.width.max(2) / 2, area.height.max(2) / 2, command) {
                Ok(()) => {
                    rt.mark_dirty();
                    Ok(json!({"ok": true, "pane": new.0}))
                }
                // Spawn failed: the leaf is already in the layout — roll it
                // back or it renders empty forever and can never be closed.
                Err(e) => {
                    rt.state.close_pane(new);
                    Ok(err(e))
                }
            }
        }
        Req::ReloadConfig => {
            rt.reload_config();
            Ok(json!({"ok": true}))
        }
        // Owned by the server loop; reaching here is a wiring bug.
        Req::ReloadManifests | Req::Handoff | Req::Shutdown | Req::AgentExplain { .. } => {
            Ok(err("handled by the server loop"))
        }
        Req::SendText { pane, text } => Ok(write_pty(rt, pane, text.as_bytes())),
        Req::Run { pane, command } => {
            Ok(write_pty(rt, pane, format!("{command}\r").as_bytes()))
        }
        Req::Read { pane, lines } => match rt.panes.get(&PaneId(pane)) {
            Some(p) => {
                let text = p.emu.bottom_text(lines.unwrap_or(30)).join("\n");
                Ok(json!({"ok": true, "text": text}))
            }
            None => Ok(err(format!("no such pane %{pane}"))),
        },
        Req::Focus { pane } => {
            if rt.state.focus_pane(PaneId(pane)) {
                rt.mark_dirty();
                Ok(json!({"ok": true}))
            } else {
                Ok(err(format!("no such pane %{pane}")))
            }
        }
        Req::AgentStart { command, split, workspace, env } => {
            let Some(wi) = resolve_ws(rt, workspace) else {
                return Ok(err("no such workspace"));
            };
            // Background spawns never steal the user's view.
            let pane = match split.as_deref() {
                Some(d) => {
                    let dir = if d == "down" { Dir::Down } else { Dir::Right };
                    let target = rt.state.workspaces[wi].active_tab().focused_pane;
                    match rt.state.split_pane(target, dir) {
                        Some(p) => p,
                        None => return Ok(err("no pane to split")),
                    }
                }
                None => rt.state.new_tab_in(wi, false),
            };
            // Full size for the tab form: compute_panes only lays out the
            // ACTIVE tab, so a background tab keeps its spawn size until
            // focused — a half-width claude mis-wraps for pane read/wait.
            let (w, h) = if split.is_some() {
                (area.width.max(2) / 2, area.height.max(2) / 2)
            } else {
                (area.width.max(4), area.height.max(4))
            };
            // An instantly-failing agent command must not cascade the tab
            // away — degrade into a shell with the error visible.
            match rt.spawn_pane_env(
                pane,
                w,
                h,
                Some(crate::agents::hold_on_failure(&command)),
                env,
            ) {
                Ok(()) => {
                    rt.mark_dirty();
                    Ok(json!({"ok": true, "pane": pane.0}))
                }
                Err(e) => {
                    rt.state.close_pane(pane);
                    Ok(err(e))
                }
            }
        }
        Req::AgentBehavior { pane, behavior } => {
            match rt.apply_behavior(PaneId(pane), behavior) {
                Ok(()) => Ok(json!({"ok": true})),
                Err(e) => Ok(err(e)),
            }
        }
        Req::ReportAgentSession { pane, session_id, pid } => {
            let pane = PaneId(pane);
            let Some(p) = rt.panes.get(&pane) else {
                return Ok(err(format!("no such pane {pane}")));
            };
            // Nested claude guard: only the pane's tracked agent process may
            // (re)bind the conversation. Unknown agent_pid (hook raced the
            // first detection poll) is let through.
            if let (Some(agent_pid), Some(reporter)) = (p.agent_pid, pid)
                && agent_pid != reporter
            {
                return Ok(json!({"ok": true, "ignored": "nested agent"}));
            }
            rt.agent_sessions.insert(pane, format!("claude:{session_id}"));
            rt.save_session(); // survive a crash between autosaves
            Ok(json!({"ok": true}))
        }
        Req::ReportAgent { pane, state, label, ttl_ms, pid } => {
            let pane = PaneId(pane);
            let Some(p) = rt.panes.get_mut(&pane) else {
                return Ok(err(format!("no such pane {pane}")));
            };
            // Only the pane's own agent may speak for it: a claude that the
            // agent spawned through its Bash tool inherits CDOCK_PANE_ID and
            // would otherwise report its own "done" onto the parent.
            if let (Some(agent_pid), Some(reporter)) = (p.agent_pid, pid)
                && agent_pid != reporter
            {
                return Ok(json!({"ok": true, "ignored": "nested agent"}));
            }
            if state == "clear" {
                p.reported = None;
                rt.mark_dirty();
                return Ok(json!({"ok": true}));
            }
            let Some(status) = parse_status(&state) else {
                return Ok(err(format!("bad state {state:?}")));
            };
            p.reported = Some(crate::runtime::Reported {
                status,
                label,
                until: Instant::now() + Duration::from_millis(ttl_ms.unwrap_or(30_000)),
            });
            rt.mark_dirty();
            Ok(json!({"ok": true}))
        }
        Req::RenamePane { pane, name } => {
            let pane = PaneId(pane);
            if !rt.panes.contains_key(&pane) {
                return Ok(err(format!("no such pane {pane}")));
            }
            rt.rename_pane(pane, name);
            rt.save_session();
            Ok(json!({"ok": true}))
        }
        Req::ReportMetadata { pane, title } => {
            let pane = PaneId(pane);
            if !rt.panes.contains_key(&pane) {
                return Ok(err(format!("no such pane {pane}")));
            }
            match title {
                Some(t) if !t.is_empty() => {
                    rt.titles.insert(pane, t);
                }
                _ => {
                    rt.titles.remove(&pane);
                }
            }
            rt.mark_dirty();
            Ok(json!({"ok": true}))
        }
        Req::WorkspaceFocus { workspace } => {
            let Some(wi) = resolve_ws(rt, Some(workspace)) else {
                return Ok(err("no such workspace"));
            };
            rt.state.active_workspace = wi;
            rt.mark_dirty();
            Ok(json!({"ok": true}))
        }
        Req::WorkspaceClose { workspace } => {
            let Some(wi) = resolve_ws(rt, Some(workspace)) else {
                return Ok(err("no such workspace"));
            };
            for pane in rt.state.workspace_panes(wi) {
                rt.kill_pane(pane);
            }
            Ok(json!({"ok": true}))
        }
        Req::WorkspaceCreate { cwd } => {
            let cwd = match cwd {
                Some(c) => std::path::PathBuf::from(c),
                None => rt.new_space_cwd(),
            };
            let name =
                cwd.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let pane = rt.state.new_workspace_full(name, cwd, None, false);
            match rt.spawn_pane(pane, area.width.max(4), area.height.max(4)) {
                Ok(()) => {
                    rt.mark_dirty();
                    let ws_id = rt
                        .state
                        .workspaces
                        .iter()
                        .find(|w| w.tabs.iter().any(|t| t.layout.contains(pane)))
                        .map(|w| w.id.0);
                    Ok(json!({"ok": true, "workspace": ws_id, "pane": pane.0}))
                }
                Err(e) => {
                    rt.state.close_pane(pane);
                    Ok(err(e))
                }
            }
        }
        Req::TabFocus { tab } => {
            let target = crate::state::ids::TabId(tab);
            for (wi, ws) in rt.state.workspaces.iter().enumerate() {
                if let Some(ti) = ws.tabs.iter().position(|t| t.id == target) {
                    rt.state.active_workspace = wi;
                    rt.state.workspaces[wi].active_tab = ti;
                    rt.mark_dirty();
                    return Ok(json!({"ok": true}));
                }
            }
            Ok(err("no such tab"))
        }
        Req::TabClose { tab } => {
            let target = crate::state::ids::TabId(tab);
            let panes: Vec<PaneId> = rt
                .state
                .workspaces
                .iter()
                .flat_map(|w| w.tabs.iter())
                .filter(|t| t.id == target)
                .flat_map(|t| t.layout.panes())
                .collect();
            if panes.is_empty() {
                return Ok(err("no such tab"));
            }
            for pane in panes {
                rt.kill_pane(pane);
            }
            Ok(json!({"ok": true}))
        }
        Req::TabCreate { workspace } => {
            let Some(wi) = resolve_ws(rt, workspace) else {
                return Ok(err("no such workspace"));
            };
            let pane = rt.state.new_tab_in(wi, false);
            match rt.spawn_pane(pane, area.width.max(4), area.height.max(4)) {
                Ok(()) => {
                    rt.mark_dirty();
                    Ok(json!({"ok": true, "pane": pane.0}))
                }
                Err(e) => {
                    rt.state.close_pane(pane);
                    Ok(err(e))
                }
            }
        }
        Req::WorktreeList { workspace } => {
            let Some(wi) = resolve_ws(rt, workspace) else {
                return Ok(err("no such workspace"));
            };
            let list: Vec<Value> = crate::git::worktrees(&rt.state.workspaces[wi].cwd)
                .into_iter()
                .map(|(path, branch)| json!({"path": path.to_string_lossy(), "branch": branch}))
                .collect();
            Ok(json!({"ok": true, "worktrees": list}))
        }
        Req::WorktreeCreate { workspace, branch } => {
            let Some(wi) = resolve_ws(rt, workspace) else {
                return Ok(err("no such workspace"));
            };
            let ws = &rt.state.workspaces[wi];
            let (repo, parent_id) = (ws.cwd.clone(), ws.id);
            match crate::git::worktree_add(&repo, &branch, &rt.cfg.worktrees.root()) {
                Ok(path) => {
                    rt.open_worktree(parent_id, path.clone(), area, false);
                    rt.mark_dirty();
                    Ok(json!({"ok": true, "path": path.to_string_lossy()}))
                }
                Err(e) => Ok(err(e)),
            }
        }
        Req::WorktreeOpen { workspace, branch } => {
            let Some(wi) = resolve_ws(rt, workspace) else {
                return Ok(err("no such workspace"));
            };
            let ws = &rt.state.workspaces[wi];
            let (cwd, parent_id) = (ws.cwd.clone(), ws.id);
            match crate::git::worktrees(&cwd).into_iter().find(|(_, b)| *b == branch) {
                Some((path, _)) => {
                    rt.open_worktree(parent_id, path.clone(), area, false);
                    rt.mark_dirty();
                    Ok(json!({"ok": true, "path": path.to_string_lossy()}))
                }
                None => Ok(err(format!("no worktree for branch {branch:?}"))),
            }
        }
        Req::WorktreeRemove { workspace, force } => {
            let Some(wi) = rt
                .state
                .workspaces
                .iter()
                .position(|w| w.id == crate::state::ids::WorkspaceId(workspace))
            else {
                return Ok(err("no such workspace"));
            };
            let ws = &rt.state.workspaces[wi];
            let Some(parent_id) = ws.parent else {
                return Ok(err("not a worktree space (no parent)"));
            };
            let Some(repo) =
                rt.state.workspaces.iter().find(|w| w.id == parent_id).map(|w| w.cwd.clone())
            else {
                return Ok(err("parent space is gone"));
            };
            let path = ws.cwd.clone();
            if let Err(e) = crate::git::worktree_remove(&repo, &path, force) {
                return Ok(err(e));
            }
            for pane in rt.state.workspace_panes(wi) {
                rt.kill_pane(pane);
            }
            rt.mark_dirty();
            Ok(json!({"ok": true}))
        }
        Req::WaitAgentStatus { pane, status, timeout_ms } => {
            let Some(status) = parse_status(&status) else {
                return Ok(err(format!("bad status {status:?}")));
            };
            wait(rt, pane, WaitCond::Status(status), timeout_ms)
        }
        Req::WaitOutput { pane, needle, timeout_ms } => {
            wait(rt, pane, WaitCond::Output(needle, String::new()), timeout_ms)
        }
    }
}

/// Machine-readable API reference: one valid example request per command
/// (`cdock api reference`). The unit test parses every example — the
/// reference cannot silently drift from the enum.
pub const REFERENCE: &str = r#"[
  {"cmd":"pane-list"},
  {"cmd":"snapshot"},
  {"cmd":"split","pane":1,"direction":"right","command":"cargo test"},
  {"cmd":"send-text","pane":1,"text":"hello"},
  {"cmd":"run","pane":1,"command":"ls"},
  {"cmd":"read","pane":1,"lines":30},
  {"cmd":"focus","pane":1},
  {"cmd":"agent-start","command":"claude","split":"right","workspace":3,"env":[["K","V"]]},
  {"cmd":"report-agent-session","pane":1,"session_id":"uuid"},
  {"cmd":"report-agent","pane":1,"state":"blocked","label":"awaiting review","ttl_ms":60000,"pid":4321},
  {"cmd":"report-metadata","pane":1,"title":"builder"},
  {"cmd":"rename-pane","pane":1,"name":"kafka refactor"},
  {"cmd":"reload-manifests"},
  {"cmd":"agent-explain","pane":1},
  {"cmd":"agent-behavior","pane":1,"behavior":"global:researcher"},
  {"cmd":"reload-config"},
  {"cmd":"handoff"},
  {"cmd":"shutdown"},
  {"cmd":"workspace-focus","workspace":3},
  {"cmd":"workspace-close","workspace":3},
  {"cmd":"workspace-create","cwd":"/tmp"},
  {"cmd":"tab-focus","tab":2},
  {"cmd":"tab-close","tab":2},
  {"cmd":"tab-create","workspace":3},
  {"cmd":"worktree-list","workspace":3},
  {"cmd":"worktree-create","workspace":3,"branch":"feature"},
  {"cmd":"worktree-open","workspace":3,"branch":"feature"},
  {"cmd":"worktree-remove","workspace":6,"force":false},
  {"cmd":"wait-agent-status","pane":1,"status":"idle","timeout_ms":60000},
  {"cmd":"wait-output","pane":1,"match":"done","timeout_ms":60000},
  {"cmd":"subscribe","events":["agent-status","output"],"pane":1}
]"#;

/// Public wrapper for the server loop (worktree-create runs off-loop).
pub fn resolve_ws_pub(rt: &Runtime, id: Option<u64>) -> Option<usize> {
    resolve_ws(rt, id)
}

/// Workspace index by public id; None id → the active workspace.
fn resolve_ws(rt: &Runtime, id: Option<u64>) -> Option<usize> {
    match id {
        None => Some(rt.state.active_workspace),
        Some(id) => rt
            .state
            .workspaces
            .iter()
            .position(|w| w.id == crate::state::ids::WorkspaceId(id)),
    }
}

fn wait(
    rt: &Runtime,
    pane: u64,
    cond: WaitCond,
    timeout_ms: Option<u64>,
) -> Result<Value, PendingWait> {
    let pane = PaneId(pane);
    if !rt.panes.contains_key(&pane) {
        return Ok(err(format!("no such pane {pane}")));
    }
    // Even "no timeout" expires eventually: an abandoned CLI (Ctrl-C) leaves
    // the waiter, its fd, and its per-chunk tail work behind forever.
    let ms = timeout_ms.unwrap_or(24 * 3600 * 1000).min(24 * 3600 * 1000);
    let deadline = Some(Instant::now() + Duration::from_millis(ms));
    Err(PendingWait { pane, cond, deadline })
}

fn write_pty(rt: &mut Runtime, pane: u64, bytes: &[u8]) -> Value {
    match rt.panes.get_mut(&PaneId(pane)) {
        Some(p) => {
            p.pty.write(bytes);
            json!({"ok": true})
        }
        None => err(format!("no such pane %{pane}")),
    }
}

fn pane_list(rt: &Runtime) -> Value {
    let focused = rt.state.focused_pane();
    let mut panes = Vec::new();
    for ws in &rt.state.workspaces {
        for tab in &ws.tabs {
            for id in tab.layout.panes() {
                let Some(p) = rt.panes.get(&id) else { continue };
                let title = rt.titles.get(&id).map(String::as_str).unwrap_or("");
                panes.push(json!({
                    "id": id.0,
                    "workspace": ws.id.0,
                    "tab": tab.id.0,
                    "program": p.program,
                    "title": title,
                    "agent": p.agent,
                    // The pid a status report must come from (hooks pass
                    // $PPID); null until the agent process is spotted.
                    "agent_pid": p.agent_pid,
                    "status": p.effective_status().word(),
                    // "done" | "blocked" while the user has not looked at
                    // the pane since it finished/blocked (the sidebar marks
                    // it); null once seen.
                    "unseen": p.unseen.map(|k| match k {
                        crate::runtime::NoticeKind::Done => "done",
                        crate::runtime::NoticeKind::Blocked => "blocked",
                    }),
                    "focused": id == focused,
                }));
            }
        }
    }
    json!({"ok": true, "panes": panes})
}

/// One-shot full-state reply: everything a script needs to orient itself.
fn snapshot(rt: &Runtime) -> Value {
    let focused = rt.state.focused_pane();
    let workspaces: Vec<Value> = rt
        .state
        .workspaces
        .iter()
        .enumerate()
        .map(|(wi, ws)| {
            let tabs: Vec<Value> = ws
                .tabs
                .iter()
                .enumerate()
                .map(|(ti, tab)| {
                    let panes: Vec<Value> = tab
                        .layout
                        .panes()
                        .into_iter()
                        .filter_map(|id| {
                            let p = rt.panes.get(&id)?;
                            let title = rt.titles.get(&id).map(String::as_str).unwrap_or("");
                            Some(json!({
                                "id": id.0,
                                "program": p.program,
                                "title": title,
                                "agent": p.agent,
                                "agent_session": rt.agent_sessions.get(&id),
                                "status": p.effective_status().word(),
                                "focused": id == focused,
                            }))
                        })
                        .collect();
                    json!({
                        "id": tab.id.0,
                        "name": tab.name,
                        "active": ti == ws.active_tab,
                        "panes": panes,
                    })
                })
                .collect();
            json!({
                "id": ws.id.0,
                "name": ws.name,
                "cwd": ws.cwd.to_string_lossy(),
                "branch": rt.branches.get(&ws.id),
                "active": wi == rt.state.active_workspace,
                "tabs": tabs,
            })
        })
        .collect();
    json!({"ok": true, "workspaces": workspaces})
}

/// Append a PTY chunk to output-waiters of this pane (called from the
/// server's data path) — capped rolling tail so needles can't scroll past.
pub fn feed_waiters(waiters: &mut [(PendingWait, Replier)], pane: PaneId, chunk: &[u8]) {
    for (w, _) in waiters.iter_mut() {
        if w.pane == pane
            && let WaitCond::Output(_, tail) = &mut w.cond
        {
            tail.push_str(&String::from_utf8_lossy(chunk));
            if tail.len() > 16 * 1024 {
                // Char-boundary-safe cut: agent TUIs stream multibyte glyphs
                // and a raw byte offset would panic the whole server loop.
                let mut cut = tail.len() - 8 * 1024;
                while !tail.is_char_boundary(cut) {
                    cut -= 1;
                }
                tail.drain(..cut);
            }
        }
    }
}

/// Resolve parked waits: condition met, pane gone, or deadline passed.
/// Called from the server's 500ms agent poll — that granularity is the
/// wait resolution.
pub fn check_waiters(rt: &Runtime, waiters: &mut Vec<(PendingWait, Replier)>) {
    // The CLI hung up (Ctrl-C)? Nobody is listening — drop the waiter.
    waiters.retain(|(_, tx)| !tx.is_closed());
    let mut i = 0;
    while i < waiters.len() {
        let (w, _) = &waiters[i];
        let result = match rt.panes.get(&w.pane) {
            None => Some(err("pane closed")),
            Some(p) => match &w.cond {
                WaitCond::Status(want) if p.effective_status() == *want => {
                    Some(json!({"ok": true, "status": want.word()}))
                }
                WaitCond::Output(needle, tail) => {
                    let on_screen = p.emu.bottom_text(30).join("\n").contains(needle.as_str());
                    if on_screen || tail.contains(needle.as_str()) {
                        Some(json!({"ok": true}))
                    } else if w.deadline.is_some_and(|d| Instant::now() >= d) {
                        Some(err("timeout"))
                    } else {
                        None
                    }
                }
                _ if w.deadline.is_some_and(|d| Instant::now() >= d) => Some(err("timeout")),
                _ => None,
            },
        };
        match result {
            Some(v) => {
                let (_, tx) = waiters.swap_remove(i);
                let _ = tx.send(v);
            }
            None => i += 1,
        }
    }
}

/// API socket path for the session (mirrors proto::socket_path).
pub fn socket_path() -> Option<std::path::PathBuf> {
    let name = std::env::var("CDOCK_SESSION").unwrap_or_else(|_| "default".to_string());
    crate::logging::state_dir().map(|d| d.join(format!("api-{name}.sock")))
}

/// Serve one API connection: NDJSON lines in, one reply line per request.
/// A `subscribe` request flips the connection into a one-way event stream.
pub fn spawn_conn(stream: tokio::net::UnixStream, tx: mpsc::UnboundedSender<ConnMsg>) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    tokio::spawn(async move {
        let (r, mut w) = stream.into_split();
        let mut lines = BufReader::new(r).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if line.trim().is_empty() {
                continue;
            }
            let v: Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(e) => {
                    let out = format!("{}\n", err(format!("bad request: {e}")));
                    if w.write_all(out.as_bytes()).await.is_err() {
                        return;
                    }
                    continue;
                }
            };
            if v["cmd"] == "subscribe" {
                let spec: SubSpec = serde_json::from_value(v).unwrap_or_default();
                let (etx, mut erx) = mpsc::unbounded_channel::<Value>();
                if tx.send(ConnMsg::Sub(spec, etx)).is_err() {
                    return;
                }
                let _ = w.write_all(b"{\"ok\":true,\"subscribed\":true}\n").await;
                while let Some(event) = erx.recv().await {
                    let mut out = event.to_string();
                    out.push('\n');
                    if w.write_all(out.as_bytes()).await.is_err() {
                        return; // reader gone; sender side prunes on next emit
                    }
                }
                return;
            }
            let reply = match serde_json::from_value::<Req>(v) {
                Ok(req) => {
                    let (rtx, rrx) = oneshot::channel();
                    if tx.send(ConnMsg::Req(req, rtx)).is_err() {
                        break;
                    }
                    rrx.await.unwrap_or_else(|_| err("server shutting down"))
                }
                Err(e) => err(format!("bad request: {e}")),
            };
            let mut reply = reply;
            // Version skew detection: old CLIs silently ignore new fields,
            // so every reply names the server that produced it.
            if let Some(obj) = reply.as_object_mut() {
                obj.insert(
                    "server_version".to_string(),
                    Value::String(crate::update::CURRENT.to_string()),
                );
            }
            let mut out = reply.to_string();
            out.push('\n');
            if w.write_all(out.as_bytes()).await.is_err() {
                break;
            }
        }
    });
}

/// Blocking subscription from the CLI: prints a line per event via `f`,
/// until the server goes away.
pub fn subscribe(spec: &SubSpec, mut f: impl FnMut(Value)) -> std::io::Result<()> {
    let sock = socket_path().ok_or_else(|| std::io::Error::other("cannot determine state dir"))?;
    let mut stream = std::os::unix::net::UnixStream::connect(&sock).map_err(|e| {
        std::io::Error::other(format!("no cdock server on {sock:?} ({e}); start `cdock` first"))
    })?;
    let mut line = serde_json::to_string(&serde_json::json!({
        "cmd": "subscribe", "events": spec.events, "pane": spec.pane,
    }))?;
    line.push('\n');
    stream.write_all(line.as_bytes())?;
    let reader = std::io::BufReader::new(stream);
    for line in std::io::BufRead::lines(reader) {
        let line = line?;
        match serde_json::from_str::<Value>(&line) {
            Ok(v) if v["subscribed"] == true => {}
            Ok(v) => f(v),
            Err(_) => {}
        }
    }
    // The server never ends a subscription voluntarily — EOF means it went
    // away (handoff, crash). Exit non-zero so watching scripts notice
    // instead of dying silently with success.
    Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "event stream ended (server restarted?)"))
}

/// Blocking one-shot request from the CLI side. No read timeout — wait-*
/// requests legitimately hold the line for minutes.
pub fn request(req: &Req) -> std::io::Result<Value> {
    request_inner(req, None)
}

/// Bounded request for hook contexts: the SessionStart hook runs inside
/// every claude launch — a wedged server loop must not stall the agent for
/// Claude Code's full 60s hook timeout.
pub fn request_with_timeout(req: &Req, timeout: Duration) -> std::io::Result<Value> {
    request_inner(req, Some(timeout))
}

fn request_inner(req: &Req, timeout: Option<Duration>) -> std::io::Result<Value> {
    let sock = socket_path().ok_or_else(|| std::io::Error::other("cannot determine state dir"))?;
    let stream = std::os::unix::net::UnixStream::connect(&sock)
        .map_err(|e| std::io::Error::other(format!("no cdock server on {sock:?} ({e}); start `cdock` first")))?;
    stream.set_read_timeout(timeout)?;
    stream.set_write_timeout(timeout)?;
    let mut stream = stream;
    let mut line = serde_json::to_string(req)?;
    line.push('\n');
    stream.write_all(line.as_bytes())?;
    let mut reader = std::io::BufReader::new(stream);
    let mut resp = String::new();
    reader.read_line(&mut resp)?;
    serde_json::from_str(&resp).map_err(std::io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every example in the published reference must parse as a real
    /// request ("subscribe" is connection-level, checked by shape).
    #[test]
    fn reference_examples_parse() {
        let examples: Vec<serde_json::Value> =
            serde_json::from_str(REFERENCE).expect("reference is valid JSON");
        assert!(examples.len() >= 25);
        for ex in examples {
            if ex["cmd"] == "subscribe" {
                assert!(serde_json::from_value::<SubSpec>(ex).is_ok());
            } else {
                let text = ex.to_string();
                assert!(
                    serde_json::from_value::<Req>(ex).is_ok(),
                    "reference example does not parse: {text}"
                );
            }
        }
    }

    /// The wire format is public API for scripts — keep it stable.
    #[test]
    fn req_wire_format() {
        let req: Req =
            serde_json::from_str(r#"{"cmd":"wait-output","pane":4,"match":"ok","timeout_ms":5}"#)
                .expect("wait-output parses");
        assert!(matches!(req, Req::WaitOutput { pane: 4, .. }));
        assert!(serde_json::from_str::<Req>(r#"{"cmd":"pane-list"}"#).is_ok());
        assert!(serde_json::from_str::<Req>(r#"{"cmd":"split","direction":"down"}"#).is_ok());
        assert!(serde_json::from_str::<Req>(r#"{"cmd":"nope"}"#).is_err());
        assert_eq!(parse_status("blocked"), Some(crate::detect::Status::Blocked));
        assert_eq!(parse_status("bogus"), None);
    }
}
