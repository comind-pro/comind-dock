use std::path::PathBuf;

/// Current working directory of a process (the pane's shell): the space
/// follows `cd`. Linux: /proc. macOS: libproc's PROC_PIDVNODEPATHINFO.
#[cfg(target_os = "linux")]
pub fn process_cwd(pid: u32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
}

#[cfg(target_os = "macos")]
pub fn process_cwd(pid: u32) -> Option<PathBuf> {
    use std::os::raw::{c_int, c_void};

    // proc_pidinfo(pid, PROC_PIDVNODEPATHINFO, 0, &buf, size) fills two
    // vnode_info_path structs (cwd, root). Each is a 152-byte vnode_info
    // followed by a MAXPATHLEN (1024) path. Stable public libproc ABI.
    const PROC_PIDVNODEPATHINFO: c_int = 9;
    const VNODE_INFO_SIZE: usize = 152;
    const MAXPATHLEN: usize = 1024;
    const SIZE: usize = 2 * (VNODE_INFO_SIZE + MAXPATHLEN);

    unsafe extern "C" {
        fn proc_pidinfo(
            pid: c_int,
            flavor: c_int,
            arg: u64,
            buffer: *mut c_void,
            buffersize: c_int,
        ) -> c_int;
    }

    let mut buf = [0u8; SIZE];
    let n = unsafe {
        proc_pidinfo(
            pid as c_int,
            PROC_PIDVNODEPATHINFO,
            0,
            buf.as_mut_ptr() as *mut c_void,
            SIZE as c_int,
        )
    };
    if n <= 0 {
        return None;
    }
    let path_bytes = &buf[VNODE_INFO_SIZE..VNODE_INFO_SIZE + MAXPATHLEN];
    let end = path_bytes.iter().position(|&b| b == 0)?;
    if end == 0 {
        return None;
    }
    Some(PathBuf::from(String::from_utf8_lossy(&path_bytes[..end]).into_owned()))
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    #[test]
    fn own_cwd_readable() {
        let cwd = super::process_cwd(std::process::id()).expect("own process cwd");
        assert_eq!(cwd, std::env::current_dir().unwrap());
    }
}
