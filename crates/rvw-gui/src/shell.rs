//! The application shell every screen sits in: top bar, rail, two side panels,
//! the centre and the key strip.

use gpui::prelude::*;
use gpui::{div, px, Context, Div, FontWeight, Pixels, Render, Rgba, Window};

use crate::data::ShellData;
use crate::theme;

const KEYS: [&str; 13] = [
    "Move J K",
    "Open ↵",
    "Open file O",
    "Comment C",
    "Add ⌘↵",
    "Submit ⇧⌘R",
    "Accept A",
    "Reopen R",
    "Stage S",
    "Commit ⇧⌘C",
    "Undo ⌘Z",
    "Find file ⌘P",
    "Actions ⌘K",
];

pub struct Shell {
    data: ShellData,
}

impl Shell {
    pub fn new(data: ShellData) -> Self {
        Self { data }
    }
}

impl Render for Shell {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let window = div()
            .flex()
            .flex_col()
            .size_full()
            .rounded(theme::WINDOW_RADIUS)
            .overflow_hidden()
            .bg(theme::paper())
            .child(top_bar(&self.data))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h(px(0.))
                    .child(rail())
                    .child(side_panel(
                        "Changed files",
                        theme::LEFT_PANEL_WIDTH,
                        Side::Left,
                    ))
                    .child(centre())
                    .child(side_panel(
                        "Your review",
                        theme::RIGHT_PANEL_WIDTH,
                        Side::Right,
                    )),
            )
            .child(key_strip());

        div()
            .size_full()
            .bg(theme::canvas())
            .p(theme::WINDOW_MARGIN)
            .font_family(theme::UI_FONT)
            .text_size(px(theme::BASE_FONT_SIZE))
            .text_color(theme::text())
            .child(window)
    }
}

fn top_bar(data: &ShellData) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(16.))
        .h(theme::TOP_BAR_HEIGHT)
        .px(px(20.))
        .border_b_1()
        .border_color(theme::hairline())
        .child(
            div()
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme::ink())
                .child("rvw"),
        )
        .child(
            div()
                .font_family(theme::CODE_FONT)
                .text_size(px(theme::CODE_FONT_SIZE))
                .text_color(theme::muted())
                .child(data.workspace.clone()),
        )
        .child(
            div()
                .text_color(theme::muted())
                .child(format!("branch {}", data.branch)),
        )
        .child(div().flex_1())
        .child(waiting_counter(data.waiting_on_you))
        .child(counter(
            format!("◐ {} with claude", data.with_agent),
            theme::agent(),
        ))
        .child(counter(
            format!("○ {} in draft", data.in_draft),
            theme::pending(),
        ))
}

/// Orange is the only "needs you" signal, so the dot is orange and the text is
/// ink, not the other way round.
fn waiting_counter(count: usize) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(6.))
        .child(div().size(px(7.)).rounded_full().bg(theme::needs_you()))
        .child(
            div()
                .text_color(theme::ink())
                .child(format!("{count} waiting on you")),
        )
}

fn counter(label: String, color: Rgba) -> Div {
    div().text_color(color).child(label)
}

fn rail() -> Div {
    let items = [("R", true), ("C", false), ("H", false), ("B", false)];
    div()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(8.))
        .w(theme::RAIL_WIDTH)
        .h_full()
        .pt(px(12.))
        .bg(theme::panel())
        .border_r_1()
        .border_color(theme::hairline())
        .children(items.map(|(glyph, active)| {
            div()
                .flex()
                .items_center()
                .justify_center()
                .size(px(36.))
                .rounded(theme::CONTROL_RADIUS)
                .font_family(theme::CODE_FONT)
                .text_color(if active { theme::ink() } else { theme::faint() })
                .when(active, |d| d.bg(theme::selected_panel()))
                .child(glyph)
        }))
}

#[derive(Clone, Copy)]
enum Side {
    Left,
    Right,
}

fn side_panel(heading: &'static str, width: Pixels, side: Side) -> Div {
    let panel = div()
        .flex()
        .flex_col()
        .w(width)
        .h_full()
        .p(theme::PANEL_PADDING)
        .bg(theme::panel())
        .child(
            div()
                .text_size(px(22.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme::ink())
                .child(heading),
        );
    match side {
        Side::Left => panel.border_r_1().border_color(theme::hairline()),
        Side::Right => panel.border_l_1().border_color(theme::hairline()),
    }
}

fn centre() -> Div {
    div()
        .flex()
        .flex_1()
        .items_center()
        .justify_center()
        .bg(theme::paper())
        .child(
            div()
                .text_color(theme::muted())
                .child("Open a change to start reviewing"),
        )
}

fn key_strip() -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(20.))
        .h(theme::KEY_STRIP_HEIGHT)
        .px(px(20.))
        .border_t_1()
        .border_color(theme::hairline())
        .font_family(theme::CODE_FONT)
        .text_size(px(theme::KEY_FONT_SIZE))
        .text_color(theme::faint())
        .children(KEYS.map(|key| div().child(key)))
}
