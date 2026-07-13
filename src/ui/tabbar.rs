use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::config::theme::Theme;
use crate::runtime::Runtime;
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
            && !title.trim().is_empty() {
                let short: String = title.chars().take(16).collect();
                return short;
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
                && x >= cursor + w.saturating_sub(3) {
                    return Some(Hit::CloseTab(ti));
                }
            return s.hit;
        }
        cursor += w;
    }
    None
}
