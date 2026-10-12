//! A file outside the change, as the review screen shows it: its bytes on
//! disk as plain numbered lines, capped at [`MAX_LINES`].

use crate::diff::{self, FileDiff};

/// The most lines of a file that are shown; a longer file is cut to this.
pub const MAX_LINES: usize = 2000;

/// What a file's bytes show as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shown {
    /// The text as one context hunk. `cut` is true when the file had more
    /// than [`MAX_LINES`] lines and only the first ones are in `diff`.
    Text { diff: FileDiff, cut: bool },
    /// The bytes hold a NUL or are not UTF-8.
    Binary,
    /// The file has no bytes at all.
    Empty,
}

/// Classifies and converts the bytes of a file for display.
#[must_use]
pub fn shown(bytes: &[u8]) -> Shown {
    if bytes.is_empty() {
        return Shown::Empty;
    }
    if bytes.contains(&0) {
        return Shown::Binary;
    }
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Shown::Binary;
    };
    let mut lines = text.lines();
    let head: Vec<&str> = lines.by_ref().take(MAX_LINES).collect();
    let cut = lines.next().is_some();
    Shown::Text {
        diff: diff::from_text(&head.join("\n")),
        cut,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(bytes: &[u8]) -> (FileDiff, bool) {
        match shown(bytes) {
            Shown::Text { diff, cut } => (diff, cut),
            other => panic!("expected text, got {other:?}"),
        }
    }

    #[test]
    fn empty_bytes_are_empty() {
        assert_eq!(shown(b""), Shown::Empty);
    }

    #[test]
    fn a_nul_byte_is_binary() {
        assert_eq!(shown(b"PK\x00\x03"), Shown::Binary);
    }

    #[test]
    fn invalid_utf8_is_binary() {
        assert_eq!(shown(&[0xff, 0xfe, b'a']), Shown::Binary);
    }

    #[test]
    fn a_short_file_is_shown_whole_and_not_cut() {
        let (diff, cut) = text_of(b"one\ntwo\n");
        assert!(!cut);
        assert_eq!(diff.hunks[0].lines.len(), 2);
        assert_eq!(diff.hunks[0].lines[1].text, "two");
    }

    #[test]
    fn a_file_longer_than_the_cap_is_cut_to_its_first_lines() {
        let text: String = (1..=MAX_LINES + 5)
            .map(|n| format!("line {n}\n"))
            .collect::<Vec<_>>()
            .concat();
        let (diff, cut) = text_of(text.as_bytes());
        assert!(cut);
        let lines = &diff.hunks[0].lines;
        assert_eq!(lines.len(), MAX_LINES);
        assert_eq!(lines[MAX_LINES - 1].text, format!("line {MAX_LINES}"));
    }

    #[test]
    fn a_file_of_exactly_the_cap_is_not_cut() {
        let text: String = (1..=MAX_LINES)
            .map(|n| format!("{n}\n"))
            .collect::<Vec<_>>()
            .concat();
        let (_, cut) = text_of(text.as_bytes());
        assert!(!cut);
    }
}
