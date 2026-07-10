//! Thin client: owns the host terminal, forwards parsed input to the server,
//! blits the server's pre-diffed ANSI frames to stdout. Fully synchronous.
//! An unexpected disconnect (live handoff, server restart) triggers a quiet
//! reconnect loop; only an explicit Detach/Shutdown from the server — or a
//! reconnect window running dry — ends the process.

use std::io::{self, Write};
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::proto::{self, ClientMsg, PROTOCOL_VERSION, ServerMsg};

const RECONNECT_WINDOW: Duration = Duration::from_secs(10);

fn setup_terminal() -> io::Result<()> {
    crossterm::terminal::enable_raw_mode()?;
    crossterm::execute!(
        io::stdout(),
        crossterm::terminal::EnterAlternateScreen,
        crossterm::event::EnableBracketedPaste,
        crossterm::event::EnableMouseCapture,
        crossterm::cursor::Hide,
    )
}

fn restore_terminal() {
    let _ = crossterm::execute!(
        io::stdout(),
        crossterm::event::DisableMouseCapture,
        crossterm::event::DisableBracketedPaste,
        crossterm::terminal::LeaveAlternateScreen,
        crossterm::cursor::Show,
    );
    let _ = crossterm::terminal::disable_raw_mode();
}

/// Run the thin client over an established connection. Returns when the
/// server detaches us or shuts down. `folder` scopes the view (`cdock -f`)
/// and is re-sent with every Hello so reconnects keep the scope.
pub fn run(stream: UnixStream, folder: Option<std::path::PathBuf>) -> io::Result<()> {
    setup_terminal()?;
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        default_hook(info);
    }));

    // Host input parsed once for the process lifetime; sessions come and go.
    let (in_tx, in_rx) = std::sync::mpsc::channel::<crossterm::event::Event>();
    std::thread::spawn(move || {
        loop {
            match crossterm::event::read() {
                Ok(ev) => {
                    if in_tx.send(ev).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "input read failed");
                    break;
                }
            }
        }
    });

    let mut stream = Some(stream);
    loop {
        let s = match stream.take() {
            Some(s) => s,
            None => match reconnect() {
                Some(s) => s,
                None => {
                    restore_terminal();
                    println!("cdock: session ended");
                    return Ok(());
                }
            },
        };
        session(s, &in_rx, &folder)?;
        // Unexpected disconnect — try again (Detach/Shutdown exit inside).
    }
}

/// One server connection: hello, frames out, input in. Returns on an
/// unexpected disconnect; exits the process on Detach/Shutdown.
fn session(
    stream: UnixStream,
    in_rx: &std::sync::mpsc::Receiver<crossterm::event::Event>,
    folder: &Option<std::path::PathBuf>,
) -> io::Result<()> {
    let mut writer = stream.try_clone()?;
    let mut reader = stream;

    let (cols, rows) = crossterm::terminal::size()?;
    proto::write_msg(
        &mut writer,
        &ClientMsg::Hello { version: PROTOCOL_VERSION, cols, rows, folder: folder.clone() },
    )?;

    let gone = Arc::new(AtomicBool::new(false));
    let gone_r = gone.clone();
    let reader_thread = std::thread::spawn(move || {
        let mut out = io::stdout();
        loop {
            match proto::read_msg::<ServerMsg>(&mut reader) {
                Ok(ServerMsg::Welcome { .. }) => {}
                Ok(ServerMsg::Frame(bytes)) => {
                    let _ = out.write_all(&bytes);
                    let _ = out.flush();
                }
                Ok(ServerMsg::Detach) => {
                    restore_terminal();
                    println!("cdock: detached (session keeps running)");
                    std::process::exit(0);
                }
                Ok(ServerMsg::Shutdown) => {
                    restore_terminal();
                    println!("cdock: session ended");
                    std::process::exit(0);
                }
                Err(_) => {
                    // Live handoff or crash — let the outer loop reconnect.
                    gone_r.store(true, Ordering::Relaxed);
                    break;
                }
            }
        }
    });

    loop {
        if gone.load(Ordering::Relaxed) {
            break;
        }
        match in_rx.recv_timeout(Duration::from_millis(50)) {
            Ok(ev) => {
                if proto::write_msg(&mut writer, &ClientMsg::Event(ev)).is_err() {
                    break;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                // Host input is gone (stdin closed) — detach cleanly.
                let _ = proto::write_msg(&mut writer, &ClientMsg::Detach);
                let _ = reader_thread.join();
                restore_terminal();
                return Ok(());
            }
        }
    }
    let _ = reader_thread.join();
    Ok(())
}

/// Poll the session socket until the replacement server answers.
fn reconnect() -> Option<UnixStream> {
    let sock = proto::socket_path()?;
    let deadline = Instant::now() + RECONNECT_WINDOW;
    while Instant::now() < deadline {
        if let Ok(s) = UnixStream::connect(&sock) {
            tracing::info!("reconnected after server handoff");
            return Some(s);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    None
}
