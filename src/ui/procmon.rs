//! Process-monitor overlay: system load header + processes grouped by pane,
//! plus a detail panel for a single process (pid, ppid, pane, cwd, uptime,
//! cpu, mem, full cmd, kill). `InputMode::ProcessMonitor { selected, detail }`
//! toggles between the list (`detail: None`) and the detail view
//! (`detail: Some(pid)`).

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::runtime::{MonitorRow, MonitorSnapshot, Runtime};
use crate::state::ids::PaneId;

/// Selectable/killable row indices into `snap.rows`. Every process is
/// killable — including a pane's own agent/shell (killing it stops the agent
/// and closes the pane, which is what the user asked for). `selected` indexes
/// into this list, not `snap.rows` directly.
pub fn killable_rows(snap: &MonitorSnapshot) -> Vec<usize> {
    (0..snap.rows.len()).collect()
}

/// Human-readable byte size — this box's numbers never reach TiB.
fn human_bytes(n: u64) -> String {
    const KIB: f64 = 1024.0;
    let n = n as f64;
    if n < KIB {
        format!("{n:.0}B")
    } else if n < KIB * KIB {
        format!("{:.1}KiB", n / KIB)
    } else if n < KIB * KIB * KIB {
        format!("{:.1}MiB", n / (KIB * KIB))
    } else {
        format!("{:.1}GiB", n / (KIB * KIB * KIB))
    }
}

/// `Ns` / `MmSSs` / `HhMMm` since `start`.
fn human_uptime(start: std::time::SystemTime) -> String {
    let secs =
        std::time::SystemTime::now().duration_since(start).map(|d| d.as_secs()).unwrap_or(0);
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else {
        format!("{}h{:02}m", secs / 3600, (secs % 3600) / 60)
    }
}

/// Truncate to `max` chars keeping the TAIL — the script name / args at the
/// end matter more than the shared `/Users/…/` path prefix. Ellipsis on the
/// left marks the cut.
fn truncate(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        s.to_string()
    } else {
        let tail: String = s.chars().skip(count - max.saturating_sub(1)).collect();
        format!("…{tail}")
    }
}

/// A human label for the pane that owns a process: for a live pane,
/// `{space} · {custom-name|agent|pane N}`; for an orphan whose pane was closed
/// (the common case — a dead script the agent forgot), `{N} (closed)`.
fn pane_label(rt: &Runtime, pane: PaneId) -> String {
    match rt.state.locate_pane(pane) {
        Some((wi, _)) => {
            let space = &rt.state.workspaces[wi].name;
            let who = rt
                .state
                .pane_name(pane)
                .map(str::to_string)
                .or_else(|| rt.panes.get(&pane).and_then(|p| p.agent).map(str::to_string))
                .unwrap_or_else(|| format!("pane {pane}"));
            format!("{space} · {who}")
        }
        None => format!("{pane} (closed)"),
    }
}

fn row_line(row: &MonitorRow, selected: bool) -> Line<'static> {
    let cpu = row.cpu_pct.map(|p| format!("{p:>3.0}%")).unwrap_or_else(|| "  —".to_string());
    let mem = human_bytes(row.info.rss);
    let uptime = human_uptime(row.info.start);
    let orphan = if row.orphan { " [orphan]" } else { "" };
    let cmd = truncate(&row.info.cmd, 32);
    let text = format!("  {cmd:<32} {uptime:>7} {cpu} {mem:>9}{orphan}");
    let mut style = Style::new();
    if selected {
        style = style.add_modifier(Modifier::REVERSED);
    }
    Line::from(Span::styled(text, style))
}

/// Build the list box's geometry and content: the centered box `Rect`, the
/// scroll offset (so the selected row stays in view), the lines themselves,
/// and a parallel `line_pid` (same length as `lines`) giving the pid a
/// process row represents — `None` for the SYSTEM header / blank / pane-group
/// header lines. Shared by `render_list` (drawing) and `pid_at` (hit-testing)
/// so the two can never disagree about where a row lands.
fn layout(
    rt: &Runtime,
    selected: usize,
    area: Rect,
) -> (Rect, u16, Vec<Line<'static>>, Vec<Option<u32>>) {
    let Some(snap) = rt.monitor() else {
        return (Rect::default(), 0, Vec::new(), Vec::new());
    };
    let selected_row = killable_rows(snap).get(selected).copied();

    let cpu = snap.load.cpu_pct.map(|p| format!("{p:.0}%")).unwrap_or_else(|| "—".to_string());
    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled(
            format!(
                "SYSTEM  cpu {cpu}  mem {}/{}",
                human_bytes(snap.load.mem_used),
                human_bytes(snap.load.mem_total),
            ),
            Style::new().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];
    let mut line_pid: Vec<Option<u32>> = vec![None, None];

    // Group rows by pane, preserving first-seen order (no indexmap dep for
    // one small overlay list).
    let mut groups: Vec<(PaneId, Vec<usize>)> = Vec::new();
    for (i, row) in snap.rows.iter().enumerate() {
        match groups.iter_mut().find(|(p, _)| *p == row.info.pane) {
            Some((_, idxs)) => idxs.push(i),
            None => groups.push((row.info.pane, vec![i])),
        }
    }

    // Track the line the selected row lands on so the view can scroll to it.
    let mut sel_line: Option<usize> = None;
    for (pane, idxs) in &groups {
        lines.push(Line::from(Span::styled(
            pane_label(rt, *pane),
            Style::new().fg(rt.theme.accent).add_modifier(Modifier::BOLD),
        )));
        line_pid.push(None);
        for &i in idxs {
            if Some(i) == selected_row {
                sel_line = Some(lines.len());
            }
            lines.push(row_line(&snap.rows[i], Some(i) == selected_row));
            line_pid.push(Some(snap.rows[i].info.pid));
        }
    }

    let w = 64.min(area.width);
    let h = (lines.len() as u16 + 2).min(area.height);
    let inner_h = h.saturating_sub(2) as usize; // visible content rows (minus borders)
    // Scroll so the selected row stays in view once the list overflows the box.
    let scroll = match sel_line {
        Some(l) if l >= inner_h => (l - inner_h + 1) as u16,
        _ => 0,
    };
    let rect = Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    };
    (rect, scroll, lines, line_pid)
}

/// Clear + Block + Paragraph overlay (see `ui::help::render_help`): system
/// load header, then rows grouped by owning pane, the `selected`-th killable
/// row reverse-video, protected rows dimmed with no selection marker.
fn render_list(rt: &Runtime, selected: usize, area: Rect, frame: &mut Frame) {
    if rt.monitor().is_none() {
        return;
    }
    let (rect, scroll, lines, _) = layout(rt, selected, area);
    frame.render_widget(Clear, rect);
    let block = Block::new()
        .borders(Borders::ALL)
        .title(" processes ")
        .border_style(Style::new().fg(rt.theme.accent));
    frame.render_widget(Paragraph::new(lines).block(block).scroll((scroll, 0)), rect);
}

/// The pid of the process row under screen position `(x, y)`, or `None` if
/// the click landed outside the list box or on a non-row line (header/blank/
/// group label). Shares `layout` with `render_list` so hit-testing always
/// matches what's on screen.
pub fn pid_at(rt: &Runtime, selected: usize, area: Rect, x: u16, y: u16) -> Option<u32> {
    let (rect, scroll, _, line_pid) = layout(rt, selected, area);
    if rect.width == 0 || !rect.contains(Position { x, y }) {
        return None;
    }
    // Exclude the top/bottom border rows — only content maps to a pid (a click
    // on the top border with a scrolled list would otherwise pick a real row).
    if y <= rect.y || y + 1 >= rect.y + rect.height {
        return None;
    }
    let idx = (y - rect.y - 1) as usize + scroll as usize;
    line_pid.get(idx).copied().flatten()
}

/// The detail box's geometry + content for a live pid — `None` if the pid
/// dropped out of the last snapshot (it exited). Shared by `render_detail`
/// (drawing) and `detail_kill_click` (hit-testing the footer). The footer
/// (`[k] kill  [Esc] back`) is the last content line, so it renders at the box
/// bottom (`rect.y + rect.height - 2`), which is what the click test uses.
fn detail_box(rt: &Runtime, pid: u32, area: Rect) -> Option<(Rect, Vec<Line<'static>>)> {
    let row = rt.monitor().and_then(|s| s.rows.iter().find(|r| r.info.pid == pid))?;
    let info = &row.info;
    let orphan = if row.orphan { " [orphan]" } else { "" };
    let cpu = row.cpu_pct.map(|p| format!("{p:.0}%")).unwrap_or_else(|| "—".to_string());
    let cwd = crate::platform::process_cwd(pid)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "—".to_string());

    let lines: Vec<Line> = vec![
        Line::from(Span::styled(
            format!("pid {pid} · ppid {}{orphan}", info.ppid),
            Style::new().add_modifier(Modifier::BOLD),
        )),
        Line::from(format!("pane   {}", pane_label(rt, info.pane))),
        Line::from(format!("cwd    {cwd}")),
        Line::from(format!("started {}", human_uptime(info.start))),
        Line::from(format!("cpu {cpu}   mem {}", human_bytes(info.rss))),
        Line::from(""),
        Line::from(Span::styled("cmd:", Style::new().add_modifier(Modifier::BOLD))),
        Line::from(info.cmd.clone()),
        Line::from(""),
        Line::from(Span::styled(
            " [k] kill    [Esc] back ",
            Style::new().add_modifier(Modifier::BOLD),
        )),
    ];

    let w = 60.min(area.width);
    // Box height must fit the wrapped cmd (and a possibly long cwd) so the
    // kill/back footer never falls off the bottom.
    let inner_w = (w as usize).saturating_sub(2).max(1);
    let wrap_rows = |s: &str| (s.chars().count().div_ceil(inner_w)).max(1) as u16;
    let extra = wrap_rows(&info.cmd).saturating_sub(1) + wrap_rows(&cwd).saturating_sub(1);
    let h = (lines.len() as u16 + extra + 2).min(area.height);
    let rect = Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    };
    Some((rect, lines))
}

/// True when `(x, y)` lands on the detail box's footer row — the `[k] kill`
/// button — so a click there kills the process.
pub fn detail_kill_click(rt: &Runtime, pid: u32, area: Rect, x: u16, y: u16) -> bool {
    let Some((rect, _)) = detail_box(rt, pid, area) else { return false };
    let footer_row = rect.y + rect.height.saturating_sub(2);
    rect.contains(Position { x, y }) && y == footer_row
}

/// Detail panel for a single process. Falls back to a small "process ended"
/// box if the pid dropped out of the last snapshot.
fn render_detail(rt: &Runtime, pid: u32, area: Rect, frame: &mut Frame) {
    let Some((rect, lines)) = detail_box(rt, pid, area) else {
        let w = 30.min(area.width);
        let h = 3.min(area.height);
        let rect = Rect {
            x: area.x + (area.width - w) / 2,
            y: area.y + (area.height - h) / 2,
            width: w,
            height: h,
        };
        frame.render_widget(Clear, rect);
        let block = Block::new()
            .borders(Borders::ALL)
            .title(" process ")
            .border_style(Style::new().fg(rt.theme.accent));
        frame.render_widget(
            Paragraph::new(Line::from(format!("pid {pid}: process ended"))).block(block),
            rect,
        );
        return;
    };

    frame.render_widget(Clear, rect);
    let block = Block::new()
        .borders(Borders::ALL)
        .title(" process ")
        .border_style(Style::new().fg(rt.theme.accent));
    frame.render_widget(
        Paragraph::new(lines).block(block).wrap(ratatui::widgets::Wrap { trim: false }),
        rect,
    );
}

/// Render the process-monitor overlay: the list, or a detail panel if
/// `detail` names a pid.
pub fn render(rt: &Runtime, selected: usize, detail: Option<u32>, area: Rect, frame: &mut Frame) {
    match detail {
        Some(pid) => render_detail(rt, pid, area, frame),
        None => render_list(rt, selected, area, frame),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn killable_rows_are_every_row() {
        use crate::runtime::{MonitorRow, MonitorSnapshot};
        let row = |pid, protected| MonitorRow {
            info: crate::platform::ProcInfo {
                pid,
                ppid: 100,
                pane: crate::state::ids::PaneId(1),
                cmd: "x".into(),
                start: std::time::SystemTime::now(),
                cpu_ns: 0,
                rss: 0,
            },
            cpu_pct: None,
            orphan: false,
            protected,
        };
        // Every process is killable now — including the pane's own agent.
        let snap = MonitorSnapshot {
            load: Default::default(),
            rows: vec![row(10, true), row(11, false), row(12, false)],
        };
        assert_eq!(super::killable_rows(&snap), vec![0, 1, 2]);
    }
}
