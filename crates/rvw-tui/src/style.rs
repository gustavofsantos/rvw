//! The viewer's colors and styles, and the styled text pieces a row is made of.
//! Port of the palette in internal/tui/view.go. Colors are the terminal's own
//! 16-color palette indices, so the UI follows the user's terminal theme.

use ratatui::style::{Color, Modifier, Style};
use unicode_width::UnicodeWidthStr;

/// Accent: titles, the current item, modes (ANSI bright blue).
pub const COL_ACCENT: Color = Color::Indexed(12);
/// Sign: marks, matches and ids (ANSI bright yellow).
pub const COL_SIGN: Color = Color::Indexed(11);
/// Dim: borders, secondary text (ANSI bright black).
pub const COL_DIM: Color = Color::Indexed(8);

/// Plain text with no color.
#[must_use]
pub fn plain() -> Style {
    Style::new()
}

/// Pane borders.
#[must_use]
pub fn border() -> Style {
    Style::new().fg(COL_DIM)
}

/// Titles.
#[must_use]
pub fn title() -> Style {
    Style::new().fg(COL_ACCENT).add_modifier(Modifier::BOLD)
}

/// Secondary text.
#[must_use]
pub fn dim() -> Style {
    Style::new().fg(COL_DIM)
}

/// Faint text.
#[must_use]
pub fn faint() -> Style {
    Style::new().add_modifier(Modifier::DIM)
}

/// Line numbers.
#[must_use]
pub fn line_nr() -> Style {
    Style::new().fg(COL_DIM)
}

/// The cursor's line number.
#[must_use]
pub fn line_nr_current() -> Style {
    Style::new().add_modifier(Modifier::BOLD)
}

/// A rail mark on a line with a comment that is open.
#[must_use]
pub fn rail_on() -> Style {
    Style::new().fg(COL_SIGN).add_modifier(Modifier::BOLD)
}

/// A rail mark on a line with a comment that is decided.
#[must_use]
pub fn rail_off() -> Style {
    Style::new().fg(Color::Indexed(3))
}

/// A rail mark on a comment elsewhere in the file.
#[must_use]
pub fn rail_away() -> Style {
    Style::new().fg(COL_DIM).add_modifier(Modifier::DIM)
}

/// Directories in the tree.
#[must_use]
pub fn dir() -> Style {
    Style::new().fg(Color::Indexed(4))
}

/// The current item in a list.
#[must_use]
pub fn current() -> Style {
    Style::new().fg(COL_ACCENT).add_modifier(Modifier::BOLD)
}

/// A badge, such as a count.
#[must_use]
pub fn badge() -> Style {
    Style::new().fg(Color::Indexed(3))
}

/// A match in a fuzzy search.
#[must_use]
pub fn matched() -> Style {
    Style::new().fg(COL_SIGN).add_modifier(Modifier::BOLD)
}

/// An id such as `r3` or `rv1`.
#[must_use]
pub fn id() -> Style {
    Style::new().fg(COL_SIGN)
}

/// An error message.
#[must_use]
pub fn error() -> Style {
    Style::new().fg(Color::Indexed(9))
}

/// A notice message.
#[must_use]
pub fn notice() -> Style {
    Style::new().fg(Color::Indexed(10))
}

/// The mode indicator in the bottom bar.
#[must_use]
pub fn mode() -> Style {
    Style::new().fg(COL_ACCENT).add_modifier(Modifier::BOLD)
}

/// A comment or review that is done (ANSI green).
#[must_use]
pub fn done() -> Style {
    Style::new().fg(Color::Indexed(2))
}

/// A comment that was rejected (ANSI red).
#[must_use]
pub fn rejected() -> Style {
    Style::new().fg(Color::Indexed(1))
}

/// A comment that was pulled and not decided yet (ANSI yellow).
#[must_use]
pub fn pulled() -> Style {
    Style::new().fg(Color::Indexed(3))
}

/// The kind of change a line of a file has against the last commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sign {
    /// No change.
    None,
    /// A new line.
    Added,
    /// A line that replaced another.
    Changed,
    /// Lines were deleted below this one.
    Deleted,
    /// Lines were deleted above this one, the first line.
    DeletedTop,
}

impl Sign {
    /// The gutter glyph of this sign.
    #[must_use]
    pub fn glyph(self) -> &'static str {
        match self {
            Self::None => " ",
            Self::Added | Self::Changed => "▎",
            Self::Deleted => "▁",
            Self::DeletedTop => "▔",
        }
    }
}

/// The style of a change sign.
#[must_use]
pub fn sign_style(sign: Sign) -> Style {
    match sign {
        Sign::None => plain(),
        Sign::Added => Style::new().fg(Color::Indexed(2)),
        Sign::Changed => Style::new().fg(Color::Indexed(4)),
        Sign::Deleted | Sign::DeletedTop => Style::new().fg(Color::Indexed(1)),
    }
}

/// The style of a changed file's git status letter. Unknown letters are plain.
#[must_use]
pub fn status_style(letter: u8) -> Style {
    match letter {
        b'A' | b'?' => Style::new().fg(Color::Indexed(2)),
        b'M' | b'T' => Style::new().fg(Color::Indexed(4)),
        b'D' => Style::new().fg(Color::Indexed(1)),
        b'U' => Style::new().fg(Color::Indexed(3)),
        _ => plain(),
    }
}

/// Styled text. A row is a list of pieces, so a background can span them all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Piece {
    pub text: String,
    pub style: Style,
}

/// A piece of `text` in `style`.
#[must_use]
pub fn piece(text: impl Into<String>, style: Style) -> Piece {
    Piece {
        text: text.into(),
        style,
    }
}

/// The display width of a row of pieces, in terminal columns.
#[must_use]
pub fn width(pieces: &[Piece]) -> usize {
    pieces.iter().map(|p| p.text.width()).sum()
}

/// `s` the way the viewer shows source: tabs expanded to tab stops, control
/// characters made visible.
#[must_use]
pub fn expand_tabs(s: &str) -> String {
    crate::highlight::display_text(s, 0).0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn width_sums_display_columns_across_pieces() {
        let row = vec![piece("ab", plain()), piece("中", dim())];
        assert_eq!(width(&row), 4);
        assert_eq!(width(&[]), 0);
    }

    #[test]
    fn expand_tabs_uses_four_column_stops() {
        assert_eq!(expand_tabs("a\tb"), "a   b");
        assert_eq!(expand_tabs("\t\tx"), "        x");
    }

    #[test]
    fn unknown_status_letter_is_plain() {
        assert_eq!(status_style(b'Z'), plain());
        assert_ne!(status_style(b'M'), plain());
    }
}
