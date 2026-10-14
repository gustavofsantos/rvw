//! The thin strip on a panel's edge that is dragged to resize it. The
//! window's size and the limits live in `rvw_view::resize`; this draws the
//! strip, follows the pointer and calls back with the size it ends up at.

use std::cell::Cell;
use std::rc::Rc;

use gpui::prelude::*;
use gpui::{
    div, px, App, ClickEvent, Context, DragMoveEvent, ElementId, Entity, Render, Stateful, Window,
};
use rvw_view::resize::{self, Limits};

use crate::theme;

/// The width of the strip's hit area.
const THICKNESS: f32 = 5.;

/// Which edge of its panel the strip is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    /// The dock's top edge: moving up enlarges it.
    Top,
    /// The left panel's right edge: moving right enlarges it.
    Right,
}

impl Edge {
    fn vertical(self) -> bool {
        self == Self::Top
    }

    fn grows_with_pointer(self) -> bool {
        self == Self::Right
    }
}

/// The drag in flight: the size when it started, and where the pointer was
/// at its first move. Made at render, so each drag has its own.
struct Resizing {
    size: f32,
    origin: Rc<Cell<Option<f32>>>,
}

/// What a drag shows under the pointer: nothing.
struct Ghost;

impl Render for Ghost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// The strip. `size` is the panel's size now; `set` receives the size a drag
/// asks for, already within `limits`; `reset` runs on a double click.
pub fn handle<V: 'static>(
    id: impl Into<ElementId>,
    edge: Edge,
    size: f32,
    limits: Limits,
    set: impl Fn(&mut V, f32, &mut Context<V>) + 'static,
    reset: impl Fn(&mut V, &mut Context<V>) + 'static,
    cx: &mut Context<V>,
) -> Stateful<gpui::Div> {
    let value = Resizing {
        size,
        origin: Rc::new(Cell::new(None)),
    };
    let strip = div()
        .id(id)
        .absolute()
        .hover(|style| style.bg(theme::hairline_strong()));
    let strip = match edge {
        Edge::Top => strip
            .top_0()
            .left_0()
            .w_full()
            .h(px(THICKNESS))
            .cursor_row_resize(),
        Edge::Right => strip
            .top_0()
            .right_0()
            .h_full()
            .w(px(THICKNESS))
            .cursor_col_resize(),
    };
    strip
        .on_drag(value, |_: &Resizing, _, _, cx: &mut App| {
            let ghost: Entity<Ghost> = cx.new(|_| Ghost);
            ghost
        })
        .on_drag_move(cx.listener(
            move |this: &mut V, event: &DragMoveEvent<Resizing>, window: &mut Window, cx| {
                let drag = event.drag(cx);
                let (pointer, extent) = if edge.vertical() {
                    (
                        f32::from(event.event.position.y),
                        f32::from(window.viewport_size().height),
                    )
                } else {
                    (
                        f32::from(event.event.position.x),
                        f32::from(window.viewport_size().width),
                    )
                };
                let origin = drag.origin.get().unwrap_or(pointer);
                drag.origin.set(Some(origin));
                let next = resize::dragged(
                    drag.size,
                    origin,
                    pointer,
                    edge.grows_with_pointer(),
                    limits,
                    extent,
                );
                set(this, next, cx);
                cx.notify();
            },
        ))
        .on_click(cx.listener(
            move |this: &mut V, event: &ClickEvent, _: &mut Window, cx| {
                if event.click_count() == 2 {
                    reset(this, cx);
                    cx.notify();
                }
            },
        ))
}
