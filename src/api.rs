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
    /// Split a pane (default: focused) and spawn a shell or command in it.
    Split { pane: Option<u64>, direction: Option<String>, command: Option<String> },
    /// Write literal text to a pane's PTY (no Enter).
    SendText { pane: u64, text: String },
    /// Write text + Enter.
    Run { pane: u64, command: String },
    /// Read the last non-empty screen lines of a pane.
    Read { pane: u64, lines: Option<usize> },
    Focus { pane: u64 },
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
                    "agent": crate::agents::detect(title, &p.program),
                    "status": p.effective_status().word(),
                    "focused": id == focused,
                }));
            }
        }
    }
    json!({"ok": true, "panes": panes})
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
pub fn spawn_conn(stream: tokio::net::UnixStream, tx: mpsc::UnboundedSender<(Req, Replier)>) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    tokio::spawn(async move {
        let (r, mut w) = stream.into_split();
        let mut lines = BufReader::new(r).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if line.trim().is_empty() {
                continue;
            }
            let reply = match serde_json::from_str::<Req>(&line) {
                Ok(req) => {
                    let (rtx, rrx) = oneshot::channel();
                    if tx.send((req, rtx)).is_err() {
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
