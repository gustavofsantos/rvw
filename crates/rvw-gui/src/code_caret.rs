//! The caret and the character selection of the code area, in the diff view
//! and the whole-file view alike. A press in the code places the caret at the
//! character nearest the pointer; a drag selects characters across rows; a
//! double press selects a word; Shift extends. The positions and ranges are
//! `rvw_view::textpos`; this file maps pixels to them and holds the state the
//! shell keeps. Lines are addressed by their index over every line of the
//! diff, removed ones included, in the order the rows show them, so a
//! selection can run across hunks and copy as plain text.

use std::ops::Range;

use gpui::{App, ClipboardItem, Context, KeyBinding, actions};
use rvw_view::diff::FileDiff;
use rvw_view::selection::Pos;
use rvw_view::textpos::{self, TextPos, TextSelection};

use crate::shell::Shell;

actions!(rvw_code, [CopyCode]);

/// Binds ⌘C (Ctrl-C off macOS) to copying the text selection. The terminal
/// and the text fields keep their own keys, so Ctrl-C still reaches a shell.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([KeyBinding::new(
        "secondary-c",
        CopyCode,
        Some("Rvw && !Terminal && !RvwTextInput && !RvwTextArea"),
    )]);
}

/// Columns a tab takes, as `code_scroll` counts them.
pub const TAB_WIDTH: usize = 4;

/// Pixels from a row's left edge to its code text: the number column, the
/// marker and the padding before the code.
pub const TEXT_INSET: f32 = 62.;

/// The pixel gap between the two columns of a split row (its hairline).
const SPLIT_HAIRLINE: f32 = 1.;

/// The width of the caret bar.
pub const CARET_WIDTH: f32 = 2.;

/// Which column of a row a code area is in. A unified row has one; a split
/// row has one per side, each with its own gutter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Column {
    Unified,
    Left,
    Right,
}

/// The index of the first line of each hunk, counting every line of the diff.
pub fn hunk_starts(diff: &FileDiff) -> Vec<usize> {
    let mut next = 0;
    diff.hunks
        .iter()
        .map(|hunk| {
            let start = next;
            next += hunk.lines.len();
            start
        })
        .collect()
}

/// The index over every line of the diff of the line at `pos`.
pub fn line_index(starts: &[usize], pos: Pos) -> Option<usize> {
    starts.get(pos.hunk).map(|start| start + pos.line)
}

/// The text of every line of the diff, in index order.
pub fn lines(diff: &FileDiff) -> Vec<&str> {
    diff.hunks
        .iter()
        .flat_map(|hunk| hunk.lines.iter().map(|line| line.text.as_str()))
        .collect()
}

/// The window x where the code text of `column` starts, given the left edge
/// and the width of the list it is drawn in. A split row's right column
/// starts after the left one and the hairline between them.
pub fn text_origin(left: f32, width: f32, column: Column) -> f32 {
    let half = (width - SPLIT_HAIRLINE) / 2.;
    let start = match column {
        Column::Unified | Column::Left => left,
        Column::Right => left + half + SPLIT_HAIRLINE,
    };
    start + TEXT_INSET
}

/// The cell column under window x `x`, for code whose text starts at
/// `origin` and is moved left by `scroll_x`. A point left of the text is
/// column 0.
pub fn column_at(x: f32, origin: f32, scroll_x: f32, advance: f32) -> f32 {
    if advance <= 0. {
        return 0.;
    }
    ((x - origin + scroll_x) / advance).max(0.)
}

/// The byte offset in `text` nearest to cell `column`.
pub fn byte_at(text: &str, column: f32) -> usize {
    textpos::from_column(text, column, TAB_WIDTH)
}

/// `cells` cell widths in pixels.
fn cells_px(cells: usize, advance: f32) -> f32 {
    f32::from(u16::try_from(cells).unwrap_or(u16::MAX)) * advance
}

/// What one row draws of the text state, in pixels from the start of its
/// code text: the tint behind selected characters, and the caret bar.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Marks {
    /// The left edge and width of the tint.
    pub tint: Option<(f32, f32)>,
    /// The left edge of the caret.
    pub caret: Option<f32>,
}

/// The marks of the line with index `line` and text `text`. A caret shows
/// on its own line only; a selection's tint covers the selected bytes.
pub fn marks(sel: Option<TextSelection>, line: Option<usize>, text: &str, advance: f32) -> Marks {
    let (Some(sel), Some(line)) = (sel, line) else {
        return Marks::default();
    };
    if sel.is_empty() {
        let (at, _) = sel.ordered();
        let caret = (at.line == line)
            .then(|| cells_px(textpos::column_of(text, at.col, TAB_WIDTH), advance));
        return Marks { tint: None, caret };
    }
    let tint = sel
        .contains_line(line, text)
        .filter(|range| !range.is_empty())
        .map(|range| span_px(text, range, advance));
    Marks { tint, caret: None }
}

/// The left edge and width in pixels of the byte `range` of `text`.
fn span_px(text: &str, range: Range<usize>, advance: f32) -> (f32, f32) {
    let start = textpos::column_of(text, range.start, TAB_WIDTH);
    let end = textpos::column_of(text, range.end, TAB_WIDTH);
    (cells_px(start, advance), cells_px(end - start, advance))
}

/// The caret and selection of the open file. The state is kept for one file
/// and one diff generation; another of either drops it.
#[derive(Default)]
pub struct CodeCaret {
    /// The caret (an empty selection) or the selected characters; `None`
    /// when no code position is active.
    sel: Option<TextSelection>,
    /// Where the selection started; Shift extends from here.
    anchor: TextPos,
    /// A press in the code is held, so the moves over the rows extend it.
    dragging: bool,
    /// The file and the diff generation the state was made for.
    scope: Option<(String, u64)>,
}

impl CodeCaret {
    /// The caret or selection to draw, if any.
    pub fn selection(&self) -> Option<TextSelection> {
        self.sel
    }

    /// True while a press in the code is held.
    pub fn is_dragging(&self) -> bool {
        self.dragging
    }

    /// Keeps the state while `path` at `generation` is the file open; any
    /// other drops it.
    pub fn scope(&mut self, path: &str, generation: u64) {
        let same = self
            .scope
            .as_ref()
            .is_some_and(|(open, seen)| open == path && *seen == generation);
        if !same {
            *self = CodeCaret {
                scope: Some((path.to_owned(), generation)),
                ..CodeCaret::default()
            };
        }
    }

    /// A press places the caret at `at` and starts a drag from it.
    pub fn place(&mut self, at: TextPos) {
        self.anchor = at;
        self.sel = Some(TextSelection::new(at, at));
        self.dragging = true;
    }

    /// A Shift press moves the head to `at`, keeping the anchor. With no
    /// selection yet it places the caret.
    pub fn extend(&mut self, at: TextPos) {
        if self.sel.is_none() {
            self.place(at);
            return;
        }
        self.sel = Some(TextSelection::new(self.anchor, at));
        self.dragging = true;
    }

    /// A double press selects the bytes `range` of `line`, a word.
    pub fn select_word(&mut self, line: usize, range: Range<usize>) {
        let start = TextPos {
            line,
            col: range.start,
        };
        self.anchor = start;
        self.sel = Some(TextSelection::new(
            start,
            TextPos {
                line,
                col: range.end,
            },
        ));
        self.dragging = false;
    }

    /// The pointer moved over `at` with the press held. Returns whether the
    /// selection changed, so the caller repaints only then.
    pub fn drag_to(&mut self, at: TextPos) -> bool {
        if !self.dragging {
            return false;
        }
        let next = Some(TextSelection::new(self.anchor, at));
        let changed = self.sel != next;
        self.sel = next;
        changed
    }

    /// The button went up.
    pub fn release(&mut self) {
        self.dragging = false;
    }

    /// Drops the caret and the selection.
    pub fn clear(&mut self) {
        self.sel = None;
        self.dragging = false;
    }

    /// The selected text of `diff`, or `None` when nothing is selected.
    pub fn copied(&self, diff: &FileDiff) -> Option<String> {
        let sel = self.sel.filter(|sel| !sel.is_empty())?;
        Some(sel.text(&lines(diff)))
    }
}

impl Shell {
    /// Keeps the caret only while its file and diff are the ones open.
    pub(crate) fn scope_code_caret(&mut self) {
        match self.selected.as_ref() {
            Some(selected) => self.code_caret.scope(&selected.path, self.diff_generation),
            None => self.code_caret = CodeCaret::default(),
        }
    }

    /// The flat index of the line at `pos` in the open file, and its text.
    fn code_line(&self, pos: Pos) -> Option<(usize, &str)> {
        let diff = self.selected.as_ref()?.diff.as_ref().ok()?;
        let line = diff.hunks.get(pos.hunk)?.lines.get(pos.line)?;
        let index = line_index(&hunk_starts(diff), pos)?;
        Some((index, line.text.as_str()))
    }

    /// The text position under window x `x` on the row at `pos`.
    fn code_point(&self, pos: Pos, column: Column, x: f32) -> Option<TextPos> {
        let (line, text) = self.code_line(pos)?;
        let list = self.diff_list.as_ref()?;
        let origin = text_origin(
            f32::from(list.viewport().left()),
            f32::from(list.viewport().size.width),
            column,
        );
        let cells = column_at(x, origin, list.scroll_x(), f32::from(self.code_advance));
        Some(TextPos {
            line,
            col: byte_at(text, cells),
        })
    }

    /// A press in the code area at `pos`: a caret, a word for a double press,
    /// or an extension with Shift. The line selection is not touched.
    pub(crate) fn press_code(
        &mut self,
        pos: Pos,
        column: Column,
        x: f32,
        shift: bool,
        clicks: usize,
        cx: &mut Context<Self>,
    ) {
        let Some(at) = self.code_point(pos, column, x) else {
            return;
        };
        if clicks >= 2 {
            let Some((_, text)) = self.code_line(pos) else {
                return;
            };
            let range = textpos::word_at(text, at.col);
            self.code_caret.select_word(at.line, range);
        } else if shift {
            self.code_caret.extend(at);
        } else {
            self.code_caret.place(at);
        }
        cx.notify();
    }

    /// The pointer moved over the row at `pos` with the button `held`. While
    /// a code press is held, the selection follows the pointer; a release
    /// ends the press.
    pub(crate) fn code_drag_over(
        &mut self,
        pos: Pos,
        column: Column,
        x: f32,
        held: bool,
        cx: &mut Context<Self>,
    ) {
        if !self.code_caret.is_dragging() {
            return;
        }
        if !held {
            self.code_caret.release();
            return;
        }
        if let Some(at) = self.code_point(pos, column, x)
            && self.code_caret.drag_to(at)
        {
            cx.notify();
        }
    }

    /// ⌘C: copies the selected text of the open file. Does nothing without a
    /// selection, so the key keeps its meaning everywhere else.
    pub(crate) fn copy_code(&self, cx: &mut Context<Self>) {
        let Some(diff) = self.selected.as_ref().and_then(|s| s.diff.as_ref().ok()) else {
            return;
        };
        if let Some(text) = self.code_caret.copied(diff) {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(line: usize, col: usize) -> TextPos {
        TextPos { line, col }
    }

    const DIFF: &str = "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,2 +1,3 @@\n a\n-b\n+cd\n@@ -9,1 +9,1 @@\n-x\n+\ty\n";

    #[test]
    fn hunk_starts_count_every_line_including_removed_ones() {
        let diff = rvw_view::diff::parse(DIFF);
        assert_eq!(hunk_starts(&diff), vec![0, 3]);
        assert_eq!(
            line_index(&hunk_starts(&diff), Pos { hunk: 1, line: 1 }),
            Some(4)
        );
        assert_eq!(
            line_index(&hunk_starts(&diff), Pos { hunk: 5, line: 0 }),
            None
        );
        assert_eq!(lines(&diff), vec!["a", "b", "cd", "x", "\ty"]);
    }

    #[test]
    fn text_starts_after_the_gutter_and_split_right_after_the_hairline() {
        assert_eq!(text_origin(100., 800., Column::Unified), 162.);
        assert_eq!(text_origin(100., 801., Column::Left), 162.);
        // Each half of 801 px less the hairline is 400 px; the right column
        // starts at 501, and its code text at 563.
        assert_eq!(text_origin(100., 801., Column::Right), 563.);
    }

    #[test]
    fn a_point_maps_to_the_nearest_cell_past_the_scroll() {
        // Text at x 162, cells 8 px: x 170 is cell 1, scrolled 16 px left it is cell 3.
        assert_eq!(column_at(170., 162., 0., 8.), 1.);
        assert_eq!(column_at(170., 162., 16., 8.), 3.);
        // Left of the text is column 0, and a zero advance never divides.
        assert_eq!(column_at(10., 162., 0., 8.), 0.);
        assert_eq!(column_at(170., 162., 0., 0.), 0.);
    }

    #[test]
    fn a_point_in_a_tab_resolves_to_its_nearer_edge() {
        // "\tx": the tab covers cells 0..4; cell 1 is nearer the start.
        assert_eq!(byte_at("\tx", 1.), 0);
        assert_eq!(byte_at("\tx", 3.), 1);
        assert_eq!(byte_at("漢a", 1.2), 3);
    }

    #[test]
    fn a_caret_is_drawn_on_its_own_line_only() {
        let caret = Some(TextSelection::new(at(2, 1), at(2, 1)));
        let here = marks(caret, Some(2), "abc", 8.);
        assert_eq!(here.caret, Some(8.));
        assert_eq!(here.tint, None);
        assert_eq!(marks(caret, Some(1), "abc", 8.), Marks::default());
        assert_eq!(marks(caret, None, "abc", 8.), Marks::default());
    }

    #[test]
    fn a_selection_tints_its_bytes_on_each_line() {
        let sel = Some(TextSelection::new(at(0, 1), at(1, 2)));
        assert_eq!(marks(sel, Some(0), "abc", 8.).tint, Some((8., 16.)));
        assert_eq!(marks(sel, Some(1), "defg", 8.).tint, Some((0., 16.)));
        assert_eq!(marks(sel, Some(2), "h", 8.).tint, None);
        assert_eq!(marks(sel, Some(0), "abc", 8.).caret, None);
    }

    #[test]
    fn an_empty_line_inside_a_selection_has_no_tint() {
        let sel = Some(TextSelection::new(at(0, 0), at(2, 1)));
        assert_eq!(marks(sel, Some(1), "", 8.).tint, None);
    }

    #[test]
    fn a_tab_in_a_tint_takes_its_cells() {
        // "a\tb": selecting "\tb" starts at cell 1 and covers the tab's
        // three cells and the b: four cells.
        let sel = Some(TextSelection::new(at(0, 1), at(0, 3)));
        assert_eq!(marks(sel, Some(0), "a\tb", 8.).tint, Some((8., 32.)));
    }

    #[test]
    fn a_press_places_and_a_drag_extends_the_caret() {
        let mut caret = CodeCaret::default();
        caret.place(at(0, 1));
        assert!(caret.is_dragging());
        assert!(caret.selection().is_some_and(|s| s.is_empty()));
        assert!(caret.drag_to(at(1, 2)));
        assert!(!caret.drag_to(at(1, 2)));
        assert_eq!(
            caret.selection(),
            Some(TextSelection::new(at(0, 1), at(1, 2)))
        );
        caret.release();
        assert!(!caret.drag_to(at(2, 0)));
        assert_eq!(
            caret.selection(),
            Some(TextSelection::new(at(0, 1), at(1, 2)))
        );
    }

    #[test]
    fn shift_extends_from_the_anchor_and_places_with_no_selection() {
        let mut caret = CodeCaret::default();
        caret.extend(at(3, 0));
        assert_eq!(
            caret.selection(),
            Some(TextSelection::new(at(3, 0), at(3, 0)))
        );
        caret.release();
        caret.extend(at(4, 2));
        assert_eq!(
            caret.selection(),
            Some(TextSelection::new(at(3, 0), at(4, 2)))
        );
    }

    #[test]
    fn a_double_press_selects_the_word_and_stops_dragging() {
        let mut caret = CodeCaret::default();
        caret.select_word(5, 4..7);
        assert_eq!(
            caret.selection(),
            Some(TextSelection::new(at(5, 4), at(5, 7)))
        );
        assert!(!caret.is_dragging());
    }

    #[test]
    fn clear_drops_the_caret_and_the_drag() {
        let mut caret = CodeCaret::default();
        caret.place(at(0, 0));
        caret.clear();
        assert_eq!(caret.selection(), None);
        assert!(!caret.is_dragging());
    }

    #[test]
    fn the_state_is_dropped_by_another_file_or_generation() {
        let mut caret = CodeCaret::default();
        caret.scope("a.rs", 1);
        caret.place(at(0, 0));
        caret.scope("a.rs", 1);
        assert!(caret.selection().is_some());
        caret.scope("a.rs", 2);
        assert_eq!(caret.selection(), None);
        caret.place(at(0, 0));
        caret.scope("b.rs", 2);
        assert_eq!(caret.selection(), None);
    }

    #[test]
    fn copy_joins_the_selected_lines_across_hunks() {
        let diff = rvw_view::diff::parse(DIFF);
        let mut caret = CodeCaret::default();
        caret.place(at(1, 0));
        caret.drag_to(at(4, 2));
        assert_eq!(caret.copied(&diff).as_deref(), Some("b\ncd\nx\n\ty"));
        caret.clear();
        assert_eq!(caret.copied(&diff), None);
    }

    #[test]
    fn a_bare_caret_copies_nothing() {
        let diff = rvw_view::diff::parse(DIFF);
        let mut caret = CodeCaret::default();
        caret.place(at(0, 0));
        caret.release();
        assert_eq!(caret.copied(&diff), None);
    }
}
