//! Selection by grid point. Points are viewport coordinates at the time of
//! the call; the emulation anchors them to the content, so a selection
//! follows output that scrolls it and is cleared when it scrolls out.

use alacritty_terminal::Term;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point, Side as TermSide};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags;

use crate::snapshot::RowSpan;

/// A cell of the viewport, row 0 being the top visible row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridPoint {
    pub row: usize,
    pub col: usize,
}

/// Which half of the cell the pointer is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

/// How a selection grows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionKind {
    /// Cell by cell.
    Simple,
    /// Whole words (double click).
    Word,
    /// Whole lines (triple click).
    Line,
    /// A rectangle.
    Block,
}

fn point<L>(term: &Term<L>, at: GridPoint) -> Point {
    let rows = term.screen_lines().max(1);
    let cols = term.columns().max(1);
    let offset = i32::try_from(term.grid().display_offset()).unwrap_or(i32::MAX);
    let row = i32::try_from(at.row.min(rows - 1)).unwrap_or(0);
    Point::new(Line(row - offset), Column(at.col.min(cols - 1)))
}

fn side(side: Side) -> TermSide {
    match side {
        Side::Left => TermSide::Left,
        Side::Right => TermSide::Right,
    }
}

pub(crate) fn start<L>(term: &mut Term<L>, kind: SelectionKind, at: GridPoint, from: Side) {
    let ty = match kind {
        SelectionKind::Simple => SelectionType::Simple,
        SelectionKind::Word => SelectionType::Semantic,
        SelectionKind::Line => SelectionType::Lines,
        SelectionKind::Block => SelectionType::Block,
    };
    term.selection = Some(Selection::new(ty, point(term, at), side(from)));
}

pub(crate) fn update<L>(term: &mut Term<L>, at: GridPoint, to: Side) {
    let target = point(term, at);
    if let Some(selection) = term.selection.as_mut() {
        selection.update(target, side(to));
    }
}

pub(crate) fn text<L>(term: &Term<L>) -> Option<String> {
    term.selection_to_string().filter(|text| !text.is_empty())
}

/// The selected cells that are in the viewport, one span per row.
pub(crate) fn spans<L>(term: &Term<L>) -> Vec<RowSpan> {
    let Some(range) = term.selection.as_ref().and_then(|s| s.to_range(term)) else {
        return Vec::new();
    };
    let rows = term.screen_lines();
    let cols = term.columns();
    let offset = i32::try_from(term.grid().display_offset()).unwrap_or(i32::MAX);
    let (first, last) = (range.start, range.end);
    let mut out = Vec::new();
    for row in 0..rows {
        let line = i32::try_from(row).unwrap_or(i32::MAX) - offset;
        if line < first.line.0 || line > last.line.0 {
            continue;
        }
        let (mut a, mut b) = if range.is_block {
            (
                first.column.0.min(last.column.0),
                first.column.0.max(last.column.0),
            )
        } else {
            (
                if line == first.line.0 {
                    first.column.0
                } else {
                    0
                },
                if line == last.line.0 {
                    last.column.0
                } else {
                    cols - 1
                },
            )
        };
        a = a.min(cols - 1);
        b = b.min(cols - 1);
        // A wide character is selected with its spacer.
        if b + 1 < cols
            && term.grid()[Line(line)][Column(b)]
                .flags
                .contains(Flags::WIDE_CHAR)
        {
            b += 1;
        }
        out.push(RowSpan {
            row,
            start: a,
            end: b,
        });
    }
    out
}
