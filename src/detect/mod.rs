//! Agent detection engine (ARCHITECTURE §4): a screen-snapshot pattern
//! matcher. Reads the pane's bottom-of-buffer text plus the OSC title,
//! runs the agent's ordered manifest rules, highest priority wins.
//! Manifests: bundled, overridden by ~/.config/comind-dock/manifests/*.toml
//! (matched by id), hot-reloadable via `cdock server reload-manifests`.
//! ponytail: the remote manifest feed arrives with the update system.

use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Status {
    Working,
    Blocked,
    Done,
    Idle,
    #[default]
    Unknown,
}

impl Status {
    pub fn word(self) -> &'static str {
        match self {
            Status::Working => "working",
            Status::Blocked => "blocked",
            Status::Done => "done",
            Status::Idle => "idle",
            Status::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct Manifest {
    pub id: String,
    #[serde(default, rename = "rule")]
    pub rules: Vec<Rule>,
}

#[derive(Debug, Deserialize)]
pub struct Rule {
    /// Higher wins; rules are evaluated in priority order.
    #[serde(default)]
    pub priority: i32,
    pub state: RuleState,
    /// Where to look: "bottom" (last non-empty screen lines) or "title".
    #[serde(default = "default_region")]
    pub region: String,
    /// All must be present (case-insensitive substrings).
    #[serde(default)]
    pub all_of: Vec<String>,
    /// At least one must be present.
    #[serde(default)]
    pub any_of: Vec<String>,
    /// None may be present.
    #[serde(default)]
    pub none_of: Vec<String>,
}

fn default_region() -> String {
    "bottom".to_string()
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuleState {
    Working,
    Blocked,
    Done,
    Idle,
}

impl From<RuleState> for Status {
    fn from(s: RuleState) -> Status {
        match s {
            RuleState::Working => Status::Working,
            RuleState::Blocked => Status::Blocked,
            RuleState::Done => Status::Done,
            RuleState::Idle => Status::Idle,
        }
    }
}

/// Classify a pane's screen against a manifest. None → no rule matched
/// (caller falls back to the PTY-activity heuristic).
pub fn classify(manifest: &Manifest, title: &str, bottom_lines: &[String]) -> Option<Status> {
    let bottom = bottom_lines.join("\n").to_lowercase();
    let title = title.to_lowercase();

    let mut rules: Vec<&Rule> = manifest.rules.iter().collect();
    rules.sort_by_key(|r| -r.priority);

    for rule in rules {
        let text = match rule.region.as_str() {
            "title" => title.as_str(),
            _ => bottom.as_str(),
        };
        let all = rule.all_of.iter().all(|s| text.contains(&s.to_lowercase()));
        let any =
            rule.any_of.is_empty() || rule.any_of.iter().any(|s| text.contains(&s.to_lowercase()));
        let none = rule.none_of.iter().all(|s| !text.contains(&s.to_lowercase()));
        if all && any && none {
            return Some(rule.state.into());
        }
    }
    None
}

/// Bundled manifests, compiled in (source of truth per ARCHITECTURE §4).
pub fn bundled() -> Vec<Manifest> {
    [
        include_str!("manifests/claude.toml"),
        include_str!("manifests/codex.toml"),
        include_str!("manifests/opencode.toml"),
    ]
    .iter()
    .filter_map(|text| match toml::from_str::<Manifest>(text) {
        Ok(m) => Some(m),
        Err(e) => {
            tracing::error!(error = %e, "bad bundled manifest");
            None
        }
    })
    .collect()
}

pub fn manifest_for<'a>(manifests: &'a [Manifest], agent: &str) -> Option<&'a Manifest> {
    manifests.iter().find(|m| m.id == agent)
}

/// Bundled manifests with local overrides applied: a file in
/// ~/.config/comind-dock/manifests/ replaces the bundled manifest with the
/// same id (or adds a new agent).
pub fn load_all() -> Vec<Manifest> {
    let mut manifests = bundled();
    let Some(dir) = crate::config::config_path(None).and_then(|p| p.parent().map(|d| d.join("manifests")))
    else {
        return manifests;
    };
    let Ok(entries) = std::fs::read_dir(&dir) else { return manifests };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "toml") {
            continue;
        }
        match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| {
            toml::from_str::<Manifest>(&t).map_err(|e| e.to_string())
        }) {
            Ok(m) => {
                tracing::info!(id = %m.id, path = %path.display(), "manifest override");
                manifests.retain(|b| b.id != m.id);
                manifests.push(m);
            }
            Err(e) => tracing::warn!(path = %path.display(), error = %e, "bad manifest override"),
        }
    }
    manifests
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn claude() -> Manifest {
        toml::from_str(include_str!("manifests/claude.toml")).expect("claude manifest parses")
    }

    #[test]
    fn bundled_manifests_parse() {
        let m = bundled();
        assert!(m.len() >= 3);
        assert!(m.iter().all(|m| !m.rules.is_empty()));
    }

    #[test]
    fn claude_states() {
        let m = claude();
        assert_eq!(
            classify(&m, "", &lines(&["✻ Churning… (3s · esc to interrupt)"])),
            Some(Status::Working)
        );
        assert_eq!(
            classify(&m, "", &lines(&["Do you want to proceed?", "❯ 1. Yes"])),
            Some(Status::Blocked)
        );
        assert_eq!(
            classify(&m, "", &lines(&["❯ ", "? for shortcuts"])),
            Some(Status::Idle)
        );
        assert_eq!(classify(&m, "", &lines(&["random text"])), None);
    }

    /// v2.1.206 spinner has no "esc to interrupt", and the input box with
    /// mode hints stays on screen mid-task — the token counter must win.
    #[test]
    fn v2_spinner_beats_persistent_input_box() {
        let m = claude();
        assert_eq!(
            classify(
                &m,
                "",
                &lines(&[
                    "· Dilly-dallying… (running stop hooks… 1/2 · 6s · ↓ 2 tokens)",
                    "❯",
                    "⏸ manual mode on · ← for agents",
                ])
            ),
            Some(Status::Working)
        );
        assert_eq!(
            classify(
                &m,
                "",
                &lines(&["✳ Churning… (5s · ↑ 1.2k tokens)", "❯", "? for shortcuts"])
            ),
            Some(Status::Working)
        );
    }

    #[test]
    fn spinner_overrides_stale_dialog_text() {
        // "do you want" can linger in transcript text while the agent is
        // already working again — the live spinner hint wins via none_of.
        let m = claude();
        assert_eq!(
            classify(&m, "", &lines(&["Do you want to proceed?", "esc to interrupt"])),
            Some(Status::Working)
        );
    }
}
