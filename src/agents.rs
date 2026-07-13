//! Agent recognition and session continuation. Identity comes from live
//! process exe paths (`detect_process`, exact component match); the OSC
//! title word-match (`detect`) survives only as the fallback for
//! interpreter-hosted installs (npm claude runs as "node"). Statuses live
//! in the detect module's manifests, not here.

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
            "opencode" => format!("opencode --session {session}"),
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
    /// CLAUDE_CONFIG_DIR profile the session belongs to; None = default
    /// (~/.claude). Short label for menus: profile_label().
    pub config_dir: Option<std::path::PathBuf>,
    /// Transcript mtime — global recency ordering across profiles.
    pub mtime: std::time::SystemTime,
}

impl ClaudeSession {
    /// "oleh" for ~/.claude-oleh, None for the default profile.
    pub fn profile_label(&self) -> Option<String> {
        profile_label_from_dir(&self.config_dir.as_ref()?.to_string_lossy())
    }
}

/// A spawned agent lives where its parent lives: unless the cdock profile
/// pinned its own CLAUDE_CONFIG_DIR, the new pane inherits the launching
/// context's claude profile (the CLI's own env, or the pane the UI spawned
/// from). Without this, an agent running as @oleh spawns subagents into the
/// DEFAULT profile — a different set of conversations and settings.
pub fn inherit_claude_profile(env: &mut Vec<(String, String)>, parent_dir: Option<&str>) {
    if env.iter().any(|(k, _)| k == "CLAUDE_CONFIG_DIR") {
        return; // the profile pinned one explicitly — it wins
    }
    if let Some(dir) = parent_dir.filter(|d| !d.trim().is_empty()) {
        env.push(("CLAUDE_CONFIG_DIR".to_string(), dir.to_string()));
    }
}

/// Short profile label from a CLAUDE_CONFIG_DIR path: ".claude-oleh" →
/// "oleh"; the default ".claude" (or anything unnamed) → None.
pub fn profile_label_from_dir(dir: &str) -> Option<String> {
    let name = std::path::Path::new(dir).file_name()?.to_string_lossy();
    match name.strip_prefix(".claude-") {
        Some(rest) if !rest.is_empty() => Some(rest.to_string()),
        _ if name == ".claude" => None,
        // A custom dir without the .claude- convention: use its name as-is.
        _ => Some(name.trim_start_matches('.').to_string()),
    }
}

/// Most recent Claude Code sessions across every project AND every profile
/// on the system (~/.claude*/projects/*/<uuid>.jsonl, newest first). A
/// profile is a CLAUDE_CONFIG_DIR like ~/.claude-oleh; the default ~/.claude
/// carries no profile. Title = first real user message; cwd from the
/// transcript itself (the dir slug is lossy).
pub fn recent_claude_sessions(limit: usize) -> Vec<ClaudeSession> {
    let Some(home) = std::env::var_os("HOME") else { return Vec::new() };
    let home = std::path::PathBuf::from(home);
    let Ok(entries) = std::fs::read_dir(&home) else { return Vec::new() };
    let mut out = Vec::new();
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name == ".claude" || name.starts_with(".claude-") {
            let config_dir = (name != ".claude").then(|| e.path());
            out.extend(sessions_under(&e.path().join("projects"), limit, config_dir));
        }
    }
    out.sort_by_key(|s| std::cmp::Reverse(s.mtime));
    out.truncate(limit);
    out
}

fn sessions_under(
    root: &std::path::Path,
    limit: usize,
    config_dir: Option<std::path::PathBuf>,
) -> Vec<ClaudeSession> {
    let Ok(projects) = std::fs::read_dir(root) else { return Vec::new() };

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
    for (mtime, path) in files {
        if out.len() >= limit {
            break;
        }
        if let Some(mut s) = parse_session(&path) {
            s.config_dir = config_dir.clone();
            s.mtime = mtime;
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
                title = Some(truncate_clean(&t, 48));
            }
        }
        if cwd.is_some() && title.is_some() {
            break;
        }
    }
    Some(ClaudeSession {
        id,
        cwd: cwd?,
        title: title.unwrap_or_else(|| "(no prompt)".into()),
        config_dir: None,
        mtime: std::time::SystemTime::UNIX_EPOCH,
    })
}

/// Which profile (CLAUDE_CONFIG_DIR) owns conversation `id`: scans every
/// ~/.claude*/projects for <id>.jsonl. None → default profile or unknown.
/// Self-heals snapshots that predate profile-env tracking.
pub fn find_session_profile(id: &str) -> Option<std::path::PathBuf> {
    let home = std::path::PathBuf::from(std::env::var_os("HOME")?);
    let entries = std::fs::read_dir(&home).ok()?;
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !(name == ".claude" || name.starts_with(".claude-")) || !e.path().is_dir() {
            continue;
        }
        let projects = e.path().join("projects");
        let Ok(dirs) = std::fs::read_dir(&projects) else { continue };
        for d in dirs.flatten() {
            if d.path().join(format!("{id}.jsonl")).exists() {
                // The default profile needs no env override.
                return (name != ".claude").then(|| e.path());
            }
        }
    }
    None
}

/// Session id of the newest codex/opencode conversation for `cwd` created
/// or touched after `since` — lets the runtime bind a pane to the session
/// the agent it just spawned created, since neither CLI has a SessionStart
/// hook. None (agent unknown, dirs absent, nothing matches) keeps the
/// current picker fallback.
pub fn newest_agent_session(
    agent: &str,
    cwd: &std::path::Path,
    since: std::time::SystemTime,
) -> Option<String> {
    let home = std::path::PathBuf::from(std::env::var_os("HOME")?);
    match agent {
        // ~/.codex/sessions/YYYY/MM/DD/rollout-<ts>-<uuid>.jsonl
        "codex" => {
            let root = std::env::var_os("CODEX_HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| home.join(".codex"));
            newest_codex_session(&root.join("sessions"), cwd, since)
        }
        // $XDG_DATA_HOME/opencode/**/ses_*.json (session info records)
        "opencode" => {
            let data = std::env::var_os("XDG_DATA_HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| home.join(".local/share"));
            newest_opencode_session(&data.join("opencode"), cwd, since)
        }
        _ => None,
    }
}

/// Files under `dir` (recursing at most `depth` levels) with their mtimes.
fn walk_files(dir: &std::path::Path, depth: usize, out: &mut Vec<(std::time::SystemTime, std::path::PathBuf)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if depth > 0 {
                walk_files(&p, depth - 1, out);
            }
        } else if let Ok(m) = e.metadata()
            && let Ok(t) = m.modified()
        {
            out.push((t, p));
        }
    }
}

/// Ids go straight into a shell command line — only accept the uuid-ish
/// shapes both CLIs actually produce.
fn safe_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Codex rollouts: sessions/YYYY/MM/DD/rollout-<timestamp>-<uuid>.jsonl,
/// head line `{"type":"session_meta","payload":{"id":...,"cwd":...}}`
/// (earlier builds put id/cwd at the top level; legacy TS-codex flat
/// rollout-*.json files carry no cwd and no resume support → skipped).
fn newest_codex_session(
    sessions: &std::path::Path,
    cwd: &std::path::Path,
    since: std::time::SystemTime,
) -> Option<String> {
    use std::io::BufRead;
    let mut files = Vec::new();
    walk_files(sessions, 3, &mut files);
    files.retain(|(mtime, p)| {
        *mtime > since
            && p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("rollout-"))
            && p.extension().is_some_and(|x| x == "jsonl")
    });
    files.sort_by_key(|(mtime, _)| std::cmp::Reverse(*mtime));
    for (_, path) in files {
        let Ok(f) = std::fs::File::open(&path) else { continue };
        for line in std::io::BufReader::new(f).lines().map_while(Result::ok).take(5) {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
            let meta = if v["type"] == "session_meta" { &v["payload"] } else { &v };
            if let (Some(id), Some(c)) = (meta["id"].as_str(), meta["cwd"].as_str()) {
                if std::path::Path::new(c) == cwd && safe_id(id) {
                    return Some(id.to_string());
                }
                break; // meta found but wrong cwd → next file
            }
        }
    }
    None
}

/// Opencode session info: <data>/opencode/storage/session/<project>/ses_*.json
/// (older builds: <data>/opencode/project/<slug>/storage/session/info/ses_*.json);
/// the JSON's `directory` field is the session's cwd.
fn newest_opencode_session(
    root: &std::path::Path,
    cwd: &std::path::Path,
    since: std::time::SystemTime,
) -> Option<String> {
    let mut files = Vec::new();
    walk_files(root, 6, &mut files);
    files.retain(|(mtime, p)| {
        *mtime > since
            && p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("ses_"))
            && p.extension().is_some_and(|x| x == "json")
    });
    files.sort_by_key(|(mtime, _)| std::cmp::Reverse(*mtime));
    for (_, path) in files {
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else { continue };
        if v["directory"].as_str().map(std::path::Path::new) != Some(cwd) {
            continue;
        }
        let id = v["id"]
            .as_str()
            .map(str::to_string)
            .or_else(|| Some(path.file_stem()?.to_string_lossy().into_owned()))?;
        if safe_id(&id) {
            return Some(id);
        }
    }
    None
}

/// Wrap a resume command so a failed resume (missing session, wrong
/// profile) degrades the pane into a shell with the error visible instead
/// of dying instantly — an instant exit cascades into closing the tab and
/// possibly the whole space.
pub fn hold_on_failure(cmd: &str) -> String {
    format!("{cmd} || exec \"${{SHELL:-/bin/sh}}\"")
}

/// Truncate to at most `n` chars without stranding a ZWJ/variation
/// selector/combining mark at the cut point (emoji-heavy OSC titles).
pub fn truncate_clean(s: &str, n: usize) -> String {
    let mut out: String = s.chars().take(n).collect();
    while out
        .chars()
        .last()
        .is_some_and(|c| c == '\u{200D}' || ('\u{FE00}'..='\u{FE0F}').contains(&c) || ('\u{0300}'..='\u{036F}').contains(&c))
    {
        out.pop();
    }
    out
}

/// Agent id from a process's executable path: a path COMPONENT must equal
/// the agent name exactly ("~/.local/share/claude/versions/2.1.206" hits
/// "claude"; "goose-sim" or "pi-app" folders do not). Precise on purpose —
/// this is the poll-time source of truth, unlike the fuzzier title match.
pub fn detect_process(ident: &str) -> Option<&'static str> {
    let lower = ident.to_ascii_lowercase();
    std::path::Path::new(&lower)
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(os) => os.to_str(),
            _ => None,
        })
        .find_map(|comp| KNOWN.iter().find(|a| **a == comp).copied())
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
        assert_eq!(resume_command("opencode:ses_9y1"), "opencode --session ses_9y1");
        assert_eq!(resume_command("claude"), "claude --resume"); // no id → picker
        assert_eq!(resume_command("goose"), "goose");
    }

    fn set_mtime(path: &std::path::Path, t: std::time::SystemTime) {
        let f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        f.set_times(std::fs::FileTimes::new().set_modified(t)).unwrap();
    }

    #[test]
    fn newest_codex_session_picks_newest_after_since_for_cwd() {
        let root = std::env::temp_dir().join(format!("cdock-codex-{}", std::process::id()));
        let day = root.join("2026/07/11");
        std::fs::create_dir_all(&day).unwrap();
        let since = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        let mk = |name: &str, meta: &str, age_secs: u64| {
            let p = day.join(name);
            std::fs::write(&p, meta).unwrap();
            set_mtime(&p, std::time::SystemTime::now() - std::time::Duration::from_secs(age_secs));
        };
        let meta = |id: &str, cwd: &str| {
            format!(r#"{{"type":"session_meta","payload":{{"id":"{id}","cwd":"{cwd}"}}}}"#)
        };
        mk("rollout-a.jsonl", &meta("too-old", "/p/x"), 120); // before since
        mk("rollout-b.jsonl", &meta("other-dir", "/p/y"), 5);
        mk("rollout-c.jsonl", &meta("older-match", "/p/x"), 30);
        mk("rollout-d.jsonl", &meta("newest-match", "/p/x"), 10);
        // Head-level meta (early Rust codex, no envelope) — newest overall
        // but wrong cwd; must not shadow the match.
        mk("rollout-e.jsonl", r#"{"id":"flat-meta","cwd":"/p/z"}"#, 1);
        // Legacy TS-codex flat .json: no resume support, ignored.
        std::fs::write(root.join("rollout-legacy.json"), r#"{"session":{"id":"old"}}"#).unwrap();

        let cwd = std::path::Path::new("/p/x");
        assert_eq!(newest_codex_session(&root, cwd, since).as_deref(), Some("newest-match"));
        assert_eq!(
            newest_codex_session(&root, std::path::Path::new("/p/z"), since).as_deref(),
            Some("flat-meta"),
            "un-enveloped head meta is parsed too"
        );
        assert_eq!(newest_codex_session(&root, std::path::Path::new("/nope"), since), None);
        assert_eq!(newest_codex_session(&root.join("missing"), cwd, since), None);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn newest_opencode_session_matches_directory_field() {
        let root = std::env::temp_dir().join(format!("cdock-oc-{}", std::process::id()));
        let dir = root.join("storage/session/proj");
        std::fs::create_dir_all(&dir).unwrap();
        let since = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        let mk = |name: &str, body: &str, age_secs: u64| {
            let p = dir.join(name);
            std::fs::write(&p, body).unwrap();
            set_mtime(&p, std::time::SystemTime::now() - std::time::Duration::from_secs(age_secs));
        };
        mk("ses_old.json", r#"{"id":"ses_old","directory":"/p/x"}"#, 120); // before since
        mk("ses_other.json", r#"{"id":"ses_other","directory":"/p/y"}"#, 5);
        mk("ses_hit.json", r#"{"id":"ses_hit","directory":"/p/x"}"#, 10);
        // Non-session storage (messages/parts) never matches the prefix.
        std::fs::write(dir.join("msg_1.json"), r#"{"id":"msg_1"}"#).unwrap();

        let cwd = std::path::Path::new("/p/x");
        assert_eq!(newest_opencode_session(&root, cwd, since).as_deref(), Some("ses_hit"));
        assert_eq!(newest_opencode_session(&root, std::path::Path::new("/nope"), since), None);
        assert_eq!(newest_opencode_session(&root.join("missing"), cwd, since), None);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn newest_agent_session_unknown_agent_is_none() {
        assert_eq!(
            newest_agent_session("goose", std::path::Path::new("/p"), std::time::SystemTime::now()),
            None
        );
    }

    #[test]
    fn spawned_agents_inherit_the_parents_claude_profile() {
        // No parent profile → nothing added (default ~/.claude).
        let mut env = Vec::new();
        inherit_claude_profile(&mut env, None);
        assert!(env.is_empty());

        // Parent runs as @oleh → the child does too.
        let mut env = vec![("FOO".to_string(), "1".to_string())];
        inherit_claude_profile(&mut env, Some("/home/u/.claude-oleh"));
        assert_eq!(
            env.iter().find(|(k, _)| k == "CLAUDE_CONFIG_DIR").map(|(_, v)| v.as_str()),
            Some("/home/u/.claude-oleh")
        );

        // A cdock profile that pinned its own profile wins over the parent.
        let mut env = vec![("CLAUDE_CONFIG_DIR".to_string(), "/home/u/.claude-sci".to_string())];
        inherit_claude_profile(&mut env, Some("/home/u/.claude-oleh"));
        assert_eq!(env.len(), 1);
        assert_eq!(env[0].1, "/home/u/.claude-sci");
    }

    fn write_jsonl(dir: &std::path::Path, name: &str, lines: &[&str]) -> std::path::PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(format!("{name}.jsonl"));
        std::fs::write(&path, lines.join("\n")).unwrap();
        path
    }

    #[test]
    fn parse_session_takes_first_real_user_message() {
        let dir = std::env::temp_dir().join(format!("cdock-sess-a-{}", std::process::id()));
        let path = write_jsonl(
            &dir,
            "aaaa-1111",
            &[
                r#"{"type":"mode","mode":"normal","sessionId":"aaaa-1111"}"#,
                r#"{"type":"user","message":{"role":"user","content":"<local-command-caveat>skip me"},"cwd":"/projects/alpha"}"#,
                r#"{"type":"user","isSidechain":true,"message":{"role":"user","content":"sidechain noise"}}"#,
                r#"{"type":"user","message":{"role":"user","content":"справжнє питання про код"}}"#,
            ],
        );
        let s = parse_session(&path).expect("parses");
        assert_eq!(s.id, "aaaa-1111");
        assert_eq!(s.cwd, std::path::PathBuf::from("/projects/alpha"));
        assert_eq!(s.title, "справжнє питання про код");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn parse_session_block_content_and_truncation() {
        let dir = std::env::temp_dir().join(format!("cdock-sess-b-{}", std::process::id()));
        let long = "x".repeat(80);
        let line = format!(
            r#"{{"type":"user","cwd":"/p","message":{{"role":"user","content":[{{"type":"image"}},{{"type":"text","text":"{long}"}}]}}}}"#
        );
        let path = write_jsonl(&dir, "bbbb", &[&line]);
        let s = parse_session(&path).expect("parses");
        assert_eq!(s.title.chars().count(), 48, "titles are truncated");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn parse_session_requires_cwd_but_not_title() {
        let dir = std::env::temp_dir().join(format!("cdock-sess-c-{}", std::process::id()));
        // No cwd anywhere → unusable for resume-in-place → None.
        let p1 = write_jsonl(&dir, "no-cwd", &[r#"{"type":"mode","mode":"normal"}"#]);
        assert!(parse_session(&p1).is_none());
        // cwd but no user message → placeholder title.
        let p2 = write_jsonl(&dir, "no-title", &[r#"{"type":"assistant","cwd":"/p"}"#]);
        assert_eq!(parse_session(&p2).unwrap().title, "(no prompt)");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn sessions_under_orders_newest_first_and_limits() {
        let root = std::env::temp_dir().join(format!("cdock-sess-d-{}", std::process::id()));
        let mk = |slug: &str, name: &str, age_secs: u64| {
            let path = write_jsonl(
                &root.join(slug),
                name,
                &[&format!(
                    r#"{{"type":"user","cwd":"/p/{name}","message":{{"role":"user","content":"{name}"}}}}"#
                )],
            );
            let mtime = std::time::SystemTime::now() - std::time::Duration::from_secs(age_secs);
            let f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
            f.set_times(std::fs::FileTimes::new().set_modified(mtime)).unwrap();
        };
        mk("proj-one", "old", 300);
        mk("proj-one", "new", 10);
        mk("proj-two", "mid", 100);
        std::fs::write(root.join("proj-one/notes.txt"), "not a session").unwrap();

        let all = sessions_under(&root, 10, None);
        let ids: Vec<&str> = all.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["new", "mid", "old"], "newest first, across projects");

        let limited = sessions_under(&root, 2, None);
        assert_eq!(limited.len(), 2);
        assert_eq!(limited[0].id, "new");

        assert!(sessions_under(&root.join("missing"), 5, None).is_empty());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn profile_labels() {
        assert_eq!(profile_label_from_dir("/Users/x/.claude"), None);
        assert_eq!(profile_label_from_dir("/Users/x/.claude-oleh"), Some("oleh".into()));
        assert_eq!(profile_label_from_dir("/Users/x/.claude-science"), Some("science".into()));
        assert_eq!(profile_label_from_dir("/opt/custom-claude-cfg"), Some("custom-claude-cfg".into()));
    }

    /// Live-system sanity: `cargo test -- --ignored print_recent`.
    #[test]
    #[ignore]
    fn print_recent_sessions() {
        for s in recent_claude_sessions(8) {
            eprintln!(
                "{} · {} · {} · @{}",
                s.id,
                s.title,
                s.cwd.display(),
                s.profile_label().unwrap_or_else(|| "default".into())
            );
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
