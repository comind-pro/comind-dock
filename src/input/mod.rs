pub mod encode;
pub mod mouse;

use std::io;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;

use crate::runtime::{InputOutcome, Runtime};
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
    Search,
    ScrollbackEditor,
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
            Action::Search => "search scrollback",
            Action::ScrollbackEditor => "scrollback in $EDITOR",
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
        ("search", &["/"], Action::Search),
        ("scrollback_editor", &["e"], Action::ScrollbackEditor),
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
    if let KeyCode::Char(c @ '1'..='9') = key.code
        && key.modifiers.is_empty() {
            return Some(c as usize - '1' as usize);
        }
    None
}

/// Handle one key event according to the input-mode machine.
/// Returns Ok(true) to quit the app.
pub fn handle_key(rt: &mut Runtime, key: KeyEvent, area: Rect) -> io::Result<InputOutcome> {
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
            Ok(InputOutcome::Continue)
        }
        InputMode::Prefix => {
            rt.state.input_mode = InputMode::Terminal;
            rt.mark_dirty();
            if is_prefix(rt, &key) {
                rt.send_key(&key); // double prefix → literal
                return Ok(InputOutcome::Continue);
            }
            if let Some(ti) = jump_tab_index(&key) {
                rt.state.jump_tab(ti);
                return Ok(InputOutcome::Continue);
            }
            match rt.keymap.lookup_prefixed(key.code, key.modifiers).cloned() {
                Some(entry) => dispatch_bound(rt, entry.bound, area),
                None => Ok(InputOutcome::Continue), // unknown chord: swallow
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
            Ok(InputOutcome::Continue)
        }
        InputMode::Help => {
            rt.state.input_mode = InputMode::Terminal;
            rt.mark_dirty();
            Ok(InputOutcome::Continue)
        }
        InputMode::Menu { .. } => {
            rt.state.input_mode = InputMode::Terminal;
            rt.mark_dirty();
            Ok(InputOutcome::Continue)
        }
        InputMode::Search { mut buffer } => {
            rt.mark_dirty();
            match key.code {
                KeyCode::Enter => {
                    let focused = rt.state.focused_pane();
                    let found = !buffer.trim().is_empty()
                        && rt
                            .panes
                            .get_mut(&focused)
                            .is_some_and(|p| p.emu.start_search(buffer.trim()));
                    rt.state.input_mode =
                        if found { InputMode::SearchNav } else { InputMode::Terminal };
                }
                KeyCode::Esc => rt.state.input_mode = InputMode::Terminal,
                KeyCode::Backspace => {
                    buffer.pop();
                    rt.state.input_mode = InputMode::Search { buffer };
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    buffer.push(c);
                    rt.state.input_mode = InputMode::Search { buffer };
                }
                _ => {}
            }
            Ok(InputOutcome::Continue)
        }
        InputMode::SearchNav => {
            rt.mark_dirty();
            let focused = rt.state.focused_pane();
            match key.code {
                KeyCode::Char('n') => {
                    if let Some(p) = rt.panes.get_mut(&focused) {
                        p.emu.search_step(true);
                    }
                }
                KeyCode::Char('N') | KeyCode::Char('p') => {
                    if let Some(p) = rt.panes.get_mut(&focused) {
                        p.emu.search_step(false);
                    }
                }
                KeyCode::Char('/') => {
                    rt.state.input_mode = InputMode::Search { buffer: String::new() };
                }
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => {
                    if let Some(p) = rt.panes.get_mut(&focused) {
                        p.emu.clear_search();
                    }
                    rt.state.input_mode = InputMode::Terminal;
                }
                _ => {}
            }
            Ok(InputOutcome::Continue)
        }
        InputMode::ConfirmClose(pane) => {
            rt.mark_dirty();
            rt.state.input_mode = InputMode::Terminal;
            if matches!(key.code, KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter) {
                rt.kill_pane(pane);
            }
            Ok(InputOutcome::Continue)
        }
        InputMode::Prompt { kind, mut buffer } => {
            rt.mark_dirty();
            match key.code {
                KeyCode::Enter => {
                    let name = buffer.trim().to_string();
                    // Rename-pane accepts the empty string: it clears the
                    // custom name and falls back to the agent's own title.
                    if let PromptKind::RenamePane(pane) = kind {
                        rt.state.rename_pane(pane, name);
                        rt.state.input_mode = InputMode::Terminal;
                        rt.save_session();
                        return Ok(InputOutcome::Continue);
                    }
                    // Empty submit clears a custom tab name back to auto
                    // (the tab then follows the agent's/pane's own title).
                    if let PromptKind::RenameTab(id) = kind
                        && name.is_empty()
                    {
                        rt.state.reset_tab_name(id);
                        rt.state.input_mode = InputMode::Terminal;
                        return Ok(InputOutcome::Continue);
                    }
                    if !name.is_empty() {
                        match kind {
                            PromptKind::RenameTab(id) => rt.state.rename_tab_by_id(id, name),
                            PromptKind::RenameWorkspace(id) => {
                                rt.state.rename_workspace_by_id(id, name)
                            }
                            PromptKind::WorktreeBranch(ws) => {
                                rt.state.input_mode = InputMode::Terminal;
                                if let Some(wi) = rt.state.workspace_index(ws) {
                                    rt.create_worktree(wi, &name, area);
                                }
                                return Ok(InputOutcome::Continue);
                            }
                            PromptKind::NewSkill => {
                                rt.state.input_mode = InputMode::Terminal;
                                match crate::profile::skill_new(&name) {
                                    Ok(md) => rt.open_in_editor(&md, area)?,
                                    Err(e) => rt.add_plain_toast(format!("skill: {e}"), 10),
                                }
                                return Ok(InputOutcome::Continue);
                            }
                            PromptKind::RenamePane(_) => unreachable!("handled above"),
                            PromptKind::NewProfile(scope) => {
                                rt.state.input_mode = InputMode::Terminal;
                                let created = match &scope {
                                    Some(cwd) => crate::profile::scaffold_ws(cwd, &name),
                                    None => crate::profile::scaffold(&name, None),
                                };
                                match created {
                                    // The role text is the thing to write
                                    // first — nano/vim open the file, not
                                    // a directory listing.
                                    Ok(dir) => rt.open_in_editor(&dir.join("agent.md"), area)?,
                                    Err(e) => rt.add_plain_toast(format!("profile: {e}"), 10),
                                }
                                return Ok(InputOutcome::Continue);
                            }
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
            Ok(InputOutcome::Continue)
        }
    }
}

const RESIZE_STEP: f32 = 0.03;

fn dispatch_bound(rt: &mut Runtime, bound: crate::config::keys::Bound, area: Rect) -> io::Result<InputOutcome> {
    match bound {
        crate::config::keys::Bound::Builtin(action) => dispatch(rt, action, area),
        crate::config::keys::Bound::Command(cmd) => {
            rt.run_custom_command(&cmd, area)?;
            Ok(InputOutcome::Continue)
        }
    }
}

fn dispatch(rt: &mut Runtime, action: Action, area: Rect) -> io::Result<InputOutcome> {
    let rects = rt.last_view.as_ref().map(|v| v.pane_rects.clone()).unwrap_or_default();
    match action {
        Action::SplitRight => rt.split_focused(Dir::Right, false, area)?,
        Action::SplitDown => rt.split_focused(Dir::Down, false, area)?,
        Action::Focus(side) => {
            rt.state.focus_neighbor(&rects, side);
        }
        Action::Swap(side) => {
            rt.state.swap_with_neighbor(&rects, side);
        }
        Action::ResizeMode => rt.state.input_mode = InputMode::Resize,
        Action::Zoom => rt.state.toggle_zoom(),
        Action::ClosePane => {
            let focused = rt.state.focused_pane();
            if rt.cfg.ui.confirm_close {
                rt.state.input_mode = InputMode::ConfirmClose(focused);
            } else {
                rt.kill_pane(focused);
            }
        }
        Action::NewTab => {
            let pane = rt.state.new_tab();
            rt.spawn_pane(pane, area.width, area.height)?;
            if rt.cfg.ui.prompt_new_tab_name {
                let id = rt.state.active_tab().id;
                rt.state.input_mode =
                    InputMode::Prompt { kind: PromptKind::RenameTab(id), buffer: String::new() };
            }
        }
        Action::NextTab => rt.state.next_tab(),
        Action::PrevTab => rt.state.prev_tab(),
        Action::RenameTab => {
            let id = rt.state.active_tab().id;
            rt.state.input_mode =
                InputMode::Prompt { kind: PromptKind::RenameTab(id), buffer: String::new() };
        }
        Action::CloseTab => {
            for pane in rt.state.active_tab_panes() {
                rt.kill_pane(pane);
            }
        }
        Action::NewWorkspace => {
            let name = rt.workspace_name();
            let cwd = rt.new_space_cwd();
            let pane = rt.state.new_workspace(name, cwd, None);
            rt.spawn_pane(pane, area.width, area.height)?;
        }
        Action::RenameWorkspace => {
            let id = rt.state.active_workspace().id;
            rt.state.input_mode = InputMode::Prompt {
                kind: PromptKind::RenameWorkspace(id),
                buffer: String::new(),
            };
        }
        Action::CloseWorkspace => {
            for pane in rt.state.active_workspace_panes() {
                rt.kill_pane(pane);
            }
        }
        Action::CycleWorkspace => rt.state.cycle_workspace(),
        Action::ToggleSidebar => rt.state.sidebar_visible = !rt.state.sidebar_visible,
        Action::Search => rt.state.input_mode = InputMode::Search { buffer: String::new() },
        Action::ScrollbackEditor => {
            let focused = rt.state.focused_pane();
            if let Some(p) = rt.panes.get(&focused) {
                let mut text = p.emu.scrollback_text();
                if p.emu.on_alt_screen() {
                    // The alt grid has no history — be honest about it.
                    text.insert_str(
                        0,
                        "# fullscreen app: visible screen only, no scrollback\n\n",
                    );
                }
                let path = std::env::temp_dir().join(format!("cdock-scrollback-{}.txt", focused.0));
                match std::fs::write(&path, text) {
                    Ok(()) => {
                        let editor = rt.cfg.terminal.editor_cmd();
                        let pane = rt.state.new_tab();
                        rt.spawn_pane_cmd(
                            pane,
                            area.width,
                            area.height,
                            Some(format!("{editor} {}", path.display())),
                        )?;
                    }
                    Err(e) => tracing::warn!(error = %e, "scrollback dump failed"),
                }
            }
        }
        Action::Help => rt.state.input_mode = InputMode::Help,
        Action::Quit => return Ok(InputOutcome::Detach),
    }
    Ok(InputOutcome::Continue)
}
