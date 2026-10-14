//! The outcome screen: one comment's story in four steps in the centre, its
//! timeline on the right, and the comments of its review on the left so the
//! reviewer can move between them. Every fact comes from `rvw_view`; this file
//! only lays them out.

use std::collections::HashMap;

use gpui::prelude::*;
use gpui::{
    div, px, ClickEvent, Context, Div, FontWeight, Rgba, SharedString, Stateful, Styled, Window,
};
use rvw_core::types::{Comment, Status};
use rvw_view::keys::Screen;
use rvw_view::outcome::{self, Moment, Story};
use rvw_view::queue::{glyph_of, Row, State};

use crate::data::Snapshot;
use crate::icon::{self, Icon};
use crate::outcome_story::{code_block, diff_block};
use crate::shell::Shell;
use crate::theme;

/// One comment open on the outcome screen.
pub struct Outcome {
    pub id: String,
    /// The screen the user came from; Back returns to it.
    pub back: Screen,
    /// The story, or why the comment cannot be read.
    pub story: Result<Story, String>,
    pub timeline: Vec<Moment>,
    /// Why marking the comment seen failed; shown in the right panel.
    pub error: Option<String>,
}

/// The left panel: Back, then the comments of the same review, or the loose
/// comments when there is no review. The open one is selected.
pub fn left(open: &Outcome, snapshot: &Snapshot, cx: &Context<Shell>) -> Div {
    let group = outcome::group_of(&snapshot.queue, &open.id);
    let (heading, detail) = match group.and_then(|group| group.review.as_ref()) {
        Some(review) => (format!("Review {}", review.id), review.summary.clone()),
        None => ("Without a review".to_owned(), String::new()),
    };
    let by_id: HashMap<&str, &Comment> = snapshot
        .comments
        .iter()
        .map(|c| (c.id.as_str(), c))
        .collect();
    let rows: Vec<Stateful<Div>> = group
        .map(|group| group.rows.as_slice())
        .unwrap_or_default()
        .iter()
        .map(|row| list_row(row, by_id.get(row.id.as_str()).copied(), &open.id, cx))
        .collect();

    div()
        .flex()
        .flex_col()
        .h_full()
        .py(px(20.))
        .child(back_link(cx))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .px(px(24.))
                .pb(px(14.))
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme::ink())
                        .child(heading),
                )
                .when(!detail.trim().is_empty(), |d| {
                    d.child(div().text_color(theme::muted()).child(detail))
                }),
        )
        .child(
            div()
                .id("outcome-rows")
                .flex()
                .flex_col()
                .gap(px(2.))
                .overflow_y_scroll()
                .children(rows),
        )
}

fn back_link(cx: &Context<Shell>) -> Stateful<Div> {
    div()
        .id("outcome-back")
        .flex()
        .items_center()
        .h(px(30.))
        .mx(px(8.))
        .px(px(12.))
        .mb(px(12.))
        .rounded(theme::ROW_RADIUS)
        .text_color(theme::muted())
        .gap(px(6.))
        .child(icon::view(Icon::Back, icon::SIZE, theme::muted()))
        .child("Back")
        .on_click(cx.listener(
            |this: &mut Shell, _: &ClickEvent, _: &mut Window, cx: &mut Context<Shell>| {
                this.close_outcome(cx);
            },
        ))
}

/// One comment of the review: its glyph, id and title. Clicking opens it.
fn list_row(
    row: &Row,
    comment: Option<&Comment>,
    current: &str,
    cx: &Context<Shell>,
) -> Stateful<Div> {
    let id = row.id.clone();
    let glyph = comment.map_or(row.state.glyph(), |c| glyph_of(c, row.state));
    let is_current = row.id == current;
    let on_click = cx.listener(
        move |this: &mut Shell, _: &ClickEvent, _: &mut Window, cx: &mut Context<Shell>| {
            this.open_outcome(&id, cx);
        },
    );
    div()
        .id(SharedString::from(format!("outcome-row-{}", row.id)))
        .flex()
        .items_center()
        .gap(px(10.))
        .h(px(30.))
        .mx(px(8.))
        .px(px(12.))
        .rounded(theme::ROW_RADIUS)
        .text_color(theme::text())
        .when(is_current, |d| {
            d.bg(theme::selected_panel())
                .text_color(theme::ink())
                .font_weight(FontWeight::MEDIUM)
        })
        .on_click(on_click)
        .child(icon::status(glyph, glyph_colour(row.state, comment)))
        .child(
            div()
                .flex_none()
                .font_family(theme::code_font())
                .text_size(px(12.))
                .text_color(theme::muted())
                .child(row.id.clone()),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .child(row.title.clone()),
        )
}

fn glyph_colour(state: State, comment: Option<&Comment>) -> Rgba {
    let rejected = comment.is_some_and(|c| c.status == Status::Rejected);
    match state {
        State::Draft | State::NotYetTaken => theme::pending(),
        State::WithAgent => theme::agent(),
        State::WaitingOnYou => theme::needs_you(),
        State::Closed if rejected => theme::rejected(),
        State::Closed => theme::done(),
    }
}

/// The centre: the title and place of the comment, then its four steps.
pub fn centre(open: &Outcome) -> Stateful<Div> {
    let body = match &open.story {
        Ok(story) => div()
            .flex()
            .flex_col()
            .gap(px(26.))
            .child(header(story))
            .child(steps(story)),
        Err(message) => div().text_color(theme::rejected()).child(message.clone()),
    };
    div()
        .id("outcome-centre")
        .flex()
        .flex_col()
        .flex_1()
        .min_w(px(0.))
        .px(px(40.))
        .py(px(32.))
        .bg(theme::paper())
        .overflow_y_scroll()
        .child(body)
}

fn header(story: &Story) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(
            div()
                .text_size(px(22.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme::ink())
                .child(story.title.clone()),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(14.))
                .child(
                    div()
                        .font_family(theme::code_font())
                        .text_size(px(12.))
                        .text_color(theme::muted())
                        .child(story.location.clone()),
                )
                .child({
                    let (icon, text) = status_line(story);
                    icon::labelled(icon, icon::SIZE, status_colour(story.status), text)
                }),
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

fn status_line(story: &Story) -> (Icon, String) {
    match story.status {
        Status::Pending => (Icon::Pending, "not yet taken".to_owned()),
        Status::Pulled => (Icon::WithAgent, "with an agent".to_owned()),
        Status::Done => (Icon::Done, format!("done by {}", story.resolved_by)),
        Status::Rejected => (Icon::Rejected, format!("rejected by {}", story.resolved_by)),
    }
}

/// Whether a decision was recorded, so steps 3 and 4 have something to show.
fn decided(story: &Story) -> bool {
    matches!(story.status, Status::Done | Status::Rejected)
}

fn steps(story: &Story) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(26.))
        .child(step(
            "1",
            format!("Asked by {}, {}", story.asked_by, story.asked_at),
            false,
            text_block(&story.text),
        ))
        .child(step(
            "2",
            "Code as reviewed".to_owned(),
            false,
            code_block(story),
        ))
        .child(step("3", decision_label(story), true, decision_body(story)))
        .child(step("4", changes_label(story), false, changes_body(story)))
}

/// One numbered step: the number, a label, then the body. An accented step
/// (the agent's decision) takes the agent colour.
fn step(number: &'static str, label: String, accent: bool, body: Div) -> Div {
    let colour = if accent {
        theme::agent()
    } else {
        theme::muted()
    };
    div()
        .flex()
        .gap(px(18.))
        .child(
            div()
                .w(px(16.))
                .flex_none()
                .text_color(if accent {
                    theme::agent()
                } else {
                    theme::faint()
                })
                .child(number),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(div().text_color(colour).child(label))
                .child(body),
        )
}

fn text_block(text: &str) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .text_color(theme::ink())
        .children(text.lines().map(|line| div().child(line.to_owned())))
}

fn decision_label(story: &Story) -> String {
    match story.status {
        Status::Done => format!("Done by {}, {}", story.resolved_by, story.resolved_at),
        Status::Rejected => format!("Rejected by {}, {}", story.resolved_by, story.resolved_at),
        Status::Pending | Status::Pulled => "Decision".to_owned(),
    }
}

fn decision_body(story: &Story) -> Div {
    if !decided(story) {
        return undecided();
    }
    let note = if story.note.trim().is_empty() {
        "No note was left.".to_owned()
    } else {
        story.note.clone()
    };
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .pl(px(16.))
        .border_l_1()
        .border_color(theme::agent())
        .text_color(theme::ink())
        .children(note.lines().map(|line| div().child(line.to_owned())))
}

fn changes_label(story: &Story) -> String {
    match story.diff.as_ref() {
        Some(diff) if decided(story) => format!(
            "What changed while resolving, +{} \u{2212}{}",
            diff.added, diff.removed
        ),
        _ => "What changed while resolving".to_owned(),
    }
}

fn changes_body(story: &Story) -> Div {
    if !decided(story) {
        return undecided();
    }
    match story.diff.as_ref() {
        Some(diff) => diff_block(diff),
        None => div()
            .text_color(theme::muted())
            .child("No diff was kept for this comment."),
    }
}

fn undecided() -> Div {
    div().text_color(theme::muted()).child("Not decided yet.")
}

/// The right panel: the comment's id and lane, a note when marking it seen
/// failed, then its timeline.
pub fn right(open: &Outcome) -> Div {
    let lane = match open.story.as_ref() {
        Ok(story) if !story.lane.is_empty() => format!("Lane {}", story.lane),
        _ => "No lane".to_owned(),
    };
    let mut panel = div()
        .flex()
        .flex_col()
        .gap(px(28.))
        .p(theme::PANEL_PADDING)
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme::ink())
                        .child(format!("Comment {}", open.id)),
                )
                .child(div().text_color(theme::muted()).child(lane)),
        );
    if let Some(error) = open.error.as_ref() {
        panel = panel.child(div().text_color(theme::rejected()).child(error.clone()));
    }
    panel.child(
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(div().text_color(theme::muted()).child("Timeline"))
            .children(open.timeline.iter().map(moment_row)),
    )
}

fn moment_row(moment: &Moment) -> Div {
    let colour = match moment.glyph {
        "◐" => theme::agent(),
        "✓" => theme::done(),
        "✗" => theme::rejected(),
        _ => theme::pending(),
    };
    div()
        .flex()
        .items_center()
        .gap(px(10.))
        .child(icon::status(moment.glyph, colour))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .text_color(theme::ink())
                .child(moment.label.clone()),
        )
        .child(
            div()
                .flex_none()
                .text_color(theme::muted())
                .child(moment.at.clone()),
        )
}

/// The screen the rail should light for an open comment: the queue when the
/// user came from it, the review otherwise.
pub fn rail_screen(back: Screen) -> Screen {
    if back == Screen::Queue {
        Screen::Queue
    } else {
        Screen::Diff
    }
}
