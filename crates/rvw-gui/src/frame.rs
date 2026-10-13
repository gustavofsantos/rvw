//! The chrome every screen sits in: top bar, rail, the two side panels, the
//! centre and the key strip. A screen supplies its three panels; the frame
//! supplies everything around them.

use gpui::prelude::*;
use gpui::{div, px, svg, ClickEvent, Context, Div, FontWeight, Pixels, Rgba, Stateful, Window};
use rvw_view::keys::Screen;

use crate::compare_picker::{self, CompareMenu};
use crate::data::Snapshot;
use crate::git_tools::GitTools;
use crate::shell::Shell;
use crate::theme;
use crate::window_chrome;

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

pub fn top_bar(
    snapshot: &Snapshot,
    git: &GitTools,
    menu: Option<&CompareMenu>,
    window: &Window,
    cx: &Context<Shell>,
) -> Div {
    let counters = snapshot.queue.counters;
    let bar = div()
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
        .child(window_chrome::no_drag(workspace_chip(
            &snapshot.workspace,
            cx,
        )))
        .child(window_chrome::no_drag(branch_chip(&snapshot.branch, cx)))
        .children(git_status(git).map(window_chrome::no_drag))
        .child(window_chrome::no_drag(compare_picker::chip(
            snapshot, menu, cx,
        )))
        .child(div().flex_1())
        .child(window_chrome::no_drag(
            div()
                .flex()
                .items_center()
                .gap(px(16.))
                .child(waiting_counter(counters.waiting_on_you))
                .child(counter(
                    format!("◐ {} with claude", counters.with_agent),
                    theme::agent(),
                ))
                .child(counter(
                    format!("○ {} in draft", counters.draft),
                    theme::pending(),
                )),
        ))
        .children(window_chrome::controls(window));
    window_chrome::drag_region(bar, window)
}

/// What the repository's sync state says after the branch name: how far it is
/// from its upstream, that it is not published, and the network command
/// running or its last result. None when there is nothing to say.
fn git_status(git: &GitTools) -> Option<Div> {
    let facts = &git.facts;
    let mut parts: Vec<Div> = Vec::new();
    if let Some(text) = ahead_behind_text(facts.ahead_behind) {
        parts.push(
            div()
                .font_family(theme::code_font())
                .text_size(px(theme::CODE_FONT_SIZE))
                .text_color(theme::muted())
                .child(text),
        );
    }
    if facts.upstream.is_none() && facts.branch.is_some() && facts.remote.is_some() {
        parts.push(div().text_color(theme::faint()).child("not published"));
    }
    if let Some(busy) = git.busy.as_ref() {
        parts.push(div().text_color(theme::muted()).child(busy.clone()));
    } else if let Some(result) = git.network_result.as_ref() {
        parts.push(
            div()
                .max_w(px(360.))
                .truncate()
                .text_color(theme::faint())
                .child(result.clone()),
        );
    }
    if parts.is_empty() {
        return None;
    }
    Some(div().flex().items_center().gap(px(12.)).children(parts))
}

/// "↑2 ↓3" for a branch two commits ahead and three behind its upstream. A
/// zero side is left out; None when the branch is level or has no upstream.
fn ahead_behind_text(ahead_behind: Option<(usize, usize)>) -> Option<String> {
    let (ahead, behind) = ahead_behind?;
    let mut parts = Vec::new();
    if ahead > 0 {
        parts.push(format!("\u{2191}{ahead}"));
    }
    if behind > 0 {
        parts.push(format!("\u{2193}{behind}"));
    }
    (!parts.is_empty()).then(|| parts.join(" "))
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

/// The open workspace, with the home directory written `~`. A click opens
/// the workspace picker.
fn workspace_chip(workspace: &str, cx: &Context<Shell>) -> Stateful<Div> {
    let home = std::env::home_dir().map(|home| home.to_string_lossy().into_owned());
    div()
        .id("workspace-chip")
        .flex()
        .items_center()
        .h(px(24.))
        .px(px(6.))
        .rounded(theme::ROW_RADIUS)
        .cursor_pointer()
        .font_family(theme::code_font())
        .text_size(px(theme::CODE_FONT_SIZE))
        .text_color(theme::muted())
        .hover(|style| style.text_color(theme::ink()).bg(theme::hairline()))
        .on_click(cx.listener(
            |this: &mut Shell, _: &ClickEvent, window: &mut Window, cx: &mut Context<Shell>| {
                this.open_workspaces(window, cx);
            },
        ))
        .child(rvw_view::workspaces::label(workspace, home.as_deref()))
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
}

/// The rail's five icons. Queue, Review, Changes and History open their
/// screens, Branches the branch picker. The ids are the old letters.
pub fn rail(screen: Screen, cx: &mut Context<Shell>) -> Div {
    let items = [
        ("Q", "icons/queue.svg", RailTarget::Show(Screen::Queue)),
        ("R", "icons/review.svg", RailTarget::Show(Screen::Diff)),
        ("C", "icons/changes.svg", RailTarget::Show(Screen::Changes)),
        ("H", "icons/history.svg", RailTarget::Show(Screen::History)),
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
            div()
                .id(id)
                .flex()
                .items_center()
                .justify_center()
                .size(px(36.))
                .rounded(theme::CONTROL_RADIUS)
                .when(active, |d| d.bg(theme::selected_panel()))
                .when(!active, |d| {
                    d.cursor_pointer().hover(|s| s.bg(theme::hairline()))
                })
                .map(|d| {
                    d.on_click(cx.listener(
                        move |this: &mut Shell,
                              _: &ClickEvent,
                              window: &mut Window,
                              cx: &mut Context<Shell>| match target {
                            RailTarget::Show(screen) => this.show(screen, cx),
                            RailTarget::Branches => this.open_branches(window, cx),
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

#[cfg(test)]
mod tests {
    use super::ahead_behind_text;

    #[test]
    fn ahead_and_behind_name_only_the_sides_that_differ() {
        assert_eq!(ahead_behind_text(None), None);
        assert_eq!(ahead_behind_text(Some((0, 0))), None);
        assert_eq!(ahead_behind_text(Some((2, 0))).as_deref(), Some("↑2"));
        assert_eq!(ahead_behind_text(Some((0, 3))).as_deref(), Some("↓3"));
        assert_eq!(ahead_behind_text(Some((2, 3))).as_deref(), Some("↑2 ↓3"));
    }
}
