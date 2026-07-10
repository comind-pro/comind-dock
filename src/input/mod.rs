pub mod encode;
pub mod mouse;

use std::io;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;

use crate::runtime::Runtime;
use crate::state::layout::{Dir, Side};
use crate::state::{InputMode, PromptKind};

/// Everything a key chord can do. The default table lives in `bindings()`;
/// M6 layers user config on top.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    SplitRight,
    SplitDown,
    Focus(Side),
    Swap(Side),
    ResizeMode,
    Zoom,
    ClosePane,
    NewTab,
    NextTab,
    PrevTab,
    RenameTab,
    CloseTab,
    NewWorkspace,
    RenameWorkspace,
    CloseWorkspace,
    CycleWorkspace,
    ToggleSidebar,
    Help,
    Quit,
}

impl Action {
    pub fn describe(self) -> &'static str {
        match self {
            Action::SplitRight => "split pane right",
            Action::SplitDown => "split pane down",
            Action::Focus(Side::Left) => "focus left",
            Action::Focus(Side::Down) => "focus down",
            Action::Focus(Side::Up) => "focus up",
            Action::Focus(Side::Right) => "focus right",
            Action::Swap(Side::Left) => "swap pane left",
            Action::Swap(Side::Down) => "swap pane down",
            Action::Swap(Side::Up) => "swap pane up",
            Action::Swap(Side::Right) => "swap pane right",
            Action::ResizeMode => "resize mode",
            Action::Zoom => "toggle zoom",
            Action::ClosePane => "close pane",
            Action::NewTab => "new tab",
            Action::NextTab => "next tab",
            Action::PrevTab => "previous tab",
            Action::RenameTab => "rename tab",
            Action::CloseTab => "close tab",
            Action::NewWorkspace => "new workspace",
            Action::RenameWorkspace => "rename workspace",
            Action::CloseWorkspace => "close workspace",
            Action::CycleWorkspace => "next workspace",
            Action::ToggleSidebar => "toggle sidebar",
            Action::Help => "help",
            Action::Quit => "quit",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chord {
    pub code: KeyCode,
    pub mods: KeyModifiers,
}

/// Default binding table (FEATURES.md defaults): config action name,
/// default keys (bare = after prefix), action. Order = help-overlay order.
pub fn default_actions() -> Vec<(&'static str, &'static [&'static str], Action)> {
    vec![
        ("split_right", &["v"], Action::SplitRight),
        ("split_down", &["-"], Action::SplitDown),
        ("focus_left", &["h"], Action::Focus(Side::Left)),
        ("focus_down", &["j"], Action::Focus(Side::Down)),
        ("focus_up", &["k"], Action::Focus(Side::Up)),
        ("focus_right", &["l"], Action::Focus(Side::Right)),
        ("swap_left", &["H"], Action::Swap(Side::Left)),
        ("swap_down", &["J"], Action::Swap(Side::Down)),
        ("swap_up", &["K"], Action::Swap(Side::Up)),
        ("swap_right", &["L"], Action::Swap(Side::Right)),
        ("resize_mode", &["r"], Action::ResizeMode),
        ("zoom", &["z"], Action::Zoom),
        ("close_pane", &["x"], Action::ClosePane),
        ("new_tab", &["c"], Action::NewTab),
        ("next_tab", &["n", "tab"], Action::NextTab),
        ("prev_tab", &["p", "backtab"], Action::PrevTab),
        ("rename_tab", &["T"], Action::RenameTab),
        ("close_tab", &["X"], Action::CloseTab),
        ("new_workspace", &["N"], Action::NewWorkspace),
        ("rename_workspace", &["W"], Action::RenameWorkspace),
        ("close_workspace", &["D"], Action::CloseWorkspace),
        ("next_workspace", &["o"], Action::CycleWorkspace),
        ("toggle_sidebar", &["b"], Action::ToggleSidebar),
        ("help", &["?"], Action::Help),
        ("quit", &["q"], Action::Quit),
    ]
}

fn is_prefix(rt: &Runtime, key: &KeyEvent) -> bool {
    let p = rt.keymap.prefix;
    let strip = |m: KeyModifiers| m & !KeyModifiers::SHIFT;
    key.code == p.code && strip(key.modifiers) == strip(p.mods)
}

/// prefix+1..9 tab jumps — a fixed indexed family, not per-key configurable.
fn jump_tab_index(key: &KeyEvent) -> Option<usize> {
    if let KeyCode::Char(c @ '1'..='9') = key.code {
        if key.modifiers.is_empty() {
            return Some(c as usize - '1' as usize);
        }
    }
    None
}

/// Handle one key event according to the input-mode machine.
/// Returns Ok(true) to quit the app.
pub fn handle_key(rt: &mut Runtime, key: KeyEvent, area: Rect) -> io::Result<bool> {
    match rt.state.input_mode.clone() {
        InputMode::Terminal => {
            if let Some(entry) = rt.keymap.lookup_direct(key.code, key.modifiers).cloned() {
                rt.mark_dirty();
                return dispatch_bound(rt, entry.bound, area);
            }
            if is_prefix(rt, &key) {
                rt.state.input_mode = InputMode::Prefix;
                rt.mark_dirty();
            } else {
                rt.send_key(&key);
            }
            Ok(false)
        }
        InputMode::Prefix => {
            rt.state.input_mode = InputMode::Terminal;
            rt.mark_dirty();
            if is_prefix(rt, &key) {
                rt.send_key(&key); // double prefix → literal
                return Ok(false);
            }
            if let Some(ti) = jump_tab_index(&key) {
                rt.state.jump_tab(ti);
                return Ok(false);
            }
            match rt.keymap.lookup_prefixed(key.code, key.modifiers).cloned() {
                Some(entry) => dispatch_bound(rt, entry.bound, area),
                None => Ok(false), // unknown chord: swallow
            }
        }
        InputMode::Resize => {
            rt.mark_dirty();
            match key.code {
                KeyCode::Char('h') | KeyCode::Left => {
                    rt.state.resize_focused(Dir::Right, -RESIZE_STEP);
                }
                KeyCode::Char('l') | KeyCode::Right => {
                    rt.state.resize_focused(Dir::Right, RESIZE_STEP);
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    rt.state.resize_focused(Dir::Down, -RESIZE_STEP);
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    rt.state.resize_focused(Dir::Down, RESIZE_STEP);
                }
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => {
                    rt.state.input_mode = InputMode::Terminal;
                }
                _ => {}
            }
            Ok(false)
        }
        InputMode::Help => {
            rt.state.input_mode = InputMode::Terminal;
            rt.mark_dirty();
            Ok(false)
        }
        InputMode::Prompt { kind, mut buffer } => {
            rt.mark_dirty();
            match key.code {
                KeyCode::Enter => {
                    let name = buffer.trim().to_string();
                    if !name.is_empty() {
                        match kind {
                            PromptKind::RenameTab => rt.state.rename_active_tab(name),
                            PromptKind::RenameWorkspace => rt.state.rename_active_workspace(name),
                        }
                    }
                    rt.state.input_mode = InputMode::Terminal;
                }
                KeyCode::Esc => rt.state.input_mode = InputMode::Terminal,
                KeyCode::Backspace => {
                    buffer.pop();
                    rt.state.input_mode = InputMode::Prompt { kind, buffer };
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    buffer.push(c);
                    rt.state.input_mode = InputMode::Prompt { kind, buffer };
                }
                _ => {}
            }
            Ok(false)
        }
    }
}

const RESIZE_STEP: f32 = 0.03;

fn dispatch_bound(rt: &mut Runtime, bound: crate::config::keys::Bound, area: Rect) -> io::Result<bool> {
    match bound {
        crate::config::keys::Bound::Builtin(action) => dispatch(rt, action, area),
        crate::config::keys::Bound::Command(cmd) => {
            rt.run_custom_command(&cmd, area)?;
            Ok(false)
        }
    }
}

fn dispatch(rt: &mut Runtime, action: Action, area: Rect) -> io::Result<bool> {
    let rects = rt.last_view.as_ref().map(|v| v.pane_rects.clone()).unwrap_or_default();
    match action {
        Action::SplitRight => rt.split_focused(Dir::Right, area)?,
        Action::SplitDown => rt.split_focused(Dir::Down, area)?,
        Action::Focus(side) => {
            rt.state.focus_neighbor(&rects, side);
        }
        Action::Swap(side) => {
            rt.state.swap_with_neighbor(&rects, side);
        }
        Action::ResizeMode => rt.state.input_mode = InputMode::Resize,
        Action::Zoom => rt.state.toggle_zoom(),
        Action::ClosePane => rt.kill_pane(rt.state.focused_pane()),
        Action::NewTab => {
            let pane = rt.state.new_tab();
            rt.spawn_pane(pane, area.width, area.height)?;
        }
        Action::NextTab => rt.state.next_tab(),
        Action::PrevTab => rt.state.prev_tab(),
        Action::RenameTab => {
            rt.state.input_mode =
                InputMode::Prompt { kind: PromptKind::RenameTab, buffer: String::new() };
        }
        Action::CloseTab => {
            for pane in rt.state.active_tab_panes() {
                rt.kill_pane(pane);
            }
        }
        Action::NewWorkspace => {
            let pane = rt.state.new_workspace();
            rt.spawn_pane(pane, area.width, area.height)?;
        }
        Action::RenameWorkspace => {
            rt.state.input_mode =
                InputMode::Prompt { kind: PromptKind::RenameWorkspace, buffer: String::new() };
        }
        Action::CloseWorkspace => {
            for pane in rt.state.active_workspace_panes() {
                rt.kill_pane(pane);
            }
        }
        Action::CycleWorkspace => rt.state.cycle_workspace(),
        Action::ToggleSidebar => rt.state.sidebar_visible = !rt.state.sidebar_visible,
        Action::Help => rt.state.input_mode = InputMode::Help,
        Action::Quit => return Ok(true),
    }
    Ok(false)
}
