use std::path::PathBuf;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;

/// Log directory: `$XDG_STATE_HOME/comind-dock` or `~/.local/state/comind-dock`.
pub fn state_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_STATE_HOME") {
        let dir = PathBuf::from(dir);
        if dir.is_absolute() {
            return Some(dir.join("comind-dock"));
        }
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state/comind-dock"))
}

/// Initialize file logging. Filter via `CDOCK_LOG` (tracing env-filter syntax), default `info`.
/// Returns a guard that must stay alive for the non-blocking writer to flush.
pub fn init() -> std::io::Result<WorkerGuard> {
    let dir = state_dir().ok_or_else(|| {
        std::io::Error::other("cannot determine state dir (HOME and XDG_STATE_HOME unset)")
    })?;
    std::fs::create_dir_all(&dir)?;

    let appender = tracing_appender::rolling::daily(&dir, "cdock.log");
    let (writer, guard) = tracing_appender::non_blocking(appender);

    let filter = EnvFilter::try_from_env("CDOCK_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(writer)
        .with_ansi(false)
        .init();
    Ok(guard)
}
