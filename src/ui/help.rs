use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::config::keys::{Bound, Keymap};
use crate::config::theme::Theme;
use crate::state::{InputMode, PromptKind};

/// Bottom key-hint strip, overlaid (no relayout) while a mode is active.
pub fn render_hint(mode: &InputMode, theme: &Theme, area: Rect, frame: &mut Frame) {
    let text = match mode {
        InputMode::Terminal => return,
        InputMode::Prefix => {
            " PREFIX  v/- split  hjkl focus  HJKL swap  r resize  z zoom  x close  c tab  N workspace  b sidebar  ? help  q quit "
        }
        InputMode::Resize => " RESIZE  h/l narrower/wider  j/k taller/shorter  Esc/Enter done ",
        InputMode::Help => " any key to close ",
        InputMode::Prompt { .. } => " Enter apply  Esc cancel ",
        InputMode::Search { .. } => " SEARCH (regex)  Enter find  Esc cancel ",
        InputMode::SearchNav => " MATCH  n older  N newer  / new search  Esc done ",
        InputMode::ConfirmClose(_) => " close pane? y = yes, any other key = cancel ",
        InputMode::Menu { .. } => " click an option · click elsewhere / any key to dismiss ",
    };
    let y = area.y + area.height.saturating_sub(1);
    let strip = Rect { x: area.x, y, width: area.width, height: 1 };
    let style = Style::new().fg(Color::Black).bg(theme.accent);
    frame.render_widget(Paragraph::new(text).style(style), strip);
}

/// Centered modal listing every active binding, generated from the keymap
/// (so user overrides and custom commands show automatically).
pub fn render_help(keymap: &Keymap, theme: &Theme, area: Rect, frame: &mut Frame) {
    let mut lines: Vec<Line> = vec![Line::from(Span::styled(
        "prefix, then:  (1..9 = jump to tab)",
        Style::new().add_modifier(Modifier::BOLD),
    ))];
    for entry in &keymap.entries {
        let desc = match &entry.bound {
            Bound::Builtin(action) => action.describe().to_string(),
            Bound::Command(cmd) => {
                if cmd.description.is_empty() { cmd.command.clone() } else { cmd.description.clone() }
            }
        };
        let key = if entry.direct {
            format!("  {:<12}", entry.label)
        } else {
            format!("  prefix+{:<10}", entry.label)
        };
        lines.push(Line::from(vec![
            Span::styled(key, Style::new().fg(theme.accent)),
            Span::raw(" "),
            Span::raw(desc),
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
        .border_style(Style::new().fg(theme.accent));
    frame.render_widget(Paragraph::new(lines).block(block), rect);
}

/// One-line centered input prompt (rename tab/workspace).
pub fn render_prompt(kind: PromptKind, buffer: &str, theme: &Theme, area: Rect, frame: &mut Frame) {
    let title = match kind {
        PromptKind::RenameTab => " rename tab ",
        PromptKind::RenameWorkspace => " rename space ",
        PromptKind::WorktreeBranch(_) => " new worktree: branch name ",
    };
    render_input_box(title, buffer, theme, area, frame);
}

/// Scrollback-search query box.
pub fn render_search(buffer: &str, theme: &Theme, area: Rect, frame: &mut Frame) {
    render_input_box(" search scrollback (regex) ", buffer, theme, area, frame);
}

fn render_input_box(title: &str, buffer: &str, theme: &Theme, area: Rect, frame: &mut Frame) {
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
        .border_style(Style::new().fg(theme.accent));
    frame.render_widget(Paragraph::new(format!("{buffer}█")).block(block), rect);
}
