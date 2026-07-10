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
    /// Skill names from the catalog (~/.config/comind-dock/skills.toml).
    pub skills: Vec<String>,
    /// Orchestrator: gets the profile roster and spawns specialists.
    pub orchestrator: bool,
}

/// One catalog entry: where the skill lives (a directory with SKILL.md).
#[derive(Debug, Deserialize, serde::Serialize, Default)]
#[serde(default)]
pub struct SkillEntry {
    pub source: String,
    pub description: String,
}

pub fn skills_path() -> Option<PathBuf> {
    crate::config::config_path(None).and_then(|p| p.parent().map(|d| d.join("skills.toml")))
}

/// The skill catalog — a machine-managed file, separate from config.toml.
pub fn skill_catalog() -> std::collections::BTreeMap<String, SkillEntry> {
    let Some(path) = skills_path() else { return Default::default() };
    std::fs::read_to_string(path).ok().and_then(|t| toml::from_str(&t).ok()).unwrap_or_default()
}

fn save_catalog(cat: &std::collections::BTreeMap<String, SkillEntry>) -> Result<(), String> {
    let path = skills_path().ok_or("cannot determine config dir")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = toml::to_string(cat).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| e.to_string())
}

pub fn skill_add(name: &str, source: &str, description: &str) -> Result<(), String> {
    let dir = PathBuf::from(shellexpand_home(source));
    if !dir.join("SKILL.md").exists() {
        return Err(format!("{} has no SKILL.md", dir.display()));
    }
    let mut cat = skill_catalog();
    cat.insert(
        name.to_string(),
        SkillEntry { source: dir.display().to_string(), description: description.to_string() },
    );
    save_catalog(&cat)
}

pub fn skill_remove(name: &str) -> Result<(), String> {
    let mut cat = skill_catalog();
    if cat.remove(name).is_none() {
        return Err(format!("no skill {name:?} in the catalog"));
    }
    save_catalog(&cat)
}

fn shellexpand_home(p: &str) -> String {
    match (p.strip_prefix("~/"), std::env::var("HOME")) {
        (Some(rest), Ok(home)) => format!("{home}/{rest}"),
        _ => p.to_string(),
    }
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
    /// The command + env the pane should run. Materialization stages one
    /// prompt file (role + memory + assigned skills + orchestrator roster)
    /// into the profile dir; the claude adapter rides it in as system
    /// prompt, other CLIs find it via $CDOCK_AGENT_PROFILE_DIR.
    pub fn resolve(&self) -> (String, Vec<(String, String)>) {
        let mut command = self.toml.command.clone();
        let staged = self.stage_prompt();

        // Claude adapter: the staged prompt rides in as system prompt.
        let base = command.split_whitespace().next().unwrap_or("");
        if base.rsplit('/').next() == Some("claude")
            && let Some(staged) = &staged
        {
            command.push_str(&format!(" --append-system-prompt \"$(cat '{}')\"", staged.display()));
        }

        let mut env: Vec<(String, String)> =
            self.toml.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        env.push(("CDOCK_AGENT_PROFILE_DIR".into(), self.dir.display().to_string()));
        env.push(("CDOCK_AGENT_PROFILE".into(), self.name.clone()));
        (command, env)
    }

    /// Write `<dir>/staged-prompt.md` — regenerated on every launch, so it
    /// is inspectable but never hand-edited. None when there is nothing to say.
    fn stage_prompt(&self) -> Option<PathBuf> {
        let mut text = String::new();
        for file in ["agent.md", "memory.md"] {
            if let Ok(t) = std::fs::read_to_string(self.dir.join(file)) {
                text.push_str(&t);
                text.push('\n');
            }
        }

        let catalog = skill_catalog();
        let mut skills = String::new();
        for name in &self.toml.skills {
            match catalog.get(name) {
                Some(e) if PathBuf::from(&e.source).join("SKILL.md").exists() => {
                    skills.push_str(&format!(
                        "- {name}: {} — read {}/SKILL.md when relevant\n",
                        e.description, e.source
                    ));
                }
                _ => tracing::warn!(profile = %self.name, skill = %name, "skill missing"),
            }
        }
        if !skills.is_empty() {
            text.push_str(&format!("\n## Your skills\n\n{skills}"));
        }

        if self.toml.orchestrator {
            let roster: String = list()
                .iter()
                .filter(|n| **n != self.name)
                .map(|n| {
                    let desc = load(n)
                        .ok()
                        .and_then(|p| std::fs::read_to_string(p.dir.join("agent.md")).ok())
                        .and_then(|t| t.lines().find(|l| !l.trim().is_empty()).map(String::from))
                        .unwrap_or_default();
                    format!("- {n}: {desc}\n")
                })
                .collect();
            text.push_str(&format!(
                "\n## You are an orchestrator\n\n\
                 Spawn specialist agents into panes and coordinate them with the\n\
                 cdock CLI (you have the cdock skill): `\"$CDOCK_BIN\" agent start\n\
                 --profile <name> [--split right|down]`, then watch them via\n\
                 `\"$CDOCK_BIN\" events --only agent-status` or `wait agent-status`,\n\
                 read their screens with `pane read`.\n\n\
                 Available profiles:\n{roster}"
            ));
        }

        if text.trim().is_empty() {
            return None;
        }
        let path = self.dir.join("staged-prompt.md");
        std::fs::write(&path, text).ok()?;
        Some(path)
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
    fn claude_adapter_stages_prompt() {
        let dir = std::env::temp_dir().join(format!("cdock-prof-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("agent.md"), "the role").unwrap();
        let p = Profile {
            name: "t".into(),
            dir: dir.clone(),
            toml: ProfileToml {
                command: "claude".into(),
                orchestrator: true,
                ..Default::default()
            },
        };
        let (cmd, env) = p.resolve();
        assert!(cmd.starts_with("claude --append-system-prompt"), "{cmd}");
        assert!(cmd.contains("staged-prompt.md"));
        let staged = std::fs::read_to_string(dir.join("staged-prompt.md")).unwrap();
        assert!(staged.contains("the role"));
        assert!(staged.contains("orchestrator"), "roster block present");
        assert!(env.iter().any(|(k, _)| k == "CDOCK_AGENT_PROFILE"));
        std::fs::remove_dir_all(&dir).unwrap();

        // Non-claude commands pass through untouched (prompt still staged
        // for $CDOCK_AGENT_PROFILE_DIR readers, command unchanged).
        let p2 = Profile {
            name: "t2".into(),
            dir: PathBuf::from("/nonexistent"),
            toml: ProfileToml { command: "codex".into(), ..Default::default() },
        };
        assert_eq!(p2.resolve().0, "codex");
    }
}
