use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::input;
use crate::state::{InputMode, PromptKind};

/// Bottom key-hint strip, overlaid (no relayout) while a mode is active.
pub fn render_hint(mode: &InputMode, area: Rect, frame: &mut Frame) {
    let text = match mode {
        InputMode::Terminal => return,
        InputMode::Prefix => {
            " PREFIX  v/- split  hjkl focus  HJKL swap  r resize  z zoom  x close  c tab  N workspace  b sidebar  ? help  q quit "
        }
        InputMode::Resize => " RESIZE  h/l narrower/wider  j/k taller/shorter  Esc/Enter done ",
        InputMode::Help => " any key to close ",
        InputMode::Prompt { .. } => " Enter apply  Esc cancel ",
    };
    let y = area.y + area.height.saturating_sub(1);
    let strip = Rect { x: area.x, y, width: area.width, height: 1 };
    let style = Style::new().fg(Color::Black).bg(Color::Cyan);
    frame.render_widget(Paragraph::new(text).style(style), strip);
}

/// Centered modal listing every active binding, generated from the table.
pub fn render_help(area: Rect, frame: &mut Frame) {
    let mut lines: Vec<Line> = vec![Line::from(Span::styled(
        "prefix = ctrl+b",
        Style::new().add_modifier(Modifier::BOLD),
    ))];
    let mut seen = std::collections::HashSet::new();
    for (_, label, action) in input::bindings() {
        // Collapse the 1..9 family to one row.
        if !seen.insert(label) {
            continue;
        }
        lines.push(Line::from(vec![
            Span::styled(format!("  prefix+{label:<10}"), Style::new().fg(Color::Cyan)),
            Span::raw(action.describe()),
        ]));
    }
    let h = (lines.len() as u16 + 2).min(area.height);
    let w = 48.min(area.width);
    let rect = Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    };
    frame.render_widget(Clear, rect);
    let block = Block::new()
        .borders(Borders::ALL)
        .title(" keys ")
        .border_style(Style::new().fg(Color::Cyan));
    frame.render_widget(Paragraph::new(lines).block(block), rect);
}

/// One-line centered input prompt (rename tab/workspace).
pub fn render_prompt(kind: PromptKind, buffer: &str, area: Rect, frame: &mut Frame) {
    let title = match kind {
        PromptKind::RenameTab => " rename tab ",
        PromptKind::RenameWorkspace => " rename workspace ",
    };
    let w = 40.min(area.width);
    let rect = Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + area.height / 2,
        width: w,
        height: 3.min(area.height),
    };
    frame.render_widget(Clear, rect);
    let block = Block::new()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::new().fg(Color::Cyan));
    frame.render_widget(Paragraph::new(format!("{buffer}█")).block(block), rect);
}
