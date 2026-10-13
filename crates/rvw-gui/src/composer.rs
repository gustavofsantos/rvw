//! The comment composer: a multi-line field that opens under the last selected
//! diff line. The shell owns it; this file lays it out.

use gpui::prelude::*;
use gpui::{div, px, Div, Entity, Subscription};

use crate::text_area::TextArea;
use crate::theme;

/// An open composer: its field, the error of the last failed add, and the
/// subscription that turns the field's Submit and Cancel into shell calls.
pub struct Composer {
    pub input: Entity<TextArea>,
    pub error: Option<String>,
    /// The id of the comment being edited; `None` for a new comment.
    pub editing: Option<String>,
    _events: Subscription,
}

impl Composer {
    pub fn new(input: Entity<TextArea>, events: Subscription, editing: Option<String>) -> Self {
        Self {
            input,
            error: None,
            editing,
            _events: events,
        }
    }
}

/// The field with its caption on the right, the hint beneath, and the error
/// in red when the last add failed.
pub fn view(input: &Entity<TextArea>, error: Option<&str>, caption: &str) -> Div {
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
        )
        .child(
            div()
                .pt(px(4.))
                .text_size(px(theme::KEY_FONT_SIZE + 0.5))
                .text_color(theme::faint())
                .child(format!(
                    "{} adds it to your review  \u{b7}  Esc cancels  \u{b7}  Return for a new line",
                    theme::submit_chord()
                )),
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
