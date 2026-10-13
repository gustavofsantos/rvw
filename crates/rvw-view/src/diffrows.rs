//! The diff of one file as a flat list of rows, in the order they are drawn:
//! hunk headers, lines, the comment threads under their line, the composer,
//! and the strip of comments that have no line in view. A virtualised list
//! draws only the rows in view, so a long diff costs what the screen shows.

use std::collections::BTreeMap;

use super::diff::FileDiff;
use super::selection::Pos;
use super::split;

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
    /// One row of the split view: the old file's line on the left, the new
    /// file's on the right. A side with no line is `None`.
    Pair {
        left: Option<Pos>,
        right: Option<Pos>,
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
    /// The composer editing an outside comment, under that comment.
    OutsideComposer,
    /// Closes a diff cut at [`MAX_ROWS`].
    Truncated,
}

/// The rows of `diff`. `thread_after` lists the comment indexes to show under
/// each line, `composer_after` is the line the composer sits under, and
/// `outside` the comments to put in the closing strip, with
/// `composer_outside` the one of them the composer sits under.
#[must_use]
pub fn rows(
    diff: &FileDiff,
    thread_after: &BTreeMap<Pos, Vec<usize>>,
    composer_after: Option<Pos>,
    outside: &[usize],
    composer_outside: Option<usize>,
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
    push_outside(&mut out, outside, composer_outside);
    out
}

/// Closes the rows with the outside strip, then cuts them at [`MAX_ROWS`].
fn push_outside(out: &mut Vec<DiffRow>, outside: &[usize], composer_outside: Option<usize>) {
    if out.len() <= MAX_ROWS && !outside.is_empty() {
        out.push(DiffRow::OutsideTitle);
        for &comment_index in outside {
            out.push(DiffRow::OutsideThread { comment_index });
            if composer_outside == Some(comment_index) {
                out.push(DiffRow::OutsideComposer);
            }
        }
    }
    if out.len() > MAX_ROWS {
        out.truncate(MAX_ROWS);
        out.push(DiffRow::Truncated);
    }
}

/// The rows of the split view: like [`rows`], but each hunk's lines are
/// paired by [`split::rows_of_hunk`], and a thread or the composer sits under
/// the pair that holds its line (on the right when both sides have one).
#[must_use]
pub fn split_rows(
    diff: &FileDiff,
    thread_after: &BTreeMap<Pos, Vec<usize>>,
    composer_after: Option<Pos>,
    outside: &[usize],
    composer_outside: Option<usize>,
) -> Vec<DiffRow> {
    let mut out = Vec::new();
    'hunks: for hunk in 0..diff.hunks.len() {
        if diff.hunks.get(hunk).is_some_and(|h| !h.header.is_empty()) {
            out.push(DiffRow::HunkHeader { hunk });
        }
        for pair in split::rows_of_hunk(diff, hunk) {
            let left = pair.left.map(|cell| cell.pos);
            let right = pair.right.map(|cell| cell.pos);
            out.push(DiffRow::Pair { left, right });
            let mut held = [right, left.filter(|pos| Some(*pos) != right)];
            held.sort_unstable();
            for pos in held.into_iter().flatten() {
                for &comment_index in thread_after.get(&pos).into_iter().flatten() {
                    out.push(DiffRow::Thread { comment_index });
                }
                if composer_after == Some(pos) {
                    out.push(DiffRow::Composer);
                }
            }
            if out.len() > MAX_ROWS {
                break 'hunks;
            }
        }
    }
    push_outside(&mut out, outside, composer_outside);
    out
}

/// The hunk the row at `index` belongs to, looking back from a thread or
/// composer row to the line it sits under. `None` for the outside strip.
#[must_use]
pub fn hunk_of_row(rows: &[DiffRow], index: usize) -> Option<usize> {
    rows.get(..=index)?.iter().rev().find_map(|row| match *row {
        DiffRow::HunkHeader { hunk } => Some(hunk),
        DiffRow::Line { pos } => Some(pos.hunk),
        DiffRow::Pair { left, right } => right.or(left).map(|pos| pos.hunk),
        _ => None,
    })
}

/// The first row of `hunk`: its header, or its first line.
#[must_use]
pub fn first_row_of_hunk(rows: &[DiffRow], hunk: usize) -> Option<usize> {
    rows.iter().position(|row| match *row {
        DiffRow::HunkHeader { hunk: at } => at == hunk,
        DiffRow::Line { pos } => pos.hunk == hunk,
        DiffRow::Pair { left, right } => right.or(left).is_some_and(|pos| pos.hunk == hunk),
        _ => false,
    })
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
            rows(&d, &BTreeMap::new(), None, &[], None),
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
            rows(&d, &BTreeMap::new(), None, &[], None),
            vec![DiffRow::Line { pos: at(0, 0) }]
        );
    }

    #[test]
    fn threads_sit_under_their_line_then_the_composer() {
        let d = diff(&[("", 2)]);
        let mut threads = BTreeMap::new();
        threads.insert(at(0, 0), vec![3, 1]);
        assert_eq!(
            rows(&d, &threads, Some(at(0, 0)), &[], None),
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
            rows(&d, &BTreeMap::new(), Some(at(0, 1)), &[], None),
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
            rows(&d, &BTreeMap::new(), None, &[4, 2], None),
            vec![
                DiffRow::Line { pos: at(0, 0) },
                DiffRow::OutsideTitle,
                DiffRow::OutsideThread { comment_index: 4 },
                DiffRow::OutsideThread { comment_index: 2 },
            ]
        );
    }

    #[test]
    fn an_outside_composer_sits_under_its_comment() {
        let d = diff(&[("", 1)]);
        assert_eq!(
            rows(&d, &BTreeMap::new(), None, &[4, 2], Some(4)),
            vec![
                DiffRow::Line { pos: at(0, 0) },
                DiffRow::OutsideTitle,
                DiffRow::OutsideThread { comment_index: 4 },
                DiffRow::OutsideComposer,
                DiffRow::OutsideThread { comment_index: 2 },
            ]
        );
    }

    #[test]
    fn an_empty_diff_has_no_rows_and_no_strip_without_comments() {
        let d = diff(&[]);
        assert!(rows(&d, &BTreeMap::new(), None, &[], None).is_empty());
    }

    #[test]
    fn a_huge_diff_is_cut_with_a_final_note() {
        let d = diff(&[("", MAX_ROWS + 5)]);
        let out = rows(&d, &BTreeMap::new(), None, &[0], None);
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
        let out = rows(&d, &BTreeMap::new(), None, &[], None);
        assert_eq!(out.len(), MAX_ROWS);
        assert!(!out.contains(&DiffRow::Truncated));
    }

    fn parsed(text: &str) -> FileDiff {
        crate::diff::parse(text)
    }

    #[test]
    fn split_rows_pair_lines_and_put_threads_under_the_right_cell() {
        let d = parsed("@@ -1,3 +1,3 @@\n c1\n-r1\n+a1\n c2\n");
        let mut threads = BTreeMap::new();
        threads.insert(at(0, 2), vec![5]);
        assert_eq!(
            split_rows(&d, &threads, Some(at(0, 3)), &[], None),
            vec![
                DiffRow::HunkHeader { hunk: 0 },
                DiffRow::Pair {
                    left: Some(at(0, 0)),
                    right: Some(at(0, 0))
                },
                DiffRow::Pair {
                    left: Some(at(0, 1)),
                    right: Some(at(0, 2))
                },
                DiffRow::Thread { comment_index: 5 },
                DiffRow::Pair {
                    left: Some(at(0, 3)),
                    right: Some(at(0, 3))
                },
                DiffRow::Composer,
            ]
        );
    }

    #[test]
    fn split_rows_place_a_removed_line_thread_on_its_left_cell() {
        let d = parsed("@@ -1,2 +0,0 @@\n-x\n-y\n");
        let mut threads = BTreeMap::new();
        threads.insert(at(0, 1), vec![0]);
        let out = split_rows(&d, &threads, None, &[], None);
        assert_eq!(out[out.len() - 1], DiffRow::Thread { comment_index: 0 });
        assert_eq!(out.len(), 4);
    }

    #[test]
    fn split_rows_close_with_the_outside_strip() {
        let d = parsed("@@ -0,0 +1 @@\n+a\n");
        let out = split_rows(&d, &BTreeMap::new(), None, &[2], Some(2));
        assert_eq!(
            &out[2..],
            &[
                DiffRow::OutsideTitle,
                DiffRow::OutsideThread { comment_index: 2 },
                DiffRow::OutsideComposer,
            ]
        );
    }

    #[test]
    fn the_hunk_of_a_row_looks_back_from_threads() {
        let rows = vec![
            DiffRow::HunkHeader { hunk: 0 },
            DiffRow::Line { pos: at(0, 0) },
            DiffRow::HunkHeader { hunk: 1 },
            DiffRow::Pair {
                left: None,
                right: Some(at(1, 0)),
            },
            DiffRow::Thread { comment_index: 0 },
            DiffRow::OutsideTitle,
        ];
        assert_eq!(hunk_of_row(&rows, 1), Some(0));
        assert_eq!(hunk_of_row(&rows, 4), Some(1));
        assert_eq!(hunk_of_row(&rows, 9), None);
        assert_eq!(first_row_of_hunk(&rows, 1), Some(2));
        assert_eq!(first_row_of_hunk(&rows, 7), None);
    }

    #[test]
    fn first_row_of_a_hunk_without_header_is_its_first_line() {
        let rows = vec![DiffRow::Pair {
            left: Some(at(0, 0)),
            right: None,
        }];
        assert_eq!(first_row_of_hunk(&rows, 0), Some(0));
    }
}
