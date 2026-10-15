//! The application window. It owns the fetched snapshot, the screen shown,
//! the queue filter and the file open in the review screen. It re-runs the
//! queue view model when the filter changes and hands each screen's panels to
//! the common frame.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use gpui::prelude::*;
use gpui::{
    App, Context, Div, Entity, FocusHandle, Focusable, KeyDownEvent, Render,
    UniformListScrollHandle, Window, div, px,
};
use rvw_core::Service;
use rvw_view::keys::Screen;
use rvw_view::panels::{Panels as Shown, Side};
use rvw_view::queue::{Filter, Queue, State};
use rvw_view::selection::Pos;
use rvw_view::viewmode::ViewMode;

use crate::changes_state::{ChangesState, CommitChanges};
use crate::code_caret::{CodeCaret, CopyCode};
use crate::composer::Composer;
use crate::data::{self, Identity, Snapshot};
use crate::diff_list::DiffList;
use crate::diff_list::drag::DragState;
use crate::disk_card::DiskCard;
use crate::draft_edit::Removal;
use crate::editing::{Edit, SaveFile};
use crate::file_view;
use crate::files_panel::{self, FileList};
use crate::finder::{self, Finder, OpenFinder};
use crate::frame::{self, Panels, ToggleLeftPanel, ToggleRightPanel};
use crate::git_tools::{GitTools, RepoFacts};
use crate::highlights::Highlighter;
use crate::history_state::HistoryState;
use crate::live::{self, Fingerprint};
use crate::outcome_screen::{self, Outcome};
use crate::palette_overlay::OpenPalette;
use crate::queue_screen;
use crate::review_screen::{self, Selected};
use crate::submit_dialog::{OpenSubmit, SubmitDialog};
use crate::terminal_dock::{NewTerminal, TerminalView, ToggleTerminal};
use crate::terminal_launcher::Launcher;
use crate::text_area::TextArea;
use crate::theme;
use crate::tools::Tools;
use crate::workspace_picker::OpenWorkspaces;

pub struct Shell {
    pub(crate) snapshot: Snapshot,
    /// Shared with the live refresh, which reads the queue off the UI thread.
    pub(crate) service: Arc<Service>,
    /// The workspace, author and lane new comments are written with.
    pub(crate) identity: Identity,
    /// The outcomes the reviewer has looked at, kept beside the database.
    /// The comment open on the outcome screen, if any.
    pub(crate) outcome: Option<Outcome>,
    /// The comment asked for last, while it is read; see `open_outcome`.
    pub(crate) outcome_pending: Option<String>,
    /// `Queue` or `Diff` (the review screen), or the Files tab of it.
    pub(crate) screen: Screen,
    filter: Filter,
    /// The snapshot's queue under `filter`.
    queue: Queue,
    /// The file the review screen shows, if any.
    pub(crate) selected: Option<Selected>,
    /// The file read asked for last, while it runs off the UI thread; see
    /// `request_file`. The file on screen stays until it lands.
    pub(crate) file_pending: Option<FileRequest>,
    /// Numbers the file reads, so a read is told apart from a later one of
    /// the same file and mode.
    file_requests: u64,
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
    /// The width of one character of the code font; see `code_scroll`.
    pub(crate) code_advance: gpui::Pixels,
    /// Done and rejected comments are drawn in the diff too. Off by default,
    /// so a file with many decided comments stays readable; for the session.
    pub(crate) show_resolved: bool,
    /// Which side panels are shown; the window's, for every screen.
    pub(crate) shown: Shown,
    /// The view the reviewer chose for a file this session; see `file_view`.
    pub(crate) file_modes: BTreeMap<String, ViewMode>,
    /// The press-drag selection in progress; see `diff_list::drag`.
    pub(crate) drag: DragState,
    /// The caret and character selection of the code area; see `code_caret`.
    pub(crate) code_caret: CodeCaret,
    /// Syntax colour of the open file, tokenized in the background.
    pub(crate) highlights: Highlighter,
    /// The files edited in their whole-file view this session, by path; see
    /// `editing`.
    pub(crate) edits: BTreeMap<String, Edit>,
    /// Bumped by each edit, so the diff list rebuilds its rows.
    pub(crate) edit_revision: u64,
    /// The caret moved or the text changed: the diff list scrolls to the caret.
    pub(crate) reveal_caret: bool,
    /// A message shown at the foot of the window, such as a refused action.
    pub(crate) edit_notice: Option<String>,
    /// The card for an open file whose disk copy moved or went; see `disk_card`.
    pub(crate) disk_card: Option<DiskCard>,
    /// An Edit asked for: the file and the line its caret goes to, placed once
    /// the file is shown in its whole-file view; see `editing::apply_edit_jump`.
    pub(crate) edit_jump: Option<(String, usize)>,
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
    /// The History screen: the commit list, the selected commit and search.
    pub(crate) history: HistoryState,
    /// The open history edit card, if any.
    pub(crate) rewrite: Option<crate::rewrite_card::RewriteCard>,
    /// The commit gate's card, while a commit, amend or push waits on it.
    pub(crate) gate: Option<crate::gate_card::GateCard>,
    /// The reason typed in the gate card last, kept for the session.
    pub(crate) gate_reason: String,
    /// Numbers the gate's reads and overrides; see `gate_card`.
    pub(crate) gate_requests: u64,
    /// The terminal dock's terminal, created the first time it is shown.
    pub(crate) terminal: Option<Entity<TerminalView>>,
    /// The new-terminal palette (⌘T), when open.
    pub(crate) launcher: Option<Launcher>,
    /// The question asked before quitting with terminals running: the
    /// sentence that names them.
    pub(crate) quit_ask: Option<String>,
    /// Holds the keys (C) when no field has focus.
    focus: FocusHandle,
    /// The lane host this shell is one lane of; `None` before it is hosted.
    pub(crate) host: Option<gpui::WeakEntity<crate::lane_host::LaneHost>>,
    /// The host's opened lanes, shared so a lane never reads the host entity
    /// (the host is mid-update when it calls into a lane).
    pub(crate) lanes: crate::lane_host::SharedLanes,
    /// Pixels of the window width the host takes beside this shell.
    pub(crate) inset: f32,
    /// The database the queue lives in. The live poller fingerprints it
    /// beside the repository; see `live`.
    pub(crate) db_path: PathBuf,
    /// The fingerprint the window last refreshed to; `None` until the first
    /// poll records one. See `live`.
    pub(crate) live_last: Option<Fingerprint>,
    /// Bumped by every re-read the window makes on the UI thread, so a live
    /// read that started before one is dropped as stale. See `live`.
    pub(crate) live_epoch: u64,
}

impl Shell {
    /// Whether no text field wants the keyboard, so the shell holds the focus.
    fn no_field_open(&self, window: &Window, cx: &Context<Self>) -> bool {
        self.composer.is_none()
            && self.submit.is_none()
            && self.finder.is_none()
            && self.launcher.is_none()
            && !self.tools.field_open()
            && self.git.ask.is_none()
            && self.rewrite.as_ref().is_none_or(|card| !card.editing())
            && self.gate.as_ref().is_none_or(|card| !card.editing())
            && !self.message_focused(window, cx)
            && !self.terminal_focused(window, cx)
    }

    /// Whether a modal card (the history edit, the commit gate or the disk
    /// card) is open.
    fn card_open(&self) -> bool {
        self.rewrite.is_some() || self.gate.is_some() || self.disk_card_open()
    }

    /// Opens on the review screen when the workspace has changes, with its
    /// first file selected; otherwise on the queue.
    pub fn new(
        snapshot: Snapshot,
        service: Arc<Service>,
        identity: Identity,
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
            db_path,
            live_last: None,
            live_epoch: 0,
            outcome: None,
            outcome_pending: None,
            screen,
            filter: Filter::default(),
            queue,
            selected,
            file_pending: None,
            file_requests: 0,
            composer: None,
            submit: None,
            listing: None,
            finder: None,
            tools: Tools::default(),
            diff_list: None,
            diff_generation: 0,
            show_resolved: false,
            shown: Shown::default(),
            code_advance: crate::code_scroll::advance(window),
            file_modes: BTreeMap::new(),
            drag: DragState::default(),
            code_caret: CodeCaret::default(),
            highlights: Highlighter::default(),
            edits: BTreeMap::new(),
            edit_revision: 0,
            reveal_caret: false,
            edit_notice: None,
            disk_card: None,
            edit_jump: None,
            removal: None,
            changes_scroll: UniformListScrollHandle::new(),
            files_scroll: UniformListScrollHandle::new(),
            changes: ChangesState::default(),
            history: HistoryState::default(),
            rewrite: None,
            gate: None,
            gate_reason: String::new(),
            gate_requests: 0,
            git: GitTools {
                facts,
                ..GitTools::default()
            },
            terminal: None,
            launcher: None,
            quit_ask: None,
            focus: cx.focus_handle(),
            host: None,
            lanes: crate::lane_host::SharedLanes::default(),
            inset: 0.,
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
        if screen == Screen::History && self.screen != Screen::History {
            self.enter_history(cx);
        }
        self.screen = screen;
        self.outcome_pending = None;
        self.removal = None;
        cx.notify();
    }

    /// Shows the file at `path` in the mode it opens in. The read runs off the
    /// UI thread, and the file on screen stays until it lands; see
    /// `request_file`.
    pub fn select_file(&mut self, path: &str, cx: &mut Context<Self>) {
        let mode = self.mode_for(path);
        self.request_file(path, mode, false, cx);
    }

    /// Reads `path` in `mode` off the UI thread, then shows it with
    /// `show_selected` when the read lands. Only the inputs the read needs are
    /// copied, not the snapshot. `switched` marks a change of view for the
    /// open file, which also remembers the mode.
    pub(crate) fn request_file(
        &mut self,
        path: &str,
        mode: ViewMode,
        switched: bool,
        cx: &mut Context<Self>,
    ) {
        self.file_requests = self.file_requests.wrapping_add(1);
        let id = self.file_requests;
        self.file_pending = Some(FileRequest {
            id,
            path: path.to_owned(),
            mode,
        });
        let workspace = self.snapshot.workspace.clone();
        let base_id = self.snapshot.base_id.clone();
        let changed = self.snapshot.changes.iter().any(|file| file.path == path);
        let read_path = path.to_owned();
        cx.spawn(async move |this, cx| {
            let selected = cx
                .background_executor()
                .spawn(async move {
                    file_view::read_selected(&workspace, &base_id, changed, &read_path, mode)
                })
                .await;
            this.update(cx, |this, cx| {
                this.show_selected(id, switched, selected, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Shows what read `id` of `request_file` read, when it is still the
    /// read asked for last. A change of view also remembers the mode and
    /// starts the diff list at the top, since its rows differ by mode.
    fn show_selected(
        &mut self,
        id: u64,
        switched: bool,
        mut selected: Selected,
        cx: &mut Context<Self>,
    ) {
        let Some(request) = self.file_pending.take_if(|request| request.id == id) else {
            return;
        };
        self.overlay_edit(&mut selected);
        if self
            .edit_jump
            .as_ref()
            .is_some_and(|(path, _)| *path != request.path)
        {
            self.edit_jump = None;
        }
        if switched {
            self.file_modes.insert(request.path, request.mode);
            // Row indices differ between the modes, so the list starts at the top.
            self.diff_list = None;
        }
        self.selected = Some(selected);
        self.diff_generation = self.diff_generation.wrapping_add(1);
        self.composer = None;
        self.removal = None;
        cx.notify();
    }

    /// The file on screen as its path and mode: the one asked for last while
    /// its read runs, else the one shown.
    pub(crate) fn shown_file(&self) -> Option<(String, ViewMode)> {
        match self.file_pending.as_ref() {
            Some(request) => Some((request.path.clone(), request.mode)),
            None => self
                .selected
                .as_ref()
                .map(|selected| (selected.path.clone(), selected.mode)),
        }
    }

    /// A sideways wheel or swipe over the review's diff moves its code.
    pub(crate) fn scroll_code(&mut self, event: &gpui::ScrollWheelEvent, cx: &mut Context<Self>) {
        let delta = crate::code_scroll::sideways(event);
        if delta == gpui::Pixels::ZERO {
            return;
        }
        if self
            .diff_list
            .as_ref()
            .is_some_and(|list| list.scroll_code(delta))
        {
            cx.notify();
        }
    }

    /// Shows or hides the done and rejected comments in the diff.
    pub fn toggle_resolved(&mut self, cx: &mut Context<Self>) {
        self.show_resolved = !self.show_resolved;
        self.diff_generation = self.diff_generation.wrapping_add(1);
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
        if self.refuse_unsaved_comment() {
            cx.notify();
            return;
        }
        self.removal = None;
        self.install_composer(None, "", window, cx);
    }

    /// Re-reads the snapshot, keeping the file open
    /// with a fresh selection.
    pub(crate) fn refresh(&mut self) -> Result<(), String> {
        let path = self.selected.as_ref().map(|selected| selected.path.clone());
        let snapshot = data::load(
            &self.service,
            &self.identity.workspace,
            &self.identity.lane,
            self.tools.compare,
        )?;
        self.live_epoch = self.live_epoch.wrapping_add(1);
        self.queue = snapshot.view(&self.filter);
        self.snapshot = snapshot;
        let mode = self.selected.as_ref().map(|selected| selected.mode);
        let loaded = path.map(|path| {
            let mode = mode.unwrap_or_else(|| self.mode_for(&path));
            self.load_selected(&path, mode)
        });
        self.selected = loaded;
        // A read asked for before this reload must not replace the file it reloads.
        self.file_pending = None;
        self.diff_generation = self.diff_generation.wrapping_add(1);
        Ok(())
    }

    /// Shows a snapshot read like `refresh` does, but keeps the open file's
    /// selection and its diff list when the file reads the same: its diff,
    /// view, and flags. `fresh` is the open file read against `snapshot` in
    /// the same view. The diff list is rebuilt only when the file's diff
    /// changed or its comments did, since its rows hold the comments. While a
    /// composer is open the file keeps its old diff, since the composer sits
    /// under the selection; the result is `false` then, and the file is
    /// re-read once the composer closes. A file read still pending is `false`
    /// too: that read may have started before this disk change, so the next
    /// tick reads again, against the file the reviewer asked for.
    pub(crate) fn apply_snapshot(&mut self, snapshot: Snapshot, fresh: Option<Selected>) -> bool {
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
            return self.file_pending.is_none();
        };
        let fresh = match fresh {
            Some(mut fresh) => {
                self.overlay_edit(&mut fresh);
                fresh
            }
            None => self.load_selected(&old.path, old.mode),
        };
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
            return false;
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
        self.file_pending.is_none()
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
        // A notice shown for the last key lasts until the next one.
        self.edit_notice = None;
        // The quit question takes Enter and Esc before anything else.
        if self.quit_key(&event.keystroke.key, cx) {
            return;
        }
        // A focused terminal has its keys; the window's plain keys are not its.
        if self.terminal_focused(window, cx) {
            return;
        }
        // The open dialog takes every key: Esc closes it from wherever focus is.
        if self.submit.is_some() {
            if event.keystroke.key == "escape" {
                self.close_submit(cx);
            }
            return;
        }
        if self.gate_key(event, cx)
            || self.rewrite_key(event, window, cx)
            || self.restore_key(event, cx)
            || self.git_ask_key(event, cx)
        {
            return;
        }
        if self.launcher.is_some() {
            match event.keystroke.key.as_str() {
                "down" => self.move_launcher(true, cx),
                "up" => self.move_launcher(false, cx),
                _ => {}
            }
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
        // The disk card takes the keys while it is up: Esc closes a card for a
        // missing file, and nothing reaches the buffer behind it.
        if self.disk_card_open() && event.keystroke.modifiers.number_of_modifiers() == 0 {
            if event.keystroke.key == "escape" {
                self.close_disk_card(cx);
            }
            return;
        }
        // With the caret in an editable file, the keys type into it, before
        // the window's own single keys (C, J, K) see them.
        if self.no_field_open(window, cx)
            && self.edit_key(
                &event.keystroke.key,
                event.keystroke.key_char.as_deref(),
                event.keystroke.modifiers,
                cx,
            )
        {
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
        if self.screen == Screen::History {
            match event.keystroke.key.as_str() {
                "j" | "down" => self.step_commit(1, cx),
                "k" | "up" => self.step_commit(-1, cx),
                "/" => self.focus_search(window, cx),
                "escape" => self.close_commit_file(cx),
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
            "e" if on_review => self.edit_here(cx),
            "j" | "down" => self.step_file(1, cx),
            "k" | "up" => self.step_file(-1, cx),
            _ => {}
        }
    }

    /// ⌘C: copies the code's selection, outside a focused terminal or field.
    fn on_copy_code(&mut self, _: &CopyCode, window: &mut Window, cx: &mut Context<Self>) {
        if !self.terminal_focused(window, cx) && !self.message_focused(window, cx) {
            self.copy_code(cx);
        }
    }

    /// ⌘S: writes the open file's unsaved edits, outside the text fields and the terminal.
    pub(crate) fn on_save_file(
        &mut self,
        _: &SaveFile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.terminal_focused(window, cx) && !self.message_focused(window, cx) {
            self.save_file(cx);
        }
    }

    /// Whether the Edit action is offered on the open file: it is a changed
    /// file's diff, editable as text, with no Compare open.
    pub(crate) fn edit_offered(&self) -> bool {
        self.tools.compare.is_none()
            && self.selected.as_ref().is_some_and(|selected| {
                selected.editing.offered() && selected.changed && selected.mode == ViewMode::Diff
            })
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

impl Shell {
    /// Brings the focus, the highlights and the diff list in line with the
    /// state before the window is drawn.
    fn sync_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // With no field open, the shell holds the focus so that C reaches it.
        // The restore confirmation is modal and has no field, so it always
        // takes the keyboard to the shell, from the commit message too.
        let no_field = self.no_field_open(window, cx);
        self.focus_gate(window, cx);
        if (no_field || self.git.confirm.is_some()) && !self.focus.is_focused(window) {
            self.focus.focus(window);
        }
        // A file with unsaved edits is drawn plain: its spans would not match its lines.
        if !self.file_dirty() {
            self.sync_highlights(cx);
        }
        self.sync_diff_list(window);
        self.apply_edit_jump();
        self.reveal_pending_caret();
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_frame(window, cx);
        let panels = self.panels(cx);
        let dock = self.terminal_dock(cx);
        let rail_screen = self.rail_screen();
        let resize_layer = crate::window_chrome::resize_layer(window);
        let extent = f32::from(window.viewport_size().width) - self.inset;
        let window = frame::window(
            frame::top_bar(
                &self.snapshot,
                &self.git,
                self.tools.compare_menu.as_ref(),
                self.shown,
                window,
                cx,
            ),
            frame::rail(rail_screen, cx),
            panels,
            self.shown,
            extent,
            dock,
            cx,
        );

        div()
            .size_full()
            .bg(theme::paper())
            .font_family(theme::ui_font())
            .text_size(px(theme::BASE_FONT_SIZE))
            .text_color(theme::text())
            .key_context(crate::terminal_dock::APP_CONTEXT)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .on_action(cx.listener(|this: &mut Self, _: &OpenSubmit, window, cx| {
                // The history edit card and the commit gate's card are modal.
                if this.card_open() {
                    return;
                }
                // ⇧⌘R opens the dialog, and submits it when it is already open.
                if this.submit.is_some() {
                    this.send_submit(cx);
                } else {
                    this.open_submit(window, cx);
                }
            }))
            .on_action(cx.listener(|this: &mut Self, _: &OpenFinder, window, cx| {
                // The history edit card and the commit gate's card are modal.
                if this.card_open() {
                    return;
                }
                this.open_finder(window, cx);
            }))
            .on_action(cx.listener(|this: &mut Self, _: &OpenPalette, window, cx| {
                // The history edit card and the commit gate's card are modal.
                if this.card_open() {
                    return;
                }
                this.open_palette(window, cx);
            }))
            .on_action(
                cx.listener(|this: &mut Self, _: &OpenWorkspaces, window, cx| {
                    // The history edit card and the commit gate's card are modal.
                    if this.card_open() {
                        return;
                    }
                    this.open_workspaces(window, cx);
                }),
            )
            .on_action(cx.listener(|this: &mut Self, _: &CommitChanges, _, cx| {
                // The history edit card is modal.
                if this.rewrite.is_some() {
                    return;
                }
                this.commit_changes(cx);
            }))
            .on_action(cx.listener(Self::on_restore_latest))
            .on_action(cx.listener(Self::on_edit_redo))
            .on_action(cx.listener(Self::on_save_file))
            .on_action(cx.listener(Self::on_edit_cut))
            .on_action(cx.listener(Self::on_edit_paste))
            .on_action(cx.listener(Self::on_copy_code))
            .on_action(cx.listener(Self::on_toggle_left_panel))
            .on_action(cx.listener(Self::on_toggle_right_panel))
            .on_action(cx.listener(Self::on_toggle_terminal))
            .on_action(cx.listener(Self::on_new_terminal))
            .on_action(cx.listener(Self::on_quit))
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
            .children(self.rewrite_overlay(cx))
            .children(
                self.gate_overlay(cx)
                    .into_iter()
                    .chain(self.disk_overlay(cx)),
            )
            .children(self.edit_notice.as_deref().map(notice_toast))
            .children(resize_layer)
    }
}

impl Shell {
    /// Whether a text field of the shell holds the keyboard: the commit message
    /// while it is focused, or the reopen note while it is open. Keys do not
    /// reach the shell then.
    pub(crate) fn message_focused(&self, window: &Window, cx: &App) -> bool {
        let commit = self
            .changes
            .message
            .as_ref()
            .is_some_and(|message| message.focus_handle(cx).is_focused(window));
        commit || self.reopening()
    }

    /// The screen the rail lights. The Files tab is part of the review
    /// screen, so R stays lit; the outcome screen lights the screen it was
    /// opened from.
    fn rail_screen(&self) -> Screen {
        match (self.screen, self.outcome.as_ref()) {
            (Screen::Files, _) => Screen::Diff,
            (Screen::Outcome, Some(open)) => outcome_screen::rail_screen(open.back),
            (Screen::Outcome, None) => Screen::Queue,
            (screen, _) => screen,
        }
    }

    /// ⌘J, or Ctrl-Shift-J: the terminal dock.
    fn on_toggle_terminal(
        &mut self,
        _: &ToggleTerminal,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The history edit card is modal.
        if self.rewrite.is_none() {
            self.toggle_terminal(window, cx);
        }
    }

    /// ⌘T, or Ctrl-Shift-T: the new-terminal palette.
    fn on_new_terminal(&mut self, _: &NewTerminal, window: &mut Window, cx: &mut Context<Self>) {
        // The history edit card is modal.
        if self.rewrite.is_none() {
            self.open_launcher(window, cx);
        }
    }

    /// ⌘Q, Ctrl-Shift-Q, or the Close control: quits, asking first when
    /// terminals are running.
    fn on_quit(&mut self, _: &crate::Quit, window: &mut Window, cx: &mut Context<Self>) {
        self.request_quit(window, cx);
    }

    /// ⌘B, or Ctrl-B: the left panel.
    fn on_toggle_left_panel(
        &mut self,
        _: &ToggleLeftPanel,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_panel(Side::Left, cx);
    }

    /// ⇧⌘B, or Ctrl-Shift-B: the right panel.
    fn on_toggle_right_panel(
        &mut self,
        _: &ToggleRightPanel,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_panel(Side::Right, cx);
    }

    /// Collapses a side panel, or shows it again; the centre takes its room.
    pub(crate) fn toggle_panel(&mut self, side: Side, cx: &mut Context<Self>) {
        // The history edit card is modal.
        if self.rewrite.is_none() {
            self.shown.toggle(side);
            cx.notify();
        }
    }

    /// The terminal dock, when it is shown.
    fn terminal_dock(&self, cx: &App) -> Option<gpui::AnyElement> {
        self.terminal
            .as_ref()
            .filter(|terminal| terminal.read(cx).shown())
            .map(crate::terminal_dock::dock)
    }

    /// Whether the terminal dock's terminal has the keyboard.
    pub(crate) fn terminal_focused(&self, window: &Window, cx: &App) -> bool {
        self.terminal
            .as_ref()
            .is_some_and(|terminal| terminal.read(cx).has_focus(window, cx))
    }

    /// Shows the terminal dock, starting the terminal the first time, or
    /// hides it. Hiding gives the keyboard back to the window.
    pub(crate) fn toggle_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let workspace = PathBuf::from(&self.identity.workspace);
        let terminal = self
            .terminal
            .get_or_insert_with(|| cx.new(|cx| TerminalView::new(workspace, cx)))
            .clone();
        terminal.update(cx, |terminal, cx| terminal.toggle(window, cx));
        if !terminal.read(cx).shown() {
            self.focus_shell(window);
        }
        cx.notify();
    }

    /// Starts a terminal in the workspace root and shows the dock with it.
    pub(crate) fn start_terminal(
        &mut self,
        launch: &rvw_view::terminals::Launch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let workspace = PathBuf::from(&self.identity.workspace);
        let terminal = self
            .terminal
            .get_or_insert_with(|| cx.new(|cx| TerminalView::new(workspace, cx)))
            .clone();
        terminal.update(cx, |terminal, cx| terminal.launch(launch, window, cx));
        cx.notify();
    }

    /// Gives the keyboard back to the shell, so S, J and K reach it.
    pub(crate) fn focus_shell(&self, window: &mut Window) {
        self.focus.focus(window);
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
                    self.show_resolved,
                    self.edit_offered(),
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
            (Screen::History, _) => Panels {
                left: crate::history_screen::left(&self.history, cx),
                centre: crate::history_screen::centre(&self.history, cx),
                right: crate::history_screen::right(&self.history, cx),
            },
            (Screen::Outcome, Some(open)) => Panels {
                left: outcome_screen::left(open, &self.snapshot, cx),
                centre: outcome_screen::centre(open, cx),
                right: outcome_screen::right(open),
            },
            _ => Panels {
                left: queue_screen::left(&self.snapshot, &self.filter, cx),
                centre: queue_screen::centre(&self.snapshot, &self.queue, cx),
                right: queue_screen::right(&self.snapshot.handoff),
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

/// The message at the foot of the window: a line on a card-like band.
fn notice_toast(text: &str) -> Div {
    div()
        .absolute()
        .left(px(16.))
        .right(px(16.))
        .bottom(px(16.))
        .px(px(12.))
        .py(px(8.))
        .rounded(px(6.))
        .bg(theme::paper())
        .border_1()
        .border_color(theme::hairline())
        .text_color(theme::ink())
        .child(text.to_owned())
}

/// A file read asked for: which file, in which view, and its number.
pub(crate) struct FileRequest {
    id: u64,
    pub(crate) path: String,
    pub(crate) mode: ViewMode,
}
