//! Minimal agent recognition: a pane counts as an agent only when its OSC
//! title or program matches a known agent CLI. Plain shells and ordinary
//! commands are not agents. ponytail: word-match against a bundled list;
//! the Phase 3 detection engine (manifests, rules, states) replaces this.

const KNOWN: &[&str] = &[
    "claude", "codex", "opencode", "aider", "gemini", "goose", "amp", "pi", "cursor", "copilot",
    "droid", "qwen", "crush",
];

/// Command that relaunches an agent back into its latest conversation in
/// the same folder. ponytail: real session references (ids via integration
/// hooks) are Phase 5 agent resume; this covers the common CLIs today.
pub fn resume_command(agent: &str) -> String {
    match agent {
        "claude" => "claude --continue".to_string(),
        "codex" => "codex resume --last".to_string(),
        "opencode" => "opencode --continue".to_string(),
        other => other.to_string(),
    }
}

/// Agent id if the pane looks like a known agent CLI.
pub fn detect(title: &str, program: &str) -> Option<&'static str> {
    let prog = program.to_ascii_lowercase();
    if let Some(a) = KNOWN.iter().find(|a| **a == prog) {
        return Some(a);
    }
    // Title words: "✳ Claude Code" → ["claude", "code"].
    let lower = title.to_ascii_lowercase();
    let words: Vec<&str> =
        lower.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).collect();
    KNOWN.iter().find(|a| words.iter().any(|w| w == *a)).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_agents_not_shells() {
        assert_eq!(detect("✳ Claude Code", "zsh"), Some("claude"));
        assert_eq!(detect("", "codex"), Some("codex"));
        assert_eq!(detect("opencode – main", "zsh"), Some("opencode"));
        assert_eq!(detect("~/projects", "zsh"), None);
        assert_eq!(detect("vim src/main.rs", "sh"), None);
        // "pi" must not match inside other words
        assert_eq!(detect("copying files", "bash"), None);
        assert_eq!(detect("pi", "zsh"), Some("pi"));
    }

    /// Child exe paths are matched as titles; the version segment is noise —
    /// any future Claude Code version under a claude/ dir must keep matching.
    #[test]
    fn exe_path_matches_regardless_of_version() {
        assert_eq!(detect("/Users/x/.local/share/claude/versions/2.1.206", ""), Some("claude"));
        assert_eq!(detect("/Users/x/.local/share/claude/versions/99.0.1-beta", ""), Some("claude"));
        assert_eq!(detect("/opt/homebrew/bin/codex", ""), Some("codex"));
        assert_eq!(detect("/usr/local/bin/rg", ""), None);
    }
}
