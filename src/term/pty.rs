use std::io::{self, Read, Write};

use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use tokio::sync::mpsc::UnboundedSender;

use crate::runtime::event::AppEvent;
use crate::state::ids::PaneId;

pub struct Pty {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
}

fn err(e: impl std::fmt::Display) -> io::Error {
    io::Error::other(e.to_string())
}

/// How to launch the process in a new pane.
pub struct SpawnOpts {
    pub shell: String,
    pub login: bool,
    pub cwd: std::path::PathBuf,
    /// Some → run `shell -c command` instead of an interactive shell.
    pub command: Option<String>,
    pub tab_id: String,
    pub workspace_id: String,
}

/// Spawn a pane process in a new PTY. A detached reader thread pumps output
/// bytes into the app event loop; child exit is reported as `PtyExit`.
pub fn spawn_shell(
    pane: PaneId,
    cols: u16,
    rows: u16,
    tx: UnboundedSender<AppEvent>,
    data_tx: tokio::sync::mpsc::Sender<crate::runtime::event::PtyData>,
    opts: &SpawnOpts,
) -> io::Result<Pty> {
    let size = PtySize { rows, cols, pixel_width: 0, pixel_height: 0 };
    let pair = native_pty_system().openpty(size).map_err(err)?;

    let mut cmd = CommandBuilder::new(&opts.shell);
    if let Some(command) = &opts.command {
        cmd.args(["-c", command]);
    } else if opts.login {
        cmd.arg("-l");
    }
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    cmd.env("CDOCK_ENV", "1");
    cmd.env("CDOCK_PANE_ID", pane.to_string());
    cmd.env("CDOCK_TAB_ID", &opts.tab_id);
    cmd.env("CDOCK_WORKSPACE_ID", &opts.workspace_id);
    cmd.cwd(&opts.cwd);

    let mut child = pair.slave.spawn_command(cmd).map_err(err)?;
    drop(pair.slave);
    let killer = child.clone_killer();

    let mut reader = pair.master.try_clone_reader().map_err(err)?;
    let writer = pair.master.take_writer().map_err(err)?;

    // The child's exit — not master EOF — closes the pane: leftover background
    // processes can hold the slave fd open long after the shell is gone.
    let exit_tx = tx.clone();
    std::thread::spawn(move || {
        let status = child.wait();
        tracing::debug!(%pane, ?status, "pty child exited");
        let _ = exit_tx.send(AppEvent::PtyExit(pane));
    });

    std::thread::spawn(move || {
        let mut buf = [0u8; 65536];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => {
                    tracing::debug!(%pane, "pty reader EOF");
                    let _ = tx.send(AppEvent::PtyExit(pane));
                    break;
                }
                Err(e) => {
                    tracing::debug!(%pane, error = %e, "pty reader error");
                    let _ = tx.send(AppEvent::PtyExit(pane));
                    break;
                }
                Ok(n) => {
                    // Bounded: blocks when the main loop is behind, letting
                    // the kernel pty buffer throttle the child (backpressure).
                    if data_tx.blocking_send((pane, buf[..n].to_vec())).is_err() {
                        break;
                    }
                }
            }
        }
    });

    Ok(Pty { master: pair.master, writer, killer })
}

impl Pty {
    pub fn write(&mut self, bytes: &[u8]) {
        if let Err(e) = self.writer.write_all(bytes).and_then(|_| self.writer.flush()) {
            tracing::warn!(error = %e, "pty write failed");
        }
    }

    pub fn resize(&self, cols: u16, rows: u16) {
        let size = PtySize { rows, cols, pixel_width: 0, pixel_height: 0 };
        if let Err(e) = self.master.resize(size) {
            tracing::warn!(error = %e, "pty resize failed");
        }
    }

    pub fn kill(&mut self) {
        let _ = self.killer.kill();
    }
}
