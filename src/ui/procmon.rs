//! Process-monitor overlay: system load header + processes grouped by pane.
//! `InputMode::ProcessMonitor { selected }` toggles it; key handling (select,
//! kill) is a later task — this module only renders.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::runtime::{MonitorRow, MonitorSnapshot, Runtime};
use crate::state::ids::PaneId;

/// Indices into `snap.rows` that aren't protected (cdock's own child procs).
/// `selected` (in `InputMode::ProcessMonitor`) indexes into this list, not
/// `snap.rows` directly — protected rows can't be selected/killed.
pub fn killable_rows(snap: &MonitorSnapshot) -> Vec<usize> {
    snap.rows.iter().enumerate().filter(|(_, r)| !r.protected).map(|(i, _)| i).collect()
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

/// Custom pane name, else its agent, else `pane %{id}`.
fn pane_label(rt: &Runtime, pane: PaneId) -> String {
    if let Some(name) = rt.state.pane_name(pane) {
        name.to_string()
    } else if let Some(agent) = rt.panes.get(&pane).and_then(|p| p.agent) {
        agent.to_string()
    } else {
        format!("pane {pane}")
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
    if row.protected {
        style = style.add_modifier(Modifier::DIM);
    }
    if selected {
        style = style.add_modifier(Modifier::REVERSED);
    }
    Line::from(Span::styled(text, style))
}

/// Clear + Block + Paragraph overlay (see `ui::help::render_help`): system
/// load header, then rows grouped by owning pane, the `selected`-th killable
/// row reverse-video, protected rows dimmed with no selection marker.
pub fn render(rt: &Runtime, selected: usize, area: Rect, frame: &mut Frame) {
    let Some(snap) = rt.monitor() else { return };
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
        for &i in idxs {
            if Some(i) == selected_row {
                sel_line = Some(lines.len());
            }
            lines.push(row_line(&snap.rows[i], Some(i) == selected_row));
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
    frame.render_widget(Clear, rect);
    let block = Block::new()
        .borders(Borders::ALL)
        .title(" processes ")
        .border_style(Style::new().fg(rt.theme.accent));
    frame.render_widget(Paragraph::new(lines).block(block).scroll((scroll, 0)), rect);
}

#[cfg(test)]
mod tests {
    #[test]
    fn killable_rows_excludes_protected() {
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
        let snap = MonitorSnapshot {
            load: Default::default(),
            rows: vec![row(10, true), row(11, false), row(12, false)],
        };
        assert_eq!(super::killable_rows(&snap), vec![1, 2]);
    }
}
