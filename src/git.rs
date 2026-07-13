//! Light git helpers: branch by reading HEAD (no subprocess), worktree
//! list/add via the git CLI.

use std::path::{Path, PathBuf};

/// Current branch of the repo containing `dir` (walks up), short sha when
/// detached, None outside a repo. Reads files only — cheap enough to poll.
pub fn branch(dir: &Path) -> Option<String> {
    let gitdir = find_gitdir(dir)?;
    let head = std::fs::read_to_string(gitdir.join("HEAD")).ok()?;
    let head = head.trim();
    if let Some(r) = head.strip_prefix("ref: ") {
        return Some(r.rsplit('/').next().unwrap_or(r).to_string());
    }
    Some(head.chars().take(7).collect())
}

/// `.git` dir for `dir`, resolving worktree `.git` pointer files.
fn find_gitdir(start: &Path) -> Option<PathBuf> {
    let mut dir = start.to_path_buf();
    for _ in 0..12 {
        let dotgit = dir.join(".git");
        if dotgit.is_dir() {
            return Some(dotgit);
        }
        if dotgit.is_file() {
            let text = std::fs::read_to_string(&dotgit).ok()?;
            let p = text.trim().strip_prefix("gitdir: ")?.trim();
            let p = PathBuf::from(p);
            return Some(if p.is_absolute() { p } else { dir.join(p) });
        }
        if !dir.pop() {
            break;
        }
    }
    None
}

/// Existing worktrees of the repo at `dir`: (path, branch).
pub fn worktrees(dir: &Path) -> Vec<(PathBuf, String)> {
    let Ok(out) = std::process::Command::new("git")
        .args(["-C", &dir.to_string_lossy(), "worktree", "list", "--porcelain"])
        .output()
    else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut result = Vec::new();
    let mut path: Option<PathBuf> = None;
    for line in text.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            path = Some(PathBuf::from(p));
        } else if let Some(b) = line.strip_prefix("branch ") {
            if let Some(p) = path.take() {
                result.push((p, b.rsplit('/').next().unwrap_or(b).to_string()));
            }
        } else if line == "detached"
            && let Some(p) = path.take()
        {
            result.push((p, "detached".to_string()));
        }
    }
    result
}

/// Create a worktree for `branch` under `root/<repo>/<branch>`; reuses the
/// branch when it already exists. Returns the worktree path.
pub fn worktree_add(repo_dir: &Path, branch: &str, root: &Path) -> Result<PathBuf, String> {
    let repo = repo_dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let slug: String = branch
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();
    let target = root.join(&repo).join(&slug);
    if let Err(e) = std::fs::create_dir_all(target.parent().unwrap_or(root)) {
        return Err(format!("mkdir failed: {e}"));
    }
    let run = |args: &[&str]| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(repo_dir)
            .args(args)
            .output()
            .map_err(|e| e.to_string())
    };
    let target_s = target.to_string_lossy().into_owned();
    // New branch first; fall back to checking out an existing one.
    let out = run(&["worktree", "add", "-b", branch, &target_s])?;
    if out.status.success() {
        return Ok(target);
    }
    let out = run(&["worktree", "add", &target_s, branch])?;
    if out.status.success() {
        Ok(target)
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Remove a worktree (git refuses on a dirty tree unless `force`).
pub fn worktree_remove(repo_dir: &Path, worktree: &Path, force: bool) -> Result<(), String> {
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo_dir)
        .args(&args)
        .arg(worktree)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_of_this_repo() {
        // The project repo itself: HEAD exists and names a branch or sha.
        let b = branch(Path::new(env!("CARGO_MANIFEST_DIR")));
        assert!(b.is_some());
        assert!(!b.unwrap().is_empty());
    }

    #[test]
    fn no_repo_no_branch() {
        assert_eq!(branch(Path::new("/tmp")), None);
    }
}
