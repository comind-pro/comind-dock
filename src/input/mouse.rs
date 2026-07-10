//! Mouse handling: hit testing against the last computed View.
//! Arbitration rule: chrome (tab bar, sidebar) and dividers always win;
//! inside a pane, an app that requested mouse reporting gets encoded events,
//! otherwise the multiplexer owns selection and wheel scrollback.

use std::time::{Duration, Instant};

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};

use crate::runtime::{MouseDrag, Runtime, osc52_copy};
use crate::state::InputMode;
use crate::state::ids::PaneId;
use crate::state::layout::Dir;
use crate::ui::menu::MenuAction;
use crate::ui::{menu, sidebar, tabbar};

const DOUBLE_CLICK: Duration = Duration::from_millis(400);

/// Handle a mouse event. Returns true to quit the app (✕ button).
pub fn handle(rt: &mut Runtime, ev: MouseEvent, area: Rect) -> bool {
    let Some(view) = rt.last_view.clone() else { return false };
    let pos = Position::new(ev.column, ev.row);
    let scroll_lines = rt.cfg.ui.mouse_scroll_lines.max(1) as i32;

    // An open context menu captures the next click.
    if let InputMode::Menu { pane, x, y } = rt.state.input_mode.clone() {
        rt.state.input_mode = InputMode::Terminal;
        rt.mark_dirty();
        if let MouseEventKind::Down(MouseButton::Left) = ev.kind {
            let mrect = menu::rect(x, y, area);
            if let Some(action) = menu::hit(mrect, ev.column, ev.row) {
                rt.state.focus_pane(pane);
                let result = match action {
                    MenuAction::SplitRight => rt.split_focused(Dir::Right, false, area),
                    MenuAction::SplitLeft => rt.split_focused(Dir::Right, true, area),
                    MenuAction::SplitDown => rt.split_focused(Dir::Down, false, area),
                    MenuAction::SplitUp => rt.split_focused(Dir::Down, true, area),
                    MenuAction::ClosePane => {
                        rt.kill_pane(pane);
                        Ok(())
                    }
                };
                if let Err(e) = result {
                    tracing::warn!(error = %e, "menu action failed");
                }
            }
        }
        return false;
    }

    match ev.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            rt.mark_dirty();

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
                    Some(tabbar::Hit::CloseApp) => {
                        crate::state::snapshot::save(&rt.state);
                        return true;
                    }
                    None => {}
                }
                return false;
            }
            if let Some(sb) = view.sidebar
                && sb.contains(pos)
            {
                let theme = rt.theme;
                match sidebar::hit(rt, &theme, ev.row - sb.y) {
                    Some(sidebar::Target::Workspace(wi)) => rt.state.active_workspace = wi,
                    Some(sidebar::Target::Pane(pane)) => {
                        rt.state.focus_pane(pane);
                    }
                    Some(sidebar::Target::NewWorkspace) => {
                        let pane = rt.state.new_workspace();
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
                return false;
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
                return false;
            }
            if let Some((id, rect)) = pane_at(&view.pane_rects, pos) {
                rt.state.active_tab_mut().focused_pane = id;
                let inner = crate::ui::content_rect(rect);
                if !inner.contains(pos) {
                    return false; // border click: focus only
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
                    if let Some(p) = rt.panes.get(&pane)
                        && let Some(text) = p.emu.selection_text()
                            && !text.is_empty() {
                                osc52_copy(&text);
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
            if let Some((id, rect)) = pane_at(&view.pane_rects, pos) {
                let inner = crate::ui::content_rect(rect);
                if !inner.contains(pos) {
                    return false;
                }
                let (col, row) = (ev.column - inner.x, ev.row - inner.y);
                let Some(p) = rt.panes.get_mut(&id) else { return false };
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
            // Right-click on a pane opens the context menu.
            if let Some((id, _)) = pane_at(&view.pane_rects, pos) {
                rt.state.active_tab_mut().focused_pane = id;
                rt.state.input_mode = InputMode::Menu { pane: id, x: ev.column, y: ev.row };
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
    false
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
