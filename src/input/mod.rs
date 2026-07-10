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
    JumpTab(u8),
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
            Action::JumpTab(_) => "jump to tab 1..9",
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

pub fn prefix_chord() -> Chord {
    // ponytail: configurable via [keys].prefix in M6
    Chord { code: KeyCode::Char('b'), mods: KeyModifiers::CONTROL }
}

fn ch(c: char) -> Chord {
    let mods = if c.is_ascii_uppercase() { KeyModifiers::SHIFT } else { KeyModifiers::NONE };
    Chord { code: KeyCode::Char(c), mods }
}

/// Default binding table (FEATURES.md defaults). Order = help-overlay order.
pub fn bindings() -> Vec<(Chord, &'static str, Action)> {
    let mut b: Vec<(Chord, &'static str, Action)> = vec![
        (ch('v'), "v", Action::SplitRight),
        (ch('-'), "-", Action::SplitDown),
        (ch('h'), "h", Action::Focus(Side::Left)),
        (ch('j'), "j", Action::Focus(Side::Down)),
        (ch('k'), "k", Action::Focus(Side::Up)),
        (ch('l'), "l", Action::Focus(Side::Right)),
        (ch('H'), "H", Action::Swap(Side::Left)),
        (ch('J'), "J", Action::Swap(Side::Down)),
        (ch('K'), "K", Action::Swap(Side::Up)),
        (ch('L'), "L", Action::Swap(Side::Right)),
        (
            Chord { code: KeyCode::Tab, mods: KeyModifiers::NONE },
            "tab",
            Action::NextTab,
        ),
        (
            Chord { code: KeyCode::BackTab, mods: KeyModifiers::SHIFT },
            "shift+tab",
            Action::PrevTab,
        ),
        (ch('r'), "r", Action::ResizeMode),
        (ch('z'), "z", Action::Zoom),
        (ch('x'), "x", Action::ClosePane),
        (ch('c'), "c", Action::NewTab),
        (ch('n'), "n", Action::NextTab),
        (ch('p'), "p", Action::PrevTab),
        (ch('T'), "T", Action::RenameTab),
        (ch('X'), "X", Action::CloseTab),
        (ch('N'), "N", Action::NewWorkspace),
        (ch('W'), "W", Action::RenameWorkspace),
        (ch('D'), "D", Action::CloseWorkspace),
        (ch('o'), "o", Action::CycleWorkspace),
        (ch('b'), "b", Action::ToggleSidebar),
        (ch('?'), "?", Action::Help),
        (ch('q'), "q", Action::Quit),
    ];
    for n in 1..=9u8 {
        b.push((ch((b'0' + n) as char), "1..9", Action::JumpTab(n)));
    }
    b
}

fn lookup(key: &KeyEvent) -> Option<Action> {
    // Char chords carry case; ignore SHIFT for comparison on chars,
    // compare mods exactly otherwise.
    bindings().into_iter().find_map(|(chord, _, action)| {
        let key_mods = key.modifiers & !KeyModifiers::SHIFT;
        let chord_mods = chord.mods & !KeyModifiers::SHIFT;
        (chord.code == key.code && key_mods == chord_mods).then_some(action)
    })
}

fn is_prefix(key: &KeyEvent) -> bool {
    let p = prefix_chord();
    key.code == p.code && key.modifiers.contains(KeyModifiers::CONTROL) == p.mods.contains(KeyModifiers::CONTROL)
}

/// Handle one key event according to the input-mode machine.
/// Returns Ok(true) to quit the app.
pub fn handle_key(rt: &mut Runtime, key: KeyEvent, area: Rect) -> io::Result<bool> {
    match rt.state.input_mode.clone() {
        InputMode::Terminal => {
            if is_prefix(&key) {
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
            if is_prefix(&key) {
                rt.send_key(&key); // double prefix → literal
                return Ok(false);
            }
            match lookup(&key) {
                Some(action) => dispatch(rt, action, area),
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
        Action::JumpTab(n) => rt.state.jump_tab((n - 1) as usize),
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
