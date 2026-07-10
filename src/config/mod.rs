//! TOML config: Phase 1 subset of docs/CONFIGURATION.md.
//! `Config::default()` is the source of truth; the embedded annotated file
//! must deserialize back to it (round-trip test below). Invalid file →
//! warn and run on defaults; invalid single binding → warn and skip.

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
    pub advanced: AdvancedCfg,
    pub experimental: ExperimentalCfg,
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
}

impl Default for TerminalCfg {
    fn default() -> Self {
        Self {
            default_shell: String::new(),
            shell_mode: ShellMode::Auto,
            new_cwd: "follow".to_string(),
        }
    }
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
        }
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
    /// alacritty takes scrollback in lines; ~120 bytes/line heuristic
    /// (documented in the annotated default config).
    pub fn scrollback_lines(&self) -> usize {
        (self.scrollback_limit_bytes / 120) as usize
    }
}

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
    match toml::from_str::<Config>(&text) {
        Ok(cfg) => (cfg, warnings),
        Err(e) => {
            // ponytail: whole-file fallback; per-field lenient recovery is a
            // documented gap until it earns its complexity.
            warnings.push(format!("config error in {}: {e}; using defaults", path.display()));
            (Config::default(), warnings)
        }
    }
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
}
