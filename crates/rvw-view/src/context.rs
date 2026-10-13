//! A file as the review screen shows it whole: its bytes on disk as numbered
//! lines, capped at [`MAX_LINES`], with the lines a change touched marked.

use std::collections::BTreeSet;

use crate::diff::{FileDiff, Hunk, Line, LineKind};

/// The most lines of a file that are shown; a longer file is cut to this. The
/// list is virtualised, so the cap bounds memory, not drawing.
pub const MAX_LINES: usize = 20_000;

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

/// Classifies and converts the bytes of a file for display, no line marked.
#[must_use]
pub fn shown(bytes: &[u8]) -> Shown {
    shown_marked(bytes, &BTreeSet::new())
}

/// Like [`shown`], with the lines numbered in `added` marked as added.
#[must_use]
pub fn shown_marked(bytes: &[u8], added: &BTreeSet<u32>) -> Shown {
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
        diff: from_text_marked(&head.join("\n"), added),
        cut,
    }
}

/// A file's text as one hunk numbered from 1, no header. A line whose number
/// is in `added` is an added line (so it is tinted like one) and every other
/// line is context; no line is removed. Empty text has no hunks.
#[must_use]
pub fn from_text_marked(text: &str, added: &BTreeSet<u32>) -> FileDiff {
    let mut marked = 0;
    let lines: Vec<Line> = text
        .lines()
        .enumerate()
        .map(|(index, text)| {
            let no = u32::try_from(index + 1).unwrap_or(u32::MAX);
            let is_added = added.contains(&no);
            marked += usize::from(is_added);
            Line {
                kind: if is_added {
                    LineKind::Added
                } else {
                    LineKind::Context
                },
                old_no: (!is_added).then_some(no),
                new_no: Some(no),
                text: text.to_owned(),
            }
        })
        .collect();
    let hunks = if lines.is_empty() {
        Vec::new()
    } else {
        vec![Hunk {
            header: String::new(),
            lines,
        }]
    };
    FileDiff {
        hunks,
        binary: false,
        added: marked,
        removed: 0,
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

    #[test]
    fn marked_lines_are_added_and_the_rest_context() {
        let added: BTreeSet<u32> = [2, 4, 9].into();
        let Shown::Text { diff, .. } = shown_marked(b"a\nb\nc\nd\n", &added) else {
            panic!("expected text");
        };
        let lines = &diff.hunks[0].lines;
        let kinds: Vec<LineKind> = lines.iter().map(|l| l.kind).collect();
        assert_eq!(
            kinds,
            [
                LineKind::Context,
                LineKind::Added,
                LineKind::Context,
                LineKind::Added
            ]
        );
        assert_eq!(diff.added, 2);
        assert_eq!(diff.removed, 0);
        assert_eq!(lines[1].old_no, None);
        assert_eq!(lines[1].new_no, Some(2));
        assert_eq!(lines[0].old_no, Some(1));
    }

    #[test]
    fn marking_nothing_is_the_plain_file() {
        assert_eq!(shown_marked(b"x\ny\n", &BTreeSet::new()), shown(b"x\ny\n"));
    }
}
