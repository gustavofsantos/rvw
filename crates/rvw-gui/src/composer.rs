//! The comment composer: a one-line field that opens under the last selected
//! diff line. The shell owns it; this file lays it out.

use gpui::prelude::*;
use gpui::{div, px, Div, Entity, Subscription};

use crate::text_input::TextInput;
use crate::theme;

/// An open composer: its field, the error of the last failed add, and the
/// subscription that turns the field's Submit and Cancel into shell calls.
pub struct Composer {
    pub input: Entity<TextInput>,
    pub error: Option<String>,
    _events: Subscription,
}

impl Composer {
    pub fn new(input: Entity<TextInput>, events: Subscription) -> Self {
        Self {
            input,
            error: None,
            _events: events,
        }
    }
}

/// The field with its caption on the right, the hint beneath, and the error
/// in red when the last add failed.
pub fn view(composer: &Composer, caption: &str) -> Div {
    let mut block = div()
        .flex()
        .flex_col()
        .mx(px(24.))
        .my(px(6.))
        .px(px(14.))
        .py(px(8.))
        .rounded(theme::CONTROL_RADIUS)
        .bg(theme::paper())
        .border_1()
        .border_color(theme::hairline_control())
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .child(div().flex_1().min_w(px(0.)).child(composer.input.clone()))
                .child(
                    div()
                        .flex_none()
                        .font_family(theme::CODE_FONT)
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
                .child("Enter adds it to your review, Esc cancels"),
        );
    if let Some(message) = &composer.error {
        block = block.child(
            div()
                .pt(px(4.))
                .truncate()
                .text_color(theme::rejected())
                .child(message.clone()),
        );
    }
    block
}
