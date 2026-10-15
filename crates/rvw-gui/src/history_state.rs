//! The History screen's state and reads: the commit list a page at a time,
//! the selected commit's details, and the diff of one file in it. Every git
//! read runs off the UI thread. A read carries the request number it was
//! asked for, and one that a newer request overtook is dropped; a replaced
//! list read is also stopped, so a search the next keystroke replaces does not
//! keep git running.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{AppContext, Context, Entity, Subscription, UniformListScrollHandle, Window};
use rvw_core::types::Session;
use rvw_git::history::{self, CommitDetail, FileChange, LogEntry, LogQuery};
use rvw_git::search::{self, Field};
use rvw_git::{GitError, blame};
use rvw_view::diff::{FileDiff, parse};
use rvw_view::history::{History, PAGE};
use rvw_view::selection::Pos;

use crate::code_scroll;
use crate::data;
use crate::git_tools::last_line;
use crate::shell::Shell;
use crate::text_input::{TextInput, TextInputEvent};

/// Lines of context in a file's diff.
pub(crate) const CONTEXT: usize = 3;

/// The width of the two line-number columns left of the code in the open
/// file's diff; `history_screen` draws them this wide.
pub const HISTORY_CODE_GUTTER: gpui::Pixels = gpui::px(114.);

/// The width left of the code in the blame: the short id, author, age and
/// line number with four 8px gaps, and the 6px before the code.
pub const BLAME_CODE_GUTTER: gpui::Pixels = gpui::px(288.);

/// Which commits the list shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// The log of HEAD, or of every branch, remote branch and tag.
    Log { all: bool },
    /// Commits matching a search.
    Search {
        text: String,
        field: Field,
        all: bool,
    },
    /// The commits that changed one file, following renames.
    File { path: String },
}

impl Source {
    /// The graph is drawn only for the whole log; a search or a file's
    /// history skips commits, so its lanes would mean nothing.
    pub fn draws_graph(&self) -> bool {
        matches!(self, Source::Log { .. })
    }
}

/// A row of the open file's diff: a hunk's header, or one of its lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffRow {
    Header(usize),
    Line(Pos),
}

/// One file of the selected commit, open in the centre.
pub struct OpenCommitFile {
    pub commit: String,
    pub path: String,
    pub diff: Arc<FileDiff>,
    pub rows: Vec<DiffRow>,
    /// The columns of the longest line; see `code_scroll`.
    pub longest_line: usize,
}

/// The blame of one file on disk: a line per line of the file.
pub struct BlameView {
    pub path: String,
    pub lines: Vec<blame::BlameLine>,
    /// The columns of the longest line; a tab counts as four.
    pub longest_line: usize,
}

/// A comparison of two revisions: the files that differ between them.
pub struct CompareView {
    /// The branch or tag chosen in the picker.
    pub from: String,
    /// The full id of the commit selected when the comparison started.
    pub to: String,
    pub files: Vec<FileChange>,
}

pub struct HistoryState {
    pub history: History,
    pub source: Source,
    /// Search every branch, remote branch and tag, not only HEAD.
    pub all: bool,
    /// What the search field matches.
    pub field: Field,
    /// The search field, made the first time the screen is shown.
    pub search: Option<Entity<TextInput>>,
    /// The sessions agents reported in the workspace, newest first; read
    /// again whenever the list is. Empty when they cannot be read.
    pub sessions: Vec<Session>,
    /// A page is being read.
    pub loading: bool,
    /// Why the last page could not be read.
    pub error: Option<String>,
    /// The selected commit's details, or why they could not be read.
    pub detail: Option<Result<CommitDetail, String>>,
    /// The file of the selected commit open in the centre, if any.
    pub open: Option<Result<OpenCommitFile, String>>,
    /// What the last action did, or why it failed.
    pub notice: Option<String>,
    pub list_scroll: UniformListScrollHandle,
    pub diff_scroll: UniformListScrollHandle,
    /// How far the open file's code is moved left; see `code_scroll`. The
    /// blame's code moves with it.
    pub code_scroll_x: gpui::Pixels,
    /// The blame of the file on disk, drawn in the centre while it is Some.
    pub blame: Option<Result<BlameView, String>>,
    pub blame_scroll: UniformListScrollHandle,
    /// The files that differ between the selected commit and a branch or tag,
    /// drawn on the right while it is Some.
    pub compare: Option<Result<CompareView, String>>,
    /// Numbers the list reads; a page read for an older number is dropped.
    list_request: u64,
    /// Stops the list read in flight when a new one replaces it.
    stop: Arc<AtomicBool>,
    /// Numbers the blame reads; a read for an older number is dropped.
    pub(crate) blame_request: u64,
    /// Stops the blame read in flight when the blame is dropped.
    pub(crate) blame_stop: Arc<AtomicBool>,
    /// Numbers the comparisons; one for an older number is dropped.
    pub(crate) compare_request: u64,
    /// Numbers the opened files; a read for an older number is dropped.
    pub(crate) open_request: u64,
    /// The commit to select once a restarted list's first page is read.
    keep: Option<String>,
    /// The commit whose details were asked for last.
    detail_request: Option<String>,
    search_events: Option<Subscription>,
}

impl Default for HistoryState {
    fn default() -> Self {
        Self {
            history: History::new(),
            source: Source::Log { all: false },
            all: false,
            field: Field::Message,
            search: None,
            sessions: Vec::new(),
            loading: false,
            error: None,
            detail: None,
            open: None,
            notice: None,
            list_scroll: UniformListScrollHandle::new(),
            diff_scroll: UniformListScrollHandle::new(),
            code_scroll_x: gpui::Pixels::ZERO,
            blame: None,
            blame_scroll: UniformListScrollHandle::new(),
            compare: None,
            list_request: 0,
            stop: Arc::new(AtomicBool::new(false)),
            blame_request: 0,
            blame_stop: Arc::new(AtomicBool::new(false)),
            compare_request: 0,
            open_request: 0,
            keep: None,
            detail_request: None,
            search_events: None,
        }
    }
}

impl HistoryState {
    /// Drops the blame and stops its read in flight.
    pub(crate) fn cancel_blame(&mut self) {
        self.blame_stop.store(true, Ordering::Relaxed);
        self.blame_stop = Arc::new(AtomicBool::new(false));
        self.blame_request = self.blame_request.wrapping_add(1);
        self.blame = None;
    }

    /// Drops the comparison and any read of one in flight.
    pub(crate) fn cancel_compare(&mut self) {
        self.compare_request = self.compare_request.wrapping_add(1);
        self.compare = None;
    }

    /// Closes the open file, and any read of one in flight.
    pub(crate) fn cancel_open(&mut self) {
        self.open_request = self.open_request.wrapping_add(1);
        self.open = None;
    }
}

impl Shell {
    /// Shows the History screen: makes the search field the first time and
    /// reads the list again, keeping the selected commit when it is still
    /// there.
    pub(crate) fn enter_history(&mut self, cx: &mut Context<Self>) {
        if self.history.search.is_none() {
            let input = cx.new(|cx| TextInput::new("Search commits", cx));
            let events = cx.subscribe(
                &input,
                |this: &mut Self, _, event: &TextInputEvent, cx: &mut Context<Self>| match event {
                    TextInputEvent::Submit => this.run_search(cx),
                    TextInputEvent::Cancel => this.clear_search(cx),
                },
            );
            self.history.search = Some(input);
            self.history.search_events = Some(events);
        }
        self.reload_history(cx);
    }

    /// Reads the list from the top again for the current source, keeping the
    /// selected commit when the new list has it within its first page.
    pub(crate) fn reload_history(&mut self, cx: &mut Context<Self>) {
        let keep = self
            .history
            .history
            .selected_row()
            .map(|row| row.entry.id.clone());
        self.restart_list(keep, cx);
    }

    /// Shows the commits of `source` from the top.
    pub fn show_history_source(&mut self, source: Source, cx: &mut Context<Self>) {
        self.history.source = source;
        self.history.cancel_blame();
        self.restart_list(None, cx);
    }

    /// Searches for the field's text in the chosen field, or shows the log
    /// again when the field is blank.
    pub fn run_search(&mut self, cx: &mut Context<Self>) {
        let text = self
            .history
            .search
            .as_ref()
            .map(|input| input.read(cx).text().trim().to_owned())
            .unwrap_or_default();
        let all = self.history.all;
        let source = if text.is_empty() {
            Source::Log { all }
        } else {
            Source::Search {
                text,
                field: self.history.field,
                all,
            }
        };
        if source != self.history.source {
            self.show_history_source(source, cx);
        }
    }

    /// Empties the search field and shows the log.
    pub fn clear_search(&mut self, cx: &mut Context<Self>) {
        if let Some(input) = self.history.search.clone() {
            input.update(cx, TextInput::clear);
        }
        let all = self.history.all;
        self.show_history_source(Source::Log { all }, cx);
    }

    /// Focuses the search field.
    pub fn focus_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(input) = self.history.search.as_ref() {
            input.read(cx).focus(window);
        }
    }

    /// Chooses what the search matches, and searches again.
    pub fn set_search_field(&mut self, field: Field, cx: &mut Context<Self>) {
        self.history.field = field;
        if matches!(self.history.source, Source::Search { .. }) {
            self.history.source = Source::Log {
                all: self.history.all,
            };
        }
        self.run_search(cx);
        cx.notify();
    }

    /// Shows HEAD's commits only, or every branch's.
    pub fn set_history_all(&mut self, all: bool, cx: &mut Context<Self>) {
        self.history.all = all;
        let source = match self.history.source.clone() {
            Source::Log { .. } => Source::Log { all },
            Source::Search { text, field, .. } => Source::Search { text, field, all },
            file @ Source::File { .. } => file,
        };
        self.show_history_source(source, cx);
    }

    /// Shows the history of `path`, from the review or the Changes screen.
    pub fn show_file_history(&mut self, path: &str, cx: &mut Context<Self>) {
        self.show(rvw_view::keys::Screen::History, cx);
        self.show_history_source(
            Source::File {
                path: path.to_owned(),
            },
            cx,
        );
    }

    /// Selects row `index` and reads its details.
    pub fn select_commit(&mut self, index: usize, cx: &mut Context<Self>) {
        self.history.history.select(index);
        self.history.cancel_open();
        self.history.cancel_compare();
        self.history
            .list_scroll
            .scroll_to_item(index, gpui::ScrollStrategy::Center);
        self.request_detail(cx);
        cx.notify();
    }

    /// Moves the selection by `delta` rows (J and K).
    pub fn step_commit(&mut self, delta: isize, cx: &mut Context<Self>) {
        self.history.history.step(delta);
        if let Some(index) = self.history.history.selected() {
            self.select_commit(index, cx);
        }
    }

    /// Reads the next page when row `last_visible` comes into view near the
    /// end of what is read.
    pub(crate) fn history_row_shown(&mut self, last_visible: usize, cx: &mut Context<Self>) {
        if !self.history.loading && self.history.history.wants_more(last_visible) {
            self.read_page(cx);
        }
    }

    /// The id of the selected commit; None when no commit is listed.
    pub(crate) fn selected_commit_id(&self) -> Option<String> {
        self.history
            .history
            .selected_row()
            .map(|row| row.entry.id.clone())
    }

    /// Opens `path` of the selected commit in the centre.
    pub fn open_commit_file(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(commit) = self.selected_commit_id() else {
            return;
        };
        let dir = self.repo_path().to_path_buf();
        let path = path.to_owned();
        self.spawn_open(move || read_commit_file(&dir, &commit, &path), cx);
    }

    /// Reads a file for the centre off the UI thread and opens it. A read
    /// that a newer open or a selection change overtook is dropped.
    pub(crate) fn spawn_open<F>(&mut self, read: F, cx: &mut Context<Self>)
    where
        F: FnOnce() -> Result<OpenCommitFile, String> + Send + 'static,
    {
        self.history.code_scroll_x = gpui::Pixels::ZERO;
        self.history.open_request = self.history.open_request.wrapping_add(1);
        let request = self.history.open_request;
        cx.spawn(async move |this, cx| {
            let read = cx.background_executor().spawn(async move { read() }).await;
            this.update(cx, |this, cx| {
                if this.history.open_request == request {
                    this.history.open = Some(read);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Closes the open file and the blame, and shows the list again.
    pub fn close_commit_file(&mut self, cx: &mut Context<Self>) {
        self.history.cancel_open();
        self.history.cancel_blame();
        cx.notify();
    }

    /// A sideways wheel or swipe over the open file's diff, or the blame,
    /// moves its code.
    pub(crate) fn scroll_history_code(
        &mut self,
        event: &gpui::ScrollWheelEvent,
        cx: &mut Context<Self>,
    ) {
        let delta = code_scroll::sideways(event);
        if delta == gpui::Pixels::ZERO {
            return;
        }
        let (longest, gutter, handle) = if let Some(Ok(open)) = self.history.open.as_ref() {
            (
                open.longest_line,
                HISTORY_CODE_GUTTER,
                &self.history.diff_scroll,
            )
        } else if let Some(Ok(blame)) = self.history.blame.as_ref() {
            (
                blame.longest_line,
                BLAME_CODE_GUTTER,
                &self.history.blame_scroll,
            )
        } else {
            return;
        };
        let view = handle.0.borrow().base_handle.bounds().size.width;
        let width = code_scroll::width(self.code_advance, longest);
        let max = code_scroll::max_offset(width, view - gutter);
        let new = code_scroll::scrolled(self.history.code_scroll_x, delta, max);
        if new != self.history.code_scroll_x {
            self.history.code_scroll_x = new;
            cx.notify();
        }
    }

    /// Starts the list again from the top, stopping the read in flight, and
    /// selects `keep` once it is read.
    fn restart_list(&mut self, keep: Option<String>, cx: &mut Context<Self>) {
        self.history.stop.store(true, Ordering::Relaxed);
        self.history.stop = Arc::new(AtomicBool::new(false));
        self.history.list_request = self.history.list_request.wrapping_add(1);
        self.history.history = History::new();
        self.history.loading = false;
        self.history.error = None;
        self.history.detail = None;
        self.history.keep = keep;
        self.history.sessions =
            data::sessions(&self.service, &self.identity.workspace).unwrap_or_default();
        self.history.cancel_open();
        self.read_page(cx);
        cx.notify();
    }

    /// Reads the next page of the list off the UI thread.
    fn read_page(&mut self, cx: &mut Context<Self>) {
        self.history.loading = true;
        let request = self.history.list_request;
        let stop = Arc::clone(&self.history.stop);
        let dir = self.repo_path().to_path_buf();
        let source = self.history.source.clone();
        let skip = self.history.history.next_skip();
        cx.spawn(async move |this, cx| {
            let page = cx
                .background_executor()
                .spawn(async move { read_page(&dir, &source, skip, &stop) })
                .await;
            this.update(cx, |this, cx| this.apply_page(request, page, cx))
                .ok();
        })
        .detach();
    }

    /// Shows a page read for `request`, when it is still the list asked for.
    fn apply_page(
        &mut self,
        request: u64,
        page: Result<Vec<LogEntry>, String>,
        cx: &mut Context<Self>,
    ) {
        if request != self.history.list_request {
            return;
        }
        self.history.loading = false;
        let first_page = self.history.history.is_empty();
        match page {
            Ok(entries) => self.history.history.append(entries, PAGE),
            Err(message) => self.history.error = Some(message),
        }
        if first_page {
            // The commit selected before the reload, when this page has it;
            // else the first row, which `append` selected.
            if let Some(keep) = self.history.keep.take() {
                self.history.history.select_id(&keep);
            }
            if let Some(index) = self.history.history.selected() {
                self.history
                    .list_scroll
                    .scroll_to_item(index, gpui::ScrollStrategy::Center);
            }
            self.request_detail(cx);
        }
        cx.notify();
    }

    /// Reads the selected commit's details off the UI thread.
    fn request_detail(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self
            .history
            .history
            .selected_row()
            .map(|row| row.entry.id.clone())
        else {
            self.history.detail = None;
            return;
        };
        if self
            .history
            .detail
            .as_ref()
            .is_some_and(|d| d.as_ref().is_ok_and(|d| d.entry.id == id))
        {
            return;
        }
        self.history.detail_request = Some(id.clone());
        let dir = self.repo_path().to_path_buf();
        cx.spawn(async move |this, cx| {
            let wanted = id.clone();
            let read = cx
                .background_executor()
                .spawn(async move { history::show(&dir, &id).map_err(|e| last_line(&e.stderr)) })
                .await;
            this.update(cx, |this, cx| {
                if this.history.detail_request.as_deref() == Some(wanted.as_str()) {
                    this.history.detail_request = None;
                    this.history.detail = Some(read);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }
}

/// One page of `source`, starting `skip` commits from the top.
fn read_page(
    dir: &Path,
    source: &Source,
    skip: usize,
    stop: &AtomicBool,
) -> Result<Vec<LogEntry>, String> {
    let read: Result<Vec<LogEntry>, GitError> = match source {
        Source::Log { all } => history::log(
            dir,
            &LogQuery {
                revs: Vec::new(),
                all: *all,
                skip,
                limit: PAGE,
            },
            stop,
        ),
        Source::Search { text, field, all } => search::commits(
            dir,
            &search::Query {
                text: text.clone(),
                field: *field,
                all: *all,
                skip,
                limit: PAGE,
            },
            stop,
        ),
        Source::File { path } => blame::file_history(dir, path, skip, PAGE, stop)
            .map(|revisions| revisions.into_iter().map(|r| r.entry).collect()),
    };
    read.map_err(|e| last_line(&e.stderr))
}

/// The diff of `path` in `commit`, parsed into rows.
fn read_commit_file(dir: &Path, commit: &str, path: &str) -> Result<OpenCommitFile, String> {
    let raw = history::commit_diff(dir, commit, path, CONTEXT).map_err(|e| last_line(&e.stderr))?;
    Ok(open_file(commit, path, parse(&raw)))
}

/// The file `path` of `commit`, shown from its parsed diff.
pub(crate) fn open_file(commit: &str, path: &str, diff: FileDiff) -> OpenCommitFile {
    let rows = diff
        .hunks
        .iter()
        .enumerate()
        .flat_map(|(hunk, h)| {
            std::iter::once(DiffRow::Header(hunk))
                .chain((0..h.lines.len()).map(move |line| DiffRow::Line(Pos { hunk, line })))
        })
        .collect();
    OpenCommitFile {
        commit: commit.to_owned(),
        path: path.to_owned(),
        longest_line: code_scroll::longest_line(&diff),
        diff: Arc::new(diff),
        rows,
    }
}
