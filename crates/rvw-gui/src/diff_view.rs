//! The diff of the selected file. A line click selects, a Shift click extends.
//! The file's comments sit under the diff line their range ends on, and the
//! composer sits under the last selected line.

use std::collections::BTreeMap;

use gpui::prelude::*;
use gpui::{div, px, Context, Div, FontWeight, MouseButton, MouseDownEvent, Pixels, Rgba, Window};
use rvw_core::types::{Comment, Status};
use rvw_view::diff::{FileDiff, Hunk, Line, LineKind};
use rvw_view::selection::{self, Pos, Selection};

use crate::composer::{self, Composer};
use crate::review_screen::{notice, LINE_HEIGHT};
use crate::shell::Shell;
use crate::theme;

/// What the diff shows on top of its lines: the selection, the comments placed
/// under their lines, the comments with no line in view, and the composer.
pub struct Overlay<'a> {
    selection: &'a Selection,
    under: BTreeMap<Pos, Vec<&'a Comment>>,
    outside: Vec<&'a Comment>,
    composer: Option<&'a Composer>,
    /// The composer's caption, "Line A-B"; empty when nothing is selected.
    caption: String,
}

impl<'a> Overlay<'a> {
    /// `outside_change` marks a file outside the change; its caption says so.
    pub fn new(
        diff: &FileDiff,
        comments: &[&'a Comment],
        selection: &'a Selection,
        composer: Option<&'a Composer>,
        outside_change: bool,
    ) -> Self {
        let mut under: BTreeMap<Pos, Vec<&'a Comment>> = BTreeMap::new();
        let mut outside = Vec::new();
        for &comment in comments {
            match selection::anchor(diff, comment.end_line) {
                Some(pos) => under.entry(pos).or_default().push(comment),
                None => outside.push(comment),
            }
        }
        let caption = selection
            .lines(diff)
            .map(|(lo, hi)| {
                let context = if outside_change {
                    " \u{b7} context"
                } else {
                    ""
                };
                format!("Line {}{context}", range_text(lo, hi))
            })
            .unwrap_or_default();
        Self {
            selection,
            under,
            outside,
            composer,
            caption,
        }
    }
}

/// `A` for one line, `A-B` for a range.
pub fn range_text(lo: u32, hi: u32) -> String {
    if lo == hi {
        lo.to_string()
    } else {
        format!("{lo}-{hi}")
    }
}

pub fn diff_view(diff: &FileDiff, overlay: &Overlay, cx: &Context<Shell>) -> Div {
    if diff.binary {
        return notice("Binary file, no diff shown");
    }
    if diff.hunks.is_empty() {
        return notice("No line changes in this file.");
    }
    let hunks = diff
        .hunks
        .iter()
        .enumerate()
        .map(|(index, hunk)| hunk_view(diff, index, hunk, overlay, cx));
    let mut body = div().flex().flex_col().py(px(8.)).children(hunks);
    if !overlay.outside.is_empty() {
        body = body.child(outside_strip(&overlay.outside));
    }
    body
}

fn hunk_view(
    diff: &FileDiff,
    hunk_index: usize,
    hunk: &Hunk,
    overlay: &Overlay,
    cx: &Context<Shell>,
) -> Div {
    let mut block = div().flex().flex_col();
    if !hunk.header.is_empty() {
        block = block.child(
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
        );
    }
    for (line_index, line) in hunk.lines.iter().enumerate() {
        let pos = Pos {
            hunk: hunk_index,
            line: line_index,
        };
        let selected = overlay.selection.contains(diff, pos);
        block = block.child(line_view(pos, line, selected, cx));
        for comment in overlay.under.get(&pos).into_iter().flatten() {
            block = block.child(thread(comment, px(62.)));
        }
        if let Some(composer) = overlay.composer {
            if overlay.selection.last() == Some(pos) {
                block = block.child(composer::view(composer, &overlay.caption));
            }
        }
    }
    block
}

fn line_view(pos: Pos, line: &Line, selected: bool, cx: &Context<Shell>) -> Div {
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
    let on_down = cx.listener(
        move |this: &mut Shell, event: &MouseDownEvent, _: &mut Window, cx: &mut Context<Shell>| {
            this.select_line(pos, event.modifiers.shift, cx);
        },
    );
    div()
        .flex()
        .items_center()
        .h(LINE_HEIGHT)
        .font_family(theme::CODE_FONT)
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

/// The comments whose range is in no shown hunk, in one strip at the end.
fn outside_strip(comments: &[&Comment]) -> Div {
    div()
        .flex()
        .flex_col()
        .child(
            div()
                .px(px(24.))
                .pt(px(16.))
                .pb(px(4.))
                .text_color(theme::faint())
                .child("Comments outside the shown hunks"),
        )
        .children(comments.iter().map(|comment| thread(comment, px(24.))))
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
                .font_family(theme::CODE_FONT)
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
        .my(px(4.))
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
