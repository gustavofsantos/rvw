//! From a terminal snapshot to what is painted: background spans and text
//! runs, and the size math between pixels and cells. Pure, so it is tested
//! without a window; `terminal_dock` shapes and paints what this returns.

use rvw_term::{CellFlags, Color, CursorKind, Snapshot};

/// Smallest grid a terminal is resized to. Programs misbehave below this.
const MIN_COLS: u16 = 2;
const MIN_ROWS: u16 = 1;

/// How a run's glyphs sit on the grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Width {
    /// Every glyph is one cell wide, so a run of n characters is n cells.
    Cells,
    /// One double-width character, two cells.
    Wide,
    /// A cell with combining marks: shaped as it is, one cell of room.
    Natural,
}

/// A count of cells as a number to multiply a cell's size by. A grid is never
/// wider than a PTY can express, so the conversion is exact.
pub fn cells(count: usize) -> f32 {
    f32::from(u16::try_from(count).unwrap_or(u16::MAX))
}

/// The attributes a run is painted with. Inverse is already applied: `fg`
/// and `bg` are what is seen.
// Independent text attributes, as the terminal reports them.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Style {
    pub fg: Color,
    pub bg: Color,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikeout: bool,
    pub dim: bool,
}

/// Text at one place on the grid, in one style.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    pub row: usize,
    pub col: usize,
    /// Cells the run covers.
    pub cols: usize,
    pub text: String,
    pub style: Style,
    pub width: Width,
}

/// A background that is not the terminal's default, over `cols` cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fill {
    pub row: usize,
    pub col: usize,
    pub cols: usize,
    pub color: Color,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Layout {
    pub fills: Vec<Fill>,
    pub runs: Vec<Run>,
}

/// The cell the block cursor is drawn on, as an inverse of that cell.
fn block_cursor(snapshot: &Snapshot, focused: bool) -> Option<(usize, usize)> {
    let cursor = snapshot.cursor;
    let drawn = focused && cursor.visible && cursor.kind == CursorKind::Block;
    (drawn && cursor.row < snapshot.rows && cursor.col < snapshot.cols)
        .then_some((cursor.row, cursor.col))
}

fn style_of(cell: &rvw_term::Cell, inverted: bool) -> Style {
    let flags = cell.flags;
    let swap = flags.contains(CellFlags::INVERSE) != inverted;
    let (fg, bg) = if swap {
        (cell.bg, cell.fg)
    } else {
        (cell.fg, cell.bg)
    };
    Style {
        fg,
        bg,
        bold: flags.contains(CellFlags::BOLD),
        italic: flags.contains(CellFlags::ITALIC),
        underline: flags.contains(CellFlags::UNDERLINE),
        strikeout: flags.contains(CellFlags::STRIKEOUT),
        dim: flags.contains(CellFlags::DIM),
    }
}

/// Whether a space in `style` shows anything of its own besides the
/// background, which a `Fill` carries.
fn space_is_visible(style: &Style) -> bool {
    style.underline || style.strikeout
}

/// Groups the cells of `snapshot` into runs of one style and background
/// spans. `focused` decides whether the block cursor is drawn as an inverse
/// cell; the other cursor shapes are drawn by the painter.
pub fn layout(snapshot: &Snapshot, focused: bool) -> Layout {
    let cursor = block_cursor(snapshot, focused);
    let mut out = Layout::default();
    for row in 0..snapshot.rows {
        row_layout(snapshot, row, cursor, &mut out);
    }
    out
}

fn row_layout(snapshot: &Snapshot, row: usize, cursor: Option<(usize, usize)>, out: &mut Layout) {
    let mut run: Option<Run> = None;
    let mut fill: Option<Fill> = None;
    for (col, cell) in snapshot.row(row).iter().enumerate() {
        if cell.flags.contains(CellFlags::WIDE_SPACER) {
            // Covered by the wide character before it; its background is
            // that character's.
            continue;
        }
        let style = style_of(cell, cursor == Some((row, col)));
        let wide = cell.flags.contains(CellFlags::WIDE);
        let cols = if wide { 2 } else { 1 };

        match fill.as_mut() {
            Some(open) if style.bg != Color::Background && open.color == style.bg => {
                open.cols += cols;
            }
            _ => {
                out.fills.extend(fill.take());
                if style.bg != Color::Background {
                    fill = Some(Fill {
                        row,
                        col,
                        cols,
                        color: style.bg,
                    });
                }
            }
        }

        let hidden = cell.flags.contains(CellFlags::HIDDEN);
        let ch = if hidden { ' ' } else { cell.ch };
        let blank = ch == ' ' || ch == '\0';
        if blank && !wide && !space_is_visible(&style) {
            // Nothing to draw. A blank between words of one style stays in
            // the run (trailing ones are trimmed); otherwise the run ends.
            match run.as_mut() {
                Some(open) if open.style == style && open.col + open.cols == col => {
                    open.text.push(' ');
                    open.cols += 1;
                }
                _ => out.runs.extend(run.take().map(trimmed).filter(has_text)),
            }
            continue;
        }
        let ch = if blank { ' ' } else { ch };
        let marks = if hidden { &[][..] } else { &cell.zerowidth[..] };
        if wide || !marks.is_empty() {
            out.runs.extend(run.take().map(trimmed).filter(has_text));
            let mut text = String::new();
            text.push(ch);
            text.extend(marks);
            out.runs.push(Run {
                row,
                col,
                cols,
                text,
                style,
                width: if wide { Width::Wide } else { Width::Natural },
            });
            continue;
        }
        match run.as_mut() {
            Some(open) if open.style == style && open.col + open.cols == col => {
                open.text.push(ch);
                open.cols += 1;
            }
            _ => {
                out.runs.extend(run.take().map(trimmed).filter(has_text));
                run = Some(Run {
                    row,
                    col,
                    cols: 1,
                    text: ch.to_string(),
                    style,
                    width: Width::Cells,
                });
            }
        }
    }
    out.fills.extend(fill);
    out.runs.extend(run.map(trimmed).filter(has_text));
}

/// A run without the spaces that end it, unless they are underlined or
/// struck, which are visible.
fn trimmed(mut run: Run) -> Run {
    if run.width == Width::Cells && !space_is_visible(&run.style) {
        let kept = run.text.trim_end_matches(' ').chars().count();
        run.text.truncate(run.text.trim_end_matches(' ').len());
        run.cols = kept;
    }
    run
}

fn has_text(run: &Run) -> bool {
    !run.text.is_empty()
}

/// The 256-colour palette's colours 16 to 255 as RGB: the 6x6x6 cube, then
/// the 24 greys. Indices below 16 are the theme's, and return `None`.
pub fn palette_rgb(index: u8) -> Option<(u8, u8, u8)> {
    const STEPS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    match index {
        0..=15 => None,
        16..=231 => {
            let n = usize::from(index - 16);
            Some((STEPS[n / 36], STEPS[(n / 6) % 6], STEPS[n % 6]))
        }
        232..=255 => {
            let level = 8 + 10 * (index - 232);
            Some((level, level, level))
        }
    }
}

/// The grid that fits `width` by `height` pixels, in cells of `cell_width`
/// by `cell_height`. `None` when a measure is not usable, which keeps the
/// terminal at its size. Never below 2 columns by 1 row, never above what a
/// PTY can express.
pub fn grid_size(width: f32, height: f32, cell_width: f32, cell_height: f32) -> Option<(u16, u16)> {
    let usable = |value: f32| value.is_finite() && value > 0.;
    if !(usable(width) && usable(height) && usable(cell_width) && usable(cell_height)) {
        return None;
    }
    // Truncation is the point: whole cells only. Casts saturate.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // whole cells, positive
    let (cols, rows) = (
        (width / cell_width).floor() as u16,
        (height / cell_height).floor() as u16,
    );
    Some((cols.max(MIN_COLS), rows.max(MIN_ROWS)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rvw_term::{Cell, Cursor, Mode};

    fn cell(ch: char) -> Cell {
        Cell {
            ch,
            zerowidth: Vec::new(),
            fg: Color::Foreground,
            bg: Color::Background,
            flags: CellFlags::default(),
        }
    }

    fn snapshot(rows: Vec<Vec<Cell>>) -> Snapshot {
        let cols = rows[0].len();
        Snapshot {
            rows: rows.len(),
            cols,
            cells: rows.into_iter().flatten().collect(),
            cursor: Cursor {
                row: 0,
                col: 0,
                kind: CursorKind::Underline,
                visible: true,
                blinking: false,
            },
            mode: Mode::default(),
            title: None,
            bell: false,
            history_lines: 0,
            exit: None,
        }
    }

    fn text_row(text: &str) -> Vec<Cell> {
        text.chars().map(cell).collect()
    }

    fn flagged(ch: char, flags: CellFlags) -> Cell {
        Cell { flags, ..cell(ch) }
    }

    fn texts(layout: &Layout) -> Vec<(usize, usize, &str)> {
        layout
            .runs
            .iter()
            .map(|run| (run.row, run.col, run.text.as_str()))
            .collect()
    }

    #[test]
    fn a_row_of_one_style_is_one_run_without_trailing_blanks() {
        let shot = snapshot(vec![text_row("ls -la    ")]);
        let layout = layout(&shot, false);
        assert_eq!(texts(&layout), [(0, 0, "ls -la")]);
        assert_eq!(layout.runs[0].cols, 6);
        assert!(layout.fills.is_empty());
    }

    #[test]
    fn a_change_of_style_splits_the_run_and_blanks_between_words_stay() {
        let mut row = text_row("ab cd");
        row[3] = flagged('c', CellFlags::BOLD);
        row[4] = flagged('d', CellFlags::BOLD);
        let layout = layout(&snapshot(vec![row]), false);
        assert_eq!(texts(&layout), [(0, 0, "ab"), (0, 3, "cd")]);
        assert!(layout.runs[1].style.bold);
        assert!(!layout.runs[0].style.bold);
    }

    #[test]
    fn adjacent_cells_of_one_background_make_one_fill() {
        let mut row = text_row("abcd");
        for cell in &mut row[1..3] {
            cell.bg = Color::Indexed(4);
        }
        let layout = layout(&snapshot(vec![row]), false);
        assert_eq!(
            layout.fills,
            [Fill {
                row: 0,
                col: 1,
                cols: 2,
                color: Color::Indexed(4)
            }]
        );
        // The background changes the style, so the text is three runs.
        assert_eq!(texts(&layout), [(0, 0, "a"), (0, 1, "bc"), (0, 3, "d")]);
    }

    #[test]
    fn inverse_swaps_the_colours_seen() {
        let shot = snapshot(vec![vec![flagged('x', CellFlags::INVERSE)]]);
        let layout = layout(&shot, false);
        assert_eq!(layout.runs[0].style.fg, Color::Background);
        assert_eq!(layout.runs[0].style.bg, Color::Foreground);
        assert_eq!(layout.fills[0].color, Color::Foreground);
    }

    #[test]
    fn a_wide_character_is_its_own_run_two_cells_wide() {
        let mut row = text_row("a  b");
        row[1] = flagged('\u{4e2d}', CellFlags::WIDE);
        row[2] = flagged(' ', CellFlags::WIDE_SPACER);
        let layout = layout(&snapshot(vec![row]), false);
        assert_eq!(
            texts(&layout),
            [(0, 0, "a"), (0, 1, "\u{4e2d}"), (0, 3, "b")]
        );
        assert_eq!(layout.runs[1].width, Width::Wide);
        assert_eq!(layout.runs[1].cols, 2);
    }

    #[test]
    fn a_cell_with_a_combining_mark_is_shaped_as_it_is() {
        let mut row = text_row("ae");
        row[1].zerowidth = vec!['\u{301}'];
        let layout = layout(&snapshot(vec![row]), false);
        assert_eq!(texts(&layout), [(0, 0, "a"), (0, 1, "e\u{301}")]);
        assert_eq!(layout.runs[1].width, Width::Natural);
        assert_eq!(layout.runs[1].cols, 1);
    }

    #[test]
    fn underlined_blanks_are_kept_and_hidden_cells_are_blank() {
        let mut row = vec![flagged(' ', CellFlags::UNDERLINE)];
        row.push(flagged('s', CellFlags::HIDDEN));
        let layout = layout(&snapshot(vec![row]), false);
        // The hidden cell is a blank with no mark of its own: nothing drawn.
        assert_eq!(texts(&layout), [(0, 0, " ")]);
    }

    #[test]
    fn the_block_cursor_inverts_its_cell_only_when_focused() {
        let mut shot = snapshot(vec![text_row("ab")]);
        shot.cursor.kind = CursorKind::Block;
        shot.cursor.col = 1;
        let focused = layout(&shot, true);
        assert_eq!(focused.fills.len(), 1);
        assert_eq!(focused.fills[0].col, 1);
        assert_eq!(focused.runs[1].style.bg, Color::Foreground);
        let unfocused = layout(&shot, false);
        assert!(unfocused.fills.is_empty());
        shot.cursor.visible = false;
        assert!(layout(&shot, true).fills.is_empty());
    }

    #[test]
    fn the_grid_fits_whole_cells() {
        assert_eq!(grid_size(800., 400., 8., 16.), Some((100, 25)));
        assert_eq!(grid_size(803., 415., 8., 16.), Some((100, 25)));
        assert_eq!(grid_size(5., 5., 8., 16.), Some((2, 1)));
        assert_eq!(grid_size(1e9, 1e9, 1., 1.), Some((u16::MAX, u16::MAX)));
    }

    #[test]
    fn an_unusable_measure_gives_no_grid() {
        assert_eq!(grid_size(0., 400., 8., 16.), None);
        assert_eq!(grid_size(800., 400., 0., 16.), None);
        assert_eq!(grid_size(f32::NAN, 400., 8., 16.), None);
        assert_eq!(grid_size(800., -1., 8., 16.), None);
    }

    #[test]
    fn the_palette_has_a_cube_and_a_grey_ramp() {
        assert_eq!(palette_rgb(15), None);
        assert_eq!(palette_rgb(16), Some((0, 0, 0)));
        assert_eq!(palette_rgb(196), Some((255, 0, 0)));
        assert_eq!(palette_rgb(231), Some((255, 255, 255)));
        assert_eq!(palette_rgb(232), Some((8, 8, 8)));
        assert_eq!(palette_rgb(255), Some((238, 238, 238)));
    }
}
