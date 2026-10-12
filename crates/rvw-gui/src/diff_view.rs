//! The rows of the diff of the selected file. A line click selects, a Shift
//! click extends. The file's comments sit under the diff line their range ends
//! on, and the composer sits under the last selected line. `diff_list` owns
//! the list; this file draws one row.

use gpui::prelude::*;
use gpui::{
    div, px, AnyElement, App, Div, FontWeight, MouseButton, MouseDownEvent, Pixels, Rgba,
    WeakEntity, Window,
};
use rvw_core::types::{Comment, Status};
use rvw_view::diff::{Line, LineKind};
use rvw_view::diffrows::{DiffRow, TRUNCATED_NOTE};
use rvw_view::selection::Selection;

use crate::composer;
use crate::diff_list::Model;
use crate::review_screen::LINE_HEIGHT;
use crate::shell::Shell;
use crate::theme;

/// `A` for one line, `A-B` for a range.
pub fn range_text(lo: u32, hi: u32) -> String {
    if lo == hi {
        lo.to_string()
    } else {
        format!("{lo}-{hi}")
    }
}

/// The composer's caption, "Line A-B"; empty when nothing is selected.
fn caption(model: &Model, selection: &Selection) -> String {
    selection
        .lines(&model.diff)
        .map(|(lo, hi)| {
            let context = if model.outside_change {
                " \u{b7} context"
            } else {
                ""
            };
            format!("Line {}{context}", range_text(lo, hi))
        })
        .unwrap_or_default()
}

/// The element of row `index`. Margins are padding here: a list item's height
/// does not include its own margin.
pub fn row(
    model: &Model,
    index: usize,
    selection: Selection,
    shell: &WeakEntity<Shell>,
) -> AnyElement {
    let Some(row) = model.rows.get(index) else {
        return div().into_any_element();
    };
    match *row {
        DiffRow::HunkHeader { hunk } => {
            let header = model.diff.hunks.get(hunk).map_or("", |h| h.header.as_str());
            note(header.to_owned(), px(24.)).into_any_element()
        }
        DiffRow::Line { pos } => {
            let Some(line) = model
                .diff
                .hunks
                .get(pos.hunk)
                .and_then(|hunk| hunk.lines.get(pos.line))
            else {
                return div().into_any_element();
            };
            let selected = selection.contains(&model.diff, pos);
            let shell = shell.clone();
            let on_down = move |event: &MouseDownEvent, _: &mut Window, cx: &mut App| {
                let shift = event.modifiers.shift;
                shell
                    .update(cx, |this, cx| this.select_line(pos, shift, cx))
                    .ok();
            };
            line_view(line, selected, on_down).into_any_element()
        }
        DiffRow::Thread { comment_index } => model.comments.get(comment_index).map_or_else(
            || div().into_any_element(),
            |comment| padded(thread(comment, px(62.))),
        ),
        DiffRow::Composer => {
            let Some(slot) = model.composer.as_ref() else {
                return div().into_any_element();
            };
            let caption = caption(model, &selection);
            div()
                .w_full()
                .py(px(6.))
                .child(composer::view(&slot.input, slot.error.as_deref(), &caption))
                .into_any_element()
        }
        DiffRow::OutsideTitle => div()
            .w_full()
            .px(px(24.))
            .pt(px(16.))
            .pb(px(4.))
            .text_color(theme::faint())
            .child("Comments outside the shown hunks")
            .into_any_element(),
        DiffRow::OutsideThread { comment_index } => model.comments.get(comment_index).map_or_else(
            || div().into_any_element(),
            |comment| padded(thread(comment, px(24.))),
        ),
        DiffRow::Truncated => note(TRUNCATED_NOTE.to_owned(), px(24.)).into_any_element(),
    }
}

fn padded(thread: Div) -> AnyElement {
    div().w_full().py(px(4.)).child(thread).into_any_element()
}

/// A dim one-line row: a hunk header, or the note that the diff was cut.
fn note(text: String, indent: Pixels) -> Div {
    div()
        .w_full()
        .flex()
        .items_center()
        .h(LINE_HEIGHT)
        .px(indent)
        .font_family(theme::code_font())
        .text_size(px(theme::CODE_FONT_SIZE))
        .text_color(theme::faint())
        .whitespace_nowrap()
        .child(text)
}

fn line_view(
    line: &Line,
    selected: bool,
    on_down: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    let (tint, marker) = match line.kind {
        LineKind::Added => (Some(theme::added_tint()), Some(theme::done())),
        LineKind::Removed => (Some(theme::removed_tint()), Some(theme::rejected())),
        LineKind::Context => (None, None),
    };
    let background = if selected {
        Some(theme::selected_line())
    } else {
        tint
    };
    div()
        .w_full()
        .flex()
        .items_center()
        .h(LINE_HEIGHT)
        .font_family(theme::code_font())
        .text_size(px(theme::CODE_FONT_SIZE))
        .text_color(theme::text())
        .when_some(background, gpui::Styled::bg)
        .on_mouse_down(MouseButton::Left, on_down)
        .child(
            div()
                .w(px(46.))
                .flex_none()
                .pr(px(14.))
                .text_right()
                .text_color(theme::faint())
                .child(line.gutter_no().map_or(String::new(), |n| n.to_string())),
        )
        .child(
            div()
                .w(px(2.))
                .flex_none()
                .h_full()
                .when_some(marker, gpui::Styled::bg),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .pl(px(14.))
                .whitespace_nowrap()
                .overflow_hidden()
                .child(line.text.clone()),
        )
}

/// One comment: a rule in its status colour, the author, the id and lines,
/// the status, then the text.
fn thread(comment: &Comment, indent: Pixels) -> Div {
    let colour = status_colour(comment.status);
    let lines = if comment.start_line == comment.end_line {
        format!("line {}", comment.start_line)
    } else {
        format!("lines {} to {}", comment.start_line, comment.end_line)
    };
    let meta = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(8.))
        .child(
            div()
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme::ink())
                .child(comment.author.clone().unwrap_or_default()),
        )
        .child(
            div()
                .font_family(theme::code_font())
                .text_size(px(theme::CODE_FONT_SIZE))
                .text_color(theme::faint())
                .child(format!("{} {lines}", comment.id)),
        )
        .child(div().text_color(colour).child(format!(
            "{} {}",
            status_glyph(comment.status),
            comment.status.as_str()
        )));
    div()
        .flex()
        .flex_col()
        .gap(px(3.))
        .ml(indent)
        .mr(px(24.))
        .pl(px(12.))
        .py(px(4.))
        .border_l_2()
        .border_color(colour)
        .child(meta)
        .child(
            div()
                .text_color(theme::text())
                .child(comment.comment.clone()),
        )
}

fn status_colour(status: Status) -> Rgba {
    match status {
        Status::Pending => theme::pending(),
        Status::Pulled => theme::agent(),
        Status::Done => theme::done(),
        Status::Rejected => theme::rejected(),
    }
}

fn status_glyph(status: Status) -> &'static str {
    match status {
        Status::Pending => "○",
        Status::Pulled => "◐",
        Status::Done => "✓",
        Status::Rejected => "✗",
    }
}
