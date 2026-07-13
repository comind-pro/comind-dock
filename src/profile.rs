//! Agent profiles (FEATURES §21): one directory per role under
//! ~/.config/comind-dock/agents/<name>/ — `profile.toml` (what to run),
//! `agent.md` (who the agent is), optional `memory.md` (per-role memory).
//! Resolution happens CLI-side: a profile turns into a plain command + env
//! for the AgentStart API request; the server knows nothing about profiles.
//! Scopes: global (~/.config/comind-dock/agents/) and per-workspace
//! metadata (~/.config/comind-dock/workspaces/<cwd-slug>/agents/) — the
//! workspace kind lives OUTSIDE the repo, keyed by the space's folder.
//! ponytail: claude adapter only (--append-system-prompt).

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

/// Catalog for WRITE paths: a parse failure must abort loudly — the silent
/// empty-map fallback would let the next save wipe the whole catalog.
fn skill_catalog_strict() -> Result<std::collections::BTreeMap<String, SkillEntry>, String> {
    let Some(path) = skills_path() else { return Ok(Default::default()) };
    match std::fs::read_to_string(&path) {
        Ok(t) => toml::from_str(&t).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Default::default()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

pub fn skill_add(name: &str, source: &str, description: &str) -> Result<(), String> {
    let dir = PathBuf::from(shellexpand_home(source));
    if !dir.join("SKILL.md").exists() {
        return Err(format!("{} has no SKILL.md", dir.display()));
    }
    let mut cat = skill_catalog_strict()?;
    cat.insert(
        name.to_string(),
        SkillEntry { source: dir.display().to_string(), description: description.to_string() },
    );
    save_catalog(&cat)
}

/// Scaffold ~/.config/comind-dock/skills/<name>/SKILL.md and register it
/// in the catalog. Refuses to overwrite.
pub fn skill_new(name: &str) -> Result<PathBuf, String> {
    if name.is_empty() || name.contains(['/', '.']) {
        return Err(format!("bad skill name {name:?}"));
    }
    let dir = crate::config::config_path(None)
        .and_then(|p| p.parent().map(|d| d.join("skills").join(name)))
        .ok_or("cannot determine config dir")?;
    if dir.exists() {
        return Err(format!("skill {name:?} already exists at {}", dir.display()));
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let md = dir.join("SKILL.md");
    std::fs::write(
        &md,
        format!("# {name}\n\nDescribe the skill: when to use it, the steps, the constraints.\n"),
    )
    .map_err(|e| e.to_string())?;
    skill_add(name, &dir.display().to_string(), "")?;
    Ok(md)
}

pub fn skill_remove(name: &str) -> Result<(), String> {
    let mut cat = skill_catalog_strict()?;
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

/// Per-workspace agent metadata: profiles scoped to one space, keyed by its
/// folder (slug = absolute path with '/' → '%'), OUTSIDE the repo itself.
pub fn ws_profiles_dir(cwd: &std::path::Path) -> Option<PathBuf> {
    let slug = cwd.to_string_lossy().replace('/', "%");
    crate::config::config_path(None)
        .and_then(|p| p.parent().map(|d| d.join("workspaces").join(slug).join("agents")))
}

fn list_in(dir: Option<PathBuf>) -> Vec<String> {
    let Some(dir) = dir else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|e| e.path().join("profile.toml").exists())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();
    names
}

pub fn list() -> Vec<String> {
    list_in(profiles_dir())
}

/// Workspace-scoped profiles for a space folder.
pub fn list_ws(cwd: &std::path::Path) -> Vec<String> {
    list_in(ws_profiles_dir(cwd))
}

fn load_in(dir: PathBuf, name: &str) -> Result<Profile, String> {
    let text = std::fs::read_to_string(dir.join("profile.toml"))
        .map_err(|e| format!("no profile {name:?} ({e}); see `cdock profile list`"))?;
    let toml: ProfileToml =
        toml::from_str(&text).map_err(|e| format!("bad profile.toml for {name:?}: {e}"))?;
    if toml.command.trim().is_empty() {
        return Err(format!("profile {name:?} has no command"));
    }
    Ok(Profile { name: name.to_string(), dir, toml })
}

pub fn load(name: &str) -> Result<Profile, String> {
    let dir = profiles_dir().ok_or("cannot determine config dir")?.join(name);
    load_in(dir, name)
}

/// Behavior ident ("global:<name>" | "ws:<name>") → profile; ws idents
/// resolve against the given space folder.
pub fn load_behavior(ident: &str, ws_cwd: &std::path::Path) -> Result<Profile, String> {
    match ident.split_once(':') {
        Some(("ws", name)) => {
            let dir = ws_profiles_dir(ws_cwd).ok_or("cannot determine config dir")?.join(name);
            load_in(dir, name)
        }
        Some(("global", name)) => load(name),
        _ => Err(format!("bad behavior ident {ident:?}")),
    }
}

/// Name → profile with scope resolution: explicit "ws:"/"global:" prefixes
/// win; a bare name tries the workspace's own agents first, then global —
/// so an agent that scaffolded "researcher" into its space just says
/// `--profile researcher` and gets its own definition.
pub fn load_any(name: &str, ws_cwd: &std::path::Path) -> Result<Profile, String> {
    if name.contains(':') {
        return load_behavior(name, ws_cwd);
    }
    load_behavior(&format!("ws:{name}"), ws_cwd).or_else(|_| load(name))
}

impl Profile {
    /// The command + env the pane should run. Materialization stages one
    /// prompt file (role + memory + assigned skills + orchestrator roster)
    /// into the profile dir; the claude adapter rides it in as system
    /// prompt, other CLIs find it via $CDOCK_AGENT_PROFILE_DIR.
    pub fn resolve(&self) -> (String, Vec<(String, String)>) {
        self.resolve_with(None)
    }

    /// `ws_cwd`: the space whose scoped agents join the orchestrator roster.
    pub fn resolve_with(
        &self,
        ws_cwd: Option<&std::path::Path>,
    ) -> (String, Vec<(String, String)>) {
        let mut command = self.toml.command.clone();
        let staged = self.stage_prompt_with(ws_cwd);

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

    /// The staged prompt CONTENT (role + memory + skills + roster) — for
    /// injecting a behavior into an already-running agent session.
    pub fn prompt_text_with(&self, ws_cwd: Option<&std::path::Path>) -> Option<String> {
        let path = self.stage_prompt_with(ws_cwd)?;
        std::fs::read_to_string(path).ok().filter(|t| !t.trim().is_empty())
    }

    pub fn stage_prompt(&self) -> Option<PathBuf> {
        self.stage_prompt_with(None)
    }

    /// Write `<dir>/staged-prompt.md` — regenerated on every launch, so it
    /// is inspectable but never hand-edited. None when there is nothing to say.
    pub fn stage_prompt_with(&self, ws_cwd: Option<&std::path::Path>) -> Option<PathBuf> {
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
            let first_line = |p: &Profile| {
                std::fs::read_to_string(p.dir.join("agent.md"))
                    .ok()
                    .and_then(|t| t.lines().find(|l| !l.trim().is_empty()).map(String::from))
                    .unwrap_or_default()
            };
            let mut roster = String::new();
            // The space's own agents first — `--profile <name>` prefers them.
            if let Some(cwd) = ws_cwd {
                for n in list_ws(cwd) {
                    if n == self.name {
                        continue;
                    }
                    if let Ok(p) = load_behavior(&format!("ws:{n}"), cwd) {
                        roster.push_str(&format!("- {n} (this workspace): {}\n", first_line(&p)));
                    }
                }
            }
            for n in list() {
                if n == self.name {
                    continue;
                }
                if let Ok(p) = load(&n) {
                    roster.push_str(&format!("- {n}: {}\n", first_line(&p)));
                }
            }
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

/// Persist `skills = [...]` in a profile's profile.toml with a targeted
/// line edit (a full toml rewrite would drop the user's comments).
pub fn set_skills(profile_dir: &std::path::Path, skills: &[String]) -> Result<(), String> {
    let path = profile_dir.join("profile.toml");
    let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let quoted: Vec<String> = skills.iter().map(|s| format!("{s:?}")).collect();
    let line = format!("skills = [{}]", quoted.join(", "));
    let mut out = Vec::new();
    let mut done = false;
    for l in text.lines() {
        if !done && l.trim_start().starts_with("skills") {
            out.push(line.clone());
            done = true;
            continue;
        }
        out.push(l.to_string());
    }
    if !done {
        // Before the first section header — top-level key territory.
        let at = out.iter().position(|l| l.trim_start().starts_with('[')).unwrap_or(out.len());
        out.insert(at, line);
    }
    std::fs::write(&path, out.join("\n") + "\n").map_err(|e| e.to_string())
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
    scaffold_at(profiles_dir(), name, from)
}

/// Scaffold a workspace-scoped profile under the space's metadata dir.
pub fn scaffold_ws(cwd: &std::path::Path, name: &str) -> Result<PathBuf, String> {
    scaffold_at(ws_profiles_dir(cwd), name, None)
}

fn scaffold_at(base: Option<PathBuf>, name: &str, from: Option<&str>) -> Result<PathBuf, String> {
    if name.is_empty() || name.contains(['/', '.']) {
        return Err(format!("bad profile name {name:?}"));
    }
    let dir = base.ok_or("cannot determine config dir")?.join(name);
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
    fn ws_scoped_profiles_and_behavior_idents() {
        // Redirect the config root so nothing touches the real one.
        let root = std::env::temp_dir().join(format!("cdock-wsprof-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        // Safety: single-threaded test env access.
        unsafe { std::env::set_var("CDOCK_CONFIG_PATH", root.join("config.toml")) };

        let cwd = std::path::Path::new("/projects/demo");
        let dir = scaffold_ws(cwd, "researcher").unwrap();
        assert!(dir.join("profile.toml").exists());
        assert!(dir.display().to_string().contains("%projects%demo"), "{}", dir.display());
        assert_eq!(list_ws(cwd), vec!["researcher".to_string()]);
        assert!(list_ws(std::path::Path::new("/other")).is_empty());

        let p = load_behavior("ws:researcher", cwd).unwrap();
        assert_eq!(p.name, "researcher");
        assert!(load_behavior("ws:researcher", std::path::Path::new("/other")).is_err());
        assert!(load_behavior("junk", cwd).is_err());

        // Duplicate scaffold refuses.
        assert!(scaffold_ws(cwd, "researcher").is_err());

        unsafe { std::env::remove_var("CDOCK_CONFIG_PATH") };
        std::fs::remove_dir_all(&root).unwrap();
    }

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
