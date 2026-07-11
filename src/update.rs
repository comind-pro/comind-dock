//! Update system (Phase 6): GitHub Releases as the feed. The server checks
//! for a newer tag in the background (menu shows "● update ready"); `cdock
//! update` downloads the platform tarball, verifies the sha256 when
//! published, atomically replaces the current binary, and with --handoff
//! execs the running server in place — no pane dies.
//! ponytail: latest-release only; stable/preview channels arrive when there
//! is something to channel. HTTP via the system curl — not worth a client
//! dependency.

use std::path::Path;

pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

fn curl(url: &str) -> Result<Vec<u8>, String> {
    let mut cmd = std::process::Command::new("curl");
    cmd.args(["-fsSL", "-m", "30"]);
    // Anonymous GitHub API caps at 60 req/h per IP — authenticate when a
    // token is around (GITHUB_TOKEN, else a logged-in gh CLI).
    if url.starts_with("https://api.github.com/")
        && let Some(token) = github_token()
    {
        cmd.arg("-H").arg(format!("Authorization: Bearer {token}"));
    }
    let out = cmd.arg(url).output().map_err(|e| format!("curl failed to start: {e}"))?;
    if !out.status.success() {
        return Err(format!("curl {url}: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(out.stdout)
}

fn github_token() -> Option<String> {
    if let Ok(t) = std::env::var("GITHUB_TOKEN")
        && !t.trim().is_empty()
    {
        return Some(t.trim().to_string());
    }
    let out = std::process::Command::new("gh").args(["auth", "token"]).output().ok()?;
    let t = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !t.is_empty()).then_some(t)
}

/// "v0.2.1" → [0, 2, 1]; non-numeric parts end the comparison key.
fn semver(tag: &str) -> Vec<u64> {
    tag.trim_start_matches('v')
        .split('.')
        .map_while(|p| p.parse::<u64>().ok())
        .collect()
}

pub fn is_newer(tag: &str) -> bool {
    semver(tag) > semver(CURRENT)
}

/// Latest release: (tag, asset urls). Blocking — call off the main loop.
pub fn latest_release(repo: &str) -> Result<(String, Vec<String>), String> {
    let body = curl(&format!("https://api.github.com/repos/{repo}/releases/latest"))?;
    let v: serde_json::Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    let tag = v["tag_name"].as_str().ok_or("release has no tag_name")?.to_string();
    let assets = v["assets"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x["browser_download_url"].as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    Ok((tag, assets))
}

/// The platform asset name this binary updates from.
pub fn asset_name() -> String {
    let arch = std::env::consts::ARCH; // aarch64 | x86_64
    let os = match std::env::consts::OS {
        "macos" => "macos",
        _ => "linux",
    };
    format!("cdock-{arch}-{os}.tar.gz")
}

/// Download the latest release and atomically replace `exe`. Returns the
/// new version tag, or None when already up to date.
pub fn self_update(exe: &Path) -> Result<Option<String>, String> {
    let repo = crate::config::load(None).0.update.repo;
    let (tag, assets) = latest_release(&repo)?;
    if !is_newer(&tag) {
        return Ok(None);
    }
    let name = asset_name();
    let url = assets
        .iter()
        .find(|u| u.ends_with(&name))
        .ok_or_else(|| format!("release {tag} has no asset {name}"))?;

    println!("downloading {url}");
    let tarball = curl(url)?;

    if let Some(sha_url) = assets.iter().find(|u| u.ends_with(&format!("{name}.sha256"))) {
        let expected = String::from_utf8_lossy(&curl(sha_url)?)
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_string();
        let actual = sha256_hex(&tarball);
        if expected != actual {
            return Err(format!("checksum mismatch: expected {expected}, got {actual}"));
        }
        println!("checksum ok");
    }

    // Unpack next to the target and rename over it (atomic on one fs).
    let dir = exe.parent().ok_or("binary has no parent dir")?;
    let staging = dir.join(".cdock-update");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    let tgz = staging.join(name);
    std::fs::write(&tgz, &tarball).map_err(|e| e.to_string())?;
    let out = std::process::Command::new("tar")
        .arg("-xzf")
        .arg(&tgz)
        .arg("-C")
        .arg(&staging)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!("tar: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let new_bin = staging.join("cdock");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&new_bin, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| e.to_string())?;
    }
    std::fs::rename(&new_bin, exe)
        .map_err(|e| format!("cannot replace {} ({e}); is it writable?", exe.display()))?;
    let _ = std::fs::remove_dir_all(&staging);
    Ok(Some(tag))
}

/// ponytail: no crypto dependency for one digest — shell out like install.sh.
fn sha256_hex(data: &[u8]) -> String {
    use std::io::Write;
    let cmd = if cfg!(target_os = "macos") { "shasum" } else { "sha256sum" };
    let args: &[&str] = if cfg!(target_os = "macos") { &["-a", "256"] } else { &[] };
    let mut child = match std::process::Command::new(cmd)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return String::new(),
    };
    if let Some(stdin) = child.stdin.take() {
        let mut stdin = stdin;
        let _ = stdin.write_all(data);
    }
    let out = child.wait_with_output().ok();
    out.and_then(|o| {
        String::from_utf8_lossy(&o.stdout).split_whitespace().next().map(String::from)
    })
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_ordering() {
        assert!(is_newer("v99.0.0"));
        assert!(!is_newer(CURRENT));
        assert!(!is_newer("v0.0.1"));
        assert!(semver("v0.10.0") > semver("v0.9.9"), "numeric, not lexicographic");
        assert_eq!(semver("garbage"), Vec::<u64>::new());
        assert!(!is_newer("garbage"), "unparseable tag is never newer");
    }

    #[test]
    fn asset_name_matches_release_convention() {
        let n = asset_name();
        assert!(n.starts_with("cdock-"), "{n}");
        assert!(n.ends_with(".tar.gz"));
        // Same shape the CI matrix publishes: cdock-<arch>-<os>.tar.gz
        assert!(n.contains("-macos.") || n.contains("-linux."));
    }
}
