//! The outcome screen: one comment's story in four steps in the centre, its
//! timeline on the right, and the comments of its review on the left so the
//! reviewer can move between them. Every fact comes from `rvw_view`; this file
//! only lays them out.

use std::collections::HashMap;

use gpui::prelude::*;
use gpui::{
    div, px, ClickEvent, Context, Div, Entity, FontWeight, Rgba, SharedString, Stateful, Styled,
    Subscription, Window,
};
use rvw_core::types::{Comment, Status, Verdict, VerdictKind};
use rvw_view::keys::Screen;
use rvw_view::outcome::{self, Moment, Story};
use rvw_view::queue::{glyph_of, Row, State};

use crate::data::Snapshot;
use crate::icon::{self, Icon};
use crate::outcome_story::{code_block, diff_block};
use crate::shell::Shell;
use crate::text_area::TextArea;
use crate::theme;

/// One comment open on the outcome screen.
pub struct Outcome {
    pub id: String,
    /// The screen the user came from; Back returns to it.
    pub back: Screen,
    /// The story, or why the comment cannot be read.
    pub story: Result<Story, String>,
    pub timeline: Vec<Moment>,
    /// Why the verdict or the reload after it failed; shown in the right panel.
    pub error: Option<String>,
    /// The note field a reopen needs, open once Reopen… is chosen.
    pub reopen: Option<ReopenNote>,
    /// A verdict call is running; the verdict buttons wait for it.
    pub judging: bool,
}

impl Outcome {
    /// Whether the comment on screen awaits a verdict. False when the comment
    /// could not be read.
    pub fn awaits_verdict(&self) -> bool {
        self.story.as_ref().is_ok_and(|story| story.awaits_verdict)
    }
}

/// The reopen note field, and the subscriptions that turn its Cancel into a
/// close and keep the Reopen button in step with what is typed.
pub struct ReopenNote {
    pub input: Entity<TextArea>,
    _events: Subscription,
    _typing: Subscription,
}

impl ReopenNote {
    pub fn new(input: Entity<TextArea>, events: Subscription, typing: Subscription) -> Self {
        Self {
            input,
            _events: events,
            _typing: typing,
        }
    }
}

/// What the verdict step under the decision shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerdictStep {
    /// Nothing to judge: no decision awaits a verdict and none was given.
    Nothing,
    /// The verdict already given, with its words and note.
    Given,
    /// The Accept and Reopen… buttons.
    Choose,
    /// The reopen note field; its Reopen button needs a note first.
    Reopen { ready: bool },
}

/// Which verdict step the outcome shows. A given verdict always shows, as text.
/// Otherwise a waiting decision offers the buttons, or the note field once a
/// reopen is chosen. `note` is what the field holds, and `reopening` whether
/// the field is open.
#[must_use]
pub fn verdict_step(story: &Story, reopening: bool, note: &str) -> VerdictStep {
    if story.verdict.is_some() {
        VerdictStep::Given
    } else if !story.awaits_verdict {
        VerdictStep::Nothing
    } else if reopening {
        VerdictStep::Reopen {
            ready: reopen_ready(note),
        }
    } else {
        VerdictStep::Choose
    }
}

/// Whether a reopen can be sent: its note is not blank. A reopen needs what
/// is still wrong, because it becomes the text of the new comment.
#[must_use]
pub fn reopen_ready(note: &str) -> bool {
    !note.trim().is_empty()
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

    // The heading and summary scroll with the rows: a long summary must not
    // push the list off screen. Only Back stays fixed.
    div()
        .flex()
        .flex_col()
        .h_full()
        .min_h(px(0.))
        .py(px(20.))
        .child(back_link(cx))
        .child(
            div()
                .id("outcome-rows")
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(0.))
                .overflow_y_scroll()
                .child(
                    div()
                        .flex()
                        .flex_none()
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
                        .flex()
                        .flex_none()
                        .flex_col()
                        .gap(px(2.))
                        .children(rows),
                ),
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
pub fn centre(open: &Outcome, cx: &Context<Shell>) -> Stateful<Div> {
    let body = match &open.story {
        Ok(story) => div()
            .flex()
            .flex_col()
            .gap(px(26.))
            .child(header(story))
            .child(steps(story, open, cx)),
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
        .when_some(story.reopens.clone(), |d, id| {
            d.child(
                div()
                    .text_color(theme::muted())
                    .child(format!("Reopens {id}")),
            )
        })
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

fn steps(story: &Story, open: &Outcome, cx: &Context<Shell>) -> Div {
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
        .child(step(
            "3",
            decision_label(story),
            true,
            decision_step(story, open, cx),
        ))
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

/// The decision's note, then the verdict step under it.
fn decision_step(story: &Story, open: &Outcome, cx: &Context<Shell>) -> Div {
    let note = open
        .reopen
        .as_ref()
        .map(|reopen| reopen.input.read(cx).text().to_owned())
        .unwrap_or_default();
    let verdict = match verdict_step(story, open.reopen.is_some(), &note) {
        VerdictStep::Nothing => None,
        VerdictStep::Given => story.verdict.as_ref().map(given_verdict),
        VerdictStep::Choose => Some(choose_verdict(open.judging, cx)),
        VerdictStep::Reopen { ready } => open
            .reopen
            .as_ref()
            .map(|reopen| reopen_note(&reopen.input, ready, open.judging, cx)),
    };
    div()
        .flex()
        .flex_col()
        .gap(px(18.))
        .child(decision_body(story))
        .when_some(verdict, gpui::ParentElement::child)
}

/// The verdict a reviewer gave: its words, when, and the note if any.
fn given_verdict(verdict: &Verdict) -> Div {
    let (glyph, label) = outcome::verdict_mark(verdict);
    let colour = match verdict.kind {
        VerdictKind::Accepted => theme::done(),
        VerdictKind::Reopened => theme::agent(),
    };
    let line = div()
        .flex()
        .items_center()
        .gap(px(10.))
        .child(icon::status(glyph, colour))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .text_color(theme::ink())
                .child(label),
        )
        .child(
            div()
                .flex_none()
                .text_color(theme::muted())
                .child(outcome::short_time(&verdict.at)),
        );
    let note = verdict.note.as_deref().unwrap_or_default();
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(line)
        .when(!note.trim().is_empty(), |d| {
            d.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .pl(px(16.))
                    .border_l_1()
                    .border_color(colour)
                    .text_color(theme::ink())
                    .children(note.lines().map(|line| div().child(line.to_owned()))),
            )
        })
}

/// Accept and Reopen…, dim while a verdict call runs.
fn choose_verdict(judging: bool, cx: &Context<Shell>) -> Div {
    let accept = action_button("verdict-accept", "Accept", true, !judging).when(!judging, |d| {
        d.on_click(cx.listener(
            |this: &mut Shell, _: &ClickEvent, _: &mut Window, cx: &mut Context<Shell>| {
                this.judge_outcome(VerdictKind::Accepted, cx);
            },
        ))
    });
    let reopen =
        action_button("verdict-reopen", "Reopen\u{2026}", false, !judging).when(!judging, |d| {
            d.on_click(cx.listener(
                |this: &mut Shell, _: &ClickEvent, window: &mut Window, cx: &mut Context<Shell>| {
                    this.open_reopen(window, cx);
                },
            ))
        });
    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(div().text_color(theme::muted()).child("Your verdict"))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .child(accept)
                .child(reopen),
        )
}

/// The reopen note field, with Cancel and the Reopen button. Reopen stays dim
/// until the note is not blank.
fn reopen_note(input: &Entity<TextArea>, ready: bool, judging: bool, cx: &Context<Shell>) -> Div {
    let cancel = div()
        .id("reopen-cancel")
        .px(px(4.))
        .text_color(theme::ink())
        .when(!judging, |d| {
            d.on_click(cx.listener(
                |this: &mut Shell, _: &ClickEvent, _: &mut Window, cx: &mut Context<Shell>| {
                    this.close_reopen(cx);
                },
            ))
        })
        .child("Cancel");
    let go = action_button("reopen-go", "Reopen", true, ready && !judging).when(
        ready && !judging,
        |d| {
            d.on_click(cx.listener(
                |this: &mut Shell, _: &ClickEvent, _: &mut Window, cx: &mut Context<Shell>| {
                    this.judge_outcome(VerdictKind::Reopened, cx);
                },
            ))
        },
    );
    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(
            div()
                .text_color(theme::muted())
                .child("What is still wrong? It becomes the text of a new comment for the agent."),
        )
        .child(
            div()
                .pb(px(2.))
                .border_b_1()
                .border_color(theme::ink())
                .child(input.clone()),
        )
        .child(
            div()
                .flex()
                .items_center()
                .justify_end()
                .gap(px(16.))
                .child(cancel)
                .child(go),
        )
}

/// A button for the verdict: filled for the main choice, outlined for the
/// other. Dim and without a click when not enabled.
fn action_button(id: &'static str, label: &str, primary: bool, enabled: bool) -> Stateful<Div> {
    let button = div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .h(px(36.))
        .px(px(20.))
        .rounded(theme::CONTROL_RADIUS)
        .font_weight(FontWeight::MEDIUM)
        .child(label.to_owned());
    match (primary, enabled) {
        (true, true) => button
            .bg(theme::ink())
            .text_color(theme::paper())
            .cursor_pointer(),
        (true, false) => button
            .bg(theme::selected_panel())
            .text_color(theme::faint()),
        (false, true) => button
            .border_1()
            .border_color(theme::hairline_control())
            .text_color(theme::ink())
            .cursor_pointer(),
        (false, false) => button
            .border_1()
            .border_color(theme::hairline())
            .text_color(theme::faint()),
    }
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

/// The right panel: the comment's id and lane, the error of the last verdict
/// or reload when one failed, then its timeline.
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

#[cfg(test)]
mod tests {
    use rvw_core::types::{Actor, Verdict};

    use super::*;

    /// A story that awaits a verdict or not, with the verdict given or not.
    fn story(awaits_verdict: bool, verdict: Option<Verdict>) -> Story {
        Story {
            id: "r9".into(),
            title: "stamp every row".into(),
            location: "src/tx.rs:40".into(),
            status: Status::Done,
            lane: "feat".into(),
            asked_by: "gustavo".into(),
            asked_at: "10:41".into(),
            text: "stamp every row".into(),
            code: String::new(),
            start_line: 40,
            resolved_by: "claude".into(),
            resolved_at: "11:03".into(),
            note: String::new(),
            diff: None,
            awaits_verdict,
            verdict,
            reopens: None,
        }
    }

    fn given(kind: VerdictKind) -> Verdict {
        Verdict {
            kind,
            by: "gustavo".into(),
            actor: Actor::Human,
            at: "2026-10-12T11:20:00+00:00".into(),
            note: None,
            followup: None,
        }
    }

    fn outcome(story: Result<Story, String>) -> Outcome {
        Outcome {
            id: "r9".into(),
            back: Screen::Queue,
            story,
            timeline: Vec::new(),
            error: None,
            reopen: None,
            judging: false,
        }
    }

    #[test]
    fn a_decision_awaiting_a_verdict_offers_accept_and_reopen() {
        assert_eq!(
            verdict_step(&story(true, None), false, ""),
            VerdictStep::Choose
        );
    }

    #[test]
    fn reopening_shows_the_note_field_and_is_ready_only_with_a_note() {
        assert_eq!(
            verdict_step(&story(true, None), true, "   "),
            VerdictStep::Reopen { ready: false }
        );
        assert_eq!(
            verdict_step(&story(true, None), true, "the error path is untested"),
            VerdictStep::Reopen { ready: true }
        );
    }

    #[test]
    fn a_given_verdict_shows_in_place_of_the_buttons_and_the_field() {
        let judged = story(false, Some(given(VerdictKind::Accepted)));
        assert_eq!(verdict_step(&judged, false, ""), VerdictStep::Given);
        assert_eq!(verdict_step(&judged, true, "a note"), VerdictStep::Given);
    }

    #[test]
    fn a_decision_awaiting_nothing_shows_no_verdict_step() {
        assert_eq!(
            verdict_step(&story(false, None), false, ""),
            VerdictStep::Nothing
        );
        assert_eq!(
            verdict_step(&story(false, None), true, "a note"),
            VerdictStep::Nothing
        );
    }

    #[test]
    fn reopen_needs_a_note_that_is_not_blank() {
        assert!(!reopen_ready(""));
        assert!(!reopen_ready(" \n\t "));
        assert!(reopen_ready("  check the error path  "));
    }

    #[test]
    fn an_outcome_awaits_a_verdict_only_when_its_story_could_be_read_and_does() {
        assert!(outcome(Ok(story(true, None))).awaits_verdict());
        assert!(!outcome(Ok(story(false, None))).awaits_verdict());
        assert!(!outcome(Err("no such comment".into())).awaits_verdict());
    }
}
