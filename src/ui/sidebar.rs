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
    /// "+ continue" at the bottom: resume any Claude session on the system.
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

/// Two rows for an agent pane: `marker + name` (name gets the full width so it
/// isn't cramped), then the muted `status · agent @profile` detail indented
/// under it. Non-agent panes (plain shells) emit nothing.
fn agent_rows(
    rt: &Runtime,
    theme: &Theme,
    pane: PaneId,
    indent: &str,
    width: u16,
    out: &mut Vec<Row>,
) {
    let Some(p) = rt.panes.get(&pane) else { return };
    let Some(agent) = p.agent else { return };
    let state = &rt.state;
    let title = rt.titles.get(&pane).map(String::as_str).unwrap_or("");
    let status = p.effective_status();
    // An unseen event outranks the (already decayed) status: the sound said
    // SOMETHING finished — the sidebar says which one. A distinct glyph, not
    // just a shade: "✓" also means a Done the user has already read.
    let (dot, dot_style) = match p.unseen {
        Some(crate::runtime::NoticeKind::Done) => {
            ("★ ", Style::new().fg(Color::LightGreen).add_modifier(Modifier::BOLD))
        }
        Some(crate::runtime::NoticeKind::Blocked) => {
            ("★ ", Style::new().fg(Color::LightRed).add_modifier(Modifier::BOLD))
        }
        None => status_marker(status, theme),
    };
    // Reporter label ("running tests") beats the generic word; an unseen event
    // says what it was, not the decayed "idle".
    let status = match (p.unseen, p.reported_label()) {
        (_, Some(label)) => label,
        (Some(crate::runtime::NoticeKind::Done), _) => "finished",
        (Some(crate::runtime::NoticeKind::Blocked), _) => "blocked",
        (None, None) => status.word(),
    };
    // Which profile the agent runs as, when not the default.
    let profile = p
        .agent_config_dir
        .as_deref()
        .and_then(crate::agents::profile_label_from_dir)
        .map(|l| format!(" @{l}"))
        .unwrap_or_default();
    // Team membership: a worker names its orchestrator, an orchestrator
    // shows how many workers report to it.
    let team = match state.teams.get(&pane) {
        Some(orch) => format!(" · team %{}", orch.0),
        None => match state.teams.values().filter(|o| **o == pane).count() {
            0 => String::new(),
            n => format!(" · ⌂{n}"),
        },
    };
    // User-given name wins; then the agent's OSC title; then the bare agent name.
    // The name gets nearly the full sidebar width — its own row, not shared.
    let orch_mark = if state.orchestrators.contains(&pane) { "⌂ " } else { "" };
    let name_budget = (width as usize)
        .saturating_sub(indent.width() + dot.width() + orch_mark.width() + 1)
        .max(6);
    let name = match state.pane_name(pane) {
        Some(n) => crate::agents::truncate_clean(n, name_budget),
        None if title.trim().is_empty() => agent.to_string(),
        None => crate::agents::truncate_clean(title, name_budget),
    };
    let focused = pane == state.focused_pane();
    let name_style = if focused {
        Style::new().fg(theme.accent).add_modifier(Modifier::BOLD)
    } else {
        Style::new().add_modifier(Modifier::BOLD)
    };
    // Orchestrator panes carry a ⌂ so the coordinator chat stands out in
    // the agent list.
    out.push(Row {
        line: Line::from(vec![
            Span::raw(indent.to_string()),
            Span::styled(dot, dot_style),
            Span::styled(orch_mark, Style::new().fg(theme.accent)),
            Span::styled(name, name_style),
        ]),
        target: Some(Target::Pane(pane)),
    });
    out.push(Row {
        line: Line::from(Span::styled(
            format!("{indent}  {status} · {agent}{profile}{team}"),
            Style::new().fg(theme.muted),
        )),
        target: Some(Target::Pane(pane)),
    });
}

/// Sidebar (mockup): one "spaces" section — each workspace shows a status dot,
/// a git-branch/counts subtitle, and its agent panes nested one indent deeper
/// (marker + name, then a `status · agent @profile` line). Worktree children
/// indent under their parent; their agents indent deeper still.
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
    ];

    // Orchestrators pinned on top: their own block, a divider separates it
    // from the spaces below. They are skipped in the per-space listing.
    let mut orchs: Vec<PaneId> = state
        .orchestrators
        .iter()
        .copied()
        .filter(|p| state.locate_pane(*p).is_some_and(|(wi, _)| state.in_scope(wi)))
        .collect();
    orchs.sort_by_key(|p| p.0);
    if !orchs.is_empty() {
        out.push(Row {
            line: Line::from(Span::styled(
                " orchestrators",
                Style::new().fg(theme.muted).add_modifier(Modifier::BOLD),
            )),
            target: None,
        });
        for pane in &orchs {
            agent_rows(rt, theme, *pane, "   ", width, &mut out);
        }
        out.push(Row { line: Line::from(""), target: None });
        out.push(Row {
            line: Line::from(Span::styled(
                "─".repeat(width as usize),
                Style::new().fg(theme.muted),
            )),
            target: None,
        });
    }

    out.push(Row {
        line: Line::from(Span::styled(
            " spaces",
            Style::new().fg(theme.muted).add_modifier(Modifier::BOLD),
        )),
        target: None,
    });

    for (wi, ws) in state.workspaces.iter().enumerate() {
        if !state.in_scope(wi) {
            continue;
        }
        // A space that holds ONLY orchestrators (their home space) has
        // nothing to say here — its chats live in the pinned block above.
        let ws_panes: Vec<PaneId> = ws.tabs.iter().flat_map(|t| t.layout.panes()).collect();
        if !ws_panes.is_empty() && ws_panes.iter().all(|p| state.orchestrators.contains(p)) {
            continue;
        }
        // A blank line before each space separates the groups so a long list
        // doesn't read as one wall of text.
        out.push(Row { line: Line::from(""), target: None });
        let active = wi == state.active_workspace;
        let child = ws.parent.is_some();
        let indent = if child { "  " } else { "" };
        let name_style = if active {
            Style::new().fg(theme.accent).add_modifier(Modifier::BOLD)
        } else {
            Style::new().add_modifier(Modifier::BOLD)
        };
        // Space header: bold name (no status dot — the group heading), with the
        // git branch / tab·pane counts right-aligned on the same line.
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
        let name = crate::agents::truncate_clean(&ws.name, 20);
        let pad =
            (width as usize).saturating_sub(indent.width() + name.width() + subtitle.width() + 1);
        let mut spans = vec![Span::raw(indent.to_string()), Span::styled(name, name_style)];
        if !subtitle.is_empty() {
            spans.push(Span::raw(" ".repeat(pad.max(1))));
            spans.push(Span::styled(subtitle, Style::new().fg(theme.muted)));
        }
        out.push(Row { line: Line::from(spans), target: Some(Target::Workspace(wi)) });
        // This space's agents, nested one indent deeper. Non-agent panes emit
        // nothing, so an empty space shows just its header line.
        let agent_indent = format!("{indent}   ");
        for tab in &ws.tabs {
            for pane in tab.layout.panes() {
                if state.orchestrators.contains(&pane) {
                    continue; // pinned in the top block
                }
                agent_rows(rt, theme, pane, &agent_indent, width, &mut out);
            }
        }
    }
    out.push(Row { line: Line::from(""), target: None });
    out.push(Row {
        line: Line::from(Span::styled("  + new space", Style::new().fg(theme.accent))),
        target: Some(Target::NewWorkspace),
    });
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
    let lines: Vec<Line> = rows(rt, theme, area.width).into_iter().map(|r| r.line).collect();
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
