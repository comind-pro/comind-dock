pub mod help;
pub mod pane_widget;
pub mod sidebar;
pub mod tabbar;
pub mod view;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};

use crate::runtime::Runtime;
use crate::state::layout::Dir;
use view::View;

const SIDEBAR_WIDTH: u16 = 24; // ponytail: [ui].sidebar_width in M6

/// Phase 1 of the frame: all geometry and mutation. Splits the screen into
/// tab bar / sidebar / content, computes pane rects, and propagates size
/// changes to emulators and PTYs. Render never mutates.
pub fn compute_view(rt: &mut Runtime, area: Rect) -> View {
    let tab_bar = Rect { height: 1.min(area.height), ..area };
    let below = Rect {
        y: area.y + tab_bar.height,
        height: area.height.saturating_sub(tab_bar.height),
        ..area
    };
    let (sidebar, content) = if rt.state.sidebar_visible && below.width > SIDEBAR_WIDTH * 2 {
        let sb = Rect { width: SIDEBAR_WIDTH, ..below };
        let content = Rect {
            x: below.x + SIDEBAR_WIDTH,
            width: below.width - SIDEBAR_WIDTH,
            ..below
        };
        (Some(sb), content)
    } else {
        (None, below)
    };

    let (pane_rects, dividers) = rt.compute_panes(content);
    let view = View { tab_bar, sidebar, pane_rects, dividers, focused: rt.state.focused_pane() };
    rt.last_pane_rects = view.pane_rects.clone();
    view
}

/// Phase 2: pure drawing from the precomputed view and immutable state.
pub fn render(view: &View, rt: &Runtime, frame: &mut Frame) {
    tabbar::render(&rt.state, view.tab_bar, frame);
    if let Some(sb) = view.sidebar {
        sidebar::render(&rt.state, sb, frame);
    }

    for (id, rect) in &view.pane_rects {
        if let Some(p) = rt.panes.get(id) {
            pane_widget::render(&p.emu.term, *rect, frame, *id == view.focused);
        }
    }

    let focused_rect =
        view.pane_rects.iter().find(|(id, _)| *id == view.focused).map(|(_, r)| *r);
    let full = frame.area();
    let buf = frame.buffer_mut();
    for d in &view.dividers {
        let symbol = if d.dir == Dir::Right { "│" } else { "─" };
        let accent = focused_rect.is_some_and(|fr| touches(d.rect, fr));
        let style = if accent {
            Style::new().fg(Color::Cyan)
        } else {
            Style::new().fg(Color::DarkGray)
        };
        for y in d.rect.y..d.rect.y + d.rect.height {
            for x in d.rect.x..d.rect.x + d.rect.width {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_symbol(symbol);
                    cell.set_style(style);
                }
            }
        }
    }

    // Mode overlays on top of everything.
    let mode = rt.state.input_mode.clone();
    help::render_hint(&mode, full, frame);
    match &mode {
        crate::state::InputMode::Help => help::render_help(full, frame),
        crate::state::InputMode::Prompt { kind, buffer } => {
            help::render_prompt(*kind, buffer, full, frame);
        }
        _ => {}
    }
}

/// Divider is adjacent to (touches an edge of) the given pane rect.
fn touches(d: Rect, r: Rect) -> bool {
    let horiz_overlap = d.x < r.x + r.width && r.x < d.x + d.width;
    let vert_overlap = d.y < r.y + r.height && r.y < d.y + d.height;
    (d.x + d.width == r.x || r.x + r.width == d.x) && vert_overlap
        || (d.y + d.height == r.y || r.y + r.height == d.y) && horiz_overlap
}
