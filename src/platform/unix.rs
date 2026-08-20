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

/// Executable path (fallback: name) of one process — the pane's direct
/// child may itself be the agent (direct spawn, no shell in between).
#[cfg(target_os = "linux")]
pub fn process_ident(pid: u32) -> Option<String> {
    std::fs::read_link(format!("/proc/{pid}/exe"))
        .map(|p| p.to_string_lossy().into_owned())
        .or_else(|_| std::fs::read_to_string(format!("/proc/{pid}/comm")))
        .ok()
        .map(|s| s.trim().to_string())
}

#[cfg(target_os = "macos")]
pub fn process_ident(pid: u32) -> Option<String> {
    use std::os::raw::{c_int, c_void};
    const PROC_PIDPATHINFO_MAXSIZE: usize = 4096;
    unsafe extern "C" {
        fn proc_pidpath(pid: c_int, buffer: *mut c_void, buffersize: u32) -> c_int;
        fn proc_name(pid: c_int, buffer: *mut c_void, buffersize: u32) -> c_int;
    }
    let mut buf = [0u8; PROC_PIDPATHINFO_MAXSIZE];
    let mut len =
        unsafe { proc_pidpath(pid as c_int, buf.as_mut_ptr() as *mut c_void, buf.len() as u32) };
    if len <= 0 {
        len = unsafe { proc_name(pid as c_int, buf.as_mut_ptr() as *mut c_void, 64) };
    }
    (len > 0).then(|| String::from_utf8_lossy(&buf[..len as usize]).into_owned())
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
        if libc::sysctl(
            mib.as_mut_ptr(),
            3,
            std::ptr::null_mut(),
            &mut size,
            std::ptr::null_mut(),
            0,
        ) != 0
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

/// Every live process cdock spawned (via a shell or directly), attributed
/// to the pane whose CDOCK_PANE_ID it inherited: the process-monitor view.
/// Scans every pid on the box — proc_listpids has no "just cdock's tree"
/// filter — but the per-pid work (one sysctl to check the tag) is cheap,
/// and the two proc_pidinfo calls only run for pids that already matched.
// ponytail: caller (runtime::refresh_monitor) is SDD process-monitor Task 3.
#[cfg(target_os = "macos")]
#[allow(dead_code)]
pub fn cdock_processes() -> Vec<crate::platform::ProcInfo> {
    use crate::platform::ProcInfo;
    use crate::state::ids::PaneId;
    use std::os::raw::c_void;
    use std::time::{Duration, UNIX_EPOCH};

    const PROC_ALL_PIDS: u32 = 1;

    // Size the buffer from a first zero-length call, doubled for headroom
    // (pids created between the sizing call and the real one).
    let first = unsafe { libc::proc_listpids(PROC_ALL_PIDS, 0, std::ptr::null_mut(), 0) };
    if first <= 0 {
        return Vec::new();
    }
    let cap = ((first as usize / std::mem::size_of::<i32>()) * 2).max(64);
    let mut pids = vec![0i32; cap];
    let bytes = unsafe {
        libc::proc_listpids(
            PROC_ALL_PIDS,
            0,
            pids.as_mut_ptr() as *mut c_void,
            std::mem::size_of_val(pids.as_slice()) as libc::c_int,
        )
    };
    if bytes <= 0 {
        return Vec::new();
    }
    let n = (bytes as usize / std::mem::size_of::<i32>()).min(pids.len());

    // Only THIS session's processes: a dev server must not list — let alone
    // kill — prod panes and vice versa, and pane ids collide across namespaces
    // (dev %16 ≠ prod %16). Match the process's namespace env to our own.
    let self_dev = std::env::var("CDOCK_DEV").ok();
    let self_session = std::env::var("CDOCK_SESSION").ok();

    pids[..n]
        .iter()
        .filter(|p| **p > 0)
        .filter_map(|&raw_pid| {
            let pid = raw_pid as u32;
            // pty.rs sets CDOCK_PANE_ID from PaneId's Display, which is "%N"
            // (the "%3"/"3" pane syntax) — strip the marker before parsing.
            let pane: u64 =
                process_env_var(pid, "CDOCK_PANE_ID")?.trim_start_matches('%').parse().ok()?;
            if process_env_var(pid, "CDOCK_DEV").as_deref() != self_dev.as_deref() {
                return None;
            }
            if process_env_var(pid, "CDOCK_SESSION").as_deref() != self_session.as_deref() {
                return None;
            }

            let mut ti: libc::proc_taskinfo = unsafe { std::mem::zeroed() };
            let ti_size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
            let ti_written = unsafe {
                libc::proc_pidinfo(
                    raw_pid,
                    libc::PROC_PIDTASKINFO,
                    0,
                    &mut ti as *mut _ as *mut c_void,
                    ti_size,
                )
            };
            if ti_written <= 0 {
                return None; // zombie/race: pid exited between listpids and here
            }

            let mut bi: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
            let bi_size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
            let bi_written = unsafe {
                libc::proc_pidinfo(
                    raw_pid,
                    libc::PROC_PIDTBSDINFO,
                    0,
                    &mut bi as *mut _ as *mut c_void,
                    bi_size,
                )
            };
            if bi_written <= 0 {
                return None;
            }

            Some(ProcInfo {
                pid,
                ppid: bi.pbi_ppid,
                pane: PaneId(pane),
                cmd: process_ident(pid).unwrap_or_default(),
                start: UNIX_EPOCH
                    + Duration::new(bi.pbi_start_tvsec, (bi.pbi_start_tvusec * 1000) as u32),
                cpu_ns: ti.pti_total_user + ti.pti_total_system,
                rss: ti.pti_resident_size,
            })
        })
        .collect()
}

/// System-wide CPU% and memory snapshot alongside `cdock_processes`. Memory
/// is one `host_statistics64` call; CPU% is a delta against the previous
/// sample cached across calls (a single snapshot only gives cumulative ticks
/// since boot, not a live rate) — `None` on the first call, before there is a
/// prior sample to diff against. No sleeping: the ~2 s poll cadence IS the
/// interval.
// ponytail: caller (runtime::refresh_monitor) is SDD process-monitor Task 3.
#[cfg(target_os = "macos")]
#[allow(deprecated)] // mach_host_self: libc's only handle, no mach2 dep here
#[allow(dead_code)]
pub fn system_load() -> crate::platform::SystemLoad {
    use crate::platform::SystemLoad;

    let mem_total: u64 = unsafe {
        let mut mib = [libc::CTL_HW, libc::HW_MEMSIZE];
        let mut total: u64 = 0;
        let mut size = std::mem::size_of::<u64>();
        if libc::sysctl(
            mib.as_mut_ptr(),
            2,
            &mut total as *mut _ as *mut libc::c_void,
            &mut size,
            std::ptr::null_mut(),
            0,
        ) == 0
        {
            total
        } else {
            0
        }
    };

    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(0) as u64;

    let mem_used: u64 = unsafe {
        let mut vmstat: libc::vm_statistics64 = std::mem::zeroed();
        let mut count = libc::HOST_VM_INFO64_COUNT;
        let kr = libc::host_statistics64(
            libc::mach_host_self(),
            libc::HOST_VM_INFO64,
            &mut vmstat as *mut _ as libc::host_info64_t,
            &mut count,
        );
        if kr == libc::KERN_SUCCESS {
            (vmstat.active_count as u64
                + vmstat.wire_count as u64
                + vmstat.compressor_page_count as u64)
                * page_size
        } else {
            0
        }
    };

    SystemLoad { cpu_pct: cpu_load_pct(), mem_used, mem_total }
}

/// Busy/total tick counters from every CPU, summed. Two calls in
/// `system_load` diff these into a percentage; a single sample can't (it's
/// cumulative since boot).
#[cfg(target_os = "macos")]
#[allow(deprecated)] // mach_host_self/mach_task_self_: libc's only handle, no mach2 dep here
fn cpu_ticks_sample() -> Option<(u64, u64)> {
    unsafe {
        let mut num_cpus: libc::natural_t = 0;
        let mut info: libc::processor_info_array_t = std::ptr::null_mut();
        let mut info_count: libc::mach_msg_type_number_t = 0;
        let kr = libc::host_processor_info(
            libc::mach_host_self(),
            libc::PROCESSOR_CPU_LOAD_INFO,
            &mut num_cpus,
            &mut info,
            &mut info_count,
        );
        if kr != libc::KERN_SUCCESS || info.is_null() {
            return None;
        }

        let loads = std::slice::from_raw_parts(
            info as *const libc::processor_cpu_load_info,
            num_cpus as usize,
        );
        let (mut busy, mut total) = (0u64, 0u64);
        for l in loads {
            let user = l.cpu_ticks[libc::CPU_STATE_USER as usize] as u64;
            let sys = l.cpu_ticks[libc::CPU_STATE_SYSTEM as usize] as u64;
            let nice = l.cpu_ticks[libc::CPU_STATE_NICE as usize] as u64;
            let idle = l.cpu_ticks[libc::CPU_STATE_IDLE as usize] as u64;
            busy += user + sys + nice;
            total += user + sys + nice + idle;
        }

        // host_processor_info hands back kernel-owned (vm_allocate'd)
        // memory; must be explicitly freed or every sample leaks it.
        #[allow(deprecated)] // mach_task_self_: libc's only handle, no mach2 dep here
        let this_task = libc::mach_task_self_;
        libc::vm_deallocate(
            this_task,
            info as libc::vm_address_t,
            info_count as libc::vm_size_t
                * std::mem::size_of::<libc::integer_t>() as libc::vm_size_t,
        );

        Some((busy, total))
    }
}

/// CPU percent since the *previous* call, not a fresh two-sample read: the
/// old version slept 50ms between samples, which — called from
/// `refresh_monitor` on the server's `tokio::select!` poll tick — froze all
/// pane I/O for that 50ms every ~2s the monitor overlay was open (the same
/// main-loop-blocking bug class as the pre-v0.6.5 PTY-write deadlock). The
/// caller already polls ~2s apart, a real delta, so cache the last sample
/// here instead of sleeping for one. First call (no prior sample, e.g. right
/// after the overlay opens) returns `None`, rendered as "—".
#[cfg(target_os = "macos")]
fn cpu_load_pct() -> Option<f32> {
    static PREV: std::sync::Mutex<Option<(u64, u64)>> = std::sync::Mutex::new(None);
    let sample = cpu_ticks_sample()?;
    let mut prev = PREV.lock().unwrap();
    let (prev_busy, prev_total) = prev.replace(sample)?;
    let d_busy = sample.0.saturating_sub(prev_busy);
    let d_total = sample.1.saturating_sub(prev_total);
    if d_total == 0 {
        return Some(0.0);
    }
    Some((d_busy as f32 / d_total as f32) * 100.0)
}

// Linux/other-unix stubs so the #[cfg(unix)] re-export in mod.rs resolves on
// every target. Real /proc bodies land in SDD process-monitor Task 2.
#[cfg(all(unix, not(target_os = "macos")))]
#[allow(dead_code)]
pub fn cdock_processes() -> Vec<crate::platform::ProcInfo> {
    Vec::new()
}
#[cfg(all(unix, not(target_os = "macos")))]
#[allow(dead_code)]
pub fn system_load() -> crate::platform::SystemLoad {
    crate::platform::SystemLoad::default()
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

    #[test]
    fn cdock_processes_sees_a_child_tagged_with_pane_env() {
        // A child that inherits CDOCK_PANE_ID must be attributed to that pane.
        //
        // NOT `sleep`: on current macOS, the kernel redacts environment
        // variables from `sysctl(KERN_PROCARGS2)` for Apple-signed "platform
        // binaries" (everything in /bin, /usr/bin — verified via `codesign
        // -dv /bin/sleep` showing `Platform identifier=26`) from ANY
        // external reader, including the spawning parent — confirmed with
        // `ps eww` on a real `sleep &` job showing no env either. Root is
        // needed to see it; this test runs unprivileged. A Homebrew-built
        // python3 is ad-hoc signed (no platform flag), so its env is
        // visible like any ordinary (non-Apple) binary — which is what
        // cdock actually needs to inspect (agent CLIs, not /bin/sleep).
        // Real pty env is "%N" (PaneId Display), not "N" — attribution must
        // strip the marker. Using the real format so this test would catch it.
        let mut child = std::process::Command::new("python3")
            .args(["-c", "import time; time.sleep(5)"])
            .env("CDOCK_PANE_ID", "%7")
            .spawn()
            .expect("spawn python3");
        std::thread::sleep(std::time::Duration::from_millis(80));
        let procs = super::cdock_processes();
        let _ = child.kill();
        let _ = child.wait();
        let mine = procs.iter().find(|p| p.pid == child.id());
        let p = mine.expect("tagged child is listed");
        assert_eq!(p.pane.0, 7, "attributed to CDOCK_PANE_ID");
        assert!(p.cmd.contains("python3") || p.cmd.contains("Python"), "cmd captured: {}", p.cmd);
        assert!(p.rss > 0, "rss captured");
    }

    #[test]
    fn system_load_is_populated() {
        let l = super::system_load();
        assert!(l.mem_total > 0, "mem_total read");
        assert!(l.mem_used <= l.mem_total);
    }
}

/// Does the system clipboard hold an image (rather than text)?
/// Cheap: one `osascript` call, and only on the empty-paste path — a Cmd+V
/// with text never reaches here.
/// ponytail: macOS only; X11/Wayland would need xclip/wl-paste probing.
#[cfg(target_os = "macos")]
pub fn clipboard_has_image() -> bool {
    let Ok(out) = std::process::Command::new("osascript").args(["-e", "clipboard info"]).output()
    else {
        return false;
    };
    let info = String::from_utf8_lossy(&out.stdout);
    info.contains("PNGf") || info.contains("TIFF") || info.contains("JPEG")
}

#[cfg(not(target_os = "macos"))]
pub fn clipboard_has_image() -> bool {
    false
}
