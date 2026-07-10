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

/// A resumable Claude Code conversation found on this system.
pub struct ClaudeSession {
    pub id: String,
    pub cwd: std::path::PathBuf,
    pub title: String,
}

/// Most recent Claude Code sessions across every project on the system
/// (~/.claude/projects/*/<uuid>.jsonl, newest first). Title = first real
/// user message; cwd from the transcript itself (the dir slug is lossy).
pub fn recent_claude_sessions(limit: usize) -> Vec<ClaudeSession> {
    let Some(home) = std::env::var_os("HOME") else { return Vec::new() };
    let root = std::path::PathBuf::from(home).join(".claude/projects");
    let Ok(projects) = std::fs::read_dir(&root) else { return Vec::new() };

    let mut files: Vec<(std::time::SystemTime, std::path::PathBuf)> = projects
        .flatten()
        .filter_map(|p| std::fs::read_dir(p.path()).ok())
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    files.sort_by_key(|(mtime, _)| std::cmp::Reverse(*mtime));

    let mut out = Vec::new();
    for (_, path) in files {
        if out.len() >= limit {
            break;
        }
        if let Some(s) = parse_session(&path) {
            out.push(s);
        }
    }
    out
}

fn parse_session(path: &std::path::Path) -> Option<ClaudeSession> {
    use std::io::BufRead;
    let id = path.file_stem()?.to_string_lossy().into_owned();
    let reader = std::io::BufReader::new(std::fs::File::open(path).ok()?);
    let (mut cwd, mut title) = (None, None);
    for line in reader.lines().map_while(Result::ok).take(300) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
        if cwd.is_none()
            && let Some(c) = v["cwd"].as_str()
        {
            cwd = Some(std::path::PathBuf::from(c));
        }
        if title.is_none() && v["type"] == "user" && v["isSidechain"] != true {
            let c = &v["message"]["content"];
            let t = c.as_str().map(str::to_string).or_else(|| {
                c.as_array()?
                    .iter()
                    .find(|b| b["type"] == "text")
                    .and_then(|b| b["text"].as_str().map(str::to_string))
            });
            // Skip synthetic user entries (command caveats, hook output).
            if let Some(t) =
                t.map(|t| t.trim().to_string()).filter(|t| !t.is_empty() && !t.starts_with('<'))
            {
                title = Some(t.chars().take(48).collect());
            }
        }
        if cwd.is_some() && title.is_some() {
            break;
        }
    }
    Some(ClaudeSession { id, cwd: cwd?, title: title.unwrap_or_else(|| "(no prompt)".into()) })
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

    /// Live-system sanity: `cargo test -- --ignored print_recent`.
    #[test]
    #[ignore]
    fn print_recent_sessions() {
        for s in recent_claude_sessions(6) {
            eprintln!("{} · {} · {}", s.id, s.title, s.cwd.display());
        }
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
