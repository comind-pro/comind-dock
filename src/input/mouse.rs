//! Mouse handling: hit testing against the last computed View.
//! Arbitration rule: chrome (tab bar, sidebar) and dividers always win;
//! inside a pane, an app that requested mouse reporting gets encoded events,
//! otherwise the multiplexer owns selection and wheel scrollback.

use std::time::{Duration, Instant};

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};

use crate::runtime::{MouseDrag, Runtime, osc52_copy};
use crate::state::ids::PaneId;
use crate::state::layout::Dir;
use crate::ui::{sidebar, tabbar};

const DOUBLE_CLICK: Duration = Duration::from_millis(400);
const SCROLL_LINES: i32 = 3; // ponytail: [ui].mouse_scroll_lines in M6

pub fn handle(rt: &mut Runtime, ev: MouseEvent) {
    let Some(view) = rt.last_view.clone() else { return };
    let pos = Position::new(ev.column, ev.row);

    match ev.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            rt.mark_dirty();

            if view.tab_bar.contains(pos) {
                if let Some(ti) = tabbar::hit(&rt.state, ev.column - view.tab_bar.x) {
                    rt.state.jump_tab(ti);
                }
                return;
            }
            if let Some(sb) = view.sidebar {
                if sb.contains(pos) {
                    match sidebar::hit(&rt.state, ev.row - sb.y) {
                        Some(sidebar::Target::Workspace(wi)) => rt.state.active_workspace = wi,
                        Some(sidebar::Target::Tab(wi, ti)) => {
                            rt.state.active_workspace = wi;
                            rt.state.jump_tab(ti);
                        }
                        None => {}
                    }
                    return;
                }
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
                return;
            }
            if let Some((id, rect)) = pane_at(&view.pane_rects, pos) {
                rt.state.active_tab_mut().focused_pane = id;
                let (col, row) = (ev.column - rect.x, ev.row - rect.y);
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
                if let Some((_, rect)) =
                    view.pane_rects.iter().find(|(id, _)| *id == pane).copied()
                {
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
                    if rt.panes.get(&id).is_some_and(|p| p.emu.wants_mouse()) {
                        let bytes =
                            sgr(32, ev.column - rect.x, ev.row - rect.y, true, ev.modifiers);
                        send_mouse(rt, id, bytes);
                    }
                }
            }
        },

        MouseEventKind::Up(MouseButton::Left) => {
            match rt.drag.take() {
                Some(MouseDrag::Select { pane }) => {
                    if let Some(p) = rt.panes.get(&pane) {
                        if let Some(text) = p.emu.selection_text() {
                            if !text.is_empty() {
                                osc52_copy(&text);
                            }
                        }
                    }
                }
                Some(MouseDrag::Divider { .. }) => {}
                None => {
                    if let Some((id, rect)) = pane_at(&view.pane_rects, pos) {
                        if rt.panes.get(&id).is_some_and(|p| p.emu.wants_mouse()) {
                            let bytes =
                                sgr(0, ev.column - rect.x, ev.row - rect.y, false, ev.modifiers);
                            send_mouse(rt, id, bytes);
                        }
                    }
                }
            }
        }

        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
            let up = ev.kind == MouseEventKind::ScrollUp;
            if let Some((id, rect)) = pane_at(&view.pane_rects, pos) {
                let (col, row) = (ev.column - rect.x, ev.row - rect.y);
                let Some(p) = rt.panes.get_mut(&id) else { return };
                if p.emu.wants_mouse() {
                    let bytes = sgr(if up { 64 } else { 65 }, col, row, true, ev.modifiers);
                    send_mouse(rt, id, bytes);
                } else if p.emu.alternate_scroll() {
                    let arrow: &[u8] = if up { b"\x1bOA" } else { b"\x1bOB" };
                    let bytes = arrow.repeat(SCROLL_LINES as usize);
                    p.pty.write(&bytes);
                } else {
                    p.emu.scroll_display(if up { SCROLL_LINES } else { -SCROLL_LINES });
                    rt.mark_dirty();
                }
            }
        }

        MouseEventKind::Down(MouseButton::Right | MouseButton::Middle) => {
            if let Some((id, rect)) = pane_at(&view.pane_rects, pos) {
                if rt.panes.get(&id).is_some_and(|p| p.emu.wants_mouse()) {
                    let btn = if matches!(ev.kind, MouseEventKind::Down(MouseButton::Middle)) {
                        1
                    } else {
                        2
                    };
                    let bytes = sgr(btn, ev.column - rect.x, ev.row - rect.y, true, ev.modifiers);
                    send_mouse(rt, id, bytes);
                }
            }
        }

        _ => {}
    }
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
