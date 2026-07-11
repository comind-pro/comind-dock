use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use crate::config::theme::Theme;
use crate::runtime::Runtime;
use crate::state::ids::PaneId;

/// What a sidebar row activates when clicked.
#[derive(Debug, Clone, Copy)]
pub enum Target {
    Workspace(usize),
    Pane(PaneId),
    NewWorkspace,
    /// The "≡ menu" row above spaces: app settings / session actions.
    AppMenu,
    /// "+ continue" under agents: resume any Claude session on the system.
    ContinueAgent,
    /// The « at the menu row's right edge: hide the sidebar.
    CollapseSidebar,
}

/// Clickable width of the « collapse zone at the menu row's right edge.
const COLLAPSE_ZONE: u16 = 3;

struct Row {
    line: Line<'static>,
    target: Option<Target>,
}

/// Space status dot: dim empty — no agents; green empty — agents, all
/// calm; green filled — an agent is working; red filled — an agent is
/// blocked and needs the user.
fn space_dot(rt: &Runtime, wi: usize, theme: &Theme) -> (&'static str, Style) {
    use crate::detect::Status;
    let ws = &rt.state.workspaces[wi];
    let mut has_agent = false;
    let mut working = false;
    let mut blocked = false;
    for pane in ws.tabs.iter().flat_map(|t| t.layout.panes()) {
        let Some(p) = rt.panes.get(&pane) else { continue };
        if p.agent.is_some() {
            has_agent = true;
            match p.effective_status() {
                Status::Blocked => blocked = true,
                Status::Working => working = true,
                _ => {}
            }
        }
    }
    if blocked {
        ("● ", Style::new().fg(Color::Red))
    } else if working {
        ("● ", Style::new().fg(Color::Green))
    } else if has_agent {
        ("○ ", Style::new().fg(Color::Green))
    } else {
        ("○ ", Style::new().fg(theme.muted))
    }
}

/// Agent-row marker and colors per detection state.
fn status_marker(status: crate::detect::Status, theme: &Theme) -> (&'static str, Style) {
    use crate::detect::Status;
    match status {
        Status::Working => ("⠿ ", Style::new().fg(Color::Yellow)),
        Status::Blocked => ("● ", Style::new().fg(Color::Red)),
        Status::Done => ("✓ ", Style::new().fg(Color::Green)),
        Status::Idle => ("○ ", Style::new().fg(Color::Green)),
        Status::Unknown => ("● ", Style::new().fg(theme.muted)),
    }
}

/// Sidebar (mockup): "spaces" — workspaces with status dot, git branch
/// subtitle, worktree children indented under their parent; "agents" — one
/// row per recognized agent pane.
fn rows(rt: &Runtime, theme: &Theme, width: u16) -> Vec<Row> {
    let state = &rt.state;
    // "« " pinned to the right edge; hit() maps clicks there to CollapseSidebar.
    let menu = " ≡ menu";
    let pad = (width as usize).saturating_sub(menu.width() + 2);
    let mut out = vec![
        Row {
            line: Line::from(vec![
                Span::styled(menu, Style::new().fg(theme.muted)),
                Span::raw(" ".repeat(pad)),
                Span::styled("« ", Style::new().fg(theme.muted)),
            ]),
            target: Some(Target::AppMenu),
        },
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
        if !state.in_scope(wi) {
            continue;
        }
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
        // Subtitle: git branch and tab/pane counts side by side — counts
        // only when non-trivial, so single-pane spaces stay quiet.
        let mut parts: Vec<String> = Vec::new();
        if let Some(b) = rt.branches.get(&ws.id) {
            parts.push(b.clone());
        }
        let tabs = ws.tabs.len();
        let panes: usize = ws.tabs.iter().map(|t| t.layout.panes().len()).sum();
        if tabs > 1 || panes > 1 {
            parts.push(format!("{tabs}·{panes}"));
        }
        let subtitle = parts.join(" · ");
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
    for (wi, ws) in state.workspaces.iter().enumerate() {
        if !state.in_scope(wi) {
            continue;
        }
        for tab in &ws.tabs {
            for pane in tab.layout.panes() {
                let Some(p) = rt.panes.get(&pane) else { continue };
                let title = rt.titles.get(&pane).map(String::as_str).unwrap_or("");
                // Only recognized agent CLIs live here; plain shells are not agents.
                let Some(agent) = p.agent else { continue };
                any_agent = true;
                let status = p.effective_status();
                let (dot, dot_style) = status_marker(status, theme);
                let status = status.word();
                let name = if title.trim().is_empty() {
                    agent.to_string()
                } else {
                    crate::agents::truncate_clean(title, 16)
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
                // Which profile the agent runs as, when not the default.
                let profile = p
                    .agent_config_dir
                    .as_deref()
                    .and_then(crate::agents::profile_label_from_dir)
                    .map(|l| format!(" @{l}"))
                    .unwrap_or_default();
                out.push(Row {
                    line: Line::from(Span::styled(
                        format!("    {status} · {agent}{profile}"),
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
    out.push(Row {
        line: Line::from(Span::styled("  + continue", Style::new().fg(theme.accent))),
        target: Some(Target::ContinueAgent),
    });
    out
}

/// Scroll offset clamped so the last row stays reachable.
fn clamped_scroll(rt: &Runtime, row_count: usize, height: u16) -> u16 {
    rt.sidebar_scroll.min((row_count as u16).saturating_sub(height))
}

pub fn max_scroll(rt: &Runtime, theme: &Theme, size: (u16, u16)) -> u16 {
    (rows(rt, theme, size.0).len() as u16).saturating_sub(size.1)
}

pub fn render(rt: &Runtime, theme: &Theme, area: Rect, frame: &mut Frame) {
    let lines: Vec<Line> =
        rows(rt, theme, area.width).into_iter().map(|r| r.line).collect();
    let scroll = clamped_scroll(rt, lines.len(), area.height);
    frame.render_widget(Paragraph::new(lines).scroll((scroll, 0)), area);
}

/// Which target sits at sidebar-relative (`x`, `y`) (viewport coordinates).
pub fn hit(rt: &Runtime, theme: &Theme, x: u16, y: u16, size: (u16, u16)) -> Option<Target> {
    let (width, height) = size;
    let rows = rows(rt, theme, width);
    let scroll = clamped_scroll(rt, rows.len(), height);
    match rows.get((y + scroll) as usize).and_then(|r| r.target) {
        Some(Target::AppMenu) if x >= width.saturating_sub(COLLAPSE_ZONE) => {
            Some(Target::CollapseSidebar)
        }
        t => t,
    }
}
