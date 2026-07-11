//! Context menus (pane right-click, space click) — data-driven from
//! `InputMode::Menu { items }`.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::config::theme::Theme;
use crate::state::ids::PaneId;
use crate::state::{MenuAction, MenuItem};

pub fn pane_items(pane: PaneId) -> Vec<MenuItem> {
    [
        ("new pane right", MenuAction::SplitRight(pane)),
        ("new pane left", MenuAction::SplitLeft(pane)),
        ("new pane below", MenuAction::SplitDown(pane)),
        ("new pane above", MenuAction::SplitUp(pane)),
        ("new agent here…", MenuAction::AgentPicker(Some(pane))),
        ("close pane", MenuAction::ClosePane(pane)),
    ]
    .into_iter()
    .map(|(label, action)| MenuItem { label: label.to_string(), action })
    .collect()
}

/// The sidebar "menu" button: app-level settings and session actions.
/// `update`: a newer release tag when the background check found one.
pub fn app_items(update: Option<&str>) -> Vec<MenuItem> {
    let mut items: Vec<MenuItem> = [
        ("new agent…", MenuAction::AgentPicker(None)),
        ("profiles", MenuAction::EditProfiles),
        ("settings", MenuAction::OpenSettings),
        ("keybinds", MenuAction::ShowKeybinds),
        ("reload config", MenuAction::ReloadConfig),
    ]
    .into_iter()
    .map(|(label, action)| MenuItem { label: label.to_string(), action })
    .collect();
    if let Some(tag) = update {
        items.push(MenuItem {
            label: format!("● update ready {tag}"),
            action: MenuAction::RunUpdate,
        });
    }
    items.push(MenuItem { label: "detach".to_string(), action: MenuAction::Detach });
    items
}

pub fn space_items(wi: usize) -> Vec<MenuItem> {
    [
        ("rename", MenuAction::RenameSpace(wi)),
        ("close", MenuAction::CloseSpace(wi)),
        ("new worktree", MenuAction::NewWorktree(wi)),
        ("open worktree...", MenuAction::ListWorktrees(wi)),
    ]
    .into_iter()
    .map(|(label, action)| MenuItem { label: label.to_string(), action })
    .collect()
}

/// Menu box for an anchor cell, clamped into `area`. Deterministic — the
/// mouse handler recomputes the same rect for hit testing.
pub fn rect(x: u16, y: u16, items: &[MenuItem], area: Rect) -> Rect {
    let label_w = items.iter().map(|i| i.label.width()).max().unwrap_or(10) as u16;
    let w = (label_w + 4).min(area.width);
    let h = (items.len() as u16 + 2).min(area.height);
    let x = x.min(area.x + area.width.saturating_sub(w));
    let y = (y + 1).min(area.y + area.height.saturating_sub(h));
    Rect { x, y, width: w, height: h }
}

/// Item under a screen position, if inside the menu body.
pub fn hit(menu: Rect, items: &[MenuItem], pos_x: u16, pos_y: u16) -> Option<MenuAction> {
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
        return items.get(idx).map(|i| i.action.clone());
    }
    None
}

pub fn render(x: u16, y: u16, items: &[MenuItem], theme: &Theme, area: Rect, frame: &mut Frame) {
    let r = rect(x, y, items, area);
    frame.render_widget(Clear, r);
    let lines: Vec<Line> = items.iter().map(|i| Line::from(format!(" {}", i.label))).collect();
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.accent));
    frame.render_widget(Paragraph::new(lines).style(Style::new().bg(Color::Reset)).block(block), r);
}
