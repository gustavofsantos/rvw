//! The Changes screen's state and actions: the staged and unstaged lists, the
//! open file's diff and the lines chosen in it, and the commit message. Every
//! action that changes the repository first saves a checkpoint, and does not
//! run when that fails. Each action is one `rvw_git` call; the screen's drawing
//! is in `changes_screen`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{
    actions, App, AppContext, Context, Entity, KeyBinding, ScrollStrategy, Subscription,
    UniformListScrollHandle,
};
use rvw_core::types::Comment;
use rvw_git::{commits, index, GitError};
use rvw_view::commit_screen::{self, LineChoice, Lists, Side};
use rvw_view::diff::{parse, FileDiff};
use rvw_view::keys::Screen;
use rvw_view::patch::{hunk_patch, lines_patch, Direction};
use rvw_view::selection::Pos;

use crate::code_scroll;
use crate::shell::Shell;
use crate::text_area::{TextArea, TextAreaEvent};

actions!(rvw_changes, [CommitChanges, RestoreLatest]);

/// The width before a Changes line's code: two numbers, the sign, padding.
const CHANGES_CODE_GUTTER: gpui::Pixels = gpui::px(114.);

/// ⇧⌘C commits from any screen with something staged; ⌘Z opens the restore of
/// the newest checkpoint when no text field takes it first.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("secondary-shift-c", CommitChanges, None),
        KeyBinding::new(
            "secondary-z",
            RestoreLatest,
            crate::terminal_dock::rvw_chord_context(),
        ),
    ]);
}

/// One row of the left panel's list: a list's heading, or a file of it by
/// index into `Lists::side`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListRow {
    Heading(Side),
    File(Side, usize),
}

/// One row of the open file's diff: a hunk's header, or a line of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffRow {
    Header(usize),
    Line(Pos),
}

/// The file open in the centre.
pub struct OpenChange {
    pub side: Side,
    pub path: String,
    /// The diff as git printed it; patches are built from this text.
    pub raw: String,
    pub diff: Arc<FileDiff>,
    /// The file moves only as a whole (new, deleted, renamed, binary...).
    pub whole_file_only: bool,
    pub rows: Vec<DiffRow>,
    /// The columns of the longest line; see `code_scroll`.
    pub longest_line: usize,
}

#[derive(Default)]
pub struct ChangesState {
    pub lists: Lists,
    /// The rows of the left panel, rebuilt with `lists`.
    pub list_rows: Vec<ListRow>,
    /// Why the lists could not be read.
    pub load_error: Option<String>,
    pub open: Option<OpenChange>,
    pub choice: LineChoice,
    /// The commit message, made the first time the screen is shown.
    pub message: Option<Entity<TextArea>>,
    /// The next commit replaces the last one.
    pub amend: bool,
    /// HEAD is on a remote branch, so amending is not offered.
    pub head_pushed: bool,
    /// HEAD exists, so there is a commit to amend.
    pub has_head: bool,
    /// The last action's failure, shown above the commit button.
    pub error: Option<String>,
    /// What the last restore did, e.g. "Restored “Before staging x”.".
    pub notice: Option<String>,
    pub list_scroll: UniformListScrollHandle,
    /// How far the open file's code is moved left; see `code_scroll`.
    pub code_scroll_x: gpui::Pixels,
    pub diff_scroll: UniformListScrollHandle,
    pub(crate) message_events: Option<Subscription>,
}

const NO_MESSAGE: &str = "Write a commit message first";
const NOTHING_STAGED: &str = "Stage something to commit";
const PUSHED: &str = "The last commit is pushed; amending it would need a force push";
const NO_COMMIT: &str = "There is no commit to amend yet";
const WHOLE_FILE: &str = "This file can only be staged as a whole";
const CHANGED: &str = "The file changed since it was shown; look again";
const NO_REASON: &str = "git failed without saying why";

impl Shell {
    /// Shows the Changes screen: makes the message field the first time and
    /// re-reads the lists.
    pub(crate) fn enter_changes(&mut self, cx: &mut Context<Self>) {
        if self.changes.message.is_none() {
            let message = cx.new(|cx| TextArea::new("Commit message", cx).with_lines(3, 10));
            let events = cx.subscribe(
                &message,
                |this: &mut Self, _, event: &TextAreaEvent, cx: &mut Context<Self>| match event {
                    TextAreaEvent::Submit => this.commit_changes(cx),
                    TextAreaEvent::Cancel => {}
                },
            );
            self.changes.message = Some(message);
            self.changes.message_events = Some(events);
        }
        self.reload_changes();
    }

    /// Re-reads the lists, keeping the open file (or its neighbour) and, when
    /// its diff did not change, the chosen lines.
    pub(crate) fn reload_changes(&mut self) {
        let read = read_changes(
            &self.repo_dir(),
            &self.snapshot.comments,
            self.open_change_place().as_ref(),
        );
        self.apply_changes(read);
    }

    /// The open file's side, path and index in its list, which a read of the
    /// lists reselects from.
    pub(crate) fn open_change_place(&self) -> Option<(Side, String, usize)> {
        let open = self.changes.open.as_ref()?;
        let index = self
            .changes
            .lists
            .find(open.side, &open.path)
            .map_or(0, |(index, _)| index);
        Some((open.side, open.path.clone(), index))
    }

    /// Shows what `read_changes` read, keeping the chosen lines when the open
    /// file's diff did not change.
    pub(crate) fn apply_changes(&mut self, read: ChangesRead) {
        self.live_epoch = self.live_epoch.wrapping_add(1);
        let previous = self.changes.open.take();
        let lists = match read.lists {
            Ok(lists) => lists,
            Err(message) => {
                self.changes.load_error = Some(message);
                self.changes.lists = Lists::default();
                self.changes.list_rows = list_rows(&self.changes.lists);
                self.changes.choice.clear();
                return;
            }
        };
        self.changes.load_error = None;
        self.changes.list_rows = list_rows(&lists);
        self.changes.lists = lists;
        self.changes.has_head = read.has_head;
        self.changes.head_pushed = read.head_pushed;
        match read.open {
            None => self.changes.choice.clear(),
            Some(Ok(open)) => {
                let same_file = previous
                    .as_ref()
                    .is_some_and(|old| old.side == open.side && old.path == open.path);
                if !same_file {
                    self.changes.code_scroll_x = gpui::Pixels::ZERO;
                }
                let same = same_file && previous.as_ref().is_some_and(|old| old.raw == open.raw);
                if !same {
                    self.changes.choice.clear();
                }
                self.changes.open = Some(open);
            }
            Some(Err(message)) => {
                self.changes.error = Some(message);
                self.changes.choice.clear();
            }
        }
    }

    /// Opens `path` from the list on `side`.
    pub fn open_change(&mut self, side: Side, path: &str, cx: &mut Context<Self>) {
        let dir = self.repo_dir();
        self.changes.choice.clear();
        self.changes.notice = None;
        self.changes.error = None;
        self.changes.code_scroll_x = gpui::Pixels::ZERO;
        match load_change(&dir, &self.changes.lists, side, path) {
            Ok(open) => self.changes.open = Some(open),
            Err(message) => {
                self.changes.open = None;
                self.changes.error = Some(message);
            }
        }
        cx.notify();
    }

    /// A sideways wheel or swipe over the open file's diff moves its code,
    /// within its longest line.
    pub(crate) fn scroll_change_code(
        &mut self,
        event: &gpui::ScrollWheelEvent,
        cx: &mut Context<Self>,
    ) {
        let delta = code_scroll::sideways(event);
        let Some(open) = self.changes.open.as_ref() else {
            return;
        };
        if delta == gpui::Pixels::ZERO {
            return;
        }
        let view = self
            .changes
            .diff_scroll
            .0
            .borrow()
            .base_handle
            .bounds()
            .size
            .width;
        let width = code_scroll::width(self.code_advance, open.longest_line);
        let max = code_scroll::max_offset(width, view - CHANGES_CODE_GUTTER);
        let new = code_scroll::scrolled(self.changes.code_scroll_x, delta, max);
        if new != self.changes.code_scroll_x {
            self.changes.code_scroll_x = new;
            cx.notify();
        }
    }

    /// Click chooses the line alone; Shift-click extends from the anchor.
    pub fn choose_change_line(&mut self, pos: Pos, extend: bool, cx: &mut Context<Self>) {
        if let Some(open) = self.changes.open.as_ref() {
            if extend {
                self.changes.choice.extend(&open.diff, pos);
            } else {
                self.changes.choice.click(&open.diff, pos);
            }
        }
        cx.notify();
    }

    /// Stages a whole file from the unstaged list, or unstages one from the
    /// staged list.
    pub fn move_file(&mut self, side: Side, path: &str, cx: &mut Context<Self>) {
        let name = file_name(path);
        match side {
            Side::Unstaged => {
                self.change_index(&format!("Before staging {name}"), cx, |dir| {
                    index::stage(dir, &[path])
                });
            }
            Side::Staged => {
                let orig = self
                    .changes
                    .lists
                    .find(side, path)
                    .and_then(|(_, entry)| entry.orig_path.as_deref());
                let paths = move_paths_of(side, path, orig);
                let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
                self.change_index(&format!("Before unstaging {name}"), cx, |dir| {
                    index::unstage(dir, &refs)
                });
            }
        }
    }

    /// "Stage all" under the unstaged heading, "Unstage all" under the staged.
    pub fn move_all(&mut self, side: Side, cx: &mut Context<Self>) {
        let count = self.changes.lists.side(side).len();
        if count == 0 {
            return;
        }
        let paths: Vec<String> = self
            .changes
            .lists
            .side(side)
            .iter()
            .flat_map(|entry| move_paths_of(side, &entry.path, entry.orig_path.as_deref()))
            .collect();
        let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
        let label = format!("Before {} {}", staging_verb(side), count_of(count, "file"));
        match side {
            Side::Unstaged => self.change_index(&label, cx, |dir| index::stage(dir, &refs)),
            Side::Staged => self.change_index(&label, cx, |dir| index::unstage(dir, &refs)),
        }
    }

    /// Stages (or unstages, on the staged side) hunk `hunk` of the open file.
    pub fn move_hunk(&mut self, hunk: usize, cx: &mut Context<Self>) {
        let Some((side, path, whole_file_only)) = self.open_facts() else {
            return;
        };
        if whole_file_only {
            self.fail(WHOLE_FILE, cx);
            return;
        }
        let Some(raw) = self.checked_raw() else {
            cx.notify();
            return;
        };
        let patch_text = match hunk_patch(&raw, hunk, direction_of(side)) {
            Ok(patch_text) => patch_text,
            Err(e) => {
                self.fail(sentence(&e.to_string()), cx);
                return;
            }
        };
        let label = format!(
            "Before {} a hunk of {}",
            staging_verb(side),
            file_name(&path)
        );
        let reverse = side == Side::Staged;
        self.change_index(&label, cx, |dir| {
            index::apply_to_index(dir, &patch_text, reverse)
        });
    }

    /// S: the chosen lines, or the whole open file when none is chosen.
    pub fn move_chosen(&mut self, cx: &mut Context<Self>) {
        let Some((side, path, whole_file_only)) = self.open_facts() else {
            return;
        };
        if self.changes.choice.is_empty() {
            self.move_file(side, &path, cx);
            return;
        }
        if whole_file_only {
            self.fail(WHOLE_FILE, cx);
            return;
        }
        let Some(raw) = self.checked_raw() else {
            cx.notify();
            return;
        };
        let Some(chosen) = self
            .changes
            .open
            .as_ref()
            .map(|open| self.changes.choice.chosen(&open.diff))
        else {
            return;
        };
        let count = chosen.len();
        let patch_text = match lines_patch(&raw, &chosen, direction_of(side)) {
            Ok(patch_text) => patch_text,
            Err(e) => {
                self.fail(sentence(&e.to_string()), cx);
                return;
            }
        };
        let label = format!(
            "Before {} {} of {}",
            staging_verb(side),
            count_of(count, "line"),
            file_name(&path)
        );
        let reverse = side == Side::Staged;
        self.change_index(&label, cx, |dir| {
            index::apply_to_index(dir, &patch_text, reverse)
        });
    }

    /// Discards the unstaged changes of `path`; ⌘Z puts them back.
    pub fn discard_file(&mut self, path: &str, cx: &mut Context<Self>) {
        if self.changes.lists.find(Side::Unstaged, path).is_none() {
            return;
        }
        let label = format!("Before discarding changes to {}", file_name(path));
        if !self.checkpoint_first(&label, cx) {
            return;
        }
        let dir = self.repo_dir();
        match commits::discard(&dir, path) {
            Ok(()) => {
                self.changes.error = None;
                self.changes.notice = None;
                self.reload_changes();
            }
            Err(e) => self.changes.error = Some(one_line(&e.stderr)),
        }
        cx.notify();
    }

    /// ⇧⌘C and the commit button: commits (or amends) the staged changes
    /// with the typed message.
    pub fn commit_changes(&mut self, cx: &mut Context<Self>) {
        if self.other_field_open() {
            return;
        }
        if self.screen != Screen::Changes {
            self.show(Screen::Changes, cx);
            return;
        }
        let Some(field) = self.changes.message.clone() else {
            return;
        };
        let message = field.read(cx).text().trim_end().to_owned();
        if let Some(reason) = self.commit_problem(&message) {
            self.fail(reason, cx);
            return;
        }
        self.write_commit(&field, &message, cx);
        cx.notify();
    }

    /// Turns amend on or off. Turning it on with an empty message fills in
    /// the last commit's message.
    pub fn toggle_amend(&mut self, cx: &mut Context<Self>) {
        let dir = self.repo_dir();
        let field = self.changes.message.clone();
        if self.changes.amend {
            self.changes.amend = false;
            if let (Some(field), Ok(last)) = (field, commits::head_message(&dir)) {
                if field.read(cx).text() == last {
                    field.update(cx, TextArea::clear);
                }
            }
        } else {
            if let Some(reason) = amend_problem(&self.changes) {
                self.fail(reason, cx);
                return;
            }
            if let Some(field) = field {
                if field.read(cx).text().trim().is_empty() {
                    match commits::head_message(&dir) {
                        Ok(last) => field.update(cx, |area, cx| area.set_text(&last, cx)),
                        Err(e) => {
                            self.fail(one_line(&e.stderr), cx);
                            return;
                        }
                    }
                }
            }
            self.changes.amend = true;
            self.changes.error = None;
        }
        cx.notify();
    }

    /// J and K on the Changes screen.
    pub fn step_change(&mut self, delta: isize, cx: &mut Context<Self>) {
        let current = self
            .changes
            .open
            .as_ref()
            .map(|open| (open.side, open.path.clone()));
        let target = commit_screen::step(
            &self.changes.lists,
            current.as_ref().map(|(s, p)| (*s, p.as_str())),
            delta,
        );
        let Some((side, path)) = target else {
            return;
        };
        let same = current.is_some_and(|(s, p)| s == side && p == path);
        if !same {
            self.open_change(side, &path, cx);
        }
        if let Some((index, _)) = self.changes.lists.find(side, &path) {
            let row = self
                .changes
                .list_rows
                .iter()
                .position(|row| *row == ListRow::File(side, index));
            if let Some(row) = row {
                self.changes
                    .list_scroll
                    .scroll_to_item(row, ScrollStrategy::Center);
            }
        }
    }
}

impl Shell {
    /// The repository root, as the git calls take it.
    fn repo_dir(&self) -> PathBuf {
        Path::new(&self.identity.workspace).to_path_buf()
    }

    /// Whether a dialog or a field elsewhere holds the keyboard, so that the
    /// Changes keys must not act.
    fn other_field_open(&self) -> bool {
        self.composer.is_some()
            || self.submit.is_some()
            || self.finder.is_some()
            || self.tools.field_open()
    }

    /// Shows `message` as the action's failure.
    fn fail(&mut self, message: impl Into<String>, cx: &mut Context<Self>) {
        self.changes.error = Some(message.into());
        cx.notify();
    }

    /// The open file's side, path and whole-file flag, when a file is open.
    fn open_facts(&self) -> Option<(Side, String, bool)> {
        self.changes
            .open
            .as_ref()
            .map(|open| (open.side, open.path.clone(), open.whole_file_only))
    }

    /// The open file's diff, read again. `Some(raw)` when it is the diff that
    /// is shown. Otherwise sets the error, re-reads the lists and gives `None`.
    fn checked_raw(&mut self) -> Option<String> {
        let dir = self.repo_dir();
        let (side, path, shown) = self
            .changes
            .open
            .as_ref()
            .map(|open| (open.side, open.path.clone(), open.raw.clone()))?;
        match read_raw(&dir, side, &path) {
            Ok(raw) if raw == shown => Some(raw),
            Ok(_) => {
                self.changes.error = Some(CHANGED.to_owned());
                self.reload_changes();
                None
            }
            Err(e) => {
                self.changes.error = Some(one_line(&e.stderr));
                self.reload_changes();
                None
            }
        }
    }

    /// Saves the checkpoint that an action takes first. False, with the error
    /// shown, when it cannot be saved: the action must then not run.
    fn checkpoint_first(&mut self, label: &str, cx: &mut Context<Self>) -> bool {
        match self.checkpoint_before(label, &[], cx) {
            Ok(()) => true,
            Err(message) => {
                self.changes.error = Some(message);
                cx.notify();
                false
            }
        }
    }

    /// Runs one index change after its checkpoint, labelled `label`.
    fn change_index(
        &mut self,
        label: &str,
        cx: &mut Context<Self>,
        act: impl FnOnce(&Path) -> Result<(), GitError>,
    ) {
        if !self.checkpoint_first(label, cx) {
            return;
        }
        let dir = self.repo_dir();
        match act(&dir) {
            Ok(()) => {
                self.changes.error = None;
                self.changes.notice = None;
                self.reload_changes();
            }
            Err(e) => self.changes.error = Some(one_line(&e.stderr)),
        }
        cx.notify();
    }

    /// Why the commit cannot be made now, as the message the user sees.
    fn commit_problem(&self, message: &str) -> Option<&'static str> {
        if message.is_empty() {
            Some(NO_MESSAGE)
        } else if !self.changes.amend && self.changes.lists.staged.is_empty() {
            Some(NOTHING_STAGED)
        } else if self.changes.amend && self.changes.head_pushed {
            Some(PUSHED)
        } else {
            None
        }
    }

    /// Checkpoints, makes the commit or amend, and re-reads the review and the
    /// repository facts (ahead and behind, checkpoints).
    fn write_commit(&mut self, field: &Entity<TextArea>, message: &str, cx: &mut Context<Self>) {
        let amend = self.changes.amend;
        let label = if amend {
            "Before amending the last commit"
        } else {
            "Before committing"
        };
        if !self.checkpoint_first(label, cx) {
            return;
        }
        let dir = self.repo_dir();
        let result = if amend {
            commits::amend(&dir, message)
        } else {
            commits::commit(&dir, message)
        };
        match result {
            Ok(_) => {
                field.update(cx, TextArea::clear);
                self.changes.amend = false;
                self.changes.error = None;
                self.changes.notice = None;
                self.reload_changes();
                self.refresh_facts();
                let _ = self.refresh();
            }
            Err(e) => self.changes.error = Some(one_line(&e.stderr)),
        }
    }
}

/// Why amend cannot be turned on, if it cannot.
fn amend_problem(state: &ChangesState) -> Option<&'static str> {
    if !state.has_head {
        Some(NO_COMMIT)
    } else if state.head_pushed {
        Some(PUSHED)
    } else {
        None
    }
}

/// The paths git moves for `path` on `side`: the path, and on the staged side
/// the path a staged rename came from, so the rename moves whole.
fn move_paths_of(side: Side, path: &str, orig: Option<&str>) -> Vec<String> {
    let mut paths = vec![path.to_owned()];
    if side == Side::Staged {
        paths.extend(orig.map(str::to_owned));
    }
    paths
}

/// The left panel's rows: the staged heading and its files, then the unstaged.
fn list_rows(lists: &Lists) -> Vec<ListRow> {
    let mut rows = vec![ListRow::Heading(Side::Staged)];
    rows.extend((0..lists.staged.len()).map(|i| ListRow::File(Side::Staged, i)));
    rows.push(ListRow::Heading(Side::Unstaged));
    rows.extend((0..lists.unstaged.len()).map(|i| ListRow::File(Side::Unstaged, i)));
    rows
}

/// The centre's rows for a diff: each hunk's header, then its lines.
fn diff_rows(diff: &FileDiff) -> Vec<DiffRow> {
    diff.hunks
        .iter()
        .enumerate()
        .flat_map(|(hunk, h)| {
            std::iter::once(DiffRow::Header(hunk))
                .chain((0..h.lines.len()).map(move |line| DiffRow::Line(Pos { hunk, line })))
        })
        .collect()
}

/// The open file for `raw`, its diff parsed and its rows built.
/// `listed_whole` is the list's flag for the file; binary diffs are whole too.
fn open_from(side: Side, path: &str, raw: String, listed_whole: bool) -> OpenChange {
    let diff = parse(&raw);
    OpenChange {
        side,
        path: path.to_owned(),
        whole_file_only: listed_whole || diff.binary,
        rows: diff_rows(&diff),
        longest_line: code_scroll::longest_line(&diff),
        diff: Arc::new(diff),
        raw,
    }
}

/// The Changes screen as git shows it now, read off the UI thread when the
/// window refreshes live; see `Shell::apply_changes`.
pub(crate) struct ChangesRead {
    /// The two lists, or why git could not list them.
    pub(crate) lists: Result<Lists, String>,
    has_head: bool,
    head_pushed: bool,
    /// The file to open: the one open before (or its neighbour when it left
    /// its list), else the first. `None` when both lists are empty.
    pub(crate) open: Option<Result<OpenChange, String>>,
}

/// Reads the lists, with `comments` counted per file, and the file to open.
/// `previous` is `Shell::open_change_place` of the file open before.
pub(crate) fn read_changes(
    dir: &Path,
    comments: &[Comment],
    previous: Option<&(Side, String, usize)>,
) -> ChangesRead {
    let lists = match index::status(dir) {
        Ok(status) => commit_screen::lists(&status, comments),
        Err(e) => {
            return ChangesRead {
                lists: Err(one_line(&e.stderr)),
                has_head: false,
                head_pushed: false,
                open: None,
            }
        }
    };
    let target = match previous {
        Some((side, path, index)) => commit_screen::reselect(&lists, *side, path, *index),
        None => commit_screen::step(&lists, None, 1),
    };
    let open = target.map(|(side, path)| load_change(dir, &lists, side, &path));
    ChangesRead {
        has_head: commits::head(dir).is_some(),
        head_pushed: commits::head_pushed(dir).unwrap_or(false),
        lists: Ok(lists),
        open,
    }
}

/// Reads the diff of `path` on `side` from git and opens it.
fn load_change(dir: &Path, lists: &Lists, side: Side, path: &str) -> Result<OpenChange, String> {
    let raw = read_raw(dir, side, path).map_err(|e| one_line(&e.stderr))?;
    let listed_whole = lists
        .find(side, path)
        .is_some_and(|(_, entry)| entry.whole_file_only);
    Ok(open_from(side, path, raw, listed_whole))
}

/// The diff git shows for `path` on `side`: the unstaged or the staged one.
fn read_raw(dir: &Path, side: Side, path: &str) -> Result<String, GitError> {
    match side {
        Side::Unstaged => index::unstaged_diff(dir, path),
        Side::Staged => index::staged_diff(dir, path),
    }
}

/// Which way a change moves when it leaves `side`'s list.
fn direction_of(side: Side) -> Direction {
    match side {
        Side::Unstaged => Direction::Stage,
        Side::Staged => Direction::Unstage,
    }
}

/// The verb of moving a change off `side`, for a checkpoint's label: "staging"
/// takes it off the unstaged list, "unstaging" off the staged one.
fn staging_verb(side: Side) -> &'static str {
    match side {
        Side::Unstaged => "staging",
        Side::Staged => "unstaging",
    }
}

/// The last non-empty line of git's error text, which is the reason. A
/// fixed text when git printed nothing.
fn one_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .rev()
        .find(|line| !line.is_empty())
        .map_or_else(|| NO_REASON.to_owned(), str::to_owned)
}

/// `text` with its first letter capitalised, for a sentence shown to the user.
fn sentence(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// "1 file", "2 files", "1 line", "3 lines".
fn count_of(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// The last component of a repository path.
fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rvw_view::commit_screen::Entry;

    /// Two hunks: hunk 0 has four lines, hunk 1 has three.
    const DIFF: &str = concat!(
        "diff --git a/f.rs b/f.rs\n",
        "--- a/f.rs\n",
        "+++ b/f.rs\n",
        "@@ -1,3 +1,3 @@\n",
        " one\n",
        "-two\n",
        "+TWO\n",
        " three\n",
        "@@ -10,2 +10,3 @@\n",
        " ten\n",
        "+eleven\n",
        " twelve\n",
    );

    fn entry(path: &str) -> Entry {
        Entry {
            path: path.to_owned(),
            orig_path: None,
            letter: 'M',
            open_comments: 0,
            whole_file_only: false,
        }
    }

    #[test]
    fn one_line_keeps_the_last_non_empty_line() {
        assert_eq!(
            one_line("fatal: first\n\n  fatal: last  \n\n"),
            "fatal: last"
        );
        assert_eq!(one_line("single"), "single");
    }

    #[test]
    fn one_line_has_a_fixed_text_for_silence() {
        assert_eq!(one_line(""), NO_REASON);
        assert_eq!(one_line("  \n \n"), NO_REASON);
    }

    #[test]
    fn counts_take_the_singular_for_one() {
        assert_eq!(count_of(1, "file"), "1 file");
        assert_eq!(count_of(2, "file"), "2 files");
        assert_eq!(count_of(0, "file"), "0 files");
        assert_eq!(count_of(1, "line"), "1 line");
        assert_eq!(count_of(3, "line"), "3 lines");
    }

    #[test]
    fn a_patch_error_reads_as_a_sentence() {
        assert_eq!(
            sentence("this file can only be staged as a whole"),
            WHOLE_FILE
        );
        assert_eq!(sentence(""), "");
    }

    #[test]
    fn file_name_is_the_last_component() {
        assert_eq!(file_name("src/lib/service.rs"), "service.rs");
        assert_eq!(file_name("README.md"), "README.md");
    }

    #[test]
    fn list_rows_always_have_both_headings() {
        assert_eq!(
            list_rows(&Lists::default()),
            vec![
                ListRow::Heading(Side::Staged),
                ListRow::Heading(Side::Unstaged)
            ]
        );
    }

    #[test]
    fn list_rows_put_each_file_under_its_heading() {
        let lists = Lists {
            staged: vec![entry("a.rs"), entry("b.rs")],
            unstaged: vec![entry("c.rs")],
        };
        assert_eq!(
            list_rows(&lists),
            vec![
                ListRow::Heading(Side::Staged),
                ListRow::File(Side::Staged, 0),
                ListRow::File(Side::Staged, 1),
                ListRow::Heading(Side::Unstaged),
                ListRow::File(Side::Unstaged, 0),
            ]
        );
    }

    #[test]
    fn diff_rows_are_each_header_then_its_lines() {
        let open = open_from(Side::Unstaged, "f.rs", DIFF.to_owned(), false);
        assert_eq!(open.rows.len(), 2 + 4 + 3);
        assert_eq!(open.rows[0], DiffRow::Header(0));
        assert_eq!(open.rows[1], DiffRow::Line(Pos { hunk: 0, line: 0 }));
        assert_eq!(open.rows[4], DiffRow::Line(Pos { hunk: 0, line: 3 }));
        assert_eq!(open.rows[5], DiffRow::Header(1));
        assert_eq!(open.rows[8], DiffRow::Line(Pos { hunk: 1, line: 2 }));
    }

    #[test]
    fn an_open_file_moves_as_a_whole_when_the_list_says_so() {
        assert!(!open_from(Side::Staged, "f.rs", DIFF.to_owned(), false).whole_file_only);
        assert!(open_from(Side::Staged, "f.rs", DIFF.to_owned(), true).whole_file_only);
    }

    #[test]
    fn an_empty_diff_has_no_rows() {
        let open = open_from(Side::Unstaged, "f.rs", String::new(), false);
        assert!(open.rows.is_empty());
    }

    #[test]
    fn move_paths_for_a_staged_rename_include_the_old_path() {
        let paths = move_paths_of(Side::Staged, "new.rs", Some("old.rs"));
        assert_eq!(paths, vec!["new.rs".to_owned(), "old.rs".to_owned()]);
        let unstaged = move_paths_of(Side::Unstaged, "new.rs", Some("old.rs"));
        assert_eq!(unstaged, vec!["new.rs".to_owned()]);
    }
}
