use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use crate::config::theme::Theme;
use crate::state::AppState;

struct Segment {
    text: String,
    tab: Option<usize>,
    active: bool,
    is_ws: bool,
}

/// One source of truth for tab-bar content — render draws it, hit() clicks it.
fn segments(state: &AppState) -> Vec<Segment> {
    let ws = state.active_workspace();
    let mut out = vec![
        Segment { text: format!(" {} ", ws.name), tab: None, active: false, is_ws: true },
        Segment { text: "│ ".to_string(), tab: None, active: false, is_ws: false },
    ];
    for (ti, tab) in ws.tabs.iter().enumerate() {
        let active = ti == ws.active_tab;
        let zoomed = active && tab.zoomed.is_some();
        out.push(Segment {
            text: format!(" {}:{}{} ", ti + 1, tab.name, if zoomed { " [Z]" } else { "" }),
            tab: Some(ti),
            active,
            is_ws: false,
        });
        out.push(Segment { text: " ".to_string(), tab: None, active: false, is_ws: false });
    }
    out
}

pub fn render(
    state: &AppState,
    theme: &Theme,
    focused_title: Option<&str>,
    area: Rect,
    frame: &mut Frame,
) {
    let mut spans: Vec<Span> = segments(state)
        .into_iter()
        .map(|s| {
            let style = if s.is_ws {
                Style::new().add_modifier(Modifier::BOLD).fg(theme.accent)
            } else if s.active {
                Style::new().fg(Color::Black).bg(theme.accent)
            } else if s.tab.is_some() {
                Style::new().fg(theme.muted)
            } else {
                Style::new()
            };
            Span::styled(s.text, style)
        })
        .collect();
    // Focused pane's OSC title, right-aligned.
    if let Some(title) = focused_title {
        let used: usize = spans.iter().map(|s| s.content.width()).sum();
        let title: String = title.chars().take(40).collect();
        let tw = title.width();
        let total = area.width as usize;
        if used + tw + 2 <= total {
            spans.push(Span::raw(" ".repeat(total - used - tw - 1)));
            spans.push(Span::styled(title, Style::new().fg(theme.muted)));
        }
    }
    let bar = Paragraph::new(Line::from(spans)).style(Style::new().bg(theme.tab_bar_bg));
    frame.render_widget(bar, area);
}

/// Which tab (index) sits under bar-relative column `x`.
pub fn hit(state: &AppState, x: u16) -> Option<usize> {
    let mut cursor: u16 = 0;
    for s in segments(state) {
        let w = s.text.width() as u16;
        if x >= cursor && x < cursor + w {
            return s.tab;
        }
        cursor += w;
    }
    None
}
