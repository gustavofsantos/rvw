//! Plain-text, case-smart search over scrollback and screen.

use std::ops::RangeInclusive;

use alacritty_terminal::Term;
use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Boundary, Column, Direction, Line, Point, Side};
use alacritty_terminal::term::search::{Match, RegexIter, RegexSearch};

use crate::snapshot::{MatchSpan, SearchStatus};

/// The most matches counted, and the most reported for one viewport.
pub const MAX_MATCHES: usize = 10_000;
/// The longest query accepted, in bytes.
pub const MAX_QUERY: usize = 1024;

pub(crate) struct Search {
    query: String,
    regex: RegexSearch,
    /// The focused match, in grid coordinates (negative lines are history).
    current: Option<Match>,
    /// History size when `current` was last known right.
    history_at: usize,
    count: usize,
    capped: bool,
    index: Option<usize>,
}

fn escape(query: &str) -> String {
    let mut out = String::with_capacity(query.len() * 2);
    for c in query.chars() {
        if "\\.+*?()|[]{}^$#&-~".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn line_i32(n: usize) -> i32 {
    i32::try_from(n).unwrap_or(i32::MAX)
}

impl Search {
    /// `None` for an empty, oversized or multi-line query.
    pub(crate) fn new(query: &str) -> Option<Self> {
        if query.is_empty() || query.len() > MAX_QUERY || query.contains(['\n', '\r']) {
            return None;
        }
        let regex = RegexSearch::new(&escape(query)).ok()?;
        Some(Self {
            query: query.to_owned(),
            regex,
            current: None,
            history_at: 0,
            count: 0,
            capped: false,
            index: None,
        })
    }

    pub(crate) fn status(&self) -> SearchStatus {
        SearchStatus {
            query: self.query.clone(),
            count: self.count,
            capped: self.capped,
            current: self.index,
        }
    }

    /// Keep the focused match while it still is where it was. History growth
    /// shifts it; once history is full the content moves under it and the
    /// focus is dropped.
    fn sync<L>(&mut self, term: &Term<L>) {
        let history = term.grid().history_size();
        if let Some(cur) = self.current.take() {
            let grown = line_i32(history.saturating_sub(self.history_at));
            let shift = |p: Point| Point::new(p.line - grown, p.column);
            let (start, end) = (shift(*cur.start()), shift(*cur.end()));
            let in_grid = start.line >= term.topmost_line() && end.line <= term.bottommost_line();
            if in_grid && term.regex_search_right(&mut self.regex, start, end) == Some(start..=end)
            {
                self.current = Some(start..=end);
            }
        }
        self.history_at = history;
    }

    /// Recount over the whole grid, up to [`MAX_MATCHES`].
    pub(crate) fn recount<L>(&mut self, term: &Term<L>) {
        self.sync(term);
        let top = Point::new(term.topmost_line(), Column(0));
        let bottom = Point::new(term.bottommost_line(), term.last_column());
        let focus = self.current.as_ref().map(|m| *m.start());
        let (mut count, mut capped, mut index) = (0, false, None);
        for found in RegexIter::new(top, bottom, Direction::Right, term, &mut self.regex) {
            if count == MAX_MATCHES {
                capped = true;
                break;
            }
            if Some(*found.start()) == focus {
                index = Some(count);
            }
            count += 1;
        }
        self.count = count;
        self.capped = capped;
        self.index = index;
    }

    /// Focus the next or previous match, wrapping around, and scroll it into
    /// view. False when there is none.
    pub(crate) fn step<L: EventListener>(&mut self, term: &mut Term<L>, forward: bool) -> bool {
        self.sync(term);
        let offset = line_i32(term.grid().display_offset());
        let (origin, direction, side) = match (&self.current, forward) {
            (Some(m), true) => (
                m.end().add(term, Boundary::None, 1),
                Direction::Right,
                Side::Left,
            ),
            (Some(m), false) => (
                m.start().sub(term, Boundary::None, 1),
                Direction::Left,
                Side::Right,
            ),
            (None, true) => (
                Point::new(Line(-offset), Column(0)),
                Direction::Right,
                Side::Left,
            ),
            (None, false) => (
                Point::new(
                    Line(line_i32(term.screen_lines()) - 1 - offset),
                    term.last_column(),
                ),
                Direction::Left,
                Side::Right,
            ),
        };
        let Some(found) = term.search_next(&mut self.regex, origin, direction, side, None) else {
            return false;
        };
        let start = *found.start();
        self.current = Some(found);
        self.history_at = term.grid().history_size();
        self.recount(term);
        term.scroll_to_point(start);
        true
    }

    /// The matches in the viewport, one span per row they cross.
    pub(crate) fn visible<L>(&mut self, term: &Term<L>) -> Vec<MatchSpan> {
        self.sync(term);
        let rows = line_i32(term.screen_lines());
        let offset = line_i32(term.grid().display_offset());
        let (top, bottom) = (-offset, rows - 1 - offset);
        let from = Point::new(Line(top), Column(0));
        let to = Point::new(Line(bottom), term.last_column());
        let focus = self.current.as_ref().map(RangeInclusive::start).copied();
        let last = term.last_column().0;
        let mut out = Vec::new();
        for (n, found) in
            RegexIter::new(from, to, Direction::Right, term, &mut self.regex).enumerate()
        {
            if n >= MAX_MATCHES {
                break;
            }
            let (s, e) = (*found.start(), *found.end());
            let current = Some(s) == focus;
            for line in s.line.0.max(top)..=e.line.0.min(bottom) {
                out.push(MatchSpan {
                    row: usize::try_from(line + offset).unwrap_or(0),
                    start: if line == s.line.0 { s.column.0 } else { 0 },
                    end: if line == e.line.0 { e.column.0 } else { last },
                    current,
                });
            }
        }
        out
    }
}
