//! The chrome every screen sits in: top bar, rail, the two side panels, the
//! centre and the key strip. A screen supplies its three panels; the frame
//! supplies everything around them.

use gpui::prelude::*;
use gpui::{div, px, svg, ClickEvent, Context, Div, FontWeight, Pixels, Rgba, Stateful, Window};
use rvw_view::keys::Screen;

use crate::data::Snapshot;
use crate::shell::Shell;
use crate::theme;

/// What a screen fills in the body of the frame.
pub struct Panels {
    pub left: Div,
    pub centre: Stateful<Div>,
    pub right: Div,
}

/// The whole window: top bar, the body of rail and panels, then the key strip.
pub fn window(top: Div, rail: Div, panels: Panels, keys: Div) -> Div {
    let body = div()
        .flex()
        .flex_1()
        .min_h(px(0.))
        .child(rail)
        .child(side(theme::LEFT_PANEL_WIDTH, Side::Left, panels.left))
        .child(div().flex().flex_1().min_w(px(0.)).child(panels.centre))
        .child(side(theme::RIGHT_PANEL_WIDTH, Side::Right, panels.right));

    div()
        .flex()
        .flex_col()
        .size_full()
        .overflow_hidden()
        .bg(theme::paper())
        .child(top)
        .child(body)
        .child(keys)
}

pub fn top_bar(snapshot: &Snapshot) -> Div {
    let counters = snapshot.queue.counters;
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
                .font_family(theme::code_font())
                .text_size(px(theme::CODE_FONT_SIZE))
                .text_color(theme::muted())
                .child(snapshot.workspace.clone()),
        )
        .child(
            div()
                .text_color(theme::muted())
                .child(format!("branch {}", snapshot.branch)),
        )
        .child(div().flex_1())
        .child(waiting_counter(counters.waiting_on_you))
        .child(counter(
            format!("◐ {} with claude", counters.with_agent),
            theme::agent(),
        ))
        .child(counter(
            format!("○ {} in draft", counters.draft),
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

/// The rail's four icons. Queue and Review open their screens; History and
/// Branches are drawn but not wired yet. The ids are the old letters.
pub fn rail(screen: Screen, cx: &mut Context<Shell>) -> Div {
    let items = [
        ("Q", "icons/queue.svg", Some(Screen::Queue)),
        ("R", "icons/review.svg", Some(Screen::Diff)),
        ("H", "icons/history.svg", None),
        ("B", "icons/branches.svg", None),
    ];
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
        .children(items.map(|(id, icon, target)| {
            let active = target == Some(screen);
            let mut item = div()
                .id(id)
                .flex()
                .items_center()
                .justify_center()
                .size(px(36.))
                .rounded(theme::CONTROL_RADIUS)
                .when(active, |d| d.bg(theme::selected_panel()))
                .child(svg().path(icon).size(px(18.)).text_color(if active {
                    theme::ink()
                } else {
                    theme::faint()
                }));
            if let Some(target) = target {
                item = item.on_click(cx.listener(
                    move |this: &mut Shell,
                          _: &ClickEvent,
                          _: &mut Window,
                          cx: &mut Context<Shell>| {
                        this.show(target, cx);
                    },
                ));
            }
            item
        }))
}

#[derive(Clone, Copy)]
enum Side {
    Left,
    Right,
}

fn side(width: Pixels, side: Side, content: Div) -> Div {
    let panel = div()
        .flex()
        .flex_col()
        .w(width)
        .h_full()
        .bg(theme::panel())
        .child(content);
    match side {
        Side::Left => panel.border_r_1().border_color(theme::hairline()),
        Side::Right => panel.border_l_1().border_color(theme::hairline()),
    }
}
