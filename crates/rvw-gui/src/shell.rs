//! The application window. It owns the fetched snapshot, the screen shown,
//! the queue filter and the file open in the review screen. It re-runs the
//! queue view model when the filter changes and hands each screen's panels to
//! the common frame.

use std::collections::BTreeMap;
use std::path::PathBuf;

use gpui::prelude::*;
use gpui::{
    div, px, App, Context, Div, FocusHandle, Focusable, KeyDownEvent, Render,
    UniformListScrollHandle, Window,
};
use rvw_core::Service;
use rvw_view::keys::{Context as KeyContext, Screen};
use rvw_view::queue::{Filter, Queue, State};
use rvw_view::selection::Pos;
use rvw_view::viewmode::ViewMode;

use crate::changes_state::{ChangesState, CommitChanges, RestoreLatest};
use crate::composer::Composer;
use crate::data::{self, Identity, Snapshot};
use crate::diff_list::drag::DragState;
use crate::diff_list::DiffList;
use crate::draft_edit::Removal;
use crate::files_panel::{self, FileList};
use crate::finder::{self, Finder, OpenFinder};
use crate::frame::{self, Panels};
use crate::git_tools::{GitTools, RepoFacts};
use crate::highlights::Highlighter;
use crate::key_strip::key_strip;
use crate::live::{self, Fingerprint};
use crate::outcome_screen::{self, Outcome};
use crate::palette_overlay::OpenPalette;
use crate::queue_screen;
use crate::review_screen::{self, Selected};
use crate::seen::Seen;
use crate::submit_dialog::{OpenSubmit, SubmitDialog};
use crate::text_area::TextArea;
use crate::theme;
use crate::tools::Tools;

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
    pub(crate) submit: Option<SubmitDialog>,
    /// The repository's files, listed the first time a view needs them.
    pub(crate) listing: Option<Result<FileList, String>>,
    /// The open find-a-file overlay, if any.
    pub(crate) finder: Option<Finder>,
    /// The compare menu, branch picker, action palette and diff mode.
    pub(crate) tools: Tools,
    /// The virtualised diff of the open file; see `diff_list`.
    pub(crate) diff_list: Option<DiffList>,
    /// Bumped whenever the open file or its snapshot is replaced, so the diff
    /// list knows its rows are stale.
    pub(crate) diff_generation: u64,
    /// The view the reviewer chose for a file this session; see `file_view`.
    pub(crate) file_modes: BTreeMap<String, ViewMode>,
    /// The press-drag selection in progress; see `diff_list::drag`.
    pub(crate) drag: DragState,
    /// Syntax colour of the open file, tokenized in the background.
    pub(crate) highlights: Highlighter,
    /// The delete the right panel offers to undo, or why one failed.
    pub(crate) removal: Option<Removal>,
    /// Keeps the Changes list's row in view when the keyboard moves.
    pub(crate) changes_scroll: UniformListScrollHandle,
    /// Keeps the Files tree's row in view when the keyboard moves.
    pub(crate) files_scroll: UniformListScrollHandle,
    /// The Changes screen: staging, the commit box and the checkpoints.
    pub(crate) changes: ChangesState,
    /// Repository facts, the git prompts and pickers, and the restore
    /// confirmation.
    pub(crate) git: GitTools,
    /// Holds the keys (C) when no field has focus.
    focus: FocusHandle,
    /// The database the queue lives in. The live poller fingerprints it
    /// beside the repository; see `live`.
    pub(crate) db_path: PathBuf,
    /// The fingerprint the window last refreshed to; `None` until the first
    /// poll records one. See `live`.
    pub(crate) live_last: Option<Fingerprint>,
}

impl Shell {
    /// Opens on the review screen when the workspace has changes, with its
    /// first file selected; otherwise on the queue.
    pub fn new(
        snapshot: Snapshot,
        service: Service,
        identity: Identity,
        seen: Seen,
        db_path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let queue = snapshot.queue.clone();
        let facts = RepoFacts::read(std::path::Path::new(&identity.workspace));
        let screen = if snapshot.changes.is_empty() {
            Screen::Queue
        } else {
            Screen::Diff
        };
        let selected = snapshot
            .changes
            .first()
            .map(|file| Selected::load(&snapshot, &file.path));
        // A change of the OS light/dark setting repaints with the other palette.
        cx.observe_window_appearance(window, |_, window, cx| {
            if theme::follow(window.appearance()) {
                cx.notify();
            }
        })
        .detach();
        live::start(cx);
        let mut shell = Self {
            snapshot,
            service,
            identity,
            seen,
            db_path,
            live_last: None,
            outcome: None,
            screen,
            filter: Filter::default(),
            queue,
            selected,
            composer: None,
            submit: None,
            listing: None,
            finder: None,
            tools: Tools::default(),
            diff_list: None,
            diff_generation: 0,
            file_modes: BTreeMap::new(),
            drag: DragState::default(),
            highlights: Highlighter::default(),
            removal: None,
            changes_scroll: UniformListScrollHandle::new(),
            files_scroll: UniformListScrollHandle::new(),
            changes: ChangesState::default(),
            git: GitTools {
                facts,
                ..GitTools::default()
            },
            focus: cx.focus_handle(),
        };
        // The staged list lights Commit on every screen, so it is read now,
        // not when the Changes screen first opens.
        shell.reload_changes();
        shell
    }

    pub fn show(&mut self, screen: Screen, cx: &mut Context<Self>) {
        if screen == Screen::Files {
            self.ensure_listing();
        }
        if screen == Screen::Changes {
            self.enter_changes(cx);
        }
        self.screen = screen;
        self.removal = None;
        cx.notify();
    }

    pub fn select_file(&mut self, path: &str, cx: &mut Context<Self>) {
        let mode = self.mode_for(path);
        self.selected = Some(self.load_selected(path, mode));
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
            self.tools.compare,
        )?;
        self.queue = snapshot.view(&self.filter);
        self.snapshot = snapshot;
        let mode = self.selected.as_ref().map(|selected| selected.mode);
        self.selected = path.map(|path| {
            let mode = mode.unwrap_or_else(|| self.mode_for(&path));
            self.load_selected(&path, mode)
        });
        self.diff_generation = self.diff_generation.wrapping_add(1);
        Ok(())
    }

    /// Re-reads the snapshot like `refresh`, but keeps the open file's
    /// selection and its diff list when the file reads the same: its diff,
    /// view, and flags. The diff list is rebuilt only when the file's diff
    /// changed or its comments did, since its rows hold the comments. While a
    /// composer is open the file keeps its old diff, since the composer sits
    /// under the selection; the result is `false` then, and the file is
    /// re-read once the composer closes. A failed read changes nothing.
    pub(crate) fn refresh_keeping(&mut self) -> Result<bool, String> {
        let acknowledged = self.seen.ids_for(&self.identity.workspace);
        let snapshot = data::load(
            &self.service,
            &self.identity.workspace,
            &self.identity.lane,
            acknowledged,
            self.tools.compare,
        )?;
        let queue = snapshot.view(&self.filter);
        let file_comments = |snapshot: &Snapshot, path: &str| {
            snapshot
                .comments
                .iter()
                .filter(|comment| comment.file == path)
                .cloned()
                .collect::<Vec<_>>()
        };
        let previous = self.selected.take();
        let before = previous
            .as_ref()
            .map(|old| file_comments(&self.snapshot, &old.path));
        self.snapshot = snapshot;
        self.queue = queue;
        let (Some(old), Some(before)) = (previous, before) else {
            return Ok(true);
        };
        let fresh = self.load_selected(&old.path, old.mode);
        let moved = file_comments(&self.snapshot, &old.path) != before;
        let same_file = fresh.diff == old.diff
            && fresh.mode == old.mode
            && fresh.outside == old.outside
            && fresh.cut == old.cut
            && fresh.changed == old.changed;
        if !same_file && self.composer.is_some() {
            // The composer's place is its selection in this diff, so the
            // diff stays until the composer closes.
            self.selected = Some(old);
            if moved {
                self.diff_generation = self.diff_generation.wrapping_add(1);
            }
            return Ok(false);
        }
        if same_file {
            // The same diff has the same lines, so the reviewer's selection
            // still points at what it did.
            self.selected = Some(Selected {
                selection: old.selection,
                ..fresh
            });
        } else {
            // The lines moved or changed, so the selection is cleared.
            self.selected = Some(fresh);
        }
        if !same_file || moved {
            self.diff_generation = self.diff_generation.wrapping_add(1);
        }
        Ok(true)
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
        if self.restore_key(event, cx) || self.git_ask_key(event, cx) {
            return;
        }
        if self.tools_key(event, cx) {
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
        if event.keystroke.modifiers.number_of_modifiers() != 0
            || self.composer.is_some()
            || self.message_focused(window, cx)
        {
            return;
        }
        if self.screen == Screen::Changes {
            match event.keystroke.key.as_str() {
                "j" | "down" => self.step_change(1, cx),
                "k" | "up" => self.step_change(-1, cx),
                "s" => self.move_chosen(cx),
                _ => {}
            }
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
        // The restore confirmation is modal and has no field, so it always
        // takes the keyboard to the shell, from the commit message too.
        let no_field = self.composer.is_none()
            && self.submit.is_none()
            && self.finder.is_none()
            && !self.tools.field_open()
            && self.git.ask.is_none()
            && !self.message_focused(window, cx);
        if (no_field || self.git.confirm.is_some()) && !self.focus.is_focused(window) {
            self.focus.focus(window);
        }
        self.sync_highlights(cx);
        self.sync_diff_list(window);
        let panels = self.panels(cx);
        // The Files tab is part of the review screen, so R stays lit; the
        // outcome screen lights the screen it was opened from.
        let rail_screen = match (self.screen, self.outcome.as_ref()) {
            (Screen::Files, _) => Screen::Diff,
            (Screen::Outcome, Some(open)) => outcome_screen::rail_screen(open.back),
            (Screen::Outcome, None) => Screen::Queue,
            (screen, _) => screen,
        };
        let key_screen = if self.finder.is_some() || self.tools.open() {
            Screen::Search
        } else {
            self.screen
        };
        let resize_layer = crate::window_chrome::resize_layer(window);
        let window = frame::window(
            frame::top_bar(
                &self.snapshot,
                &self.git,
                self.tools.compare_menu.as_ref(),
                window,
                cx,
            ),
            frame::rail(rail_screen, cx),
            panels,
            key_strip(&self.key_context(key_screen)),
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
            .on_action(cx.listener(|this: &mut Self, _: &OpenPalette, window, cx| {
                this.open_palette(window, cx);
            }))
            .on_action(cx.listener(|this: &mut Self, _: &CommitChanges, _, cx| {
                this.commit_changes(cx);
            }))
            .on_action(cx.listener(|this: &mut Self, _: &RestoreLatest, _, cx| {
                this.restore_latest(cx);
            }))
            .child(window)
            .children(self.submit.as_ref().map(|dialog| {
                crate::submit_dialog::overlay(dialog, &self.snapshot, &self.identity.lane, cx)
            }))
            .children(
                self.finder
                    .as_ref()
                    .map(|finder| finder::overlay(finder, cx)),
            )
            .children(self.tool_overlays(cx))
            .children(resize_layer)
    }
}

impl Shell {
    /// Whether the commit message field has the keyboard.
    pub(crate) fn message_focused(&self, window: &Window, cx: &App) -> bool {
        self.changes
            .message
            .as_ref()
            .is_some_and(|message| message.focus_handle(cx).is_focused(window))
    }

    /// Gives the keyboard back to the shell, so S, J and K reach it.
    pub(crate) fn focus_shell(&self, window: &mut Window) {
        self.focus.focus(window);
    }

    /// The facts the key strip lights its keys from.
    fn key_context(&self, screen: Screen) -> KeyContext {
        KeyContext {
            screen,
            composing: self.composer.is_some(),
            draft_count: self.snapshot.queue.counters.draft,
            comment_done: false,
            staged_count: self.changes.lists.staged.len(),
            can_undo: !self.git.facts.checkpoints.is_empty(),
            can_move: true,
            can_open_file: false,
        }
    }

    /// The three panels of the screen shown.
    fn panels(&self, cx: &mut Context<Self>) -> Panels {
        match (self.screen, self.outcome.as_ref()) {
            (Screen::Diff | Screen::Files, _) => Panels {
                left: self.review_left(cx),
                centre: review_screen::centre(
                    &self.snapshot,
                    self.selected.as_ref(),
                    self.diff_list.as_ref(),
                    self.split_available().then_some(self.tools.split),
                    cx,
                ),
                right: crate::draft_panel::right(&self.snapshot, self.removal.as_ref(), cx),
            },
            (Screen::Changes, _) => Panels {
                left: crate::changes_screen::left(&self.changes, cx),
                centre: crate::changes_screen::centre(&self.changes, cx),
                right: crate::changes_screen::right(
                    &self.changes,
                    &self.git.facts,
                    &self.snapshot.comments,
                    cx,
                ),
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
        }
    }

    /// The left panel of the review screen: the changes, or the Files tab.
    fn review_left(&self, cx: &Context<Self>) -> Div {
        match self.screen {
            Screen::Files => files_panel::left(self.listing.as_ref(), &self.files_scroll, cx),
            _ => review_screen::left(&self.snapshot, &self.changes_scroll, cx),
        }
    }
}
