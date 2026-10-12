//! The application window. It owns the fetched snapshot, the screen shown,
//! the queue filter and the file open in the review screen. It re-runs the
//! queue view model when the filter changes and hands each screen's panels to
//! the common frame.

use gpui::prelude::*;
use gpui::{div, px, Context, FocusHandle, KeyDownEvent, Render, Window};
use rvw_core::Service;
use rvw_view::keys::Screen;
use rvw_view::queue::{Filter, Queue, State};
use rvw_view::selection::Pos;

use crate::composer::Composer;
use crate::data::{self, Identity, Snapshot};
use crate::frame::{self, Panels};
use crate::key_strip::key_strip;
use crate::queue_screen;
use crate::review_screen::{self, Selected};
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme;

pub struct Shell {
    snapshot: Snapshot,
    service: Service,
    /// The workspace, author and lane new comments are written with.
    identity: Identity,
    /// `Queue` or `Diff` (the review screen).
    screen: Screen,
    filter: Filter,
    /// The snapshot's queue under `filter`.
    queue: Queue,
    /// The file the review screen shows, if any.
    selected: Option<Selected>,
    /// The open comment composer, if any.
    composer: Option<Composer>,
    /// Holds the keys (C) when no field has focus.
    focus: FocusHandle,
}

impl Shell {
    /// Opens on the review screen when the workspace has changes, with its
    /// first file selected; otherwise on the queue.
    pub fn new(
        snapshot: Snapshot,
        service: Service,
        identity: Identity,
        cx: &mut Context<Self>,
    ) -> Self {
        let queue = snapshot.queue.clone();
        let screen = if snapshot.changes.is_empty() {
            Screen::Queue
        } else {
            Screen::Diff
        };
        let selected = snapshot
            .changes
            .first()
            .map(|file| Selected::load(&snapshot, &file.path));
        Self {
            snapshot,
            service,
            identity,
            screen,
            filter: Filter::default(),
            queue,
            selected,
            composer: None,
            focus: cx.focus_handle(),
        }
    }

    pub fn show(&mut self, screen: Screen, cx: &mut Context<Self>) {
        self.screen = screen;
        cx.notify();
    }

    pub fn select_file(&mut self, path: &str, cx: &mut Context<Self>) {
        self.selected = Some(Selected::load(&self.snapshot, path));
        self.composer = None;
        cx.notify();
    }

    /// Click selects the line alone; Shift-click extends from the anchor.
    pub fn select_line(&mut self, pos: Pos, extend: bool, cx: &mut Context<Self>) {
        let Some(selected) = self.selected.as_mut() else {
            return;
        };
        let Ok(diff) = &selected.diff else {
            return;
        };
        if extend {
            selected.selection.extend(diff, pos);
        } else {
            selected.selection.click(diff, pos);
        }
        cx.notify();
    }

    /// Opens the composer under the selection, with focus in its field. Does
    /// nothing when it is open or nothing is selected.
    pub fn open_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let nothing_selected = self
            .selected
            .as_ref()
            .is_none_or(|selected| selected.selection.is_empty());
        if self.composer.is_some() || nothing_selected {
            return;
        }
        let input = cx.new(|cx| TextInput::new("Write a comment", cx));
        let events = cx.subscribe(
            &input,
            |this: &mut Self, _, event: &TextInputEvent, cx: &mut Context<Self>| {
                this.on_composer_event(*event, cx);
            },
        );
        input.read(cx).focus(window);
        self.composer = Some(Composer::new(input, events));
        cx.notify();
    }

    fn on_composer_event(&mut self, event: TextInputEvent, cx: &mut Context<Self>) {
        match event {
            TextInputEvent::Cancel => self.composer = None,
            TextInputEvent::Submit => self.submit_comment(cx),
        }
        cx.notify();
    }

    /// Adds the typed comment on the selection. A blank text keeps the
    /// composer open; a refused add shows its message in the composer.
    fn submit_comment(&mut self, cx: &mut Context<Self>) {
        let Some(text) = self
            .composer
            .as_ref()
            .map(|composer| composer.input.read(cx).text().trim().to_owned())
        else {
            return;
        };
        if text.is_empty() {
            return;
        }
        match self.add_selected(&text) {
            Ok(()) => self.reload(cx),
            Err(message) => {
                if let Some(composer) = self.composer.as_mut() {
                    composer.error = Some(message);
                }
                cx.notify();
            }
        }
    }

    fn add_selected(&self, text: &str) -> Result<(), String> {
        let selected = self.selected.as_ref().ok_or("no file is open")?;
        let diff = selected.diff.as_ref().map_err(Clone::clone)?;
        let (lo, hi) = selected
            .selection
            .lines(diff)
            .ok_or("select a line first")?;
        let lines = (i64::from(lo), i64::from(hi));
        data::add_comment(&self.service, &self.identity, &selected.path, lines, text)
    }

    /// Re-reads the snapshot after a write, keeping the file open with a fresh
    /// selection. A failed re-read keeps the composer open with its message.
    fn reload(&mut self, cx: &mut Context<Self>) {
        let path = self.selected.as_ref().map(|selected| selected.path.clone());
        match data::load(&self.service, &self.identity.workspace, &self.identity.lane) {
            Ok(snapshot) => {
                self.queue = snapshot.view(&self.filter);
                self.snapshot = snapshot;
                self.selected = path.map(|path| Selected::load(&self.snapshot, &path));
                self.composer = None;
            }
            Err(message) => {
                if let Some(composer) = self.composer.as_mut() {
                    composer.error = Some(format!("Saved, but the view did not reload: {message}"));
                    composer.input.update(cx, TextInput::clear);
                }
            }
        }
        cx.notify();
    }

    /// C opens the composer on the review screen, unless it is open already.
    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let on_review = matches!(self.screen, Screen::Diff);
        if on_review
            && event.keystroke.key == "c"
            && event.keystroke.modifiers.number_of_modifiers() == 0
        {
            self.open_composer(window, cx);
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // With no field open, the shell holds the focus so that C reaches it.
        if self.composer.is_none() && !self.focus.is_focused(window) {
            self.focus.focus(window);
        }
        let panels = match self.screen {
            Screen::Diff => Panels {
                left: review_screen::left(
                    &self.snapshot,
                    self.selected.as_ref().map(|s| s.path.as_str()),
                    cx,
                ),
                centre: review_screen::centre(
                    &self.snapshot,
                    self.selected.as_ref(),
                    self.composer.as_ref(),
                    cx,
                ),
                right: review_screen::right(&self.snapshot),
            },
            _ => Panels {
                left: queue_screen::left(&self.snapshot, &self.filter, cx),
                centre: queue_screen::centre(&self.snapshot, &self.queue),
                right: queue_screen::right(&self.snapshot.next_pull),
            },
        };
        let window = frame::window(
            frame::top_bar(&self.snapshot),
            frame::rail(self.screen, cx),
            panels,
            key_strip(self.screen, &self.snapshot.queue.counters),
        );

        div()
            .size_full()
            .bg(theme::canvas())
            .p(theme::WINDOW_MARGIN)
            .font_family(theme::UI_FONT)
            .text_size(px(theme::BASE_FONT_SIZE))
            .text_color(theme::text())
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .child(window)
    }
}
