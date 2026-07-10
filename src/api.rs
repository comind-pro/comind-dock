//! JSON automation API (Phase 4): newline-delimited JSON request/response on
//! a second unix socket (`api-<session>.sock`). One line in → one line out;
//! `wait-*` requests hold the line until the condition or timeout.
//! ponytail: event subscriptions and a published JSON Schema come later;
//! request/response + one-shot waits cover the agent-drives-runtime loop.

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
    ReportAgentSession { pane: u64, session_id: String },
    /// Re-read detection manifests from disk (bundled + overrides).
    /// Handled directly by the server loop, which owns the manifest set.
    ReloadManifests,
    /// Re-read config/keymap/theme. Also on the sidebar app menu.
    ReloadConfig,
    /// exec() the current binary in place: same pid, panes survive.
    Handoff,
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
    Output(String),
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
            if !rt.state.focus_pane(target) {
                return Ok(err(format!("no such pane {target}")));
            }
            let dir = match direction.as_deref() {
                Some("down") => Dir::Down,
                _ => Dir::Right,
            };
            let new = rt.state.split_focused(dir, false);
            // Provisional size; compute_view corrects it before the next frame.
            match rt.spawn_pane_cmd(new, area.width.max(2) / 2, area.height.max(2) / 2, command) {
                Ok(()) => {
                    rt.mark_dirty();
                    Ok(json!({"ok": true, "pane": new.0}))
                }
                Err(e) => Ok(err(e)),
            }
        }
        Req::ReloadConfig => {
            rt.reload_config();
            Ok(json!({"ok": true}))
        }
        // Owned by the server loop; reaching here is a wiring bug.
        Req::ReloadManifests | Req::Handoff => Ok(err("handled by the server loop")),
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
            if let Some(wi) = workspace.map(|w| resolve_ws(rt, Some(w))) {
                let Some(wi) = wi else { return Ok(err("no such workspace")) };
                rt.state.active_workspace = wi;
            }
            let pane = match split.as_deref() {
                Some("down") => rt.state.split_focused(Dir::Down, false),
                Some(_) => rt.state.split_focused(Dir::Right, false),
                None => rt.state.new_tab(),
            };
            match rt.spawn_pane_env(
                pane,
                area.width.max(2) / 2,
                area.height.max(2) / 2,
                Some(command),
                env,
            ) {
                Ok(()) => {
                    rt.mark_dirty();
                    Ok(json!({"ok": true, "pane": pane.0}))
                }
                Err(e) => Ok(err(e)),
            }
        }
        Req::ReportAgentSession { pane, session_id } => {
            let pane = PaneId(pane);
            if !rt.panes.contains_key(&pane) {
                return Ok(err(format!("no such pane {pane}")));
            }
            rt.agent_sessions.insert(pane, session_id);
            rt.save_session(); // survive a crash between autosaves
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
            let pane = rt.state.new_workspace(name, cwd, None);
            match rt.spawn_pane(pane, area.width.max(4), area.height.max(4)) {
                Ok(()) => {
                    rt.mark_dirty();
                    let ws = rt.state.active_workspace();
                    Ok(json!({"ok": true, "workspace": ws.id.0, "pane": pane.0}))
                }
                Err(e) => Ok(err(e)),
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
            rt.state.active_workspace = wi;
            let pane = rt.state.new_tab();
            match rt.spawn_pane(pane, area.width.max(4), area.height.max(4)) {
                Ok(()) => {
                    rt.mark_dirty();
                    Ok(json!({"ok": true, "pane": pane.0}))
                }
                Err(e) => Ok(err(e)),
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
                    rt.open_worktree(parent_id, path.clone(), area);
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
                    rt.open_worktree(parent_id, path.clone(), area);
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
            wait(rt, pane, WaitCond::Output(needle), timeout_ms)
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
  {"cmd":"reload-manifests"},
  {"cmd":"reload-config"},
  {"cmd":"handoff"},
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
    let deadline = timeout_ms.map(|ms| Instant::now() + Duration::from_millis(ms));
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
                    "status": p.effective_status().word(),
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

/// Resolve parked waits: condition met, pane gone, or deadline passed.
/// Called from the server's 500ms agent poll — that granularity is the
/// wait resolution.
pub fn check_waiters(rt: &Runtime, waiters: &mut Vec<(PendingWait, Replier)>) {
    let mut i = 0;
    while i < waiters.len() {
        let (w, _) = &waiters[i];
        let result = match rt.panes.get(&w.pane) {
            None => Some(err("pane closed")),
            Some(p) => match &w.cond {
                WaitCond::Status(want) if p.effective_status() == *want => {
                    Some(json!({"ok": true, "status": want.word()}))
                }
                WaitCond::Output(needle) if p.emu.bottom_text(30).join("\n").contains(needle) => {
                    Some(json!({"ok": true}))
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
    Ok(())
}

/// Blocking one-shot request from the CLI side.
pub fn request(req: &Req) -> std::io::Result<Value> {
    let sock = socket_path().ok_or_else(|| std::io::Error::other("cannot determine state dir"))?;
    let mut stream = std::os::unix::net::UnixStream::connect(&sock)
        .map_err(|e| std::io::Error::other(format!("no cdock server on {sock:?} ({e}); start `cdock` first")))?;
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
