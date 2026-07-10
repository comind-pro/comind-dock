//! Agent profiles (FEATURES §21): one directory per role under
//! ~/.config/comind-dock/agents/<name>/ — `profile.toml` (what to run),
//! `agent.md` (who the agent is), optional `memory.md` (per-role memory).
//! Resolution happens CLI-side: a profile turns into a plain command + env
//! for the AgentStart API request; the server knows nothing about profiles.
//! ponytail: claude adapter only (--append-system-prompt); skill-catalog
//! assignment, orchestrator flag, and the editor UI come later.

use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct ProfileToml {
    /// Base command, e.g. "claude" or "codex --model o3".
    pub command: String,
    /// Extra environment for the pane.
    pub env: std::collections::HashMap<String, String>,
}

pub struct Profile {
    pub name: String,
    pub dir: PathBuf,
    pub toml: ProfileToml,
}

pub fn profiles_dir() -> Option<PathBuf> {
    crate::config::config_path(None).and_then(|p| p.parent().map(|d| d.join("agents")))
}

pub fn list() -> Vec<String> {
    let Some(dir) = profiles_dir() else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|e| e.path().join("profile.toml").exists())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();
    names
}

pub fn load(name: &str) -> Result<Profile, String> {
    let dir = profiles_dir().ok_or("cannot determine config dir")?.join(name);
    let text = std::fs::read_to_string(dir.join("profile.toml"))
        .map_err(|e| format!("no profile {name:?} ({e}); see `cdock profile list`"))?;
    let toml: ProfileToml =
        toml::from_str(&text).map_err(|e| format!("bad profile.toml for {name:?}: {e}"))?;
    if toml.command.trim().is_empty() {
        return Err(format!("profile {name:?} has no command"));
    }
    Ok(Profile { name: name.to_string(), dir, toml })
}

impl Profile {
    /// The command + env the pane should run: base command with the role
    /// file staged in (per-agent adapter), CDOCK_AGENT_PROFILE_DIR set.
    pub fn resolve(&self) -> (String, Vec<(String, String)>) {
        let mut command = self.toml.command.clone();
        let agent_md = self.dir.join("agent.md");
        let memory_md = self.dir.join("memory.md");

        // Claude adapter: the role definition rides in as system prompt.
        let base = command.split_whitespace().next().unwrap_or("");
        if base.rsplit('/').next() == Some("claude") && agent_md.exists() {
            let mut cat = format!("cat '{}'", agent_md.display());
            if memory_md.exists() {
                cat.push_str(&format!(" '{}'", memory_md.display()));
            }
            command.push_str(&format!(" --append-system-prompt \"$({cat})\""));
        }

        let mut env: Vec<(String, String)> =
            self.toml.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        env.push(("CDOCK_AGENT_PROFILE_DIR".into(), self.dir.display().to_string()));
        env.push(("CDOCK_AGENT_PROFILE".into(), self.name.clone()));
        (command, env)
    }
}

const PROFILE_TOML_TEMPLATE: &str = r#"# Which agent CLI this profile runs.
command = "claude"

# Extra environment for the pane.
[env]
# MY_VAR = "value"
"#;

const AGENT_MD_TEMPLATE: &str = r#"You are the "{name}" agent.

Describe the role here: who this agent is, what it does, its constraints.

If a memory.md file exists next to this one, append the lessons you learn
each session to it — it rides along into every future launch of this role.
"#;

/// Scaffold ~/.config/comind-dock/agents/<name>/ (optionally copying an
/// existing profile). Refuses to overwrite.
pub fn scaffold(name: &str, from: Option<&str>) -> Result<PathBuf, String> {
    if name.is_empty() || name.contains(['/', '.']) {
        return Err(format!("bad profile name {name:?}"));
    }
    let dir = profiles_dir().ok_or("cannot determine config dir")?.join(name);
    if dir.exists() {
        return Err(format!("profile {name:?} already exists at {}", dir.display()));
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    match from {
        Some(src) => {
            let src_dir = load(src)?.dir;
            for file in ["profile.toml", "agent.md", "memory.md"] {
                let from_path = src_dir.join(file);
                if from_path.exists() {
                    std::fs::copy(&from_path, dir.join(file)).map_err(|e| e.to_string())?;
                }
            }
        }
        None => {
            std::fs::write(dir.join("profile.toml"), PROFILE_TOML_TEMPLATE)
                .map_err(|e| e.to_string())?;
            std::fs::write(dir.join("agent.md"), AGENT_MD_TEMPLATE.replace("{name}", name))
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_adapter_stages_role_file() {
        let dir = std::env::temp_dir().join(format!("cdock-prof-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("agent.md"), "role").unwrap();
        let p = Profile {
            name: "t".into(),
            dir: dir.clone(),
            toml: ProfileToml { command: "claude".into(), env: Default::default() },
        };
        let (cmd, env) = p.resolve();
        assert!(cmd.starts_with("claude --append-system-prompt"), "{cmd}");
        assert!(cmd.contains("agent.md"));
        assert!(env.iter().any(|(k, _)| k == "CDOCK_AGENT_PROFILE"));
        std::fs::remove_dir_all(&dir).unwrap();

        // Non-claude commands pass through untouched.
        let p2 = Profile {
            name: "t2".into(),
            dir: PathBuf::from("/nonexistent"),
            toml: ProfileToml { command: "codex".into(), env: Default::default() },
        };
        assert_eq!(p2.resolve().0, "codex");
    }
}
