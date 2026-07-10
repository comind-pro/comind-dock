use std::io::{self, Read, Write};
use std::os::fd::RawFd;

use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use tokio::sync::mpsc::UnboundedSender;

use crate::runtime::event::AppEvent;
use crate::state::ids::PaneId;

/// Who owns the master side: a portable_pty pair we spawned, or a raw fd
/// inherited across a live-handoff exec.
enum Master {
    Spawned { master: Box<dyn MasterPty + Send>, killer: Box<dyn ChildKiller + Send + Sync> },
    Inherited { fd: RawFd },
}

pub struct Pty {
    master: Master,
    writer: Box<dyn Write + Send>,
    /// Shell pid — its cwd drives space cwd tracking.
    pub child_pid: Option<u32>,
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
    /// Extra environment (agent profiles).
    pub env: Vec<(String, String)>,
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
    // Absolute path for integration hooks — panes may not have cdock in PATH.
    if let Ok(exe) = std::env::current_exe() {
        cmd.env("CDOCK_BIN", exe);
    }
    for (k, v) in &opts.env {
        cmd.env(k, v);
    }
    cmd.cwd(&opts.cwd);

    let mut child = pair.slave.spawn_command(cmd).map_err(err)?;
    drop(pair.slave);
    let killer = child.clone_killer();
    let child_pid = child.process_id();

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

    Ok(Pty { master: Master::Spawned { master: pair.master, killer }, writer, child_pid })
}

/// Rebuild a Pty around a master fd inherited across a live-handoff exec.
/// The child keeps its pid (exec does not change ours, so it is still our
/// child): waitpid still reports its exit, signals still reach it.
pub fn adopt(
    pane: PaneId,
    fd: RawFd,
    child_pid: Option<u32>,
    tx: UnboundedSender<AppEvent>,
    data_tx: tokio::sync::mpsc::Sender<crate::runtime::event::PtyData>,
) -> io::Result<Pty> {
    use std::os::fd::FromRawFd;

    let dup = |fd: RawFd| -> io::Result<std::fs::File> {
        let d = unsafe { libc::dup(fd) };
        if d < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { std::fs::File::from_raw_fd(d) })
    };
    let mut reader = dup(fd)?;
    let writer = dup(fd)?;

    // Child exit via waitpid on the preserved pid (see spawn_shell: child
    // exit — not master EOF — closes the pane).
    if let Some(pid) = child_pid {
        let exit_tx = tx.clone();
        std::thread::spawn(move || {
            let mut status: libc::c_int = 0;
            let r = unsafe { libc::waitpid(pid as libc::pid_t, &mut status, 0) };
            tracing::debug!(%pane, pid, r, "adopted pty child exited");
            let _ = exit_tx.send(AppEvent::PtyExit(pane));
        });
    }

    std::thread::spawn(move || {
        let mut buf = [0u8; 65536];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => {
                    let _ = tx.send(AppEvent::PtyExit(pane));
                    break;
                }
                Ok(n) => {
                    if data_tx.blocking_send((pane, buf[..n].to_vec())).is_err() {
                        break;
                    }
                }
            }
        }
    });

    Ok(Pty { master: Master::Inherited { fd }, writer: Box::new(writer), child_pid })
}

impl Pty {
    pub fn write(&mut self, bytes: &[u8]) {
        if let Err(e) = self.writer.write_all(bytes).and_then(|_| self.writer.flush()) {
            tracing::warn!(error = %e, "pty write failed");
        }
    }

    pub fn resize(&self, cols: u16, rows: u16) {
        match &self.master {
            Master::Spawned { master, .. } => {
                let size = PtySize { rows, cols, pixel_width: 0, pixel_height: 0 };
                if let Err(e) = master.resize(size) {
                    tracing::warn!(error = %e, "pty resize failed");
                }
            }
            Master::Inherited { fd } => {
                let ws = libc::winsize { ws_row: rows, ws_col: cols, ws_xpixel: 0, ws_ypixel: 0 };
                if unsafe { libc::ioctl(*fd, libc::TIOCSWINSZ, &ws) } != 0 {
                    tracing::warn!(error = %io::Error::last_os_error(), "pty resize failed");
                }
            }
        }
    }

    pub fn kill(&mut self) {
        match &mut self.master {
            Master::Spawned { killer, .. } => {
                let _ = killer.kill();
            }
            Master::Inherited { .. } => {
                if let Some(pid) = self.child_pid {
                    unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
                }
            }
        }
    }

    /// Master fd prepared to survive exec: a dup (dup clears FD_CLOEXEC), so
    /// it outlives this Pty — dropping the original master must neither close
    /// the handoff fd nor hang up the slave before the exec happens.
    pub fn handoff_fd(&self) -> Option<RawFd> {
        let fd = match &self.master {
            Master::Spawned { master, .. } => master.as_raw_fd()?,
            Master::Inherited { fd } => *fd,
        };
        let dup = unsafe { libc::dup(fd) };
        (dup >= 0).then_some(dup)
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        if let Master::Inherited { fd } = self.master {
            unsafe { libc::close(fd) };
        }
    }
}
