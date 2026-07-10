//! alacritty grid → ratatui buffer conversion. With term/emulator.rs, the
//! only place alacritty types are allowed.

use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::term::{Term, TermMode};
use alacritty_terminal::vte::ansi::{Color as AnsiColor, NamedColor, Rgb};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

use crate::term::emulator::EventProxy;

/// Draw a pane's terminal content into `area`. Only the focused pane
/// positions the host cursor.
pub fn render(term: &Term<EventProxy>, area: Rect, frame: &mut Frame, focused: bool) {
    let content = term.renderable_content();
    let offset = content.display_offset as i32;
    let buf = frame.buffer_mut();

    for indexed in content.display_iter {
        let row = indexed.point.line.0 + offset;
        let col = indexed.point.column.0;
        if row < 0 || row >= area.height as i32 || col >= area.width as usize {
            continue;
        }
        let flags = indexed.cell.flags;
        if flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
            continue;
        }

        let x = area.x + col as u16;
        let y = area.y + row as u16;
        let Some(cell) = buf.cell_mut((x, y)) else { continue };

        // ponytail: selection highlight lands with mouse support in M5
        let style = Style::new()
            .fg(convert_color(indexed.cell.fg, content.colors))
            .bg(convert_color(indexed.cell.bg, content.colors))
            .add_modifier(convert_flags(flags));

        let c = indexed.cell.c;
        cell.set_char(if c == '\t' { ' ' } else { c });
        cell.set_style(style);
    }

    // Host cursor only when the viewport is at the live bottom.
    if focused && content.mode.contains(TermMode::SHOW_CURSOR) && offset == 0 {
        let p = content.cursor.point;
        if p.line.0 >= 0 && (p.line.0 as u16) < area.height && (p.column.0 as u16) < area.width {
            frame.set_cursor_position((area.x + p.column.0 as u16, area.y + p.line.0 as u16));
        }
    }
}

fn rgb(c: Rgb) -> Color {
    Color::Rgb(c.r, c.g, c.b)
}

/// OSC-set palette entries win; otherwise map to the host terminal's colors.
fn convert_color(color: AnsiColor, colors: &Colors) -> Color {
    match color {
        AnsiColor::Spec(c) => rgb(c),
        AnsiColor::Indexed(i) => colors[i as usize].map(rgb).unwrap_or(Color::Indexed(i)),
        AnsiColor::Named(n) => colors[n].map(rgb).unwrap_or_else(|| named_fallback(n)),
    }
}

fn named_fallback(n: NamedColor) -> Color {
    match n {
        NamedColor::Black | NamedColor::DimBlack => Color::Black,
        NamedColor::Red | NamedColor::DimRed => Color::Red,
        NamedColor::Green | NamedColor::DimGreen => Color::Green,
        NamedColor::Yellow | NamedColor::DimYellow => Color::Yellow,
        NamedColor::Blue | NamedColor::DimBlue => Color::Blue,
        NamedColor::Magenta | NamedColor::DimMagenta => Color::Magenta,
        NamedColor::Cyan | NamedColor::DimCyan => Color::Cyan,
        NamedColor::White | NamedColor::DimWhite => Color::Gray,
        NamedColor::BrightBlack => Color::DarkGray,
        NamedColor::BrightRed => Color::LightRed,
        NamedColor::BrightGreen => Color::LightGreen,
        NamedColor::BrightYellow => Color::LightYellow,
        NamedColor::BrightBlue => Color::LightBlue,
        NamedColor::BrightMagenta => Color::LightMagenta,
        NamedColor::BrightCyan => Color::LightCyan,
        NamedColor::BrightWhite => Color::White,
        _ => Color::Reset,
    }
}

fn convert_flags(flags: Flags) -> Modifier {
    let mut m = Modifier::empty();
    if flags.contains(Flags::BOLD) {
        m |= Modifier::BOLD;
    }
    if flags.contains(Flags::ITALIC) {
        m |= Modifier::ITALIC;
    }
    if flags.intersects(Flags::UNDERLINE | Flags::DOUBLE_UNDERLINE) {
        m |= Modifier::UNDERLINED;
    }
    if flags.contains(Flags::DIM) {
        m |= Modifier::DIM;
    }
    if flags.contains(Flags::INVERSE) {
        m |= Modifier::REVERSED;
    }
    if flags.contains(Flags::HIDDEN) {
        m |= Modifier::HIDDEN;
    }
    if flags.contains(Flags::STRIKEOUT) {
        m |= Modifier::CROSSED_OUT;
    }
    m
}
