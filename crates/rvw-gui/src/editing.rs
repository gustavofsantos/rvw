//! Editing the open file in its whole-file view. Typing, Enter, Tab, the
//! motions and the undo of a file's `FileBuf` are keys here; the buffer holds
//! the text, and the code area's caret (`code_caret`) mirrors its cursor and
//! selection so that a press, a drag and a key always agree. The rows are
//! rebuilt from the buffer after each change, with the lines the change
//! touched still marked as they were when the file opened. Nothing is written
//! to disk here; saving is a later step.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use gpui::prelude::*;
use gpui::{App, ClipboardItem, Context, Div, KeyBinding, Modifiers, Window, actions, div, px};
use rvw_view::context::{self, Shown};
use rvw_view::diff::{FileDiff, Hunk, Line, LineKind};
use rvw_view::filebuf::{DiskState, FileBuf};
use rvw_view::filesave;
use rvw_view::keys::Screen;
use rvw_view::selection::Pos;
use rvw_view::textpos::TextPos;
use rvw_view::viewmode::ViewMode;

use crate::changes_state::RestoreLatest;
use crate::disk_card::{self, DiskCard, Need, SaveFailure};
use crate::file_view;
use crate::highlights::Highlighter;
use crate::review_screen::{Editing, Selected};
use crate::shell::Shell;
use crate::theme;

actions!(rvw_edit, [EditCut, EditPaste, EditRedo, SaveFile]);

/// The message when a comment is asked for on a file with unsaved edits.
pub const SAVE_FIRST: &str =
    "Save the file first (\u{2318}S): comments are written against the saved file.";

/// Binds ⌘X, ⌘V and ⇧⌘Z to the file being edited. Undo (⌘Z) is the shell's
/// own action, which undoes the edit when there is one. The text fields and
/// the terminal keep their keys.
pub fn bind_keys(cx: &mut App) {
    let context = Some("Rvw && !Terminal && !RvwTextInput && !RvwTextArea");
    cx.bind_keys([
        KeyBinding::new("secondary-x", EditCut, context),
        KeyBinding::new("secondary-v", EditPaste, context),
        KeyBinding::new("secondary-shift-z", EditRedo, context),
        KeyBinding::new("secondary-s", SaveFile, context),
    ]);
}

/// A file being edited: its buffer, the line numbers the change marked when
/// the file opened, and whether the buffer was dirty when last shown.
pub struct Edit {
    pub buf: FileBuf,
    pub marks: BTreeSet<u32>,
    dirty: bool,
    /// The blob id the buffer saves against, when it is not the buffer's own:
    /// set by Keep mine to the disk's blob, cleared by a save.
    pub base: Option<String>,
    /// The reviewer closed the card for a file that is gone.
    pub gone_acked: bool,
    /// Whether the rows show the empty line after a final line break.
    tail: bool,
}

impl Edit {
    /// The blob id the buffer's save is made against.
    fn base(&self) -> &str {
        self.base
            .as_deref()
            .unwrap_or_else(|| self.buf.loaded_blob())
    }
}

/// The message that names unsaved files, for the quit question.
/// `None` when neither the terminals nor the files need a question.
pub fn quit_question(terminals: Option<String>, unsaved: &[String]) -> Option<String> {
    if unsaved.is_empty() {
        return terminals;
    }
    let names = unsaved
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    let edits = format!("Unsaved edits in {names} will be discarded.");
    Some(match terminals {
        Some(running) => format!("{running} {edits}"),
        None => edits,
    })
}

/// A small dot for a file with unsaved edits.
pub fn dirty_dot() -> Div {
    div()
        .w(px(6.))
        .h(px(6.))
        .flex_none()
        .rounded_full()
        .bg(theme::ink())
}

/// The diff row at `flat`, counted over every line of every hunk, as the
/// code caret counts them.
pub fn line_pos(diff: &FileDiff, flat: usize) -> Option<Pos> {
    let mut rest = flat;
    for (hunk, shown) in diff.hunks.iter().enumerate() {
        if rest < shown.lines.len() {
            return Some(Pos { hunk, line: rest });
        }
        rest -= shown.lines.len();
    }
    None
}

/// The file line (0-based) an Edit lands on for the diff row `at`: the row's
/// new-file line, or the nearest line above it that has one. A removed line
/// with none above it, and no row at all, land on the first line.
pub fn edit_line(diff: &FileDiff, at: Option<Pos>) -> usize {
    let Some(at) = at else {
        return 0;
    };
    let Some(shown) = diff.hunks.get(at.hunk) else {
        return 0;
    };
    shown
        .lines
        .iter()
        .take(at.line.saturating_add(1))
        .rev()
        .find_map(|line| line.new_no)
        .map_or(0, |no| (no as usize).saturating_sub(1))
}

/// Whether the rows show the empty line after the file's last line break. It
/// is shown only when the caret or the selection's anchor is on it, so that a
/// file ending in a newline has no extra row to type into.
pub fn shows_tail(buf: &FileBuf) -> bool {
    let last = buf.line_count() - 1;
    let (anchor, head) = buf.caret();
    last == 0 || !buf.line(last).is_empty() || anchor.0 == last || head.0 == last
}

/// A caret motion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    Left,
    Right,
    Up,
    Down,
    WordLeft,
    WordRight,
    LineStart,
    LineEnd,
    DocStart,
    DocEnd,
}

/// One edit or motion the keys ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Text(String),
    Newline,
    Tab,
    Backspace,
    Delete,
    Move { motion: Motion, extend: bool },
    Undo,
    Redo,
}

/// The command a key press means in the editor, or `None` when the key is
/// not an editing key and falls through to the window's shortcuts. `key` is
/// the key's name and `key_char` the text it types, if any.
pub fn command(key: &str, key_char: Option<&str>, mods: Modifiers) -> Option<Command> {
    let secondary = mods.platform || mods.control;
    let word = mods.alt || mods.control;
    let motion = |motion, extend| Some(Command::Move { motion, extend });
    let extend = mods.shift;
    match key {
        "left" if mods.platform => motion(Motion::LineStart, extend),
        "right" if mods.platform => motion(Motion::LineEnd, extend),
        "left" if word => motion(Motion::WordLeft, extend),
        "right" if word => motion(Motion::WordRight, extend),
        "left" => motion(Motion::Left, extend),
        "right" => motion(Motion::Right, extend),
        "up" if mods.platform => motion(Motion::DocStart, extend),
        "down" if mods.platform => motion(Motion::DocEnd, extend),
        "up" => motion(Motion::Up, extend),
        "down" => motion(Motion::Down, extend),
        "home" if secondary => motion(Motion::DocStart, extend),
        "end" if secondary => motion(Motion::DocEnd, extend),
        "home" => motion(Motion::LineStart, extend),
        "end" => motion(Motion::LineEnd, extend),
        "enter" if !secondary => Some(Command::Newline),
        "tab" if !secondary => Some(Command::Tab),
        "backspace" if !secondary => Some(Command::Backspace),
        "delete" if !secondary => Some(Command::Delete),
        _ if secondary => None,
        _ => key_char
            .filter(|text| !text.is_empty() && !text.chars().any(char::is_control))
            .map(|text| Command::Text(text.to_owned())),
    }
}

/// Applies `command` to `buf`. Returns true when the text may have changed,
/// so the rows must be rebuilt; a motion returns false.
pub fn apply(buf: &mut FileBuf, command: &Command) -> bool {
    match command {
        Command::Text(text) => buf.insert(text),
        Command::Newline => buf.newline(),
        Command::Tab => buf.tab(),
        Command::Backspace => buf.backspace(),
        Command::Delete => buf.delete(),
        Command::Undo => return buf.undo(),
        Command::Redo => return buf.redo(),
        Command::Move { motion, extend } => {
            let extend = *extend;
            match motion {
                Motion::Left => buf.move_left(extend),
                Motion::Right => buf.move_right(extend),
                Motion::Up => buf.move_up(extend),
                Motion::Down => buf.move_down(extend),
                Motion::WordLeft => buf.word_left(extend),
                Motion::WordRight => buf.word_right(extend),
                Motion::LineStart => buf.home(extend),
                Motion::LineEnd => buf.end(extend),
                Motion::DocStart => buf.doc_start(extend),
                Motion::DocEnd => buf.doc_end(extend),
            }
            return false;
        }
    }
    true
}

/// The buffer's lines as a diff: one hunk, every line a context line except
/// the numbers in `marks`, which are added lines as the change marked them.
/// The empty line after a final line break is left out unless `shows_tail`.
pub fn rows(buf: &FileBuf, marks: &BTreeSet<u32>) -> FileDiff {
    let count = if shows_tail(buf) {
        buf.line_count()
    } else {
        buf.line_count() - 1
    };
    let lines: Vec<Line> = (0..count)
        .map(|index| {
            let no = u32::try_from(index + 1).unwrap_or(u32::MAX);
            let added = marks.contains(&no);
            Line {
                kind: if added {
                    LineKind::Added
                } else {
                    LineKind::Context
                },
                old_no: (!added).then_some(no),
                new_no: Some(no),
                text: buf.line(index).to_owned(),
            }
        })
        .collect();
    let added = lines
        .iter()
        .filter(|line| line.kind == LineKind::Added)
        .count();
    FileDiff {
        hunks: vec![Hunk {
            header: String::new(),
            lines,
        }],
        binary: false,
        added,
        removed: 0,
    }
}

/// The numbers of the added lines of `diff`, the lines a change marked.
pub fn marks_of(diff: &FileDiff) -> BTreeSet<u32> {
    diff.hunks
        .iter()
        .flat_map(|hunk| hunk.lines.iter())
        .filter(|line| line.kind == LineKind::Added)
        .filter_map(|line| line.new_no)
        .collect()
}

/// Whether the open file can be edited: its whole-file view is shown as text,
/// the view is not cut short, and no Compare is open.
pub fn editable(
    mode: ViewMode,
    diff: &Result<Arc<FileDiff>, String>,
    cut: bool,
    compare_open: bool,
) -> bool {
    mode == ViewMode::File
        && !cut
        && !compare_open
        && diff
            .as_ref()
            .is_ok_and(|diff| !diff.binary && !diff.hunks.is_empty())
}

/// The caret of a buffer as the code area has it: the anchor and the head.
pub fn caret_of(buf: &FileBuf) -> (TextPos, TextPos) {
    let (anchor, head) = buf.caret();
    (
        TextPos {
            line: anchor.0,
            col: anchor.1,
        },
        TextPos {
            line: head.0,
            col: head.1,
        },
    )
}

/// Moves the buffer's selection to the code area's caret.
pub fn apply_caret(buf: &mut FileBuf, anchor: TextPos, head: TextPos) {
    buf.select((anchor.line, anchor.col), (head.line, head.col));
}

/// Whether a comment may be written on the file at `path`: not while its
/// buffer has unsaved edits.
pub fn comment_allowed(edits: &BTreeMap<String, Edit>, path: &str) -> bool {
    edits.get(path).is_none_or(|edit| !edit.buf.is_dirty())
}

impl Shell {
    /// Whether the open file takes typing: it can be edited, the code area
    /// has a caret, and the review screen shows it.
    pub(crate) fn can_type(&self) -> bool {
        self.selected.as_ref().is_some_and(|selected| {
            editable(
                selected.mode,
                &selected.diff,
                selected.cut,
                self.tools.compare.is_some(),
            )
        }) && self.code_caret.ends().is_some()
            && matches!(self.screen, Screen::Diff | Screen::Files)
    }

    /// Whether the open file is editable and has a buffer, for undo and redo.
    fn has_buffer(&self) -> bool {
        self.selected.as_ref().is_some_and(|selected| {
            editable(
                selected.mode,
                &selected.diff,
                selected.cut,
                self.tools.compare.is_some(),
            ) && self.edits.contains_key(&selected.path)
        })
    }

    /// Whether the open file has unsaved edits, so the highlighter stays off.
    pub(crate) fn file_dirty(&self) -> bool {
        self.selected
            .as_ref()
            .and_then(|selected| self.edits.get(&selected.path))
            .is_some_and(|edit| edit.buf.is_dirty())
    }

    /// Refuses a comment on a file with unsaved edits: sets the message and
    /// returns true. Otherwise returns false and does nothing.
    pub(crate) fn refuse_unsaved_comment(&mut self) -> bool {
        let Some(selected) = self.selected.as_ref() else {
            return false;
        };
        if comment_allowed(&self.edits, &selected.path) {
            return false;
        }
        self.edit_notice = Some(SAVE_FIRST.to_owned());
        true
    }

    /// Creates the buffer of the open file from its bytes on disk, with the
    /// caret where the code area has it. False when the file cannot be read
    /// as text; the buffer is then not created.
    fn open_buffer(&mut self) -> bool {
        let Some(selected) = self.selected.as_ref() else {
            return false;
        };
        let path = selected.path.clone();
        let marks = match &selected.diff {
            Ok(diff) => marks_of(diff),
            Err(_) => return false,
        };
        let dir = Path::new(&self.snapshot.workspace);
        let Ok(bytes) = file_view::read(dir, &path) else {
            return false;
        };
        let text = match context::shown(&bytes) {
            Shown::Text { cut: false, .. } => match std::str::from_utf8(&bytes) {
                Ok(text) => text.to_owned(),
                Err(_) => return false,
            },
            _ => return false,
        };
        let mut buf = FileBuf::open(path.clone(), &text, filesave::blob_id(&bytes));
        if let Some((anchor, head)) = self.code_caret.ends() {
            apply_caret(&mut buf, anchor, head);
        }
        let tail = shows_tail(&buf);
        self.edits.insert(
            path,
            Edit {
                buf,
                marks,
                dirty: false,
                base: None,
                gone_acked: false,
                tail,
            },
        );
        true
    }

    /// Rebuilds the open file's rows from its buffer and moves the code's
    /// caret to the buffer's. Edits bump the edit revision, which rebuilds
    /// the diff list, and reveal the caret.
    fn refresh_edit_rows(&mut self) {
        let Some(path) = self.selected.as_ref().map(|selected| selected.path.clone()) else {
            return;
        };
        let Some(edit) = self.edits.get_mut(&path) else {
            return;
        };
        let dirty = edit.buf.is_dirty();
        let flipped = dirty != edit.dirty;
        edit.dirty = dirty;
        edit.tail = shows_tail(&edit.buf);
        let diff = rows(&edit.buf, &edit.marks);
        let (anchor, head) = caret_of(&edit.buf);
        if let Some(selected) = self.selected.as_mut() {
            selected.diff = Ok(Arc::new(diff));
            selected.editing = Editing::of(true, dirty);
        }
        // Highlighting is off while the file is dirty, since its lines no
        // longer match the spans; it starts again when the file is clean.
        if flipped || dirty {
            self.highlights = Highlighter::default();
        }
        self.edit_revision = self.edit_revision.wrapping_add(1);
        self.code_caret.set_ends(anchor, head);
        self.reveal_caret = true;
    }

    /// Runs `command` on the open file's buffer, creating it first, then
    /// keeps the caret and the rows in step with the buffer. The rows are
    /// rebuilt when the buffer is new or its text changed. Returns whether the
    /// text changed.
    pub(crate) fn run_edit(&mut self, command: &Command, cx: &mut Context<Self>) -> bool {
        let Some(path) = self.selected.as_ref().map(|selected| selected.path.clone()) else {
            return false;
        };
        let created = !self.edits.contains_key(&path);
        if created && !self.open_buffer() {
            return false;
        }
        let Some(edit) = self.edits.get_mut(&path) else {
            return false;
        };
        let changed = apply(&mut edit.buf, command);
        let reshaped = shows_tail(&edit.buf) != edit.tail;
        if changed || created || reshaped {
            self.refresh_edit_rows();
        } else if let Some(edit) = self.edits.get(&path) {
            let (anchor, head) = caret_of(&edit.buf);
            self.code_caret.set_ends(anchor, head);
            self.reveal_caret = true;
        }
        self.edit_notice = None;
        cx.notify();
        changed
    }

    /// Undo or redo on the open file. False when the file has no buffer or
    /// nothing to undo or redo, so ⌘Z can fall through to its other meaning.
    pub(crate) fn edit_history(&mut self, redo: bool, cx: &mut Context<Self>) -> bool {
        if !self.has_buffer() {
            return false;
        }
        let command = if redo { Command::Redo } else { Command::Undo };
        self.run_edit(&command, cx)
    }

    /// Drops the caret; the keys go back to the window's shortcuts.
    pub(crate) fn drop_caret(&mut self, cx: &mut Context<Self>) {
        self.code_caret.clear();
        cx.notify();
    }

    /// After a press or drag in the code area, the buffer's selection follows
    /// the caret, when the open file has a buffer.
    pub(crate) fn caret_to_buffer(&mut self) {
        let Some(path) = self.selected.as_ref().map(|selected| selected.path.clone()) else {
            return;
        };
        let Some((anchor, head)) = self.code_caret.ends() else {
            return;
        };
        if let Some(edit) = self.edits.get_mut(&path) {
            apply_caret(&mut edit.buf, anchor, head);
        }
    }

    /// ⌘Z: undoes an edit of the open file; otherwise it restores.
    pub(crate) fn on_restore_latest(
        &mut self,
        _: &RestoreLatest,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.edit_history(false, cx) {
            self.restore_latest(cx);
        }
    }

    /// ⌘⇧Z: redoes the last undone edit of the open file.
    pub(crate) fn on_edit_redo(&mut self, _: &EditRedo, _: &mut Window, cx: &mut Context<Self>) {
        self.edit_history(true, cx);
    }

    /// ⌘X: removes the selection and copies it, outside the text fields and the terminal.
    pub(crate) fn on_edit_cut(&mut self, _: &EditCut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.terminal_focused(window, cx) && !self.message_focused(window, cx) {
            self.edit_cut(cx);
        }
    }

    /// ⌘V: pastes at the caret of the open file, outside the text fields and the terminal.
    pub(crate) fn on_edit_paste(
        &mut self,
        _: &EditPaste,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.terminal_focused(window, cx) && !self.message_focused(window, cx) {
            self.edit_paste(cx);
        }
    }

    /// ⌘X: copies the selection and removes it, when typing is on.
    pub(crate) fn edit_cut(&mut self, cx: &mut Context<Self>) {
        if !self.can_type() {
            return;
        }
        let Some(path) = self.selected.as_ref().map(|selected| selected.path.clone()) else {
            return;
        };
        if !self.edits.contains_key(&path) && !self.open_buffer() {
            return;
        }
        let Some(edit) = self.edits.get_mut(&path) else {
            return;
        };
        let text = edit.buf.selected_text().to_owned();
        if text.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.run_edit(&Command::Backspace, cx);
    }

    /// ⌘V: inserts the clipboard's text at the caret, when typing is on.
    pub(crate) fn edit_paste(&mut self, cx: &mut Context<Self>) {
        if !self.can_type() {
            return;
        }
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        if text.is_empty() {
            return;
        }
        let Some(path) = self.selected.as_ref().map(|selected| selected.path.clone()) else {
            return;
        };
        if !self.edits.contains_key(&path) && !self.open_buffer() {
            return;
        }
        self.run_edit(&Command::Text(text.replace("\r\n", "\n")), cx);
    }

    /// Keeps the buffer of the open file across a snapshot or a read from
    /// disk. The disk is compared with the buffer's save base: a clean buffer
    /// whose file changed is dropped, so the disk's text shows; a dirty one
    /// keeps its own rows and raises the disk card. The open file's rows are
    /// the buffer's when it is in its whole-file view.
    pub(crate) fn overlay_edit(&mut self, selected: &mut Selected) {
        let path = selected.path.clone();
        let Some(edit) = self.edits.get(&path) else {
            return;
        };
        let disk = file_view::read(Path::new(&self.snapshot.workspace), &path)
            .ok()
            .map(|bytes| filesave::blob_id(&bytes));
        let state = disk_card::disk_state(edit.buf.is_dirty(), edit.base(), disk.as_deref());
        let gone_acked = edit.gone_acked;
        match disk_card::need(state, disk.as_deref(), gone_acked) {
            Need::Reload => {
                self.edits.remove(&path);
                self.highlights = Highlighter::default();
                return;
            }
            Need::Ask(prompt) => {
                self.disk_card = Some(DiskCard {
                    path: path.clone(),
                    prompt,
                });
            }
            Need::Nothing => {}
        }
        if let Some(edit) = self
            .edits
            .get_mut(&path)
            .filter(|_| state != DiskState::Gone)
        {
            edit.gone_acked = false;
        }
        if selected.mode != ViewMode::File {
            return;
        }
        let Some(edit) = self.edits.get(&path) else {
            return;
        };
        selected.diff = Ok(Arc::new(rows(&edit.buf, &edit.marks)));
        selected.editing = Editing::of(true, edit.buf.is_dirty());
        if edit.buf.is_dirty() {
            self.highlights = Highlighter::default();
        }
    }

    /// Whether the file at `path` has unsaved edits.
    pub(crate) fn dirty_path(&self, path: &str) -> bool {
        self.edits.get(path).is_some_and(|edit| edit.buf.is_dirty())
    }

    /// The paths of every file with unsaved edits, for the quit question.
    pub(crate) fn unsaved_paths(&self) -> Vec<String> {
        self.edits
            .iter()
            .filter(|(_, edit)| edit.buf.is_dirty())
            .map(|(path, _)| path.clone())
            .collect()
    }

    /// ⌘S: writes the open file's buffer when it has unsaved edits. A file
    /// that moved or went shows the card; any other failure is one line.
    pub(crate) fn save_file(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.selected.as_ref().map(|selected| selected.path.clone()) else {
            return;
        };
        let Some(edit) = self.edits.get(&path) else {
            return;
        };
        if !edit.buf.is_dirty() {
            return;
        }
        let text = edit.buf.to_disk_text();
        let base = edit.base().to_owned();
        let full = Path::new(&self.snapshot.workspace).join(&path);
        match filesave::save(&full, &text, &base, false) {
            Ok(blob) => self.saved(&path, blob, cx),
            Err(error) => match disk_card::save_failure(&error) {
                SaveFailure::Ask(prompt) => {
                    self.disk_card = Some(DiskCard { path, prompt });
                }
                SaveFailure::Notice(message) => self.edit_notice = Some(message),
            },
        }
        cx.notify();
    }

    /// After a write: the buffer is clean at the new blob, the snapshot and
    /// the Changes list are read again from disk, the change marks are the
    /// saved file's, and the rows and caret follow the buffer.
    fn saved(&mut self, path: &str, blob: String, cx: &mut Context<Self>) {
        if let Some(edit) = self.edits.get_mut(path) {
            edit.buf.mark_saved(blob);
            edit.base = None;
            edit.gone_acked = false;
        }
        if self
            .disk_card
            .as_ref()
            .is_some_and(|card| card.path == path)
        {
            self.disk_card = None;
        }
        self.edit_notice = None;
        if let Err(message) = self.refresh() {
            self.edit_notice = Some(format!("Saved, but the view did not reload: {message}"));
        }
        self.reload_changes();
        self.refresh_marks(path);
        if self
            .selected
            .as_ref()
            .is_some_and(|selected| selected.path == path && selected.mode == ViewMode::File)
        {
            // The reload moved the generation, which drops the caret; the
            // buffer has the caret, so it comes back from there.
            self.code_caret.scope(path, self.diff_generation);
            self.refresh_edit_rows();
        }
        cx.notify();
    }

    /// Recomputes the change marks of `path` from the saved file against the
    /// base. Done on save, since the marks drift while the file is edited.
    fn refresh_marks(&mut self, path: &str) {
        let fresh = file_view::load_selected(&self.snapshot, path, ViewMode::File);
        if let (Ok(diff), Some(edit)) = (&fresh.diff, self.edits.get_mut(path)) {
            edit.marks = marks_of(diff);
        }
    }

    /// The Edit action: switches the open file from its diff to its whole-file
    /// view, with the caret on the diff's selected line (or the caret's row).
    /// Does nothing where the affordance is absent.
    pub(crate) fn edit_here(&mut self, cx: &mut Context<Self>) {
        let Some(selected) = self.selected.as_ref() else {
            return;
        };
        if selected.mode != ViewMode::Diff
            || !selected.changed
            || !selected.editing.offered()
            || self.tools.compare.is_some()
        {
            return;
        }
        let Ok(diff) = &selected.diff else {
            return;
        };
        let at = selected.selection.last().or_else(|| {
            self.code_caret
                .ends()
                .and_then(|(_, head)| line_pos(diff, head.line))
        });
        let line = edit_line(diff, at);
        let path = selected.path.clone();
        self.edit_jump = Some((path.clone(), line));
        self.request_file(&path, ViewMode::File, true, cx);
    }

    /// Places the caret for a pending Edit once the file is shown in its
    /// whole-file view, and keeps the request until then.
    pub(crate) fn apply_edit_jump(&mut self) {
        let Some((path, line)) = self.edit_jump.clone() else {
            return;
        };
        let Some(selected) = self.selected.as_ref() else {
            return;
        };
        if selected.path != path || selected.mode != ViewMode::File {
            return;
        }
        let last = match &selected.diff {
            Ok(diff) => diff
                .hunks
                .iter()
                .map(|shown| shown.lines.len())
                .sum::<usize>()
                .saturating_sub(1),
            Err(_) => 0,
        };
        self.edit_jump = None;
        let at = TextPos {
            line: line.min(last),
            col: 0,
        };
        self.code_caret.set_ends(at, at);
        self.caret_to_buffer();
        self.reveal_caret = true;
    }

    /// The key handling of the editor, for a key press on the review screen
    /// with no field open. True when the key was taken.
    pub(crate) fn edit_key(
        &mut self,
        key: &str,
        key_char: Option<&str>,
        mods: Modifiers,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.can_type() {
            return false;
        }
        if key == "escape" {
            self.drop_caret(cx);
            return true;
        }
        match command(key, key_char, mods) {
            Some(command) => {
                self.run_edit(&command, cx);
                true
            }
            None => false,
        }
    }

    /// Scrolls the diff list to the caret, once per change of the caret.
    pub(crate) fn reveal_pending_caret(&mut self) {
        if !std::mem::take(&mut self.reveal_caret) {
            return;
        }
        if let (Some(list), Some((_, head))) = (self.diff_list.as_ref(), self.code_caret.ends()) {
            list.reveal_line(head.line);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf(text: &str) -> FileBuf {
        FileBuf::open("a.rs".to_owned(), text, "b1".to_owned())
    }

    fn mods(shift: bool, alt: bool, platform: bool) -> Modifiers {
        Modifiers {
            shift,
            alt,
            platform,
            ..Modifiers::default()
        }
    }

    #[test]
    fn printable_text_types_and_control_text_does_not() {
        assert_eq!(
            command("a", Some("a"), Modifiers::default()),
            Some(Command::Text("a".to_owned()))
        );
        assert_eq!(
            command("a", Some("A"), mods(true, false, false)),
            Some(Command::Text("A".to_owned()))
        );
        // A chord with ⌘ is a shortcut, not text.
        assert_eq!(command("a", Some("a"), mods(false, false, true)), None);
        assert_eq!(command("f1", None, Modifiers::default()), None);
    }

    #[test]
    fn named_keys_map_to_their_edits_and_motions() {
        let none = Modifiers::default();
        assert_eq!(command("enter", None, none), Some(Command::Newline));
        assert_eq!(command("tab", None, none), Some(Command::Tab));
        assert_eq!(command("backspace", None, none), Some(Command::Backspace));
        assert_eq!(command("delete", None, none), Some(Command::Delete));
        assert_eq!(
            command("left", None, mods(true, false, false)),
            Some(Command::Move {
                motion: Motion::Left,
                extend: true
            })
        );
        assert_eq!(
            command("left", None, mods(false, true, false)),
            Some(Command::Move {
                motion: Motion::WordLeft,
                extend: false
            })
        );
        assert_eq!(
            command("right", None, mods(false, false, true)),
            Some(Command::Move {
                motion: Motion::LineEnd,
                extend: false
            })
        );
        assert_eq!(
            command("up", None, mods(false, false, true)),
            Some(Command::Move {
                motion: Motion::DocStart,
                extend: false
            })
        );
        assert_eq!(command("escape", None, none), None);
    }

    #[test]
    fn typed_keys_apply_to_the_buffer_and_motions_do_not_change_text() {
        let mut b = buf("ab");
        b.set_cursor(0, 2, false);
        assert!(apply(&mut b, &Command::Text("c".to_owned())));
        assert_eq!(b.text(), "abc");
        assert!(apply(&mut b, &Command::Newline));
        assert!(apply(&mut b, &Command::Backspace));
        assert_eq!(b.text(), "abc");
        assert!(!apply(
            &mut b,
            &Command::Move {
                motion: Motion::DocStart,
                extend: false
            }
        ));
        assert_eq!(b.cursor_pos(), (0, 0));
    }

    #[test]
    fn undo_and_redo_report_whether_anything_changed() {
        let mut b = buf("x");
        b.set_cursor(0, 1, false);
        apply(&mut b, &Command::Text("y".to_owned()));
        assert!(apply(&mut b, &Command::Undo));
        assert_eq!(b.text(), "x");
        assert!(!apply(&mut b, &Command::Undo));
        assert!(apply(&mut b, &Command::Redo));
        assert_eq!(b.text(), "xy");
    }

    fn texts(diff: &FileDiff) -> Vec<&str> {
        diff.hunks[0]
            .lines
            .iter()
            .map(|l| l.text.as_str())
            .collect()
    }

    #[test]
    fn a_file_ending_in_a_newline_has_no_extra_row_unless_the_caret_is_on_it() {
        let mut b = buf("a\n");
        assert_eq!(texts(&rows(&b, &BTreeSet::new())), vec!["a"]);
        b.set_cursor(1, 0, false);
        assert_eq!(texts(&rows(&b, &BTreeSet::new())), vec!["a", ""]);
        let empty = rows(&buf(""), &BTreeSet::new());
        assert_eq!(empty.hunks[0].lines.len(), 1);
        assert_eq!(empty.hunks[0].lines[0].text, "");
    }

    #[test]
    fn a_line_break_in_the_middle_keeps_its_empty_line() {
        let b = buf("a\n\nb");
        assert_eq!(texts(&rows(&b, &BTreeSet::new())), vec!["a", "", "b"]);
    }

    #[test]
    fn quit_names_the_unsaved_files_and_the_terminals() {
        let files = vec!["a.rs".to_owned(), "src/b.rs".to_owned()];
        assert_eq!(quit_question(None, &[]), None);
        assert_eq!(
            quit_question(Some("Two terminals run.".to_owned()), &[]),
            Some("Two terminals run.".to_owned())
        );
        assert_eq!(
            quit_question(None, &files),
            Some("Unsaved edits in a.rs, src/b.rs will be discarded.".to_owned())
        );
        assert_eq!(
            quit_question(Some("One terminal runs.".to_owned()), &files[..1]),
            Some("One terminal runs. Unsaved edits in a.rs will be discarded.".to_owned())
        );
    }

    #[test]
    fn an_edit_lands_on_the_new_line_of_the_selected_row_or_the_one_above_it() {
        let diff = FileDiff {
            hunks: vec![Hunk {
                header: String::new(),
                lines: vec![
                    Line {
                        kind: LineKind::Context,
                        old_no: Some(1),
                        new_no: Some(1),
                        text: "a".to_owned(),
                    },
                    Line {
                        kind: LineKind::Removed,
                        old_no: Some(2),
                        new_no: None,
                        text: "gone".to_owned(),
                    },
                    Line {
                        kind: LineKind::Added,
                        old_no: None,
                        new_no: Some(2),
                        text: "b".to_owned(),
                    },
                ],
            }],
            binary: false,
            added: 1,
            removed: 1,
        };
        assert_eq!(edit_line(&diff, Some(Pos { hunk: 0, line: 2 })), 1);
        // A removed row has no new line; the line above it is the place.
        assert_eq!(edit_line(&diff, Some(Pos { hunk: 0, line: 1 })), 0);
        assert_eq!(edit_line(&diff, None), 0);
        assert_eq!(line_pos(&diff, 2), Some(Pos { hunk: 0, line: 2 }));
        assert_eq!(line_pos(&diff, 3), None);
    }

    #[test]
    fn marked_lines_stay_added_and_are_read_back_from_a_diff() {
        let marks: BTreeSet<u32> = [2].into_iter().collect();
        let diff = rows(&buf("a\nb\nc"), &marks);
        let kinds: Vec<LineKind> = diff.hunks[0].lines.iter().map(|l| l.kind).collect();
        assert_eq!(
            kinds,
            vec![LineKind::Context, LineKind::Added, LineKind::Context]
        );
        assert_eq!(diff.added, 1);
        assert_eq!(marks_of(&diff), marks);
    }

    #[test]
    fn a_file_is_editable_only_as_text_in_its_whole_view() {
        let text = Ok(Arc::new(rows(&buf("a"), &BTreeSet::new())));
        assert!(editable(ViewMode::File, &text, false, false));
        assert!(!editable(ViewMode::Diff, &text, false, false));
        assert!(!editable(ViewMode::File, &text, true, false));
        assert!(!editable(ViewMode::File, &text, false, true));
        let err: Result<Arc<FileDiff>, String> = Err("Cannot read".to_owned());
        assert!(!editable(ViewMode::File, &err, false, false));
    }

    #[test]
    fn a_dirty_file_refuses_a_comment_and_a_clean_one_allows_it() {
        let mut edits = BTreeMap::new();
        let mut dirty = buf("x");
        dirty.set_cursor(0, 1, false);
        dirty.insert("y");
        edits.insert(
            "dirty.rs".to_owned(),
            Edit {
                buf: dirty,
                marks: BTreeSet::new(),
                dirty: true,
                base: None,
                gone_acked: false,
                tail: false,
            },
        );
        edits.insert(
            "clean.rs".to_owned(),
            Edit {
                buf: buf("x"),
                marks: BTreeSet::new(),
                dirty: false,
                base: None,
                gone_acked: false,
                tail: false,
            },
        );
        assert!(!comment_allowed(&edits, "dirty.rs"));
        assert!(comment_allowed(&edits, "clean.rs"));
        assert!(comment_allowed(&edits, "other.rs"));
        assert!(SAVE_FIRST.contains("Save the file first"));
    }

    #[test]
    fn the_caret_round_trips_through_the_buffer_in_either_direction() {
        let mut b = buf("hello\nworld");
        let anchor = TextPos { line: 0, col: 4 };
        let head = TextPos { line: 1, col: 2 };
        apply_caret(&mut b, anchor, head);
        assert_eq!(caret_of(&b), (anchor, head));
        assert_eq!(b.selected_text(), "o\nwo");
        apply_caret(&mut b, head, head);
        assert_eq!(caret_of(&b), (head, head));
    }
}
