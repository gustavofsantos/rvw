//! The key strip: the thirteen keys, each with the light it shows on the queue.

use gpui::prelude::*;
use gpui::{div, px, Div, FontWeight};
use rvw_view::keys::{self, Context, Group, Key, Light, Screen};
use rvw_view::queue::Counters;

use crate::theme;

pub fn key_strip(counters: &Counters) -> Div {
    let context = Context {
        screen: Screen::Queue,
        composing: false,
        draft_count: counters.draft,
        comment_done: false,
        staged_count: 0,
        can_undo: false,
    };

    let mut strip = div()
        .flex()
        .items_center()
        .h(theme::KEY_STRIP_HEIGHT)
        .px(px(24.))
        .border_t_1()
        .border_color(theme::hairline());
    let mut previous: Option<Group> = None;
    for (key, light) in Key::ALL.into_iter().zip(keys::lights(&context)) {
        if previous.is_some_and(|group| group != key.group()) {
            strip = strip.child(separator());
        }
        previous = Some(key.group());
        strip = strip.child(key_cell(key, light));
    }
    strip
}

fn key_cell(key: Key, light: Light) -> Div {
    let (led, text, weight) = match light {
        Light::Off => (theme::selected_panel(), theme::faint(), FontWeight::NORMAL),
        Light::Available => (theme::ink(), theme::ink(), FontWeight::MEDIUM),
        Light::Needed => (theme::needs_you(), theme::ink(), FontWeight::MEDIUM),
    };
    div()
        .flex()
        .flex_col()
        .flex_1()
        .items_center()
        .gap(px(5.))
        .text_color(text)
        .font_weight(weight)
        .child(div().size(px(6.)).rounded_full().bg(led))
        .child(
            div()
                .font_family(theme::CODE_FONT)
                .text_size(px(theme::KEY_FONT_SIZE))
                .child(key.glyph()),
        )
        .child(div().text_size(px(theme::KEY_FONT_SIZE)).child(key.label()))
}

fn separator() -> Div {
    div().w(px(1.)).h(px(28.)).mx(px(8.)).bg(theme::hairline())
}
