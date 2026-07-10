//! Right-click context menu for a pane.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

use crate::config::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    SplitRight,
    SplitLeft,
    SplitDown,
    SplitUp,
    ClosePane,
}

pub const ITEMS: &[(&str, MenuAction)] = &[
    ("new pane right", MenuAction::SplitRight),
    ("new pane left", MenuAction::SplitLeft),
    ("new pane below", MenuAction::SplitDown),
    ("new pane above", MenuAction::SplitUp),
    ("close pane", MenuAction::ClosePane),
];

const WIDTH: u16 = 20;

/// Menu box for an anchor cell, clamped into `area`. Deterministic — the
/// mouse handler recomputes the same rect for hit testing.
pub fn rect(x: u16, y: u16, area: Rect) -> Rect {
    let h = ITEMS.len() as u16 + 2;
    let w = WIDTH.min(area.width);
    let x = x.min(area.x + area.width.saturating_sub(w));
    let y = (y + 1).min(area.y + area.height.saturating_sub(h));
    Rect { x, y, width: w, height: h.min(area.height) }
}

/// Item under a screen position, if inside the menu body.
pub fn hit(menu: Rect, pos_x: u16, pos_y: u16) -> Option<MenuAction> {
    let inner = Rect {
        x: menu.x + 1,
        y: menu.y + 1,
        width: menu.width.saturating_sub(2),
        height: menu.height.saturating_sub(2),
    };
    if pos_x >= inner.x
        && pos_x < inner.x + inner.width
        && pos_y >= inner.y
        && pos_y < inner.y + inner.height
    {
        let idx = (pos_y - inner.y) as usize;
        return ITEMS.get(idx).map(|(_, a)| *a);
    }
    None
}

pub fn render(x: u16, y: u16, theme: &Theme, area: Rect, frame: &mut Frame) {
    let r = rect(x, y, area);
    frame.render_widget(Clear, r);
    let lines: Vec<Line> = ITEMS.iter().map(|(label, _)| Line::from(format!(" {label}"))).collect();
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.accent));
    frame.render_widget(
        Paragraph::new(lines).style(Style::new().bg(Color::Reset)).block(block),
        r,
    );
}
