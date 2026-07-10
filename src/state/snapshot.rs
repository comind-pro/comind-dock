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

/// What restore knows about a pane: the agent ident that ran there (if
/// any) and the pane's own cwd — an agent must resume in ITS folder, not
/// wherever the workspace cwd drifted to.
#[derive(Debug, Clone, Default)]
pub struct PaneMeta {
    pub agent: Option<String>,
    pub cwd: Option<PathBuf>,
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
    },
    Split { dir: DirSnap, ratio: f32, a: Box<NodeSnap>, b: Box<NodeSnap> },
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
        NodeSnap::Leaf { agent, cwd } => {
            let id = ids.pane();
            agents.push((id, PaneMeta { agent: agent.clone(), cwd: cwd.clone().map(PathBuf::from) }));
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
                continue;
            }
            let active_tab = ws.active_tab.min(tabs.len() - 1);
            let cwd = if ws.cwd.is_empty() {
                std::env::current_dir().unwrap_or_else(|_| "/".into())
            } else {
                std::path::PathBuf::from(&ws.cwd)
            };
            workspaces.push(Workspace {
                id: ids.workspace(),
                name: ws.name.clone(),
                cwd,
                custom_name: ws.custom_name,
                parent: None, // linked below by saved index
                tabs,
                active_tab,
            });
        }
        if workspaces.is_empty() {
            return None;
        }
        // Re-link worktree parents by saved index.
        for (i, snap_ws) in self.workspaces.iter().enumerate() {
            if let (Some(pi), true) = (snap_ws.parent, i < workspaces.len())
                && pi < workspaces.len() && pi != i {
                    workspaces[i].parent = Some(workspaces[pi].id);
                }
        }
        let active_workspace = self.active_workspace.min(workspaces.len() - 1);
        let state = AppState {
            workspaces,
            active_workspace,
            sidebar_visible: true,
            scope: None,
            input_mode: InputMode::Terminal,
            ids,
        };
        // Keep only panes that survived (degenerate tabs were skipped).
        panes.retain(|(id, _)| state.workspaces.iter().any(|w| {
            w.tabs.iter().any(|t| t.layout.contains(*id))
        }));
        state.check_invariants();
        Some((state, panes))
    }
}

pub fn path() -> Option<PathBuf> {
    crate::logging::state_dir().map(|d| d.join("session.json"))
}

pub fn save(state: &AppState, panes: &std::collections::HashMap<PaneId, PaneMeta>) {
    let Some(p) = path() else { return };
    let snap = Snapshot::of(state, panes);
    match serde_json::to_string_pretty(&snap) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&p, json) {
                tracing::warn!(error = %e, "failed to save session snapshot");
            }
        }
        Err(e) => tracing::warn!(error = %e, "failed to serialize session snapshot"),
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
        let mut s = AppState::new("main".into(), std::path::PathBuf::from("/tmp"));
        s.split_focused(Dir::Right, false);
        s.split_focused(Dir::Down, false);
        s.new_tab();
        s.new_workspace("x".into(), std::path::PathBuf::from("/tmp"), None);
        s.rename_active_workspace("proj".into());

        // Mark the focused pane of ws1/tab1 as a claude pane living in its
        // own folder — the workspace cwd may have drifted elsewhere.
        let claude_pane = s.workspaces[0].tabs[0].focused_pane;
        let metas = std::collections::HashMap::from([(
            claude_pane,
            PaneMeta {
                agent: Some("claude:uuid-1".to_string()),
                cwd: Some(std::path::PathBuf::from("/projects/real-home")),
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
        assert!(restored.check_invariants());
    }
}
