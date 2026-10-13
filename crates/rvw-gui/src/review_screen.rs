//! The review screen: the files that differ from the base on the left, the
//! read-only diff of the selected file in the centre, and the draft comments
//! on the right. Every fact shown comes from `rvw_view` or the snapshot; this
//! file only lays it out.

use gpui::prelude::*;
use std::ops::Range;
use std::rc::Rc;

use gpui::{
    canvas, div, px, uniform_list, AnyElement, App, ClickEvent, Context, Div, FontWeight,
    MouseButton, MouseMoveEvent, MouseUpEvent, Pixels, SharedString, Stateful,
    UniformListScrollHandle, WeakEntity, Window,
};
use rvw_view::changes::ChangedFile;
use rvw_view::diff::FileDiff;
use rvw_view::keys::Screen;
use rvw_view::selection::Selection;
use rvw_view::viewmode::ViewMode;

use crate::data::{self, Snapshot};
use crate::diff_list::DiffList;
use crate::diff_view::range_text;
use crate::queue_screen::noun;
use crate::shell::Shell;
use crate::theme;

/// One diff line's height; the gutter and text share it so empty lines keep
/// their height.
pub const LINE_HEIGHT: Pixels = px(21.);

/// The file open in the centre: its path, and its diff or why it is unreadable.
pub struct Selected {
    pub path: String,
    pub diff: Result<Rc<FileDiff>, String>,
    /// The reviewer's lines in this file; reset when the file changes.
    pub selection: Selection,
    /// The change does not touch the file; it is shown as plain content.
    pub outside: bool,
    /// The file is longer than the lines shown.
    pub cut: bool,
    /// The diff, or the whole file with the changed lines marked.
    pub mode: ViewMode,
    /// The change touches the file, so both modes exist for it.
    pub changed: bool,
}

impl Selected {
    /// Reads the diff of `path` against the snapshot's base.
    pub fn load(snapshot: &Snapshot, path: &str) -> Self {
        Self {
            path: path.to_owned(),
            diff: data::load_diff(&snapshot.workspace, &snapshot.base_id, path).map(Rc::new),
            selection: Selection::default(),
            outside: false,
            cut: false,
            mode: ViewMode::Diff,
            changed: true,
        }
    }
}

pub fn left(snapshot: &Snapshot, scroll: &UniformListScrollHandle, cx: &Context<Shell>) -> Div {
    let count = snapshot.changes.len();
    let summary = format!(
        "{count} {} against {}",
        noun(count, "file", "files"),
        snapshot.base_name
    );
    // Only the rows in view are built, from the shell's current state.
    let rows = uniform_list(
        "change-list",
        count,
        cx.processor(
            |this: &mut Shell, range: Range<usize>, _: &mut Window, cx: &mut Context<Shell>| {
                let selected = this.selected.as_ref().map(|s| s.path.as_str());
                let files = this.snapshot.changes.get(range).unwrap_or_default();
                files
                    .iter()
                    .map(|file| {
                        let owned = file.path.clone();
                        let on_click = cx.listener(
                            move |this: &mut Shell,
                                  _: &ClickEvent,
                                  _: &mut Window,
                                  cx: &mut Context<Shell>| {
                                this.select_file(&owned, cx);
                            },
                        );
                        file_row(file, selected == Some(file.path.as_str()), on_click)
                    })
                    .collect::<Vec<_>>()
            },
        ),
    )
    .track_scroll(scroll.clone())
    .flex_1()
    .min_h(px(0.));
    panel(Screen::Diff, &summary, rows.into_any_element(), cx)
}

/// The frame of the left panel, shared by both tabs: the Changes | Files
/// control, the find box, a summary line, and the tab's list.
pub fn panel(active: Screen, summary: &str, list: AnyElement, cx: &Context<Shell>) -> Div {
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
                .font_family(theme::code_font())
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
                .font_family(theme::code_font())
                .text_size(px(12.))
                .text_color(theme::faint())
                .child(file.status.to_string()),
        )
        .child(
            div()
                .flex_none()
                .font_family(theme::code_font())
                .text_size(px(12.))
                .child(name.to_owned()),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .font_family(theme::code_font())
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
    diff_list: Option<&DiffList>,
    split: Option<bool>,
    cx: &Context<Shell>,
) -> Stateful<Div> {
    div()
        .id("review-centre")
        .flex()
        .flex_col()
        .flex_1()
        .min_w(px(0.))
        .bg(theme::paper())
        .child(file_header(selected, split, cx))
        .children(selected.and_then(context_banner))
        .child(diff_body(snapshot, selected, diff_list, cx))
}

/// The file's path, its added and removed line counts when it has a diff, and
/// the selection's range with the Comment affordance.
fn file_header(selected: Option<&Selected>, split: Option<bool>, cx: &Context<Shell>) -> Div {
    let path = selected.map_or("", |s| s.path.as_str()).to_owned();
    let mut header = div()
        .flex()
        .items_center()
        .gap(px(12.))
        .h(px(48.))
        .px(px(24.))
        .border_b_1()
        .border_color(theme::hairline())
        .font_family(theme::code_font())
        .text_size(px(theme::CODE_FONT_SIZE))
        .text_color(theme::ink())
        .child(div().flex_1().min_w(px(0.)).truncate().child(path));
    let Some(selected) = selected else {
        return header;
    };
    if let Ok(diff) = &selected.diff {
        if !selected.outside && selected.mode == ViewMode::Diff {
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
    if selected.changed {
        header = header.child(crate::file_view::mode_toggle(selected.mode, cx));
    }
    if let Some(split) = split {
        header = header.child(crate::split_view::toggle(split, cx));
    }
    header
}

/// The notice above a file outside the change: a comment there travels with
/// the review as context. Either file says so when it is cut short.
fn context_banner(selected: &Selected) -> Option<Div> {
    if !selected.outside && !selected.cut {
        return None;
    }
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
        .text_color(theme::muted());
    if selected.outside {
        banner = banner.child(
            "This file is outside your change. A comment here travels with your review as context.",
        );
    }
    if selected.cut {
        banner = banner.child(
            div()
                .font_family(theme::code_font())
                .text_size(px(theme::KEY_FONT_SIZE))
                .text_color(theme::faint())
                .child(format!(
                    "Showing the first {} lines",
                    rvw_view::context::MAX_LINES
                )),
        );
    }
    Some(banner)
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
        .font_family(theme::ui_font())
        .text_color(theme::ink())
        .on_click(cx.listener(
            |this: &mut Shell, _: &ClickEvent, window: &mut Window, cx: &mut Context<Shell>| {
                this.open_composer(window, cx);
            },
        ))
        .child("Comment")
        .child(
            div()
                .font_family(theme::code_font())
                .text_size(px(theme::KEY_FONT_SIZE))
                .text_color(theme::faint())
                .child("C"),
        )
}

fn diff_body(
    snapshot: &Snapshot,
    selected: Option<&Selected>,
    diff_list: Option<&DiffList>,
    cx: &Context<Shell>,
) -> Div {
    let body = div().flex_1().min_h(px(0.));
    match (selected, snapshot.changes_note.as_deref()) {
        (_, Some(note)) => body.child(notice(note)),
        (None, None) if snapshot.changes.is_empty() => body.child(notice(&format!(
            "No changes against {}.",
            snapshot.base_name
        ))),
        (None, None) => body.child(notice("Select a file to see its diff.")),
        (Some(selected), None) => match (&selected.diff, diff_list) {
            (Ok(_), Some(list)) => body
                .py(px(8.))
                .child(list.element(cx.weak_entity()).size_full())
                .child(drag_listeners(cx.weak_entity())),
            (Ok(diff), None) if diff.binary => body.child(notice("Binary file, no diff shown")),
            (Ok(_), None) => body.child(notice("No line changes in this file.")),
            (Err(message), _) => body.child(notice(message)),
        },
    }
}

/// Window-level pointer listeners of a line drag. They sit beside the list
/// so a drag keeps going when the pointer leaves the rows, and ends on a
/// release anywhere in the window.
fn drag_listeners(shell: WeakEntity<Shell>) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |_, (), window, _| {
            let moves = shell.clone();
            window.on_mouse_event(move |event: &MouseMoveEvent, _, _, cx| {
                let held = event.pressed_button == Some(MouseButton::Left);
                moves
                    .update(cx, |this, _| this.drag_pointer(event.position, held))
                    .ok();
            });
            window.on_mouse_event(move |event: &MouseUpEvent, _, _, cx| {
                if event.button == MouseButton::Left {
                    shell.update(cx, |this, _| this.end_drag()).ok();
                }
            });
        },
    )
    .absolute()
    .size_0()
}

pub fn notice(text: &str) -> Div {
    div()
        .px(px(24.))
        .py(px(16.))
        .text_color(theme::muted())
        .child(text.to_owned())
}
