//! Always-open team panel: the right column of an orchestrator's tab.
//! Shows the TEAM — the panes assigned to this orchestrator — plus a
//! "+ add" row that opens a picker of the other active agent panes.
//! A member's name jumps to its pane, its ✓ mark removes it; the wheel
//! scrolls long teams.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

use crate::config::theme::Theme;
use crate::runtime::Runtime;
use crate::state::ids::PaneId;

pub const WIDTH: u16 = 28;

/// What a click inside the panel means.
pub enum Hit {
    /// The "+ add" row: open the candidate picker.
    Add,
    /// A member's name: jump to that pane.
    Focus(PaneId),
    /// A member's ✓ mark: remove it from the team.
    Remove(PaneId),
}

/// This orchestrator's members, by pane id — render and hit() must agree.
pub fn members(rt: &Runtime, orch: PaneId) -> Vec<PaneId> {
    let mut out: Vec<PaneId> =
        rt.state.teams.iter().filter(|(_, o)| **o == orch).map(|(w, _)| *w).collect();
    out.sort_by_key(|p| p.0);
    out
}

/// Active agent panes NOT yet on this team — what "+ add" offers.
pub fn candidates(rt: &Runtime, orch: PaneId) -> Vec<PaneId> {
    let mut out: Vec<PaneId> = Vec::new();
    for ws in &rt.state.workspaces {
        for tab in &ws.tabs {
            for id in tab.layout.panes() {
                if id == orch || rt.state.teams.get(&id) == Some(&orch) {
                    continue;
                }
                let Some(p) = rt.panes.get(&id) else { continue };
                if p.agent.is_none() {
                    continue;
                }
                out.push(id);
            }
        }
    }
    out.sort_by_key(|p| p.0);
    out
}

pub fn pane_label(rt: &Runtime, id: PaneId) -> String {
    rt.state
        .pane_name(id)
        .map(str::to_string)
        .or_else(|| rt.titles.get(&id).cloned().filter(|t| !t.trim().is_empty()))
        .or_else(|| rt.panes.get(&id).map(|p| p.program.clone()))
        .unwrap_or_default()
}

/// Picker label for a candidate: name, id, which agent CLI it runs (with
/// its claude profile, as in the sidebar) and the space it lives in —
/// twins are indistinguishable without those.
pub fn candidate_label(rt: &Runtime, id: PaneId) -> String {
    let name = crate::agents::truncate_clean(&pane_label(rt, id), 28);
    let agent = rt.panes.get(&id).and_then(|p| p.agent).unwrap_or("?");
    let profile = rt
        .panes
        .get(&id)
        .and_then(|p| p.agent_config_dir.as_deref())
        .and_then(crate::agents::profile_label_from_dir)
        .map(|l| format!(" @{l}"))
        .unwrap_or_default();
    let ws = rt
        .state
        .locate_pane(id)
        .and_then(|(wi, _)| rt.state.workspaces.get(wi))
        .map(|w| crate::agents::truncate_clean(&w.name, 18))
        .unwrap_or_default();
    format!("{name} %{} · {agent}{profile} · {ws}", id.0)
}

/// Total text lines the panel body holds (the "+ add" row + 2 per member).
fn line_count(rt: &Runtime, orch: PaneId) -> u16 {
    1 + members(rt, orch).len() as u16 * 2
}

/// Highest useful scroll offset for the wheel handler.
pub fn max_scroll(rt: &Runtime, orch: PaneId, rect: Rect) -> u16 {
    line_count(rt, orch).saturating_sub(rect.height.saturating_sub(2))
}

pub fn render(rt: &Runtime, theme: &Theme, orch: PaneId, rect: Rect, frame: &mut Frame) {
    let mut lines: Vec<Line> =
        vec![Line::from(Span::styled("+ add", Style::new().fg(theme.accent)))];
    let inner_w = rect.width.saturating_sub(2) as usize;
    for id in members(rt, orch) {
        let status = rt.panes.get(&id).map(|p| p.effective_status());
        let suffix = format!(" %{}", id.0);
        let budget = inner_w.saturating_sub(2 + suffix.len()).max(4);
        let name = crate::agents::truncate_clean(&pane_label(rt, id), budget);
        let word = status.map(|s| s.word()).unwrap_or("?");
        lines.push(Line::from(vec![
            Span::styled("✓ ", Style::new().fg(theme.accent)),
            Span::raw(name),
            Span::styled(suffix, Style::new().fg(theme.muted)),
        ]));
        lines.push(Line::from(Span::styled(format!("    {word}"), Style::new().fg(theme.muted))));
    }
    let scroll = rt.team_scroll.min(max_scroll(rt, orch, rect));
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.muted))
        .title(Span::styled("⌂ team", Style::new().add_modifier(Modifier::BOLD)));
    frame.render_widget(Paragraph::new(lines).scroll((scroll, 0)).block(block), rect);
}

/// What sits under a click, honoring the scroll offset. A member's two
/// rows (name + status) both belong to that member: the ✓ mark removes,
/// anywhere else on the rows focuses the pane.
pub fn hit(rt: &Runtime, orch: PaneId, rect: Rect, x: u16, y: u16) -> Option<Hit> {
    let inner = Rect {
        x: rect.x + 1,
        y: rect.y + 1,
        width: rect.width.saturating_sub(2),
        height: rect.height.saturating_sub(2),
    };
    if !inner.contains(ratatui::layout::Position::new(x, y)) {
        return None;
    }
    let scroll = rt.team_scroll.min(max_scroll(rt, orch, rect));
    let line = y - inner.y + scroll;
    if line == 0 {
        return Some(Hit::Add);
    }
    let on_mark = line % 2 == 1 && x < inner.x + 2;
    let idx = ((line - 1) / 2) as usize;
    members(rt, orch).get(idx).map(|p| if on_mark { Hit::Remove(*p) } else { Hit::Focus(*p) })
}
