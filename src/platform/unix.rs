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

/// (pid, executable path — fallback name) of a process's direct children:
/// how a pane knows an agent CLI runs inside its shell, and which pid to ask
/// about the agent's environment. Full paths, not p_comm: Claude Code execs
/// a version-named binary ("2.1.206"), only its path still says "claude".
#[cfg(target_os = "linux")]
pub fn child_process_idents(pid: u32) -> Vec<(u32, String)> {
    let Ok(kids) = std::fs::read_to_string(format!("/proc/{pid}/task/{pid}/children")) else {
        return Vec::new();
    };
    kids.split_whitespace()
        .filter_map(|c| {
            let cpid: u32 = c.parse().ok()?;
            let ident = std::fs::read_link(format!("/proc/{c}/exe"))
                .map(|p| p.to_string_lossy().into_owned())
                .or_else(|_| std::fs::read_to_string(format!("/proc/{c}/comm")))
                .ok()?;
            Some((cpid, ident.trim().to_string()))
        })
        .collect()
}

#[cfg(target_os = "macos")]
pub fn child_process_idents(pid: u32) -> Vec<(u32, String)> {
    use std::os::raw::{c_int, c_void};

    // proc_listpids(PROC_PPID_ONLY, ppid) → child pids; proc_pidpath → exe
    // path; proc_name as fallback. Stable public libproc ABI;
    // proc_listpids returns bytes written.
    const PROC_PPID_ONLY: u32 = 6;
    const PROC_PIDPATHINFO_MAXSIZE: usize = 4096;
    unsafe extern "C" {
        fn proc_listpids(t: u32, typeinfo: u32, buffer: *mut c_void, buffersize: c_int) -> c_int;
        fn proc_pidpath(pid: c_int, buffer: *mut c_void, buffersize: u32) -> c_int;
        fn proc_name(pid: c_int, buffer: *mut c_void, buffersize: u32) -> c_int;
    }

    let mut pids = [0i32; 64];
    let bytes = unsafe {
        proc_listpids(
            PROC_PPID_ONLY,
            pid,
            pids.as_mut_ptr() as *mut c_void,
            std::mem::size_of_val(&pids) as c_int,
        )
    };
    if bytes <= 0 {
        return Vec::new();
    }
    let n = (bytes as usize / std::mem::size_of::<i32>()).min(pids.len());
    pids[..n]
        .iter()
        .filter(|p| **p > 0)
        .filter_map(|&p| {
            let mut buf = [0u8; PROC_PIDPATHINFO_MAXSIZE];
            let mut len =
                unsafe { proc_pidpath(p, buf.as_mut_ptr() as *mut c_void, buf.len() as u32) };
            if len <= 0 {
                len = unsafe { proc_name(p, buf.as_mut_ptr() as *mut c_void, 64) };
            }
            (len > 0)
                .then(|| (p as u32, String::from_utf8_lossy(&buf[..len as usize]).into_owned()))
        })
        .collect()
}

/// One environment variable of a live process — how a pane learns which
/// CLAUDE_CONFIG_DIR its agent runs under (profiles are config dirs).
#[cfg(target_os = "linux")]
pub fn process_env_var(pid: u32, key: &str) -> Option<String> {
    let raw = std::fs::read(format!("/proc/{pid}/environ")).ok()?;
    let prefix = format!("{key}=");
    raw.split(|b| *b == 0)
        .filter_map(|c| std::str::from_utf8(c).ok())
        .find_map(|c| c.strip_prefix(&prefix).map(str::to_string))
}

#[cfg(target_os = "macos")]
pub fn process_env_var(pid: u32, key: &str) -> Option<String> {
    // sysctl KERN_PROCARGS2: argc, exec path, argv, then the environment as
    // NUL-separated KEY=VALUE strings. We scan every chunk for the prefix —
    // an argv element spoofing "KEY=" is theoretical for our keys.
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid as libc::c_int];
    let mut size: libc::size_t = 0;
    unsafe {
        if libc::sysctl(mib.as_mut_ptr(), 3, std::ptr::null_mut(), &mut size, std::ptr::null_mut(), 0)
            != 0
        {
            return None;
        }
        let mut buf = vec![0u8; size];
        if libc::sysctl(
            mib.as_mut_ptr(),
            3,
            buf.as_mut_ptr() as *mut libc::c_void,
            &mut size,
            std::ptr::null_mut(),
            0,
        ) != 0
        {
            return None;
        }
        buf.truncate(size);
        let prefix = format!("{key}=");
        buf[4.min(buf.len())..]
            .split(|b| *b == 0)
            .filter_map(|c| std::str::from_utf8(c).ok())
            .find_map(|c| c.strip_prefix(&prefix).map(str::to_string))
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    #[test]
    fn own_cwd_readable() {
        let cwd = super::process_cwd(std::process::id()).expect("own process cwd");
        assert_eq!(cwd, std::env::current_dir().unwrap());
    }

    #[test]
    fn children_visible_by_path() {
        let mut child = std::process::Command::new("sleep").arg("5").spawn().expect("spawn sleep");
        std::thread::sleep(std::time::Duration::from_millis(50));
        let idents = super::child_process_idents(std::process::id());
        let _ = child.kill();
        let _ = child.wait();
        // Full exe path, so word-matching sees every path segment; pid rides along.
        assert!(
            idents.iter().any(|(pid, n)| *pid > 0 && n.ends_with("/sleep")),
            "children: {idents:?}"
        );
    }
}
