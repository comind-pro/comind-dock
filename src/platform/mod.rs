//! Platform boundary: OS-specific bodies live in per-OS files, compile-gated.
//! Core modules contain no OS conditionals (ARCHITECTURE.md §7).

#[cfg(unix)]
mod unix;

#[cfg(unix)]
pub use unix::{child_process_idents, process_cwd};

#[cfg(not(unix))]
pub fn process_cwd(_pid: u32) -> Option<std::path::PathBuf> {
    None
}

#[cfg(not(unix))]
pub fn child_process_idents(_pid: u32) -> Vec<String> {
    Vec::new()
}
