//! Platform boundary: OS-specific bodies live in per-OS files, compile-gated.
//! Core modules contain no OS conditionals (ARCHITECTURE.md §7).

#[cfg(unix)]
mod unix;

#[cfg(unix)]
pub use unix::{child_process_idents, process_cwd, process_env_var, process_ident};

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
