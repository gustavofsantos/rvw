//! The queue screen: state, lane and author filters on the left, the grouped
//! queue in the centre, and what the next pull would take on the right. Every
//! fact shown comes from `rvw_view`; this file only lays it out.

use std::collections::HashMap;

use gpui::prelude::*;
use gpui::{
    div, px, AnyElement, App, ClickEvent, Context, Div, ElementId, FontWeight, Rgba, SharedString,
    Stateful, Window,
};
use rvw_core::types::{Comment, Review, Status};
use rvw_view::handoff::{CommentItem, LanePreview, Preview, ReviewItem};
use rvw_view::queue::{glyph_of, Filter, Group, Queue, Row, State};

use crate::data::Snapshot;
use crate::icon;
use crate::shell::Shell;
use crate::theme;

pub fn left(snapshot: &Snapshot, filter: &Filter, cx: &mut Context<Shell>) -> Div {
    let counters = snapshot.queue.counters;
    let mut states = vec![
        (State::WaitingOnYou, counters.waiting_on_you),
        (State::WithAgent, counters.with_agent),
        (State::NotYetTaken, counters.not_yet_taken),
        (State::Closed, counters.closed),
    ];
    if counters.draft > 0 {
        states.push((State::Draft, counters.draft));
    }
    let state_rows: Vec<AnyElement> = states
        .into_iter()
        .map(|(state, count)| {
            let on_click = cx.listener(
                move |this: &mut Shell, _: &ClickEvent, _: &mut Window, cx: &mut Context<Shell>| {
                    this.pick_state(state, cx);
                },
            );
            filter_row(
                state.label(),
                state.glyph(),
                state.label(),
                Some(count),
                filter.state == Some(state),
                on_click,
            )
            .into_any_element()
        })
        .collect();

    let lane_rows = facet_rows(
        &snapshot.facets.lanes,
        filter.lane.as_deref(),
        "lane",
        cx,
        Shell::pick_lane,
    );
    let author_rows = facet_rows(
        &snapshot.facets.authors,
        filter.author.as_deref(),
        "author",
        cx,
        Shell::pick_author,
    );

    div()
        .flex()
        .flex_col()
        .gap(px(22.))
        .py(px(20.))
        .child(section("State", state_rows))
        .child(section("Lane", lane_rows))
        .child(section("Author", author_rows))
}

/// One row per name, each toggling its filter. `pick` runs on the shell.
fn facet_rows(
    names: &[String],
    selected: Option<&str>,
    kind: &'static str,
    cx: &mut Context<Shell>,
    pick: fn(&mut Shell, &str, &mut Context<Shell>),
) -> Vec<AnyElement> {
    if names.is_empty() {
        return vec![div()
            .px(px(24.))
            .text_color(theme::faint())
            .child("None yet")
            .into_any_element()];
    }
    names
        .iter()
        .map(|name| {
            let owned = name.clone();
            let on_click = cx.listener(
                move |this: &mut Shell, _: &ClickEvent, _: &mut Window, cx: &mut Context<Shell>| {
                    pick(this, &owned, cx);
                },
            );
            let id = SharedString::from(format!("{kind}-{name}"));
            filter_row(
                id,
                "",
                name,
                None,
                selected == Some(name.as_str()),
                on_click,
            )
            .into_any_element()
        })
        .collect()
}

fn section(title: &'static str, rows: Vec<AnyElement>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(
            div()
                .px(px(24.))
                .pb(px(6.))
                .text_color(theme::muted())
                .child(title),
        )
        .children(rows)
}

fn filter_row(
    id: impl Into<ElementId>,
    glyph: &str,
    label: &str,
    count: Option<usize>,
    selected: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(10.))
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
        .child(icon::status(glyph, theme::muted()))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .child(label.to_owned()),
        )
        .child(
            div()
                .font_family(theme::code_font())
                .text_color(theme::muted())
                .child(count.map_or(String::new(), |n| n.to_string())),
        )
}

pub fn centre(snapshot: &Snapshot, queue: &Queue, cx: &Context<Shell>) -> Stateful<Div> {
    let by_id: HashMap<&str, &Comment> = snapshot
        .comments
        .iter()
        .map(|c| (c.id.as_str(), c))
        .collect();

    let body = if queue.groups.is_empty() {
        let message = if snapshot.queue.groups.is_empty() {
            "Nothing in the queue. Comments you add will appear here."
        } else {
            "No comments match this filter."
        };
        div()
            .text_color(theme::muted())
            .child(message)
            .into_any_element()
    } else {
        div()
            .flex()
            .flex_col()
            .gap(px(28.))
            .children(
                queue
                    .groups
                    .iter()
                    .map(|group| group_view(group, &by_id, cx)),
            )
            .into_any_element()
    };

    div()
        .id("queue-centre")
        .flex()
        .flex_col()
        .flex_1()
        .min_w(px(0.))
        .p(px(24.))
        .gap(px(20.))
        .bg(theme::paper())
        .overflow_y_scroll()
        .child(
            div()
                .text_size(px(22.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme::ink())
                .child("Queue"),
        )
        .child(body)
}

fn group_view(group: &Group, by_id: &HashMap<&str, &Comment>, cx: &Context<Shell>) -> Div {
    div()
        .flex()
        .flex_col()
        .child(group_header(group.review.as_ref()))
        .children(group.rows.iter().map(|row| row_view(row, by_id, cx)))
}

fn group_header(review: Option<&Review>) -> Div {
    let header = div()
        .flex()
        .items_center()
        .gap(px(10.))
        .pb(px(10.))
        .border_b_1()
        .border_color(theme::ink());
    match review {
        None => header.child(
            div()
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme::ink())
                .child("Without a review"),
        ),
        Some(review) => {
            let summary = if review.summary.trim().is_empty() {
                "(no summary)"
            } else {
                review.summary.as_str()
            };
            let who: Vec<&str> = [review.author.as_deref(), review.lane.as_deref()]
                .into_iter()
                .flatten()
                .filter(|s| !s.is_empty())
                .collect();
            header
                .child(
                    div()
                        .font_family(theme::code_font())
                        .text_size(px(theme::CODE_FONT_SIZE))
                        .text_color(theme::muted())
                        .child(review.id.clone()),
                )
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme::ink())
                        .child(summary.to_owned()),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .text_color(theme::muted())
                        .child(review.decision.clone()),
                )
                .child(div().text_color(theme::muted()).child(who.join(" · ")))
        }
    }
}

/// One comment's row. A click opens its outcome screen.
fn row_view(row: &Row, by_id: &HashMap<&str, &Comment>, cx: &Context<Shell>) -> Stateful<Div> {
    let comment = by_id.get(row.id.as_str()).copied();
    let id = row.id.clone();
    let on_click = cx.listener(
        move |this: &mut Shell, _: &ClickEvent, _: &mut Window, cx: &mut Context<Shell>| {
            this.open_outcome(&id, cx);
        },
    );
    let glyph = comment.map_or(row.state.glyph(), |c| glyph_of(c, row.state));
    let is_rejected = comment.is_some_and(|c| c.status == Status::Rejected);
    let glyph_colour = match row.state {
        State::Draft | State::NotYetTaken => theme::pending(),
        State::WithAgent => theme::agent(),
        State::WaitingOnYou => theme::needs_you(),
        State::Closed if is_rejected => theme::rejected(),
        State::Closed => theme::done(),
    };
    let label_colour = match row.state {
        State::WaitingOnYou => theme::ink(),
        State::WithAgent => theme::agent(),
        State::Draft | State::NotYetTaken | State::Closed => theme::muted(),
    };

    // Two lines, so the title has the row's width: the glyph, the title and
    // the state on the first; the id and the location, faint, on the second.
    // Five columns on one line left the title no room.
    let mut meta = if row.location.is_empty() {
        row.id.clone()
    } else {
        format!("{}  ·  {}", row.id, row.location)
    };
    if !row.author.is_empty() {
        meta = format!("{meta}  ·  {}", row.author);
    }
    // A closed row says how its decision was judged; a follow-up says what it
    // reopens. Both are small and muted, after the faint meta line.
    let verdict = match row.state {
        State::Closed => row.verdict,
        _ => None,
    };
    div()
        .id(SharedString::from(format!("queue-row-{}", row.id)))
        .flex()
        .items_start()
        .gap(px(12.))
        .py(px(9.))
        .border_b_1()
        .border_color(theme::hairline())
        .cursor_pointer()
        .on_click(on_click)
        .child(icon::status(glyph, glyph_colour).mt(px(2.)))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .flex_1()
                .min_w(px(0.))
                .child(
                    div()
                        .truncate()
                        .text_color(theme::ink())
                        .child(row.title.clone()),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .min_w(px(0.))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .truncate()
                                .font_family(theme::code_font())
                                .text_size(px(12.))
                                .text_color(theme::faint())
                                .child(meta),
                        )
                        .children(verdict.map(|kind| tag(kind.as_str().to_owned())))
                        .children(row.reopens.as_ref().map(|id| tag(format!("reopens {id}")))),
                ),
        )
        .child(
            div()
                .flex_none()
                .text_color(label_colour)
                .child(row.state.label()),
        )
}

/// The right panel: what a pull would take, lane by lane, and what waits in
/// other lanes. The heading and the notice stay in place; the lanes scroll.
pub fn right(preview: &Preview) -> Div {
    let pinned = preview.pinned.as_str();
    let taken = preview
        .lanes
        .iter()
        .filter(|lane| pinned.is_empty() || lane.lane == pinned)
        .filter(|lane| !lane.is_empty())
        .map(|lane| lane_view(lane, theme::ink()));
    let waiting: Vec<&LanePreview> = preview
        .lanes
        .iter()
        .filter(|lane| !pinned.is_empty() && lane.lane != pinned && !lane.is_empty())
        .collect();

    let mut lanes = div().flex().flex_col().gap(px(24.)).children(taken);
    if !waiting.is_empty() {
        lanes = lanes.child(
            div()
                .flex()
                .flex_col()
                .gap(px(16.))
                .pt(px(14.))
                .border_t_1()
                .border_color(theme::hairline_control())
                .child(
                    div()
                        .text_color(theme::muted())
                        .child("Waits in other lanes"),
                )
                .children(
                    waiting
                        .into_iter()
                        .map(|lane| lane_view(lane, theme::muted())),
                ),
        );
    }

    div()
        .flex()
        .flex_col()
        .h_full()
        .p(theme::PANEL_PADDING)
        .gap(px(18.))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_none()
                .gap(px(8.))
                .child(
                    div()
                        .text_size(px(22.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme::ink())
                        .child("Next pull"),
                )
                .child(div().text_color(theme::text()).child(preview.notice())),
        )
        .child(
            // A long list of handoffs scrolls here, not off the panel.
            div()
                .id("next-pull-lanes")
                .flex_1()
                .min_h(px(0.))
                .overflow_y_scroll()
                .child(lanes),
        )
}

/// One lane: its label and count, then its reviews and standalone comments.
/// `colour` is for the text; a lane waiting elsewhere is drawn muted.
fn lane_view(lane: &LanePreview, colour: Rgba) -> Div {
    let count = lane.handoffs();
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(px(8.))
                .pb(px(6.))
                .border_b_1()
                .border_color(theme::hairline())
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .truncate()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(colour)
                        .child(lane.label()),
                )
                .child(
                    div()
                        .flex_none()
                        .text_color(theme::muted())
                        .child(format!("{count} {}", noun(count, "handoff", "handoffs"))),
                ),
        )
        .children(lane.reviews.iter().map(|review| review_row(review, colour)))
        .children(
            lane.comments
                .iter()
                .map(|comment| comment_row(comment, colour)),
        )
}

/// `rv2 comment · Looks good · 2 comments`, on one line.
fn review_row(review: &ReviewItem, colour: Rgba) -> Div {
    let comments = format!(
        "{} {}",
        review.comments,
        noun(review.comments, "comment", "comments")
    );
    div().truncate().text_color(colour).child(format!(
        "{} {} · {} · {}",
        review.id, review.decision, review.title, comments
    ))
}

/// `r9 src/a.rs:3-5` with its tags, then the title under it.
fn comment_row(comment: &CommentItem, colour: Rgba) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .min_w(px(0.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .min_w(px(0.))
                .child(
                    div()
                        .flex_none()
                        .font_family(theme::code_font())
                        .text_size(px(12.))
                        .text_color(theme::muted())
                        .child(comment.id.clone()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .truncate()
                        .font_family(theme::code_font())
                        .text_size(px(12.))
                        .text_color(theme::faint())
                        .child(comment.location.clone()),
                )
                .children(comment.draft.then(|| tag("your draft".to_owned())))
                .children(
                    comment
                        .reopens
                        .as_ref()
                        .map(|id| tag(format!("reopens {id}"))),
                ),
        )
        .child(
            div()
                .truncate()
                .text_color(colour)
                .child(comment.title.clone()),
        )
}

/// A small muted word after a row's text: a verdict, a follow-up's origin.
fn tag(text: String) -> Div {
    div()
        .flex_none()
        .text_size(px(12.))
        .text_color(theme::muted())
        .child(text)
}

pub(crate) fn noun(count: usize, one: &'static str, many: &'static str) -> &'static str {
    if count == 1 {
        one
    } else {
        many
    }
}
