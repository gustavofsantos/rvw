//! The key strip: the thirteen keys, each with the light it shows on the
//! screen it sits under. A key that is off explains itself on hover.

use gpui::prelude::*;
use gpui::{div, px, AnyView, App, Context, Div, FontWeight, Render, Stateful, Window};
use rvw_view::keys::{self, Context as KeyContext, Group, Key, Light, Screen};
use rvw_view::queue::Counters;

use crate::theme;

pub fn key_strip(screen: Screen, counters: &Counters) -> Div {
    let context = KeyContext {
        screen,
        composing: false,
        draft_count: counters.draft,
        comment_done: false,
        staged_count: 0,
        can_undo: false,
        can_move: true,
        can_open_file: false,
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
        strip = strip.child(key_cell(key, light, keys::hint(&context, key)));
    }
    strip
}

/// One key. `hint` is the text of its tooltip, when it is off and has one.
fn key_cell(key: Key, light: Light, hint: Option<&'static str>) -> Stateful<Div> {
    let (led, text, weight) = match light {
        Light::Off => (theme::selected_panel(), theme::faint(), FontWeight::NORMAL),
        Light::Available => (theme::ink(), theme::ink(), FontWeight::MEDIUM),
        Light::Needed => (theme::needs_you(), theme::ink(), FontWeight::MEDIUM),
    };
    let cell = div()
        .id(key.label())
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
                .font_family(theme::code_font())
                .text_size(px(theme::KEY_FONT_SIZE))
                .child(key.glyph()),
        )
        .child(div().text_size(px(theme::KEY_FONT_SIZE)).child(key.label()));
    match hint {
        Some(text) => cell.tooltip(move |_: &mut Window, cx: &mut App| -> AnyView {
            cx.new(|_| HintCard { text }).into()
        }),
        None => cell,
    }
}

/// The card a hint shows in: a small paper-coloured tooltip.
struct HintCard {
    text: &'static str,
}

impl Render for HintCard {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(10.))
            .py(px(6.))
            .rounded(theme::CONTROL_RADIUS)
            .border_1()
            .border_color(theme::hairline())
            .bg(theme::paper())
            .text_size(px(12.))
            .text_color(theme::text())
            .child(self.text)
    }
}

fn separator() -> Div {
    div().w(px(1.)).h(px(28.)).mx(px(8.)).bg(theme::hairline())
}
