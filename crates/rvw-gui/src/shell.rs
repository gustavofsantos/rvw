//! The application window. It owns the fetched snapshot and the current
//! filter, re-runs the queue view model when the filter changes, and hands
//! the queue screen's panels to the common frame.

use gpui::prelude::*;
use gpui::{div, px, Context, Render, Window};
use rvw_view::queue::{Filter, Queue, State};

use crate::data::Snapshot;
use crate::frame::{self, Panels};
use crate::key_strip::key_strip;
use crate::queue_screen;
use crate::theme;

pub struct Shell {
    snapshot: Snapshot,
    filter: Filter,
    /// The snapshot's queue under `filter`.
    queue: Queue,
}

impl Shell {
    pub fn new(snapshot: Snapshot) -> Self {
        let queue = snapshot.queue.clone();
        Self {
            snapshot,
            filter: Filter::default(),
            queue,
        }
    }

    /// Selecting the state already shown clears it.
    pub fn pick_state(&mut self, state: State, cx: &mut Context<Self>) {
        let filter = Filter {
            state: toggled(self.filter.state.as_ref(), state),
            ..self.filter.clone()
        };
        self.apply(filter, cx);
    }

    pub fn pick_lane(&mut self, lane: &str, cx: &mut Context<Self>) {
        let filter = Filter {
            lane: toggled(self.filter.lane.as_ref(), lane.to_owned()),
            ..self.filter.clone()
        };
        self.apply(filter, cx);
    }

    pub fn pick_author(&mut self, author: &str, cx: &mut Context<Self>) {
        let filter = Filter {
            author: toggled(self.filter.author.as_ref(), author.to_owned()),
            ..self.filter.clone()
        };
        self.apply(filter, cx);
    }

    fn apply(&mut self, filter: Filter, cx: &mut Context<Self>) {
        self.queue = self.snapshot.view(&filter);
        self.filter = filter;
        cx.notify();
    }
}

/// The value, or none when it is already the one set.
fn toggled<T: PartialEq>(current: Option<&T>, value: T) -> Option<T> {
    if current == Some(&value) {
        None
    } else {
        Some(value)
    }
}

impl Render for Shell {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let panels = Panels {
            left: queue_screen::left(&self.snapshot, &self.filter, cx),
            centre: queue_screen::centre(&self.snapshot, &self.queue),
            right: queue_screen::right(&self.snapshot.next_pull),
        };
        let window = frame::window(
            frame::top_bar(&self.snapshot),
            panels,
            key_strip(&self.snapshot.queue.counters),
        );

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
