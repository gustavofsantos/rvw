//! Choosing a range of diff lines to comment on.
//!
//! A comment is anchored to new-file line numbers, so a removed line, which
//! has none, can be neither the start nor the end of a selection.

use super::diff::{FileDiff, Line, LineKind};

/// A line of the diff, addressed by hunk and line index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Pos {
    pub hunk: usize,
    pub line: usize,
}

/// The reviewer's current selection: an anchor line and a head line, both
/// set or both unset. The default selection is empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Selection {
    anchor: Option<Pos>,
    head: Option<Pos>,
}

impl Selection {
    /// Whether nothing is selected.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.anchor.is_none()
    }

    /// Click on a line: selects it alone. A removed line, or a position
    /// outside the diff, leaves the selection unchanged.
    pub fn click(&mut self, diff: &FileDiff, pos: Pos) {
        if selectable(diff, pos) {
            *self = Selection {
                anchor: Some(pos),
                head: Some(pos),
            };
        }
    }

    /// Shift-click: extends from the anchor to `pos`. With no anchor it
    /// behaves as `click`. A removed line, or a position outside the diff,
    /// is ignored.
    pub fn extend(&mut self, diff: &FileDiff, pos: Pos) {
        if !selectable(diff, pos) {
            return;
        }
        if self.anchor.is_none() {
            self.click(diff, pos);
        } else {
            self.head = Some(pos);
        }
    }

    /// Drops the selection.
    pub fn clear(&mut self) {
        *self = Selection::default();
    }

    /// The selected range as new-file line numbers (start <= end), or `None`
    /// when empty. Removed lines between the ends are skipped.
    #[must_use]
    pub fn lines(&self, diff: &FileDiff) -> Option<(u32, u32)> {
        let (lo, hi) = self.bounds()?;
        let mut numbers = cells(diff)
            .filter(|(pos, _)| lo <= *pos && *pos <= hi)
            .filter_map(|(_, line)| line.new_no);
        let first = numbers.next()?;
        Some(numbers.fold((first, first), |(min, max), n| (min.min(n), max.max(n))))
    }

    /// Whether the line at `pos` is inside the selection, for highlighting.
    /// Removed lines are never inside.
    #[must_use]
    pub fn contains(&self, diff: &FileDiff, pos: Pos) -> bool {
        let Some((lo, hi)) = self.bounds() else {
            return false;
        };
        lo <= pos && pos <= hi && selectable(diff, pos)
    }

    /// The later of the anchor and head in diff order, or `None` when empty.
    /// This is the line a composer for the selection sits under.
    #[must_use]
    pub fn last(&self) -> Option<Pos> {
        self.bounds().map(|(_, hi)| hi)
    }

    /// The anchor and head in diff order.
    fn bounds(&self) -> Option<(Pos, Pos)> {
        let (anchor, head) = (self.anchor?, self.head?);
        Some(if anchor <= head {
            (anchor, head)
        } else {
            (head, anchor)
        })
    }
}

/// Where a comment that ends on new-file line `end_line` sits in the diff: the
/// added or context line with that number. `None` when no shown hunk has it,
/// as for a line outside every hunk or a comment on a removed line.
#[must_use]
pub fn anchor(diff: &FileDiff, end_line: i64) -> Option<Pos> {
    cells(diff)
        .find(|(_, line)| line.new_no.is_some_and(|n| i64::from(n) == end_line))
        .map(|(pos, _)| pos)
}

/// The line at `pos`, if the diff has one there.
fn line_at(diff: &FileDiff, pos: Pos) -> Option<&Line> {
    diff.hunks.get(pos.hunk)?.lines.get(pos.line)
}

/// Whether `pos` is a line of the diff that can be selected: not removed.
fn selectable(diff: &FileDiff, pos: Pos) -> bool {
    line_at(diff, pos).is_some_and(|line| line.kind != LineKind::Removed)
}

/// Every line of the diff with its position, in diff order.
fn cells(diff: &FileDiff) -> impl Iterator<Item = (Pos, &Line)> + '_ {
    diff.hunks.iter().enumerate().flat_map(|(hunk, h)| {
        h.lines
            .iter()
            .enumerate()
            .map(move |(line, l)| (Pos { hunk, line }, l))
    })
}

#[cfg(test)]
mod tests {
    use super::super::diff::parse;
    use super::*;

    /// Two hunks: new lines 10..=12 (with removed line 11), then 40..=42.
    const SAMPLE: &str = "\
@@ -10,3 +10,3 @@
 ten
-eleven
+ELEVEN
 twelve
@@ -40,2 +40,3 @@
 forty
+new a
 fifty
";

    fn sample() -> FileDiff {
        parse(SAMPLE)
    }

    fn pos(hunk: usize, line: usize) -> Pos {
        Pos { hunk, line }
    }

    #[test]
    fn default_is_empty_and_has_no_lines() {
        let sel = Selection::default();
        assert!(sel.is_empty());
        assert_eq!(sel.lines(&sample()), None);
        assert!(!sel.contains(&sample(), pos(0, 0)));
    }

    #[test]
    fn click_selects_one_line() {
        let diff = sample();
        let mut sel = Selection::default();
        sel.click(&diff, pos(0, 0));
        assert!(!sel.is_empty());
        assert_eq!(sel.lines(&diff), Some((10, 10)));
    }

    #[test]
    fn extend_forward_and_backward_give_the_same_range() {
        let diff = sample();
        let mut forward = Selection::default();
        forward.click(&diff, pos(0, 0));
        forward.extend(&diff, pos(0, 3));

        let mut backward = Selection::default();
        backward.click(&diff, pos(0, 3));
        backward.extend(&diff, pos(0, 0));

        assert_eq!(forward.lines(&diff), Some((10, 12)));
        assert_eq!(backward.lines(&diff), Some((10, 12)));
    }

    #[test]
    fn extend_across_two_hunks_uses_new_file_numbers() {
        let diff = sample();
        let mut sel = Selection::default();
        sel.click(&diff, pos(0, 2));
        sel.extend(&diff, pos(1, 1));
        assert_eq!(sel.lines(&diff), Some((11, 41)));
    }

    #[test]
    fn extend_without_anchor_behaves_as_click() {
        let diff = sample();
        let mut sel = Selection::default();
        sel.extend(&diff, pos(0, 2));
        assert_eq!(sel.lines(&diff), Some((11, 11)));
    }

    #[test]
    fn click_after_extend_resets_to_one_line() {
        let diff = sample();
        let mut sel = Selection::default();
        sel.click(&diff, pos(0, 0));
        sel.extend(&diff, pos(1, 2));
        sel.click(&diff, pos(0, 3));
        assert_eq!(sel.lines(&diff), Some((12, 12)));
    }

    #[test]
    fn click_on_removed_line_keeps_selection() {
        let diff = sample();
        let mut sel = Selection::default();
        sel.click(&diff, pos(0, 0));
        sel.click(&diff, pos(0, 1));
        assert_eq!(sel.lines(&diff), Some((10, 10)));
    }

    #[test]
    fn click_on_removed_line_when_empty_stays_empty() {
        let diff = sample();
        let mut sel = Selection::default();
        sel.click(&diff, pos(0, 1));
        assert!(sel.is_empty());
    }

    #[test]
    fn extend_to_removed_line_is_ignored() {
        let diff = sample();
        let mut sel = Selection::default();
        sel.click(&diff, pos(0, 0));
        sel.extend(&diff, pos(0, 1));
        assert_eq!(sel.lines(&diff), Some((10, 10)));
    }

    #[test]
    fn removed_lines_between_the_ends_are_skipped_in_the_range() {
        let diff = sample();
        let mut sel = Selection::default();
        sel.click(&diff, pos(0, 0));
        sel.extend(&diff, pos(0, 3));
        // Line 11 is removed in the old file only: the range is 10..=12.
        assert_eq!(sel.lines(&diff), Some((10, 12)));
    }

    #[test]
    fn positions_outside_the_diff_are_ignored() {
        let diff = sample();
        let mut sel = Selection::default();
        sel.click(&diff, pos(5, 0));
        assert!(sel.is_empty());
        sel.click(&diff, pos(0, 99));
        assert!(sel.is_empty());

        sel.click(&diff, pos(0, 0));
        sel.extend(&diff, pos(2, 0));
        sel.extend(&diff, pos(1, 99));
        assert_eq!(sel.lines(&diff), Some((10, 10)));
    }

    #[test]
    fn contains_skips_removed_lines_between_the_ends() {
        let diff = sample();
        let mut sel = Selection::default();
        sel.click(&diff, pos(0, 0));
        sel.extend(&diff, pos(0, 3));
        assert!(sel.contains(&diff, pos(0, 0)));
        assert!(sel.contains(&diff, pos(0, 2)));
        assert!(sel.contains(&diff, pos(0, 3)));
        assert!(!sel.contains(&diff, pos(0, 1)));
        assert!(!sel.contains(&diff, pos(1, 0)));
    }

    #[test]
    fn contains_follows_the_range_across_hunks() {
        let diff = sample();
        let mut sel = Selection::default();
        sel.click(&diff, pos(0, 2));
        sel.extend(&diff, pos(1, 1));
        assert!(sel.contains(&diff, pos(0, 2)));
        assert!(sel.contains(&diff, pos(1, 0)));
        assert!(sel.contains(&diff, pos(1, 1)));
        assert!(!sel.contains(&diff, pos(1, 2)));
        assert!(!sel.contains(&diff, pos(0, 0)));
    }

    #[test]
    fn last_is_the_later_end_in_diff_order() {
        let diff = sample();
        let mut sel = Selection::default();
        assert_eq!(sel.last(), None);
        sel.click(&diff, pos(1, 1));
        sel.extend(&diff, pos(0, 0));
        assert_eq!(sel.last(), Some(pos(1, 1)));
    }

    #[test]
    fn anchor_finds_the_new_numbered_line() {
        let diff = sample();
        assert_eq!(anchor(&diff, 10), Some(pos(0, 0)));
        // The removed line 11 has no new number; the added one is 11.
        assert_eq!(anchor(&diff, 11), Some(pos(0, 2)));
        assert_eq!(anchor(&diff, 12), Some(pos(0, 3)));
        assert_eq!(anchor(&diff, 41), Some(pos(1, 1)));
    }

    #[test]
    fn anchor_is_none_outside_every_shown_hunk() {
        let diff = sample();
        assert_eq!(anchor(&diff, 9), None);
        assert_eq!(anchor(&diff, 13), None);
        assert_eq!(anchor(&diff, 0), None);
        assert_eq!(anchor(&diff, -1), None);
    }

    #[test]
    fn clear_empties_the_selection() {
        let diff = sample();
        let mut sel = Selection::default();
        sel.click(&diff, pos(0, 0));
        sel.extend(&diff, pos(1, 2));
        sel.clear();
        assert!(sel.is_empty());
        assert_eq!(sel.lines(&diff), None);
        assert!(!sel.contains(&diff, pos(0, 0)));
    }
}
