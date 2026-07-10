//! Agent detection engine (ARCHITECTURE §4): a screen-snapshot pattern
//! matcher. Reads the pane's bottom-of-buffer text plus the OSC title,
//! runs the agent's ordered manifest rules, highest priority wins.
//! ponytail: bundled manifests only — remote feed, local overrides, and
//! hot reload arrive with the update system.

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
