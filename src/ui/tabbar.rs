use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::config::theme::Theme;
use crate::runtime::{Runtime, TabDrop};
use crate::state::AppState;

/// What a tab-bar click hits.
#[derive(Debug, Clone, Copy)]
pub enum Hit {
    Tab(usize),
    /// The ✕ inside a tab: close that tab.
    CloseTab(usize),
    NewTab,
    /// The ✕ at the right edge: quit cdock, saving the session.
    /// The ✕ at the right edge: asks whether to leave (agents keep running)
    /// or stop the dock. Never acts on its own — the two endings are far too
    /// different to hang on one unlabelled click.
    CloseApp,
    /// The ≡ at the left edge, shown only while the sidebar is hidden.
    ShowSidebar,
}

struct Segment {
    text: String,
    hit: Option<Hit>,
    active: bool,
}

/// A tab whose name was never changed from its numeric default shows the
/// focused pane's OSC title instead — tabs read as "what's running there".
fn tab_label(rt: &Runtime, state: &AppState, ti: usize) -> String {
    let ws = state.active_workspace();
    let tab = &ws.tabs[ti];
    if tab.name.chars().all(|c| c.is_ascii_digit()) {
        // Auto-named tab: the user's name for the pane wins, then the app's
        // own OSC title, then the program — same precedence as the sidebar.
        if let Some(name) = state.pane_name(tab.focused_pane) {
            return crate::agents::truncate_clean(name, 16);
        }
        if let Some(title) = rt.titles.get(&tab.focused_pane)
            && !title.trim().is_empty()
        {
            return crate::agents::truncate_clean(title, 16);
        }
        if let Some(p) = rt.panes.get(&tab.focused_pane) {
            return p.program.clone();
        }
    }
    tab.name.clone()
}

/// One source of truth for the bar — render draws it, hit() clicks it.
fn segments(rt: &Runtime) -> Vec<Segment> {
    let state = &rt.state;
    let ws = state.active_workspace();
    let mut out = if state.sidebar_visible {
        vec![Segment { text: " ".into(), hit: None, active: false }]
    } else {
        vec![Segment { text: " ≡  ".into(), hit: Some(Hit::ShowSidebar), active: false }]
    };
    for ti in 0..ws.tabs.len() {
        let active = ti == ws.active_tab;
        let zoomed = active && ws.tabs[ti].zoomed.is_some();
        out.push(Segment {
            text: format!("  {}{} ✕ ", tab_label(rt, state, ti), if zoomed { " [Z]" } else { "" }),
            hit: Some(Hit::Tab(ti)),
            active,
        });
        out.push(Segment { text: " ".into(), hit: None, active: false });
    }
    out.push(Segment { text: "  +  ".into(), hit: Some(Hit::NewTab), active: false });
    out
}

pub fn render(rt: &Runtime, theme: &Theme, area: Rect, frame: &mut Frame) {
    let mut spans: Vec<Span> = segments(rt)
        .into_iter()
        .map(|s| {
            let style = if s.active {
                Style::new().fg(Color::Black).bg(theme.accent)
            } else if matches!(s.hit, Some(Hit::NewTab)) {
                Style::new().fg(theme.accent)
            } else if s.hit.is_some() {
                Style::new().fg(theme.muted)
            } else {
                Style::new()
            };
            Span::styled(s.text, style)
        })
        .collect();
    // ✕ pinned to the right edge (quit + save session).
    use unicode_width::UnicodeWidthStr as _;
    let used: usize = spans.iter().map(|s| s.content.width()).sum();
    let total = area.width as usize;
    if used + CLOSE_WIDTH <= total {
        spans.push(Span::raw(" ".repeat(total - used - CLOSE_WIDTH)));
        spans.push(Span::styled(" ✕ ", Style::new().fg(theme.muted)));
    }
    let bar = Paragraph::new(Line::from(spans)).style(Style::new().bg(theme.tab_bar_bg));
    frame.render_widget(bar, area);
}

/// Display width of the " ✕ " button.
const CLOSE_WIDTH: usize = 3;

/// What sits under bar-relative column `x` (`width` = bar width).
pub fn hit(rt: &Runtime, x: u16, width: u16) -> Option<Hit> {
    // The buttons exist only when render actually drew them — on a full bar
    // the right edge shows a tab, and quitting the dock from a mis-hit there
    // would kill every agent.
    use unicode_width::UnicodeWidthStr as _;
    let used: usize = segments(rt).iter().map(|s| s.text.width()).sum();
    if used + CLOSE_WIDTH <= width as usize && x >= width.saturating_sub(CLOSE_WIDTH as u16) {
        return Some(Hit::CloseApp);
    }
    let mut cursor: u16 = 0;
    for s in segments(rt) {
        let w = s.text.width() as u16;
        if x >= cursor && x < cursor + w {
            // The trailing " ✕ " of a tab closes it.
            if let Some(Hit::Tab(ti)) = s.hit
                && x >= cursor + w.saturating_sub(3)
            {
                return Some(Hit::CloseTab(ti));
            }
            return s.hit;
        }
        cursor += w;
    }
    None
}

/// Screen rect of the segment matching `target` — same width-walk render()
/// and hit() use, so the highlight matches the click. Takes plain tabs
/// (just needs each tab's id) so it's testable without a `Runtime`.
fn segment_rect(
    segs: &[Segment],
    tabs: &[crate::state::workspace::Tab],
    target: TabDrop,
    bar: Rect,
) -> Option<Rect> {
    use unicode_width::UnicodeWidthStr as _;
    let mut cursor: u16 = 0;
    for s in segs {
        let w = s.text.width() as u16;
        let matched = match (target, s.hit) {
            (TabDrop::Tab(id), Some(Hit::Tab(ti))) => {
                tabs.get(ti).is_some_and(|t| t.id == id)
            }
            (TabDrop::NewTab, Some(Hit::NewTab)) => true,
            _ => false,
        };
        if matched {
            let x = bar.x + cursor.min(bar.width);
            let width = w.min(bar.width.saturating_sub(cursor));
            return Some(Rect { x, width, ..bar });
        }
        cursor += w;
    }
    None
}

/// Screen rect of the segment a pane drag would drop on — same segment walk
/// as render() and hit(), so the highlight matches the click.
pub fn drop_rect(rt: &Runtime, target: TabDrop, bar: Rect) -> Option<Rect> {
    let ws = rt.state.active_workspace();
    segment_rect(&segments(rt), &ws.tabs, target, bar)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::ids::{PaneId, TabId};
    use crate::state::workspace::Tab;

    #[test]
    fn segment_rect_matches_and_clamps() {
        let tabs = vec![
            Tab::new(TabId(1), "1".into(), PaneId(1)),
            Tab::new(TabId(2), "2".into(), PaneId(2)),
        ];
        // A couple of tab segments plus the `+` (NewTab) segment, same shape
        // `segments()` produces: widths 2, 3, 1 at cursors 0, 2, 5.
        let segs = vec![
            Segment { text: "AA".into(), hit: Some(Hit::Tab(0)), active: false },
            Segment { text: "BBB".into(), hit: Some(Hit::Tab(1)), active: false },
            Segment { text: "+".into(), hit: Some(Hit::NewTab), active: false },
        ];

        // (a) TabDrop::Tab(id) returns the segment's exact span.
        let bar = Rect::new(10, 0, 20, 1);
        let r = segment_rect(&segs, &tabs, TabDrop::Tab(TabId(1)), bar).unwrap();
        assert_eq!(r, Rect::new(10, 0, 2, 1));

        // (b) TabDrop::NewTab returns the `+` segment's exact span.
        let r = segment_rect(&segs, &tabs, TabDrop::NewTab, bar).unwrap();
        assert_eq!(r, Rect::new(15, 0, 1, 1));

        // (c) a segment starting beyond a too-narrow bar clamps to width 0
        // at the bar's edge (cursor.min(bar.width) + saturating_sub) rather
        // than overflowing.
        let narrow = Rect::new(0, 0, 3, 1);
        let r = segment_rect(&segs, &tabs, TabDrop::NewTab, narrow).unwrap();
        assert_eq!(r, Rect::new(3, 0, 0, 1));

        // No matching segment (unknown id): None, not a panic.
        assert!(segment_rect(&segs, &tabs, TabDrop::Tab(TabId(99)), bar).is_none());
    }
}
