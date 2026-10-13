//! Sideways scrolling of code. A diff list scrolls only up and down, so the
//! code text of every line row is drawn shifted left by one shared offset,
//! inside a cell that clips it; the line numbers and the comment threads stay
//! put. The offset stops when the longest line's end is in view.

use gpui::prelude::*;
use gpui::{div, font, px, AnyElement, Div, Pixels, ScrollWheelEvent, Window};
use rvw_view::diff::FileDiff;

use crate::theme;

/// The room kept after the end of the longest line.
const END_MARGIN: Pixels = px(24.);

/// Columns a tab takes when a line's width is reckoned.
const TAB_COLUMNS: usize = 4;

/// The width of one character of the code font.
pub fn advance(window: &Window) -> Pixels {
    let text = window.text_system();
    let id = text.resolve_font(&font(theme::code_font()));
    text.em_advance(id, px(theme::CODE_FONT_SIZE))
        .unwrap_or(px(theme::CODE_FONT_SIZE * 0.6))
}

/// The columns of the longest line of `diff`, a tab taking four.
pub fn longest_line(diff: &FileDiff) -> usize {
    diff.hunks
        .iter()
        .flat_map(|hunk| hunk.lines.iter())
        .map(|line| {
            line.text
                .chars()
                .map(|c| if c == '\t' { TAB_COLUMNS } else { 1 })
                .sum::<usize>()
        })
        .max()
        .unwrap_or(0)
}

/// The width of `columns` characters `advance` wide. A line longer than
/// 65535 columns counts as that long.
pub fn width(advance: Pixels, columns: usize) -> Pixels {
    advance * f32::from(u16::try_from(columns).unwrap_or(u16::MAX))
}

/// The furthest the code can move left: its width past the room it has.
pub fn max_offset(content: Pixels, room: Pixels) -> Pixels {
    (content + END_MARGIN - room).max(Pixels::ZERO)
}

/// The offset after a wheel turn moved it by `delta`, kept in `0..=max`.
/// Turning right (a negative delta) moves the code left.
pub fn scrolled(offset: Pixels, delta: Pixels, max: Pixels) -> Pixels {
    (offset - delta).clamp(Pixels::ZERO, max.max(Pixels::ZERO))
}

/// The sideways part of a wheel or trackpad event, in pixels.
pub fn sideways(event: &ScrollWheelEvent) -> Pixels {
    event.delta.pixel_delta(px(20.)).x
}

/// `code` shifted left by `offset`; its cell clips it.
pub fn shifted(code: AnyElement, offset: Pixels) -> Div {
    div().relative().left(-offset).child(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_code_never_moves() {
        assert_eq!(max_offset(px(300.), px(800.)), Pixels::ZERO);
        assert_eq!(scrolled(Pixels::ZERO, px(-50.), Pixels::ZERO), Pixels::ZERO);
    }

    #[test]
    fn long_code_moves_until_its_end_is_in_view() {
        let max = max_offset(px(1000.), px(600.));
        assert_eq!(max, px(424.));
        assert_eq!(scrolled(Pixels::ZERO, px(-100.), max), px(100.));
        assert_eq!(scrolled(px(400.), px(-100.), max), max);
        assert_eq!(scrolled(px(50.), px(100.), max), Pixels::ZERO);
    }

    #[test]
    fn a_tab_counts_as_four_columns() {
        let diff = rvw_view::diff::parse(
            "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,2 +1,2 @@\n-ab\n+\tab\n x\n",
        );
        assert_eq!(longest_line(&diff), 6);
    }
}
