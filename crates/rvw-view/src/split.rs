//! The side-by-side (split) view of one file's diff: the old file on the left,
//! the new file on the right. Each row pairs a left cell with a right cell;
//! a cell keeps the `Pos` of its line in the unified hunk, so selection and
//! comment anchoring work the same in both views.

use super::diff::{FileDiff, Line, LineKind};
use super::selection::Pos;

/// One cell of a split row: the line it shows and where it sits in the unified diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    pub pos: Pos,
    pub kind: LineKind,
    /// The line number on this cell's side of the diff.
    pub no: Option<u32>,
    pub text: String,
}

/// One visual row: a pair of cells. Context lines appear on both sides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitRow {
    pub left: Option<Cell>,
    pub right: Option<Cell>,
}

/// The split rows of one hunk (hunk index `hunk` of the diff), in order.
/// A hunk index out of range gives no rows.
#[must_use]
pub fn rows_of_hunk(diff: &FileDiff, hunk: usize) -> Vec<SplitRow> {
    let Some(body) = diff.hunks.get(hunk) else {
        return Vec::new();
    };
    let lines = &body.lines;
    let mut rows = Vec::new();
    let mut i = 0;
    while let Some(line) = lines.get(i) {
        match line.kind {
            LineKind::Context => {
                rows.push(SplitRow {
                    left: cell_at(hunk, lines, i, Side::Old),
                    right: cell_at(hunk, lines, i, Side::New),
                });
                i += 1;
            }
            LineKind::Removed | LineKind::Added => {
                // A run of removed lines, then the run of added lines after it,
                // are paired index by index.
                let removed_end = run_end(lines, i, LineKind::Removed);
                let added_end = run_end(lines, removed_end, LineKind::Added);
                for k in 0..(removed_end - i).max(added_end - removed_end) {
                    let left = (i + k < removed_end)
                        .then(|| cell_at(hunk, lines, i + k, Side::Old))
                        .flatten();
                    let right = (removed_end + k < added_end)
                        .then(|| cell_at(hunk, lines, removed_end + k, Side::New))
                        .flatten();
                    rows.push(SplitRow { left, right });
                }
                i = added_end;
            }
        }
    }
    rows
}

/// The number of split rows of the whole diff, summed over its hunks.
#[must_use]
pub fn row_count(diff: &FileDiff) -> usize {
    (0..diff.hunks.len())
        .map(|hunk| rows_of_hunk(diff, hunk).len())
        .sum()
}

/// Which file a cell shows.
#[derive(Clone, Copy)]
enum Side {
    Old,
    New,
}

/// The cell for the line at `index` on `side`, or `None` past the last line.
fn cell_at(hunk: usize, lines: &[Line], index: usize, side: Side) -> Option<Cell> {
    let line = lines.get(index)?;
    let no = match side {
        Side::Old => line.old_no,
        Side::New => line.new_no,
    };
    Some(Cell {
        pos: Pos { hunk, line: index },
        kind: line.kind,
        no,
        text: line.text.clone(),
    })
}

/// The index just past the run of `kind` lines that starts at `from`.
fn run_end(lines: &[Line], from: usize, kind: LineKind) -> usize {
    let mut end = from;
    while lines.get(end).is_some_and(|line| line.kind == kind) {
        end += 1;
    }
    end
}

#[cfg(test)]
mod tests {
    use super::super::diff::parse;
    use super::*;

    fn pos(hunk: usize, line: usize) -> Pos {
        Pos { hunk, line }
    }

    fn cell(pos: Pos, kind: LineKind, no: Option<u32>, text: &str) -> Cell {
        Cell {
            pos,
            kind,
            no,
            text: text.to_owned(),
        }
    }

    #[test]
    fn context_only_hunk_has_both_sides_on_every_row() {
        let diff = parse("@@ -1,2 +1,2 @@\n one\n two\n");
        assert_eq!(
            rows_of_hunk(&diff, 0),
            vec![
                SplitRow {
                    left: Some(cell(pos(0, 0), LineKind::Context, Some(1), "one")),
                    right: Some(cell(pos(0, 0), LineKind::Context, Some(1), "one")),
                },
                SplitRow {
                    left: Some(cell(pos(0, 1), LineKind::Context, Some(2), "two")),
                    right: Some(cell(pos(0, 1), LineKind::Context, Some(2), "two")),
                },
            ]
        );
    }

    #[test]
    fn replace_of_two_by_three_pairs_then_leaves_left_empty() {
        let diff = parse("@@ -1,2 +1,3 @@\n-a\n-b\n+A\n+B\n+C\n");
        assert_eq!(
            rows_of_hunk(&diff, 0),
            vec![
                SplitRow {
                    left: Some(cell(pos(0, 0), LineKind::Removed, Some(1), "a")),
                    right: Some(cell(pos(0, 2), LineKind::Added, Some(1), "A")),
                },
                SplitRow {
                    left: Some(cell(pos(0, 1), LineKind::Removed, Some(2), "b")),
                    right: Some(cell(pos(0, 3), LineKind::Added, Some(2), "B")),
                },
                SplitRow {
                    left: None,
                    right: Some(cell(pos(0, 4), LineKind::Added, Some(3), "C")),
                },
            ]
        );
    }

    #[test]
    fn pure_removal_has_no_right_side() {
        let diff = parse("@@ -1,2 +0,0 @@\n-x\n-y\n");
        let rows = rows_of_hunk(&diff, 0);
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| row.right.is_none()));
        assert_eq!(
            rows[1].left,
            Some(cell(pos(0, 1), LineKind::Removed, Some(2), "y"))
        );
    }

    #[test]
    fn pure_addition_has_no_left_side() {
        let diff = parse("@@ -0,0 +1,2 @@\n+a\n+b\n");
        let rows = rows_of_hunk(&diff, 0);
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| row.left.is_none()));
        assert_eq!(
            rows[0].right,
            Some(cell(pos(0, 0), LineKind::Added, Some(1), "a"))
        );
    }

    #[test]
    fn interleaved_context_removed_added_context() {
        let diff = parse("@@ -1,5 +1,5 @@\n c1\n-r1\n-r2\n+a1\n c2\n+a2\n c3\n");
        assert_eq!(
            rows_of_hunk(&diff, 0),
            vec![
                SplitRow {
                    left: Some(cell(pos(0, 0), LineKind::Context, Some(1), "c1")),
                    right: Some(cell(pos(0, 0), LineKind::Context, Some(1), "c1")),
                },
                SplitRow {
                    left: Some(cell(pos(0, 1), LineKind::Removed, Some(2), "r1")),
                    right: Some(cell(pos(0, 3), LineKind::Added, Some(2), "a1")),
                },
                SplitRow {
                    left: Some(cell(pos(0, 2), LineKind::Removed, Some(3), "r2")),
                    right: None,
                },
                SplitRow {
                    left: Some(cell(pos(0, 4), LineKind::Context, Some(4), "c2")),
                    right: Some(cell(pos(0, 4), LineKind::Context, Some(3), "c2")),
                },
                SplitRow {
                    left: None,
                    right: Some(cell(pos(0, 5), LineKind::Added, Some(4), "a2")),
                },
                SplitRow {
                    left: Some(cell(pos(0, 6), LineKind::Context, Some(5), "c3")),
                    right: Some(cell(pos(0, 6), LineKind::Context, Some(5), "c3")),
                },
            ]
        );
    }

    #[test]
    fn line_numbers_follow_a_hunk_starting_at_old_ten_and_new_twelve() {
        let diff = parse("@@ -10,3 +12,4 @@\n ten\n-eleven\n+ELEVEN\n+extra\n twelve\n");
        let rows = rows_of_hunk(&diff, 0);
        assert_eq!(rows.len(), 4);
        let number = |cell: &Option<Cell>| cell.as_ref().and_then(|c| c.no);
        assert_eq!(
            (number(&rows[0].left), number(&rows[0].right)),
            (Some(10), Some(12))
        );
        assert_eq!(
            (number(&rows[1].left), number(&rows[1].right)),
            (Some(11), Some(13))
        );
        assert_eq!(
            (number(&rows[2].left), number(&rows[2].right)),
            (None, Some(14))
        );
        assert_eq!(
            (number(&rows[3].left), number(&rows[3].right)),
            (Some(12), Some(15))
        );
    }

    #[test]
    fn positions_address_the_unified_hunk_in_the_second_hunk() {
        let diff = parse("@@ -1,1 +1,1 @@\n-a\n+b\n@@ -5,2 +5,2 @@\n x\n-y\n+Y\n");
        let rows = rows_of_hunk(&diff, 1);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].left.as_ref().map(|c| c.pos), Some(pos(1, 0)));
        assert_eq!(rows[1].left.as_ref().map(|c| c.pos), Some(pos(1, 1)));
        assert_eq!(rows[1].right.as_ref().map(|c| c.pos), Some(pos(1, 2)));
    }

    #[test]
    fn out_of_range_hunk_has_no_rows() {
        let diff = parse("@@ -1,1 +1,1 @@\n-a\n+b\n");
        assert_eq!(rows_of_hunk(&diff, 1), Vec::new());
        assert_eq!(rows_of_hunk(&diff, usize::MAX), Vec::new());
    }

    #[test]
    fn row_count_sums_the_rows_of_every_hunk() {
        let diff = parse("@@ -1,2 +1,3 @@\n-a\n-b\n+A\n+B\n+C\n@@ -10,2 +11,2 @@\n one\n two\n");
        assert_eq!(row_count(&diff), 5);
        assert_eq!(row_count(&parse("")), 0);
    }
}
