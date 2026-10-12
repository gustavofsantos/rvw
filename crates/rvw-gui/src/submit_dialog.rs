//! The submit dialog: a centred card over the window that sends the draft
//! comments as one review, with a verdict and a summary for the agent. The
//! shell owns the open dialog and its calls; this file lays it out and maps
//! the verdict to the service's decision strings.

use gpui::prelude::*;
use gpui::{
    actions, div, px, rgba, App, BoxShadow, ClickEvent, Context, Div, Entity, FontWeight,
    KeyBinding, SharedString, Stateful, Subscription, Window,
};
use rvw_view::queue::{Filter, Row, State};

use crate::data::Snapshot;
use crate::queue_screen::noun;
use crate::shell::Shell;
use crate::text_input::TextInput;
use crate::theme;
use crate::verdict::Verdict;

actions!(rvw_submit, [OpenSubmit]);

/// Binds ⇧⌘R (Ctrl-Shift-R off macOS) to open the dialog on every screen.
/// Call once at startup.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("secondary-shift-r", OpenSubmit, None)]);
}

/// An open dialog: the verdict, the summary field, the error of the last
/// refused submit, and the subscription that turns the field's Cancel into a
/// close.
pub struct SubmitDialog {
    pub verdict: Verdict,
    pub summary: Entity<TextInput>,
    pub error: Option<String>,
    _events: Subscription,
}

impl SubmitDialog {
    pub fn new(verdict: Verdict, summary: Entity<TextInput>, events: Subscription) -> Self {
        Self {
            verdict,
            summary,
            error: None,
            _events: events,
        }
    }
}

/// The draft comments: the rows the right panel shows under "In your draft".
pub fn draft_rows(snapshot: &Snapshot) -> Vec<Row> {
    snapshot
        .view(&Filter {
            state: Some(State::Draft),
            ..Filter::default()
        })
        .groups
        .into_iter()
        .flat_map(|group| group.rows)
        .collect()
}

/// The scrim and the card over the window. `lane` is the reviewer's lane, or
/// empty for none.
pub fn overlay(dialog: &SubmitDialog, snapshot: &Snapshot, lane: &str, cx: &Context<Shell>) -> Div {
    let rows = draft_rows(snapshot);
    div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .occlude()
        .flex()
        .items_center()
        .justify_center()
        .bg(rgba(0x1616_1429))
        .child(card(
            dialog,
            &rows,
            snapshot.queue.counters.with_agent,
            lane,
            cx,
        ))
}

fn card(
    dialog: &SubmitDialog,
    rows: &[Row],
    with_agent: usize,
    lane: &str,
    cx: &Context<Shell>,
) -> Div {
    div()
        .flex()
        .flex_col()
        .w(px(720.))
        .rounded(px(14.))
        .bg(theme::paper())
        .border_1()
        .border_color(theme::hairline_strong())
        .shadow(vec![BoxShadow {
            color: rgba(0x1616_1426).into(),
            offset: gpui::point(px(0.), px(12.)),
            blur_radius: px(40.),
            spread_radius: px(0.),
        }])
        .overflow_hidden()
        .child(header())
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(26.))
                .px(px(28.))
                .py(px(24.))
                .child(verdict_section(dialog.verdict, cx))
                .child(summary_section(dialog))
                .child(list_section(rows))
                .when(with_agent > 0, |d| d.child(with_agent_note(with_agent)))
                .when_some(dialog.error.clone(), |d, message| {
                    d.child(
                        div()
                            .truncate()
                            .text_color(theme::rejected())
                            .child(message),
                    )
                }),
        )
        .child(footer(lane, cx))
}

fn header() -> Div {
    div()
        .flex()
        .items_center()
        .h(px(56.))
        .px(px(28.))
        .border_b_1()
        .border_color(theme::hairline())
        .child(
            div()
                .flex_1()
                .text_size(px(16.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme::ink())
                .child("Submit review"),
        )
        .child(key_hint("esc"))
}

fn verdict_section(chosen: Verdict, cx: &Context<Shell>) -> Div {
    let radios = Verdict::ALL.map(|verdict| radio(verdict, chosen, cx));
    div()
        .flex()
        .flex_col()
        .child(label("Verdict"))
        .child(div().flex().gap(px(32.)).children(radios))
        .child(
            div()
                .mt(px(8.))
                .text_color(theme::muted())
                .child(chosen.hint()),
        )
}

fn radio(verdict: Verdict, chosen: Verdict, cx: &Context<Shell>) -> Stateful<Div> {
    let on = verdict == chosen;
    div()
        .id(SharedString::from(format!("verdict-{}", verdict.id())))
        .flex()
        .items_center()
        .gap(px(10.))
        .on_click(cx.listener(
            move |this: &mut Shell, _: &ClickEvent, _: &mut Window, cx: &mut Context<Shell>| {
                this.pick_verdict(verdict, cx);
            },
        ))
        .child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .flex_none()
                .size(px(14.))
                .rounded_full()
                .border_1()
                .border_color(if on { theme::ink() } else { theme::faint() })
                .when(on, |d| {
                    d.child(div().size(px(6.)).rounded_full().bg(theme::ink()))
                }),
        )
        .child(
            div()
                .text_color(if on { theme::ink() } else { theme::text() })
                .font_weight(if on {
                    FontWeight::MEDIUM
                } else {
                    FontWeight::NORMAL
                })
                .child(verdict.label()),
        )
}

fn summary_section(dialog: &SubmitDialog) -> Div {
    div()
        .flex()
        .flex_col()
        .child(label("Summary for the agent"))
        .child(
            div()
                .pb(px(2.))
                .border_b_1()
                .border_color(theme::ink())
                .child(dialog.summary.clone()),
        )
}

/// The draft comments with their glyph, id, title and location.
fn list_section(rows: &[Row]) -> Div {
    let items: Vec<Div> = rows.iter().map(review_row).collect();
    div()
        .flex()
        .flex_col()
        .child(label(&format!("Comments in this review, {}", rows.len())))
        .child(
            div()
                .id("submit-comments")
                .flex()
                .flex_col()
                .max_h(px(200.))
                .overflow_y_scroll()
                .children(items),
        )
}

fn review_row(row: &Row) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(14.))
        .h(px(38.))
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
                .w(px(40.))
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
                .text_size(px(12.))
                .text_color(theme::faint())
                .child(row.location.clone()),
        )
}

fn with_agent_note(count: usize) -> Div {
    let (verb, pronoun) = if count == 1 {
        ("is", "it")
    } else {
        ("are", "them")
    };
    div().text_color(theme::muted()).child(format!(
        "{count} {} {verb} still with claude. The next pull will not include {pronoun} again.",
        noun(count, "comment", "comments")
    ))
}

/// The lane note, Keep as draft, and the Submit button.
fn footer(lane: &str, cx: &Context<Shell>) -> Div {
    let handed = if lane.is_empty() {
        "Handed to every lane. Taken once.".to_owned()
    } else {
        format!("Handed to lane {lane}. Taken once.")
    };
    let keep = div()
        .id("keep-as-draft")
        .px(px(4.))
        .text_color(theme::ink())
        .on_click(cx.listener(
            |this: &mut Shell, _: &ClickEvent, _: &mut Window, cx: &mut Context<Shell>| {
                this.close_submit(cx);
            },
        ))
        .child("Keep as draft");
    let submit = div()
        .id("submit-review-go")
        .flex()
        .items_center()
        .gap(px(8.))
        .h(px(36.))
        .px(px(20.))
        .rounded(theme::CONTROL_RADIUS)
        .bg(theme::ink())
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme::paper())
        .on_click(cx.listener(
            |this: &mut Shell, _: &ClickEvent, _: &mut Window, cx: &mut Context<Shell>| {
                this.send_submit(cx);
            },
        ))
        .child("Submit")
        .child(key_hint("⇧⌘R"));
    div()
        .flex()
        .items_center()
        .gap(px(16.))
        .h(px(68.))
        .px(px(28.))
        .border_t_1()
        .border_color(theme::hairline())
        .child(div().text_color(theme::muted()).child(handed))
        .child(div().flex_1())
        .child(keep)
        .child(submit)
}

fn label(text: &str) -> Div {
    div()
        .mb(px(10.))
        .text_color(theme::muted())
        .child(text.to_owned())
}

fn key_hint(text: &str) -> Div {
    div()
        .font_family(theme::CODE_FONT)
        .text_size(px(theme::KEY_FONT_SIZE))
        .text_color(theme::faint())
        .child(text.to_owned())
}
