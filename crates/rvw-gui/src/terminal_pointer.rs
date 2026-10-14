//! The pure side of the terminal's pointer and search handling: pixels to
//! cells, wheel accumulation, click counts, link checks and the search bar's
//! state. No window and no terminal here, so all of it is tested directly;
//! `terminal_mouse` and `terminal_dock` apply it.

use crate::terminal_grid::cells;
use rvw_term::{GridPoint, RowSpan, SearchStatus, SelectionKind, Side, UrlMatch, MAX_MATCHES};

/// Lines one wheel notch scrolls.
pub const LINES_PER_NOTCH: f32 = 3.;
/// The most lines one wheel event moves, so a flick cannot queue a flood.
pub const MAX_EVENT_LINES: i32 = 200;

/// A cell under the pointer and which half of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hit {
    pub point: GridPoint,
    pub side: Side,
}

/// The cell at `position` of a grid whose top left is `origin`, in cells of
/// `cell` pixels, `cols` by `rows`. Outside the grid it is `None`, or with
/// `clamp` the nearest edge cell, which keeps a drag going past the edge.
pub fn cell_at(
    position: (f32, f32),
    origin: (f32, f32),
    cell: (f32, f32),
    grid: (usize, usize),
    clamp: bool,
) -> Option<Hit> {
    let (cols, rows) = grid;
    let usable = |v: f32| v.is_finite() && v > 0.;
    if cols == 0 || rows == 0 || !usable(cell.0) || !usable(cell.1) {
        return None;
    }
    let x = (position.0 - origin.0) / cell.0;
    let y = (position.1 - origin.1) / cell.1;
    if !x.is_finite() || !y.is_finite() {
        return None;
    }
    let (cols_f, rows_f) = (cells(cols), cells(rows));
    let inside = x >= 0. && y >= 0. && x < cols_f && y < rows_f;
    if !inside && !clamp {
        return None;
    }
    // Whole cells; clamped into the grid first, so the cast is in range.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // clamped to the grid
    let (col, row) = (
        x.clamp(0., cols_f - 1.) as usize,
        y.clamp(0., rows_f - 1.) as usize,
    );
    let side = if x < 0. {
        Side::Left
    } else if x >= cols_f || x.fract() >= 0.5 {
        Side::Right
    } else {
        Side::Left
    };
    Some(Hit {
        point: GridPoint { row, col },
        side,
    })
}

/// What a press with this click count starts: one click selects cell by
/// cell (a rectangle with Alt), two a word, three a line. Further clicks
/// cycle, as in other terminals.
pub fn selection_kind(click_count: usize, alt: bool) -> SelectionKind {
    match (click_count.max(1) - 1) % 3 {
        0 if alt => SelectionKind::Block,
        0 => SelectionKind::Simple,
        1 => SelectionKind::Word,
        _ => SelectionKind::Line,
    }
}

/// A press that may become a drag. A plain click selects nothing and only
/// clears; the selection starts when the pointer has moved off the cell or
/// onto its other half.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Anchor {
    pub hit: Hit,
    pub kind: SelectionKind,
}

impl Anchor {
    pub fn moved(&self, to: Hit) -> bool {
        self.hit != to
    }
}

/// Wheel and trackpad deltas turned into whole lines, keeping the
/// remainder so slow pixel scrolling still moves.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WheelAccumulator {
    lines: f32,
}

impl WheelAccumulator {
    /// A delta of `notches` (a mouse wheel's clicks). Positive is up, toward
    /// older output. Returns the whole lines to move, signed.
    pub fn push_notches(&mut self, notches: f32) -> i32 {
        self.push_lines(notches * LINES_PER_NOTCH)
    }

    /// A precise delta of `pixels` on a cell `cell_height` high.
    pub fn push_pixels(&mut self, pixels: f32, cell_height: f32) -> i32 {
        if !cell_height.is_finite() || cell_height <= 0. {
            return 0;
        }
        self.push_lines(pixels / cell_height)
    }

    fn push_lines(&mut self, lines: f32) -> i32 {
        if !lines.is_finite() {
            return 0;
        }
        self.lines += lines;
        let whole = self.lines.trunc();
        self.lines -= whole;
        // Whole lines of a bounded float; the cast saturates.
        #[allow(clippy::cast_possible_truncation)] // whole lines, clamped next
        let whole = whole as i32;
        whole.clamp(-MAX_EVENT_LINES, MAX_EVENT_LINES)
    }
}

/// Whether a URL may be handed to the platform opener: http, https and
/// file only, with nothing a command line could take as an option first.
pub fn openable(url: &str) -> bool {
    let lower = url.get(..8).unwrap_or(url).to_ascii_lowercase();
    let schemed = lower.starts_with("https://")
        || lower.starts_with("http://")
        || lower.starts_with("file://");
    schemed && !url.chars().any(|c| c.is_control() || c.is_whitespace())
}

/// The platform's opener program.
pub const fn opener() -> &'static str {
    if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    }
}

/// The cells a URL covers, one span per row, for underlining.
pub fn url_spans(url: &UrlMatch, cols: usize) -> Vec<RowSpan> {
    let last_col = cols.saturating_sub(1);
    (url.start_row..=url.end_row)
        .map(|row| RowSpan {
            row,
            start: if row == url.start_row {
                url.start_col
            } else {
                0
            },
            end: if row == url.end_row {
                url.end_col
            } else {
                last_col
            },
        })
        .collect()
}

/// What the scrolled-back note says.
pub fn scrolled_label(lines: usize) -> String {
    match lines {
        1 => "Scrolled 1 line".to_owned(),
        n => format!("Scrolled {n} lines"),
    }
}

/// The search bar's state: whether it is open and the query typed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchBar {
    open: bool,
    query: String,
}

impl SearchBar {
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    /// Opens the bar, keeping the last query so it can be stepped again.
    pub fn open(&mut self) {
        self.open = true;
    }

    /// Closes the bar and forgets the query.
    pub fn close(&mut self) {
        self.open = false;
        self.query.clear();
    }

    /// Adds typed text; a line break ends the line, so only its first line
    /// is taken. Returns whether the query changed.
    pub fn insert(&mut self, text: &str) -> bool {
        let line = text.split(['\n', '\r']).next().unwrap_or("");
        let line: String = line.chars().filter(|c| !c.is_control()).collect();
        if line.is_empty() || self.query.len() + line.len() > rvw_term::MAX_QUERY {
            return false;
        }
        self.query.push_str(&line);
        true
    }

    /// Removes the last character. Returns whether the query changed.
    pub fn backspace(&mut self) -> bool {
        self.query.pop().is_some()
    }

    /// What the bar says about the matches.
    pub fn label(&self, status: Option<&SearchStatus>) -> String {
        if self.query.is_empty() {
            return String::new();
        }
        let Some(status) = status else {
            return "No matches".to_owned();
        };
        if status.count == 0 {
            return "No matches".to_owned();
        }
        let total = if status.capped || status.count >= MAX_MATCHES {
            format!("{MAX_MATCHES}+")
        } else {
            status.count.to_string()
        };
        match status.current {
            Some(index) => format!("{}/{total}", index + 1),
            None => total,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CELL: (f32, f32) = (8., 16.);

    fn hit(row: usize, col: usize, side: Side) -> Hit {
        Hit {
            point: GridPoint { row, col },
            side,
        }
    }

    #[test]
    fn pixels_map_to_cells_and_halves() {
        let at = |x, y| cell_at((x, y), (10., 20.), CELL, (80, 24), false);
        assert_eq!(at(10., 20.), Some(hit(0, 0, Side::Left)));
        assert_eq!(at(17.9, 35.9), Some(hit(0, 0, Side::Right)));
        assert_eq!(
            at(10. + 8. * 5. + 1., 20. + 16. * 3.),
            Some(hit(3, 5, Side::Left))
        );
        assert_eq!(at(9., 20.), None);
        assert_eq!(at(10., 19.), None);
        assert_eq!(at(10. + 8. * 80., 20.), None);
        assert_eq!(at(10., 20. + 16. * 24.), None);
    }

    #[test]
    fn a_drag_past_the_edge_clamps_to_the_nearest_cell() {
        let at = |x, y| cell_at((x, y), (0., 0.), CELL, (10, 5), true);
        assert_eq!(at(-30., -30.), Some(hit(0, 0, Side::Left)));
        assert_eq!(at(500., 500.), Some(hit(4, 9, Side::Right)));
        assert_eq!(at(500., 20.), Some(hit(1, 9, Side::Right)));
    }

    #[test]
    fn an_unusable_measure_gives_no_cell() {
        assert_eq!(cell_at((1., 1.), (0., 0.), (0., 16.), (10, 5), true), None);
        assert_eq!(cell_at((1., 1.), (0., 0.), CELL, (0, 5), true), None);
        assert_eq!(cell_at((f32::NAN, 1.), (0., 0.), CELL, (10, 5), true), None);
    }

    #[test]
    fn click_counts_pick_the_selection() {
        assert_eq!(selection_kind(1, false), SelectionKind::Simple);
        assert_eq!(selection_kind(1, true), SelectionKind::Block);
        assert_eq!(selection_kind(2, false), SelectionKind::Word);
        assert_eq!(selection_kind(3, false), SelectionKind::Line);
        assert_eq!(selection_kind(4, false), SelectionKind::Simple);
        assert_eq!(selection_kind(0, false), SelectionKind::Simple);
        // Alt only makes the first click a block.
        assert_eq!(selection_kind(2, true), SelectionKind::Word);
    }

    #[test]
    fn a_press_becomes_a_drag_once_it_leaves_its_half_cell() {
        let anchor = Anchor {
            hit: hit(2, 3, Side::Left),
            kind: SelectionKind::Simple,
        };
        assert!(!anchor.moved(hit(2, 3, Side::Left)));
        assert!(anchor.moved(hit(2, 3, Side::Right)));
        assert!(anchor.moved(hit(2, 4, Side::Left)));
    }

    #[test]
    fn a_notch_is_three_lines_and_pixels_accumulate() {
        let mut wheel = WheelAccumulator::default();
        assert_eq!(wheel.push_notches(1.), 3);
        assert_eq!(wheel.push_notches(-2.), -6);
        // Nine pixels on 18 pixel lines: nothing, then a line.
        assert_eq!(wheel.push_pixels(9., 18.), 0);
        assert_eq!(wheel.push_pixels(9., 18.), 1);
        // The remainder carries across events and directions.
        assert_eq!(wheel.push_pixels(27., 18.), 1);
        assert_eq!(wheel.push_pixels(-9., 18.), 0);
        assert_eq!(wheel.push_pixels(-18., 18.), -1);
    }

    #[test]
    fn a_wheel_event_is_bounded_and_bad_input_moves_nothing() {
        let mut wheel = WheelAccumulator::default();
        assert_eq!(wheel.push_pixels(1e9, 18.), MAX_EVENT_LINES);
        assert_eq!(wheel.push_pixels(f32::NAN, 18.), 0);
        assert_eq!(wheel.push_pixels(10., 0.), 0);
        assert_eq!(wheel.push_notches(f32::INFINITY), 0);
    }

    #[test]
    fn only_http_https_and_file_open() {
        assert!(openable("https://example.com/a?b=c"));
        assert!(openable("HTTP://example.com"));
        assert!(openable("file:///tmp/a.txt"));
        assert!(!openable("javascript:alert(1)"));
        assert!(!openable("ftp://example.com"));
        assert!(!openable("-https://example.com"));
        assert!(!openable("https://a b"));
        assert!(!openable("https://a\u{7}b"));
        assert!(!openable(""));
    }

    #[test]
    fn a_wrapped_url_underlines_every_row() {
        let url = UrlMatch {
            url: String::new(),
            start_row: 1,
            start_col: 6,
            end_row: 3,
            end_col: 4,
        };
        let spans = url_spans(&url, 20);
        assert_eq!(
            spans,
            [
                RowSpan {
                    row: 1,
                    start: 6,
                    end: 19
                },
                RowSpan {
                    row: 2,
                    start: 0,
                    end: 19
                },
                RowSpan {
                    row: 3,
                    start: 0,
                    end: 4
                },
            ]
        );
    }

    fn status(count: usize, current: Option<usize>, capped: bool) -> SearchStatus {
        SearchStatus {
            query: "x".to_owned(),
            count,
            capped,
            current,
        }
    }

    #[test]
    fn the_search_bar_edits_a_one_line_query() {
        let mut bar = SearchBar::default();
        assert!(!bar.is_open());
        bar.open();
        assert!(bar.insert("fo"));
        assert!(bar.insert("o\nbar"));
        assert_eq!(bar.query(), "foo");
        assert!(!bar.insert("\n"));
        assert!(!bar.insert("\u{7}"));
        assert!(bar.backspace());
        assert_eq!(bar.query(), "fo");
        bar.close();
        assert!(!bar.is_open());
        assert_eq!(bar.query(), "");
        assert!(!bar.backspace());
    }

    #[test]
    fn the_search_bar_bounds_the_query() {
        let mut bar = SearchBar::default();
        assert!(bar.insert(&"a".repeat(rvw_term::MAX_QUERY)));
        assert!(!bar.insert("a"));
    }

    #[test]
    fn the_scrolled_note_counts_lines() {
        assert_eq!(scrolled_label(1), "Scrolled 1 line");
        assert_eq!(scrolled_label(12), "Scrolled 12 lines");
    }

    #[test]
    fn the_search_label_counts_and_caps() {
        let mut bar = SearchBar::default();
        assert_eq!(bar.label(None), "");
        bar.insert("x");
        assert_eq!(bar.label(None), "No matches");
        assert_eq!(bar.label(Some(&status(0, None, false))), "No matches");
        assert_eq!(bar.label(Some(&status(42, Some(2), false))), "3/42");
        assert_eq!(bar.label(Some(&status(42, None, false))), "42");
        assert_eq!(bar.label(Some(&status(10_000, None, true))), "10000+");
        assert_eq!(bar.label(Some(&status(10_000, Some(0), true))), "1/10000+");
    }
}
