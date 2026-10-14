//! The compare picker: the top bar's "compared with <base>" chip and the
//! small menu under it that chooses what the changes are compared with. The
//! entries come from `rvw_view::compare_menu`; this file draws them and calls
//! the shell.

use gpui::prelude::*;
use gpui::{
    anchored, deferred, div, point, px, ClickEvent, Context, Div, MouseButton, MouseDownEvent,
    SharedString, Window,
};
use rvw_view::changes::Compare;
use rvw_view::compare_menu::Item;

use crate::data::Snapshot;
use crate::icon::{self, Icon};
use crate::shell::Shell;
use crate::theme;

/// The open menu: its entries and the error of a choice that failed to load.
pub struct CompareMenu {
    pub items: Vec<Item>,
    pub error: Option<String>,
}

impl Shell {
    /// Opens the menu, resolving the three comparisons now; a second call
    /// closes it.
    pub fn toggle_compare_menu(&mut self, cx: &mut Context<Self>) {
        if self.tools.compare_menu.is_some() {
            self.close_compare_menu(cx);
            return;
        }
        self.tools.close_all();
        self.tools.compare_menu = Some(CompareMenu {
            items: self.compare_items(),
            error: None,
        });
        cx.notify();
    }

    pub fn close_compare_menu(&mut self, cx: &mut Context<Self>) {
        self.tools.compare_menu = None;
        cx.notify();
    }

    /// Compares with `compare` from now on and re-reads the changes. A failed
    /// read keeps the menu open with its message and the old comparison.
    pub fn set_compare(&mut self, compare: Compare, cx: &mut Context<Self>) {
        let previous = self.tools.compare;
        self.tools.compare = Some(compare);
        match self.reload_for_new_base(cx) {
            Ok(()) => self.tools.compare_menu = None,
            Err(message) => {
                self.tools.compare = previous;
                if let Some(menu) = self.tools.compare_menu.as_mut() {
                    menu.error = Some(message);
                }
            }
        }
        cx.notify();
    }
}

/// The chip and, when `menu` is open, the card under it. The card is
/// deferred so that it paints over the panels below the bar.
pub fn chip(snapshot: &Snapshot, menu: Option<&CompareMenu>, cx: &Context<Shell>) -> Div {
    let name = if snapshot.base_name.is_empty() {
        "nothing"
    } else {
        snapshot.base_name.as_str()
    };
    let open = menu.is_some();
    let button = div()
        .id("compare-chip")
        .flex()
        .items_center()
        .h(px(24.))
        .px(px(6.))
        .rounded(theme::ROW_RADIUS)
        .cursor_pointer()
        .text_color(if open { theme::ink() } else { theme::muted() })
        .hover(|style| style.text_color(theme::ink()).bg(theme::hairline()))
        .on_click(cx.listener(
            |this: &mut Shell, _: &ClickEvent, _: &mut Window, cx: &mut Context<Shell>| {
                this.toggle_compare_menu(cx);
            },
        ))
        .gap(px(4.))
        .child(format!("compared with {name}"))
        .child(icon::view(
            Icon::Expanded,
            icon::SMALL,
            if open { theme::ink() } else { theme::muted() },
        ));
    let mut wrapper = div().relative().child(button);
    if let Some(menu) = menu {
        wrapper = wrapper.child(deferred(
            anchored()
                .offset(point(px(0.), px(28.)))
                .child(card(menu, cx)),
        ));
    }
    wrapper
}

/// A transparent layer over the window under the open menu: a click on it
/// closes the menu and goes no further.
pub fn scrim(cx: &Context<Shell>) -> Div {
    div().absolute().inset_0().occlude().on_mouse_down(
        MouseButton::Left,
        cx.listener(
            |this: &mut Shell, _: &MouseDownEvent, _: &mut Window, cx: &mut Context<Shell>| {
                this.close_compare_menu(cx);
            },
        ),
    )
}

fn card(menu: &CompareMenu, cx: &Context<Shell>) -> Div {
    let mut card = div()
        .occlude()
        .flex()
        .flex_col()
        .w(px(300.))
        .py(px(6.))
        .rounded(theme::CONTROL_RADIUS)
        .bg(theme::paper())
        .border_1()
        .border_color(theme::hairline_strong())
        .children(
            menu.items
                .iter()
                .enumerate()
                .map(|(i, item)| row(i, item, cx)),
        );
    if let Some(error) = menu.error.as_deref() {
        card = card.child(
            div()
                .px(px(14.))
                .py(px(6.))
                .text_color(theme::rejected())
                .child(error.lines().next().unwrap_or_default().trim().to_owned()),
        );
    }
    card
}

/// One comparison: a check mark when current, its label, and on the right the
/// base it resolves to, or the reason it cannot be chosen (the row is dim and
/// does nothing).
fn row(index: usize, item: &Item, cx: &Context<Shell>) -> impl IntoElement {
    let usable = item.reason.is_none();
    let compare = item.compare;
    let right = item
        .base
        .clone()
        .or_else(|| item.reason.clone())
        .unwrap_or_default();
    let mut row = div()
        .id(SharedString::from(format!("compare-{index}")))
        .flex()
        .items_center()
        .gap(px(8.))
        .h(px(32.))
        .mx(px(6.))
        .px(px(8.))
        .rounded(theme::ROW_RADIUS)
        .text_color(if usable { theme::ink() } else { theme::faint() })
        .child(div().w(icon::SIZE).flex_none().when(item.current, |d| {
            d.child(icon::view(
                Icon::Done,
                icon::SIZE,
                if usable { theme::ink() } else { theme::faint() },
            ))
        }))
        .child(div().flex_none().child(item.label))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .text_right()
                .font_family(theme::code_font())
                .text_size(px(theme::KEY_FONT_SIZE))
                .text_color(theme::faint())
                .child(right),
        );
    if usable {
        row = row
            .cursor_pointer()
            .hover(|style| style.bg(theme::hairline()))
            .on_click(cx.listener(
                move |this: &mut Shell, _: &ClickEvent, _: &mut Window, cx: &mut Context<Shell>| {
                    this.set_compare(compare, cx);
                },
            ));
    }
    row
}
