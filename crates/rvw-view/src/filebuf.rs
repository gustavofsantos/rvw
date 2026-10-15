//! The file buffer of the in-place editor: text, dirty state and the blob it was loaded from.
//!
//! A `FileBuf` is one file open for editing. It wraps an `Editor` (the whole
//! text, cursor, selection and undo) and adds what a file needs: the line
//! index, the line endings to write back, whether the file indents with tabs,
//! and the blob id it was loaded from so a change on disk is never clobbered.

use std::cell::OnceCell;

use crate::editor::Editor;

/// How the file on disk compares with the buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskState {
    /// The file on disk is the one the buffer was loaded from or last saved as.
    Same,
    /// The file changed on disk and the buffer has no unsaved edits: reload it.
    ChangedClean,
    /// The file changed on disk and the buffer has unsaved edits: a conflict.
    ChangedDirty,
    /// The file is no longer on disk.
    Gone,
}

/// One file open for editing.
#[derive(Debug, Clone)]
pub struct FileBuf {
    path: String,
    editor: Editor,
    /// The text the buffer was loaded or last saved with, for the dirty check.
    loaded: String,
    /// The blob id of the bytes the buffer was loaded or last saved from.
    loaded_blob: String,
    /// Whether the file's line breaks are CRLF, written back on save.
    crlf: bool,
    /// Whether the file indents with tabs, which Tab then inserts.
    tabs: bool,
    /// Byte offset of each line's start. Rebuilt on the first read after a change.
    lines: OnceCell<Vec<usize>>,
}

impl FileBuf {
    /// Opens `text`, the bytes of the file whose blob id is `blob_id`. The
    /// cursor starts at the top of the file with no history.
    pub fn open(path: String, text: &str, blob_id: String) -> FileBuf {
        let crlf = is_crlf(text);
        let editor = Editor::from_text(text);
        let loaded = editor.text().to_string();
        let tabs = indents_with_tabs(&loaded);
        let mut buf = FileBuf {
            path,
            editor,
            loaded,
            loaded_blob: blob_id,
            crlf,
            tabs,
            lines: OnceCell::new(),
        };
        buf.editor.set_cursor(0, false);
        buf
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    /// The whole text, with `\n` line breaks.
    pub fn text(&self) -> &str {
        self.editor.text()
    }

    pub fn loaded_blob(&self) -> &str {
        &self.loaded_blob
    }

    /// True when the text differs from the text last loaded or saved. Undoing
    /// back to that text clears it.
    pub fn is_dirty(&self) -> bool {
        self.editor.text() != self.loaded
    }

    /// The text as it is written to disk: CRLF again when the file used it.
    pub fn to_disk_text(&self) -> String {
        if self.crlf {
            self.editor.text().replace('\n', "\r\n")
        } else {
            self.editor.text().to_string()
        }
    }

    /// Records that the buffer's text is now on disk as `new_blob_id`. Clears
    /// the dirty state and keeps the undo history.
    pub fn mark_saved(&mut self, new_blob_id: String) {
        self.loaded = self.editor.text().to_string();
        self.loaded_blob = new_blob_id;
    }

    /// Compares the buffer with the file on disk, whose blob id is `disk_blob`
    /// (`None` when the file is gone).
    pub fn disk_state(&self, disk_blob: Option<&str>) -> DiskState {
        match disk_blob {
            None => DiskState::Gone,
            Some(blob) if blob == self.loaded_blob => DiskState::Same,
            Some(_) if self.is_dirty() => DiskState::ChangedDirty,
            Some(_) => DiskState::ChangedClean,
        }
    }

    /// The number of lines. Always at least 1.
    pub fn line_count(&self) -> usize {
        self.starts().len()
    }

    /// Line `n` without its newline. Lines past the end give the last line.
    pub fn line(&self, n: usize) -> &str {
        let n = n.min(self.line_count() - 1);
        let start = self.starts()[n];
        let end = self
            .starts()
            .get(n + 1)
            .map_or(self.editor.text().len(), |next| next - 1);
        &self.editor.text()[start..end]
    }

    /// Byte offset where line `n` starts. Lines past the end give the last line.
    pub fn line_start(&self, n: usize) -> usize {
        self.starts()[n.min(self.line_count() - 1)]
    }

    /// The cursor as (line, column in bytes), both 0-based.
    pub fn cursor_pos(&self) -> (usize, usize) {
        let offset = self.editor.cursor();
        let line = self.starts().partition_point(|&start| start <= offset) - 1;
        (line, offset - self.starts()[line])
    }

    /// Moves the cursor to (line, column in bytes). The column is clamped to
    /// the line, and the line to the file. With `extend`, the anchor stays put.
    pub fn set_cursor(&mut self, line: usize, col: usize, extend: bool) {
        let line = line.min(self.line_count() - 1);
        let start = self.line_start(line);
        let end = start + self.line(line).len();
        self.editor.set_cursor((start + col).min(end), extend);
    }

    /// Replaces the selection, or inserts at the cursor.
    pub fn insert(&mut self, s: &str) {
        self.editor.insert(s);
        self.changed();
    }

    /// Deletes the selection, or the character before the cursor.
    pub fn backspace(&mut self) {
        self.editor.backspace();
        self.changed();
    }

    /// Deletes the selection, or the character after the cursor.
    pub fn delete(&mut self) {
        self.editor.delete();
        self.changed();
    }

    /// Breaks the line at the cursor. The new line starts with the leading
    /// spaces and tabs of the cursor's line.
    pub fn newline(&mut self) {
        let (line, _) = self.cursor_pos();
        let indent: String = self
            .line(line)
            .chars()
            .take_while(|&c| c == ' ' || c == '\t')
            .collect();
        self.insert(&format!("\n{indent}"));
    }

    /// Inserts one indent unit: a tab in a file that indents with tabs, else
    /// four spaces.
    pub fn tab(&mut self) {
        if self.tabs {
            self.insert("\t");
        } else {
            self.insert("    ");
        }
    }

    /// Reverts the last edit step. Returns false when there is nothing to undo.
    pub fn undo(&mut self) -> bool {
        let done = self.editor.undo();
        self.changed();
        done
    }

    /// Re-applies the last undone step. Returns false when there is nothing to redo.
    pub fn redo(&mut self) -> bool {
        let done = self.editor.redo();
        self.changed();
        done
    }

    pub fn move_left(&mut self, extend: bool) {
        self.editor.move_left(extend);
    }

    pub fn move_right(&mut self, extend: bool) {
        self.editor.move_right(extend);
    }

    pub fn move_up(&mut self, extend: bool) {
        self.editor.move_up(extend);
    }

    pub fn move_down(&mut self, extend: bool) {
        self.editor.move_down(extend);
    }

    pub fn word_left(&mut self, extend: bool) {
        self.editor.word_left(extend);
    }

    pub fn word_right(&mut self, extend: bool) {
        self.editor.word_right(extend);
    }

    /// Moves to the start of the cursor's line (Home).
    pub fn home(&mut self, extend: bool) {
        self.editor.line_start(extend);
    }

    /// Moves to the end of the cursor's line (End).
    pub fn end(&mut self, extend: bool) {
        self.editor.line_end(extend);
    }

    pub fn doc_start(&mut self, extend: bool) {
        self.editor.doc_start(extend);
    }

    pub fn doc_end(&mut self, extend: bool) {
        self.editor.doc_end(extend);
    }

    /// Line start offsets, built on first use after a change.
    fn starts(&self) -> &[usize] {
        self.lines.get_or_init(|| {
            std::iter::once(0)
                .chain(self.editor.text().match_indices('\n').map(|(i, _)| i + 1))
                .collect()
        })
    }

    /// Drops the line index after the text may have changed.
    fn changed(&mut self) {
        self.lines = OnceCell::new();
    }
}

/// True when the file's line breaks are mostly CRLF.
fn is_crlf(text: &str) -> bool {
    let crlf = text.matches("\r\n").count();
    let all = text.matches('\n').count();
    crlf > 0 && crlf * 2 >= all
}

/// True when the first line with text that is indented starts with a tab.
fn indents_with_tabs(text: &str) -> bool {
    text.lines()
        .find(|line| (line.starts_with(' ') || line.starts_with('\t')) && !line.trim().is_empty())
        .is_some_and(|line| line.starts_with('\t'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf(text: &str) -> FileBuf {
        FileBuf::open("src/a.rs".to_string(), text, "b1".to_string())
    }

    #[test]
    fn open_starts_clean_at_the_top_with_the_loaded_blob() {
        let b = buf("hello\nworld");
        assert_eq!(b.path(), "src/a.rs");
        assert_eq!(b.loaded_blob(), "b1");
        assert!(!b.is_dirty());
        assert_eq!(b.cursor_pos(), (0, 0));
    }

    #[test]
    fn edit_is_dirty_and_undo_back_to_loaded_is_clean() {
        let mut b = buf("hello");
        b.set_cursor(0, 5, false);
        b.insert("!");
        assert_eq!(b.text(), "hello!");
        assert!(b.is_dirty());
        assert!(b.undo());
        assert_eq!(b.text(), "hello");
        assert!(!b.is_dirty());
    }

    #[test]
    fn typed_run_is_one_undo_step_back_to_clean() {
        let mut b = buf("hi");
        b.set_cursor(0, 2, false);
        b.insert("x");
        b.insert("y");
        assert!(b.undo());
        assert_eq!(b.text(), "hi");
        assert!(!b.is_dirty());
        assert!(!b.undo());
    }

    #[test]
    fn dirty_compares_text_not_history() {
        let mut b = buf("hi");
        b.set_cursor(0, 2, false);
        b.insert("x");
        b.backspace();
        assert_eq!(b.text(), "hi");
        assert!(!b.is_dirty());
    }

    #[test]
    fn redo_restores_the_edit_and_dirty() {
        let mut b = buf("hi");
        b.set_cursor(0, 2, false);
        b.insert("x");
        b.undo();
        assert!(b.redo());
        assert_eq!(b.text(), "hix");
        assert!(b.is_dirty());
    }

    #[test]
    fn crlf_file_round_trips_and_edits_keep_crlf() {
        let mut b = buf("a\r\nb\r\n");
        assert_eq!(b.text(), "a\nb\n");
        assert_eq!(b.line_count(), 3);
        assert_eq!(b.to_disk_text(), "a\r\nb\r\n");
        b.set_cursor(2, 0, false);
        b.insert("c");
        assert_eq!(b.to_disk_text(), "a\r\nb\r\nc");
    }

    #[test]
    fn lf_file_stays_lf_and_lone_cr_is_normalised() {
        let b = buf("a\nb");
        assert_eq!(b.to_disk_text(), "a\nb");
        let lone = buf("a\rb");
        assert_eq!(lone.text(), "a\nb");
        assert_eq!(lone.to_disk_text(), "a\nb");
    }

    #[test]
    fn newline_keeps_the_leading_whitespace_of_the_line() {
        let mut b = buf("fn f() {\n    x\n}");
        b.set_cursor(1, 5, false);
        b.newline();
        assert_eq!(b.line(2), "    ");
        assert_eq!(b.cursor_pos(), (2, 4));
        b.insert("y");
        assert_eq!(b.line(2), "    y");
    }

    #[test]
    fn newline_on_unindented_line_adds_no_indent() {
        let mut b = buf("a");
        b.set_cursor(0, 1, false);
        b.newline();
        assert_eq!(b.text(), "a\n");
        assert_eq!(b.cursor_pos(), (1, 0));
    }

    #[test]
    fn tab_inserts_four_spaces_in_a_spaces_file() {
        let mut b = buf("fn a() {\n    x\n}");
        b.set_cursor(0, 0, false);
        b.tab();
        assert_eq!(b.line(0), "    fn a() {");
        assert_eq!(b.cursor_pos(), (0, 4));
    }

    #[test]
    fn tab_inserts_a_tab_in_a_tab_file_and_skips_blank_lines() {
        let mut b = buf("if x {\n   \n\ty\n}");
        b.set_cursor(0, 0, false);
        b.tab();
        assert_eq!(b.text(), "\tif x {\n   \n\ty\n}");
    }

    #[test]
    fn line_index_follows_edits() {
        let mut b = buf("a\nb\nc");
        assert_eq!(b.line_count(), 3);
        assert_eq!(b.line_start(2), 4);
        b.set_cursor(1, 1, false);
        b.insert("\nx");
        assert_eq!(b.line_count(), 4);
        assert_eq!(b.line(2), "x");
        assert_eq!(b.line_start(3), 6);
        b.undo();
        assert_eq!(b.line_count(), 3);
        assert_eq!(b.line(1), "b");
    }

    #[test]
    fn line_past_the_end_gives_the_last_line() {
        let b = buf("a\nbc");
        assert_eq!(b.line(9), "bc");
        assert_eq!(b.line_start(9), 2);
    }

    #[test]
    fn cursor_column_is_in_bytes_and_clamped_to_the_line() {
        let mut b = buf("é\nxy");
        b.set_cursor(0, 2, false);
        assert_eq!(b.cursor_pos(), (0, 2));
        b.set_cursor(0, 99, false);
        assert_eq!(b.cursor_pos(), (0, 2));
        b.set_cursor(99, 1, false);
        assert_eq!(b.cursor_pos(), (1, 1));
    }

    #[test]
    fn home_end_and_up_down_move_the_cursor() {
        let mut b = buf("abc\ndef");
        b.set_cursor(1, 2, false);
        b.home(false);
        assert_eq!(b.cursor_pos(), (1, 0));
        b.end(false);
        assert_eq!(b.cursor_pos(), (1, 3));
        b.move_up(false);
        assert_eq!(b.cursor_pos(), (0, 3));
        b.move_down(false);
        assert_eq!(b.cursor_pos(), (1, 3));
    }

    #[test]
    fn disk_state_matrix_when_clean() {
        let b = buf("x");
        assert_eq!(b.disk_state(Some("b1")), DiskState::Same);
        assert_eq!(b.disk_state(Some("b2")), DiskState::ChangedClean);
        assert_eq!(b.disk_state(None), DiskState::Gone);
    }

    #[test]
    fn disk_state_matrix_when_dirty() {
        let mut b = buf("x");
        b.set_cursor(0, 1, false);
        b.insert("y");
        assert_eq!(b.disk_state(Some("b1")), DiskState::Same);
        assert_eq!(b.disk_state(Some("b2")), DiskState::ChangedDirty);
        assert_eq!(b.disk_state(None), DiskState::Gone);
    }

    #[test]
    fn mark_saved_clears_dirty_and_keeps_undo() {
        let mut b = buf("x");
        b.set_cursor(0, 1, false);
        b.insert("y");
        b.mark_saved("b2".to_string());
        assert!(!b.is_dirty());
        assert_eq!(b.loaded_blob(), "b2");
        assert_eq!(b.disk_state(Some("b2")), DiskState::Same);
        assert!(b.undo());
        assert_eq!(b.text(), "x");
        assert!(b.is_dirty());
    }

    #[test]
    fn mark_saved_writes_crlf_back_on_later_saves() {
        let mut b = buf("a\r\nb");
        b.set_cursor(1, 1, false);
        b.insert("c");
        b.mark_saved("b2".to_string());
        assert_eq!(b.to_disk_text(), "a\r\nbc");
    }
}
