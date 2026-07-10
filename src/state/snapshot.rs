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

/// A pane to spawn on restore, with the agent that ran there (if any).
pub type PaneSpawn = (PaneId, Option<String>);

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
    /// A pane; `agent` records which agent CLI ran there so restore can
    /// relaunch it into its conversation.
    Leaf {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent: Option<String>,
    },
    Split { dir: DirSnap, ratio: f32, a: Box<NodeSnap>, b: Box<NodeSnap> },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirSnap {
    Right,
    Down,
}

fn node_to_snap(node: &Node, agents: &std::collections::HashMap<PaneId, String>) -> NodeSnap {
    match node {
        Node::Leaf(id) => NodeSnap::Leaf { agent: agents.get(id).cloned() },
        Node::Split { dir, ratio, a, b } => NodeSnap::Split {
            dir: match dir {
                Dir::Right => DirSnap::Right,
                Dir::Down => DirSnap::Down,
            },
            ratio: *ratio,
            a: Box::new(node_to_snap(a, agents)),
            b: Box::new(node_to_snap(b, agents)),
        },
    }
}

fn snap_to_node(snap: &NodeSnap, ids: &mut IdGen, agents: &mut Vec<PaneSpawn>) -> Node {
    match snap {
        NodeSnap::Leaf { agent } => {
            let id = ids.pane();
            agents.push((id, agent.clone()));
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
    /// `agents`: which agent CLI ran in which pane (for resume on restore).
    pub fn of(state: &AppState, agents: &std::collections::HashMap<PaneId, String>) -> Self {
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
                        .map(|t| TabSnap {
                            name: t.name.clone(),
                            layout: node_to_snap(&t.layout, agents),
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

pub fn save(state: &AppState, agents: &std::collections::HashMap<PaneId, String>) {
    let Some(p) = path() else { return };
    let snap = Snapshot::of(state, agents);
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
        let mut s = AppState::new("main".into());
        s.split_focused(Dir::Right, false);
        s.split_focused(Dir::Down, false);
        s.new_tab();
        s.new_workspace("x".into());
        s.rename_active_workspace("proj".into());

        // Mark the focused pane of ws1/tab1 as a claude pane.
        let claude_pane = s.workspaces[0].tabs[0].focused_pane;
        let agents =
            std::collections::HashMap::from([(claude_pane, "claude".to_string())]);
        let snap = Snapshot::of(&s, &agents);
        let json = serde_json::to_string(&snap).unwrap();
        let back: Snapshot = serde_json::from_str(&json).unwrap();
        let (restored, panes) = back.restore().unwrap();

        assert_eq!(restored.workspaces.len(), 2);
        assert_eq!(restored.workspaces[0].tabs.len(), 2);
        assert_eq!(restored.workspaces[0].tabs[0].layout.panes().len(), 3);
        assert_eq!(restored.workspaces[1].name, "proj");
        assert_eq!(panes.len(), 5);
        assert_eq!(
            panes.iter().filter(|(_, a)| a.as_deref() == Some("claude")).count(),
            1,
            "exactly one pane remembers its agent"
        );
        assert!(restored.check_invariants());
    }
}
