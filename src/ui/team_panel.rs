//! Always-open team panel: the right column of an orchestrator's tab.
//! Lists every agent pane in the dock — click toggles membership in this
//! orchestrator's team; members show ✓ and sort first.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

use crate::config::theme::Theme;
use crate::runtime::Runtime;
use crate::state::ids::PaneId;

pub const WIDTH: u16 = 28;

/// Candidate rows in render order: members first, then the rest, both by
/// pane id. Deterministic — render and hit() must agree.
pub fn rows(rt: &Runtime, orch: PaneId) -> Vec<(PaneId, bool)> {
    let mut out: Vec<(PaneId, bool)> = Vec::new();
    for ws in &rt.state.workspaces {
        for tab in &ws.tabs {
            for id in tab.layout.panes() {
                if id == orch {
                    continue;
                }
                let Some(p) = rt.panes.get(&id) else { continue };
                if p.agent.is_none() {
                    continue;
                }
                out.push((id, rt.state.teams.get(&id) == Some(&orch)));
            }
        }
    }
    out.sort_by_key(|(id, member)| (!member, id.0));
    out
}

fn pane_label(rt: &Runtime, id: PaneId) -> String {
    rt.state
        .pane_name(id)
        .map(str::to_string)
        .or_else(|| rt.titles.get(&id).cloned().filter(|t| !t.trim().is_empty()))
        .or_else(|| rt.panes.get(&id).map(|p| p.program.clone()))
        .unwrap_or_default()
}

pub fn render(rt: &Runtime, theme: &Theme, orch: PaneId, rect: Rect, frame: &mut Frame) {
    let mut lines: Vec<Line> = Vec::new();
    let inner_w = rect.width.saturating_sub(2) as usize;
    for (id, member) in rows(rt, orch) {
        let status = rt.panes.get(&id).map(|p| p.effective_status());
        let (glyph, style) = if member {
            ("✓ ", Style::new().fg(theme.accent))
        } else {
            ("+ ", Style::new().fg(theme.muted))
        };
        let suffix = format!(" %{}", id.0);
        let budget = inner_w.saturating_sub(2 + suffix.len() + 3).max(4);
        let name = crate::agents::truncate_clean(&pane_label(rt, id), budget);
        let word = status.map(|s| s.word()).unwrap_or("?");
        let name_style = if member { Style::new() } else { Style::new().fg(theme.muted) };
        lines.push(Line::from(vec![
            Span::styled(glyph, style),
            Span::styled(name, name_style),
            Span::styled(suffix, Style::new().fg(theme.muted)),
        ]));
        lines.push(Line::from(Span::styled(format!("    {word}"), Style::new().fg(theme.muted))));
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled("no agent panes yet", Style::new().fg(theme.muted))));
    }
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.muted))
        .title(Span::styled("⌂ team", Style::new().add_modifier(Modifier::BOLD)));
    frame.render_widget(Paragraph::new(lines).block(block), rect);
}

/// Row under a click: Some((pane, was_member)). Two rows per pane (name +
/// status detail) — both toggle.
pub fn hit(rt: &Runtime, orch: PaneId, rect: Rect, x: u16, y: u16) -> Option<(PaneId, bool)> {
    let inner = Rect {
        x: rect.x + 1,
        y: rect.y + 1,
        width: rect.width.saturating_sub(2),
        height: rect.height.saturating_sub(2),
    };
    if !inner.contains(ratatui::layout::Position::new(x, y)) {
        return None;
    }
    let idx = ((y - inner.y) / 2) as usize;
    rows(rt, orch).get(idx).copied()
}
