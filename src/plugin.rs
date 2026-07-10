//! Plugins (FEATURES: manifest + out-of-process actions). A plugin is a
//! directory with plugin.toml living (or symlinked) under
//! ~/.config/comind-dock/plugins/<id>/. Actions are shell commands run with
//! the cdock env (CDOCK_BIN, CDOCK_PLUGIN_DIR) — they drive the runtime
//! through the same CLI/API agents use.
//! ponytail: `link` for local development only; install-from-GitHub, event
//! hooks, link handlers, and managed panes arrive with plugin marketplace
//! work. Actions run CLI-side — the server never executes plugin code.

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
    let mut plugins: Vec<Plugin> = entries
        .flatten()
        .filter_map(|e| load_dir(&e.path()).ok())
        .collect();
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

/// Symlink a local plugin directory into the plugins dir (development flow).
pub fn link(path: &str) -> Result<String, String> {
    let src = std::fs::canonicalize(path).map_err(|e| format!("{path}: {e}"))?;
    let plugin = load_dir(&src)?;
    let dir = plugins_dir().ok_or("cannot determine config dir")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let dst = dir.join(&plugin.manifest.id);
    if dst.exists() {
        return Err(format!("plugin {:?} already present at {}", plugin.manifest.id, dst.display()));
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
}
