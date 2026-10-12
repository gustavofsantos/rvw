//! The review screen: the files that differ from the base on the left, the
//! read-only diff of the selected file in the centre, and the draft comments
//! on the right. Every fact shown comes from `rvw_view` or the snapshot; this
//! file only lays it out.

use gpui::prelude::*;
use gpui::{
    div, px, AnyElement, App, ClickEvent, Context, Div, FontWeight, Pixels, ScrollHandle,
    SharedString, Stateful, Window,
};
use rvw_core::types::Comment;
use rvw_view::changes::ChangedFile;
use rvw_view::diff::FileDiff;
use rvw_view::keys::Screen;
use rvw_view::queue::Row;
use rvw_view::selection::Selection;

use crate::composer::Composer;
use crate::data::{self, Snapshot};
use crate::diff_view::{diff_view, range_text, Overlay};
use crate::queue_screen::noun;
use crate::shell::Shell;
use crate::submit_dialog;
use crate::theme;

/// One diff line's height; the gutter and text share it so empty lines keep
/// their height.
pub const LINE_HEIGHT: Pixels = px(21.);

/// The file open in the centre: its path, and its diff or why it is unreadable.
pub struct Selected {
    pub path: String,
    pub diff: Result<FileDiff, String>,
    /// The reviewer's lines in this file; reset when the file changes.
    pub selection: Selection,
    /// The change does not touch the file; it is shown as plain content.
    pub outside: bool,
    /// The file is longer than the lines shown.
    pub cut: bool,
}

impl Selected {
    /// Reads the diff of `path` against the snapshot's base.
    pub fn load(snapshot: &Snapshot, path: &str) -> Self {
        Self {
            path: path.to_owned(),
            diff: data::load_diff(&snapshot.workspace, &snapshot.base_id, path),
            selection: Selection::default(),
            outside: false,
            cut: false,
        }
    }
}

pub fn left(
    snapshot: &Snapshot,
    selected: Option<&str>,
    scroll: &ScrollHandle,
    cx: &Context<Shell>,
) -> Div {
    let rows: Vec<AnyElement> = snapshot
        .changes
        .iter()
        .map(|file| {
            let owned = file.path.clone();
            let on_click = cx.listener(
                move |this: &mut Shell, _: &ClickEvent, _: &mut Window, cx: &mut Context<Shell>| {
                    this.select_file(&owned, cx);
                },
            );
            file_row(file, selected == Some(file.path.as_str()), on_click).into_any_element()
        })
        .collect();

    let count = snapshot.changes.len();
    let summary = format!(
        "{count} {} against {}",
        noun(count, "file", "files"),
        snapshot.base_name
    );
    panel(
        Screen::Diff,
        &summary,
        list("change-list", rows, scroll),
        cx,
    )
}

/// The frame of the left panel, shared by both tabs: the Changes | Files
/// control, the find box, a summary line, and the tab's list.
pub fn panel(active: Screen, summary: &str, list: Stateful<Div>, cx: &Context<Shell>) -> Div {
    div()
        .flex()
        .flex_col()
        .h_full()
        .py(px(16.))
        .child(tabs(active, cx))
        .child(search_box(cx))
        .child(
            div()
                .px(px(24.))
                .pb(px(8.))
                .text_color(theme::muted())
                .child(summary.to_owned()),
        )
        .child(list)
}

/// A scrolling list of rows. `scroll` lets the keyboard bring a row into view.
pub fn list(id: &'static str, rows: Vec<AnyElement>, scroll: &ScrollHandle) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_col()
        .flex_1()
        .min_h(px(0.))
        .overflow_y_scroll()
        .track_scroll(scroll)
        .children(rows)
}

/// The "Changes | Files" control. Each segment shows its tab.
fn tabs(active: Screen, cx: &Context<Shell>) -> Div {
    div()
        .flex()
        .mx(px(16.))
        .mb(px(12.))
        .p(px(3.))
        .rounded(px(9.))
        .bg(theme::hairline())
        .child(segment("Changes", active == Screen::Diff, Screen::Diff, cx))
        .child(segment("Files", active == Screen::Files, Screen::Files, cx))
}

fn segment(
    label: &'static str,
    active: bool,
    screen: Screen,
    cx: &Context<Shell>,
) -> Stateful<Div> {
    div()
        .id(label)
        .flex_1()
        .py(px(5.))
        .rounded(px(7.))
        .text_center()
        .when(active, |d| {
            d.bg(theme::paper())
                .text_color(theme::ink())
                .font_weight(FontWeight::MEDIUM)
        })
        .when(!active, |d| d.text_color(theme::muted()))
        .on_click(cx.listener(
            move |this: &mut Shell, _: &ClickEvent, _: &mut Window, cx: &mut Context<Shell>| {
                this.show(screen, cx);
            },
        ))
        .child(label)
}

/// The find box: a click opens the finder, as ⌘P does.
fn search_box(cx: &Context<Shell>) -> Stateful<Div> {
    div()
        .id("find-file")
        .flex()
        .items_center()
        .mx(px(16.))
        .mb(px(12.))
        .h(px(32.))
        .px(px(12.))
        .rounded(theme::CONTROL_RADIUS)
        .bg(theme::hairline())
        .text_color(theme::muted())
        .on_click(cx.listener(
            |this: &mut Shell, _: &ClickEvent, window: &mut Window, cx: &mut Context<Shell>| {
                this.open_finder(window, cx);
            },
        ))
        .child(div().flex_1().child("Find a file"))
        .child(
            div()
                .font_family(theme::CODE_FONT)
                .text_size(px(theme::KEY_FONT_SIZE))
                .text_color(theme::faint())
                .child("⌘P"),
        )
}

fn file_row(
    file: &ChangedFile,
    selected: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let (dir, name) = split_path(&file.path);
    div()
        .id(SharedString::from(format!("change-{}", file.path)))
        .flex()
        .items_center()
        .gap(px(8.))
        .h(px(30.))
        .mx(px(8.))
        .px(px(12.))
        .rounded(theme::ROW_RADIUS)
        .text_color(theme::text())
        .when(selected, |d| {
            d.bg(theme::selected_panel())
                .text_color(theme::ink())
                .font_weight(FontWeight::MEDIUM)
        })
        .on_click(on_click)
        .child(
            div()
                .w(px(12.))
                .flex_none()
                .font_family(theme::CODE_FONT)
                .text_size(px(12.))
                .text_color(theme::faint())
                .child(file.status.to_string()),
        )
        .child(
            div()
                .flex_none()
                .font_family(theme::CODE_FONT)
                .text_size(px(12.))
                .child(name.to_owned()),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .font_family(theme::CODE_FONT)
                .text_size(px(12.))
                .text_color(theme::faint())
                .child(dir.to_owned()),
        )
        .when(file.open_comments > 0, |d| {
            d.child(
                div()
                    .flex_none()
                    .font_weight(FontWeight::NORMAL)
                    .text_color(theme::pending())
                    .child(format!("○ {}", file.open_comments)),
            )
        })
}

/// The directory and the base name of a path; the directory is empty at the top.
fn split_path(path: &str) -> (&str, &str) {
    path.rsplit_once('/').unwrap_or(("", path))
}

pub fn centre(
    snapshot: &Snapshot,
    selected: Option<&Selected>,
    composer: Option<&Composer>,
    cx: &Context<Shell>,
) -> Stateful<Div> {
    div()
        .id("review-centre")
        .flex()
        .flex_col()
        .flex_1()
        .min_w(px(0.))
        .bg(theme::paper())
        .child(file_header(selected, cx))
        .children(selected.filter(|s| s.outside).map(context_banner))
        .child(diff_body(snapshot, selected, composer, cx))
}

/// The file's path, its added and removed line counts when it has a diff, and
/// the selection's range with the Comment affordance.
fn file_header(selected: Option<&Selected>, cx: &Context<Shell>) -> Div {
    let path = selected.map_or("", |s| s.path.as_str()).to_owned();
    let mut header = div()
        .flex()
        .items_center()
        .gap(px(12.))
        .h(px(48.))
        .px(px(24.))
        .border_b_1()
        .border_color(theme::hairline())
        .font_family(theme::CODE_FONT)
        .text_size(px(theme::CODE_FONT_SIZE))
        .text_color(theme::ink())
        .child(div().flex_1().min_w(px(0.)).truncate().child(path));
    let Some(selected) = selected else {
        return header;
    };
    if let Ok(diff) = &selected.diff {
        if !selected.outside {
            header = header
                .child(
                    div()
                        .text_color(theme::done())
                        .child(format!("+{}", diff.added)),
                )
                .child(
                    div()
                        .text_color(theme::rejected())
                        .child(format!("\u{2212}{}", diff.removed)),
                );
        }
        if let Some((lo, hi)) = selected.selection.lines(diff) {
            let noun = if lo == hi { "Line" } else { "Lines" };
            header = header
                .child(
                    div()
                        .text_color(theme::faint())
                        .child(format!("{noun} {}", range_text(lo, hi))),
                )
                .child(comment_button(cx));
        }
    }
    header
}

/// The notice above a file outside the change: a comment there travels with
/// the review as context. Says so when the file is cut short.
fn context_banner(selected: &Selected) -> Div {
    let mut banner = div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .mx(px(24.))
        .mt(px(16.))
        .px(px(14.))
        .py(px(10.))
        .rounded(theme::CONTROL_RADIUS)
        .bg(theme::panel())
        .text_color(theme::muted())
        .child(
            "This file is outside your change. A comment here travels with your review as context.",
        );
    if selected.cut {
        banner = banner.child(
            div()
                .font_family(theme::CODE_FONT)
                .text_size(px(theme::KEY_FONT_SIZE))
                .text_color(theme::faint())
                .child(format!(
                    "Showing the first {} lines",
                    rvw_view::context::MAX_LINES
                )),
        );
    }
    banner
}

/// Opens the composer for the selection; the same as pressing C.
fn comment_button(cx: &Context<Shell>) -> Stateful<Div> {
    div()
        .id("comment-button")
        .flex()
        .items_center()
        .gap(px(8.))
        .h(px(26.))
        .px(px(10.))
        .rounded(theme::CONTROL_RADIUS)
        .bg(theme::hairline())
        .font_family(theme::UI_FONT)
        .text_color(theme::ink())
        .on_click(cx.listener(
            |this: &mut Shell, _: &ClickEvent, window: &mut Window, cx: &mut Context<Shell>| {
                this.open_composer(window, cx);
            },
        ))
        .child("Comment")
        .child(
            div()
                .font_family(theme::CODE_FONT)
                .text_size(px(theme::KEY_FONT_SIZE))
                .text_color(theme::faint())
                .child("C"),
        )
}

fn diff_body(
    snapshot: &Snapshot,
    selected: Option<&Selected>,
    composer: Option<&Composer>,
    cx: &Context<Shell>,
) -> Stateful<Div> {
    let content = match (selected, snapshot.changes_note.as_deref()) {
        (_, Some(note)) => notice(note),
        (None, None) if snapshot.changes.is_empty() => {
            notice(&format!("No changes against {}.", snapshot.base_name))
        }
        (None, None) => notice("Select a file to see its diff."),
        (Some(selected), None) => match &selected.diff {
            Ok(diff) => {
                let comments: Vec<&Comment> = snapshot
                    .comments
                    .iter()
                    .filter(|comment| comment.file == selected.path)
                    .collect();
                let overlay = Overlay::new(
                    diff,
                    &comments,
                    &selected.selection,
                    composer,
                    selected.outside,
                );
                diff_view(diff, &overlay, cx)
            }
            Err(message) => notice(message),
        },
    };
    div()
        .id("diff-body")
        .flex_1()
        .min_h(px(0.))
        .overflow_y_scroll()
        .child(content)
}

pub fn notice(text: &str) -> Div {
    div()
        .px(px(24.))
        .py(px(16.))
        .text_color(theme::muted())
        .child(text.to_owned())
}

pub fn right(snapshot: &Snapshot, cx: &Context<Shell>) -> Div {
    let rows = submit_dialog::draft_rows(snapshot);
    let drafts = rows.len();
    let list = if rows.is_empty() {
        div()
            .text_color(theme::muted())
            .child("No comments in your draft yet.")
    } else {
        div().flex().flex_col().children(rows.iter().map(draft_row))
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
        .child(list)
        .child(div().flex_1())
        .child(submit_button(drafts, cx))
}

fn draft_row(row: &Row) -> Div {
    div()
        .flex()
        .items_start()
        .gap(px(10.))
        .py(px(10.))
        .border_b_1()
        .border_color(theme::hairline())
        .child(
            div()
                .w(px(14.))
                .flex_none()
                .text_center()
                .text_color(theme::pending())
                .child(row.state.glyph()),
        )
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
                        .font_family(theme::CODE_FONT)
                        .text_size(px(theme::CODE_FONT_SIZE))
                        .text_color(theme::faint())
                        .child(row.location.clone()),
                ),
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
                .font_family(theme::CODE_FONT)
                .text_size(px(theme::KEY_FONT_SIZE))
                .text_color(theme::faint())
                .child("⇧⌘R"),
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
