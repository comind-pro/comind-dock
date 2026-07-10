//! Minimal agent recognition: a pane counts as an agent only when its OSC
//! title or program matches a known agent CLI. Plain shells and ordinary
//! commands are not agents. ponytail: word-match against a bundled list;
//! the Phase 3 detection engine (manifests, rules, states) replaces this.

const KNOWN: &[&str] = &[
    "claude", "codex", "opencode", "aider", "gemini", "goose", "amp", "pi", "cursor", "copilot",
    "droid", "qwen", "crush",
];

/// Command that relaunches an agent after a restart. `ident` is either a
/// bare agent id ("claude") or "agent:session-id" reported by the
/// SessionStart integration hook — with an id the pane resumes exactly its
/// own conversation; without one it opens the CLI's session picker
/// (--continue would silently grab whatever conversation was touched last
/// in that cwd, including ones that never ran in cdock).
pub fn resume_command(ident: &str) -> String {
    if let Some((agent, session)) = ident.split_once(':') {
        return match agent {
            "claude" => format!("claude --resume {session}"),
            "codex" => format!("codex resume {session}"),
            other => other.to_string(),
        };
    }
    match ident {
        "claude" => "claude --resume".to_string(),
        "codex" => "codex resume".to_string(),
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

    #[test]
    fn resume_uses_reported_session_id() {
        assert_eq!(resume_command("claude:abc-123"), "claude --resume abc-123");
        assert_eq!(resume_command("codex:xyz"), "codex resume xyz");
        assert_eq!(resume_command("claude"), "claude --resume"); // no id → picker
        assert_eq!(resume_command("goose"), "goose");
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
