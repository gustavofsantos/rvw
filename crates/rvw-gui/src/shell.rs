//! The application window. It owns the fetched snapshot, the screen shown,
//! the queue filter and the file open in the review screen. It re-runs the
//! queue view model when the filter changes and hands each screen's panels to
//! the common frame.

use gpui::prelude::*;
use gpui::{
    div, px, Context, Div, FocusHandle, KeyDownEvent, Render, UniformListScrollHandle, Window,
};
use rvw_core::Service;
use rvw_view::keys::Screen;
use rvw_view::queue::{Filter, Queue, State};
use rvw_view::selection::Pos;

use crate::composer::Composer;
use crate::data::{self, Identity, Snapshot};
use crate::diff_list::DiffList;
use crate::draft_edit::Removal;
use crate::files_panel::{self, FileList};
use crate::finder::{self, Finder, OpenFinder};
use crate::frame::{self, Panels};
use crate::highlights::Highlighter;
use crate::key_strip::key_strip;
use crate::outcome_screen::{self, Outcome};
use crate::queue_screen;
use crate::review_screen::{self, Selected};
use crate::seen::Seen;
use crate::submit_dialog::{self, OpenSubmit, SubmitDialog};
use crate::text_area::{TextArea, TextAreaEvent};
use crate::theme;
use crate::verdict::Verdict;

pub struct Shell {
    pub(crate) snapshot: Snapshot,
    pub(crate) service: Service,
    /// The workspace, author and lane new comments are written with.
    pub(crate) identity: Identity,
    /// The outcomes the reviewer has looked at, kept beside the database.
    pub(crate) seen: Seen,
    /// The comment open on the outcome screen, if any.
    pub(crate) outcome: Option<Outcome>,
    /// `Queue` or `Diff` (the review screen), or the Files tab of it.
    pub(crate) screen: Screen,
    filter: Filter,
    /// The snapshot's queue under `filter`.
    queue: Queue,
    /// The file the review screen shows, if any.
    pub(crate) selected: Option<Selected>,
    /// The open comment composer, if any.
    pub(crate) composer: Option<Composer>,
    /// The open submit dialog, if any.
    submit: Option<SubmitDialog>,
    /// The repository's files, listed the first time a view needs them.
    pub(crate) listing: Option<Result<FileList, String>>,
    /// The open find-a-file overlay, if any.
    pub(crate) finder: Option<Finder>,
    /// The virtualised diff of the open file; see `diff_list`.
    pub(crate) diff_list: Option<DiffList>,
    /// Bumped whenever the open file or its snapshot is replaced, so the diff
    /// list knows its rows are stale.
    pub(crate) diff_generation: u64,
    /// Syntax colour of the open file, tokenized in the background.
    pub(crate) highlights: Highlighter,
    /// The delete the right panel offers to undo, or why one failed.
    pub(crate) removal: Option<Removal>,
    /// Keeps the Changes list's row in view when the keyboard moves.
    pub(crate) changes_scroll: UniformListScrollHandle,
    /// Keeps the Files tree's row in view when the keyboard moves.
    pub(crate) files_scroll: UniformListScrollHandle,
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
        seen: Seen,
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
            seen,
            outcome: None,
            screen,
            filter: Filter::default(),
            queue,
            selected,
            composer: None,
            submit: None,
            listing: None,
            finder: None,
            diff_list: None,
            diff_generation: 0,
            highlights: Highlighter::default(),
            removal: None,
            changes_scroll: UniformListScrollHandle::new(),
            files_scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
        }
    }

    /// The file a click or the finder opens: a change's diff, or, for a file
    /// the change does not touch, its content on disk.
    fn load_selected(&self, path: &str) -> Selected {
        if self.snapshot.changes.iter().any(|file| file.path == path) {
            Selected::load(&self.snapshot, path)
        } else {
            files_panel::load_outside(&self.snapshot.workspace, path)
        }
    }

    pub fn show(&mut self, screen: Screen, cx: &mut Context<Self>) {
        if screen == Screen::Files {
            self.ensure_listing();
        }
        self.screen = screen;
        self.removal = None;
        cx.notify();
    }

    pub fn select_file(&mut self, path: &str, cx: &mut Context<Self>) {
        self.selected = Some(self.load_selected(path));
        self.diff_generation = self.diff_generation.wrapping_add(1);
        self.composer = None;
        self.removal = None;
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
        self.removal = None;
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
        self.removal = None;
        self.install_composer(None, "", window, cx);
    }

    /// Re-reads the snapshot, with the seen outcomes, keeping the file open
    /// with a fresh selection.
    pub(crate) fn refresh(&mut self) -> Result<(), String> {
        let path = self.selected.as_ref().map(|selected| selected.path.clone());
        let acknowledged = self.seen.ids_for(&self.identity.workspace);
        let snapshot = data::load(
            &self.service,
            &self.identity.workspace,
            &self.identity.lane,
            acknowledged,
        )?;
        self.queue = snapshot.view(&self.filter);
        self.snapshot = snapshot;
        self.selected = path.map(|path| self.load_selected(&path));
        self.diff_generation = self.diff_generation.wrapping_add(1);
        Ok(())
    }

    /// Re-reads the snapshot after a write. A failed re-read keeps the
    /// composer open with its message.
    pub(crate) fn reload(&mut self, cx: &mut Context<Self>) {
        match self.refresh() {
            Ok(()) => self.composer = None,
            Err(message) => {
                if let Some(composer) = self.composer.as_mut() {
                    composer.error = Some(format!("Saved, but the view did not reload: {message}"));
                    composer.input.update(cx, TextArea::clear);
                }
            }
        }
        cx.notify();
    }

    /// Keys with no field open: C, and the file keys J, K and the arrows.
    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // The open dialog takes every key: Esc closes it from wherever focus is.
        if self.submit.is_some() {
            if event.keystroke.key == "escape" {
                self.close_submit(cx);
            }
            return;
        }
        if self.finder.is_some() {
            match event.keystroke.key.as_str() {
                "down" => self.move_highlight(true, cx),
                "up" => self.move_highlight(false, cx),
                _ => {}
            }
            return;
        }
        // With no field open, plain keys move through the review: C opens the
        // composer, and J or Down and K or Up select the next or previous file.
        if event.keystroke.modifiers.number_of_modifiers() != 0 || self.composer.is_some() {
            return;
        }
        if self.screen == Screen::Outcome {
            match event.keystroke.key.as_str() {
                "j" | "down" => self.step_outcome(1, cx),
                "k" | "up" => self.step_outcome(-1, cx),
                _ => {}
            }
            return;
        }
        let on_review = matches!(self.screen, Screen::Diff | Screen::Files);
        match event.keystroke.key.as_str() {
            "c" if on_review => self.open_composer(window, cx),
            "j" | "down" => self.step_file(1, cx),
            "k" | "up" => self.step_file(-1, cx),
            _ => {}
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

    /// Opens the submit dialog with the verdict the drafts suggest: request
    /// changes with drafts, comment without. Does nothing while the dialog or
    /// the finder is open.
    pub fn open_submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.submit.is_some() || self.finder.is_some() {
            return;
        }
        self.removal = None;
        let drafts = submit_dialog::draft_rows(&self.snapshot).len();
        let summary = cx.new(|cx| TextArea::new("Summary for the agent", cx).with_lines(2, 6));
        let events = cx.subscribe(
            &summary,
            |this: &mut Self, _, event: &TextAreaEvent, cx: &mut Context<Self>| {
                if *event == TextAreaEvent::Cancel {
                    this.close_submit(cx);
                }
            },
        );
        summary.update(cx, |area, cx| area.focus(window, cx));
        self.submit = Some(SubmitDialog::new(
            Verdict::for_drafts(drafts),
            summary,
            events,
        ));
        cx.notify();
    }

    pub fn close_submit(&mut self, cx: &mut Context<Self>) {
        self.submit = None;
        cx.notify();
    }

    pub fn pick_verdict(&mut self, verdict: Verdict, cx: &mut Context<Self>) {
        if let Some(dialog) = self.submit.as_mut() {
            dialog.verdict = verdict;
        }
        cx.notify();
    }

    /// Submits the drafts as one review. On success the dialog closes and the
    /// snapshot reloads; a refused submit shows its message in the dialog.
    pub fn send_submit(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.submit.as_ref() else {
            return;
        };
        let summary = dialog.summary.read(cx).text().to_owned();
        let decision = dialog.verdict.decision();
        let ids = submit_dialog::draft_rows(&self.snapshot)
            .into_iter()
            .map(|row| row.id)
            .collect();
        match data::submit_review(&self.service, &self.identity, decision, &summary, ids) {
            Ok(_) => {
                self.submit = None;
                self.reload(cx);
            }
            Err(message) => {
                if let Some(dialog) = self.submit.as_mut() {
                    dialog.error = Some(message);
                }
                cx.notify();
            }
        }
    }

    fn apply(&mut self, filter: Filter, cx: &mut Context<Self>) {
        self.queue = self.snapshot.view(&filter);
        self.filter = filter;
        self.removal = None;
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
        if self.composer.is_none()
            && self.submit.is_none()
            && self.finder.is_none()
            && !self.focus.is_focused(window)
        {
            self.focus.focus(window);
        }
        self.sync_highlights(cx);
        self.sync_diff_list(window);
        let panels = match (self.screen, self.outcome.as_ref()) {
            (Screen::Diff | Screen::Files, _) => Panels {
                left: self.review_left(cx),
                centre: review_screen::centre(
                    &self.snapshot,
                    self.selected.as_ref(),
                    self.diff_list.as_ref(),
                    cx,
                ),
                right: crate::draft_panel::right(&self.snapshot, self.removal.as_ref(), cx),
            },
            (Screen::Outcome, Some(open)) => Panels {
                left: outcome_screen::left(open, &self.snapshot, cx),
                centre: outcome_screen::centre(open),
                right: outcome_screen::right(open),
            },
            _ => Panels {
                left: queue_screen::left(&self.snapshot, &self.filter, cx),
                centre: queue_screen::centre(&self.snapshot, &self.queue, cx),
                right: queue_screen::right(&self.snapshot.next_pull),
            },
        };
        // The Files tab is part of the review screen, so R stays lit; the
        // outcome screen lights the screen it was opened from.
        let rail_screen = match (self.screen, self.outcome.as_ref()) {
            (Screen::Files, _) => Screen::Diff,
            (Screen::Outcome, Some(open)) => outcome_screen::rail_screen(open.back),
            (Screen::Outcome, None) => Screen::Queue,
            (screen, _) => screen,
        };
        let key_screen = if self.finder.is_some() {
            Screen::Search
        } else {
            self.screen
        };
        let window = frame::window(
            frame::top_bar(&self.snapshot),
            frame::rail(rail_screen, cx),
            panels,
            key_strip(key_screen, &self.snapshot.queue.counters),
        );

        div()
            .size_full()
            .bg(theme::paper())
            .font_family(theme::ui_font())
            .text_size(px(theme::BASE_FONT_SIZE))
            .text_color(theme::text())
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .on_action(cx.listener(|this: &mut Self, _: &OpenSubmit, window, cx| {
                // ⇧⌘R opens the dialog, and submits it when it is already open.
                if this.submit.is_some() {
                    this.send_submit(cx);
                } else {
                    this.open_submit(window, cx);
                }
            }))
            .on_action(cx.listener(|this: &mut Self, _: &OpenFinder, window, cx| {
                this.open_finder(window, cx);
            }))
            .child(window)
            .children(self.submit.as_ref().map(|dialog| {
                submit_dialog::overlay(dialog, &self.snapshot, &self.identity.lane, cx)
            }))
            .children(
                self.finder
                    .as_ref()
                    .map(|finder| finder::overlay(finder, cx)),
            )
    }
}

impl Shell {
    /// The left panel of the review screen: the changes, or the Files tab.
    fn review_left(&self, cx: &Context<Self>) -> Div {
        match self.screen {
            Screen::Files => files_panel::left(self.listing.as_ref(), &self.files_scroll, cx),
            _ => review_screen::left(&self.snapshot, &self.changes_scroll, cx),
        }
    }
}
