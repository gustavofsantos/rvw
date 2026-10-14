//! The key strip: the thirteen keys, each with the light it shows on the
//! screen it sits under. A key that is off explains itself on hover.

use gpui::prelude::*;
use gpui::{div, px, AnyView, App, Context, Div, FontWeight, Render, Stateful, Window};
use rvw_view::keys::{self, Context as KeyContext, Group, Key, Light};

use crate::theme;

/// The chords of rvw that act while a terminal has the keyboard, each with
/// what it does, spelled for the platform. Every other key goes to the
/// program. On macOS the command key never reaches a program, so the chords
/// are the usual ones; elsewhere they are the Ctrl-Shift forms, so Ctrl alone
/// stays the shell's.
pub fn terminal_chords(is_mac: bool) -> Vec<(String, &'static str)> {
    let keys: [(&str, &str, &'static str); 12] = [
        ("\u{2318}J", "J", "Hide dock"),
        ("\u{2318}T", "T", "New"),
        ("\u{2318}W", "W", "Close"),
        ("\u{2318}C", "C", "Copy"),
        ("\u{2318}F", "F", "Find"),
        ("\u{2318}1-9", "1-9", "Switch"),
        ("\u{2318}P", "P", "Find file"),
        ("\u{2318}K", "K", "Actions"),
        ("\u{2318}O", "O", "Workspaces"),
        ("\u{21e7}\u{2318}R", "R", "Submit"),
        ("\u{2318}Z", "Z", "Restore"),
        ("\u{2318}Q", "Q", "Quit"),
    ];
    keys.into_iter()
        .map(|(mac, letter, label)| {
            let chord = if is_mac {
                mac.to_owned()
            } else {
                format!("Ctrl+Shift+{letter}")
            };
            (chord, label)
        })
        .collect()
}

/// The strip while a terminal has the keyboard: it says so, and lists the
/// only chords that are not the terminal's.
fn terminal_strip() -> Div {
    let mut strip = div()
        .flex()
        .items_center()
        .gap(px(20.))
        .h(theme::KEY_STRIP_HEIGHT)
        .px(px(24.))
        .border_t_1()
        .border_color(theme::hairline())
        .child(
            div()
                .text_size(px(theme::KEY_FONT_SIZE))
                .text_color(theme::muted())
                .child("The terminal receives every key except"),
        );
    for (chord, label) in terminal_chords(cfg!(target_os = "macos")) {
        strip = strip.child(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .text_size(px(theme::KEY_FONT_SIZE))
                .child(
                    div()
                        .font_family(theme::code_font())
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme::ink())
                        .child(chord),
                )
                .child(div().text_color(theme::muted()).child(label)),
        );
    }
    strip
}

pub fn key_strip(context: &KeyContext, terminal_focused: bool) -> Div {
    if terminal_focused {
        return terminal_strip();
    }
    let mut strip = div()
        .flex()
        .items_center()
        .h(theme::KEY_STRIP_HEIGHT)
        .px(px(24.))
        .border_t_1()
        .border_color(theme::hairline());
    let mut previous: Option<Group> = None;
    for (key, light) in Key::ALL.into_iter().zip(keys::lights(context)) {
        if previous.is_some_and(|group| group != key.group()) {
            strip = strip.child(separator());
        }
        previous = Some(key.group());
        strip = strip.child(key_cell(key, light, keys::hint(context, key)));
    }
    strip
}

/// One key. `hint` is the text of its tooltip, when it is off and has one.
fn key_cell(key: Key, light: Light, hint: Option<&'static str>) -> Stateful<Div> {
    let (text, weight) = match light {
        Light::Off => (theme::faint(), FontWeight::NORMAL),
        Light::Available => (theme::ink(), FontWeight::MEDIUM),
        Light::Needed => (theme::needs_you(), FontWeight::MEDIUM),
    };
    let cell = div()
        .id(key.label())
        .flex()
        .flex_col()
        .flex_1()
        .items_center()
        .gap(px(3.))
        .text_color(text)
        .font_weight(weight)
        .child(
            div()
                .font_family(theme::code_font())
                .text_size(px(theme::KEY_FONT_SIZE))
                .child(theme::chord(key.glyph())),
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
pub struct HintCard {
    pub text: &'static str,
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

#[cfg(test)]
mod tests {
    use super::terminal_chords;

    #[test]
    fn the_terminal_chords_are_spelled_for_the_platform() {
        let mac = terminal_chords(true);
        assert_eq!(mac[0], ("\u{2318}J".to_owned(), "Hide dock"));
        assert!(mac.iter().any(|(chord, _)| chord == "\u{21e7}\u{2318}R"));
        let other = terminal_chords(false);
        assert_eq!(other[0], ("Ctrl+Shift+J".to_owned(), "Hide dock"));
        // Ctrl alone is never an rvw chord where a terminal has the keyboard.
        assert!(other
            .iter()
            .all(|(chord, _)| chord.starts_with("Ctrl+Shift+")));
        assert_eq!(mac.len(), other.len());
    }
}
