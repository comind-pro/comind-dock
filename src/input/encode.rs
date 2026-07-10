//! Re-serialize parsed crossterm key events into the byte sequences the
//! pane's application expects, honoring that pane's terminal modes.
//!
//! ponytail: xterm-classic encoding only for M1; kitty keyboard protocol and
//! modifyOtherKeys fidelity land in M4 (the fidelity-critical milestone).

use alacritty_terminal::term::TermMode;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub fn encode_key(key: &KeyEvent, mode: &TermMode) -> Option<Vec<u8>> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEventKind;

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        let mut k = KeyEvent::new(code, mods);
        k.kind = KeyEventKind::Press;
        k
    }

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
}
