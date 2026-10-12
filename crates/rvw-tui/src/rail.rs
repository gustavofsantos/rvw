//! Port of internal/tui/rail.go: the gutter rail that marks every line of a
//! comment's range, and the line arithmetic for jumping between comments.
//!
//! Lines are 1-indexed, as comments store them.

use rvw_core::types::Comment;

/// The glyph of a comment that covers exactly one line.
pub const RAIL_SINGLE: &str = "●";
/// The glyph of a comment's first line, when it covers several.
pub const RAIL_FIRST: &str = "╭";
/// The glyph of the lines between a comment's first and last.
pub const RAIL_MIDDLE: &str = "│";
/// The glyph of a comment's last line, when it covers several.
pub const RAIL_LAST: &str = "╰";

/// Which way [`next_comment`] and [`nearest`] look from a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Toward higher line numbers.
    Forward,
    /// Toward lower line numbers.
    Backward,
}

/// The comment whose rail is drawn on a line, and its glyph: the most recent
/// of the comments covering it (comments come oldest first). `None` for an
/// uncovered line.
#[must_use]
pub fn rail_at(cs: &[Comment], line: i64) -> Option<(&Comment, &'static str)> {
    cs.iter()
        .rev()
        .find(|c| covers(c, line))
        .map(|c| (c, rail_glyph(c, line)))
}

/// The glyph a comment draws on a line it covers: a single glyph for a
/// one-line comment, else the first, last or a middle glyph of its range.
/// The caller passes a line the comment covers.
#[must_use]
pub fn rail_glyph(c: &Comment, line: i64) -> &'static str {
    if c.start_line == c.end_line {
        RAIL_SINGLE
    } else if line == c.start_line {
        RAIL_FIRST
    } else if line == c.end_line {
        RAIL_LAST
    } else {
        RAIL_MIDDLE
    }
}

/// Whether the comment's range includes the line.
#[must_use]
pub fn covers(c: &Comment, line: i64) -> bool {
    c.start_line <= line && line <= c.end_line
}

/// Every comment covering a line, oldest first.
#[must_use]
pub fn covering(cs: &[Comment], line: i64) -> Vec<&Comment> {
    cs.iter().filter(|c| covers(c, line)).collect()
}

/// The first line of the nearest comment starting after `line` (forward) or
/// before it (backward). `None` when there is no such comment.
#[must_use]
pub fn next_comment(cs: &[Comment], line: i64, dir: Direction) -> Option<i64> {
    nearest(cs.iter().map(|c| c.start_line), line, dir)
}

/// The closest of `lines` after `line` (forward) or before it (backward).
/// `None` when no line is on that side. When two are equally close, the
/// first one given wins.
#[must_use]
pub fn nearest(lines: impl IntoIterator<Item = i64>, line: i64, dir: Direction) -> Option<i64> {
    let mut best: Option<i64> = None;
    for s in lines {
        let on_side = match dir {
            Direction::Forward => s > line,
            Direction::Backward => s < line,
        };
        if !on_side {
            continue;
        }
        let closer = match (best, dir) {
            (None, _) => true,
            (Some(b), Direction::Forward) => s < b,
            (Some(b), Direction::Backward) => s > b,
        };
        if closer {
            best = Some(s);
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use rvw_core::types::Status;

    /// A pending comment on lines `start..=end` of one file.
    fn comment(id: &str, start: i64, end: i64) -> Comment {
        Comment {
            id: id.to_owned(),
            status: Status::Pending,
            workspace: "/ws".to_owned(),
            lane: None,
            author: None,
            file: "a.py".to_owned(),
            path: "/ws/a.py".to_owned(),
            start_line: start,
            end_line: end,
            filetype: "python".to_owned(),
            code: String::new(),
            comment: "note".to_owned(),
            created_at: "2026-01-01T00:00:00Z".to_owned(),
            pulled_at: None,
            resolved_at: None,
            resolved_by: None,
            resolution_note: None,
            file_version: String::new(),
            resolved_file_version: String::new(),
            review_id: String::new(),
            edited_at: String::new(),
        }
    }

    #[test]
    fn rail_marks_every_line_of_a_range() {
        let cs = [
            comment("r1", 2, 5),
            comment("r2", 7, 7),
            comment("r3", 4, 4),
        ];
        let want = [" ", "╭", "│", "●", "╰", " ", "●", " "];
        for (idx, want) in want.iter().enumerate() {
            let line = i64::try_from(idx + 1).unwrap();
            let glyph = rail_at(&cs, line).map_or(" ", |(_, g)| g);
            assert_eq!(glyph, *want, "line {line}");
        }
        let (c, _) = rail_at(&cs, 4).unwrap();
        assert_eq!(c.id, "r3", "overlap draws the most recent");
    }

    #[test]
    fn covering_lists_oldest_first() {
        let cs = [
            comment("r1", 2, 5),
            comment("r2", 7, 7),
            comment("r3", 4, 4),
        ];
        let got: Vec<&str> = covering(&cs, 4).iter().map(|c| c.id.as_str()).collect();
        assert_eq!(got, ["r1", "r3"]);
        assert!(covering(&cs, 1).is_empty());
    }

    #[test]
    fn next_and_previous_comment_lines() {
        let cs = [
            comment("r1", 2, 5),
            comment("r2", 7, 7),
            comment("r3", 4, 4),
        ];
        assert_eq!(next_comment(&cs, 2, Direction::Forward), Some(4));
        assert_eq!(next_comment(&cs, 4, Direction::Backward), Some(2));
        assert_eq!(next_comment(&cs, 7, Direction::Forward), None);
        assert_eq!(next_comment(&cs, 2, Direction::Backward), None);
        assert_eq!(next_comment(&[], 1, Direction::Forward), None);
    }

    #[test]
    fn next_comment_uses_start_lines_so_a_covered_line_is_not_a_stop() {
        // Line 3 is inside r1 but starts nothing: forward from 2 goes to 4.
        let cs = [comment("r1", 2, 5), comment("r2", 4, 4)];
        assert_eq!(next_comment(&cs, 2, Direction::Forward), Some(4));
    }

    #[test]
    fn nearest_picks_the_closest_line_on_each_side() {
        let lines = [10, 3, 7, 3, 12];
        assert_eq!(nearest(lines, 5, Direction::Forward), Some(7));
        assert_eq!(nearest(lines, 5, Direction::Backward), Some(3));
        assert_eq!(nearest(lines, 12, Direction::Forward), None);
        assert_eq!(nearest(lines, 2, Direction::Backward), None);
        assert_eq!(nearest(Vec::<i64>::new(), 5, Direction::Forward), None);
    }

    #[test]
    fn nearest_is_not_fooled_by_a_line_equal_to_the_origin() {
        assert_eq!(nearest([5], 5, Direction::Forward), None);
        assert_eq!(nearest([5], 5, Direction::Backward), None);
    }

    #[test]
    fn rail_glyph_for_a_single_line_comment_is_always_the_dot() {
        let c = comment("r1", 9, 9);
        assert_eq!(rail_glyph(&c, 9), RAIL_SINGLE);
    }

    #[test]
    fn rail_at_an_uncovered_line_is_none() {
        let cs = [comment("r1", 2, 3)];
        assert!(rail_at(&cs, 1).is_none());
        assert!(rail_at(&cs, 4).is_none());
        assert!(rail_at(&[], 1).is_none());
    }

    #[test]
    fn covers_is_inclusive_at_both_ends() {
        let c = comment("r1", 2, 4);
        assert!(covers(&c, 2) && covers(&c, 4));
        assert!(!covers(&c, 1) && !covers(&c, 5));
    }
}
