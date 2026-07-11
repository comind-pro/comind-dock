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
    /// Config-load warnings surfaced as toasts on the first attach.
    pub boot_warnings: Vec<String>,
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
    /// Next frame must be a full repaint (fresh client / area change).
    needs_full: bool,
    /// Last reported terminal size — the shared area re-fits to a survivor
    /// when another client leaves.
    size: (u16, u16),
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
    let update_check_tx = tx.clone();
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
    let mut frame_cache = FrameCache { prev: None };

    for stream in initial {
        let _ = ctl_tx.send(ClientCtl::New(stream));
    }

    let mut tick = tokio::time::interval(Duration::from_millis(16));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut ws_poll = tokio::time::interval(Duration::from_millis(2000));
    ws_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut manifests = crate::detect::load_all();
    let mut plugins = crate::plugin::list();
    // Per-pane damping for plugin hooks: the activity fallback can flap
    // Working/Idle on every output pause — shelling out each time is a storm.
    let mut hook_last: HashMap<crate::state::ids::PaneId, std::time::Instant> = HashMap::new();
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

    // Background release check: on start and every 6h (menu "update ready").
    // The thread re-reads the config each round, so a reload that flips
    // update.channel/repo/check takes effect without a restart.
    if rt.cfg.update.check {
        let update_tx = update_check_tx;
        std::thread::spawn(move || {
            loop {
                let upd = crate::config::load(None).0.update;
                if !upd.check {
                    std::thread::sleep(Duration::from_secs(6 * 3600));
                    continue;
                }
                let (repo, channel) = (upd.repo, upd.channel);
                // A failed check (network, GitHub rate limit) retries in
                // 10min, not 6h — restarts reset the in-memory flag, so a
                // silent failure would hide a known update for hours.
                let next = match crate::update::latest_release(&repo, channel) {
                    Ok(rel) if crate::update::is_newer(&rel.tag) => {
                        if update_tx.send(AppEvent::UpdateAvailable(rel.tag)).is_err() {
                            return;
                        }
                        Duration::from_secs(6 * 3600)
                    }
                    Ok(_) => Duration::from_secs(6 * 3600),
                    Err(e) => {
                        tracing::warn!(error = %e, "update check failed; retrying in 10min");
                        Duration::from_secs(600)
                    }
                };
                std::thread::sleep(next);
            }
        });
    }

    rt.persist = !opts.exit_when_no_clients;
    for w in &opts.boot_warnings {
        rt.add_plain_toast(format!("⚠ config: {w}"), 20);
    }
    if rt.persist
        && let Some(notes) = crate::update::take_pending_release_notes()
    {
        let title = notes.lines().next().unwrap_or("").trim_start_matches(['#', ' ']).to_string();
        rt.add_plain_toast(format!("updated to {title} — notes in the log"), 20);
        tracing::info!(%notes, "release notes");
    }

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
                    clients.insert(id, Client { tx: out_tx, needs_full: true, size: (0, 0) });
                    spawn_client_io(id, read_half, write_half, out_rx, ctl_tx.clone());
                    tracing::info!(client = id, "client connected");
                }
                ClientCtl::In(id, msg) => match msg {
                    ClientMsg::Hello { version, cols, rows, folder } => {
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
                            c.size = (cols, rows);
                        }
                        // Folder-scoped attach: last Hello wins; a plain
                        // attach widens the view back (the escape hatch
                        // until a widen-scope toggle exists).
                        match folder {
                            Some(folder) => {
                                tracing::info!(client = id, folder = %folder.display(), "scoped attach");
                                if let Some(pane) = rt.state.attach_scope(folder)
                                    && let Err(e) = rt.spawn_pane(pane, area.width.max(4), area.height.max(4)) {
                                        tracing::warn!(error = %e, "scoped attach: pane spawn failed");
                                    }
                            }
                            None => rt.state.scope = None,
                        }
                        resize_all(&mut area, &mut term, &mut clients, &mut frame_cache, cols, rows)?;
                        rt.mark_dirty();
                    }
                    ClientMsg::Event(ev) => {
                        if let crossterm::event::Event::Resize(cols, rows) = ev {
                            if let Some(c) = clients.get_mut(&id) {
                                c.size = (cols, rows);
                            }
                            resize_all(&mut area, &mut term, &mut clients, &mut frame_cache, cols, rows)?;
                        }
                        match runtime::handle_input(&mut rt, ev, area)? {
                            InputOutcome::Continue => {}
                            InputOutcome::Detach => {
                                // Only the client that pressed prefix+q — a
                                // second attached terminal keeps working.
                                if let Some(c) = clients.remove(&id) {
                                    let _ = c.tx.send(ServerMsg::Detach);
                                }
                                rt.save_session();
                                if clients.is_empty() && opts.exit_when_no_clients {
                                    flush_writers().await;
                                    return Ok(RunOutcome::Exit);
                                }
                                refit_area(&mut area, &mut term, &mut clients, &mut frame_cache, &mut rt)?;
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
                        refit_area(&mut area, &mut term, &mut clients, &mut frame_cache, &mut rt)?;
                    }
                },
                ClientCtl::Gone(id) => {
                    clients.remove(&id);
                    tracing::info!(client = id, "client disconnected");
                    if clients.is_empty() && opts.exit_when_no_clients {
                        rt.save_session();
                        return Ok(RunOutcome::Exit);
                    }
                    refit_area(&mut area, &mut term, &mut clients, &mut frame_cache, &mut rt)?;
                }
            },
            Some(ev) = rx.recv() => {
                let mut next = Some(ev);
                while let Some(ev) = next.take() {
                    match ev {
                        AppEvent::PtyExit(id) => runtime::handle_pane_exit(&mut rt, id, area),
                        AppEvent::Term(id, tev) => runtime::handle_term_event(&mut rt, id, tev),
                        AppEvent::UpdateAvailable(tag) => {
                            tracing::info!(%tag, "update available");
                            rt.update_available = Some(tag);
                            rt.mark_dirty();
                        }
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
                    api::feed_waiters(&mut waiters, id, &bytes);
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
                // git worktree add checks out a whole tree — seconds to
                // minutes on big repos. Run it off-loop, then re-enter as a
                // WorktreeOpen (finds the fresh checkout by branch).
                api::Req::WorktreeCreate { workspace, branch } => {
                    let Some(ws) =
                        api::resolve_ws_pub(&rt, workspace).map(|wi| &rt.state.workspaces[wi])
                    else {
                        let _ = reply.send(serde_json::json!({"ok": false, "error": "no such workspace"}));
                        continue;
                    };
                    let (repo, ws_id) = (ws.cwd.clone(), ws.id.0);
                    let root = rt.cfg.worktrees.root();
                    let inner_tx = api_tx.clone();
                    std::thread::spawn(move || {
                        match crate::git::worktree_add(&repo, &branch, &root) {
                            Ok(path) => {
                                let (otx, _orx) = tokio::sync::oneshot::channel();
                                let _ = inner_tx.send(api::ConnMsg::Req(
                                    api::Req::WorktreeOpen { workspace: Some(ws_id), branch },
                                    otx,
                                ));
                                let _ = reply.send(serde_json::json!({
                                    "ok": true, "path": path.to_string_lossy(),
                                }));
                            }
                            Err(e) => {
                                let _ =
                                    reply.send(serde_json::json!({"ok": false, "error": e}));
                            }
                        }
                    });
                }
                // The loop owns the manifest set and the exec decision.
                api::Req::ReloadManifests => {
                    manifests = crate::detect::load_all();
                    plugins = crate::plugin::list();
                    let _ = reply.send(serde_json::json!({"ok": true, "count": manifests.len()}));
                }
                api::Req::AgentExplain { pane } => {
                    use crate::state::ids::PaneId;
                    let pane = PaneId(pane);
                    let v = match rt.panes.get(&pane) {
                        None => serde_json::json!({"ok": false, "error": "no such pane"}),
                        Some(p) => {
                            let title =
                                rt.titles.get(&pane).cloned().unwrap_or_default();
                            match p.agent.and_then(|a| crate::detect::manifest_for(&manifests, a)) {
                                Some(m) => {
                                    let lines = p.emu.bottom_text(15);
                                    let ex = crate::detect::classify_explain(m, &title, &lines);
                                    serde_json::json!({
                                        "ok": true,
                                        "agent": p.agent,
                                        "effective_status": p.effective_status().word(),
                                        "explain": ex,
                                    })
                                }
                                None => serde_json::json!({
                                    "ok": true,
                                    "agent": p.agent,
                                    "effective_status": p.effective_status().word(),
                                    "explain": null,
                                    "note": "no manifest for this pane — activity heuristic only",
                                }),
                            }
                        }
                    };
                    let _ = reply.send(v);
                }
                api::Req::Shutdown => {
                    rt.save_session();
                    let _ = reply.send(serde_json::json!({"ok": true}));
                    shutdown_clients(&clients);
                    flush_writers().await;
                    return Ok(RunOutcome::Exit);
                }
                api::Req::Handoff => {
                    rt.save_session();
                    let h = runtime::capture_handoff(&rt, area);
                    // Persist BEFORE acking: a write failure (full disk) at
                    // the point of no return would kill every pane. Refuse
                    // the handoff instead and keep serving.
                    let written = runtime::handoff_path()
                        .ok_or_else(|| "no state dir".to_string())
                        .and_then(|p| {
                            serde_json::to_vec(&h)
                                .map_err(|e| e.to_string())
                                .and_then(|json| std::fs::write(&p, json).map_err(|e| e.to_string()))
                        });
                    match written {
                        Ok(()) => {
                            let _ = reply.send(serde_json::json!({"ok": true}));
                            flush_writers().await; // let the reply reach the CLI
                            // Leak the runtime ON PURPOSE: dropping it closes
                            // the original pty masters, and on macOS a master
                            // close hangs up the slave even while our
                            // handoff dups are open — idle shells read EOF
                            // and exit before the heir can adopt them. exec
                            // replaces the whole image; nothing needs Drop.
                            std::mem::forget(rt);
                            return Ok(RunOutcome::Handoff(Box::new(h)));
                        }
                        Err(e) => {
                            // Undo capture: the dup'd masters have no CLOEXEC
                            // and would leak into every future child.
                            for hp in &h.panes {
                                unsafe { libc::close(hp.fd) };
                            }
                            if let Some(p) = runtime::handoff_path() {
                                let _ = std::fs::remove_file(p);
                            }
                            tracing::warn!(error = %e, "handoff refused: cannot persist state");
                            let _ = reply.send(serde_json::json!({"ok": false, "error": e}));
                        }
                    }
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
                    // Plugin [[hooks]]: fire-and-forget shell on AGENT status
                    // changes only — plain shells flap Working/Idle on every
                    // output pause and would storm the hooks.
                    let damped = hook_last
                        .get(&ch.pane)
                        .is_some_and(|t| t.elapsed() < Duration::from_secs(5));
                    if ch.agent.is_some() && !damped {
                        hook_last.insert(ch.pane, std::time::Instant::now());
                        for cmd in crate::plugin::event_hooks(&plugins, ch.to.word()) {
                            let mut c = std::process::Command::new("/bin/sh");
                            c.args(["-c", &cmd])
                                .env("CDOCK_PANE", ch.pane.0.to_string())
                                .env("CDOCK_STATUS", ch.to.word());
                            spawn_and_reap(c); // reaped — a bare spawn leaks zombies
                        }
                    }
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
                    render_frame(&mut rt, &mut term, area, &mut clients, &mut frame_cache)?;
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

/// Spawn a fire-and-forget helper and reap it — a dropped Child is never
/// waited on, and every unreaped exit is a zombie that survives handoffs.
fn spawn_and_reap(mut cmd: std::process::Command) {
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if let Ok(mut child) = cmd.spawn() {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
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
            let mut cmd = std::process::Command::new("afplay");
            cmd.arg(file);
            spawn_and_reap(cmd);
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
            let mut cmd = std::process::Command::new("osascript");
            cmd.args(["-e", &script]);
            spawn_and_reap(cmd);
        }
        #[cfg(target_os = "linux")]
        {
            let mut cmd = std::process::Command::new("notify-send");
            cmd.args(["cdock", &text]);
            spawn_and_reap(cmd);
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

/// After a client leaves, re-fit the shared area to a survivor — otherwise
/// the remaining client renders at the departed one's size forever.
fn refit_area(
    area: &mut Rect,
    term: &mut Terminal<TestBackend>,
    clients: &mut HashMap<ClientId, Client>,
    cache: &mut FrameCache,
    rt: &mut Runtime,
) -> io::Result<()> {
    if let Some((cols, rows)) = clients.values().map(|c| c.size).find(|s| s.0 > 0) {
        resize_all(area, term, clients, cache, cols, rows)?;
        rt.mark_dirty();
    }
    Ok(())
}

fn resize_all(
    area: &mut Rect,
    term: &mut Terminal<TestBackend>,
    clients: &mut HashMap<ClientId, Client>,
    cache: &mut FrameCache,
    cols: u16,
    rows: u16,
) -> io::Result<()> {
    let new = Rect::new(0, 0, cols.max(4), rows.max(4));
    if *area != new {
        *area = new;
        *term = Terminal::new(TestBackend::new(new.width, new.height)).expect("test backend is infallible");
        cache.prev = None;
        for c in clients.values_mut() {
            c.needs_full = true; // full repaint at the new size
        }
    }
    Ok(())
}

/// Shared previous frame: all clients see the same area, so one diff and
/// one stored buffer serve everyone (cloning per client per 16ms tick was
/// the loop's largest constant cost).
struct FrameCache {
    prev: Option<Buffer>,
}

fn render_frame(
    rt: &mut Runtime,
    term: &mut Terminal<TestBackend>,
    area: Rect,
    clients: &mut HashMap<ClientId, Client>,
    cache: &mut FrameCache,
) -> io::Result<()> {
    let view = ui::compute_view(rt, area);
    term.draw(|f| ui::render(&view, rt, f)).expect("test backend is infallible");
    let curr = term.backend().buffer().clone();
    let cursor = ui::cursor_for(&view, rt);

    let any_full = clients.values().any(|c| c.needs_full);
    let diff = ansi_diff(cache.prev.as_ref(), &curr, cursor);
    let full = if any_full { ansi_diff(None, &curr, cursor) } else { Vec::new() };
    for c in clients.values_mut() {
        let bytes = if c.needs_full { &full } else { &diff };
        c.needs_full = false;
        if !bytes.is_empty() {
            let _ = c.tx.send(ServerMsg::Frame(bytes.clone()));
        }
    }
    cache.prev = Some(curr);
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
