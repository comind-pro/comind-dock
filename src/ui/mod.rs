pub mod help;
pub mod menu;
pub mod pane_widget;
pub mod sidebar;
pub mod tabbar;
pub mod toast;
pub mod view;

use crate::runtime::Runtime;
use ratatui::Frame;
use ratatui::layout::Rect;
use view::View;

/// Phase 1 of the frame: all geometry and mutation. Splits the screen into
/// tab bar / sidebar / content, computes pane rects, and propagates size
/// changes to emulators and PTYs. Render never mutates.
pub fn compute_view(rt: &Runtime, area: Rect) -> View {
    // Sidebar runs the full height; the tab bar sits over the content column only.
    let sb_width = rt.cfg.ui.sidebar_width.clamp(10, area.width / 2);
    let (sidebar, content_col) = if rt.state.sidebar_visible && area.width > sb_width * 2 {
        let sb = Rect { width: sb_width, ..area };
        let col = Rect { x: area.x + sb_width, width: area.width - sb_width, ..area };
        (Some(sb), col)
    } else {
        (None, area)
    };

    let hide_bar =
        rt.cfg.ui.hide_tab_bar_when_single_tab && rt.state.active_workspace().tabs.len() == 1;
    let bar_height = if hide_bar { 0 } else { 1.min(content_col.height) };
    let tab_bar = Rect { height: bar_height, ..content_col };
    let content = Rect {
        y: content_col.y + bar_height,
        height: content_col.height.saturating_sub(bar_height),
        ..content_col
    };

    let (pane_rects, dividers) = rt.layout_panes(content);
    View { tab_bar, sidebar, pane_rects, dividers, focused: rt.state.focused_pane() }
}

/// The pty size a pane would take in this view (content rect minus chrome).
pub fn pane_sizes(view: &View) -> Vec<(crate::state::ids::PaneId, (u16, u16))> {
    view.pane_rects
        .iter()
        .map(|(id, rect)| {
            let inner = content_rect(*rect);
            (*id, (inner.width, inner.height))
        })
        .collect()
}

/// Phase 2: pure drawing from the precomputed view and immutable state.
pub fn render(view: &View, rt: &Runtime, frame: &mut Frame) {
    if view.tab_bar.height > 0 {
        tabbar::render(rt, &rt.theme, view.tab_bar, frame);
    }
    if let Some(sb) = view.sidebar {
        sidebar::render(rt, &rt.theme, sb, frame);
    }

    // Each pane draws its own rounded border; the divider gap stays empty,
    // so adjacent panes read as separate cards (mockup look).
    for (id, rect) in &view.pane_rects {
        if let Some(p) = rt.panes.get(id) {
            let title = rt
                .titles
                .get(id)
                .filter(|t| !t.trim().is_empty())
                .cloned()
                .unwrap_or_else(|| p.program.clone());
            pane_widget::render(
                &p.emu.term,
                *rect,
                frame,
                *id == view.focused,
                &title,
                &rt.theme,
                if *id == view.focused { p.emu.focused_match() } else { None },
            );
        }
    }
    let full = frame.area();
    toast::render(rt, full, frame);

    // Drag-drop hover highlight: above panes, below modals.
    if let Some(
        crate::runtime::MouseDrag::Tab { hover: Some(target), .. }
        | crate::runtime::MouseDrag::Pane { hover: Some(target), .. },
    ) = rt.drag
    {
        render_drop_highlight(view, rt, target, frame);
    }

    // Mode overlays on top of everything.
    let mode = rt.state.input_mode.clone();
    help::render_hint(&mode, &rt.theme, full, frame);
    match &mode {
        crate::state::InputMode::Help => help::render_help(&rt.keymap, &rt.theme, full, frame),
        crate::state::InputMode::Prompt { kind, buffer } => {
            help::render_prompt(kind, buffer, &rt.theme, full, frame);
        }
        crate::state::InputMode::Search { buffer } => {
            help::render_search(buffer, &rt.theme, full, frame);
        }
        crate::state::InputMode::Menu { x, y, items } => {
            menu::render(*x, *y, items, &rt.theme, full, frame);
        }
        _ => {}
    }
}

/// Host-cursor position for the focused pane (server-side renderer).
pub fn cursor_for(view: &View, rt: &Runtime) -> Option<(u16, u16)> {
    if !matches!(rt.state.input_mode, crate::state::InputMode::Terminal) {
        return None;
    }
    let (_, rect) = view.pane_rects.iter().find(|(id, _)| *id == view.focused)?;
    let p = rt.panes.get(&view.focused)?;
    pane_widget::cursor_position(&p.emu.term, content_rect(*rect))
}

/// The terminal-content area inside a pane's border frame.
pub fn content_rect(rect: Rect) -> Rect {
    Rect {
        x: rect.x + 1,
        y: rect.y + 1,
        width: rect.width.saturating_sub(2),
        height: rect.height.saturating_sub(2),
    }
}

/// Drop zone under `pos` inside `rect`: the middle 50%×50% box is Center,
/// otherwise the nearest edge (normalized distance, ties prefer horizontal).
pub fn zone_at(rect: Rect, pos: ratatui::layout::Position) -> crate::runtime::Zone {
    use crate::runtime::Zone;
    let rx = (pos.x.saturating_sub(rect.x)) as f32 / rect.width.max(1) as f32;
    let ry = (pos.y.saturating_sub(rect.y)) as f32 / rect.height.max(1) as f32;
    if (0.25..0.75).contains(&rx) && (0.25..0.75).contains(&ry) {
        return Zone::Center;
    }
    let (dx, hz) = if rx < 0.5 { (rx, Zone::Left) } else { (1.0 - rx, Zone::Right) };
    let (dy, vz) = if ry < 0.5 { (ry, Zone::Up) } else { (1.0 - ry, Zone::Down) };
    if dx <= dy { hz } else { vz }
}

/// The half of `rect` a zone highlights (Center → the whole rect).
pub fn zone_rect(rect: Rect, zone: crate::runtime::Zone) -> Rect {
    use crate::runtime::Zone;
    let (hw, hh) = (rect.width / 2, rect.height / 2);
    match zone {
        Zone::Left => Rect { width: hw, ..rect },
        Zone::Right => Rect { x: rect.x + hw, width: rect.width - hw, ..rect },
        Zone::Up => Rect { height: hh, ..rect },
        Zone::Down => Rect { y: rect.y + hh, height: rect.height - hh, ..rect },
        Zone::Center => rect,
    }
}

/// Paint the current drop target: an accent-tinted half-pane for edge zones,
/// an accent border for a center swap, reverse-video for a tab-bar segment.
/// set_style keeps the glyphs underneath readable.
fn render_drop_highlight(
    view: &View,
    rt: &Runtime,
    target: crate::runtime::DropTarget,
    frame: &mut Frame,
) {
    use crate::runtime::{DropTarget, Zone};
    use ratatui::style::{Modifier, Style};
    match target {
        DropTarget::Zone { pane, zone } => {
            let Some((_, r)) = view.pane_rects.iter().find(|(id, _)| *id == pane) else {
                return;
            };
            match zone {
                Zone::Center => {
                    let block = ratatui::widgets::Block::bordered()
                        .border_type(ratatui::widgets::BorderType::Rounded)
                        .border_style(Style::new().fg(rt.theme.accent));
                    frame.render_widget(block, *r);
                }
                z => {
                    let zr = zone_rect(*r, z);
                    frame.buffer_mut().set_style(zr, Style::new().bg(rt.theme.accent));
                }
            }
        }
        DropTarget::TabBar(td) => {
            if let Some(zr) = tabbar::drop_rect(rt, td, view.tab_bar) {
                frame.buffer_mut().set_style(zr, Style::new().add_modifier(Modifier::REVERSED));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;

    /// A pane two clients both display must end up at ONE pty size — the
    /// smallest, so neither viewer sees it cropped. (server::render_clients
    /// folds pane_sizes() across clients with exactly this min.)
    #[test]
    fn shared_pane_takes_the_smallest_viewers_size() {
        use crate::state::ids::PaneId;
        use crate::ui::view::View;
        use std::collections::HashMap;
        let view = |w: u16, h: u16| View {
            tab_bar: Rect::new(0, 0, w, 1),
            sidebar: None,
            pane_rects: vec![(PaneId(1), Rect::new(0, 1, w, h))],
            dividers: Vec::new(),
            focused: PaneId(1),
        };
        let mut wanted: HashMap<PaneId, (u16, u16)> = HashMap::new();
        for v in [view(120, 40), view(80, 24)] {
            for (pane, size) in pane_sizes(&v) {
                wanted
                    .entry(pane)
                    .and_modify(|s| *s = (s.0.min(size.0), s.1.min(size.1)))
                    .or_insert(size);
            }
        }
        let wide = pane_sizes(&view(120, 40))[0].1;
        let narrow = pane_sizes(&view(80, 24))[0].1;
        assert_eq!(wanted[&PaneId(1)], narrow, "the narrow client wins");
        assert!(narrow.0 < wide.0);
    }

    #[test]
    fn zone_at_center_and_edges() {
        use crate::runtime::Zone;
        use ratatui::layout::Position;
        let r = Rect::new(10, 10, 40, 20);
        assert_eq!(zone_at(r, Position::new(30, 20)), Zone::Center);
        assert_eq!(zone_at(r, Position::new(11, 20)), Zone::Left);
        assert_eq!(zone_at(r, Position::new(48, 20)), Zone::Right);
        assert_eq!(zone_at(r, Position::new(30, 10)), Zone::Up);
        assert_eq!(zone_at(r, Position::new(30, 29)), Zone::Down);
    }

    #[test]
    fn zone_rect_halves() {
        use crate::runtime::Zone;
        let r = Rect::new(0, 0, 41, 20);
        assert_eq!(zone_rect(r, Zone::Left), Rect::new(0, 0, 20, 20));
        assert_eq!(zone_rect(r, Zone::Right), Rect::new(20, 0, 21, 20));
        assert_eq!(zone_rect(r, Zone::Up), Rect::new(0, 0, 41, 10));
        assert_eq!(zone_rect(r, Zone::Down), Rect::new(0, 10, 41, 10));
        assert_eq!(zone_rect(r, Zone::Center), r);
    }
}
