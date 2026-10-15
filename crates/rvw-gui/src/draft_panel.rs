//! The right panel of the review screen: the drafts so far and the submit
//! button.

use gpui::prelude::*;
use gpui::{ClickEvent, Context, Div, FontWeight, SharedString, Stateful, Window, div, px};
use rvw_core::types::Location;
use rvw_view::queue::Row;

use crate::data::Snapshot;
use crate::diff_view;
use crate::draft_edit::Removal;
use crate::icon;
use crate::shell::Shell;
use crate::submit_dialog;
use crate::theme;

pub fn right(snapshot: &Snapshot, removal: Option<&Removal>, cx: &Context<Shell>) -> Div {
    let rows = submit_dialog::draft_rows(snapshot);
    let drafts = rows.len();
    let list = if rows.is_empty() {
        div()
            .text_color(theme::muted())
            .child("No comments in your draft yet.")
    } else {
        div().flex().flex_col().children(
            rows.iter()
                .map(|row| draft_row(row, points_at(snapshot, &row.id), cx)),
        )
    };

    div()
        .flex()
        .flex_col()
        .h_full()
        .p(theme::PANEL_PADDING)
        .gap(px(18.))
        .child(
            div()
                .text_size(px(22.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme::ink())
                .child("Your review"),
        )
        .children(removal.map(|removal| removal_line(removal, cx)))
        .child(list)
        .child(div().flex_1())
        .child(submit_button(drafts, cx))
}

/// The line after a delete: what was removed and Undo, or why it failed.
fn removal_line(removal: &Removal, cx: &Context<Shell>) -> Div {
    let line = div().flex().items_center().gap(px(10.));
    match removal {
        Removal::Removed(removed) => line
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .truncate()
                    .text_color(theme::muted())
                    .child(format!("Removed {}", removed.id)),
            )
            .child(
                div()
                    .id("undo-remove")
                    .cursor_pointer()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme::ink())
                    .on_click(cx.listener(
                        |this: &mut Shell,
                         _: &ClickEvent,
                         _: &mut Window,
                         cx: &mut Context<Shell>| {
                            this.undo_removal(cx);
                        },
                    ))
                    .child("Undo"),
            ),
        Removal::Failed(message) => {
            line.child(div().text_color(theme::rejected()).child(message.clone()))
        }
    }
}

/// The changed location draft `id` points at, if it has one.
fn points_at<'a>(snapshot: &'a Snapshot, id: &str) -> Option<&'a Location> {
    snapshot
        .comments
        .iter()
        .find(|comment| comment.id == id)
        .and_then(|comment| comment.points_at.as_ref())
}

/// One draft comment. A click opens its outcome screen.
fn draft_row(row: &Row, points: Option<&Location>, cx: &Context<Shell>) -> Stateful<Div> {
    let id = row.id.clone();
    let on_click = cx.listener(
        move |this: &mut Shell, _: &ClickEvent, _: &mut Window, cx: &mut Context<Shell>| {
            this.open_outcome(&id, cx);
        },
    );
    div()
        .id(SharedString::from(format!("draft-row-{}", row.id)))
        .flex()
        .items_start()
        .on_click(on_click)
        .gap(px(10.))
        .py(px(10.))
        .border_b_1()
        .border_color(theme::hairline())
        .child(icon::status(row.state.glyph(), theme::pending()).mt(px(2.)))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .truncate()
                        .text_color(theme::ink())
                        .child(row.title.clone()),
                )
                .child(
                    div()
                        .font_family(theme::code_font())
                        .text_size(px(theme::CODE_FONT_SIZE))
                        .text_color(theme::faint())
                        .child(row.location.clone()),
                )
                .when_some(points, |column, at| {
                    column.child(diff_view::points_note(at))
                }),
        )
}

/// Opens the submit dialog. Enabled only with at least one draft comment; the
/// key hint shows the shortcut that does the same.
fn submit_button(drafts: usize, cx: &Context<Shell>) -> Stateful<Div> {
    let ready = drafts > 0;
    let mut button = div()
        .id("submit-review-button")
        .flex()
        .items_center()
        .justify_center()
        .gap(px(8.))
        .h(px(36.))
        .rounded(theme::CONTROL_RADIUS)
        .child("Submit review")
        .child(
            div()
                .font_family(theme::code_font())
                .text_size(px(theme::KEY_FONT_SIZE))
                .text_color(theme::faint())
                .child(theme::chord("⇧⌘R")),
        );
    if ready {
        button = button
            .bg(theme::ink())
            .text_color(theme::paper())
            .font_weight(FontWeight::MEDIUM)
            .on_click(cx.listener(
                |this: &mut Shell, _: &ClickEvent, window: &mut Window, cx: &mut Context<Shell>| {
                    this.open_submit(window, cx);
                },
            ));
    } else {
        button = button
            .bg(theme::selected_panel())
            .text_color(theme::faint());
    }
    button
}
