//! An owned copy of what a terminal shows. The GUI paints from this and never
//! holds the terminal's lock.

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell as TermCell, Flags};
use alacritty_terminal::term::{Term, TermMode};
use alacritty_terminal::vte::ansi::{Color as TermColor, CursorShape, NamedColor};

use crate::terminal::Exit;

/// A color as the program asked for it. Resolving it to RGB, and what the
/// default colors are, is the painter's job.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Color {
    /// The theme's default foreground.
    Foreground,
    /// The theme's default background.
    Background,
    /// The 256-color palette: 0 to 7 normal, 8 to 15 bright, then the cube
    /// and the grey ramp.
    Indexed(u8),
    Rgb(u8, u8, u8),
}

/// Text attributes of one cell.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CellFlags(u16);

impl CellFlags {
    pub const BOLD: Self = Self(1);
    pub const ITALIC: Self = Self(1 << 1);
    pub const UNDERLINE: Self = Self(1 << 2);
    pub const INVERSE: Self = Self(1 << 3);
    pub const DIM: Self = Self(1 << 4);
    pub const HIDDEN: Self = Self(1 << 5);
    pub const STRIKEOUT: Self = Self(1 << 6);
    /// The cell holds a double-width character; the next cell is its spacer.
    pub const WIDE: Self = Self(1 << 7);
    /// The cell is the right half of a double-width character: paint nothing.
    pub const WIDE_SPACER: Self = Self(1 << 8);

    #[must_use]
    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    fn from_term(flags: Flags) -> Self {
        const MAP: [(Flags, CellFlags); 9] = [
            (Flags::BOLD, CellFlags::BOLD),
            (Flags::ITALIC, CellFlags::ITALIC),
            (Flags::ALL_UNDERLINES, CellFlags::UNDERLINE),
            (Flags::INVERSE, CellFlags::INVERSE),
            (Flags::DIM, CellFlags::DIM),
            (Flags::HIDDEN, CellFlags::HIDDEN),
            (Flags::STRIKEOUT, CellFlags::STRIKEOUT),
            (Flags::WIDE_CHAR, CellFlags::WIDE),
            (Flags::WIDE_CHAR_SPACER, CellFlags::WIDE_SPACER),
        ];
        let mut out = 0;
        for (from, to) in MAP {
            if flags.intersects(from) {
                out |= to.0;
            }
        }
        Self(out)
    }
}

/// One character cell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    /// Combining characters that follow `ch`; empty for nearly every cell.
    pub zerowidth: Vec<char>,
    pub fg: Color,
    pub bg: Color,
    pub flags: CellFlags,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorKind {
    Block,
    Underline,
    Beam,
    HollowBlock,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cursor {
    pub row: usize,
    pub col: usize,
    pub kind: CursorKind,
    /// False when the program hid the cursor.
    pub visible: bool,
    pub blinking: bool,
}

/// The terminal modes a view or an input encoder needs.
// Independent switches the program flips, not states of one machine.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mode {
    /// Arrows send `ESC O x`; pass to `encode_key`.
    pub app_cursor: bool,
    /// Pastes are wrapped; pass to `encode_paste`.
    pub bracketed_paste: bool,
    /// The alternate screen is up (a full-screen program).
    pub alt_screen: bool,
    /// The program wants focus in and out reports.
    pub focus_reporting: bool,
    /// Some mouse reporting is on (click, drag or motion).
    pub mouse_reporting: bool,
    /// Mouse reports use the SGR encoding.
    pub sgr_mouse: bool,
    /// Wheel events become arrow keys on the alternate screen.
    pub alternate_scroll: bool,
}

/// What a terminal shows at one moment. Row-major, `rows * cols` cells.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub rows: usize,
    pub cols: usize,
    pub cells: Vec<Cell>,
    pub cursor: Cursor,
    pub mode: Mode,
    /// The title the program set, if any.
    pub title: Option<String>,
    /// The bell rang since the previous snapshot.
    pub bell: bool,
    /// Lines of history above the visible rows.
    pub history_lines: usize,
    /// Set once the child has exited.
    pub exit: Option<Exit>,
}

impl Snapshot {
    /// The cells of one row.
    ///
    /// # Panics
    ///
    /// When `row >= rows`.
    #[must_use]
    pub fn row(&self, row: usize) -> &[Cell] {
        &self.cells[row * self.cols..(row + 1) * self.cols]
    }

    /// One row's text, trailing blanks removed. Wide-character spacers are
    /// skipped.
    #[must_use]
    pub fn row_text(&self, row: usize) -> String {
        let mut text = String::new();
        for cell in self.row(row) {
            if cell.flags.contains(CellFlags::WIDE_SPACER) {
                continue;
            }
            text.push(cell.ch);
            text.extend(&cell.zerowidth);
        }
        text.truncate(text.trim_end().len());
        text
    }

    /// All rows' text joined by newlines, trailing blank rows removed.
    #[must_use]
    pub fn text(&self) -> String {
        let mut lines: Vec<String> = (0..self.rows).map(|row| self.row_text(row)).collect();
        while lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        lines.join("\n")
    }
}

fn convert_color(color: TermColor) -> Color {
    match color {
        TermColor::Spec(rgb) => Color::Rgb(rgb.r, rgb.g, rgb.b),
        TermColor::Indexed(index) => Color::Indexed(index),
        TermColor::Named(named) => match named {
            NamedColor::Black | NamedColor::DimBlack => Color::Indexed(0),
            NamedColor::Red | NamedColor::DimRed => Color::Indexed(1),
            NamedColor::Green | NamedColor::DimGreen => Color::Indexed(2),
            NamedColor::Yellow | NamedColor::DimYellow => Color::Indexed(3),
            NamedColor::Blue | NamedColor::DimBlue => Color::Indexed(4),
            NamedColor::Magenta | NamedColor::DimMagenta => Color::Indexed(5),
            NamedColor::Cyan | NamedColor::DimCyan => Color::Indexed(6),
            NamedColor::White | NamedColor::DimWhite => Color::Indexed(7),
            NamedColor::BrightBlack => Color::Indexed(8),
            NamedColor::BrightRed => Color::Indexed(9),
            NamedColor::BrightGreen => Color::Indexed(10),
            NamedColor::BrightYellow => Color::Indexed(11),
            NamedColor::BrightBlue => Color::Indexed(12),
            NamedColor::BrightMagenta => Color::Indexed(13),
            NamedColor::BrightCyan => Color::Indexed(14),
            NamedColor::BrightWhite => Color::Indexed(15),
            NamedColor::Background => Color::Background,
            NamedColor::Foreground
            | NamedColor::Cursor
            | NamedColor::BrightForeground
            | NamedColor::DimForeground => Color::Foreground,
        },
    }
}

fn convert_cell(cell: &TermCell) -> Cell {
    Cell {
        ch: cell.c,
        zerowidth: cell.zerowidth().map(<[char]>::to_vec).unwrap_or_default(),
        fg: convert_color(cell.fg),
        bg: convert_color(cell.bg),
        flags: CellFlags::from_term(cell.flags),
    }
}

fn convert_mode(mode: TermMode) -> Mode {
    Mode {
        app_cursor: mode.contains(TermMode::APP_CURSOR),
        bracketed_paste: mode.contains(TermMode::BRACKETED_PASTE),
        alt_screen: mode.contains(TermMode::ALT_SCREEN),
        focus_reporting: mode.contains(TermMode::FOCUS_IN_OUT),
        mouse_reporting: mode.intersects(TermMode::MOUSE_MODE),
        sgr_mouse: mode.contains(TermMode::SGR_MOUSE),
        alternate_scroll: mode.contains(TermMode::ALTERNATE_SCROLL),
    }
}

/// Copy the visible grid out of a locked terminal. The title, bell and exit
/// status live outside the emulation and are filled in by the caller.
pub(crate) fn capture<L>(term: &Term<L>) -> Snapshot {
    let grid = term.grid();
    let rows = grid.screen_lines();
    let cols = grid.columns();
    let mut cells = Vec::with_capacity(rows * cols);
    for line in 0..rows {
        // A grid never has more than i32::MAX lines.
        let row = &grid[Line(i32::try_from(line).unwrap_or(i32::MAX))];
        for col in 0..cols {
            cells.push(convert_cell(&row[Column(col)]));
        }
    }
    let point = grid.cursor.point;
    let style = term.cursor_style();
    let mode = *term.mode();
    let kind = match style.shape {
        CursorShape::Underline => CursorKind::Underline,
        CursorShape::Beam => CursorKind::Beam,
        CursorShape::HollowBlock => CursorKind::HollowBlock,
        CursorShape::Block | CursorShape::Hidden => CursorKind::Block,
    };
    Snapshot {
        rows,
        cols,
        cells,
        cursor: Cursor {
            row: usize::try_from(point.line.0).unwrap_or(0),
            col: point.column.0,
            kind,
            visible: mode.contains(TermMode::SHOW_CURSOR) && style.shape != CursorShape::Hidden,
            blinking: style.blinking,
        },
        mode: convert_mode(mode),
        title: None,
        bell: false,
        history_lines: grid.history_size(),
        exit: None,
    }
}
