//! Thin client: owns the host terminal, forwards parsed input to the server,
//! blits the server's pre-diffed ANSI frames to stdout. Fully synchronous.

use std::io::{self, Write};
use std::os::unix::net::UnixStream;

use crate::proto::{self, ClientMsg, PROTOCOL_VERSION, ServerMsg};

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
/// server detaches us or shuts down.
pub fn run(stream: UnixStream) -> io::Result<()> {
    let mut writer = stream.try_clone()?;
    let mut reader = stream;

    let (cols, rows) = crossterm::terminal::size()?;
    proto::write_msg(&mut writer, &ClientMsg::Hello { version: PROTOCOL_VERSION, cols, rows })?;

    setup_terminal()?;
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        default_hook(info);
    }));

    // Frames from the server go straight to stdout; on Detach/Shutdown we
    // restore the terminal and end the whole process (the input thread is
    // parked in a blocking crossterm read).
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
                Ok(ServerMsg::Shutdown) | Err(_) => {
                    restore_terminal();
                    println!("cdock: session ended");
                    std::process::exit(0);
                }
            }
        }
    });

    loop {
        match crossterm::event::read() {
            Ok(ev) => {
                if proto::write_msg(&mut writer, &ClientMsg::Event(ev)).is_err() {
                    break;
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "input read failed");
                let _ = proto::write_msg(&mut writer, &ClientMsg::Detach);
                break;
            }
        }
    }
    let _ = reader_thread.join();
    restore_terminal();
    Ok(())
}
