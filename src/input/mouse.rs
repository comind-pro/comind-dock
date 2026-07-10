//! Mouse handling: hit testing against the last computed View.
//! Arbitration rule: chrome (tab bar, sidebar) and dividers always win;
//! inside a pane, an app that requested mouse reporting gets encoded events,
//! otherwise the multiplexer owns selection and wheel scrollback.

use std::time::{Duration, Instant};

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};

use crate::runtime::{InputOutcome, MouseDrag, Runtime, osc52_bytes};
use crate::state::ids::PaneId;
use crate::state::layout::Dir;
use crate::state::{InputMode, MenuAction, MenuItem, PromptKind};
use crate::ui::{menu, sidebar, tabbar};

const DOUBLE_CLICK: Duration = Duration::from_millis(400);

/// Handle a mouse event.
pub fn handle(rt: &mut Runtime, ev: MouseEvent, area: Rect) -> InputOutcome {
    let Some(view) = rt.last_view.clone() else { return InputOutcome::Continue };
    let pos = Position::new(ev.column, ev.row);
    let scroll_lines = rt.cfg.ui.mouse_scroll_lines.max(1) as i32;

    // The help overlay closes on any click (keys already close it).
    if rt.state.input_mode == InputMode::Help {
        if matches!(ev.kind, MouseEventKind::Down(_)) {
            rt.state.input_mode = InputMode::Terminal;
            rt.mark_dirty();
        }
        return InputOutcome::Continue;
    }

    // An open context menu captures clicks. Releases and moves (including
    // the release of the click that opened it) keep it open.
    if let InputMode::Menu { x, y, items } = rt.state.input_mode.clone() {
        match ev.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                rt.state.input_mode = InputMode::Terminal;
                rt.mark_dirty();
                let mrect = menu::rect(x, y, &items, area);
                if let Some(action) = menu::hit(mrect, &items, ev.column, ev.row) {
                    return run_menu_action(rt, action, x, y, area);
                }
            }
            MouseEventKind::Down(_) => {
                rt.state.input_mode = InputMode::Terminal;
                rt.mark_dirty();
            }
            // Moves, releases, and trackpad scroll inertia keep the menu open.
            _ => {}
        }
        return InputOutcome::Continue;
    }

    match ev.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            rt.mark_dirty();

            // Toasts float above all chrome: click = jump to the pane.
            if let Some(i) = crate::ui::toast::rects(rt, area)
                .iter()
                .position(|(r, _)| r.contains(pos))
            {
                let pane = rt.toasts.remove(i).pane;
                rt.state.focus_pane(pane);
                return InputOutcome::Continue;
            }

            if view.tab_bar.contains(pos) {
                match tabbar::hit(rt, ev.column - view.tab_bar.x, view.tab_bar.width) {
                    Some(tabbar::Hit::Tab(ti)) => rt.state.jump_tab(ti),
                    Some(tabbar::Hit::NewTab) => {
                        let pane = rt.state.new_tab();
                        let size = view
                            .pane_rects
                            .first()
                            .map(|(_, r)| (r.width, r.height))
                            .unwrap_or((80, 24));
                        if let Err(e) = rt.spawn_pane(pane, size.0.max(4), size.1.max(4)) {
                            tracing::warn!(error = %e, "new tab spawn failed");
                        }
                    }
                    Some(tabbar::Hit::CloseTab(ti)) => {
                        let panes = rt
                            .state
                            .active_workspace()
                            .tabs
                            .get(ti)
                            .map(|t| t.layout.panes())
                            .unwrap_or_default();
                        for pane in panes {
                            rt.kill_pane(pane);
                        }
                    }
                    Some(tabbar::Hit::CloseApp) => {
                        return InputOutcome::Shutdown;
                    }
                    Some(tabbar::Hit::ShowSidebar) => {
                        rt.state.sidebar_visible = true;
                    }
                    None => {}
                }
                return InputOutcome::Continue;
            }
            if let Some(sb) = view.sidebar
                && sb.contains(pos)
            {
                let theme = rt.theme;
                match sidebar::hit(rt, &theme, ev.column - sb.x, ev.row - sb.y, (sb.width, sb.height)) {
                    Some(sidebar::Target::Workspace(wi)) => {
                        // Plain click switches; the menu lives on right-click.
                        rt.state.active_workspace = wi;
                    }
                    Some(sidebar::Target::CollapseSidebar) => {
                        rt.state.sidebar_visible = false;
                    }
                    Some(sidebar::Target::AppMenu) => {
                        rt.state.input_mode = InputMode::Menu {
                            x: ev.column,
                            y: ev.row,
                            items: menu::app_items(),
                        };
                    }
                    Some(sidebar::Target::ContinueAgent) => {
                        return run_menu_action(
                            rt,
                            MenuAction::ContinuePicker,
                            ev.column,
                            ev.row,
                            area,
                        );
                    }
                    Some(sidebar::Target::Pane(pane)) => {
                        rt.state.focus_pane(pane);
                    }
                    Some(sidebar::Target::NewWorkspace) => {
                        let name = rt.workspace_name();
                        let cwd = rt.new_space_cwd();
                        let pane = rt.state.new_workspace(name, cwd, None);
                        let size = view
                            .pane_rects
                            .first()
                            .map(|(_, r)| (r.width, r.height))
                            .unwrap_or((80, 24));
                        if let Err(e) = rt.spawn_pane(pane, size.0.max(4), size.1.max(4)) {
                            tracing::warn!(error = %e, "new workspace spawn failed");
                        }
                    }
                    None => {}
                }
                return InputOutcome::Continue;
            }
            if let Some(d) = view.dividers.iter().find(|d| d.rect.contains(pos)) {
                let last_pos = if d.dir == Dir::Right { ev.column } else { ev.row };
                rt.drag = Some(MouseDrag::Divider {
                    before: d.before,
                    after: d.after,
                    dir: d.dir,
                    extent: d.extent,
                    last_pos,
                });
                return InputOutcome::Continue;
            }
            if let Some((id, rect)) = pane_at(&view.pane_rects, pos) {
                rt.state.active_tab_mut().focused_pane = id;
                let inner = crate::ui::content_rect(rect);
                if !inner.contains(pos) {
                    return InputOutcome::Continue; // border click: focus only
                }
                let (col, row) = (ev.column - inner.x, ev.row - inner.y);
                let semantic = rt
                    .last_click
                    .is_some_and(|(t, x, y)| {
                        t.elapsed() < DOUBLE_CLICK && x == ev.column && y == ev.row
                    });
                rt.last_click = Some((Instant::now(), ev.column, ev.row));
                if let Some(p) = rt.panes.get_mut(&id) {
                    if p.emu.wants_mouse() {
                        let bytes = sgr(0, col, row, true, ev.modifiers);
                        send_mouse(rt, id, bytes);
                    } else {
                        p.emu.clear_selection();
                        p.emu.start_selection(col, row, semantic);
                        rt.drag = Some(MouseDrag::Select { pane: id });
                    }
                }
            }
        }

        MouseEventKind::Drag(MouseButton::Left) => match rt.drag {
            Some(MouseDrag::Divider { before, after, dir, extent, last_pos }) => {
                let now = if dir == Dir::Right { ev.column } else { ev.row };
                let delta_cells = now as i32 - last_pos as i32;
                if delta_cells != 0 && extent > 0 {
                    let delta = delta_cells as f32 / extent as f32;
                    rt.state.active_tab_mut().layout.resize_split(before, after, delta);
                    rt.drag = Some(MouseDrag::Divider { before, after, dir, extent, last_pos: now });
                    rt.mark_dirty();
                }
            }
            Some(MouseDrag::Select { pane }) => {
                if let Some((_, r)) =
                    view.pane_rects.iter().find(|(id, _)| *id == pane).copied()
                {
                    let rect = crate::ui::content_rect(r);
                    let col = ev.column.clamp(rect.x, rect.x + rect.width.saturating_sub(1)) - rect.x;
                    let row = ev.row.clamp(rect.y, rect.y + rect.height.saturating_sub(1)) - rect.y;
                    if let Some(p) = rt.panes.get_mut(&pane) {
                        p.emu.update_selection(col, row);
                        rt.mark_dirty();
                    }
                }
            }
            None => {
                // Drag inside a mouse-reporting app.
                if let Some((id, rect)) = pane_at(&view.pane_rects, pos) {
                    let inner = crate::ui::content_rect(rect);
                    if inner.contains(pos)
                        && rt.panes.get(&id).is_some_and(|p| p.emu.wants_mouse())
                    {
                        let bytes =
                            sgr(32, ev.column - inner.x, ev.row - inner.y, true, ev.modifiers);
                        send_mouse(rt, id, bytes);
                    }
                }
            }
        },

        MouseEventKind::Up(MouseButton::Left) => {
            match rt.drag.take() {
                Some(MouseDrag::Select { pane }) => {
                    let text = rt.panes.get(&pane).and_then(|p| p.emu.selection_text());
                    if let Some(text) = text
                        && !text.is_empty() {
                            rt.host_write(osc52_bytes(&text));
                        }
                }
                Some(MouseDrag::Divider { .. }) => {}
                None => {
                    if let Some((id, rect)) = pane_at(&view.pane_rects, pos) {
                        let inner = crate::ui::content_rect(rect);
                        if inner.contains(pos)
                            && rt.panes.get(&id).is_some_and(|p| p.emu.wants_mouse())
                        {
                            let bytes =
                                sgr(0, ev.column - inner.x, ev.row - inner.y, false, ev.modifiers);
                            send_mouse(rt, id, bytes);
                        }
                    }
                }
            }
        }

        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
            let up = ev.kind == MouseEventKind::ScrollUp;
            if let Some(sb) = view.sidebar
                && sb.contains(pos)
            {
                let theme = rt.theme;
                let max = sidebar::max_scroll(rt, &theme, (sb.width, sb.height));
                let step = scroll_lines as u16;
                let cur = rt.sidebar_scroll.min(max);
                rt.sidebar_scroll =
                    if up { cur.saturating_sub(step) } else { (cur + step).min(max) };
                if rt.sidebar_scroll != cur {
                    rt.mark_dirty();
                }
                return InputOutcome::Continue;
            }
            if let Some((id, rect)) = pane_at(&view.pane_rects, pos) {
                let inner = crate::ui::content_rect(rect);
                if !inner.contains(pos) {
                    return InputOutcome::Continue;
                }
                let (col, row) = (ev.column - inner.x, ev.row - inner.y);
                let Some(p) = rt.panes.get_mut(&id) else { return InputOutcome::Continue };
                if p.emu.wants_mouse() {
                    let bytes = sgr(if up { 64 } else { 65 }, col, row, true, ev.modifiers);
                    send_mouse(rt, id, bytes);
                } else if p.emu.alternate_scroll() {
                    let arrow: &[u8] = if up { b"\x1bOA" } else { b"\x1bOB" };
                    let bytes = arrow.repeat(scroll_lines as usize);
                    p.pty.write(&bytes);
                } else {
                    p.emu.scroll_display(if up { scroll_lines } else { -scroll_lines });
                    rt.mark_dirty();
                }
            }
        }

        MouseEventKind::Down(MouseButton::Right) => {
            // Right-click on a sidebar space opens its menu (same gesture as
            // the pane context menu).
            if let Some(sb) = view.sidebar
                && sb.contains(pos)
            {
                let theme = rt.theme;
                if let Some(sidebar::Target::Workspace(wi)) =
                    sidebar::hit(rt, &theme, ev.column - sb.x, ev.row - sb.y, (sb.width, sb.height))
                {
                    rt.state.active_workspace = wi;
                    rt.state.input_mode = InputMode::Menu {
                        x: ev.column,
                        y: ev.row,
                        items: menu::space_items(wi),
                    };
                    rt.mark_dirty();
                }
                return InputOutcome::Continue;
            }
            // Right-click on a pane opens the context menu.
            if let Some((id, _)) = pane_at(&view.pane_rects, pos) {
                rt.state.active_tab_mut().focused_pane = id;
                rt.state.input_mode = InputMode::Menu {
                    x: ev.column,
                    y: ev.row,
                    items: menu::pane_items(id),
                };
                rt.mark_dirty();
            }
        }

        MouseEventKind::Down(MouseButton::Middle) => {
            if let Some((id, rect)) = pane_at(&view.pane_rects, pos) {
                let inner = crate::ui::content_rect(rect);
                if inner.contains(pos)
                    && rt.panes.get(&id).is_some_and(|p| p.emu.wants_mouse())
                {
                    let bytes = sgr(1, ev.column - inner.x, ev.row - inner.y, true, ev.modifiers);
                    send_mouse(rt, id, bytes);
                }
            }
        }

        _ => {}
    }
    InputOutcome::Continue
}

fn run_menu_action(
    rt: &mut Runtime,
    action: MenuAction,
    x: u16,
    y: u16,
    area: Rect,
) -> InputOutcome {
    let result = match action {
        MenuAction::SplitRight(pane) => {
            rt.state.focus_pane(pane);
            rt.split_focused(Dir::Right, false, area)
        }
        MenuAction::SplitLeft(pane) => {
            rt.state.focus_pane(pane);
            rt.split_focused(Dir::Right, true, area)
        }
        MenuAction::SplitDown(pane) => {
            rt.state.focus_pane(pane);
            rt.split_focused(Dir::Down, false, area)
        }
        MenuAction::SplitUp(pane) => {
            rt.state.focus_pane(pane);
            rt.split_focused(Dir::Down, true, area)
        }
        MenuAction::ClosePane(pane) => {
            rt.kill_pane(pane);
            Ok(())
        }
        MenuAction::RenameSpace(wi) => {
            rt.state.active_workspace = wi.min(rt.state.workspaces.len().saturating_sub(1));
            rt.state.input_mode =
                InputMode::Prompt { kind: PromptKind::RenameWorkspace, buffer: String::new() };
            Ok(())
        }
        MenuAction::CloseSpace(wi) => {
            for pane in rt.state.workspace_panes(wi) {
                rt.kill_pane(pane);
            }
            Ok(())
        }
        MenuAction::NewWorktree(wi) => {
            rt.state.input_mode = InputMode::Prompt {
                kind: PromptKind::WorktreeBranch(wi),
                buffer: String::new(),
            };
            Ok(())
        }
        MenuAction::OpenSettings => {
            // Seed the file with the annotated defaults so $EDITOR opens
            // something editable, then edit it in a fresh tab.
            match crate::config::config_path(None) {
                Some(path) => {
                    if !path.exists() {
                        if let Some(dir) = path.parent() {
                            let _ = std::fs::create_dir_all(dir);
                        }
                        let _ = std::fs::write(&path, crate::config::DEFAULT_CONFIG);
                    }
                    let editor =
                        std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
                    let pane = rt.state.new_tab();
                    rt.spawn_pane_cmd(
                        pane,
                        area.width,
                        area.height,
                        Some(format!("{editor} {}", path.display())),
                    )
                }
                None => Ok(()),
            }
        }
        MenuAction::AgentPicker(split) => {
            let items: Vec<MenuItem> = crate::profile::list()
                .into_iter()
                .map(|name| MenuItem {
                    label: name.clone(),
                    action: MenuAction::StartProfile(name, split),
                })
                .collect();
            if items.is_empty() {
                tracing::info!("no profiles yet — `cdock profile new <name>`");
            } else {
                rt.state.input_mode = InputMode::Menu { x, y, items };
            }
            Ok(())
        }
        MenuAction::StartProfile(name, split) => match crate::profile::load(&name) {
            Ok(p) => {
                let (command, env) = p.resolve();
                let pane = match split {
                    Some(target) => {
                        rt.state.focus_pane(target);
                        rt.state.split_focused(Dir::Right, false)
                    }
                    None => rt.state.new_tab(),
                };
                rt.spawn_pane_env(
                    pane,
                    area.width.max(2) / 2,
                    area.height.max(2) / 2,
                    Some(command),
                    env,
                )
            }
            Err(e) => {
                tracing::warn!(profile = %name, error = %e, "profile launch failed");
                Ok(())
            }
        },
        MenuAction::ContinuePicker => {
            // Sessions already open in a pane are hidden — no double resume.
            let open: std::collections::HashSet<&String> =
                rt.agent_sessions.values().collect();
            let items: Vec<MenuItem> = crate::agents::recent_claude_sessions(9)
                .into_iter()
                .filter(|s| !open.contains(&s.id))
                .map(|s| {
                    let folder = s
                        .cwd
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    MenuItem {
                        label: format!("{} · {}", s.title, folder),
                        action: MenuAction::ResumeClaudeSession(s.id, s.cwd),
                    }
                })
                .collect();
            if items.is_empty() {
                tracing::info!("no resumable claude sessions found");
            } else {
                rt.state.input_mode = InputMode::Menu { x, y, items };
            }
            Ok(())
        }
        MenuAction::ResumeClaudeSession(id, cwd) => {
            // Land in the space anchored at the session's folder (reuse or
            // create) — the conversation is folder-bound.
            let pane = match rt.state.workspaces.iter().position(|w| w.cwd == cwd) {
                Some(wi) => {
                    rt.state.active_workspace = wi;
                    rt.state.new_tab()
                }
                None => {
                    let name = cwd
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "/".to_string());
                    rt.state.new_workspace(name, cwd.clone(), None)
                }
            };
            rt.spawn_pane_full(
                pane,
                area.width.max(2) / 2,
                area.height.max(2) / 2,
                Some(format!("claude --resume {id}")),
                Vec::new(),
                Some(cwd),
            )
        }
        MenuAction::EditProfiles => {
            match crate::profile::profiles_dir() {
                Some(dir) => {
                    let _ = std::fs::create_dir_all(&dir);
                    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
                    let pane = rt.state.new_tab();
                    rt.spawn_pane_cmd(
                        pane,
                        area.width,
                        area.height,
                        Some(format!("{editor} {}", dir.display())),
                    )
                }
                None => Ok(()),
            }
        }
        MenuAction::ShowKeybinds => {
            rt.state.input_mode = InputMode::Help;
            Ok(())
        }
        MenuAction::ReloadConfig => {
            rt.reload_config();
            Ok(())
        }
        MenuAction::Detach => return InputOutcome::Detach,
        MenuAction::ListWorktrees(wi) => {
            let Some(ws) = rt.state.workspaces.get(wi) else {
                return InputOutcome::Continue;
            };
            let current = ws.cwd.clone();
            let items: Vec<MenuItem> = crate::git::worktrees(&ws.cwd)
                .into_iter()
                .filter(|(path, _)| *path != current)
                .map(|(path, branch)| MenuItem {
                    label: branch,
                    action: MenuAction::OpenWorktree(wi, path),
                })
                .collect();
            if items.is_empty() {
                tracing::info!("no other worktrees to open");
            } else {
                rt.state.input_mode = InputMode::Menu { x, y, items };
            }
            Ok(())
        }
        MenuAction::OpenWorktree(wi, path) => {
            if let Some(parent_id) = rt.state.workspaces.get(wi).map(|w| w.id) {
                rt.open_worktree(parent_id, path, area);
            }
            Ok(())
        }
    };
    if let Err(e) = result {
        tracing::warn!(error = %e, "menu action failed");
    }
    InputOutcome::Continue
}

fn pane_at(rects: &[(PaneId, Rect)], pos: Position) -> Option<(PaneId, Rect)> {
    rects.iter().find(|(_, r)| r.contains(pos)).copied()
}

/// SGR (1006) mouse encoding with pane-local 1-based coordinates.
/// ponytail: SGR only — X10 fallback if a legacy app ever needs it.
fn sgr(button: u8, col: u16, row: u16, press: bool, mods: KeyModifiers) -> Vec<u8> {
    let mut b = button;
    if mods.contains(KeyModifiers::SHIFT) {
        b += 4;
    }
    if mods.contains(KeyModifiers::ALT) {
        b += 8;
    }
    if mods.contains(KeyModifiers::CONTROL) {
        b += 16;
    }
    format!("\x1b[<{};{};{}{}", b, col + 1, row + 1, if press { 'M' } else { 'm' }).into_bytes()
}

fn send_mouse(rt: &mut Runtime, pane: PaneId, bytes: Vec<u8>) {
    if let Some(p) = rt.panes.get_mut(&pane) {
        p.pty.write(&bytes);
    }
}
