//! Headless server: owns the runtime (PTYs, emulators, state), renders a
//! frame per tick into an in-memory buffer, and streams pre-diffed ANSI to
//! attached clients. Clients are thin and disposable — agents keep running
//! when every client detaches.

use std::collections::HashMap;
use std::io;
use std::time::Duration;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;

use crate::api;
use crate::config::Config;
use crate::proto::{self, ClientMsg, PROTOCOL_VERSION, ServerMsg};
use crate::runtime::event::{AppEvent, PtyData};
use crate::runtime::{self, InputOutcome, Runtime};
use crate::ui;

pub struct ServerOpts {
    /// --no-session: stop when the last client disconnects.
    pub exit_when_no_clients: bool,
}

/// How the server loop ended.
pub enum RunOutcome {
    Exit,
    /// Live handoff requested: the caller writes this out and execs the
    /// (possibly updated) binary in place.
    Handoff(Box<runtime::Handoff>),
}

type ClientId = u64;

struct Client {
    tx: mpsc::UnboundedSender<ServerMsg>,
    /// Previous frame buffer; None → send a full repaint.
    prev: Option<Buffer>,
}

enum ClientCtl {
    New(UnixStream),
    In(ClientId, ClientMsg),
    Gone(ClientId),
}

pub async fn run(
    cfg: Config,
    listener: Option<UnixListener>,
    api_listener: Option<UnixListener>,
    initial: Vec<UnixStream>,
    handoff: Option<runtime::Handoff>,
    opts: ServerOpts,
) -> io::Result<RunOutcome> {
    let (tx, mut rx) = mpsc::unbounded_channel::<AppEvent>();
    let (data_tx, mut data_rx) = mpsc::channel::<PtyData>(16);
    let (raw_tx, mut raw_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let (ctl_tx, mut ctl_rx) = mpsc::unbounded_channel::<ClientCtl>();

    // Screen size before any client says hello (a handoff carries its own).
    let mut area = match &handoff {
        Some(h) => Rect::new(0, 0, h.area.0.max(4), h.area.1.max(4)),
        None => Rect::new(0, 0, 100, 30),
    };
    let mut rt = match handoff {
        Some(h) => runtime::build_from_handoff(cfg, h, tx, data_tx, raw_tx)?,
        None => runtime::build(cfg, tx, data_tx, raw_tx, area)?,
    };

    let mut clients: HashMap<ClientId, Client> = HashMap::new();
    let mut next_client: ClientId = 1;
    // Automation API: parked wait-* requests resolve on the agent poll;
    // subscribers get pushed events (agent-status, output).
    let (api_tx, mut api_rx) = mpsc::unbounded_channel::<api::ConnMsg>();
    let mut waiters: Vec<(api::PendingWait, api::Replier)> = Vec::new();
    let mut subs: Vec<(api::SubSpec, mpsc::UnboundedSender<serde_json::Value>)> = Vec::new();
    let mut term = Terminal::new(TestBackend::new(area.width, area.height)).expect("test backend is infallible");

    for stream in initial {
        let _ = ctl_tx.send(ClientCtl::New(stream));
    }

    let mut tick = tokio::time::interval(Duration::from_millis(16));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut ws_poll = tokio::time::interval(Duration::from_millis(2000));
    ws_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut manifests = crate::detect::load_all();
    let mut agent_poll = tokio::time::interval(Duration::from_millis(500));
    agent_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // Debounced session persistence (ARCHITECTURE §6).
    let mut autosave = tokio::time::interval(Duration::from_millis(5000));
    autosave.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    // Belt to the process-group braces: never die on tty hangup.
    let mut sighup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())?;
    tokio::spawn(async move {
        loop {
            sighup.recv().await;
            tracing::debug!("ignoring SIGHUP");
        }
    });

    tracing::info!("server running");

    loop {
        tokio::select! {
            biased;
            Some(ctl) = ctl_rx.recv() => match ctl {
                ClientCtl::New(stream) => {
                    let id = next_client;
                    next_client += 1;
                    let (read_half, write_half) = stream.into_split();
                    let (out_tx, out_rx) = mpsc::unbounded_channel::<ServerMsg>();
                    clients.insert(id, Client { tx: out_tx, prev: None });
                    spawn_client_io(id, read_half, write_half, out_rx, ctl_tx.clone());
                    tracing::info!(client = id, "client connected");
                }
                ClientCtl::In(id, msg) => match msg {
                    ClientMsg::Hello { version, cols, rows } => {
                        if version != PROTOCOL_VERSION {
                            tracing::warn!(client = id, version, "protocol mismatch");
                            if let Some(c) = clients.get(&id) {
                                let _ = c.tx.send(ServerMsg::Shutdown);
                            }
                            clients.remove(&id);
                            continue;
                        }
                        if let Some(c) = clients.get_mut(&id) {
                            let _ = c.tx.send(ServerMsg::Welcome { version: PROTOCOL_VERSION });
                        }
                        resize_all(&mut area, &mut term, &mut clients, cols, rows)?;
                        rt.mark_dirty();
                    }
                    ClientMsg::Event(ev) => {
                        if let crossterm::event::Event::Resize(cols, rows) = ev {
                            resize_all(&mut area, &mut term, &mut clients, cols, rows)?;
                        }
                        match runtime::handle_input(&mut rt, ev, area)? {
                            InputOutcome::Continue => {}
                            InputOutcome::Detach => {
                                for c in clients.values() {
                                    let _ = c.tx.send(ServerMsg::Detach);
                                }
                                clients.clear();
                                rt.save_session();
                                if opts.exit_when_no_clients {
                                    flush_writers().await;
                                    return Ok(RunOutcome::Exit);
                                }
                            }
                            InputOutcome::Shutdown => {
                                rt.save_session();
                                shutdown_clients(&clients);
                                flush_writers().await;
                                return Ok(RunOutcome::Exit);
                            }
                        }
                    }
                    ClientMsg::Detach => {
                        if let Some(c) = clients.remove(&id) {
                            let _ = c.tx.send(ServerMsg::Detach);
                        }
                        if clients.is_empty() && opts.exit_when_no_clients {
                            rt.save_session();
                            return Ok(RunOutcome::Exit);
                        }
                    }
                },
                ClientCtl::Gone(id) => {
                    clients.remove(&id);
                    tracing::info!(client = id, "client disconnected");
                    if clients.is_empty() && opts.exit_when_no_clients {
                        rt.save_session();
                        return Ok(RunOutcome::Exit);
                    }
                }
            },
            Some(ev) = rx.recv() => {
                let mut next = Some(ev);
                while let Some(ev) = next.take() {
                    match ev {
                        AppEvent::PtyExit(id) => runtime::handle_pane_exit(&mut rt, id, area),
                        AppEvent::Term(id, tev) => runtime::handle_term_event(&mut rt, id, tev),
                    }
                    next = rx.try_recv().ok();
                }
            }
            Some(bytes) = raw_rx.recv() => {
                for c in clients.values() {
                    let _ = c.tx.send(ServerMsg::Frame(bytes.clone()));
                }
            }
            maybe = data_rx.recv() => {
                let Some(first) = maybe else { return Ok(RunOutcome::Exit) };
                let stream_output = api::wanted(&subs, "output");
                let mut budget = runtime::PTY_DRAIN_BUDGET;
                let mut next = Some(first);
                while let Some((id, bytes)) = next.take() {
                    budget = budget.saturating_sub(bytes.len());
                    if stream_output {
                        api::emit(&mut subs, "output", Some(id.0), &serde_json::json!({
                            "event": "output", "pane": id.0,
                            "data": String::from_utf8_lossy(&bytes),
                        }));
                    }
                    runtime::feed_pty(&mut rt, id, &bytes);
                    if budget > 0 {
                        next = data_rx.try_recv().ok();
                    }
                }
            }
            Some(Ok((stream, _))) = accept_next(&listener) => {
                let _ = ctl_tx.send(ClientCtl::New(stream));
            }
            Some(Ok((stream, _))) = accept_next(&api_listener) => {
                api::spawn_conn(stream, api_tx.clone());
            }
            Some(conn) = api_rx.recv() => match conn {
                api::ConnMsg::Sub(spec, tx) => {
                    tracing::info!(?spec.events, pane = ?spec.pane, "event subscriber");
                    subs.push((spec, tx));
                }
                api::ConnMsg::Req(req, reply) => match req {
                // The loop owns the manifest set and the exec decision.
                api::Req::ReloadManifests => {
                    manifests = crate::detect::load_all();
                    let _ = reply.send(serde_json::json!({"ok": true, "count": manifests.len()}));
                }
                api::Req::Handoff => {
                    rt.save_session();
                    let h = runtime::capture_handoff(&rt, area);
                    let _ = reply.send(serde_json::json!({"ok": true}));
                    flush_writers().await; // let the reply reach the CLI
                    return Ok(RunOutcome::Handoff(Box::new(h)));
                }
                    req => match api::handle(&mut rt, area, req) {
                        Ok(v) => { let _ = reply.send(v); }
                        Err(pending) => waiters.push((pending, reply)),
                    }
                },
            },
            _ = ws_poll.tick() => rt.poll_workspaces(),
            _ = agent_poll.tick() => {
                rt.expire_toasts();
                let (notices, changes) = rt.poll_agent_status(&manifests);
                for ch in &changes {
                    api::emit(&mut subs, "agent-status", Some(ch.pane.0), &serde_json::json!({
                        "event": "agent-status", "pane": ch.pane.0, "agent": ch.agent,
                        "from": ch.from.word(), "to": ch.to.word(),
                    }));
                }
                // After the poll — waiters must see fresh statuses.
                api::check_waiters(&rt, &mut waiters);
                for notice in notices {
                    // Suppress for the pane the user is looking at right now.
                    let visible = !clients.is_empty() && rt.state.focused_pane() == notice.pane;
                    if !visible {
                        notify(&mut rt, &clients, &notice);
                    }
                }
            }
            _ = autosave.tick() => {
                if !clients.is_empty() || !opts.exit_when_no_clients {
                    rt.save_session();
                }
            }
            _ = tick.tick() => {
                if rt.take_dirty() && !clients.is_empty() {
                    render_frame(&mut rt, &mut term, area, &mut clients)?;
                } else if rt.take_dirty() {
                    // Headless: still run geometry so PTY sizes stay right.
                    let _ = ui::compute_view(&mut rt, area);
                }
            }
        }
    }
}

async fn accept_next(
    listener: &Option<UnixListener>,
) -> Option<io::Result<(UnixStream, tokio::net::unix::SocketAddr)>> {
    match listener {
        Some(l) => Some(l.accept().await),
        None => std::future::pending().await,
    }
}

/// Sound + toast for an agent transition. Delivery: "app" (clickable
/// top-right overlay), "system" (OS notification), "both", "off".
fn notify(rt: &mut Runtime, clients: &HashMap<ClientId, Client>, notice: &runtime::Notice) {
    let delivery = rt.cfg.ui.toast.delivery.clone();
    if matches!(delivery.as_str(), "app" | "both") {
        rt.add_toast(notice);
    }
    let sound_on = rt.cfg.ui.sound.enabled && std::env::var_os("CDOCK_DISABLE_SOUND").is_none();
    if sound_on {
        // Terminal bell for attached clients (dock bounce / tab highlight)…
        for c in clients.values() {
            let _ = c.tx.send(ServerMsg::Frame(b"\x07".to_vec()));
        }
        // …and an audible system sound on macOS.
        #[cfg(target_os = "macos")]
        {
            let file = match notice.kind {
                runtime::NoticeKind::Blocked => "/System/Library/Sounds/Basso.aiff",
                runtime::NoticeKind::Done => "/System/Library/Sounds/Glass.aiff",
            };
            let _ = std::process::Command::new("afplay")
                .arg(file)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
        }
    }

    if matches!(delivery.as_str(), "system" | "both") {
        let text = match notice.kind {
            runtime::NoticeKind::Blocked => format!("{} needs your input", notice.name),
            runtime::NoticeKind::Done => format!("{} finished", notice.name),
        };
        #[cfg(target_os = "macos")]
        {
            let script =
                format!("display notification \"{}\" with title \"cdock\"", text.replace('\"', ""));
            let _ = std::process::Command::new("osascript")
                .args(["-e", &script])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
        }
        #[cfg(target_os = "linux")]
        {
            let _ = std::process::Command::new("notify-send")
                .args(["cdock", &text])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        let _ = text;
    }
    tracing::info!(pane = %notice.pane, kind = ?notice.kind, "notified");
}

/// Returning from run() drops the tokio runtime and aborts the writer
/// tasks; give them a beat to flush Detach/Shutdown to clients first.
async fn flush_writers() {
    tokio::time::sleep(Duration::from_millis(120)).await;
}

fn shutdown_clients(clients: &HashMap<ClientId, Client>) {
    for c in clients.values() {
        let _ = c.tx.send(ServerMsg::Shutdown);
    }
}

fn resize_all(
    area: &mut Rect,
    term: &mut Terminal<TestBackend>,
    clients: &mut HashMap<ClientId, Client>,
    cols: u16,
    rows: u16,
) -> io::Result<()> {
    let new = Rect::new(0, 0, cols.max(4), rows.max(4));
    if *area != new {
        *area = new;
        *term = Terminal::new(TestBackend::new(new.width, new.height)).expect("test backend is infallible");
        for c in clients.values_mut() {
            c.prev = None; // full repaint at the new size
        }
    }
    Ok(())
}

fn render_frame(
    rt: &mut Runtime,
    term: &mut Terminal<TestBackend>,
    area: Rect,
    clients: &mut HashMap<ClientId, Client>,
) -> io::Result<()> {
    let view = ui::compute_view(rt, area);
    term.draw(|f| ui::render(&view, rt, f)).expect("test backend is infallible");
    let curr = term.backend().buffer().clone();
    let cursor = ui::cursor_for(&view, rt);

    for c in clients.values_mut() {
        let bytes = ansi_diff(c.prev.as_ref(), &curr, cursor);
        if !bytes.is_empty() {
            let _ = c.tx.send(ServerMsg::Frame(bytes));
        }
        c.prev = Some(curr.clone());
    }
    Ok(())
}

/// Encode the difference between two frames as host-terminal ANSI bytes —
/// the client writes them verbatim ("pre-diffed ANSI" from ARCHITECTURE §2).
fn ansi_diff(prev: Option<&Buffer>, curr: &Buffer, cursor: Option<(u16, u16)>) -> Vec<u8> {
    use std::fmt::Write as _;

    let mut out = String::from("\x1b[?25l");
    let mut last_style: Option<(Color, Color, Modifier)> = None;
    let mut last_pos: Option<(u16, u16)> = None;

    let updates: Vec<(u16, u16, &ratatui::buffer::Cell)> = match prev {
        Some(p) if p.area == curr.area => p.diff(curr),
        _ => {
            out.push_str("\x1b[2J");
            let mut v = Vec::with_capacity(curr.content.len());
            for i in 0..curr.content.len() {
                let (x, y) = curr.pos_of(i);
                v.push((x, y, &curr.content[i]));
            }
            v
        }
    };

    for (x, y, cell) in updates {
        if cell.symbol().is_empty() {
            continue; // wide-char continuation
        }
        // Move only when the cursor isn't already there.
        if last_pos != Some((x, y)) {
            let _ = write!(out, "\x1b[{};{}H", y + 1, x + 1);
        }
        let style = (cell.fg, cell.bg, cell.modifier);
        if last_style != Some(style) {
            out.push_str(&sgr(cell.fg, cell.bg, cell.modifier));
            last_style = Some(style);
        }
        out.push_str(cell.symbol());
        let w = unicode_width::UnicodeWidthStr::width(cell.symbol()).max(1) as u16;
        last_pos = Some((x + w, y));
    }
    out.push_str("\x1b[0m");
    match cursor {
        Some((x, y)) => {
            let _ = write!(out, "\x1b[{};{}H\x1b[?25h", y + 1, x + 1);
        }
        None => out.push_str("\x1b[?25l"),
    }
    if out == "\x1b[?25l\x1b[0m\x1b[?25l" {
        return Vec::new(); // nothing changed
    }
    out.into_bytes()
}

fn sgr(fg: Color, bg: Color, m: Modifier) -> String {
    let mut s = String::from("\x1b[0m");
    if m.contains(Modifier::BOLD) {
        s.push_str("\x1b[1m");
    }
    if m.contains(Modifier::DIM) {
        s.push_str("\x1b[2m");
    }
    if m.contains(Modifier::ITALIC) {
        s.push_str("\x1b[3m");
    }
    if m.contains(Modifier::UNDERLINED) {
        s.push_str("\x1b[4m");
    }
    if m.contains(Modifier::REVERSED) {
        s.push_str("\x1b[7m");
    }
    if m.contains(Modifier::HIDDEN) {
        s.push_str("\x1b[8m");
    }
    if m.contains(Modifier::CROSSED_OUT) {
        s.push_str("\x1b[9m");
    }
    s.push_str(&color_seq(fg, false));
    s.push_str(&color_seq(bg, true));
    s
}

fn color_seq(c: Color, bg: bool) -> String {
    let base = if bg { 40 } else { 30 };
    match c {
        Color::Reset => String::new(),
        Color::Black => format!("\x1b[{}m", base),
        Color::Red => format!("\x1b[{}m", base + 1),
        Color::Green => format!("\x1b[{}m", base + 2),
        Color::Yellow => format!("\x1b[{}m", base + 3),
        Color::Blue => format!("\x1b[{}m", base + 4),
        Color::Magenta => format!("\x1b[{}m", base + 5),
        Color::Cyan => format!("\x1b[{}m", base + 6),
        Color::Gray => format!("\x1b[{}m", base + 7),
        Color::DarkGray => format!("\x1b[{}m", base + 60),
        Color::LightRed => format!("\x1b[{}m", base + 61),
        Color::LightGreen => format!("\x1b[{}m", base + 62),
        Color::LightYellow => format!("\x1b[{}m", base + 63),
        Color::LightBlue => format!("\x1b[{}m", base + 64),
        Color::LightMagenta => format!("\x1b[{}m", base + 65),
        Color::LightCyan => format!("\x1b[{}m", base + 66),
        Color::White => format!("\x1b[{}m", base + 67),
        Color::Indexed(i) => format!("\x1b[{};5;{}m", if bg { 48 } else { 38 }, i),
        Color::Rgb(r, g, b) => format!("\x1b[{};2;{};{};{}m", if bg { 48 } else { 38 }, r, g, b),
    }
}

fn spawn_client_io(
    id: ClientId,
    mut read_half: tokio::net::unix::OwnedReadHalf,
    mut write_half: tokio::net::unix::OwnedWriteHalf,
    mut out_rx: mpsc::UnboundedReceiver<ServerMsg>,
    ctl_tx: mpsc::UnboundedSender<ClientCtl>,
) {
    let ctl_read = ctl_tx.clone();
    tokio::spawn(async move {
        loop {
            match proto::read_msg_async::<ClientMsg>(&mut read_half).await {
                Ok(msg) => {
                    if ctl_read.send(ClientCtl::In(id, msg)).is_err() {
                        break;
                    }
                }
                Err(_) => {
                    let _ = ctl_read.send(ClientCtl::Gone(id));
                    break;
                }
            }
        }
    });
    tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            let last = matches!(msg, ServerMsg::Detach | ServerMsg::Shutdown);
            if proto::write_msg_async(&mut write_half, &msg).await.is_err() {
                let _ = ctl_tx.send(ClientCtl::Gone(id));
                break;
            }
            if last {
                break;
            }
        }
    });
}
