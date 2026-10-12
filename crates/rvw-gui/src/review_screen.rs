//! The review screen: the files that differ from the base on the left, the
//! read-only diff of the selected file in the centre, and the draft comments
//! on the right. Every fact shown comes from `rvw_view` or the snapshot; this
//! file only lays it out.

use gpui::prelude::*;
use gpui::{
    div, px, App, ClickEvent, Context, Div, FontWeight, Pixels, SharedString, Stateful, Window,
};
use rvw_view::changes::ChangedFile;
use rvw_view::diff::{FileDiff, Hunk, Line, LineKind};
use rvw_view::queue::{Filter, Row, State};

use crate::data::{self, Snapshot};
use crate::queue_screen::noun;
use crate::shell::Shell;
use crate::theme;

/// One diff line's height; the gutter and text share it so empty lines keep
/// their height.
const LINE_HEIGHT: Pixels = px(21.);

/// The file open in the centre: its path, and its diff or why it is unreadable.
pub struct Selected {
    pub path: String,
    pub diff: Result<FileDiff, String>,
}

impl Selected {
    /// Reads the diff of `path` against the snapshot's base.
    pub fn load(snapshot: &Snapshot, path: &str) -> Self {
        Self {
            path: path.to_owned(),
            diff: data::load_diff(&snapshot.workspace, &snapshot.base_id, path),
        }
    }
}

pub fn left(snapshot: &Snapshot, selected: Option<&str>, cx: &mut Context<Shell>) -> Div {
    let rows: Vec<_> = snapshot
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

    div()
        .flex()
        .flex_col()
        .h_full()
        .py(px(16.))
        .child(segmented())
        .child(search_box())
        .child(
            div()
                .px(px(24.))
                .pb(px(8.))
                .text_color(theme::muted())
                .child(summary),
        )
        .child(
            div()
                .id("change-list")
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(0.))
                .overflow_y_scroll()
                .children(rows),
        )
}

/// The "Changes | Files" control. Only Changes works; Files comes later.
fn segmented() -> Div {
    div()
        .flex()
        .mx(px(16.))
        .mb(px(12.))
        .p(px(3.))
        .rounded(px(9.))
        .bg(theme::hairline())
        .child(segment("Changes", true))
        .child(segment("Files", false))
}

fn segment(label: &'static str, active: bool) -> Div {
    div()
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
        .child(label)
}

/// The search box is drawn but not wired; find-a-file comes later.
fn search_box() -> Div {
    div()
        .flex()
        .items_center()
        .mx(px(16.))
        .mb(px(12.))
        .h(px(32.))
        .px(px(12.))
        .rounded(theme::CONTROL_RADIUS)
        .bg(theme::hairline())
        .text_color(theme::muted())
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

pub fn centre(snapshot: &Snapshot, selected: Option<&Selected>) -> Stateful<Div> {
    div()
        .id("review-centre")
        .flex()
        .flex_col()
        .flex_1()
        .min_w(px(0.))
        .bg(theme::paper())
        .child(file_header(selected))
        .child(diff_body(snapshot, selected))
}

/// The file's path, and its added and removed line counts when it has a diff.
fn file_header(selected: Option<&Selected>) -> Div {
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
    if let Some(Ok(diff)) = selected.map(|s| &s.diff) {
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
    header
}

fn diff_body(snapshot: &Snapshot, selected: Option<&Selected>) -> Stateful<Div> {
    let content = match (selected, snapshot.changes_note.as_deref()) {
        (_, Some(note)) => notice(note),
        (None, None) if snapshot.changes.is_empty() => {
            notice(&format!("No changes against {}.", snapshot.base_name))
        }
        (None, None) => notice("Select a file to see its diff."),
        (Some(selected), None) => match &selected.diff {
            Ok(diff) => diff_view(diff),
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

fn notice(text: &str) -> Div {
    div()
        .px(px(24.))
        .py(px(16.))
        .text_color(theme::muted())
        .child(text.to_owned())
}

fn diff_view(diff: &FileDiff) -> Div {
    if diff.binary {
        return notice("Binary file, no diff shown");
    }
    if diff.hunks.is_empty() {
        return notice("No line changes in this file.");
    }
    div()
        .flex()
        .flex_col()
        .py(px(8.))
        .children(diff.hunks.iter().map(hunk_view))
}

fn hunk_view(hunk: &Hunk) -> Div {
    div()
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .items_center()
                .h(LINE_HEIGHT)
                .px(px(24.))
                .font_family(theme::CODE_FONT)
                .text_size(px(theme::CODE_FONT_SIZE))
                .text_color(theme::faint())
                .whitespace_nowrap()
                .child(hunk.header.clone()),
        )
        .children(hunk.lines.iter().map(line_view))
}

fn line_view(line: &Line) -> Div {
    let (tint, marker) = match line.kind {
        LineKind::Added => (Some(theme::added_tint()), Some(theme::done())),
        LineKind::Removed => (Some(theme::removed_tint()), Some(theme::rejected())),
        LineKind::Context => (None, None),
    };
    div()
        .flex()
        .items_center()
        .h(LINE_HEIGHT)
        .font_family(theme::CODE_FONT)
        .text_size(px(theme::CODE_FONT_SIZE))
        .text_color(theme::text())
        .when_some(tint, gpui::Styled::bg)
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

pub fn right(snapshot: &Snapshot) -> Div {
    let drafts = snapshot.view(&Filter {
        state: Some(State::Draft),
        ..Filter::default()
    });
    let rows: Vec<&Row> = drafts
        .groups
        .iter()
        .flat_map(|group| group.rows.iter())
        .collect();
    let list = if rows.is_empty() {
        div()
            .text_color(theme::muted())
            .child("No comments in your draft yet.")
    } else {
        div()
            .flex()
            .flex_col()
            .children(rows.into_iter().map(draft_row))
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
        .child(submit_button())
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

/// Drawn but not wired: submitting a review comes in a later slice.
fn submit_button() -> Div {
    div()
        .flex()
        .items_center()
        .justify_center()
        .h(px(36.))
        .rounded(theme::CONTROL_RADIUS)
        .bg(theme::selected_panel())
        .text_color(theme::faint())
        .child("Submit review")
}
