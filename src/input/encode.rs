//! Re-serialize parsed crossterm key events into the byte sequences the
//! pane's application expects, honoring that pane's terminal modes.
//!
//! Two encodings: xterm-classic (default) and the kitty keyboard protocol
//! (CSI u), used when the app pushed any kitty progressive-enhancement flag.
//!
//! ponytail: modifyOtherKeys is not gated here — alacritty_terminal 0.26's
//! Term never implements vte's set_modify_other_keys, so the mode is not
//! observable from TermMode; kitty CSI-u covers the disambiguation need.

use alacritty_terminal::term::TermMode;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

pub fn encode_key(key: &KeyEvent, mode: &TermMode) -> Option<Vec<u8>> {
    if mode.intersects(TermMode::KITTY_KEYBOARD_PROTOCOL) {
        encode_kitty(key, mode)
    } else {
        encode_classic(key, mode)
    }
}

// ---------------------------------------------------------------- classic

fn encode_classic(key: &KeyEvent, mode: &TermMode) -> Option<Vec<u8>> {
    let mods = key.modifiers;
    let ctrl = mods.contains(KeyModifiers::CONTROL);
    let alt = mods.contains(KeyModifiers::ALT);
    let shift = mods.contains(KeyModifiers::SHIFT);
    let app_cursor = mode.contains(TermMode::APP_CURSOR);

    // xterm modifier parameter: 1 + shift(1) + alt(2) + ctrl(4)
    let modifier_param = 1 + (shift as u8) + ((alt as u8) << 1) + ((ctrl as u8) << 2);
    let has_mods = modifier_param > 1;

    let mut out: Vec<u8> = Vec::with_capacity(8);

    match key.code {
        KeyCode::Char(c) => {
            if alt {
                out.push(0x1b);
            }
            if ctrl {
                match ctrl_byte(c) {
                    Some(b) => out.push(b),
                    None => out.extend(c.to_string().as_bytes()),
                }
            } else {
                out.extend(c.to_string().as_bytes());
            }
        }
        KeyCode::Enter => {
            if alt {
                out.push(0x1b);
            }
            out.push(b'\r');
        }
        KeyCode::Backspace => {
            if alt {
                out.push(0x1b);
            }
            out.push(if ctrl { 0x08 } else { 0x7f });
        }
        KeyCode::Tab => out.push(b'\t'),
        KeyCode::BackTab => out.extend(b"\x1b[Z"),
        KeyCode::Esc => out.push(0x1b),
        KeyCode::Up => arrow(&mut out, b'A', app_cursor, has_mods, modifier_param),
        KeyCode::Down => arrow(&mut out, b'B', app_cursor, has_mods, modifier_param),
        KeyCode::Right => arrow(&mut out, b'C', app_cursor, has_mods, modifier_param),
        KeyCode::Left => arrow(&mut out, b'D', app_cursor, has_mods, modifier_param),
        KeyCode::Home => arrow(&mut out, b'H', app_cursor, has_mods, modifier_param),
        KeyCode::End => arrow(&mut out, b'F', app_cursor, has_mods, modifier_param),
        KeyCode::Insert => tilde(&mut out, 2, has_mods, modifier_param),
        KeyCode::Delete => tilde(&mut out, 3, has_mods, modifier_param),
        KeyCode::PageUp => tilde(&mut out, 5, has_mods, modifier_param),
        KeyCode::PageDown => tilde(&mut out, 6, has_mods, modifier_param),
        KeyCode::F(n) => f_key(&mut out, n, has_mods, modifier_param)?,
        _ => return None,
    }
    Some(out)
}

/// Ctrl+letter → C0 control byte.
fn ctrl_byte(c: char) -> Option<u8> {
    match c {
        'a'..='z' => Some(c as u8 - b'a' + 1),
        'A'..='Z' => Some(c.to_ascii_lowercase() as u8 - b'a' + 1),
        ' ' | '@' | '2' => Some(0x00),
        '[' | '3' => Some(0x1b),
        '\\' | '4' => Some(0x1c),
        ']' | '5' => Some(0x1d),
        '^' | '6' => Some(0x1e),
        '_' | '/' | '7' => Some(0x1f),
        '?' | '8' => Some(0x7f),
        _ => None,
    }
}

/// Arrows + Home/End: `ESC O x` (app cursor), `ESC [ x`, or `ESC [ 1 ; m x`.
fn arrow(out: &mut Vec<u8>, letter: u8, app_cursor: bool, has_mods: bool, m: u8) {
    if has_mods {
        out.extend(format!("\x1b[1;{m}").as_bytes());
    } else if app_cursor {
        out.extend(b"\x1bO");
    } else {
        out.extend(b"\x1b[");
    }
    out.push(letter);
}

/// `ESC [ n ~` family, with `ESC [ n ; m ~` when modified.
fn tilde(out: &mut Vec<u8>, n: u8, has_mods: bool, m: u8) {
    if has_mods {
        out.extend(format!("\x1b[{n};{m}~").as_bytes());
    } else {
        out.extend(format!("\x1b[{n}~").as_bytes());
    }
}

fn f_key(out: &mut Vec<u8>, n: u8, has_mods: bool, m: u8) -> Option<()> {
    match n {
        1..=4 => {
            let letter = b'P' + (n - 1);
            if has_mods {
                out.extend(format!("\x1b[1;{m}").as_bytes());
                out.push(letter);
            } else {
                out.extend(b"\x1bO");
                out.push(letter);
            }
        }
        5..=12 => {
            let code = [15u8, 17, 18, 19, 20, 21, 23, 24][(n - 5) as usize];
            tilde(out, code, has_mods, m);
        }
        _ => return None,
    }
    Some(())
}

// ------------------------------------------------------------------ kitty
// https://sw.kovidgoyal.net/kitty/keyboard-protocol/
//
// While the protocol is active, DECCKM (APP_CURSOR) is ignored per spec.

/// Kitty modifier value: 1 + shift(1) alt(2) ctrl(4) super(8) hyper(16) meta(32).
fn kitty_mods(m: KeyModifiers) -> u8 {
    1 + m.contains(KeyModifiers::SHIFT) as u8
        + ((m.contains(KeyModifiers::ALT) as u8) << 1)
        + ((m.contains(KeyModifiers::CONTROL) as u8) << 2)
        + ((m.contains(KeyModifiers::SUPER) as u8) << 3)
        + ((m.contains(KeyModifiers::HYPER) as u8) << 4)
        + ((m.contains(KeyModifiers::META) as u8) << 5)
}

fn encode_kitty(key: &KeyEvent, mode: &TermMode) -> Option<Vec<u8>> {
    let report_all = mode.contains(TermMode::REPORT_ALL_KEYS_AS_ESC);
    let report_events = mode.contains(TermMode::REPORT_EVENT_TYPES);
    let release = key.kind == KeyEventKind::Release;
    if release && !report_events {
        return None; // app did not ask for release events
    }

    // BackTab is Shift+Tab in kitty terms.
    let (code, modifiers) = match key.code {
        KeyCode::BackTab => (KeyCode::Tab, key.modifiers | KeyModifiers::SHIFT),
        c => (c, key.modifiers),
    };

    let mods = kitty_mods(modifiers);
    let event = match key.kind {
        KeyEventKind::Repeat => 2u8,
        KeyEventKind::Release => 3,
        _ => 1,
    };
    // "modifiers[:event-type]" — empty when both are at their defaults.
    let mod_field = if report_events && event > 1 {
        format!("{mods}:{event}")
    } else if mods > 1 {
        mods.to_string()
    } else {
        String::new()
    };

    let csi_u = |code_field: &str| -> Vec<u8> {
        if mod_field.is_empty() {
            format!("\x1b[{code_field}u").into_bytes()
        } else {
            format!("\x1b[{code_field};{mod_field}u").into_bytes()
        }
    };
    // Arrows/Home/End keep their letter-terminated CSI form.
    let letter = |l: char| -> Vec<u8> {
        if mod_field.is_empty() {
            format!("\x1b[{l}").into_bytes()
        } else {
            format!("\x1b[1;{mod_field}{l}").into_bytes()
        }
    };
    let tilde = |n: u8| -> Vec<u8> {
        if mod_field.is_empty() {
            format!("\x1b[{n}~").into_bytes()
        } else {
            format!("\x1b[{n};{mod_field}~").into_bytes()
        }
    };

    let out = match code {
        KeyCode::Char(c) => {
            let text_mods_only = modifiers.difference(KeyModifiers::SHIFT).is_empty();
            // Unmodified (or shift-only) printables stay plain text unless
            // the app asked for every key as an escape code.
            if !report_all && text_mods_only && !release {
                return Some(c.to_string().into_bytes());
            }
            // Key code is the unshifted codepoint; shift lives in modifiers.
            let base = c.to_lowercase().next().unwrap_or(c);
            let code_field = if mode.contains(TermMode::REPORT_ALTERNATE_KEYS) && c != base {
                format!("{}:{}", base as u32, c as u32)
            } else {
                (base as u32).to_string()
            };
            if mode.contains(TermMode::REPORT_ASSOCIATED_TEXT) && text_mods_only && !release {
                // Text field forces an explicit (possibly default) mod field.
                let m = if mod_field.is_empty() { "1" } else { mod_field.as_str() };
                return Some(format!("\x1b[{code_field};{m};{}u", c as u32).into_bytes());
            }
            csi_u(&code_field)
        }
        // Enter/Tab/Backspace keep legacy bytes while unmodified presses.
        KeyCode::Enter if !report_all && mod_field.is_empty() => b"\r".to_vec(),
        KeyCode::Tab if !report_all && mod_field.is_empty() => b"\t".to_vec(),
        KeyCode::Backspace if !report_all && mod_field.is_empty() => vec![0x7f],
        KeyCode::Enter => csi_u("13"),
        KeyCode::Tab => csi_u("9"),
        KeyCode::Backspace => csi_u("127"),
        // Esc is always escape-coded: the whole point of disambiguation.
        KeyCode::Esc => csi_u("27"),
        KeyCode::Up => letter('A'),
        KeyCode::Down => letter('B'),
        KeyCode::Right => letter('C'),
        KeyCode::Left => letter('D'),
        KeyCode::Home => letter('H'),
        KeyCode::End => letter('F'),
        KeyCode::Insert => tilde(2),
        KeyCode::Delete => tilde(3),
        KeyCode::PageUp => tilde(5),
        KeyCode::PageDown => tilde(6),
        KeyCode::F(n @ 1..=4) => {
            let l = (b'P' + n - 1) as char;
            if !mod_field.is_empty() {
                format!("\x1b[1;{mod_field}{l}").into_bytes()
            } else if report_all {
                format!("\x1b[{l}").into_bytes()
            } else {
                format!("\x1bO{l}").into_bytes()
            }
        }
        KeyCode::F(n @ 5..=12) => tilde([15u8, 17, 18, 19, 20, 21, 23, 24][(n - 5) as usize]),
        _ => return None,
    };
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        let mut k = KeyEvent::new(code, mods);
        k.kind = KeyEventKind::Press;
        k
    }

    const DISAMBIG: TermMode = TermMode::DISAMBIGUATE_ESC_CODES;

    #[test]
    fn plain_char() {
        let got = encode_key(&key(KeyCode::Char('a'), KeyModifiers::NONE), &TermMode::empty());
        assert_eq!(got, Some(b"a".to_vec()));
    }

    #[test]
    fn ctrl_c() {
        let got = encode_key(&key(KeyCode::Char('c'), KeyModifiers::CONTROL), &TermMode::empty());
        assert_eq!(got, Some(vec![0x03]));
    }

    #[test]
    fn alt_char_prefixes_esc() {
        let got = encode_key(&key(KeyCode::Char('f'), KeyModifiers::ALT), &TermMode::empty());
        assert_eq!(got, Some(b"\x1bf".to_vec()));
    }

    #[test]
    fn arrow_modes() {
        let up = key(KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(encode_key(&up, &TermMode::empty()), Some(b"\x1b[A".to_vec()));
        assert_eq!(encode_key(&up, &TermMode::APP_CURSOR), Some(b"\x1bOA".to_vec()));
        let ctrl_up = key(KeyCode::Up, KeyModifiers::CONTROL);
        assert_eq!(encode_key(&ctrl_up, &TermMode::empty()), Some(b"\x1b[1;5A".to_vec()));
    }

    #[test]
    fn utf8_char() {
        let got = encode_key(&key(KeyCode::Char('ї'), KeyModifiers::NONE), &TermMode::empty());
        assert_eq!(got, Some("ї".as_bytes().to_vec()));
    }

    // ------------------------------------------------------- kitty mode

    #[test]
    fn kitty_ctrl_char_vs_classic() {
        let k = key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        // Classic: C0 byte. Kitty: CSI codepoint ; mods u.
        assert_eq!(encode_key(&k, &TermMode::empty()), Some(vec![0x03]));
        assert_eq!(encode_key(&k, &DISAMBIG), Some(b"\x1b[99;5u".to_vec()));
    }

    #[test]
    fn kitty_plain_char_stays_text_unless_report_all() {
        let k = key(KeyCode::Char('a'), KeyModifiers::NONE);
        assert_eq!(encode_key(&k, &DISAMBIG), Some(b"a".to_vec()));
        assert_eq!(
            encode_key(&k, &TermMode::REPORT_ALL_KEYS_AS_ESC),
            Some(b"\x1b[97u".to_vec())
        );
    }

    #[test]
    fn kitty_esc_disambiguated() {
        let k = key(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(encode_key(&k, &TermMode::empty()), Some(vec![0x1b]));
        assert_eq!(encode_key(&k, &DISAMBIG), Some(b"\x1b[27u".to_vec()));
    }

    #[test]
    fn kitty_ctrl_i_distinct_from_tab() {
        let ctrl_i = key(KeyCode::Char('i'), KeyModifiers::CONTROL);
        let tab = key(KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(encode_key(&ctrl_i, &DISAMBIG), Some(b"\x1b[105;5u".to_vec()));
        assert_eq!(encode_key(&tab, &DISAMBIG), Some(b"\t".to_vec()));
    }

    #[test]
    fn kitty_arrow_ignores_app_cursor() {
        let up = key(KeyCode::Up, KeyModifiers::NONE);
        let mode = DISAMBIG.union(TermMode::APP_CURSOR);
        assert_eq!(encode_key(&up, &mode), Some(b"\x1b[A".to_vec()));
        let shift_up = key(KeyCode::Up, KeyModifiers::SHIFT);
        assert_eq!(encode_key(&shift_up, &DISAMBIG), Some(b"\x1b[1;2A".to_vec()));
    }

    #[test]
    fn kitty_modified_enter_tab_backspace() {
        let m = KeyModifiers::SHIFT;
        assert_eq!(encode_key(&key(KeyCode::Enter, m), &DISAMBIG), Some(b"\x1b[13;2u".to_vec()));
        assert_eq!(encode_key(&key(KeyCode::Tab, m), &DISAMBIG), Some(b"\x1b[9;2u".to_vec()));
        assert_eq!(
            encode_key(&key(KeyCode::Backspace, KeyModifiers::CONTROL), &DISAMBIG),
            Some(b"\x1b[127;5u".to_vec())
        );
        // Unmodified keep legacy bytes.
        assert_eq!(
            encode_key(&key(KeyCode::Enter, KeyModifiers::NONE), &DISAMBIG),
            Some(b"\r".to_vec())
        );
    }

    #[test]
    fn kitty_backtab_is_shift_tab() {
        let k = key(KeyCode::BackTab, KeyModifiers::NONE);
        assert_eq!(encode_key(&k, &DISAMBIG), Some(b"\x1b[9;2u".to_vec()));
    }

    #[test]
    fn kitty_shifted_char_unshifted_code() {
        // Ctrl+Shift+A: code is unshifted 'a' (97), mods carry shift+ctrl.
        let k = key(KeyCode::Char('A'), KeyModifiers::CONTROL | KeyModifiers::SHIFT);
        assert_eq!(encode_key(&k, &DISAMBIG), Some(b"\x1b[97;6u".to_vec()));
        // With alternate-keys reporting, the shifted key rides along.
        let mode = DISAMBIG.union(TermMode::REPORT_ALTERNATE_KEYS);
        assert_eq!(encode_key(&k, &mode), Some(b"\x1b[97:65;6u".to_vec()));
    }

    #[test]
    fn kitty_associated_text() {
        let mode = TermMode::REPORT_ALL_KEYS_AS_ESC.union(TermMode::REPORT_ASSOCIATED_TEXT);
        let k = key(KeyCode::Char('a'), KeyModifiers::NONE);
        assert_eq!(encode_key(&k, &mode), Some(b"\x1b[97;1;97u".to_vec()));
    }

    #[test]
    fn kitty_event_types() {
        let mode = DISAMBIG.union(TermMode::REPORT_EVENT_TYPES);
        let mut rel = key(KeyCode::Esc, KeyModifiers::NONE);
        rel.kind = KeyEventKind::Release;
        assert_eq!(encode_key(&rel, &mode), Some(b"\x1b[27;1:3u".to_vec()));
        // Releases are dropped when the app did not ask for event types.
        assert_eq!(encode_key(&rel, &DISAMBIG), None);
    }

    #[test]
    fn kitty_f_keys() {
        assert_eq!(
            encode_key(&key(KeyCode::F(1), KeyModifiers::NONE), &DISAMBIG),
            Some(b"\x1bOP".to_vec())
        );
        assert_eq!(
            encode_key(&key(KeyCode::F(1), KeyModifiers::CONTROL), &DISAMBIG),
            Some(b"\x1b[1;5P".to_vec())
        );
        assert_eq!(
            encode_key(&key(KeyCode::F(5), KeyModifiers::NONE), &DISAMBIG),
            Some(b"\x1b[15~".to_vec())
        );
    }
}
