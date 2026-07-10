use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::state::AppState;

pub fn render(state: &AppState, area: Rect, frame: &mut Frame) {
    let ws = state.active_workspace();
    let mut spans = vec![
        Span::styled(format!(" {} ", ws.name), Style::new().add_modifier(Modifier::BOLD).fg(Color::Cyan)),
        Span::raw("│ "),
    ];
    for (ti, tab) in ws.tabs.iter().enumerate() {
        let active = ti == ws.active_tab;
        let zoomed = active && tab.zoomed.is_some();
        let label = format!(" {}:{}{} ", ti + 1, tab.name, if zoomed { " [Z]" } else { "" });
        let style = if active {
            Style::new().fg(Color::Black).bg(Color::Cyan)
        } else {
            Style::new().fg(Color::Gray)
        };
        spans.push(Span::styled(label, style));
        spans.push(Span::raw(" "));
    }
    let bar = Paragraph::new(Line::from(spans)).style(Style::new().bg(Color::Rgb(20, 20, 30)));
    frame.render_widget(bar, area);
}
