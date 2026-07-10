use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::config::theme::Theme;
use crate::runtime::Runtime;
use crate::state::ids::PaneId;

/// What a sidebar row activates when clicked.
#[derive(Debug, Clone, Copy)]
pub enum Target {
    Workspace(usize),
    Pane(PaneId),
    NewWorkspace,
}

struct Row {
    line: Line<'static>,
    target: Option<Target>,
}

/// Space status dot: empty/dim — no agents; green empty — agents, all
/// idle; green filled — an agent is working; red — an agent is blocked
/// (blocked arrives with the Phase 3 detection engine).
fn space_dot(rt: &Runtime, wi: usize, theme: &Theme) -> (&'static str, Style) {
    let ws = &rt.state.workspaces[wi];
    let mut has_agent = false;
    let mut working = false;
    for pane in ws.tabs.iter().flat_map(|t| t.layout.panes()) {
        let Some(p) = rt.panes.get(&pane) else { continue };
        let title = rt.titles.get(&pane).map(String::as_str).unwrap_or("");
        if crate::agents::detect(title, &p.program).is_some() {
            has_agent = true;
            working |= p.working();
        }
    }
    match (has_agent, working) {
        (false, _) => ("○ ", Style::new().fg(theme.muted)),
        (true, false) => ("○ ", Style::new().fg(Color::Green)),
        (true, true) => ("● ", Style::new().fg(Color::Green)),
    }
}

/// Sidebar (mockup): "spaces" — workspaces with status dot, git branch
/// subtitle, worktree children indented under their parent; "agents" — one
/// row per recognized agent pane.
fn rows(rt: &Runtime, theme: &Theme) -> Vec<Row> {
    let state = &rt.state;
    let mut out = vec![
        Row { line: Line::from(""), target: None },
        Row {
            line: Line::from(Span::styled(
                " spaces",
                Style::new().fg(theme.muted).add_modifier(Modifier::BOLD),
            )),
            target: None,
        },
    ];

    for (wi, ws) in state.workspaces.iter().enumerate() {
        let active = wi == state.active_workspace;
        let child = ws.parent.is_some();
        let indent = if child { "    " } else { "  " };
        let (dot, dot_style) = space_dot(rt, wi, theme);
        let name_style = if active {
            Style::new().fg(theme.accent).add_modifier(Modifier::BOLD)
        } else {
            Style::new().add_modifier(Modifier::BOLD)
        };
        out.push(Row {
            line: Line::from(vec![
                Span::raw(indent),
                Span::styled(dot, dot_style),
                Span::styled(ws.name.clone(), name_style),
            ]),
            target: Some(Target::Workspace(wi)),
        });
        // Subtitle: git branch when in a repo, otherwise tab/pane counts.
        let subtitle = match rt.branches.get(&ws.id) {
            Some(b) => b.clone(),
            None => {
                let panes: usize = ws.tabs.iter().map(|t| t.layout.panes().len()).sum();
                format!("{} tabs · {panes} panes", ws.tabs.len())
            }
        };
        out.push(Row {
            line: Line::from(Span::styled(
                format!("{indent}  {subtitle}"),
                Style::new().fg(theme.muted),
            )),
            target: Some(Target::Workspace(wi)),
        });
    }
    out.push(Row {
        line: Line::from(Span::styled("  + new space", Style::new().fg(theme.accent))),
        target: Some(Target::NewWorkspace),
    });

    out.push(Row { line: Line::from(""), target: None });
    out.push(Row {
        line: Line::from(Span::styled(
            " agents",
            Style::new().fg(theme.muted).add_modifier(Modifier::BOLD),
        )),
        target: None,
    });

    let mut any_agent = false;
    for ws in &state.workspaces {
        for tab in &ws.tabs {
            for pane in tab.layout.panes() {
                let Some(p) = rt.panes.get(&pane) else { continue };
                let title = rt.titles.get(&pane).map(String::as_str).unwrap_or("");
                // Only recognized agent CLIs live here; plain shells are not agents.
                let Some(agent) = crate::agents::detect(title, &p.program) else { continue };
                any_agent = true;
                let working = p.working();
                let (dot, dot_style, status) = if working {
                    ("⠿ ", Style::new().fg(Color::Yellow), "working")
                } else {
                    ("● ", Style::new().fg(theme.muted), "idle")
                };
                let name = if title.trim().is_empty() {
                    agent.to_string()
                } else {
                    title.chars().take(16).collect::<String>()
                };
                let focused = pane == state.focused_pane();
                let name_style = if focused {
                    Style::new().fg(theme.accent).add_modifier(Modifier::BOLD)
                } else {
                    Style::new().add_modifier(Modifier::BOLD)
                };
                out.push(Row {
                    line: Line::from(vec![
                        Span::raw("  "),
                        Span::styled(dot, dot_style),
                        Span::styled(name, name_style),
                    ]),
                    target: Some(Target::Pane(pane)),
                });
                out.push(Row {
                    line: Line::from(Span::styled(
                        format!("    {status} · {agent}"),
                        Style::new().fg(theme.muted),
                    )),
                    target: Some(Target::Pane(pane)),
                });
            }
        }
    }
    if !any_agent {
        out.push(Row {
            line: Line::from(Span::styled("  none yet", Style::new().fg(theme.muted))),
            target: None,
        });
    }
    out
}

pub fn render(rt: &Runtime, theme: &Theme, area: Rect, frame: &mut Frame) {
    let lines: Vec<Line> = rows(rt, theme).into_iter().map(|r| r.line).collect();
    frame.render_widget(Paragraph::new(lines), area);
}

/// Which target sits on sidebar-relative row `y`.
pub fn hit(rt: &Runtime, theme: &Theme, y: u16) -> Option<Target> {
    rows(rt, theme).get(y as usize).and_then(|r| r.target)
}
