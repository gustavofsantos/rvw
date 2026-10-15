//! Text positions: pixel and column to (line, byte), character selection over lines.
//!
//! Columns are measured in monospace cells from the line's left edge. Tabs
//! advance to the next multiple of the tab width, wide characters take two
//! cells and combining marks take none. Byte offsets always fall on char
//! boundaries. Nothing here knows about the GUI.

use std::ops::Range;

/// A position in a file: a zero-based line and a byte offset into that line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TextPos {
    /// Zero-based line index.
    pub line: usize,
    /// Byte offset in the line, on a char boundary.
    pub col: usize,
}

/// The byte offset of the char boundary nearest to `column` cells from the
/// left edge of `line_text`. A column between two boundaries that are equally
/// near resolves to the earlier one. A column past the end clamps to the line
/// length. A tab width of zero behaves as one.
pub fn from_column(line_text: &str, column: f32, tab_width: usize) -> usize {
    let column = f64::from(column);
    let mut best = 0;
    let mut best_distance = f64::INFINITY;
    let mut cell = 0;
    let mut consider = |byte: usize, at: usize| {
        let distance = (cells_f64(at) - column).abs();
        if distance < best_distance {
            best = byte;
            best_distance = distance;
        }
    };
    for (byte, ch) in line_text.char_indices() {
        consider(byte, cell);
        cell = advance(cell, ch, tab_width);
    }
    consider(line_text.len(), cell);
    best
}

/// The cell column of the byte offset `col` in `line_text`, the inverse of
/// [`from_column`]. An offset past the end gives the width of the whole line.
pub fn column_of(line_text: &str, col: usize, tab_width: usize) -> usize {
    let mut cell = 0;
    for (byte, ch) in line_text.char_indices() {
        if byte >= col {
            break;
        }
        cell = advance(cell, ch, tab_width);
    }
    cell
}

/// A selection between two positions. The anchor is where it started and the
/// head is where it currently ends; the head may come before the anchor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextSelection {
    anchor: TextPos,
    head: TextPos,
}

impl TextSelection {
    /// A selection from `anchor` to `head`.
    pub fn new(anchor: TextPos, head: TextPos) -> Self {
        Self { anchor, head }
    }

    /// True when the anchor and the head are the same position.
    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// The start and the end of the selection, in document order.
    pub fn ordered(&self) -> (TextPos, TextPos) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    /// The selected byte range of `line`, given that line's text. The first
    /// line of a selection runs to its end, interior lines are whole, and the
    /// last line runs up to the end position. `None` when the selection is
    /// empty or does not reach `line`. A line the selection spans but selects
    /// no bytes of (an empty line in the middle) gives an empty range.
    pub fn contains_line(&self, line: usize, line_text: &str) -> Option<Range<usize>> {
        if self.is_empty() {
            return None;
        }
        let (start, end) = self.ordered();
        if !(start.line..=end.line).contains(&line) {
            return None;
        }
        let from = if line == start.line {
            clamp_col(line_text, start.col)
        } else {
            0
        };
        let to = if line == end.line {
            clamp_col(line_text, end.col)
        } else {
            line_text.len()
        };
        Some(from..to.max(from))
    }

    /// The selected text, one `\n` between lines. Lines past the end of
    /// `lines` are left out.
    pub fn text(&self, lines: &[&str]) -> String {
        if self.is_empty() {
            return String::new();
        }
        let (start, end) = self.ordered();
        let mut out = String::new();
        for line in start.line..=end.line {
            let Some(text) = lines.get(line) else {
                break;
            };
            if line > start.line {
                out.push('\n');
            }
            if let Some(range) = self.contains_line(line, text) {
                out.push_str(text.get(range).unwrap_or_default());
            }
        }
        out
    }
}

/// The byte range of the word under `col` in `line_text`, for a double click.
/// A word is a run of alphanumeric characters or `_`. A run of spaces and a
/// run of other punctuation select as runs too. An offset at the end of the
/// line selects the run before it. An empty line gives `0..0`.
pub fn word_at(line_text: &str, col: usize) -> Range<usize> {
    let mut pos = col.min(line_text.len());
    while !line_text.is_char_boundary(pos) {
        pos -= 1;
    }
    if pos == line_text.len() {
        match line_text.char_indices().next_back() {
            Some((last, _)) => pos = last,
            None => return 0..0,
        }
    }
    let Some(ch) = line_text[pos..].chars().next() else {
        return 0..0;
    };
    let class = CharClass::of(ch);
    let start = line_text[..pos]
        .char_indices()
        .rev()
        .take_while(|&(_, c)| CharClass::of(c) == class)
        .last()
        .map_or(pos, |(i, _)| i);
    let end = line_text[pos..]
        .char_indices()
        .find(|&(_, c)| CharClass::of(c) != class)
        .map_or(line_text.len(), |(i, _)| pos + i);
    start..end
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CharClass {
    Word,
    Space,
    Punctuation,
}

impl CharClass {
    fn of(ch: char) -> Self {
        if ch.is_alphanumeric() || ch == '_' {
            Self::Word
        } else if ch.is_whitespace() {
            Self::Space
        } else {
            Self::Punctuation
        }
    }
}

/// The cell column after `ch`, when `ch` starts at cell `cell`.
fn advance(cell: usize, ch: char, tab_width: usize) -> usize {
    if ch == '\t' {
        let width = tab_width.max(1);
        cell + width - cell % width
    } else {
        cell + char_width(ch)
    }
}

/// Cells taken by one character: 2 for East Asian wide and emoji, 0 for
/// combining marks and zero-width characters, 1 otherwise. A small table
/// covering the common ranges, not the full Unicode width data.
fn char_width(ch: char) -> usize {
    let cp = u32::from(ch);
    if matches!(cp,
        0x0300..=0x036F
        | 0x0483..=0x0489
        | 0x0591..=0x05BD
        | 0x0610..=0x061A
        | 0x064B..=0x065F
        | 0x1AB0..=0x1AFF
        | 0x1DC0..=0x1DFF
        | 0x200B..=0x200F
        | 0x20D0..=0x20FF
        | 0xFE00..=0xFE0F
        | 0xFE20..=0xFE2F)
    {
        return 0;
    }
    if matches!(cp,
        0x1100..=0x115F
        | 0x2E80..=0x303E
        | 0x3041..=0x33FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xA000..=0xA4CF
        | 0xAC00..=0xD7A3
        | 0xF900..=0xFAFF
        | 0xFE30..=0xFE4F
        | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6
        | 0x1F300..=0x1F64F
        | 0x1F680..=0x1F6FF
        | 0x1F900..=0x1F9FF
        | 0x20000..=0x3FFFD)
    {
        return 2;
    }
    1
}

/// A cell count as a float. Cell counts past `u32::MAX` are not real lines.
fn cells_f64(cells: usize) -> f64 {
    f64::from(u32::try_from(cells).unwrap_or(u32::MAX))
}

/// `col` limited to the line length and rounded down to a char boundary.
fn clamp_col(line_text: &str, col: usize) -> usize {
    let mut col = col.min(line_text.len());
    while !line_text.is_char_boundary(col) {
        col -= 1;
    }
    col
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(line: usize, col: usize) -> TextPos {
        TextPos { line, col }
    }

    #[test]
    fn tabs_advance_to_the_next_tab_stop() {
        // "\tx": the tab spans cells 0..4, so x starts at cell 4.
        assert_eq!(column_of("\tx", 1, 4), 4);
        assert_eq!(from_column("\tx", 4.0, 4), 1);
        assert_eq!(from_column("\tx", 1.0, 4), 0);
        assert_eq!(from_column("\tx", 3.0, 4), 1);
        // "a\tb": the tab starts at cell 1 and ends at cell 4.
        assert_eq!(column_of("a\tb", 1, 4), 1);
        assert_eq!(column_of("a\tb", 2, 4), 4);
        assert_eq!(column_of("a\tb", 3, 4), 5);
        assert_eq!(from_column("a\tb", 3.0, 4), 2);
        // A tab at a stop takes a full tab width.
        assert_eq!(column_of("abcd\te", 5, 4), 8);
    }

    #[test]
    fn zero_tab_width_behaves_as_one() {
        assert_eq!(column_of("\t\tx", 2, 0), 2);
        assert_eq!(column_of("\t\tx", 2, 1), 2);
    }

    #[test]
    fn wide_characters_take_two_cells() {
        // "漢a": 漢 is 3 bytes and 2 cells, so a starts at cell 2.
        assert_eq!(column_of("漢a", 3, 4), 2);
        assert_eq!(column_of("漢a", 4, 4), 3);
        assert_eq!(from_column("漢a", 1.2, 4), 3);
        assert_eq!(from_column("漢a", 0.8, 4), 0);
        // Emoji are wide too.
        assert_eq!(column_of("😀x", 4, 4), 2);
        assert_eq!(from_column("😀x", 2.0, 4), 4);
    }

    #[test]
    fn combining_marks_take_no_cells() {
        // "e" + combining acute + "x": the mark sits on e's cell.
        let line = "e\u{301}x";
        assert_eq!(column_of(line, 1, 4), 1);
        assert_eq!(column_of(line, 3, 4), 1);
        assert_eq!(column_of(line, 4, 4), 2);
        assert_eq!(from_column(line, 2.0, 4), 4);
    }

    #[test]
    fn past_the_end_clamps_to_the_line_length() {
        assert_eq!(from_column("abc", 10.0, 4), 3);
        assert_eq!(from_column("abc", -5.0, 4), 0);
        assert_eq!(from_column("", 3.0, 4), 0);
        assert_eq!(column_of("abc", 99, 4), 3);
        assert_eq!(column_of("", 0, 4), 0);
    }

    #[test]
    fn column_round_trips_on_char_boundaries() {
        for line in ["ab", "a\tb漢c", "\t\t漢😀\tz", ""] {
            for byte in (0..=line.len()).filter(|&b| line.is_char_boundary(b)) {
                let column = column_of(line, byte, 4);
                let back = from_column(line, f32::from(u16::try_from(column).unwrap()), 4);
                assert_eq!(back, byte, "line {line:?} byte {byte}");
            }
        }
    }

    #[test]
    fn selection_on_one_line() {
        let sel = TextSelection::new(pos(0, 1), pos(0, 3));
        assert_eq!(sel.contains_line(0, "abcdef"), Some(1..3));
        assert_eq!(sel.contains_line(1, "abcdef"), None);
        assert!(!sel.is_empty());
    }

    #[test]
    fn selection_across_two_lines() {
        let lines = ["abc", "def"];
        let sel = TextSelection::new(pos(0, 2), pos(1, 1));
        assert_eq!(sel.contains_line(0, lines[0]), Some(2..3));
        assert_eq!(sel.contains_line(1, lines[1]), Some(0..1));
        assert_eq!(sel.contains_line(2, "ghi"), None);
    }

    #[test]
    fn selection_across_three_lines_and_reversed() {
        let lines = ["abc", "def", "ghi"];
        let forward = TextSelection::new(pos(0, 2), pos(2, 1));
        let reversed = TextSelection::new(pos(2, 1), pos(0, 2));
        for sel in [forward, reversed] {
            assert_eq!(sel.ordered(), (pos(0, 2), pos(2, 1)));
            assert_eq!(sel.contains_line(0, lines[0]), Some(2..3));
            assert_eq!(sel.contains_line(1, lines[1]), Some(0..3));
            assert_eq!(sel.contains_line(2, lines[2]), Some(0..1));
        }
    }

    #[test]
    fn selection_over_an_empty_middle_line_keeps_the_line() {
        let lines = ["abc", "", "def"];
        let sel = TextSelection::new(pos(0, 0), pos(2, 0));
        assert_eq!(sel.contains_line(1, lines[1]), Some(0..0));
    }

    #[test]
    fn selection_ends_are_clamped_to_the_line() {
        let sel = TextSelection::new(pos(0, 0), pos(1, 99));
        assert_eq!(sel.contains_line(0, "ab"), Some(0..2));
        assert_eq!(sel.contains_line(1, "cd"), Some(0..2));
    }

    #[test]
    fn empty_selection_selects_nothing() {
        let sel = TextSelection::new(pos(1, 2), pos(1, 2));
        assert!(sel.is_empty());
        assert_eq!(sel.contains_line(1, "abcdef"), None);
        assert_eq!(sel.text(&["abc", "def"]), "");
    }

    #[test]
    fn text_joins_lines_with_newlines() {
        let lines = ["abc", "def", "ghi"];
        assert_eq!(TextSelection::new(pos(0, 1), pos(0, 2)).text(&lines), "b");
        assert_eq!(
            TextSelection::new(pos(0, 2), pos(2, 1)).text(&lines),
            "c\ndef\ng"
        );
        assert_eq!(
            TextSelection::new(pos(2, 1), pos(0, 2)).text(&lines),
            "c\ndef\ng"
        );
        assert_eq!(
            TextSelection::new(pos(0, 0), pos(2, 3)).text(&lines),
            "abc\ndef\nghi"
        );
    }

    #[test]
    fn text_keeps_empty_lines_and_skips_missing_ones() {
        let lines = ["abc", "", "def"];
        assert_eq!(
            TextSelection::new(pos(0, 0), pos(2, 1)).text(&lines),
            "abc\n\nd"
        );
        let short = ["abc", "def"];
        assert_eq!(
            TextSelection::new(pos(0, 0), pos(5, 0)).text(&short),
            "abc\ndef"
        );
    }

    #[test]
    fn text_respects_char_boundaries() {
        let lines = ["漢字"];
        assert_eq!(TextSelection::new(pos(0, 3), pos(0, 6)).text(&lines), "字");
    }

    #[test]
    fn word_on_a_word() {
        assert_eq!(word_at("foo bar_1 baz", 1), 0..3);
        assert_eq!(word_at("foo bar_1 baz", 5), 4..9);
        assert_eq!(word_at("foo bar_1 baz", 0), 0..3);
    }

    #[test]
    fn word_on_a_space_selects_the_run_of_spaces() {
        assert_eq!(word_at("foo   bar", 4), 3..6);
        assert_eq!(word_at("foo bar", 3), 3..4);
    }

    #[test]
    fn word_on_punctuation_selects_the_run_of_punctuation() {
        assert_eq!(word_at("a.(b)", 1), 1..3);
        assert_eq!(word_at("a.(b)", 3), 3..4);
        assert_eq!(word_at("foo--->bar", 4), 3..7);
    }

    #[test]
    fn word_at_line_end_selects_the_last_run() {
        assert_eq!(word_at("foo bar", 7), 4..7);
        assert_eq!(word_at("foo.", 4), 3..4);
        assert_eq!(word_at("foo", 99), 0..3);
    }

    #[test]
    fn word_on_an_empty_line_is_empty() {
        assert_eq!(word_at("", 0), 0..0);
        assert_eq!(word_at("", 5), 0..0);
    }

    #[test]
    fn word_handles_multibyte_characters() {
        // "é" is 2 bytes and alphanumeric, so the run is the whole word.
        assert_eq!(word_at("café x", 4), 0..5);
        assert_eq!(word_at("café x", 3), 0..5);
    }
}
