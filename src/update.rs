//! Update system (Phase 6): GitHub Releases as the feed. The server checks
//! for a newer tag in the background (menu shows "● update ready"); `cdock
//! update` downloads the platform tarball, verifies the sha256 when
//! published, atomically replaces the current binary, and with --handoff
//! execs the running server in place — no pane dies.
//! ponytail: HTTP via the system curl — not worth a client dependency.

use std::path::{Path, PathBuf};

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
/// Numeric parts + a final "is-release" rank: leading digits of each dot
/// part parse even with a prerelease suffix ("1-rc2" → 1), and at equal
/// numbers a full release outranks any prerelease ("0.4.1" > "0.4.1-rc1").
/// ponytail: rc1 vs rc2 at the same version compares by the suffix STRING —
/// fine up to rc9, but "rc10" < "rc2" lexicographically. Known ceiling:
/// this repo never ships double-digit rcs of one version; a real SemVer
/// prerelease parse is the upgrade path if that ever changes.
fn semver(tag: &str) -> (Vec<u64>, bool, String) {
    let v = tag.trim_start_matches('v');
    let nums: Vec<u64> = v
        .split('.')
        .map_while(|p| {
            let digits: String = p.chars().take_while(char::is_ascii_digit).collect();
            digits.parse::<u64>().ok()
        })
        .collect();
    let suffix = v.split_once('-').map(|(_, s)| s.to_string()).unwrap_or_default();
    (nums, suffix.is_empty(), suffix)
}

pub fn is_newer(tag: &str) -> bool {
    let t = semver(tag);
    // An unparseable tag ((0 numbers)) must never look newer.
    !t.0.is_empty() && t > semver(CURRENT)
}

/// Which release feed to follow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    /// Latest full release (GitHub `/releases/latest` excludes prereleases
    /// and drafts by definition).
    #[default]
    Stable,
    /// Newest published release, prereleases included.
    Preview,
}

#[derive(Debug, PartialEq)]
pub struct Release {
    pub tag: String,
    pub assets: Vec<String>,
    /// Release notes (the GitHub release `body`).
    pub notes: String,
}

/// Pure: one release JSON object → Release.
fn parse_release(v: &serde_json::Value) -> Result<Release, String> {
    let tag = v["tag_name"].as_str().ok_or("release has no tag_name")?.to_string();
    let assets = v["assets"]
        .as_array()
        .map(|a| {
            a.iter().filter_map(|x| x["browser_download_url"].as_str().map(String::from)).collect()
        })
        .unwrap_or_default();
    let notes = v["body"].as_str().unwrap_or_default().to_string();
    Ok(Release { tag, assets, notes })
}

/// Pure: newest non-draft entry of a `/releases` array (GitHub orders
/// newest-first; prereleases are eligible — that is the point of preview).
fn first_non_draft(list: &serde_json::Value) -> Result<&serde_json::Value, String> {
    list.as_array()
        .and_then(|a| a.iter().find(|r| r["draft"] != true))
        .ok_or_else(|| "no published releases".to_string())
}

/// Latest release on the channel. Blocking — call off the main loop.
pub fn latest_release(repo: &str, channel: Channel) -> Result<Release, String> {
    let body = match channel {
        Channel::Stable => curl(&format!("https://api.github.com/repos/{repo}/releases/latest"))?,
        Channel::Preview => {
            curl(&format!("https://api.github.com/repos/{repo}/releases?per_page=10"))?
        }
    };
    let v: serde_json::Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    match channel {
        Channel::Stable => parse_release(&v),
        Channel::Preview => parse_release(first_non_draft(&v)?),
    }
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
    let update_cfg = crate::config::load(None).0.update;
    let rel = latest_release(&update_cfg.repo, update_cfg.channel)?;
    let (tag, assets) = (rel.tag, rel.assets);
    if !is_newer(&tag) {
        return Ok(None);
    }
    let name = asset_name();
    let url = assets
        .iter()
        .find(|u| u.ends_with(&name))
        .ok_or_else(|| format!("release {tag} has no asset {name}"))?;

    let sha_url = sha_asset(&assets, &name)?;

    println!("downloading {url}");
    let tarball = curl(url)?;

    let expected = String::from_utf8_lossy(&curl(sha_url)?)
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_string();
    let actual = sha256_hex(&tarball);
    if expected.is_empty() || expected != actual {
        return Err(format!("checksum mismatch: expected {expected}, got {actual}"));
    }
    println!("checksum ok");

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
    // Leave the release notes for the next boot to toast (best-effort).
    if let Some(dir) = crate::logging::state_dir() {
        let _ = write_pending_notes(&dir, &tag, &rel.notes);
    }
    Ok(Some(tag))
}

fn pending_notes_path(dir: &Path) -> PathBuf {
    dir.join("release-notes-pending.md")
}

fn write_pending_notes(dir: &Path, tag: &str, notes: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(pending_notes_path(dir), format!("# {tag}\n\n{notes}\n"))
}

fn take_pending_notes(dir: &Path) -> Option<String> {
    let path = pending_notes_path(dir);
    let text = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    Some(text)
}

/// Release notes left behind by a successful `self_update`, consumed on
/// first read (read + delete) — the boot path toasts the headline.
pub fn take_pending_release_notes() -> Option<String> {
    take_pending_notes(&crate::logging::state_dir()?)
}

/// The `.sha256` asset for `name`, or an error — self-update never installs
/// an unverified tarball, even when a release simply forgot the checksum.
fn sha_asset<'a>(assets: &'a [String], name: &str) -> Result<&'a String, String> {
    let suffix = format!("{name}.sha256");
    assets
        .iter()
        .find(|u| u.ends_with(&suffix))
        .ok_or_else(|| format!("release has no {suffix} asset; refusing unverified update"))
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
    out.and_then(|o| String::from_utf8_lossy(&o.stdout).split_whitespace().next().map(String::from))
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
        assert!(semver("garbage").0.is_empty());
        assert!(!is_newer("garbage"), "unparseable tag is never newer");
        // Prerelease tags keep their patch number and rank below the release.
        assert!(semver("v0.4.1-rc1").0 == vec![0, 4, 1]);
        assert!(semver("v0.4.1") > semver("v0.4.1-rc1"));
        assert!(semver("v0.4.1-rc2") > semver("v0.4.1-rc1"));
        assert!(is_newer("v99.0.0-rc1"), "prerelease of a newer version is newer");
    }

    #[test]
    fn update_without_checksum_asset_is_refused() {
        let name = "cdock-aarch64-macos.tar.gz";
        let with = vec![format!("https://x/{name}"), format!("https://x/{name}.sha256")];
        assert!(sha_asset(&with, name).is_ok());
        let without = vec![format!("https://x/{name}")];
        let err = sha_asset(&without, name).unwrap_err();
        assert!(err.contains("sha256"), "error names the missing checksum: {err}");
    }

    #[test]
    fn preview_picks_first_non_draft_release() {
        let list: serde_json::Value = serde_json::json!([
            { "tag_name": "v9.9.9", "draft": true, "prerelease": true, "body": "wip" },
            {
                "tag_name": "v1.2.0-rc.1",
                "draft": false,
                "prerelease": true,
                "body": "rc notes",
                "assets": [{ "browser_download_url": "https://x/cdock-aarch64-macos.tar.gz" }]
            },
            { "tag_name": "v1.1.0", "draft": false, "prerelease": false, "body": "stable" },
        ]);
        let rel = parse_release(first_non_draft(&list).unwrap()).unwrap();
        assert_eq!(rel.tag, "v1.2.0-rc.1", "drafts skipped, prereleases eligible");
        assert_eq!(rel.notes, "rc notes");
        assert_eq!(rel.assets, vec!["https://x/cdock-aarch64-macos.tar.gz".to_string()]);
        assert!(first_non_draft(&serde_json::json!([])).is_err(), "empty list is an error");
    }

    #[test]
    fn pending_notes_round_trip() {
        let dir = std::env::temp_dir().join(format!("cdock-notes-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(take_pending_notes(&dir), None, "nothing pending in a fresh dir");
        write_pending_notes(&dir, "v1.2.3", "bug fixes").unwrap();
        assert_eq!(take_pending_notes(&dir).as_deref(), Some("# v1.2.3\n\nbug fixes\n"));
        assert_eq!(take_pending_notes(&dir), None, "take consumes the file");
        let _ = std::fs::remove_dir_all(&dir);
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
