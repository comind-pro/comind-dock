//! Mouse handling: hit testing against the last computed View.
//! Arbitration rule: chrome (tab bar, sidebar) and dividers always win;
//! inside a pane, an app that requested mouse reporting gets encoded events,
//! otherwise the multiplexer owns selection and wheel scrollback.

use std::time::{Duration, Instant};

use alacritty_terminal::term::TermMode;
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
                if let Some(pane) = rt.toasts.remove(i).pane {
                    rt.state.focus_pane(pane);
                }
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
                            items: menu::app_items(rt.update_available.as_deref()),
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
                // Clicking away mid-search: close the search on the OLD pane
                // or its highlight/mode go stale.
                if matches!(rt.state.input_mode, InputMode::Search { .. } | InputMode::SearchNav)
                    && id != rt.state.focused_pane()
                {
                    let old = rt.state.focused_pane();
                    if let Some(p) = rt.panes.get_mut(&old) {
                        p.emu.clear_search();
                    }
                    rt.state.input_mode = InputMode::Terminal;
                }
                // focus_pane validates: the view is a frame old, the pane
                // may have died — a raw assignment plants a dead id.
                if !rt.state.focus_pane(id) {
                    return InputOutcome::Continue;
                }
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
                        let bytes =
                            encode_mouse(*p.emu.term.mode(), 0, col, row, true, ev.modifiers);
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
                // Drag inside a mouse-reporting app — only when it asked
                // for drag (1002) or any-motion (1003) reporting.
                if let Some((id, rect)) = pane_at(&view.pane_rects, pos) {
                    let inner = crate::ui::content_rect(rect);
                    if inner.contains(pos)
                        && let Some(mode) = mouse_mode(rt, id)
                        && mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION)
                    {
                        let bytes = encode_mouse(
                            mode,
                            32,
                            ev.column - inner.x,
                            ev.row - inner.y,
                            true,
                            ev.modifiers,
                        );
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
                            && let Some(mode) = mouse_mode(rt, id)
                        {
                            let bytes = encode_mouse(
                                mode,
                                0,
                                ev.column - inner.x,
                                ev.row - inner.y,
                                false,
                                ev.modifiers,
                            );
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
                    let bytes = encode_mouse(
                        *p.emu.term.mode(),
                        if up { 64 } else { 65 },
                        col,
                        row,
                        true,
                        ev.modifiers,
                    );
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
            // Right-click a tab: new / rename / close (the ✕ still closes).
            if view.tab_bar.contains(pos)
                && let Some(tabbar::Hit::Tab(ti) | tabbar::Hit::CloseTab(ti)) =
                    tabbar::hit(rt, ev.column - view.tab_bar.x, view.tab_bar.width)
                && let Some(tab) = rt.state.active_workspace().tabs.get(ti)
            {
                let id = tab.id;
                rt.state.input_mode =
                    InputMode::Menu { x: ev.column, y: ev.row, items: menu::tab_items(id) };
                rt.mark_dirty();
                return InputOutcome::Continue;
            }
            // Right-click on a sidebar space opens its menu (same gesture as
            // the pane context menu).
            if let Some(sb) = view.sidebar
                && sb.contains(pos)
            {
                let theme = rt.theme;
                match sidebar::hit(rt, &theme, ev.column - sb.x, ev.row - sb.y, (sb.width, sb.height))
                {
                    Some(sidebar::Target::Workspace(wi)) => {
                        rt.state.active_workspace = wi;
                        let ws_id = rt.state.workspaces[wi].id;
                        rt.state.input_mode = InputMode::Menu {
                            x: ev.column,
                            y: ev.row,
                            items: menu::space_items(ws_id),
                        };
                        rt.mark_dirty();
                    }
                    // Agent row: the options menu (behavior, focus, close).
                    Some(sidebar::Target::Pane(pane)) => {
                        return run_menu_action(
                            rt,
                            MenuAction::AgentOptions(pane),
                            ev.column,
                            ev.row,
                            area,
                        );
                    }
                    _ => {}
                }
                return InputOutcome::Continue;
            }
            // Right-click on a pane opens the context menu.
            if let Some((id, _)) = pane_at(&view.pane_rects, pos) {
                if !rt.state.focus_pane(id) {
                    return InputOutcome::Continue;
                }
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
                    && let Some(mode) = mouse_mode(rt, id)
                {
                    let bytes = encode_mouse(
                        mode,
                        1,
                        ev.column - inner.x,
                        ev.row - inner.y,
                        true,
                        ev.modifiers,
                    );
                    send_mouse(rt, id, bytes);
                }
            }
        }

        // Right/middle drags and bare motion for apps that asked: drag
        // buttons carry 32 + button, buttonless motion is 32 + 3 (X11
        // "no button" = 3), only under any-motion (1003) for the latter.
        MouseEventKind::Drag(btn @ (MouseButton::Right | MouseButton::Middle)) => {
            if let Some((id, rect)) = pane_at(&view.pane_rects, pos) {
                let inner = crate::ui::content_rect(rect);
                if inner.contains(pos)
                    && let Some(mode) = mouse_mode(rt, id)
                    && mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION)
                {
                    let button = if btn == MouseButton::Middle { 32 + 1 } else { 32 + 2 };
                    let bytes = encode_mouse(
                        mode,
                        button,
                        ev.column - inner.x,
                        ev.row - inner.y,
                        true,
                        ev.modifiers,
                    );
                    send_mouse(rt, id, bytes);
                }
            }
        }
        MouseEventKind::Moved => {
            if let Some((id, rect)) = pane_at(&view.pane_rects, pos) {
                let inner = crate::ui::content_rect(rect);
                if inner.contains(pos)
                    && let Some(mode) = mouse_mode(rt, id)
                    && mode.contains(TermMode::MOUSE_MOTION)
                {
                    let bytes = encode_mouse(
                        mode,
                        32 + 3,
                        ev.column - inner.x,
                        ev.row - inner.y,
                        true,
                        ev.modifiers,
                    );
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
        // The pane may have died while the menu was open — a failed focus
        // must not split whatever happens to be focused now.
        MenuAction::SplitRight(pane) => {
            if !rt.state.focus_pane(pane) {
                return InputOutcome::Continue;
            }
            rt.split_focused(Dir::Right, false, area)
        }
        MenuAction::SplitLeft(pane) => {
            if !rt.state.focus_pane(pane) {
                return InputOutcome::Continue;
            }
            rt.split_focused(Dir::Right, true, area)
        }
        MenuAction::SplitDown(pane) => {
            if !rt.state.focus_pane(pane) {
                return InputOutcome::Continue;
            }
            rt.split_focused(Dir::Down, false, area)
        }
        MenuAction::SplitUp(pane) => {
            if !rt.state.focus_pane(pane) {
                return InputOutcome::Continue;
            }
            rt.split_focused(Dir::Down, true, area)
        }
        MenuAction::ClosePane(pane) => {
            rt.kill_pane(pane);
            Ok(())
        }
        MenuAction::RenameSpace(ws) => {
            if rt.state.workspace_index(ws).is_some() {
                rt.state.input_mode = InputMode::Prompt {
                    kind: PromptKind::RenameWorkspace(ws),
                    buffer: String::new(),
                };
            }
            Ok(())
        }
        MenuAction::CloseSpace(ws) => {
            // Resolve id → index at CLICK time: the workspaces vec may have
            // shifted while the menu was open (a pane exited in background).
            if let Some(wi) = rt.state.workspace_index(ws) {
                for pane in rt.state.workspace_panes(wi) {
                    rt.kill_pane(pane);
                }
            }
            Ok(())
        }
        MenuAction::NewWorktree(ws) => {
            rt.state.input_mode = InputMode::Prompt {
                kind: PromptKind::WorktreeBranch(ws),
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
                        rt.cfg.terminal.editor_cmd();
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
            // The space's assigned profile leads the list as the default.
            let assoc = rt
                .state
                .workspaces
                .get(rt.state.active_workspace)
                .and_then(|w| w.profile.clone());
            let ws_cwd = rt.state.workspaces.get(rt.state.active_workspace).map(|w| w.cwd.clone());
            let mut items: Vec<MenuItem> = ws_cwd
                .as_deref()
                .map(crate::profile::list_ws)
                .unwrap_or_default()
                .into_iter()
                .map(|name| MenuItem {
                    label: format!("{name} (space)"),
                    action: MenuAction::StartProfile(format!("ws:{name}"), split),
                })
                .collect();
            items.extend(crate::profile::list().into_iter().map(|name| MenuItem {
                label: if assoc.as_deref() == Some(name.as_str()) {
                    format!("{name} (space default)")
                } else {
                    name.clone()
                },
                action: MenuAction::StartProfile(name, split),
            }));
            items.sort_by_key(|i| !i.label.ends_with("(space default)"));
            if items.is_empty() {
                tracing::info!("no profiles yet — `cdock profile new <name>`");
            } else {
                rt.state.input_mode = InputMode::Menu { x, y, items };
            }
            Ok(())
        }
        MenuAction::SpaceProfilePicker(ws) => {
            let mut items: Vec<MenuItem> = crate::profile::list()
                .into_iter()
                .map(|name| MenuItem {
                    label: name.clone(),
                    action: MenuAction::SetSpaceProfile(ws, Some(name)),
                })
                .collect();
            items.push(MenuItem {
                label: "(none)".to_string(),
                action: MenuAction::SetSpaceProfile(ws, None),
            });
            rt.state.input_mode = InputMode::Menu { x, y, items };
            Ok(())
        }
        MenuAction::SetSpaceProfile(ws, profile) => {
            if let Some(w) = rt.state.workspaces.iter_mut().find(|w| w.id == ws) {
                w.profile = profile;
                rt.mark_dirty();
            }
            Ok(())
        }
        MenuAction::StartProfile(name, split) => {
            let ws_cwd = rt
                .state
                .workspaces
                .get(rt.state.active_workspace)
                .map(|w| w.cwd.clone());
            match crate::profile::load_any(&name, ws_cwd.as_deref().unwrap_or(std::path::Path::new("/")))
            {
            Ok(p) => {
                let (command, mut env) = p.resolve_with(ws_cwd.as_deref());
                // The new agent lives where its parent lives: the pane we
                // split, else the focused one.
                let parent = split.unwrap_or_else(|| rt.state.focused_pane());
                let parent_dir =
                    rt.panes.get(&parent).and_then(|p| p.agent_config_dir.clone());
                crate::agents::inherit_claude_profile(&mut env, parent_dir.as_deref());
                let pane = match split {
                    Some(target) => match rt.state.split_pane(target, Dir::Right) {
                        Some(p) => p,
                        None => return InputOutcome::Continue, // pane died under the menu
                    },
                    None => rt.state.new_tab(),
                };
                rt.spawn_pane_env(
                    pane,
                    area.width.max(2) / 2,
                    area.height.max(2) / 2,
                    Some(crate::agents::hold_on_failure(&command)),
                    env,
                )
            }
            Err(e) => {
                tracing::warn!(profile = %name, error = %e, "profile launch failed");
                Ok(())
            }
        }},
        MenuAction::ContinuePicker => {
            // Sessions open in a pane with a LIVE agent are hidden — no
            // double resume. Panes whose agent exited (shell remains) keep a
            // stale map entry; their conversations must stay listable.
            // agent_sessions holds full "agent:id" idents; the picker's ids
            // are bare — compare on the id, or every live session shows up
            // here as resumable.
            let open: std::collections::HashSet<String> = rt
                .panes
                .iter()
                .filter(|(_, p)| p.agent.is_some())
                .filter_map(|(id, _)| rt.agent_sessions.get(id))
                .map(|ident| ident.split_once(':').map_or(ident.clone(), |(_, i)| i.to_string()))
                .collect();
            let names = crate::agents::session_names();
            let items: Vec<MenuItem> = crate::agents::recent_claude_sessions(9)
                .into_iter()
                .filter(|s| !open.contains(&s.id))
                .map(|s| {
                    let folder = s
                        .cwd
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    // The user's name for this conversation wins over its
                    // first prompt — same name as the sidebar and tab bar.
                    let title = names.get(&s.id).cloned().unwrap_or_else(|| s.title.clone());
                    let label = match s.profile_label() {
                        Some(p) => format!("{title} · {folder} · @{p}"),
                        None => format!("{title} · {folder}"),
                    };
                    MenuItem {
                        label,
                        action: MenuAction::ResumeClaudeSession(s.id, s.cwd, s.config_dir),
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
        MenuAction::ResumeClaudeSession(id, cwd, config_dir) => {
            let stored_name = crate::agents::session_names().get(&id).cloned();
            // Land in the deepest space containing the session's folder
            // (cpgps/alert-service joins cpgps, not a new sibling space);
            // create one only when nothing contains it.
            let found = rt
                .state
                .workspaces
                .iter()
                .enumerate()
                .filter(|(_, w)| cwd.starts_with(&w.cwd))
                .max_by_key(|(_, w)| w.cwd.as_os_str().len())
                .map(|(i, _)| i);
            let pane = match found {
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
            let env = config_dir
                .map(|d| vec![("CLAUDE_CONFIG_DIR".to_string(), d.display().to_string())])
                .unwrap_or_default();
            let spawned = rt.spawn_pane_full(
                pane,
                area.width.max(2) / 2,
                area.height.max(2) / 2,
                Some(crate::agents::hold_on_failure(&format!("claude --resume {id}"))),
                env,
                Some(cwd),
            );
            // Bind the conversation now (the hook re-reports it in a moment)
            // so its stored name shows immediately.
            rt.agent_sessions.insert(pane, format!("claude:{id}"));
            if let Some(name) = stored_name {
                rt.state.rename_pane(pane, name);
            }
            spawned
        }
        MenuAction::ProfileBrowser => {
            let mut items: Vec<MenuItem> = crate::profile::list()
                .into_iter()
                .map(|name| MenuItem {
                    label: name.clone(),
                    action: MenuAction::ProfileMenu(name),
                })
                .collect();
            items.push(MenuItem {
                label: "+ new global profile...".to_string(),
                action: MenuAction::ProfileNew(None),
            });
            let ws_cwd = rt.state.workspaces.get(rt.state.active_workspace).map(|w| w.cwd.clone());
            items.push(MenuItem {
                label: "+ new profile for this space...".to_string(),
                action: MenuAction::ProfileNew(ws_cwd),
            });
            items.push(MenuItem {
                label: "(open folder in editor)".to_string(),
                action: MenuAction::EditProfiles,
            });
            rt.state.input_mode = InputMode::Menu { x, y, items };
            Ok(())
        }
        MenuAction::ProfileMenu(name) => {
            let items = vec![
                MenuItem {
                    label: format!("start {name}"),
                    action: MenuAction::StartProfile(name.clone(), None),
                },
                MenuItem {
                    label: "show command".to_string(),
                    action: MenuAction::ProfileInfo(name.clone()),
                },
                MenuItem {
                    label: "edit role (agent.md)".to_string(),
                    action: MenuAction::ProfileEdit(name.clone(), "agent.md"),
                },
                MenuItem {
                    label: "edit config (profile.toml)".to_string(),
                    action: MenuAction::ProfileEdit(name.clone(), "profile.toml"),
                },
                MenuItem {
                    label: "skills...".to_string(),
                    action: MenuAction::ProfileSkills(name),
                },
            ];
            rt.state.input_mode = InputMode::Menu { x, y, items };
            Ok(())
        }
        MenuAction::ProfileSkills(name) => {
            match crate::profile::load(&name) {
                Ok(p) => {
                    let assigned = &p.toml.skills;
                    let items: Vec<MenuItem> = crate::profile::skill_catalog()
                        .into_keys()
                        .map(|skill| MenuItem {
                            label: if assigned.contains(&skill) {
                                format!("{skill} ✓")
                            } else {
                                skill.clone()
                            },
                            action: MenuAction::ToggleProfileSkill(name.clone(), skill),
                        })
                        .collect();
                    if items.is_empty() {
                        rt.add_plain_toast("no skills in the catalog yet".to_string(), 8);
                    } else {
                        rt.state.input_mode = InputMode::Menu { x, y, items };
                    }
                }
                Err(e) => rt.add_plain_toast(format!("profile: {e}"), 10),
            }
            Ok(())
        }
        MenuAction::ToggleProfileSkill(name, skill) => {
            match crate::profile::load(&name) {
                Ok(p) => {
                    let mut skills = p.toml.skills.clone();
                    match skills.iter().position(|s| s == &skill) {
                        Some(i) => {
                            skills.remove(i);
                        }
                        None => skills.push(skill),
                    }
                    if let Err(e) = crate::profile::set_skills(&p.dir, &skills) {
                        rt.add_plain_toast(format!("skills: {e}"), 10);
                    }
                    // Reopen with the fresh checkmarks — toggling several
                    // skills in a row shouldn't need re-navigating.
                    return run_menu_action(rt, MenuAction::ProfileSkills(name), x, y, area);
                }
                Err(e) => rt.add_plain_toast(format!("profile: {e}"), 10),
            }
            Ok(())
        }
        MenuAction::ProfileEdit(name, file) => {
            match crate::profile::profiles_dir() {
                Some(dir) => rt.open_in_editor(&dir.join(&name).join(file), area),
                None => Ok(()),
            }
        }
        MenuAction::SkillNew => {
            rt.state.input_mode =
                InputMode::Prompt { kind: crate::state::PromptKind::NewSkill, buffer: String::new() };
            Ok(())
        }
        MenuAction::ProfileNew(scope) => {
            rt.state.input_mode = InputMode::Prompt {
                kind: crate::state::PromptKind::NewProfile(scope),
                buffer: String::new(),
            };
            Ok(())
        }
        MenuAction::AgentOptions(pane) => {
            let behavior = rt.panes.get(&pane).and_then(|p| p.behavior.clone());
            let items = vec![
                MenuItem {
                    label: match behavior {
                        Some(b) => format!("behavior: {}...", b.split_once(':').map_or(b.as_str(), |(_, n)| n)),
                        None => "attach behavior...".to_string(),
                    },
                    action: MenuAction::BehaviorPicker(pane),
                },
                MenuItem {
                    label: "rename…".to_string(),
                    action: MenuAction::RenamePane(pane),
                },
                MenuItem { label: "focus".to_string(), action: MenuAction::FocusPane(pane) },
                MenuItem { label: "close pane".to_string(), action: MenuAction::ClosePane(pane) },
            ];
            rt.state.input_mode = InputMode::Menu { x, y, items };
            Ok(())
        }
        MenuAction::RenameTab(tab) => {
            // Seed with the custom name; an auto-named tab ("3") starts empty.
            let buffer = rt
                .state
                .active_workspace()
                .tabs
                .iter()
                .find(|t| t.id == tab)
                .map(|t| t.name.clone())
                .filter(|n| !n.chars().all(|c| c.is_ascii_digit()))
                .unwrap_or_default();
            rt.state.input_mode =
                InputMode::Prompt { kind: crate::state::PromptKind::RenameTab(tab), buffer };
            Ok(())
        }
        MenuAction::CloseTab(tab) => {
            let panes = rt
                .state
                .active_workspace()
                .tabs
                .iter()
                .find(|t| t.id == tab)
                .map(|t| t.layout.panes())
                .unwrap_or_default();
            for pane in panes {
                rt.kill_pane(pane); // PtyExit drives the close (one path)
            }
            Ok(())
        }
        MenuAction::NewTab => {
            let pane = rt.state.new_tab();
            rt.spawn_pane(pane, area.width.max(4), area.height.max(4))
        }
        MenuAction::RenamePane(pane) => {
            // Seed with the current name so editing beats retyping; empty
            // submit clears back to the agent's own title.
            let buffer = rt.state.pane_name(pane).unwrap_or_default().to_string();
            rt.state.input_mode = InputMode::Prompt {
                kind: crate::state::PromptKind::RenamePane(pane),
                buffer,
            };
            Ok(())
        }
        MenuAction::FocusPane(pane) => {
            rt.state.focus_pane(pane);
            Ok(())
        }
        MenuAction::BehaviorPicker(pane) => {
            // Space-scoped behaviors first (this pane's workspace metadata),
            // then the global ones.
            let ws_cwd = rt
                .state
                .locate_pane(pane)
                .and_then(|(wi, _)| rt.state.workspaces.get(wi))
                .map(|w| w.cwd.clone());
            let mut items: Vec<MenuItem> = Vec::new();
            if let Some(cwd) = &ws_cwd {
                items.extend(crate::profile::list_ws(cwd).into_iter().map(|n| MenuItem {
                    label: format!("{n} (space)"),
                    action: MenuAction::SetBehavior(pane, Some(format!("ws:{n}"))),
                }));
            }
            items.extend(crate::profile::list().into_iter().map(|n| MenuItem {
                label: format!("{n} (global)"),
                action: MenuAction::SetBehavior(pane, Some(format!("global:{n}"))),
            }));
            items.push(MenuItem {
                label: "+ new for this space...".to_string(),
                action: MenuAction::ProfileNew(ws_cwd),
            });
            items.push(MenuItem {
                label: "+ new global...".to_string(),
                action: MenuAction::ProfileNew(None),
            });
            items.push(MenuItem {
                label: "(clear)".to_string(),
                action: MenuAction::SetBehavior(pane, None),
            });
            rt.state.input_mode = InputMode::Menu { x, y, items };
            Ok(())
        }
        MenuAction::SetBehavior(pane, ident) => {
            let label = ident.clone();
            match rt.apply_behavior(pane, ident) {
                Ok(()) => {
                    if let Some(l) = label {
                        rt.add_plain_toast(format!("behavior {l} → %{}", pane.0), 8);
                    }
                }
                Err(e) => rt.add_plain_toast(format!("behavior: {e}"), 10),
            }
            Ok(())
        }
        MenuAction::SkillEdit(source) => {
            let editor = rt.cfg.terminal.editor_cmd();
            let pane = rt.state.new_tab();
            rt.spawn_pane_cmd(pane, area.width, area.height, Some(format!("{editor} {source}")))
        }
        MenuAction::ProfileInfo(name) => {
            match crate::profile::load(&name) {
                Ok(p) => {
                    let (command, _env) = p.resolve();
                    rt.add_plain_toast(format!("{name}: {command}"), 10);
                }
                Err(e) => rt.add_plain_toast(format!("{name}: {e}"), 10),
            }
            Ok(())
        }
        MenuAction::SkillBrowser => {
            // Read-only catalog: picking a skill opens its source in $EDITOR.
            let items: Vec<MenuItem> = crate::profile::skill_catalog()
                .into_iter()
                .map(|(name, entry)| MenuItem {
                    label: if entry.description.is_empty() {
                        name.clone()
                    } else {
                        format!("{name} — {}", crate::agents::truncate_clean(&entry.description, 32))
                    },
                    action: MenuAction::SkillEdit(entry.source),
                })
                .collect();
            let mut items = items;
            items.push(MenuItem { label: "+ new skill...".to_string(), action: MenuAction::SkillNew });
            rt.state.input_mode = InputMode::Menu { x, y, items };
            Ok(())
        }
        MenuAction::EditProfiles => {
            match crate::profile::profiles_dir() {
                Some(dir) => {
                    let _ = std::fs::create_dir_all(&dir);
                    let editor = rt.cfg.terminal.editor_cmd();
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
        MenuAction::EditorPicker => {
            let current = rt.cfg.terminal.editor_cmd();
            let items: Vec<MenuItem> = ["nano", "vim", "vi", "hx", "micro", "code --wait"]
                .into_iter()
                .map(|e| MenuItem {
                    label: if e == current { format!("{e} ✓") } else { e.to_string() },
                    action: MenuAction::SetEditor(e.to_string()),
                })
                .collect();
            rt.state.input_mode = InputMode::Menu { x, y, items };
            Ok(())
        }
        MenuAction::SetEditor(editor) => {
            match crate::config::set_editor(&editor) {
                Ok(()) => {
                    rt.cfg.terminal.editor = editor.clone();
                    rt.add_plain_toast(format!("editor → {editor}"), 6);
                }
                Err(e) => rt.add_plain_toast(format!("editor: {e}"), 10),
            }
            Ok(())
        }
        MenuAction::ShowKeybinds => {
            rt.state.input_mode = InputMode::Help;
            Ok(())
        }
        MenuAction::ReloadConfig => {
            rt.reload_config();
            Ok(())
        }
        MenuAction::RunUpdate => {
            // A visible tab: the user watches the download, then the server
            // execs the new binary in place and the client reconnects.
            let bin = std::env::current_exe()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| "cdock".to_string());
            let pane = rt.state.new_tab();
            rt.spawn_pane_cmd(
                pane,
                area.width,
                area.height,
                // A failed update (offline, rate limit) must leave the error
                // readable in a shell, not vanish the tab within a frame.
                Some(crate::agents::hold_on_failure(&format!("'{bin}' update --handoff"))),
            )
        }
        MenuAction::Detach => return InputOutcome::Detach,
        MenuAction::ListWorktrees(ws_id) => {
            let Some(ws) = rt.state.workspace_index(ws_id).map(|wi| &rt.state.workspaces[wi])
            else {
                return InputOutcome::Continue;
            };
            let current = ws.cwd.clone();
            let items: Vec<MenuItem> = crate::git::worktrees(&ws.cwd)
                .into_iter()
                .filter(|(path, _)| *path != current)
                .map(|(path, branch)| MenuItem {
                    label: branch,
                    action: MenuAction::OpenWorktree(ws_id, path),
                })
                .collect();
            if items.is_empty() {
                tracing::info!("no other worktrees to open");
            } else {
                rt.state.input_mode = InputMode::Menu { x, y, items };
            }
            Ok(())
        }
        MenuAction::OpenWorktree(ws_id, path) => {
            if rt.state.workspace_index(ws_id).is_some() {
                rt.open_worktree(ws_id, path, area, true);
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

/// The pane's TermMode, if the app in it requested mouse reporting.
fn mouse_mode(rt: &Runtime, id: PaneId) -> Option<TermMode> {
    rt.panes.get(&id).filter(|p| p.emu.wants_mouse()).map(|p| *p.emu.term.mode())
}

/// Encode a mouse event honoring the pane's requested protocol:
/// SGR-1006 when SGR_MOUSE is set, legacy X10 bytes otherwise.
fn encode_mouse(
    mode: TermMode,
    button: u8,
    col: u16,
    row: u16,
    press: bool,
    mods: KeyModifiers,
) -> Vec<u8> {
    if mode.contains(TermMode::SGR_MOUSE) {
        sgr(button, col, row, press, mods)
    } else {
        x10(button, col, row, press, mods)
    }
}

/// Shift(4)/Alt(8)/Ctrl(16) button bits shared by SGR and X10.
fn button_mods(mods: KeyModifiers) -> u8 {
    (mods.contains(KeyModifiers::SHIFT) as u8) * 4
        + (mods.contains(KeyModifiers::ALT) as u8) * 8
        + (mods.contains(KeyModifiers::CONTROL) as u8) * 16
}

/// SGR (1006) mouse encoding with pane-local 1-based coordinates.
fn sgr(button: u8, col: u16, row: u16, press: bool, mods: KeyModifiers) -> Vec<u8> {
    let b = button + button_mods(mods);
    format!("\x1b[<{};{};{}{}", b, col + 1, row + 1, if press { 'M' } else { 'm' }).into_bytes()
}

/// Legacy X10-style encoding: `ESC [ M cb cx cy`, single 32-offset bytes.
/// The protocol has no per-button release — releases collapse to cb=3 —
/// and 1-based coordinates clamp at 223 (byte 255); apps needing more
/// negotiate SGR.
fn x10(button: u8, col: u16, row: u16, press: bool, mods: KeyModifiers) -> Vec<u8> {
    let cb = (if press { button } else { 3 }) + button_mods(mods);
    vec![
        0x1b,
        b'[',
        b'M',
        32 + cb,
        32 + 1 + col.min(222) as u8,
        32 + 1 + row.min(222) as u8,
    ]
}

fn send_mouse(rt: &mut Runtime, pane: PaneId, bytes: Vec<u8>) {
    if let Some(p) = rt.panes.get_mut(&pane) {
        p.pty.write(&bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn x10_byte_layout() {
        // Left press at pane cell (0,0) → cb=32, cx=cy=33.
        assert_eq!(x10(0, 0, 0, true, KeyModifiers::NONE), b"\x1b[M\x20\x21\x21".to_vec());
        // Release collapses to cb=3; ctrl adds 16.
        assert_eq!(
            x10(0, 4, 9, false, KeyModifiers::CONTROL),
            vec![0x1b, b'[', b'M', 32 + 3 + 16, 32 + 5, 32 + 10]
        );
        // Coordinates clamp at byte 255 (0-based cell 222).
        assert_eq!(x10(0, 500, 222, true, KeyModifiers::NONE)[4..], [255, 255]);
    }

    #[test]
    fn encode_mouse_dispatches_on_sgr_flag() {
        let sgr_mode = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        assert_eq!(
            encode_mouse(sgr_mode, 0, 4, 9, true, KeyModifiers::NONE),
            b"\x1b[<0;5;10M".to_vec()
        );
        // Mouse reporting without SGR falls back to X10 bytes.
        assert_eq!(
            encode_mouse(TermMode::MOUSE_REPORT_CLICK, 0, 4, 9, true, KeyModifiers::NONE),
            b"\x1b[M\x20\x25\x2a".to_vec()
        );
    }
}
