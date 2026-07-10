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
}

struct Row {
    line: Line<'static>,
    target: Option<Target>,
}

/// Sidebar layout (mockup): "spaces" — workspace list with status dot and a
/// subtitle line; "agents" — one row per pane with an activity status.
/// Real agent detection is Phase 3; today's status is the PTY-activity
/// heuristic (working = output in the last seconds, else idle).
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
        // Space dot: any pane working → working color.
        let working = ws
            .tabs
            .iter()
            .flat_map(|t| t.layout.panes())
            .any(|p| rt.panes.get(&p).is_some_and(|pr| pr.working()));
        let dot_style =
            if working { Style::new().fg(Color::Yellow) } else { Style::new().fg(theme.muted) };
        let name_style = if active {
            Style::new().fg(theme.accent).add_modifier(Modifier::BOLD)
        } else {
            Style::new().add_modifier(Modifier::BOLD)
        };
        out.push(Row {
            line: Line::from(vec![
                Span::raw("  "),
                Span::styled("● ", dot_style),
                Span::styled(ws.name.clone(), name_style),
            ]),
            target: Some(Target::Workspace(wi)),
        });
        let panes: usize = ws.tabs.iter().map(|t| t.layout.panes().len()).sum();
        out.push(Row {
            line: Line::from(Span::styled(
                format!("    {} tabs · {panes} panes", ws.tabs.len()),
                Style::new().fg(theme.muted),
            )),
            target: Some(Target::Workspace(wi)),
        });
    }

    out.push(Row { line: Line::from(""), target: None });
    out.push(Row {
        line: Line::from(Span::styled(
            " agents",
            Style::new().fg(theme.muted).add_modifier(Modifier::BOLD),
        )),
        target: None,
    });

    for ws in &state.workspaces {
        for tab in &ws.tabs {
            for pane in tab.layout.panes() {
                let Some(p) = rt.panes.get(&pane) else { continue };
                let working = p.working();
                let (dot, dot_style, status) = if working {
                    ("⠿ ", Style::new().fg(Color::Yellow), "working")
                } else {
                    ("● ", Style::new().fg(theme.muted), "idle")
                };
                let name = rt
                    .titles
                    .get(&pane)
                    .filter(|t| !t.trim().is_empty())
                    .map(|t| t.chars().take(16).collect::<String>())
                    .unwrap_or_else(|| p.program.clone());
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
                        format!("    {status} · {}", p.program),
                        Style::new().fg(theme.muted),
                    )),
                    target: Some(Target::Pane(pane)),
                });
            }
        }
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
