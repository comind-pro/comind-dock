//! Plugins (FEATURES: manifest + out-of-process actions). A plugin is a
//! directory with plugin.toml living (or symlinked) under
//! ~/.config/comind-dock/plugins/<id>/. Actions are shell commands run with
//! the cdock env (CDOCK_BIN, CDOCK_PLUGIN_DIR) — they drive the runtime
//! through the same CLI/API agents use.
//! ponytail: link handlers arrive with plugin marketplace work. Actions and
//! hooks run as shell commands; the server only shells out for [[hooks]]
//! (fire-and-forget) — everything else stays CLI-side.

use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(rename = "action")]
    pub actions: Vec<Action>,
    pub hooks: Vec<Hook>,
    pub panes: Vec<ManagedPane>,
}

/// `[[hooks]]` — shell command to run on an agent status change. The caller
/// provides CDOCK_PANE (pane id) and CDOCK_STATUS (new status word) in the
/// command's env. `event = "status"` matches every change.
#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct Hook {
    /// "blocked" | "done" | "status" (any change).
    pub event: String,
    pub run: String,
}

/// `[[panes]]` — a pane the plugin wants opened via the API.
#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct ManagedPane {
    pub title: String,
    pub command: String,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct Action {
    pub id: String,
    pub label: String,
    /// Shell command; runs with cwd = the plugin directory.
    pub command: String,
}

pub struct Plugin {
    pub dir: PathBuf,
    pub manifest: Manifest,
}

pub fn plugins_dir() -> Option<PathBuf> {
    crate::config::config_path(None).and_then(|p| p.parent().map(|d| d.join("plugins")))
}

pub fn list() -> Vec<Plugin> {
    let Some(dir) = plugins_dir() else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut plugins: Vec<Plugin> =
        entries.flatten().filter_map(|e| load_dir(&e.path()).ok()).collect();
    plugins.sort_by(|a, b| a.manifest.id.cmp(&b.manifest.id));
    plugins
}

fn load_dir(dir: &Path) -> Result<Plugin, String> {
    let text = std::fs::read_to_string(dir.join("plugin.toml"))
        .map_err(|e| format!("no plugin.toml in {} ({e})", dir.display()))?;
    let manifest: Manifest =
        toml::from_str(&text).map_err(|e| format!("bad plugin.toml in {}: {e}", dir.display()))?;
    if manifest.id.is_empty() {
        return Err(format!("{}: plugin.toml has no id", dir.display()));
    }
    Ok(Plugin { dir: dir.to_path_buf(), manifest })
}

pub fn load(id: &str) -> Result<Plugin, String> {
    list()
        .into_iter()
        .find(|p| p.manifest.id == id)
        .ok_or_else(|| format!("no plugin {id:?}; see `cdock plugin list`"))
}

/// Hook commands to run for a status-change event ("blocked", "done", …).
/// Callers spawn each with CDOCK_PANE and CDOCK_STATUS set in the env.
pub fn event_hooks(plugins: &[Plugin], event: &str) -> Vec<String> {
    plugins
        .iter()
        .flat_map(|p| &p.manifest.hooks)
        .filter(|h| h.event == event || h.event == "status")
        .map(|h| h.run.clone())
        .collect()
}

/// Panes the plugin declares via `[[panes]]`; callers open them via the API.
pub fn managed_panes(plugin: &Plugin) -> &[ManagedPane] {
    &plugin.manifest.panes
}

/// Install a plugin: "gh:owner/repo" (shallow clone) or a local path (symlink).
/// Returns the plugin id.
pub fn install(spec: &str) -> Result<String, String> {
    match parse_gh_spec(spec)? {
        Some(url) => install_github(&url),
        None => link(spec),
    }
}

/// "gh:owner/repo" → clone URL; Ok(None) for non-gh specs (local paths).
/// Pure — no network, no fs.
fn parse_gh_spec(spec: &str) -> Result<Option<String>, String> {
    let Some(rest) = spec.strip_prefix("gh:") else { return Ok(None) };
    let ok_part = |s: &str| {
        !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
    };
    match rest.split('/').collect::<Vec<_>>()[..] {
        [owner, repo] if ok_part(owner) && ok_part(repo) => {
            Ok(Some(format!("https://github.com/{owner}/{repo}")))
        }
        _ => Err(format!("bad GitHub spec {spec:?}; expected gh:owner/repo")),
    }
}

/// Shallow-clone into the plugins dir, validate the manifest, rename to the
/// plugin id. Any failure removes the clone.
/// ponytail: one fixed temp dir — concurrent installs collide; fine for a CLI.
fn install_github(url: &str) -> Result<String, String> {
    let plugins = plugins_dir().ok_or("cannot determine config dir")?;
    std::fs::create_dir_all(&plugins).map_err(|e| e.to_string())?;
    let tmp = plugins.join(".installing");
    let _ = std::fs::remove_dir_all(&tmp); // stale leftover from a crashed install
    let status = std::process::Command::new("git")
        .args(["clone", "--depth", "1", url])
        .arg(&tmp)
        .status()
        .map_err(|e| format!("git: {e}"))?;
    let cleanup = |e: String| {
        let _ = std::fs::remove_dir_all(&tmp);
        e
    };
    if !status.success() {
        return Err(cleanup(format!("git clone {url} failed")));
    }
    let plugin = load_dir(&tmp).map_err(cleanup)?;
    let dst = plugins.join(&plugin.manifest.id);
    if dst.exists() {
        return Err(cleanup(format!(
            "plugin {:?} already present at {}",
            plugin.manifest.id,
            dst.display()
        )));
    }
    std::fs::rename(&tmp, &dst).map_err(|e| cleanup(e.to_string()))?;
    Ok(plugin.manifest.id)
}

/// Symlink a local plugin directory into the plugins dir (development flow).
pub fn link(path: &str) -> Result<String, String> {
    let src = std::fs::canonicalize(path).map_err(|e| format!("{path}: {e}"))?;
    let plugin = load_dir(&src)?;
    let dir = plugins_dir().ok_or("cannot determine config dir")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let dst = dir.join(&plugin.manifest.id);
    if dst.exists() {
        return Err(format!(
            "plugin {:?} already present at {}",
            plugin.manifest.id,
            dst.display()
        ));
    }
    std::os::unix::fs::symlink(&src, &dst).map_err(|e| e.to_string())?;
    Ok(plugin.manifest.id)
}

/// Remove a plugin entry: a symlink is unlinked, a real dir is refused
/// (delete it yourself — we don't rm -rf things we didn't create).
pub fn unlink(id: &str) -> Result<(), String> {
    let dir = plugins_dir().ok_or("cannot determine config dir")?.join(id);
    let meta = std::fs::symlink_metadata(&dir).map_err(|_| format!("no plugin {id:?}"))?;
    if meta.file_type().is_symlink() {
        std::fs::remove_file(&dir).map_err(|e| e.to_string())
    } else {
        Err(format!("{} is a real directory, not a link — remove it manually", dir.display()))
    }
}

/// Run one plugin action in the foreground, plugin dir as cwd, cdock env set.
/// Exit status is the CLI's exit status.
pub fn invoke(plugin_id: &str, action_id: &str) -> Result<bool, String> {
    let plugin = load(plugin_id)?;
    let action = plugin
        .manifest
        .actions
        .iter()
        .find(|a| a.id == action_id)
        .ok_or_else(|| format!("plugin {plugin_id:?} has no action {action_id:?}"))?;
    let mut cmd = std::process::Command::new("/bin/sh");
    cmd.arg("-c").arg(&action.command).current_dir(&plugin.dir);
    cmd.env("CDOCK_PLUGIN_DIR", &plugin.dir);
    if let Ok(exe) = std::env::current_exe() {
        cmd.env("CDOCK_BIN", exe);
    }
    let status = cmd.status().map_err(|e| e.to_string())?;
    Ok(status.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_parses_actions() {
        let m: Manifest = toml::from_str(
            r#"
id = "demo"
name = "Demo"
[[action]]
id = "hi"
label = "Say hi"
command = "echo hi"
"#,
        )
        .unwrap();
        assert_eq!(m.id, "demo");
        assert_eq!(m.actions.len(), 1);
        assert_eq!(m.actions[0].command, "echo hi");
    }

    #[test]
    fn manifest_parses_hooks_and_panes() {
        let m: Manifest = toml::from_str(
            r#"
id = "demo"
[[hooks]]
event = "blocked"
run = "notify-send blocked"
[[hooks]]
event = "status"
run = "log-any"
[[panes]]
title = "Logs"
command = "tail -f app.log"
"#,
        )
        .unwrap();
        assert_eq!(m.hooks.len(), 2);
        assert_eq!(m.hooks[0].event, "blocked");
        assert_eq!(m.panes.len(), 1);
        assert_eq!(m.panes[0].title, "Logs");
        let p = Plugin { dir: PathBuf::new(), manifest: m };
        assert_eq!(managed_panes(&p)[0].command, "tail -f app.log");
    }

    #[test]
    fn event_hooks_filters_by_event() {
        let m: Manifest = toml::from_str(
            r#"
id = "demo"
[[hooks]]
event = "blocked"
run = "on-blocked"
[[hooks]]
event = "done"
run = "on-done"
[[hooks]]
event = "status"
run = "on-any"
"#,
        )
        .unwrap();
        let plugins = vec![Plugin { dir: PathBuf::new(), manifest: m }];
        assert_eq!(event_hooks(&plugins, "blocked"), vec!["on-blocked", "on-any"]);
        assert_eq!(event_hooks(&plugins, "done"), vec!["on-done", "on-any"]);
        assert_eq!(event_hooks(&plugins, "working"), vec!["on-any"]);
        assert!(event_hooks(&[], "blocked").is_empty());
    }

    #[test]
    fn gh_spec_parsing() {
        assert_eq!(
            parse_gh_spec("gh:owner/repo").unwrap().as_deref(),
            Some("https://github.com/owner/repo")
        );
        assert_eq!(
            parse_gh_spec("gh:my-org/my.plugin_2").unwrap().as_deref(),
            Some("https://github.com/my-org/my.plugin_2")
        );
        // Not a gh spec → local-path fallback.
        assert_eq!(parse_gh_spec("./some/dir").unwrap(), None);
        // Garbage gh specs are rejected, not treated as paths.
        for bad in ["gh:", "gh:owner", "gh:owner/", "gh:/repo", "gh:a/b/c", "gh:ow ner/repo"] {
            assert!(parse_gh_spec(bad).is_err(), "{bad} should be rejected");
        }
    }
}
