//! In-app notification toasts: one-line overlays in the top-right corner.
//! Click focuses the pane (jump-to-agent); they expire on their own.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::runtime::{NoticeKind, Runtime};
use crate::state::ids::PaneId;

/// Deterministic toast rects — render and mouse hit testing must agree.
pub fn rects(rt: &Runtime, area: Rect) -> Vec<(Rect, Option<PaneId>)> {
    rt.toasts
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let w = (t.text.width() as u16 + 2).min(area.width);
            let x = area.width.saturating_sub(w + 1);
            let y = (1 + i as u16).min(area.height.saturating_sub(1));
            (Rect::new(x, y, w, 1), t.pane)
        })
        .collect()
}

pub fn render(rt: &Runtime, area: Rect, frame: &mut Frame) {
    for ((rect, _), toast) in rects(rt, area).into_iter().zip(&rt.toasts) {
        let style = match toast.kind {
            NoticeKind::Blocked => Style::new().fg(Color::White).bg(Color::Red),
            NoticeKind::Done => Style::new().fg(Color::Black).bg(Color::Green),
        };
        frame.render_widget(Clear, rect);
        frame.render_widget(
            Paragraph::new(Span::styled(
                format!(" {} ", toast.text),
                style.add_modifier(Modifier::BOLD),
            )),
            rect,
        );
    }
}
