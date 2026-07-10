use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::state::AppState;

/// What a sidebar row activates when clicked.
#[derive(Debug, Clone, Copy)]
pub enum Target {
    Workspace(usize),
    Tab(usize, usize),
}

/// One source of truth for sidebar rows — render draws them, hit() clicks them.
fn rows(state: &AppState) -> Vec<(String, Style, Target)> {
    let mut out = Vec::new();
    for (wi, ws) in state.workspaces.iter().enumerate() {
        let ws_active = wi == state.active_workspace;
        let ws_style = if ws_active {
            Style::new().add_modifier(Modifier::BOLD).fg(Color::Cyan)
        } else {
            Style::new().add_modifier(Modifier::BOLD)
        };
        out.push((format!("▸ {}", ws.name), ws_style, Target::Workspace(wi)));
        for (ti, tab) in ws.tabs.iter().enumerate() {
            let tab_active = ws_active && ti == ws.active_tab;
            let style = if tab_active {
                Style::new().fg(Color::Cyan)
            } else {
                Style::new().fg(Color::Gray)
            };
            let marker = if tab_active { "●" } else { "○" };
            let panes = tab.layout.panes().len();
            out.push((
                format!("  {marker} {}  {panes}p", tab.name),
                style,
                Target::Tab(wi, ti),
            ));
        }
    }
    out
}

/// Workspace → tab tree. Agent-state rollups arrive in Phase 3.
pub fn render(state: &AppState, area: Rect, frame: &mut Frame) {
    let lines: Vec<Line> = rows(state)
        .into_iter()
        .map(|(text, style, _)| Line::from(Span::styled(text, style)))
        .collect();
    let block = Block::new().borders(Borders::RIGHT).border_style(Style::new().fg(Color::DarkGray));
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// Which target sits on sidebar-relative row `y`.
pub fn hit(state: &AppState, y: u16) -> Option<Target> {
    rows(state).get(y as usize).map(|(_, _, t)| *t)
}
