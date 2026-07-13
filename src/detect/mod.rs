//! Agent detection engine (ARCHITECTURE §4): a screen-snapshot pattern
//! matcher. Reads the pane's bottom-of-buffer text plus the OSC title,
//! runs the agent's ordered manifest rules, highest priority wins.
//! Manifests: bundled, overridden by ~/.config/comind-dock/manifests/*.toml
//! (matched by id), hot-reloadable via `cdock server reload-manifests`.
//! Manifests ship with the binary; user overrides live in
//! ~/.config/comind-dock/manifests. ponytail: no remote manifest feed —
//! `cdock update` ships new manifests with each release, which is enough
//! until detection rules need to move faster than releases do.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
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

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
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
    // ponytail: delegates to the explain path so the two can never diverge;
    // the extra per-rule trace allocation is a few tiny strings per tick.
    classify_explain(manifest, title, bottom_lines).outcome
}

/// One pattern string from a rule and whether the region text contained it.
#[derive(Debug, Serialize)]
pub struct PatternCheck {
    pub pattern: String,
    pub found: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// all_of + any_of satisfied, no none_of present.
    Matched,
    /// all_of or any_of not satisfied.
    Failed,
    /// Would have matched, but a none_of string was present.
    Vetoed,
}

/// Trace of one rule evaluation, in priority order.
#[derive(Debug, Serialize)]
pub struct RuleTrace {
    pub priority: i32,
    pub state: RuleState,
    pub region: String,
    pub all_of: Vec<PatternCheck>,
    pub any_of: Vec<PatternCheck>,
    pub none_of: Vec<PatternCheck>,
    pub verdict: Verdict,
}

/// Full reasoning for one classification: every rule's evaluation plus the
/// exact inputs and the final outcome (None → activity-heuristic fallback).
#[derive(Debug, Serialize)]
pub struct Explain {
    pub manifest_id: String,
    pub title: String,
    pub bottom_lines: Vec<String>,
    pub rules: Vec<RuleTrace>,
    pub outcome: Option<Status>,
}

/// Like `classify`, but records why: every rule in priority order with each
/// pattern's hit/miss and a verdict. `classify()` is exactly `.outcome`.
pub fn classify_explain(manifest: &Manifest, title: &str, bottom_lines: &[String]) -> Explain {
    let bottom = bottom_lines.join("\n").to_lowercase();
    let title_lc = title.to_lowercase();

    let mut rules: Vec<&Rule> = manifest.rules.iter().collect();
    rules.sort_by_key(|r| -r.priority);

    let mut outcome = None;
    let mut traces = Vec::with_capacity(rules.len());
    for rule in rules {
        let text = match rule.region.as_str() {
            "title" => title_lc.as_str(),
            _ => bottom.as_str(),
        };
        let check = |pats: &[String]| -> Vec<PatternCheck> {
            pats.iter()
                .map(|p| PatternCheck {
                    pattern: p.clone(),
                    found: text.contains(&p.to_lowercase()),
                })
                .collect()
        };
        let all_of = check(&rule.all_of);
        let any_of = check(&rule.any_of);
        let none_of = check(&rule.none_of);

        let all = all_of.iter().all(|c| c.found);
        let any = any_of.is_empty() || any_of.iter().any(|c| c.found);
        let verdict = if !(all && any) {
            Verdict::Failed
        } else if none_of.iter().any(|c| c.found) {
            Verdict::Vetoed
        } else {
            Verdict::Matched
        };
        if verdict == Verdict::Matched && outcome.is_none() {
            outcome = Some(rule.state.into());
        }
        traces.push(RuleTrace {
            priority: rule.priority,
            state: rule.state,
            region: rule.region.clone(),
            all_of,
            any_of,
            none_of,
            verdict,
        });
    }
    Explain {
        manifest_id: manifest.id.clone(),
        title: title.to_string(),
        bottom_lines: bottom_lines.to_vec(),
        rules: traces,
        outcome,
    }
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
    let Some(dir) =
        crate::config::config_path(None).and_then(|p| p.parent().map(|d| d.join("manifests")))
    else {
        return manifests;
    };
    let Ok(entries) = std::fs::read_dir(&dir) else { return manifests };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "toml") {
            continue;
        }
        match std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|t| toml::from_str::<Manifest>(&t).map_err(|e| e.to_string()))
        {
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
        assert_eq!(classify(&m, "", &lines(&["❯ ", "? for shortcuts"])), Some(Status::Idle));
        assert_eq!(classify(&m, "", &lines(&["random text"])), None);
    }

    /// Menus the user opened themselves (rewind picker, autocomplete, theme
    /// select) all print an "esc to cancel" hint — the agent is not blocked.
    #[test]
    fn a_user_opened_menu_is_not_a_permission_prompt() {
        let m = claude();
        assert_eq!(
            classify(
                &m,
                "",
                &lines(&[
                    "↑/↓ to select · Enter to confirm · Tab/Esc to cancel",
                    "⏵⏵ auto mode on (shift+tab to cycle)",
                ])
            ),
            Some(Status::Idle)
        );
        // A real approval still blocks.
        assert_eq!(
            classify(&m, "", &lines(&["waiting for your approval"])),
            Some(Status::Blocked)
        );
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
            classify(&m, "", &lines(&["✳ Churning… (5s · ↑ 1.2k tokens)", "❯", "? for shortcuts"])),
            Some(Status::Working)
        );
    }

    #[test]
    fn explain_working_spinner() {
        let m = claude();
        // Stale dialog text + live spinner: blocked rules are vetoed by the
        // spinner hint, the working rule matches.
        let input =
            lines(&["Do you want to proceed?", "❯ 1. Yes", "✻ Churning… (3s · esc to interrupt)"]);
        let ex = classify_explain(&m, "", &input);
        assert_eq!(ex.outcome, Some(Status::Working));
        assert_eq!(ex.outcome, classify(&m, "", &input));
        assert_eq!(ex.bottom_lines, input);
        assert_eq!(ex.rules.len(), m.rules.len());

        let vetoed = ex.rules.iter().find(|r| r.verdict == Verdict::Vetoed).expect("vetoed rule");
        assert_eq!(vetoed.priority, 100);
        assert_eq!(vetoed.state, RuleState::Blocked);
        assert!(vetoed.none_of.iter().any(|c| c.found && c.pattern == "esc to interrupt"));

        let matched =
            ex.rules.iter().find(|r| r.verdict == Verdict::Matched).expect("matched rule");
        assert_eq!(matched.priority, 90);
        assert_eq!(matched.state, RuleState::Working);
        assert!(matched.any_of.iter().any(|c| c.found && c.pattern == "esc to interrupt"));
    }

    #[test]
    fn explain_blocked_dialog() {
        let m = claude();
        let input = lines(&["Do you want to proceed?", "❯ 1. Yes"]);
        let ex = classify_explain(&m, "", &input);
        assert_eq!(ex.outcome, Some(Status::Blocked));
        assert_eq!(ex.outcome, classify(&m, "", &input));

        let matched =
            ex.rules.iter().find(|r| r.verdict == Verdict::Matched).expect("matched rule");
        assert_eq!(matched.priority, 100);
        assert_eq!(matched.state, RuleState::Blocked);
        assert!(matched.all_of.iter().all(|c| c.found));
        assert!(matched.any_of.iter().any(|c| c.found && c.pattern == "do you want to proceed"));
        assert!(matched.none_of.iter().all(|c| !c.found));
        // Serializes for the CLI.
        let json = serde_json::to_string(&ex).expect("explain serializes");
        assert!(json.contains("\"outcome\":\"blocked\""));
        assert!(json.contains("\"verdict\":\"matched\""));
    }

    #[test]
    fn explain_no_match() {
        let m = claude();
        let input = lines(&["random text"]);
        let ex = classify_explain(&m, "", &input);
        assert_eq!(ex.outcome, None);
        assert_eq!(classify(&m, "", &input), None);
        assert_eq!(ex.rules.len(), m.rules.len());
        assert!(ex.rules.iter().all(|r| r.verdict == Verdict::Failed));
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
