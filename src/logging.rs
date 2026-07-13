use std::path::PathBuf;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;

/// Dev namespace: invoked as `cdock-dev` (a symlink to the real binary) or
/// with CDOCK_DEV=1, cdock keeps its state, sockets, and snapshot fully
/// apart from the production install — dev runs cannot touch the real
/// session. argv[0], not current_exe: exe resolution follows the symlink.
pub fn dev_mode() -> bool {
    if std::env::var_os("CDOCK_DEV").is_some() {
        return true;
    }
    std::env::args()
        .next()
        .map(|a| a.rsplit('/').next().unwrap_or(&a).to_string())
        .is_some_and(|s| s.ends_with("-dev"))
}

fn app_dir() -> &'static str {
    if dev_mode() { "comind-dock-dev" } else { "comind-dock" }
}

/// State directory: `$XDG_STATE_HOME/<app>` or `~/.local/state/<app>`,
/// where `<app>` is comind-dock or comind-dock-dev (see `dev_mode`).
pub fn state_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_STATE_HOME") {
        let dir = PathBuf::from(dir);
        if dir.is_absolute() {
            return Some(dir.join(app_dir()));
        }
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state").join(app_dir()))
}

/// Clamp to owner-only permissions. The state dir holds session snapshots,
/// screen tails, and the control sockets — API access means full PTY
/// control — so nothing in it may carry group/other bits. Applied on every
/// boot: it also repairs dirs created by older builds under a loose umask.
pub fn owner_only(path: &std::path::Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

/// Initialize file logging. Filter via `CDOCK_LOG` (tracing env-filter syntax), default `info`.
/// Returns a guard that must stay alive for the non-blocking writer to flush.
pub fn init() -> std::io::Result<WorkerGuard> {
    let dir = state_dir().ok_or_else(|| {
        std::io::Error::other("cannot determine state dir (HOME and XDG_STATE_HOME unset)")
    })?;
    std::fs::create_dir_all(&dir)?;
    owner_only(&dir, 0o700)?;

    let appender = tracing_appender::rolling::daily(&dir, "cdock.log");
    let (writer, guard) = tracing_appender::non_blocking(appender);

    let filter = EnvFilter::try_from_env("CDOCK_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).with_writer(writer).with_ansi(false).init();
    Ok(guard)
}

#[cfg(test)]
mod tests {
    #[test]
    fn owner_only_strips_group_other_bits() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("cdock-perm-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        super::owner_only(&dir, 0o700).unwrap();
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
