//! Binding syntax → chords, and the resolved keymap the dispatcher uses.

use crossterm::event::{KeyCode, KeyModifiers};

use super::{CustomCommand, KeysCfg};
use crate::input::{Action, Chord, default_actions};

/// A resolved binding: how it's typed, what it does, how help shows it.
#[derive(Debug, Clone)]
pub struct KeyEntry {
    pub chord: Chord,
    /// Direct chords fire from terminal mode without the prefix.
    pub direct: bool,
    pub label: String,
    pub bound: Bound,
}

#[derive(Debug, Clone)]
pub enum Bound {
    Builtin(Action),
    Command(CustomCommand),
}

#[derive(Debug, Clone)]
pub struct Keymap {
    pub prefix: Chord,
    pub entries: Vec<KeyEntry>,
}

impl Keymap {
    pub fn lookup_prefixed(&self, code: KeyCode, mods: KeyModifiers) -> Option<&KeyEntry> {
        self.entries.iter().find(|e| !e.direct && chord_matches(e.chord, code, mods))
    }

    pub fn lookup_direct(&self, code: KeyCode, mods: KeyModifiers) -> Option<&KeyEntry> {
        self.entries.iter().find(|e| e.direct && chord_matches(e.chord, code, mods))
    }
}

/// Chars carry their case/symbol already — ignore SHIFT for them.
fn chord_matches(chord: Chord, code: KeyCode, mods: KeyModifiers) -> bool {
    let strip = |m: KeyModifiers| m & !KeyModifiers::SHIFT;
    chord.code == code && strip(chord.mods) == strip(mods)
}

/// Parse `"ctrl+shift+t"`, `"prefix+v"`, `"-"`, `"f5"`, named punctuation.
/// Returns (chord, explicit_prefix_form).
pub fn parse_binding(s: &str) -> Result<(Chord, bool), String> {
    let mut mods = KeyModifiers::NONE;
    let mut prefixed = false;
    let parts: Vec<&str> = s.split('+').collect();
    let (mod_parts, key_part) = match parts.split_last() {
        Some((last, rest)) => (rest, *last),
        None => return Err(format!("empty binding: {s:?}")),
    };
    // "prefix+-" style: split on '+' may eat a literal '+' key.
    let key_part = if key_part.is_empty() && s.ends_with('+') { "+" } else { key_part };

    for m in mod_parts {
        match m.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => mods |= KeyModifiers::CONTROL,
            "alt" | "option" => mods |= KeyModifiers::ALT,
            "shift" => mods |= KeyModifiers::SHIFT,
            "cmd" | "super" => mods |= KeyModifiers::SUPER,
            "prefix" => prefixed = true,
            other => return Err(format!("unknown modifier {other:?} in {s:?}")),
        }
    }

    let code = match key_part.to_ascii_lowercase().as_str() {
        "space" => KeyCode::Char(' '),
        "minus" | "dash" => KeyCode::Char('-'),
        "plus" => KeyCode::Char('+'),
        "question" => KeyCode::Char('?'),
        "tab" => KeyCode::Tab,
        "backtab" => KeyCode::BackTab,
        "enter" | "return" => KeyCode::Enter,
        "esc" | "escape" => KeyCode::Esc,
        "backspace" => KeyCode::Backspace,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" => KeyCode::PageUp,
        "pagedown" => KeyCode::PageDown,
        "insert" => KeyCode::Insert,
        "delete" | "del" => KeyCode::Delete,
        k if k.len() == 1 => {
            // Preserve case from the original (shift chars like '?').
            KeyCode::Char(key_part.chars().next().expect("len checked"))
        }
        k if k.starts_with('f') && k[1..].parse::<u8>().is_ok() => {
            KeyCode::F(k[1..].parse().expect("checked"))
        }
        other => return Err(format!("unknown key {other:?} in {s:?}")),
    };

    Ok((Chord { code, mods }, prefixed))
}

/// Bare key or `prefix+X` → prefixed binding; modifier chord without
/// `prefix` → direct (fires from terminal mode).
fn is_direct(explicit_prefix: bool, chord: Chord) -> bool {
    !explicit_prefix && !(chord.mods & !KeyModifiers::SHIFT).is_empty()
}

/// Build the runtime keymap: defaults, then user action overrides, then
/// custom commands. Bad entries warn and are skipped, never fatal.
pub fn build_keymap(cfg: &KeysCfg) -> (Keymap, Vec<String>) {
    let mut warnings = Vec::new();

    let prefix = match parse_binding(&cfg.prefix) {
        Ok((chord, _)) => chord,
        Err(e) => {
            warnings.push(format!("bad [keys].prefix: {e}; keeping ctrl+b"));
            Chord { code: KeyCode::Char('b'), mods: KeyModifiers::CONTROL }
        }
    };

    let mut entries: Vec<KeyEntry> = Vec::new();
    for (name, default_keys, action) in default_actions() {
        let mut keys: Vec<String> = default_keys.iter().map(|k| k.to_string()).collect();
        if let Some(v) = cfg.actions.get(name) {
            match binding_strings(v) {
                Some(list) => keys = list,
                None => warnings.push(format!("[keys].{name}: expected string or array")),
            }
        }
        for k in keys {
            match parse_binding(&k) {
                Ok((chord, explicit_prefix)) => entries.push(KeyEntry {
                    chord,
                    direct: is_direct(explicit_prefix, chord),
                    label: k,
                    bound: Bound::Builtin(action),
                }),
                Err(e) => warnings.push(format!("[keys].{name}: {e}; skipped")),
            }
        }
    }

    for cmd in &cfg.commands {
        match parse_binding(&cmd.key) {
            Ok((chord, explicit_prefix)) => entries.push(KeyEntry {
                chord,
                direct: is_direct(explicit_prefix, chord),
                label: cmd.key.clone(),
                bound: Bound::Command(cmd.clone()),
            }),
            Err(e) => warnings.push(format!("[[keys.command]] {}: {e}; skipped", cmd.key)),
        }
    }

    (Keymap { prefix, entries }, warnings)
}

fn binding_strings(v: &toml::Value) -> Option<Vec<String>> {
    match v {
        toml::Value::String(s) => Some(vec![s.clone()]),
        toml::Value::Array(a) => {
            a.iter().map(|x| x.as_str().map(String::from)).collect::<Option<Vec<_>>>()
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_and_modified() {
        let (c, p) = parse_binding("v").unwrap();
        assert_eq!(c.code, KeyCode::Char('v'));
        assert!(!p);
        let (c, p) = parse_binding("prefix+v").unwrap();
        assert_eq!(c.code, KeyCode::Char('v'));
        assert!(p);
        let (c, _) = parse_binding("ctrl+shift+t").unwrap();
        assert_eq!(c.code, KeyCode::Char('t'));
        assert!(c.mods.contains(KeyModifiers::CONTROL | KeyModifiers::SHIFT));
        let (c, _) = parse_binding("f5").unwrap();
        assert_eq!(c.code, KeyCode::F(5));
        assert!(parse_binding("hyper+x").is_err());
    }

    #[test]
    fn direct_vs_prefixed() {
        let (chord, p) = parse_binding("ctrl+alt+t").unwrap();
        assert!(is_direct(p, chord));
        let (chord, p) = parse_binding("prefix+t").unwrap();
        assert!(!is_direct(p, chord));
        let (chord, p) = parse_binding("t").unwrap();
        assert!(!is_direct(p, chord));
    }

    #[test]
    fn override_and_bad_binding_warns() {
        let cfg: crate::config::Config = toml::from_str(
            r#"
[keys]
prefix = "ctrl+a"
zoom = "m"
split_right = "not+a+key"
"#,
        )
        .unwrap();
        let (map, warnings) = build_keymap(&cfg.keys);
        assert_eq!(map.prefix.code, KeyCode::Char('a'));
        assert!(map.lookup_prefixed(KeyCode::Char('m'), KeyModifiers::NONE).is_some());
        assert!(!warnings.is_empty(), "bad split_right must warn");
        // The rest of the table survives one bad binding.
        assert!(map.lookup_prefixed(KeyCode::Char('x'), KeyModifiers::NONE).is_some());
    }
}
