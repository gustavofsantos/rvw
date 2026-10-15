//! Viewport scrolling over the scrollback.

use alacritty_terminal::Term;
use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Scroll as TermScroll;

/// A viewport move. "Up" goes toward older output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scroll {
    /// Up by this many lines.
    Up(usize),
    /// Down by this many lines, toward the live screen.
    Down(usize),
    /// One screenful up.
    PageUp,
    /// One screenful down.
    PageDown,
    /// To the oldest line kept.
    Top,
    /// To the live screen.
    Bottom,
}

/// The most lines one `Scroll::Up` or `Scroll::Down` moves.
pub(crate) const MAX_SCROLL_LINES: usize = 1 << 20;

pub(crate) fn apply<L: EventListener>(term: &mut Term<L>, scroll: Scroll) {
    let delta = |lines: usize, sign: i32| {
        sign * i32::try_from(lines.min(MAX_SCROLL_LINES)).unwrap_or(i32::MAX)
    };
    let scroll = match scroll {
        Scroll::Up(n) => TermScroll::Delta(delta(n, 1)),
        Scroll::Down(n) => TermScroll::Delta(delta(n, -1)),
        Scroll::PageUp => TermScroll::PageUp,
        Scroll::PageDown => TermScroll::PageDown,
        Scroll::Top => TermScroll::Top,
        Scroll::Bottom => TermScroll::Bottom,
    };
    term.scroll_display(scroll);
}
