//! The split diff: the Unified | Split control of the file header and one
//! row of two columns. Which lines pair up is `rvw_view::split`; the rows
//! come from `diffrows::split_rows`, and a cell is drawn like a unified line
//! (same tints, same syntax colours, same selection by position). Each cell
//! has its own gutter, which is where a line is selected from.

use gpui::prelude::*;
use gpui::{
    AnyElement, ClickEvent, Context, Div, FontWeight, Stateful, WeakEntity, Window, div, px,
};
use rvw_view::diff::LineKind;
use rvw_view::selection::{Pos, Selection};

use crate::diff_list::Model;
use crate::diff_view::{code_area, code_text, drag_move, gutter};
use crate::review_screen::LINE_HEIGHT;
use crate::shell::Shell;
use crate::theme;

/// The "Unified | Split" control; `split` is the segment lit.
pub fn toggle(split: bool, cx: &Context<Shell>) -> Div {
    div()
        .flex()
        .flex_none()
        .p(px(2.))
        .rounded(px(7.))
        .bg(theme::hairline())
        .font_family(theme::ui_font())
        .child(segment("Unified", !split, false, cx))
        .child(segment("Split", split, true, cx))
}

fn segment(label: &'static str, active: bool, to: bool, cx: &Context<Shell>) -> Stateful<Div> {
    div()
        .id(label)
        .px(px(10.))
        .py(px(3.))
        .rounded(px(5.))
        .cursor_pointer()
        .when(active, |d| {
            d.bg(theme::paper())
                .text_color(theme::ink())
                .font_weight(FontWeight::MEDIUM)
        })
        .when(!active, |d| {
            d.text_color(theme::muted())
                .hover(|style| style.text_color(theme::ink()))
        })
        .on_click(cx.listener(
            move |this: &mut Shell, _: &ClickEvent, _: &mut Window, cx: &mut Context<Shell>| {
                this.set_split(to, cx);
            },
        ))
        .child(label)
}

/// Which file a cell shows a line of, and so which line number it carries.
#[derive(Clone, Copy)]
enum Side {
    Old,
    New,
}

/// One row: the old file's line, a hairline, the new file's line.
pub fn pair(
    model: &Model,
    left: Option<Pos>,
    right: Option<Pos>,
    selection: Selection,
    shell: &WeakEntity<Shell>,
) -> AnyElement {
    div()
        .w_full()
        .flex()
        .h(LINE_HEIGHT)
        .child(cell(model, left, Side::Old, selection, shell))
        .child(div().w(px(1.)).flex_none().h_full().bg(theme::hairline()))
        .child(cell(model, right, Side::New, selection, shell))
        .into_any_element()
}

fn cell(
    model: &Model,
    pos: Option<Pos>,
    side: Side,
    selection: Selection,
    shell: &WeakEntity<Shell>,
) -> Div {
    let base = div()
        .flex_1()
        .min_w(px(0.))
        .h_full()
        .flex()
        .items_center()
        .overflow_hidden()
        .font_family(theme::code_font())
        .text_size(px(theme::CODE_FONT_SIZE))
        .text_color(theme::text());
    let Some(pos) = pos else {
        return base.bg(theme::panel());
    };
    let Some(line) = model
        .diff
        .hunks
        .get(pos.hunk)
        .and_then(|hunk| hunk.lines.get(pos.line))
    else {
        return base;
    };
    let (tint, marker) = match line.kind {
        LineKind::Added => (Some(theme::added_tint()), Some(theme::done())),
        LineKind::Removed => (Some(theme::removed_tint()), Some(theme::rejected())),
        LineKind::Context => (None, None),
    };
    let background = if selection.contains(&model.diff, pos) {
        Some(theme::selected_line())
    } else {
        tint
    };
    let number = match side {
        Side::Old => line.old_no,
        Side::New => line.new_no,
    };
    let spans = model.spans.borrow().clone();
    let spans = spans
        .as_deref()
        .and_then(|file| file.get(pos.hunk))
        .and_then(|hunk| hunk.get(pos.line))
        .map_or(&[][..], Vec::as_slice);
    base.when_some(background, gpui::Styled::bg)
        .on_mouse_move(drag_move(pos, shell))
        .child(gutter(
            number.map_or(String::new(), |n| n.to_string()),
            marker,
            pos,
            shell,
        ))
        .child(code_area(
            code_text(&line.text, spans),
            model.scroll_x.get(),
            pos,
            shell,
        ))
}
