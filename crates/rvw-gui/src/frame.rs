//! The chrome every screen sits in: top bar, rail, the two side panels, the
//! centre and the key strip. A screen supplies its three panels; the frame
//! supplies everything around them.

use gpui::prelude::*;
use gpui::{div, px, svg, ClickEvent, Context, Div, FontWeight, Pixels, Rgba, Stateful, Window};
use rvw_view::keys::Screen;

use crate::compare_picker::{self, CompareMenu};
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

pub fn top_bar(snapshot: &Snapshot, menu: Option<&CompareMenu>, cx: &Context<Shell>) -> Div {
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
        .child(branch_chip(&snapshot.branch, cx))
        .child(compare_picker::chip(snapshot, menu, cx))
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

/// The branch name; a click opens the branch picker, as the rail's B does.
fn branch_chip(branch: &str, cx: &Context<Shell>) -> Stateful<Div> {
    div()
        .id("branch-chip")
        .flex()
        .items_center()
        .h(px(24.))
        .px(px(6.))
        .rounded(theme::ROW_RADIUS)
        .cursor_pointer()
        .text_color(theme::muted())
        .hover(|style| style.text_color(theme::ink()).bg(theme::hairline()))
        .on_click(cx.listener(
            |this: &mut Shell, _: &ClickEvent, window: &mut Window, cx: &mut Context<Shell>| {
                this.open_branches(window, cx);
            },
        ))
        .child(format!("branch {branch}"))
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

/// What a rail icon does.
#[derive(Clone, Copy)]
enum RailTarget {
    Show(Screen),
    Branches,
    None,
}

/// The rail's four icons. Queue and Review open their screens, Branches the
/// branch picker; History is drawn but not wired yet. The ids are the old
/// letters.
pub fn rail(screen: Screen, cx: &mut Context<Shell>) -> Div {
    let items = [
        ("Q", "icons/queue.svg", RailTarget::Show(Screen::Queue)),
        ("R", "icons/review.svg", RailTarget::Show(Screen::Diff)),
        ("H", "icons/history.svg", RailTarget::None),
        ("B", "icons/branches.svg", RailTarget::Branches),
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
            let active = matches!(target, RailTarget::Show(s) if s == screen);
            let live = !matches!(target, RailTarget::None);
            div()
                .id(id)
                .flex()
                .items_center()
                .justify_center()
                .size(px(36.))
                .rounded(theme::CONTROL_RADIUS)
                .when(active, |d| d.bg(theme::selected_panel()))
                .when(live && !active, |d| {
                    d.cursor_pointer().hover(|s| s.bg(theme::hairline()))
                })
                .when(live, |d| {
                    d.on_click(cx.listener(
                        move |this: &mut Shell,
                              _: &ClickEvent,
                              window: &mut Window,
                              cx: &mut Context<Shell>| match target {
                            RailTarget::Show(screen) => this.show(screen, cx),
                            RailTarget::Branches => this.open_branches(window, cx),
                            RailTarget::None => {}
                        },
                    ))
                })
                .child(svg().path(icon).size(px(18.)).text_color(if active {
                    theme::ink()
                } else {
                    theme::faint()
                }))
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
