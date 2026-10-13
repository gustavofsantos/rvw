//! Press-drag-release selection of diff lines, as in a terminal. The shell
//! owns the drag; rows only report a press and the line under the pointer.
//! Moving over rows costs a comparison unless the line under the pointer is
//! new, and while the pointer is held at or beyond the list's top or bottom
//! edge a 60 Hz timer scrolls the list and extends the selection.

use std::time::Duration;

use gpui::Pixels;
use gpui::{px, AsyncApp, Context, Point, WeakEntity};
use rvw_view::selection::{self, Pos};

use crate::shell::Shell;

/// Pointer distance from an edge inside which the list scrolls.
const EDGE_ZONE: f32 = 28.;
/// Pixels per tick at the zone's inner border and at the edge itself; the
/// speed keeps growing, to twice the zone, as the pointer leaves the list.
const SLOWEST: f32 = 2.;
const FASTEST: f32 = 9.;
const TICK: Duration = Duration::from_millis(16);

#[derive(Default)]
pub struct DragState {
    active: bool,
    /// The line the selection was last extended to; hovering it again is
    /// free.
    last: Option<Pos>,
    /// A pending timer carries the value it was armed with; a drag ending
    /// or restarting bumps it, which stops the old chain.
    generation: u64,
    pointer: Point<Pixels>,
    /// The pointer moved since the press, so a still click never scrolls.
    moved: bool,
}

/// Signed pixels to scroll for a pointer at `y` over a list from `top` to
/// `bottom`: negative up, positive down, zero away from the edges.
pub fn autoscroll_step(y: f32, top: f32, bottom: f32) -> f32 {
    let speed = |depth: f32| {
        let depth = (depth / EDGE_ZONE).clamp(0., 2.);
        SLOWEST + depth * (FASTEST - SLOWEST)
    };
    if y < top + EDGE_ZONE {
        -speed(top + EDGE_ZONE - y)
    } else if y > bottom - EDGE_ZONE {
        speed(y - (bottom - EDGE_ZONE))
    } else {
        0.
    }
}

impl Shell {
    /// A press on the line at `pos`: selects it (or extends with Shift) and
    /// starts a drag, unless the line cannot be selected.
    pub(crate) fn press_line(&mut self, pos: Pos, shift: bool, cx: &mut Context<Self>) {
        self.select_line(pos, shift, cx);
        let selectable = self
            .selected
            .as_ref()
            .is_some_and(|s| s.diff.as_ref().is_ok_and(|d| selection::selectable(d, pos)));
        self.drag.active = selectable;
        self.drag.last = Some(pos);
        self.drag.moved = false;
        self.drag.generation = self.drag.generation.wrapping_add(1);
        if selectable {
            Self::schedule_scroll(self.drag.generation, cx);
        }
    }

    /// The pointer is over the line at `pos`. Does nothing unless a drag is
    /// on and the line is new; repaints only when the selection changed.
    pub(crate) fn drag_over(&mut self, pos: Pos, left_held: bool, cx: &mut Context<Self>) {
        if !self.drag.active {
            return;
        }
        if !left_held {
            self.end_drag();
            return;
        }
        if self.extend_drag(pos) {
            cx.notify();
        }
    }

    /// Any pointer move in the window: keeps the pointer for the timer, and
    /// ends a drag whose button release was never delivered.
    pub(crate) fn drag_pointer(&mut self, at: Point<Pixels>, left_held: bool) {
        if !self.drag.active {
            return;
        }
        if left_held {
            self.drag.pointer = at;
            self.drag.moved = true;
        } else {
            self.end_drag();
        }
    }

    /// The button went up, or the drag was lost.
    pub(crate) fn end_drag(&mut self) {
        if self.drag.active {
            self.drag.active = false;
            self.drag.last = None;
            self.drag.generation = self.drag.generation.wrapping_add(1);
        }
    }

    fn extend_drag(&mut self, pos: Pos) -> bool {
        if self.drag.last == Some(pos) {
            return false;
        }
        self.drag.last = Some(pos);
        let Some(selected) = self.selected.as_mut() else {
            return false;
        };
        let Ok(diff) = &selected.diff else {
            return false;
        };
        selected.selection.drag_to(diff, pos)
    }

    fn schedule_scroll(generation: u64, cx: &mut Context<Self>) {
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this: WeakEntity<Shell>, cx: &mut AsyncApp| {
            executor.timer(TICK).await;
            this.update(cx, |this, cx| this.scroll_tick(generation, cx))
                .ok();
        })
        .detach();
    }

    /// Scrolls while the pointer is held at an edge, then extends to the
    /// line now under it, and arms the next tick while the drag lasts.
    fn scroll_tick(&mut self, generation: u64, cx: &mut Context<Self>) {
        if !self.drag.active || self.drag.generation != generation {
            return;
        }
        let pointer = self.drag.pointer;
        let target = match self.diff_list.as_ref() {
            Some(list) if self.drag.moved => {
                let view = list.viewport();
                let step = autoscroll_step(
                    f32::from(pointer.y),
                    f32::from(view.top()),
                    f32::from(view.bottom()),
                );
                if step == 0. {
                    None
                } else {
                    list.scroll_by(px(step));
                    Some(list.pos_near(pointer))
                }
            }
            _ => None,
        };
        if let Some(pos) = target {
            self.extend_drag_or_none(pos);
            cx.notify();
        }
        Self::schedule_scroll(generation, cx);
    }

    fn extend_drag_or_none(&mut self, pos: Option<Pos>) {
        if let Some(pos) = pos {
            self.extend_drag(pos);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_middle_of_the_list_does_not_scroll() {
        assert!(autoscroll_step(300., 100., 500.).abs() < f32::EPSILON);
    }

    #[test]
    fn the_edges_scroll_toward_themselves_and_speed_up_outside() {
        let at_zone = autoscroll_step(100. + 27., 100., 500.);
        let at_edge = autoscroll_step(100., 100., 500.);
        let beyond = autoscroll_step(40., 100., 500.);
        assert!(at_zone < 0. && at_edge < at_zone && beyond < at_edge);
        assert!(autoscroll_step(500., 100., 500.) > 0.);
        assert!(autoscroll_step(900., 100., 500.) > autoscroll_step(500., 100., 500.));
    }

    #[test]
    fn the_speed_is_capped() {
        let far = autoscroll_step(-10_000., 100., 500.);
        assert!((far + 16.).abs() < 1e-3);
    }
}
