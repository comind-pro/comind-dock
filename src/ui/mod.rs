pub mod pane_widget;

use std::collections::HashMap;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};

use crate::runtime::PaneRuntime;
use crate::state::ids::PaneId;
use crate::state::layout::{Dir, Divider};

/// Draw all panes of the active tab plus their dividers. Read-only:
/// geometry and PTY resizes already happened in compute_view.
pub fn render_panes(
    frame: &mut Frame,
    rects: &[(PaneId, Rect)],
    dividers: &[Divider],
    panes: &HashMap<PaneId, PaneRuntime>,
    focused: PaneId,
) {
    for (id, rect) in rects {
        if let Some(p) = panes.get(id) {
            pane_widget::render(&p.emu.term, *rect, frame, *id == focused);
        }
    }

    let buf = frame.buffer_mut();
    for d in dividers {
        let symbol = if d.dir == Dir::Right { "│" } else { "─" };
        let style = Style::new().fg(Color::DarkGray);
        for y in d.rect.y..d.rect.y + d.rect.height {
            for x in d.rect.x..d.rect.x + d.rect.width {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_symbol(symbol);
                    cell.set_style(style);
                }
            }
        }
    }
}
