//! TOML config: Phase 1 subset of docs/CONFIGURATION.md.
//! `Config::default()` is the source of truth; the embedded annotated file
//! must deserialize back to it (round-trip test below). Unparseable file →
//! warn and run on defaults; invalid section → warn, default that section,
//! keep the rest; invalid single binding → warn and skip.

pub mod keys;
pub mod theme;

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Config {
    pub theme: ThemeCfg,
    pub terminal: TerminalCfg,
    pub keys: KeysCfg,
    pub ui: UiCfg,
    pub worktrees: WorktreesCfg,
    pub update: UpdateCfg,
    pub advanced: AdvancedCfg,
    pub experimental: ExperimentalCfg,
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(default)]
pub struct UpdateCfg {
    /// Background check for new releases (menu shows "update ready").
    pub check: bool,
    /// GitHub repo the update feed reads from.
    pub repo: String,
    /// `stable` (full releases only) | `preview` (prereleases included).
    pub channel: crate::update::Channel,
}

impl Default for UpdateCfg {
    fn default() -> Self {
        Self {
            check: true,
            repo: "comind-pro/comind-dock".to_string(),
            channel: crate::update::Channel::Stable,
        }
    }
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(default)]
pub struct ThemeCfg {
    /// Built-in theme name; `terminal` follows the host palette.
    pub name: String,
    /// Per-token color overrides: hex `#rrggbb`, named, `rgb(r,g,b)`, `reset`.
    pub custom: BTreeMap<String, String>,
}

impl Default for ThemeCfg {
    fn default() -> Self {
        Self { name: "default".to_string(), custom: BTreeMap::new() }
    }
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(default)]
pub struct TerminalCfg {
    /// Executable for new panes (empty → `$SHELL` → `/bin/sh`).
    pub default_shell: String,
    /// `auto` uses login shells on macOS for PATH setup.
    pub shell_mode: ShellMode,
    /// `follow | home | current | <fixed path>`.
    pub new_cwd: String,
    /// Editor for settings/profiles/skills (empty → $EDITOR → $VISUAL → nano).
    pub editor: String,
}

impl Default for TerminalCfg {
    fn default() -> Self {
        Self {
            default_shell: String::new(),
            shell_mode: ShellMode::Auto,
            new_cwd: "follow".to_string(),
            editor: String::new(),
        }
    }
}

impl TerminalCfg {
    /// The editor command: config wins, then $EDITOR/$VISUAL, then nano —
    /// friendlier than the classic vi fallback for non-vi hands.
    pub fn editor_cmd(&self) -> String {
        if !self.editor.trim().is_empty() {
            return self.editor.clone();
        }
        std::env::var("EDITOR")
            .or_else(|_| std::env::var("VISUAL"))
            .ok()
            .filter(|e| !e.trim().is_empty())
            .unwrap_or_else(|| "nano".to_string())
    }
}

/// Persist `[terminal] editor = "<value>"` into the config file with a
/// targeted line edit — a full toml rewrite would drop the user's comments.
pub fn set_editor(value: &str) -> Result<(), String> {
    let path = config_path(None).ok_or("cannot determine config dir")?;
    set_editor_at(&path, value)
}

fn set_editor_at(path: &std::path::Path, value: &str) -> Result<(), String> {
    if !path.exists() {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(path, DEFAULT_CONFIG).map_err(|e| e.to_string())?;
    }
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let line = format!("editor = {:?}", value);
    let mut out = Vec::new();
    let mut in_terminal = false;
    let mut done = false;
    for l in text.lines() {
        let t = l.trim();
        if t.starts_with('[') {
            // Leaving [terminal] without having found an editor line.
            if in_terminal && !done {
                out.push(line.clone());
                done = true;
            }
            in_terminal = t == "[terminal]";
        } else if in_terminal && !done && t.starts_with("editor") {
            out.push(line.clone());
            done = true;
            continue;
        }
        out.push(l.to_string());
    }
    if !done {
        if !text.contains("[terminal]") {
            out.push("[terminal]".to_string());
        }
        out.push(line);
    }
    std::fs::write(path, out.join("\n") + "\n").map_err(|e| e.to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShellMode {
    Auto,
    Login,
    NonLogin,
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(default)]
pub struct KeysCfg {
    /// The prefix chord.
    pub prefix: String,
    /// Per-action overrides: `split_right = "v"` (after prefix) or a
    /// modifier chord like `"ctrl+alt+t"` (direct, no prefix).
    #[serde(flatten)]
    pub actions: BTreeMap<String, toml::Value>,
    /// `[[keys.command]]` custom bindings.
    #[serde(rename = "command")]
    pub commands: Vec<CustomCommand>,
}

impl Default for KeysCfg {
    fn default() -> Self {
        Self { prefix: "ctrl+b".to_string(), actions: BTreeMap::new(), commands: Vec::new() }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct CustomCommand {
    pub key: String,
    #[serde(rename = "type")]
    pub kind: CommandKind,
    pub command: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandKind {
    /// Run in a new pane (new tab).
    Pane,
    /// Run silently in the background.
    Shell,
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(default)]
pub struct UiCfg {
    pub sidebar_width: u16,
    pub mouse_capture: bool,
    pub mouse_scroll_lines: u16,
    pub confirm_close: bool,
    pub prompt_new_tab_name: bool,
    pub hide_tab_bar_when_single_tab: bool,
    pub sound: SoundCfg,
    pub toast: ToastCfg,
}

impl Default for UiCfg {
    fn default() -> Self {
        Self {
            sidebar_width: 24,
            mouse_capture: true,
            mouse_scroll_lines: 3,
            confirm_close: false,
            prompt_new_tab_name: false,
            hide_tab_bar_when_single_tab: false,
            sound: SoundCfg::default(),
            toast: ToastCfg::default(),
        }
    }
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(default)]
pub struct SoundCfg {
    /// Also killed globally by the CDOCK_DISABLE_SOUND env var.
    pub enabled: bool,
}

impl Default for SoundCfg {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(default)]
pub struct ToastCfg {
    /// app | system | both | off. "app" toasts are clickable (jump to pane).
    pub delivery: String,
}

impl Default for ToastCfg {
    fn default() -> Self {
        Self { delivery: "both".to_string() }
    }
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(default)]
pub struct WorktreesCfg {
    /// Checkout root, laid out as `<dir>/<repo>/<branch-slug>`.
    pub directory: String,
}

impl Default for WorktreesCfg {
    fn default() -> Self {
        Self { directory: "~/.comind-dock/worktrees".to_string() }
    }
}

impl WorktreesCfg {
    pub fn root(&self) -> std::path::PathBuf {
        let d = &self.directory;
        if let Some(rest) = d.strip_prefix("~/")
            && let Some(home) = std::env::var_os("HOME")
        {
            return std::path::PathBuf::from(home).join(rest);
        }
        std::path::PathBuf::from(d)
    }
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(default)]
pub struct AdvancedCfg {
    pub scrollback_limit_bytes: u64,
}

impl Default for AdvancedCfg {
    fn default() -> Self {
        Self { scrollback_limit_bytes: 10_000_000 }
    }
}

#[derive(Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ExperimentalCfg {
    pub allow_nested: bool,
}

impl AdvancedCfg {
    /// alacritty takes scrollback in lines. A stored row is a full-width
    /// Vec<Cell> (~24 B/cell, ~4 KB at 180 cols) — the old 120 B/line
    /// heuristic overshot real memory ~40x. 10k lines is plenty of history.
    pub fn scrollback_lines(&self) -> usize {
        ((self.scrollback_limit_bytes / 4000) as usize).clamp(1000, 10_000)
    }
}

/// The annotated default config, also written on first "settings" open.
pub const DEFAULT_CONFIG: &str = include_str!("../default_config.toml");

/// Config file path: --config > CDOCK_CONFIG_PATH > XDG default.
pub fn config_path(cli_override: Option<PathBuf>) -> Option<PathBuf> {
    if let Some(p) = cli_override {
        return Some(p);
    }
    if let Some(p) = std::env::var_os("CDOCK_CONFIG_PATH") {
        return Some(PathBuf::from(p));
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("comind-dock/config.toml"))
}

/// Load config; on any file/parse error return defaults plus a warning.
pub fn load(cli_override: Option<PathBuf>) -> (Config, Vec<String>) {
    let mut warnings = Vec::new();
    let Some(path) = config_path(cli_override) else {
        return (Config::default(), warnings);
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return (Config::default(), warnings);
        }
        Err(e) => {
            warnings.push(format!("cannot read {}: {e}; using defaults", path.display()));
            return (Config::default(), warnings);
        }
    };
    let (cfg, section_warnings) = from_str(&text);
    warnings.extend(section_warnings.into_iter().map(|w| format!("{}: {w}", path.display())));
    (cfg, warnings)
}

/// Lenient parse: syntax error → all defaults + warning; otherwise
/// per-section recovery via `from_table`.
fn from_str(text: &str) -> (Config, Vec<String>) {
    match text.parse::<toml::Table>() {
        Ok(table) => from_table(table),
        Err(e) => (Config::default(), vec![format!("config error: {e}; using defaults")]),
    }
}

/// Deserialize each known top-level section independently; a bad section
/// falls back to its default with a warning, the rest survive. Unknown
/// sections are ignored.
fn from_table(table: toml::Table) -> (Config, Vec<String>) {
    let mut cfg = Config::default();
    let mut warnings = Vec::new();
    macro_rules! section {
        ($name:literal, $field:ident) => {
            if let Some(v) = table.get($name) {
                match v.clone().try_into() {
                    Ok(s) => cfg.$field = s,
                    Err(e) => warnings.push(format!(
                        "config section [{}] invalid: {e}; using defaults for it",
                        $name
                    )),
                }
            }
        };
    }
    section!("theme", theme);
    section!("terminal", terminal);
    section!("keys", keys);
    section!("ui", ui);
    section!("worktrees", worktrees);
    section!("update", update);
    section!("advanced", advanced);
    section!("experimental", experimental);
    (cfg, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_file_round_trips() {
        let text = include_str!("../default_config.toml");
        let parsed: Config = toml::from_str(text).expect("default config must parse");
        assert_eq!(parsed, Config::default(), "default_config.toml drifted from Config::default()");
    }

    #[test]
    fn partial_config_fills_defaults() {
        let cfg: Config = toml::from_str("[ui]\nsidebar_width = 30\n").unwrap();
        assert_eq!(cfg.ui.sidebar_width, 30);
        assert_eq!(cfg.ui.mouse_scroll_lines, 3);
        assert_eq!(cfg.keys.prefix, "ctrl+b");
    }

    #[test]
    fn custom_commands_parse() {
        let cfg: Config = toml::from_str(
            r#"
[[keys.command]]
key = "g"
type = "pane"
command = "lazygit"
description = "open lazygit"
"#,
        )
        .unwrap();
        assert_eq!(cfg.keys.commands.len(), 1);
        assert_eq!(cfg.keys.commands[0].kind, CommandKind::Pane);
    }

    #[test]
    fn update_channel_parses_and_defaults_to_stable() {
        assert_eq!(Config::default().update.channel, crate::update::Channel::Stable);
        let cfg: Config = toml::from_str("[update]\nchannel = \"preview\"\n").unwrap();
        assert_eq!(cfg.update.channel, crate::update::Channel::Preview);
    }

    #[test]
    fn bad_section_defaults_others_survive() {
        let table: toml::Table =
            "[update]\ncheck = \"yes\"\n[keys]\nprefix = \"ctrl+a\"\n".parse().unwrap();
        let (cfg, warnings) = from_table(table);
        assert_eq!(cfg.keys.prefix, "ctrl+a");
        assert_eq!(cfg.update, UpdateCfg::default());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("update"), "warning must name the section: {warnings:?}");
    }

    #[test]
    fn broken_toml_all_defaults_with_warning() {
        let (cfg, warnings) = from_str("[update\ncheck =");
        assert_eq!(cfg, Config::default());
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn set_editor_preserves_comments_and_replaces_line() {
        let dir = std::env::temp_dir().join(format!("cdock-ed-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(
            &path,
            "# top comment\n[terminal]\nnew_cwd = \"follow\" # keep me\n[keys]\nprefix = \"ctrl+b\"\n",
        )
        .unwrap();
        set_editor_at(&path, "vim").unwrap();
        let t = std::fs::read_to_string(&path).unwrap();
        assert!(t.contains("# top comment") && t.contains("# keep me"), "{t}");
        assert!(t.contains("editor = \"vim\""), "{t}");
        let (cfg, w) = from_str(&t);
        assert!(w.is_empty());
        assert_eq!(cfg.terminal.editor, "vim");
        assert_eq!(cfg.keys.prefix, "ctrl+b");
        // Replace, not duplicate.
        set_editor_at(&path, "nano").unwrap();
        let t = std::fs::read_to_string(&path).unwrap();
        assert_eq!(t.matches("editor =").count(), 1, "{t}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn unknown_section_ignored_silently() {
        let (cfg, warnings) = from_str("[ponies]\nrainbow = true\n");
        assert_eq!(cfg, Config::default());
        assert!(warnings.is_empty());
    }
}
