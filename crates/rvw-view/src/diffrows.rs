//! The diff of one file as a flat list of rows, in the order they are drawn:
//! hunk headers, lines, the comment threads under their line, the composer,
//! and the strip of comments that have no line in view. A virtualised list
//! draws only the rows in view, so a long diff costs what the screen shows.

use std::collections::BTreeMap;

use super::diff::FileDiff;
use super::selection::Pos;

/// Rows past this many are not listed; a pathological file must not freeze
/// the window.
pub const MAX_ROWS: usize = 20_000;

/// The text of the last row of a diff that was cut at [`MAX_ROWS`].
pub const TRUNCATED_NOTE: &str = "Showing the first 20,000 lines of this diff.";

/// One row of the flat diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffRow {
    /// The `@@ ... @@` header of the hunk at this index.
    HunkHeader {
        hunk: usize,
    },
    Line {
        pos: Pos,
    },
    /// A comment under its line; the index is into the file's comments.
    Thread {
        comment_index: usize,
    },
    /// The composer, under the last selected line.
    Composer,
    /// The title of the strip of comments with no line in view.
    OutsideTitle,
    OutsideThread {
        comment_index: usize,
    },
    /// Closes a diff cut at [`MAX_ROWS`].
    Truncated,
}

/// The rows of `diff`. `thread_after` lists the comment indexes to show under
/// each line, `composer_after` is the line the composer sits under, and
/// `outside` the comments to put in the closing strip.
#[must_use]
pub fn rows(
    diff: &FileDiff,
    thread_after: &BTreeMap<Pos, Vec<usize>>,
    composer_after: Option<Pos>,
    outside: &[usize],
) -> Vec<DiffRow> {
    let mut out = Vec::new();
    'hunks: for (hunk, body) in diff.hunks.iter().enumerate() {
        if !body.header.is_empty() {
            out.push(DiffRow::HunkHeader { hunk });
        }
        for line in 0..body.lines.len() {
            let pos = Pos { hunk, line };
            out.push(DiffRow::Line { pos });
            for &comment_index in thread_after.get(&pos).into_iter().flatten() {
                out.push(DiffRow::Thread { comment_index });
            }
            if composer_after == Some(pos) {
                out.push(DiffRow::Composer);
            }
            if out.len() > MAX_ROWS {
                break 'hunks;
            }
        }
    }
    if out.len() <= MAX_ROWS && !outside.is_empty() {
        out.push(DiffRow::OutsideTitle);
        out.extend(
            outside
                .iter()
                .map(|&comment_index| DiffRow::OutsideThread { comment_index }),
        );
    }
    if out.len() > MAX_ROWS {
        out.truncate(MAX_ROWS);
        out.push(DiffRow::Truncated);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::{Hunk, Line, LineKind};

    fn line(no: u32) -> Line {
        Line {
            kind: LineKind::Added,
            old_no: None,
            new_no: Some(no),
            text: String::new(),
        }
    }

    fn diff(hunks: &[(&str, usize)]) -> FileDiff {
        FileDiff {
            hunks: hunks
                .iter()
                .map(|(header, count)| Hunk {
                    header: (*header).to_owned(),
                    lines: (1..=u32::try_from(*count).unwrap_or(0)).map(line).collect(),
                })
                .collect(),
            binary: false,
            added: 0,
            removed: 0,
        }
    }

    fn at(hunk: usize, line: usize) -> Pos {
        Pos { hunk, line }
    }

    #[test]
    fn headers_and_lines_come_in_order() {
        let d = diff(&[("@@ a @@", 2), ("@@ b @@", 1)]);
        assert_eq!(
            rows(&d, &BTreeMap::new(), None, &[]),
            vec![
                DiffRow::HunkHeader { hunk: 0 },
                DiffRow::Line { pos: at(0, 0) },
                DiffRow::Line { pos: at(0, 1) },
                DiffRow::HunkHeader { hunk: 1 },
                DiffRow::Line { pos: at(1, 0) },
            ]
        );
    }

    #[test]
    fn an_empty_header_has_no_row() {
        let d = diff(&[("", 1)]);
        assert_eq!(
            rows(&d, &BTreeMap::new(), None, &[]),
            vec![DiffRow::Line { pos: at(0, 0) }]
        );
    }

    #[test]
    fn threads_sit_under_their_line_then_the_composer() {
        let d = diff(&[("", 2)]);
        let mut threads = BTreeMap::new();
        threads.insert(at(0, 0), vec![3, 1]);
        assert_eq!(
            rows(&d, &threads, Some(at(0, 0)), &[]),
            vec![
                DiffRow::Line { pos: at(0, 0) },
                DiffRow::Thread { comment_index: 3 },
                DiffRow::Thread { comment_index: 1 },
                DiffRow::Composer,
                DiffRow::Line { pos: at(0, 1) },
            ]
        );
    }

    #[test]
    fn the_composer_sits_under_a_line_without_threads() {
        let d = diff(&[("", 2)]);
        assert_eq!(
            rows(&d, &BTreeMap::new(), Some(at(0, 1)), &[]),
            vec![
                DiffRow::Line { pos: at(0, 0) },
                DiffRow::Line { pos: at(0, 1) },
                DiffRow::Composer,
            ]
        );
    }

    #[test]
    fn outside_comments_close_the_diff_under_a_title() {
        let d = diff(&[("", 1)]);
        assert_eq!(
            rows(&d, &BTreeMap::new(), None, &[4, 2]),
            vec![
                DiffRow::Line { pos: at(0, 0) },
                DiffRow::OutsideTitle,
                DiffRow::OutsideThread { comment_index: 4 },
                DiffRow::OutsideThread { comment_index: 2 },
            ]
        );
    }

    #[test]
    fn an_empty_diff_has_no_rows_and_no_strip_without_comments() {
        let d = diff(&[]);
        assert!(rows(&d, &BTreeMap::new(), None, &[]).is_empty());
    }

    #[test]
    fn a_huge_diff_is_cut_with_a_final_note() {
        let d = diff(&[("", MAX_ROWS + 5)]);
        let out = rows(&d, &BTreeMap::new(), None, &[0]);
        assert_eq!(out.len(), MAX_ROWS + 1);
        assert_eq!(out[MAX_ROWS], DiffRow::Truncated);
        assert_eq!(
            out[MAX_ROWS - 1],
            DiffRow::Line {
                pos: at(0, MAX_ROWS - 1)
            }
        );
    }

    #[test]
    fn exactly_the_limit_is_not_cut() {
        let d = diff(&[("", MAX_ROWS)]);
        let out = rows(&d, &BTreeMap::new(), None, &[]);
        assert_eq!(out.len(), MAX_ROWS);
        assert!(!out.contains(&DiffRow::Truncated));
    }
}
