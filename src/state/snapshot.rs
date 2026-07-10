//! Session snapshot: the tree STRUCTURE (workspaces, tabs, split shapes,
//! names) saved on quit and restored on start. Pane ids are not persisted —
//! restore allocates fresh ones and the runtime spawns fresh shells.
//! ponytail: structure only; cwds and screen history are Phase 2 persistence.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::ids::{IdGen, PaneId};
use super::layout::{Dir, Node};
use super::workspace::{Tab, Workspace};
use super::{AppState, InputMode};

#[derive(Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub active_workspace: usize,
    pub workspaces: Vec<WsSnap>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WsSnap {
    pub name: String,
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
    Leaf,
    Split { dir: DirSnap, ratio: f32, a: Box<NodeSnap>, b: Box<NodeSnap> },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirSnap {
    Right,
    Down,
}

fn node_to_snap(node: &Node) -> NodeSnap {
    match node {
        Node::Leaf(_) => NodeSnap::Leaf,
        Node::Split { dir, ratio, a, b } => NodeSnap::Split {
            dir: match dir {
                Dir::Right => DirSnap::Right,
                Dir::Down => DirSnap::Down,
            },
            ratio: *ratio,
            a: Box::new(node_to_snap(a)),
            b: Box::new(node_to_snap(b)),
        },
    }
}

fn snap_to_node(snap: &NodeSnap, ids: &mut IdGen) -> Node {
    match snap {
        NodeSnap::Leaf => Node::Leaf(ids.pane()),
        NodeSnap::Split { dir, ratio, a, b } => Node::Split {
            dir: match dir {
                DirSnap::Right => Dir::Right,
                DirSnap::Down => Dir::Down,
            },
            ratio: ratio.clamp(0.05, 0.95),
            a: Box::new(snap_to_node(a, ids)),
            b: Box::new(snap_to_node(b, ids)),
        },
    }
}

impl Snapshot {
    pub fn of(state: &AppState) -> Self {
        Snapshot {
            active_workspace: state.active_workspace,
            workspaces: state
                .workspaces
                .iter()
                .map(|ws| WsSnap {
                    name: ws.name.clone(),
                    active_tab: ws.active_tab,
                    tabs: ws
                        .tabs
                        .iter()
                        .map(|t| TabSnap { name: t.name.clone(), layout: node_to_snap(&t.layout) })
                        .collect(),
                })
                .collect(),
        }
    }

    /// Rebuild state with fresh pane ids. Returns the state and every pane
    /// id that needs a PTY spawned. None if the snapshot is empty/degenerate.
    pub fn restore(&self) -> Option<(AppState, Vec<PaneId>)> {
        let mut ids = IdGen::default();
        let mut workspaces = Vec::new();
        for ws in &self.workspaces {
            let mut tabs = Vec::new();
            for tab in &ws.tabs {
                let layout = snap_to_node(&tab.layout, &mut ids);
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
                continue;
            }
            let active_tab = ws.active_tab.min(tabs.len() - 1);
            workspaces.push(Workspace {
                id: ids.workspace(),
                name: ws.name.clone(),
                tabs,
                active_tab,
            });
        }
        if workspaces.is_empty() {
            return None;
        }
        let active_workspace = self.active_workspace.min(workspaces.len() - 1);
        let state = AppState {
            workspaces,
            active_workspace,
            sidebar_visible: true,
            input_mode: InputMode::Terminal,
            ids,
        };
        let panes = state
            .workspaces
            .iter()
            .flat_map(|w| w.tabs.iter())
            .flat_map(|t| t.layout.panes())
            .collect();
        state.check_invariants();
        Some((state, panes))
    }
}

pub fn path() -> Option<PathBuf> {
    crate::logging::state_dir().map(|d| d.join("session.json"))
}

pub fn save(state: &AppState) {
    let Some(p) = path() else { return };
    let snap = Snapshot::of(state);
    match serde_json::to_string_pretty(&snap) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&p, json) {
                tracing::warn!(error = %e, "failed to save session snapshot");
            }
        }
        Err(e) => tracing::warn!(error = %e, "failed to serialize session snapshot"),
    }
}

pub fn delete() {
    if let Some(p) = path() {
        let _ = std::fs::remove_file(p);
    }
}

pub fn load() -> Option<Snapshot> {
    let p = path()?;
    let text = std::fs::read_to_string(p).ok()?;
    match serde_json::from_str(&text) {
        Ok(s) => Some(s),
        Err(e) => {
            tracing::warn!(error = %e, "session snapshot unreadable; starting fresh");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::layout::Dir;

    #[test]
    fn snapshot_round_trip_preserves_structure() {
        let mut s = AppState::new();
        s.split_focused(Dir::Right, false);
        s.split_focused(Dir::Down, false);
        s.new_tab();
        s.new_workspace();
        s.rename_active_workspace("proj".into());

        let snap = Snapshot::of(&s);
        let json = serde_json::to_string(&snap).unwrap();
        let back: Snapshot = serde_json::from_str(&json).unwrap();
        let (restored, panes) = back.restore().unwrap();

        assert_eq!(restored.workspaces.len(), 2);
        assert_eq!(restored.workspaces[0].tabs.len(), 2);
        assert_eq!(restored.workspaces[0].tabs[0].layout.panes().len(), 3);
        assert_eq!(restored.workspaces[1].name, "proj");
        assert_eq!(panes.len(), 5);
        assert!(restored.check_invariants());
    }
}
