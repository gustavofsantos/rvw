//! The comment composer: a multi-line field that opens under the last selected
//! diff line, and under it a one-line "Points at" field. The shell owns it;
//! this file lays it out.

use gpui::prelude::*;
use gpui::{Context, Div, Entity, Subscription, div, px};
use rvw_core::types::Location;
use rvw_view::points_at::parse_points_at;

use crate::shell::Shell;
use crate::text_area::TextArea;
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme;

/// The placeholder of the "Points at" field on a file outside the change.
pub const CONTEXT_PLACEHOLDER: &str = "e.g. src/service.rs:90-96";

/// The label of the "Points at" field, drawn in front of it.
const POINTS_LABEL: &str = "Points at (file:lines)";

/// An open composer: its field, the error of the last failed add, the "Points
/// at" field when the composer makes a new comment, and the subscription that
/// turns the field's Submit and Cancel into shell calls.
pub struct Composer {
    pub input: Entity<TextArea>,
    pub error: Option<String>,
    /// The id of the comment being edited; `None` for a new comment.
    pub editing: Option<String>,
    /// The "Points at" field. `None` when editing: an edit keeps the
    /// comment's reference, which core does not change.
    pub points: Option<PointsField>,
    _events: Subscription,
}

/// The one-line "Points at" field and the subscriptions that keep it live.
pub struct PointsField {
    pub input: Entity<TextInput>,
    _events: Subscription,
    /// Redraws the window as the field is typed in, so its problem line is
    /// current; the field itself redraws only itself.
    _typing: Subscription,
}

impl Composer {
    pub fn new(
        input: Entity<TextArea>,
        events: Subscription,
        editing: Option<String>,
        points: Option<PointsField>,
    ) -> Self {
        Self {
            input,
            error: None,
            editing,
            points,
            _events: events,
        }
    }
}

impl PointsField {
    /// A field for a comment on a file outside the change shows the example;
    /// on a changed file it stays empty.
    pub fn new(outside_change: bool, cx: &mut Context<Shell>) -> Self {
        let placeholder = if outside_change {
            CONTEXT_PLACEHOLDER
        } else {
            ""
        };
        let input = cx.new(|cx| TextInput::new(placeholder, cx));
        let events = cx.subscribe(
            &input,
            |this: &mut Shell, _, event: &TextInputEvent, cx: &mut Context<Shell>| {
                match event {
                    TextInputEvent::Submit => this.submit_comment(cx),
                    TextInputEvent::Cancel => this.composer = None,
                }
                cx.notify();
            },
        );
        let typing = cx.observe(&input, |_, _, cx: &mut Context<Shell>| cx.notify());
        Self {
            input,
            _events: events,
            _typing: typing,
        }
    }
}

/// What the "Points at" field holds: `None` when it is blank, the location
/// when it parses, or the message why it does not.
///
/// # Errors
///
/// Fails with the one-line message of `parse_points_at` when the text is not
/// a `FILE:N` or `FILE:N-M` reference.
pub fn points_spec(text: &str) -> Result<Option<Location>, String> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    parse_points_at(text).map(Some)
}

/// The "Points at" row as drawn: its field, and why the text cannot be used.
pub struct PointsRow<'a> {
    pub field: &'a Entity<TextInput>,
    pub problem: Option<String>,
}

/// The field with its caption on the right, the points row under it, the hint
/// beneath, and the error in red when the last add failed. A bad "Points at"
/// text is shown under its own field and turns the hint into the reason the
/// comment cannot be added yet.
pub fn view(
    input: &Entity<TextArea>,
    error: Option<&str>,
    caption: &str,
    points: Option<PointsRow<'_>>,
) -> Div {
    let refused = points.as_ref().is_some_and(|row| row.problem.is_some());
    let mut block = div()
        .flex()
        .flex_col()
        .mx(px(24.))
        .px(px(14.))
        .py(px(8.))
        .rounded(theme::CONTROL_RADIUS)
        .bg(theme::paper())
        .border_1()
        .border_color(theme::hairline_control())
        .child(
            div()
                .flex()
                .items_start()
                .gap(px(12.))
                .child(div().flex_1().min_w(px(0.)).child(input.clone()))
                .child(
                    div()
                        .flex_none()
                        .font_family(theme::code_font())
                        .text_size(px(theme::CODE_FONT_SIZE))
                        .text_color(theme::faint())
                        .child(caption.to_owned()),
                ),
        );
    if let Some(row) = points {
        block = block.child(points_line(&row));
    }
    block = block.child(
        div()
            .pt(px(4.))
            .text_size(px(theme::KEY_FONT_SIZE + 0.5))
            .text_color(theme::faint())
            .child(if refused {
                "Fix Points at to add it  \u{b7}  Esc cancels  \u{b7}  Return for a new line"
                    .to_owned()
            } else {
                format!(
                    "{} adds it to your review  \u{b7}  Esc cancels  \u{b7}  Return for a new line",
                    theme::submit_chord()
                )
            }),
    );
    if let Some(message) = error {
        block = block.child(
            div()
                .pt(px(4.))
                .truncate()
                .text_color(theme::rejected())
                .child(message.to_owned()),
        );
    }
    block
}

/// The "Points at" label, its field, and the problem under it when there is one.
fn points_line(row: &PointsRow<'_>) -> Div {
    let mut line = div().flex().flex_col().gap(px(2.)).pt(px(6.)).child(
        div()
            .flex()
            .items_center()
            .gap(px(12.))
            .child(
                div()
                    .flex_none()
                    .font_family(theme::code_font())
                    .text_size(px(theme::CODE_FONT_SIZE))
                    .text_color(theme::faint())
                    .child(POINTS_LABEL),
            )
            .child(div().flex_1().min_w(px(0.)).child(row.field.clone())),
    );
    if let Some(problem) = row.problem.as_ref() {
        line = line.child(
            div()
                .truncate()
                .text_size(px(theme::KEY_FONT_SIZE + 0.5))
                .text_color(theme::rejected())
                .child(problem.clone()),
        );
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blank_points_field_means_no_reference() {
        assert_eq!(points_spec(""), Ok(None));
        assert_eq!(points_spec("   "), Ok(None));
    }

    #[test]
    fn a_points_field_that_parses_is_the_location() {
        assert_eq!(
            points_spec(" src/service.rs:90-96 "),
            Ok(Some(Location {
                file: "src/service.rs".into(),
                start_line: 90,
                end_line: 96,
            }))
        );
    }

    #[test]
    fn a_points_field_that_does_not_parse_says_why() {
        assert_eq!(
            points_spec("src/service.rs"),
            Err(parse_points_at("src/service.rs").expect_err("no colon"))
        );
    }

    #[test]
    fn the_context_placeholder_is_the_example_from_the_design() {
        assert_eq!(CONTEXT_PLACEHOLDER, "e.g. src/service.rs:90-96");
    }
}
