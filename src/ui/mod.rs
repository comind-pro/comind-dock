pub mod help;
pub mod menu;
pub mod pane_widget;
pub mod sidebar;
pub mod tabbar;
pub mod toast;
pub mod view;

use ratatui::Frame;
use ratatui::layout::Rect;
use crate::runtime::Runtime;
use view::View;

/// Phase 1 of the frame: all geometry and mutation. Splits the screen into
/// tab bar / sidebar / content, computes pane rects, and propagates size
/// changes to emulators and PTYs. Render never mutates.
pub fn compute_view(rt: &mut Runtime, area: Rect) -> View {
    // Sidebar runs the full height; the tab bar sits over the content column only.
    let sb_width = rt.cfg.ui.sidebar_width.clamp(10, area.width / 2);
    let (sidebar, content_col) = if rt.state.sidebar_visible && area.width > sb_width * 2 {
        let sb = Rect { width: sb_width, ..area };
        let col = Rect { x: area.x + sb_width, width: area.width - sb_width, ..area };
        (Some(sb), col)
    } else {
        (None, area)
    };

    let hide_bar = rt.cfg.ui.hide_tab_bar_when_single_tab
        && rt.state.active_workspace().tabs.len() == 1;
    let bar_height = if hide_bar { 0 } else { 1.min(content_col.height) };
    let tab_bar = Rect { height: bar_height, ..content_col };
    let content = Rect {
        y: content_col.y + bar_height,
        height: content_col.height.saturating_sub(bar_height),
        ..content_col
    };

    let (pane_rects, dividers) = rt.compute_panes(content);
    let view = View { tab_bar, sidebar, pane_rects, dividers, focused: rt.state.focused_pane() };
    rt.last_view = Some(view.clone());
    view
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
