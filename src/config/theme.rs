//! Theme tokens. Built-ins: `default` (accented chrome on host colors) and
//! `terminal` (fully follows the host palette). `[theme.custom]` overrides
//! individual tokens.

use ratatui::style::Color;

use super::ThemeCfg;

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub accent: Color,
    pub divider: Color,
    pub tab_bar_bg: Color,
    pub muted: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            accent: Color::Cyan,
            divider: Color::DarkGray,
            tab_bar_bg: Color::Rgb(20, 20, 30),
            muted: Color::Gray,
        }
    }
}

pub fn resolve(cfg: &ThemeCfg) -> (Theme, Vec<String>) {
    let mut warnings = Vec::new();
    let mut theme = match cfg.name.as_str() {
        "terminal" => Theme { tab_bar_bg: Color::Reset, ..Theme::default() },
        "default" => Theme::default(),
        other => {
            warnings.push(format!("unknown theme {other:?}; using default"));
            Theme::default()
        }
    };
    for (token, value) in &cfg.custom {
        let Some(color) = parse_color(value) else {
            warnings.push(format!("[theme.custom].{token}: bad color {value:?}; skipped"));
            continue;
        };
        match token.as_str() {
            "accent" => theme.accent = color,
            "divider" => theme.divider = color,
            "tab_bar_bg" => theme.tab_bar_bg = color,
            "muted" => theme.muted = color,
            other => warnings.push(format!("[theme.custom].{other}: unknown token; skipped")),
        }
    }
    (theme, warnings)
}

/// `#rrggbb`, `rgb(r,g,b)`, named ANSI colors, `reset`/`transparent`.
pub fn parse_color(s: &str) -> Option<Color> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        if hex.len() == 6 {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            return Some(Color::Rgb(r, g, b));
        }
        return None;
    }
    if let Some(body) = s.strip_prefix("rgb(").and_then(|x| x.strip_suffix(')')) {
        let parts: Vec<_> = body.split(',').map(str::trim).collect();
        if let [r, g, b] = parts[..] {
            return Some(Color::Rgb(r.parse().ok()?, g.parse().ok()?, b.parse().ok()?));
        }
        return None;
    }
    match s.to_ascii_lowercase().as_str() {
        "reset" | "transparent" => Some(Color::Reset),
        "black" => Some(Color::Black),
        "red" => Some(Color::Red),
        "green" => Some(Color::Green),
        "yellow" => Some(Color::Yellow),
        "blue" => Some(Color::Blue),
        "magenta" => Some(Color::Magenta),
        "cyan" => Some(Color::Cyan),
        "white" => Some(Color::White),
        "gray" | "grey" => Some(Color::Gray),
        "darkgray" | "darkgrey" => Some(Color::DarkGray),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_formats() {
        assert_eq!(parse_color("#ff8800"), Some(Color::Rgb(255, 136, 0)));
        assert_eq!(parse_color("rgb(1, 2, 3)"), Some(Color::Rgb(1, 2, 3)));
        assert_eq!(parse_color("cyan"), Some(Color::Cyan));
        assert_eq!(parse_color("reset"), Some(Color::Reset));
        assert_eq!(parse_color("#zzz"), None);
        assert_eq!(parse_color("nope"), None);
    }

    #[test]
    fn custom_overrides_and_warns() {
        let cfg = ThemeCfg {
            name: "default".into(),
            custom: [("accent".to_string(), "#00ff00".to_string()),
                     ("bogus".to_string(), "red".to_string())]
                .into_iter()
                .collect(),
        };
        let (theme, warnings) = resolve(&cfg);
        assert_eq!(theme.accent, Color::Rgb(0, 255, 0));
        assert_eq!(warnings.len(), 1);
    }
}
