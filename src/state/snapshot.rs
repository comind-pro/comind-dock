//! Session snapshot: the tree (workspaces, tabs, split shapes, names) plus
//! per-pane restore metadata — the agent ident to resume, the pane's own
//! cwd, profile env, behavior role, the user's name for it. Pane ids are
//! reallocated on restore; the saved id survives only to key the pane's
//! screen-history file (bottom of this module).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::ids::{IdGen, PaneId};
use super::layout::{Dir, Node};
use super::workspace::{Tab, Workspace};
use super::{AppState, InputMode};

/// What restore knows about a pane: the agent ident that ran there (if
/// any) and the pane's own cwd — an agent must resume in ITS folder, not
/// wherever the workspace cwd drifted to.
#[derive(Debug, Clone, Default)]
pub struct PaneMeta {
    pub agent: Option<String>,
    pub cwd: Option<PathBuf>,
    /// Extra env restore must respawn with (e.g. CLAUDE_CONFIG_DIR profile).
    pub env: Vec<(String, String)>,
    /// Absolute exe path of the agent (resume without relying on PATH).
    pub agent_bin: Option<String>,
    /// Behavior profile ident attached to the pane ("global:x" | "ws:x").
    pub behavior: Option<String>,
    /// User-given pane name (wins over the agent's OSC title).
    pub name: Option<String>,
    /// Pane id at SAVE time — keys the screens-<session>/pane-<id>.txt
    /// file; restore-side only (save derives it from the layout leaf).
    pub saved_pane: Option<u64>,
}

/// A pane to spawn on restore with its metadata.
pub type PaneSpawn = (PaneId, PaneMeta);

#[derive(Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub active_workspace: usize,
    pub workspaces: Vec<WsSnap>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WsSnap {
    pub name: String,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub custom_name: bool,
    /// Index of the parent workspace (worktree grouping).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<usize>,
    /// The space's default agent profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    pub active_tab: usize,
    pub tabs: Vec<TabSnap>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TabSnap {
    pub name: String,
    pub layout: NodeSnap,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeSnap {
    /// A pane; `agent` records which agent CLI ran there so restore can
    /// relaunch it into its conversation, `cwd` where the pane's process
    /// actually lived (agent sessions are folder-bound).
    Leaf {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        env: Vec<(String, String)>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_bin: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        behavior: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        /// Pane id at save time — names the screen-history file.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pane: Option<u64>,
    },
    Split {
        dir: DirSnap,
        ratio: f32,
        a: Box<NodeSnap>,
        b: Box<NodeSnap>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirSnap {
    Right,
    Down,
}

fn node_to_snap(node: &Node, panes: &std::collections::HashMap<PaneId, PaneMeta>) -> NodeSnap {
    match node {
        Node::Leaf(id) => {
            let meta = panes.get(id).cloned().unwrap_or_default();
            NodeSnap::Leaf {
                agent: meta.agent,
                cwd: meta.cwd.map(|c| c.to_string_lossy().into_owned()),
                env: meta.env,
                agent_bin: meta.agent_bin,
                behavior: meta.behavior,
                name: meta.name,
                pane: Some(id.0),
            }
        }
        Node::Split { dir, ratio, a, b } => NodeSnap::Split {
            dir: match dir {
                Dir::Right => DirSnap::Right,
                Dir::Down => DirSnap::Down,
            },
            ratio: *ratio,
            a: Box::new(node_to_snap(a, panes)),
            b: Box::new(node_to_snap(b, panes)),
        },
    }
}

fn snap_to_node(snap: &NodeSnap, ids: &mut IdGen, agents: &mut Vec<PaneSpawn>) -> Node {
    match snap {
        NodeSnap::Leaf { agent, cwd, env, agent_bin, behavior, name, pane } => {
            let id = ids.pane();
            agents.push((
                id,
                PaneMeta {
                    agent: agent.clone(),
                    cwd: cwd.clone().map(PathBuf::from),
                    env: env.clone(),
                    agent_bin: agent_bin.clone(),
                    behavior: behavior.clone(),
                    name: name.clone(),
                    saved_pane: *pane,
                },
            ));
            Node::Leaf(id)
        }
        NodeSnap::Split { dir, ratio, a, b } => Node::Split {
            dir: match dir {
                DirSnap::Right => Dir::Right,
                DirSnap::Down => Dir::Down,
            },
            ratio: ratio.clamp(0.05, 0.95),
            a: Box::new(snap_to_node(a, ids, agents)),
            b: Box::new(snap_to_node(b, ids, agents)),
        },
    }
}

impl Snapshot {
    /// `panes`: per-pane restore metadata (agent ident, actual cwd).
    pub fn of(state: &AppState, panes: &std::collections::HashMap<PaneId, PaneMeta>) -> Self {
        Snapshot {
            active_workspace: state.active_workspace,
            workspaces: state
                .workspaces
                .iter()
                .map(|ws| WsSnap {
                    name: ws.name.clone(),
                    cwd: ws.cwd.to_string_lossy().into_owned(),
                    custom_name: ws.custom_name,
                    parent: ws
                        .parent
                        .and_then(|pid| state.workspaces.iter().position(|w| w.id == pid)),
                    profile: ws.profile.clone(),
                    active_tab: ws.active_tab,
                    tabs: ws
                        .tabs
                        .iter()
                        .map(|t| TabSnap {
                            name: t.name.clone(),
                            layout: node_to_snap(&t.layout, panes),
                        })
                        .collect(),
                })
                .collect(),
        }
    }

    /// Rebuild state with fresh pane ids. Returns the state and every pane
    /// to spawn with the agent that ran there. None if the snapshot is
    /// empty/degenerate.
    pub fn restore(&self) -> Option<(AppState, Vec<PaneSpawn>)> {
        let mut ids = IdGen::default();
        let mut workspaces = Vec::new();
        let mut panes = Vec::new();
        // Snapshot index → restored index: skipped (degenerate) workspaces
        // shift positions, and parent links are by index.
        let mut idx_map: Vec<Option<usize>> = Vec::with_capacity(self.workspaces.len());
        for ws in &self.workspaces {
            let mut tabs = Vec::new();
            for tab in &ws.tabs {
                let layout = snap_to_node(&tab.layout, &mut ids, &mut panes);
                let focused = *layout.panes().first()?;
                tabs.push(Tab {
                    id: ids.tab(),
                    name: tab.name.clone(),
                    layout,
                    zoomed: None,
                    focused_pane: focused,
                });
            }
            if tabs.is_empty() {
                idx_map.push(None);
                continue;
            }
            let active_tab = ws.active_tab.min(tabs.len() - 1);
            let cwd = if ws.cwd.is_empty() {
                std::env::current_dir().unwrap_or_else(|_| "/".into())
            } else {
                std::path::PathBuf::from(&ws.cwd)
            };
            idx_map.push(Some(workspaces.len()));
            workspaces.push(Workspace {
                id: ids.workspace(),
                name: ws.name.clone(),
                cwd,
                custom_name: ws.custom_name,
                parent: None, // linked below by saved index
                profile: ws.profile.clone(),
                tabs,
                active_tab,
            });
        }
        if workspaces.is_empty() {
            return None;
        }
        // Re-link worktree parents through the index map — restored
        // positions shift when a degenerate workspace was skipped.
        for (snap_i, snap_ws) in self.workspaces.iter().enumerate() {
            if let Some(pi) = snap_ws.parent
                && let Some(Some(child)) = idx_map.get(snap_i)
                && let Some(Some(parent)) = idx_map.get(pi)
                && child != parent
            {
                workspaces[*child].parent = Some(workspaces[*parent].id);
            }
        }
        // Through idx_map: skipped degenerate workspaces shift indices,
        // same as parent links above.
        let active_workspace = idx_map
            .get(self.active_workspace)
            .copied()
            .flatten()
            .unwrap_or(0)
            .min(workspaces.len() - 1);
        let pane_names =
            panes.iter().filter_map(|(id, m)| m.name.clone().map(|n| (*id, n))).collect();
        let state = AppState {
            pane_names,
            workspaces,
            active_workspace,
            sidebar_visible: true,
            scope: None,
            input_mode: InputMode::Terminal,
            ids,
        };
        // Keep only panes that survived (degenerate tabs were skipped).
        panes.retain(|(id, _)| {
            state.workspaces.iter().any(|w| w.tabs.iter().any(|t| t.layout.contains(*id)))
        });
        state.check_invariants();
        Some((state, panes))
    }
}

pub fn path() -> Option<PathBuf> {
    let name = std::env::var("CDOCK_SESSION").unwrap_or_else(|_| "default".to_string());
    let dir = crate::logging::state_dir()?;
    let named = dir.join(format!("session-{name}.json"));
    // Migrate the pre-namespacing file once (default session only).
    if name == "default" && !named.exists() {
        let legacy = dir.join("session.json");
        if legacy.exists() {
            let _ = std::fs::rename(&legacy, &named);
        }
    }
    Some(named)
}

/// Write via tmp + rename: a crash, kill, or full disk mid-write must not
/// truncate the only good copy. fsync before the rename — renaming unsynced
/// data over the old file can survive a power cut as an empty file.
fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let tmp = path.with_extension("json.tmp");
    let mut f = std::fs::File::create(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, path)?;
    // Durability of the rename itself needs the parent dir synced too.
    // Best-effort: a filesystem that can't fsync a dir must not fail saves.
    if let Some(dir) = path.parent() {
        let _ = std::fs::File::open(dir).and_then(|d| d.sync_all());
    }
    Ok(())
}

/// A snapshot staged on the event loop and persisted later — possibly on a
/// blocking thread. The sequence number lets a late write of an older
/// snapshot be dropped instead of clobbering a newer one.
pub struct Pending {
    seq: u64,
    snap: Snapshot,
    screens_enabled: bool,
    screens: Vec<(u64, Option<String>)>,
}

pub fn stage(
    state: &AppState,
    panes: &std::collections::HashMap<PaneId, PaneMeta>,
    screens_enabled: bool,
    screens: Vec<(u64, Option<String>)>,
) -> Pending {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    Pending {
        seq: NEXT.fetch_add(1, Ordering::Relaxed),
        snap: Snapshot::of(state, panes),
        screens_enabled,
        screens,
    }
}

/// Persist a staged snapshot to the session's files. Safe to call from a
/// blocking thread; concurrent calls serialize on an internal lock.
pub fn persist(p: Pending) {
    let Some(file) = path() else { return };
    let Some(scr) = screens_dir() else { return };
    persist_at(p, &file, &scr);
}

fn persist_at(p: Pending, file: &std::path::Path, screens: &std::path::Path) {
    // Lock held across the writes: overlapping persists must not interleave,
    // and an older seq must never land after a newer one.
    static LAST: std::sync::Mutex<u64> = std::sync::Mutex::new(0);
    let mut last = LAST.lock().unwrap();
    if p.seq < *last {
        return;
    }
    *last = p.seq;
    match serde_json::to_string_pretty(&p.snap) {
        Ok(json) => {
            if let Err(e) = write_atomic(file, json.as_bytes()) {
                tracing::warn!(error = %e, "failed to save session snapshot");
            }
        }
        Err(e) => tracing::warn!(error = %e, "failed to serialize session snapshot"),
    }
    persist_screens(p.screens_enabled, screens, &p.screens);
}

pub fn load() -> Option<Snapshot> {
    // On parse failure the file is renamed aside: within 5s the autosave
    // would overwrite it with a fresh state, and the next boot would copy
    // that over boot-bak — silently destroying the only good copy.
    let p = path()?;
    let text = std::fs::read_to_string(&p).ok()?;
    match serde_json::from_str(&text) {
        Ok(s) => Some(s),
        Err(e) => {
            let bad = p.with_extension("json.bad");
            let _ = std::fs::rename(&p, &bad);
            tracing::warn!(error = %e, kept = %bad.display(), "session snapshot unreadable; starting fresh");
            None
        }
    }
}

// ── Screen history ──────────────────────────────────────────────────
// Plain-text tail of each pane's scrollback, one file per pane, replayed
// into the emulator on cold restore. ponytail: text only — styled replay
// would mean serializing the grid; the words above the prompt are the value.

pub const SCREEN_MAX_LINES: usize = 200;
const SCREEN_MAX_BYTES: usize = 64 * 1024;

/// Where this session's per-pane screen tails live.
pub fn screens_dir() -> Option<PathBuf> {
    let name = std::env::var("CDOCK_SESSION").unwrap_or_else(|_| "default".to_string());
    Some(crate::logging::state_dir()?.join(format!("screens-{name}")))
}

fn screen_file(dir: &std::path::Path, id: u64) -> PathBuf {
    dir.join(format!("pane-{id}.txt"))
}

/// Last `SCREEN_MAX_LINES` lines, hard-capped at `SCREEN_MAX_BYTES`. The
/// byte cut lands after a newline, so it can't split a UTF-8 char either.
fn screen_tail(text: &str) -> String {
    // The separator an earlier restore replayed is plain emulator text now.
    // Drop it, or every restart saves one more and they stack up.
    let cleaned: String = match text.contains(RESTORED_SEP) {
        true => text.split_inclusive('\n').filter(|l| l.trim_end() != RESTORED_SEP).collect(),
        false => text.to_owned(),
    };
    let mut s: &str = &cleaned;
    if let Some((i, _)) = s.rmatch_indices('\n').nth(SCREEN_MAX_LINES) {
        s = &s[i + 1..];
    }
    if s.len() > SCREEN_MAX_BYTES {
        let over = s.len() - SCREEN_MAX_BYTES;
        // ponytail: a single line longer than the whole cap is dropped.
        s = match s.as_bytes()[over..].iter().position(|&b| b == b'\n') {
            Some(nl) => &s[over + nl + 1..],
            None => "",
        };
    }
    s.to_owned()
}

/// The separator replay draws between restored history and live output.
const RESTORED_SEP: &str = "\u{2500}\u{2500} restored \u{2500}\u{2500}";

/// Sanitized bytes to feed the emulator on restore: control chars (incl.
/// ESC) stripped except \t, \n → \r\n, dim "restored" separator appended.
pub fn screen_replay(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() + 32);
    let mut buf = [0u8; 4];
    for c in text.chars() {
        match c {
            '\n' => out.extend_from_slice(b"\r\n"),
            '\t' => out.push(b'\t'),
            c if c.is_control() => {}
            c => out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes()),
        }
    }
    out.extend_from_slice(format!("\x1b[2m{RESTORED_SEP}\x1b[0m\r\n").as_bytes());
    out
}

/// Write per-pane screen tails under `dir`; files of dead panes are removed.
/// `None` text = keep the pane's existing file untouched (alt-screen panes:
/// the visible TUI frame is garbage to replay, but the primary-screen tail
/// saved earlier is still the right thing to restore).
/// Root-parameterized so tests run against a temp dir.
pub fn save_screens(dir: &std::path::Path, screens: &[(u64, Option<String>)]) {
    if let Err(e) = std::fs::create_dir_all(dir) {
        tracing::warn!(error = %e, "screens dir create failed");
        return;
    }
    let live: std::collections::HashSet<std::ffi::OsString> =
        screens.iter().map(|(id, _)| format!("pane-{id}.txt").into()).collect();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            if !live.contains(&e.file_name()) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    for (id, text) in screens {
        let Some(text) = text else { continue };
        if let Err(e) = std::fs::write(screen_file(dir, *id), screen_tail(text)) {
            tracing::warn!(error = %e, pane = id, "screen save failed");
        }
    }
}

/// Persistence gate for screen tails ([restore] screen_history). Disabled
/// doesn't just skip the write — it purges anything stored earlier, so
/// flipping the flag off also scrubs old secrets from disk.
pub fn persist_screens(enabled: bool, dir: &std::path::Path, screens: &[(u64, Option<String>)]) {
    if enabled {
        save_screens(dir, screens);
    } else {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// Replay gate: when the feature is off, stored tails (e.g. from an older
/// build) are never read back into a pane.
pub fn restore_screen(enabled: bool, dir: &std::path::Path, id: u64) -> Option<String> {
    if enabled { take_screen(dir, id) } else { None }
}

/// One-shot read of a pane's saved screen: the file is deleted on success.
pub fn take_screen(dir: &std::path::Path, id: u64) -> Option<String> {
    let p = screen_file(dir, id);
    let text = std::fs::read_to_string(&p).ok()?;
    let _ = std::fs::remove_file(&p);
    (!text.trim().is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::layout::Dir;

    #[test]
    fn snapshot_round_trip_preserves_structure() {
        let mut s = AppState::new("main".into(), std::path::PathBuf::from("/tmp"));
        s.split_focused(Dir::Right, false);
        s.split_focused(Dir::Down, false);
        s.new_tab();
        s.new_workspace("x".into(), std::path::PathBuf::from("/tmp"), None);
        let ws_id = s.active_workspace().id;
        s.rename_workspace_by_id(ws_id, "proj".into());

        // Mark the focused pane of ws1/tab1 as a claude pane living in its
        // own folder — the workspace cwd may have drifted elsewhere.
        let claude_pane = s.workspaces[0].tabs[0].focused_pane;
        let metas = std::collections::HashMap::from([(
            claude_pane,
            PaneMeta {
                agent: Some("claude:uuid-1".to_string()),
                cwd: Some(std::path::PathBuf::from("/projects/real-home")),
                env: vec![("CLAUDE_CONFIG_DIR".into(), "/home/u/.claude-oleh".into())],
                agent_bin: Some("/usr/local/bin/claude".into()),
                behavior: Some("ws:researcher".into()),
                name: Some("kafka refactor".into()),
                saved_pane: None,
            },
        )]);
        let snap = Snapshot::of(&s, &metas);
        let json = serde_json::to_string(&snap).unwrap();
        let back: Snapshot = serde_json::from_str(&json).unwrap();
        let (restored, panes) = back.restore().unwrap();

        assert_eq!(restored.workspaces.len(), 2);
        assert_eq!(restored.workspaces[0].tabs.len(), 2);
        assert_eq!(restored.workspaces[0].tabs[0].layout.panes().len(), 3);
        assert_eq!(restored.workspaces[1].name, "proj");
        assert_eq!(panes.len(), 5);
        let agent: Vec<_> = panes.iter().filter(|(_, m)| m.agent.is_some()).collect();
        assert_eq!(agent.len(), 1, "exactly one pane remembers its agent");
        assert_eq!(agent[0].1.agent.as_deref(), Some("claude:uuid-1"));
        assert_eq!(
            agent[0].1.cwd.as_deref(),
            Some(std::path::Path::new("/projects/real-home")),
            "the agent restores in ITS folder, not the workspace's"
        );
        assert_eq!(
            agent[0].1.env,
            vec![("CLAUDE_CONFIG_DIR".to_string(), "/home/u/.claude-oleh".to_string())],
            "the profile env survives the round trip"
        );
        assert_eq!(
            agent[0].1.saved_pane,
            Some(claude_pane.0),
            "the OLD pane id rides along so restore finds the screen file"
        );
        assert_eq!(
            agent[0].1.behavior.as_deref(),
            Some("ws:researcher"),
            "the behavior ident survives the round trip"
        );
        assert_eq!(
            agent[0].1.name.as_deref(),
            Some("kafka refactor"),
            "the user's pane name survives the round trip"
        );
        assert_eq!(
            restored.pane_name(agent[0].0),
            Some("kafka refactor"),
            "and is seeded into the restored state under the NEW pane id"
        );
        assert!(restored.check_invariants());
    }

    /// A degenerate (empty-tabs) workspace between a parent and its worktree
    /// child shifts restored indexes — the parent link must survive.
    #[test]
    fn parent_relink_survives_skipped_workspace() {
        let json = r#"{
            "active_workspace": 0,
            "workspaces": [
                { "name": "repo", "cwd": "/r", "custom_name": false, "active_tab": 0,
                  "tabs": [ { "name": "1", "layout": { "leaf": {} } } ] },
                { "name": "ghost", "cwd": "/g", "custom_name": false, "active_tab": 0,
                  "tabs": [] },
                { "name": "feat", "cwd": "/w/feat", "custom_name": false, "parent": 0,
                  "active_tab": 0,
                  "tabs": [ { "name": "1", "layout": { "leaf": {} } } ] }
            ]
        }"#;
        let snap: Snapshot = serde_json::from_str(json).unwrap();
        let (restored, _) = snap.restore().unwrap();
        assert_eq!(restored.workspaces.len(), 2, "ghost dropped");
        assert_eq!(
            restored.workspaces[1].parent,
            Some(restored.workspaces[0].id),
            "feat still parents to repo despite the shifted index"
        );
    }

    #[test]
    fn alt_screen_pane_keeps_existing_tail() {
        let dir = std::env::temp_dir().join(format!("cdock-altkeep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        save_screens(&dir, &[(7, Some("primary tail\n".into()))]);
        // Next autosave: pane 7 is on the alt screen → None keeps the file.
        save_screens(&dir, &[(7, None)]);
        assert_eq!(take_screen(&dir, 7).as_deref(), Some("primary tail\n"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn stale_persist_never_clobbers_newer_snapshot() {
        let dir = std::env::temp_dir().join(format!("cdock-seq-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("session-test.json");
        let scr = dir.join("screens");
        let s1 = AppState::new("older".into(), std::path::PathBuf::from("/tmp"));
        let mut s2 = AppState::new("newer".into(), std::path::PathBuf::from("/tmp"));
        s2.new_tab();
        let older = stage(&s1, &std::collections::HashMap::new(), false, Vec::new());
        let newer = stage(&s2, &std::collections::HashMap::new(), false, Vec::new());
        persist_at(newer, &file, &scr);
        persist_at(older, &file, &scr); // late write of an older snapshot
        let kept: Snapshot =
            serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(kept.workspaces[0].tabs.len(), 2, "newer snapshot must survive");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn write_atomic_replaces_content_and_leaves_no_tmp() {
        let dir = std::env::temp_dir().join(format!("cdock-atomic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("session-test.json");
        write_atomic(&p, b"{\"v\":1}").unwrap();
        write_atomic(&p, b"{\"v\":2}").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "{\"v\":2}");
        let leftovers: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().collect();
        assert_eq!(leftovers.len(), 1, "no tmp file left behind");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn screens_disabled_purges_previously_stored_tails() {
        let dir = std::env::temp_dir().join(format!("cdock-scroff-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        save_screens(&dir, &[(7, Some("old secret\n".into()))]);
        persist_screens(false, &dir, &[(7, Some("new secret\n".into()))]);
        assert!(!dir.exists(), "disabled persistence must purge stored tails");
    }

    #[test]
    fn screens_opt_in_saves_and_replays() {
        let dir = std::env::temp_dir().join(format!("cdock-scron-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        persist_screens(true, &dir, &[(7, Some("tail\n".into()))]);
        assert_eq!(restore_screen(true, &dir, 7).as_deref(), Some("tail\n"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn replay_disabled_ignores_existing_file() {
        let dir = std::env::temp_dir().join(format!("cdock-scrgate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        save_screens(&dir, &[(7, Some("stale secret\n".into()))]);
        assert_eq!(restore_screen(false, &dir, 7), None, "off = never replayed");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn screen_tail_bounds_lines_and_bytes() {
        // Line cap: 300 lines in, last 200 out.
        let text: String = (0..300).map(|i| format!("line{i}\n")).collect();
        let tail = screen_tail(&text);
        assert_eq!(tail.lines().count(), SCREEN_MAX_LINES);
        assert!(tail.starts_with("line100\n"));
        assert!(tail.ends_with("line299\n"));

        // Byte cap: 200 fat lines blow the 64KB cap; result fits and
        // starts on a line boundary (no split UTF-8 chars).
        let fat: String = (0..200).map(|i| format!("{i}-{}\n", "é".repeat(500))).collect();
        let tail = screen_tail(&fat);
        assert!(tail.len() <= SCREEN_MAX_BYTES);
        assert!(tail.ends_with('\n'));
        assert!(tail.chars().next().unwrap().is_ascii_digit(), "cut lands on a line start");

        // Short text passes through untouched.
        assert_eq!(screen_tail("hi\n"), "hi\n");
    }

    /// Save → replay → save must not stack separators: the marker fed to the
    /// emulator on restore is scooped back up by the next autosave.
    #[test]
    fn screen_tail_drops_replayed_separators() {
        let replayed = String::from_utf8(screen_replay("prompt$\n")).unwrap();
        // The emulator's text afterwards: no ESC, and CRLF reads back as LF.
        let emu = replayed.replace("\r\n", "\n").replace("\u{1b}[2m", "").replace("\u{1b}[0m", "");
        assert!(emu.contains(RESTORED_SEP), "the marker is in the emulator");

        let saved = screen_tail(&format!("{emu}prompt$\n"));
        assert!(!saved.contains(RESTORED_SEP), "but never lands back in the file: {saved:?}");
        assert_eq!(saved, "prompt$\nprompt$\n", "real output is untouched");
    }

    #[test]
    fn screen_replay_sanitizes() {
        let out = screen_replay("a\x1b[31mb\r\x07c\nd\te\n");
        let s = String::from_utf8(out).unwrap();
        let (body, sep) = s.split_at(s.find("\u{1b}[2m").unwrap());
        assert_eq!(body, "a[31mbc\r\nd\te\r\n", "ESC/CR/BEL stripped, \\n → \\r\\n, \\t kept");
        assert_eq!(sep, "\u{1b}[2m\u{2500}\u{2500} restored \u{2500}\u{2500}\u{1b}[0m\r\n");
    }

    #[test]
    fn screens_save_take_round_trip_and_stale_cleanup() {
        let dir = std::env::temp_dir().join(format!("cdock-screens-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        save_screens(&dir, &[(1, Some("one\n".into())), (2, Some("two\n".into()))]);
        // Pane 2 died: its file must be swept on the next save.
        save_screens(&dir, &[(1, Some("one\n".into()))]);
        assert_eq!(take_screen(&dir, 2), None, "stale file swept");
        assert_eq!(take_screen(&dir, 1).as_deref(), Some("one\n"));
        assert_eq!(take_screen(&dir, 1), None, "read is one-shot");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
