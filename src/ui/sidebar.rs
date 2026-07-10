use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::state::AppState;

/// Workspace → tab tree. Agent-state rollups arrive in Phase 3.
pub fn render(state: &AppState, area: Rect, frame: &mut Frame) {
    let mut lines: Vec<Line> = Vec::new();
    for (wi, ws) in state.workspaces.iter().enumerate() {
        let ws_active = wi == state.active_workspace;
        let ws_style = if ws_active {
            Style::new().add_modifier(Modifier::BOLD).fg(Color::Cyan)
        } else {
            Style::new().add_modifier(Modifier::BOLD)
        };
        lines.push(Line::from(Span::styled(format!("▸ {}", ws.name), ws_style)));
        for (ti, tab) in ws.tabs.iter().enumerate() {
            let tab_active = ws_active && ti == ws.active_tab;
            let style = if tab_active { Style::new().fg(Color::Cyan) } else { Style::new().fg(Color::Gray) };
            let panes = tab.layout.panes().len();
            let marker = if tab_active { "●" } else { "○" };
            lines.push(Line::from(Span::styled(
                format!("  {marker} {}  {panes}p", tab.name),
                style,
            )));
        }
    }
    let block = Block::new().borders(Borders::RIGHT).border_style(Style::new().fg(Color::DarkGray));
    frame.render_widget(Paragraph::new(lines).block(block), area);
}
