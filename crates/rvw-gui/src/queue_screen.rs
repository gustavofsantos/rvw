//! The queue screen: state, lane and author filters on the left, the grouped
//! queue in the centre, and what the next pull would take on the right. Every
//! fact shown comes from `rvw_view`; this file only lays it out.

use std::collections::HashMap;

use gpui::prelude::*;
use gpui::{
    div, px, AnyElement, App, ClickEvent, Context, Div, ElementId, FontWeight, SharedString,
    Stateful, Window,
};
use rvw_core::types::{Comment, Review, Status};
use rvw_view::queue::{glyph_of, Filter, Group, Queue, Row, State};

use crate::data::{NextPull, Snapshot};
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
        .child(div().w(px(14.)).text_center().child(glyph.to_owned()))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .child(label.to_owned()),
        )
        .child(
            div()
                .font_family(theme::CODE_FONT)
                .text_color(theme::muted())
                .child(count.map_or(String::new(), |n| n.to_string())),
        )
}

pub fn centre(snapshot: &Snapshot, queue: &Queue) -> Stateful<Div> {
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
            .children(queue.groups.iter().map(|group| group_view(group, &by_id)))
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

fn group_view(group: &Group, by_id: &HashMap<&str, &Comment>) -> Div {
    div()
        .flex()
        .flex_col()
        .child(group_header(group.review.as_ref()))
        .children(group.rows.iter().map(|row| row_view(row, by_id)))
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
                        .font_family(theme::CODE_FONT)
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

fn row_view(row: &Row, by_id: &HashMap<&str, &Comment>) -> Div {
    let comment = by_id.get(row.id.as_str()).copied();
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

    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .h(px(42.))
        .border_b_1()
        .border_color(theme::hairline())
        .child(
            div()
                .w(px(14.))
                .flex_none()
                .text_center()
                .text_color(glyph_colour)
                .child(glyph),
        )
        .child(
            div()
                .w(px(30.))
                .flex_none()
                .font_family(theme::CODE_FONT)
                .text_size(px(12.))
                .text_color(theme::muted())
                .child(row.id.clone()),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .text_color(theme::ink())
                .child(row.title.clone()),
        )
        .child(
            div()
                .flex_none()
                .font_family(theme::CODE_FONT)
                .text_size(px(theme::CODE_FONT_SIZE))
                .text_color(theme::muted())
                .child(row.location.clone()),
        )
        .child(
            div()
                .flex_none()
                .w(px(120.))
                .text_color(label_colour)
                .child(row.state.label()),
        )
}

pub fn right(next: &NextPull) -> Div {
    let heading = if next.lane.is_empty() {
        "Next pull".to_owned()
    } else {
        format!("Next pull, lane {}", next.lane)
    };
    let summary = if next.count == 0 {
        "Nothing waits to be taken.".to_owned()
    } else {
        "The next pull takes these, and they leave the queue.".to_owned()
    };

    let mut panel = div()
        .flex()
        .flex_col()
        .gap(px(26.))
        .p(theme::PANEL_PADDING)
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(div().text_color(theme::muted()).child(heading))
                .child(
                    div()
                        .text_size(px(22.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme::ink())
                        .child(format!(
                            "{} {}",
                            next.count,
                            noun(next.count, "comment", "comments")
                        )),
                )
                .child(div().text_color(theme::muted()).child(summary)),
        );

    if next.elsewhere > 0 {
        panel = panel.child(
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .pt(px(14.))
                .border_t_1()
                .border_color(theme::hairline_control())
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme::ink())
                        .child(format!(
                            "{} {} in another lane",
                            next.elsewhere,
                            elsewhere_verb(next.elsewhere)
                        )),
                )
                .child(
                    div()
                        .text_color(theme::muted())
                        .child(elsewhere_detail(next)),
                ),
        );
    }
    panel
}

fn elsewhere_verb(count: usize) -> &'static str {
    if count == 1 {
        "comment waits"
    } else {
        "comments wait"
    }
}

fn elsewhere_detail(next: &NextPull) -> String {
    let lanes: Vec<&str> = next
        .elsewhere_lanes
        .iter()
        .map(|lane| {
            if lane.is_empty() {
                "no lane"
            } else {
                lane.as_str()
            }
        })
        .collect();
    // the notice only shows for a pinned pull, so a lane is always set here
    format!(
        "{} holds them. A pull pinned to {} will not take them.",
        lanes.join(", "),
        next.lane
    )
}

fn noun(count: usize, one: &'static str, many: &'static str) -> &'static str {
    if count == 1 {
        one
    } else {
        many
    }
}
