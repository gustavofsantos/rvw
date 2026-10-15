//! The code and the diff of the outcome screen, as the diff view draws them:
//! a panel-coloured block of numbered lines, with the same tints.

use gpui::prelude::*;
use gpui::{Div, Rgba, Styled, div, px};
use rvw_view::diff::{FileDiff, Hunk, Line, LineKind};
use rvw_view::outcome::Story;

use crate::theme;

/// The code the comment was made on, numbered from its first line. The
/// panel-coloured block and the 46px gutter match the diff view.
pub(crate) fn code_block(story: &Story) -> Div {
    let rows = story.code.lines().enumerate().map(|(index, text)| {
        let offset = i64::try_from(index).unwrap_or_default();
        code_row((story.start_line + offset).to_string(), None, None, text)
    });
    block().children(rows)
}

/// The resolution diff, with the same tints as the diff view.
pub(crate) fn diff_block(diff: &FileDiff) -> Div {
    if diff.hunks.is_empty() {
        let note = if diff.binary {
            "A binary file changed."
        } else {
            "No line changes."
        };
        return div().text_color(theme::muted()).child(note);
    }
    block().children(diff.hunks.iter().flat_map(hunk_rows))
}

fn hunk_rows(hunk: &Hunk) -> Vec<Div> {
    let header = code_row(String::new(), None, None, &hunk.header);
    std::iter::once(header)
        .chain(hunk.lines.iter().map(line_row))
        .collect()
}

fn line_row(line: &Line) -> Div {
    let (tint, marker) = match line.kind {
        LineKind::Added => (Some(theme::added_tint()), Some(theme::done())),
        LineKind::Removed => (Some(theme::removed_tint()), Some(theme::rejected())),
        LineKind::Context => (None, None),
    };
    let number = line.gutter_no().map_or(String::new(), |n| n.to_string());
    code_row(number, marker, tint, &line.text)
}

/// The panel-coloured block that holds code or a diff.
fn block() -> Div {
    div()
        .flex()
        .flex_col()
        .py(px(6.))
        .rounded(theme::CONTROL_RADIUS)
        .overflow_hidden()
        .bg(theme::panel())
        .font_family(theme::code_font())
        .text_size(px(theme::CODE_FONT_SIZE))
        .text_color(theme::text())
}

/// One line: its number in a 46px gutter, a 2px marker bar, then the text.
fn code_row(number: String, marker: Option<Rgba>, tint: Option<Rgba>, text: &str) -> Div {
    div()
        .flex()
        .items_center()
        .h(px(21.))
        .when_some(tint, Styled::bg)
        .child(
            div()
                .w(px(46.))
                .flex_none()
                .pr(px(14.))
                .text_right()
                .text_color(theme::faint())
                .child(number),
        )
        .child(
            div()
                .w(px(2.))
                .flex_none()
                .h_full()
                .when_some(marker, Styled::bg),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .pl(px(14.))
                .whitespace_nowrap()
                .overflow_hidden()
                .child(text.replace('\t', "    ")),
        )
}
