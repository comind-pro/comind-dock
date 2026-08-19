//! Platform boundary: OS-specific bodies live in per-OS files, compile-gated.
//! Core modules contain no OS conditionals (ARCHITECTURE.md §7).

#[cfg(unix)]
mod unix;

#[cfg(unix)]
pub use unix::{
    child_process_idents, clipboard_has_image, process_cwd, process_env_var, process_ident,
};

// ponytail: cdock_processes/system_load land ahead of their caller — SDD
// process-monitor Task 3 (runtime::refresh_monitor) wires them up. Until
// then nothing in the binary calls them, hence the allow.
#[cfg(unix)]
#[allow(unused_imports)]
pub use unix::{cdock_processes, system_load};

/// One process cdock spawned (directly or via a shell), attributed to the
/// pane whose CDOCK_PANE_ID it inherited: the data behind a process-monitor
/// view.
// ponytail: fields read by SDD process-monitor Task 3, not yet in this task.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ProcInfo {
    pub pid: u32,
    pub ppid: u32,
    pub pane: crate::state::ids::PaneId,
    pub cmd: String,
    pub start: std::time::SystemTime,
    pub cpu_ns: u64,
    pub rss: u64,
}

/// System-wide CPU/memory snapshot alongside the per-pane process list.
#[allow(dead_code)] // ponytail: read by SDD process-monitor Task 3
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemLoad {
    pub cpu_pct: f32,
    pub mem_used: u64,
    pub mem_total: u64,
}

#[cfg(not(unix))]
pub fn process_cwd(_pid: u32) -> Option<std::path::PathBuf> {
    None
}

#[cfg(not(unix))]
pub fn child_process_idents(_pid: u32) -> Vec<(u32, String)> {
    Vec::new()
}

#[cfg(not(unix))]
pub fn process_env_var(_pid: u32, _key: &str) -> Option<String> {
    None
}

#[cfg(not(unix))]
pub fn process_ident(_pid: u32) -> Option<String> {
    None
}

#[cfg(not(unix))]
pub fn clipboard_has_image() -> bool {
    false
}

#[cfg(not(unix))]
pub fn cdock_processes() -> Vec<ProcInfo> {
    Vec::new()
}

#[cfg(not(unix))]
pub fn system_load() -> SystemLoad {
    SystemLoad::default()
}
