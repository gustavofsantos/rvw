//! Port of the update half of internal/tui: tui.go (key handling, jumps,
//! writing comments and the submit and compare menus), picker.go and mouse.go.
//! Loading, scrolling and the state changes they need live in model.rs; this
//! file says what each event changes and which effects the shell must run.

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use rvw_core::operations::{AddInput, EditInput, SubmitInput};
use rvw_core::types::DECISIONS;
use unicode_width::UnicodeWidthStr;

use crate::changes::Compare;
use crate::editor::{quote_code, strip_template, template, write_temp};
use crate::fuzzy::{rank, Candidate};
use crate::model::{
    clamp, CompareMenu, EditKind, EditRequest, Effect, Event, Model, MouseButton, MouseEvent,
    MouseKind, Pane, Picker, PickerKind, Sidebar, SubmitMenu, Target, Theme, STANDALONE,
};
use crate::pages::location;
use crate::rail::{covering, nearest, next_comment, Direction};
use crate::style::expand_tabs;
use crate::tree::{NodeId, ROOT};

/// Rows one wheel notch scrolls.
const WHEEL_STEP: i64 = 3;

/// Caps a typed count. Far above any file's length, so every use of the count
/// clamps to the same place as the uncapped Go value would.
const MAX_COUNT: i64 = 1_000_000_000;

/// The rows of the compare menu, in the order `Compare` indexes them in Go.
const COMPARE_ORDER: [Compare; 3] = [Compare::Head, Compare::Default, Compare::Prev];

impl Model {
    /// Applies an event and says what the shell must do. Calls `scroll` last,
    /// with margins except for mouse events (as the Go `Update`).
    pub fn update(&mut self, ev: Event) -> Vec<Effect> {
        let mut margins = true;
        let effects = match ev {
            Event::Resize { width, height } => {
                self.width = width;
                self.height = height;
                Vec::new()
            }
            Event::Background { dark } => {
                self.theme = Theme::new(dark);
                Vec::new()
            }
            Event::FlashDone(seq) => {
                if seq == self.flash_seq {
                    self.flash.clear();
                }
                Vec::new()
            }
            Event::EditorDone { req, path, error } => self.editor_done(&req, &path, error),
            Event::Key(name) => self.key(&name),
            Event::Mouse(e) => {
                margins = false;
                self.mouse(e)
            }
        };
        self.scroll(margins);
        effects
    }

    // ── keys ────────────────────────────────────────────────────────────────

    fn key(&mut self, s: &str) -> Vec<Effect> {
        if s == "ctrl+c" {
            return vec![Effect::Quit];
        }
        if self.help {
            self.help = false;
            return Vec::new();
        }
        if self.submit.is_some() {
            return self.submit_key(s);
        }
        if self.compare.is_some() {
            return self.compare_key(s);
        }
        if self.picker.is_some() {
            return self.picker_key(s);
        }
        if self.go_line.is_some() {
            self.go_line_key(s);
            return Vec::new();
        }
        if self.leader {
            self.leader = false;
            match s {
                "p" => self.open_picker(PickerKind::Files),
                "l" => self.open_picker(PickerKind::Comments),
                _ => {}
            }
            return Vec::new();
        }
        if s == self.leader_key() {
            self.leader = true;
            self.count.clear();
            return Vec::new();
        }
        if !self.prefix.is_empty() {
            return self.prefixed_key(s);
        }
        if is_digit(s) && (s != "0" || !self.count.is_empty()) {
            self.count.push_str(s);
            return Vec::new();
        }
        let count = take_count(&mut self.count);

        match s {
            "tab" => {
                self.focus = match self.focus {
                    Pane::Tree => Pane::Viewer,
                    Pane::Viewer => Pane::Tree,
                };
                if self.focus == Pane::Tree {
                    self.select_shown();
                }
                return Vec::new();
            }
            "?" => {
                self.help = true;
                return Vec::new();
            }
            "q" => return vec![Effect::Quit],
            "s" => {
                self.submit = Some(SubmitMenu::default());
                return Vec::new();
            }
            "t" => {
                let next = self.side.next();
                if let Err(e) = self.show_side(next) {
                    return self.fail(&e);
                }
                return Vec::new();
            }
            "b" => {
                self.compare = Some(CompareMenu {
                    cursor: compare_index(self.cmp),
                });
                return Vec::new();
            }
            "r" => {
                if let Err(e) = self.reload(true) {
                    return self.fail(&e);
                }
                return self.set_flash("reloaded", false);
            }
            "g" | "]" | "[" => {
                s.clone_into(&mut self.prefix);
                self.prefix_count = to_u(count);
                return Vec::new();
            }
            "esc" => {
                self.visual = false;
                if self.focus == Pane::Viewer && self.detail.is_some() {
                    self.detail = None;
                }
                return Vec::new();
            }
            _ => {}
        }
        if self.focus == Pane::Tree {
            self.tree_key(s, count)
        } else {
            self.viewer_key(s, count)
        }
    }

    /// The key after a pending g, ] or [.
    fn prefixed_key(&mut self, s: &str) -> Vec<Effect> {
        let prefix = std::mem::take(&mut self.prefix);
        let count = to_i(self.prefix_count).max(1);
        self.prefix_count = 0;
        match (prefix.as_str(), s) {
            ("g", "g") => {
                if self.focus == Pane::Tree {
                    self.tree_cur = 0;
                } else if let Some(d) = self.detail.as_mut() {
                    d.offset = 0;
                } else if self.file.is_some() {
                    self.goto_line(count);
                }
                Vec::new()
            }
            ("]", "c") => self.jump_comment(Direction::Forward),
            ("[", "c") => self.jump_comment(Direction::Backward),
            ("]", "h") => self.jump_hunk(Direction::Forward),
            ("[", "h") => self.jump_hunk(Direction::Backward),
            _ => Vec::new(),
        }
    }

    fn tree_key(&mut self, s: &str, count: i64) -> Vec<Effect> {
        if self.rows.is_empty() {
            return Vec::new();
        }
        let n = count.max(1);
        let last = to_i(self.rows.len()) - 1;
        let cur = self.rows[self.tree_cur.min(self.rows.len() - 1)].node;
        let (path, is_dir, expanded) = self.node_info(cur);
        match s {
            "j" | "down" => self.tree_cur = to_u(clamp(to_i(self.tree_cur) + n, 0, last)),
            "k" | "up" => self.tree_cur = to_u(clamp(to_i(self.tree_cur) - n, 0, last)),
            "G" => self.tree_cur = to_u(last),
            "l" | "right" | "enter" => {
                if self.side == Sidebar::Reviews && path != STANDALONE {
                    return self.open_review_row(cur);
                }
                if !is_dir {
                    return self.open_from_tree(&path);
                }
                // Enter toggles; l and right always expand.
                let now = !expanded || s != "enter";
                self.tree_mut().set_expanded(cur, now);
                self.refresh_rows();
            }
            "h" | "left" => {
                let parent = self.tree().parent(cur);
                if is_dir && expanded {
                    self.tree_mut().set_expanded(cur, false);
                    self.refresh_rows();
                } else if let Some(p) = parent.filter(|&p| p != ROOT) {
                    let parent_path = self.tree().node(p).path.clone();
                    self.select_tree_row(&parent_path);
                }
            }
            _ => {}
        }
        Vec::new()
    }

    /// The path, directory flag and expansion of a node of the shown tree.
    fn node_info(&self, id: NodeId) -> (String, bool, bool) {
        let n = self.tree().node(id);
        (n.path.clone(), n.dir, n.expanded)
    }

    /// Shows a review or comment of the reviews side in the viewer, expanding
    /// a review to list its comments.
    fn open_review_row(&mut self, id: NodeId) -> Vec<Effect> {
        let (path, is_dir, expanded) = self.node_info(id);
        if is_dir && !expanded {
            self.tree_mut().set_expanded(id, true);
            self.refresh_rows();
        }
        self.open_from_tree(&path)
    }

    /// Opens a file picked in the tree and moves to the viewer. From the
    /// changes view it lands on the file's first change; a file listed there
    /// as deleted has nothing on disk to open.
    fn open_from_tree(&mut self, path: &str) -> Vec<Effect> {
        if self.side == Sidebar::Reviews {
            if let Err(e) = self.show_detail(path) {
                return self.fail(&e);
            }
            self.focus = Pane::Viewer;
            return Vec::new();
        }
        if self.side == Sidebar::Changes && self.status.get(path) == Some(&b'D') {
            return self.set_flash(&format!("{path} was deleted: nothing to show"), false);
        }
        if let Err(e) = self.open_file(path, 0) {
            return self.fail(&e);
        }
        if self.side == Sidebar::Changes {
            let first = self
                .file
                .as_ref()
                .and_then(|f| f.hunks.iter().min().copied());
            if let Some(line) = first {
                self.goto_line(to_i(line));
                let body = to_i(self.body_height() / 3);
                if let Some(f) = self.file.as_mut() {
                    f.offset = to_u((to_i(f.cursor) - body).max(0));
                }
            }
        }
        self.focus = Pane::Viewer;
        Vec::new()
    }

    fn viewer_key(&mut self, s: &str, count: i64) -> Vec<Effect> {
        if self.detail.is_some() {
            return self.detail_key(s, count);
        }
        let (cur, len) = match self.file.as_ref() {
            Some(f) => (to_i(f.cursor), to_i(f.len())),
            None => return Vec::new(),
        };
        let n = count.max(1);
        match s {
            "j" | "down" => self.set_cursor(cur + n),
            "k" | "up" => self.set_cursor(cur - n),
            "ctrl+d" | "pgdown" => self.half_page(1),
            "ctrl+u" | "pgup" => self.half_page(-1),
            "G" => {
                if count > 0 {
                    self.goto_line(count);
                } else {
                    self.goto_line(len);
                }
            }
            ":" => self.go_line = Some(String::new()),
            "V" => {
                if len > 0 {
                    self.visual = !self.visual;
                    self.anchor = to_u(cur);
                }
            }
            "c" => return self.start_comment(),
            "e" => return self.start_edit(),
            _ => {}
        }
        Vec::new()
    }

    /// The key that starts leader mappings: space, unless configured.
    fn leader_key(&self) -> &str {
        if self.opts.leader.is_empty() {
            "space"
        } else {
            self.opts.leader.as_str()
        }
    }

    /// Scrolls a review or comment page, or leaves it: o for the file the
    /// comment is on, esc for the file underneath.
    fn detail_key(&mut self, s: &str, count: i64) -> Vec<Effect> {
        if s == "o" {
            return self.open_detail_comment();
        }
        let body = to_i(self.body_height());
        let width = self.view_width();
        let n = count.max(1);
        let Some(d) = self.detail.as_mut() else {
            return Vec::new();
        };
        match s {
            "j" | "down" => d.offset = shift(d.offset, n),
            "k" | "up" => d.offset = shift(d.offset, -n),
            "ctrl+d" | "pgdown" => d.offset = shift(d.offset, (body / 2).max(1)),
            "ctrl+u" | "pgup" => d.offset = shift(d.offset, -(body / 2).max(1)),
            "G" => d.offset = d.layout(width).len(),
            _ => {}
        }
        Vec::new()
    }

    /// Opens the file of the comment a detail page shows, on its first line.
    fn open_detail_comment(&mut self) -> Vec<Effect> {
        let target = self
            .detail
            .as_ref()
            .and_then(|d| d.ev.as_ref())
            .map(|ev| (ev.comment.file.clone(), ev.comment.start_line));
        let Some((file, line)) = target else {
            return self.set_flash(
                "a review is on no one file: open one of its comments",
                false,
            );
        };
        if let Err(e) = self.open_file(&file, to_u(line)) {
            return self.fail(&e);
        }
        self.select_shown();
        Vec::new()
    }

    /// Moves the viewer cursor to the 0-indexed `line`, clamped to the file.
    fn set_cursor(&mut self, line: i64) {
        if let Some(f) = self.file.as_mut() {
            let hi = to_i(f.len()) - 1;
            f.cursor = to_u(clamp(line, 0, hi));
        }
    }

    /// Moves the viewer cursor to the 1-indexed `line`.
    fn goto_line(&mut self, line: i64) {
        self.set_cursor(line - 1);
    }

    /// Moves the cursor and the scroll position by half a body in `dir`.
    fn half_page(&mut self, dir: i64) {
        let body = to_i(self.body_height());
        let h = (body / 2).max(1) * dir;
        let Some(f) = self.file.as_mut() else {
            return;
        };
        let len = to_i(f.len());
        f.cursor = to_u(clamp(to_i(f.cursor) + h, 0, len - 1));
        f.offset = to_u(clamp(to_i(f.offset) + h, 0, (len - body).max(0)));
    }

    fn jump_comment(&mut self, dir: Direction) -> Vec<Effect> {
        let Some(f) = self.file.as_ref() else {
            return Vec::new();
        };
        if self.detail.is_some() || self.focus != Pane::Viewer {
            return Vec::new();
        }
        let line = next_comment(&f.comments, to_i(f.cursor) + 1, dir);
        self.jump_to(line, dir, "comment")
    }

    fn jump_hunk(&mut self, dir: Direction) -> Vec<Effect> {
        let Some(f) = self.file.as_ref() else {
            return Vec::new();
        };
        if self.detail.is_some() || self.focus != Pane::Viewer {
            return Vec::new();
        }
        if f.hunks.is_empty() {
            return self.set_flash("no changes in this file", false);
        }
        let hunks: Vec<i64> = f.hunks.iter().map(|&h| to_i(h)).collect();
        let cursor = to_i(f.cursor) + 1;
        if let Some(line) = nearest(hunks.iter().copied(), cursor, dir) {
            self.goto_line(line);
            return Vec::new();
        }
        // Past the last change, go round to the first; before the first, to the last.
        if dir == Direction::Forward {
            let first = hunks.iter().copied().min().unwrap_or(1);
            self.goto_line(first);
            self.set_flash("wrapped to the first change", false)
        } else {
            let last = hunks.iter().copied().max().unwrap_or(1);
            self.goto_line(last);
            self.set_flash("wrapped to the last change", false)
        }
    }

    /// Moves to the line a jump found, or says there is none in this file.
    fn jump_to(&mut self, line: Option<i64>, dir: Direction, what: &str) -> Vec<Effect> {
        if let Some(l) = line {
            self.goto_line(l);
            return Vec::new();
        }
        let word = if dir == Direction::Forward {
            "next"
        } else {
            "previous"
        };
        self.set_flash(&format!("no {word} {what} in this file"), false)
    }

    /// The :N prompt: digits build the number, enter goes to it.
    fn go_line_key(&mut self, s: &str) {
        match s {
            "esc" => self.go_line = None,
            "enter" => {
                let text = self.go_line.take().unwrap_or_default();
                if let Ok(n) = text.parse::<i64>() {
                    if self.file.is_some() {
                        self.goto_line(n);
                    }
                }
            }
            "backspace" => {
                let empty = self.go_line.as_ref().is_some_and(String::is_empty);
                if empty {
                    self.go_line = None;
                } else if let Some(text) = self.go_line.as_mut() {
                    text.pop();
                }
            }
            _ => {
                if is_digit(s) {
                    if let Some(text) = self.go_line.as_mut() {
                        text.push_str(s);
                    }
                }
            }
        }
    }

    // ── writing ─────────────────────────────────────────────────────────────

    fn start_comment(&mut self) -> Vec<Effect> {
        let Some(f) = self.file.as_ref() else {
            return Vec::new();
        };
        if f.binary {
            return self.set_flash("rvw: a binary file cannot be commented on", true);
        }
        if f.is_empty() {
            return self.set_flash("rvw: an empty file has no line to comment on", true);
        }
        let (start, end) = self.selection();
        let mut context = vec![format!(
            "New comment on {}",
            location(&f.rel, to_i(start), to_i(end))
        )];
        context.extend(quote_code(&f.plain[start - 1..end], to_i(start)));
        let req = self.comment_request();
        self.edit(&template("", &context), req)
    }

    /// A new comment on the selection, carrying the file as the viewer shows
    /// it: the snapshot is what the reviewer read, even if the file changes
    /// while the editor is open.
    fn comment_request(&self) -> EditRequest {
        let (start, end) = self.selection();
        let (file, source) = self
            .file
            .as_ref()
            .map(|f| (f.abs.clone(), f.source.clone()))
            .unwrap_or_default();
        EditRequest {
            kind: EditKind::Add,
            file,
            source: Some(source),
            start: to_i(start),
            end: to_i(end),
            id: String::new(),
            decision: String::new(),
            ids: Vec::new(),
        }
    }

    fn start_edit(&mut self) -> Vec<Effect> {
        let found = self.file.as_ref().and_then(|f| {
            covering(&f.comments, to_i(f.cursor) + 1)
                .last()
                .map(|c| (*c).clone())
        });
        let Some(c) = found else {
            return self.set_flash("no comment on this line", false);
        };
        let mut context = vec![format!("Editing {} on {}", c.id, c.location())];
        context.extend(quote_code(
            &c.code.split('\n').map(str::to_owned).collect::<Vec<_>>(),
            c.start_line,
        ));
        let req = EditRequest {
            kind: EditKind::Edit,
            file: String::new(),
            source: None,
            start: 0,
            end: 0,
            id: c.id.clone(),
            decision: String::new(),
            ids: Vec::new(),
        };
        self.edit(&template(&c.comment, &context), req)
    }

    fn start_summary(&mut self, decision: &str) -> Vec<Effect> {
        let req = self.summary_request(decision);
        let mut context = vec![format!("Review: {decision}")];
        context.push(String::new());
        if req.ids.is_empty() {
            context
                .push("You have no pending comments here: the review is summary-only.".to_owned());
        } else {
            context.push("Links your pending comments:".to_owned());
            for c in self.my_pending() {
                context.push(format!(
                    "  {}  {}  {}",
                    c.id,
                    c.location(),
                    first_line(&c.comment)
                ));
            }
        }
        self.edit(&template("", &context), req)
    }

    /// A review over the pending comments the editor will list, named by id,
    /// so one queued meanwhile is not linked unseen.
    fn summary_request(&self, decision: &str) -> EditRequest {
        let ids = self.my_pending().into_iter().map(|c| c.id).collect();
        EditRequest {
            kind: EditKind::Summary,
            file: String::new(),
            source: None,
            start: 0,
            end: 0,
            id: String::new(),
            decision: decision.to_owned(),
            ids,
        }
    }

    /// Writes the editor's temp file and asks the shell to run the editor on
    /// it; the shell reports back with `Event::EditorDone`.
    fn edit(&mut self, content: &str, req: EditRequest) -> Vec<Effect> {
        match write_temp(content) {
            Ok(path) => vec![Effect::RunEditor { req, path }],
            Err(e) => self.fail(&e.to_string()),
        }
    }

    /// Saves what the user wrote. The note's file goes only once it is saved:
    /// when saving fails, the error says where the note was kept.
    fn editor_done(
        &mut self,
        req: &EditRequest,
        path: &Path,
        error: Option<String>,
    ) -> Vec<Effect> {
        if let Some(err) = error {
            let msg = format!(
                "editor '{}' failed: {} — note kept at {}",
                self.opts.editor,
                err,
                path.display()
            );
            return self.fail(&msg);
        }
        let data = match fs::read(path) {
            Ok(data) => data,
            Err(e) => return self.fail(&e.to_string()),
        };
        let text = strip_template(&String::from_utf8_lossy(&data));
        if text.is_empty() {
            let _ = fs::remove_file(path);
            return self.set_flash("cancelled: empty note", false);
        }
        let flash = match self.save(req, &text) {
            Ok(flash) => flash,
            Err(e) => {
                let msg = format!("{e} — note kept at {}", path.display());
                return self.fail(&msg);
            }
        };
        let _ = fs::remove_file(path);
        if let Err(e) = self.reload(false) {
            return self.fail(&e);
        }
        self.set_flash(&flash, false)
    }

    /// Makes the one service call an edit request stands for, and says what
    /// it did.
    fn save(&mut self, req: &EditRequest, text: &str) -> Result<String, String> {
        let workspace = self.opts.workspace.clone();
        match req.kind {
            EditKind::Add => {
                let c = self
                    .opts
                    .service
                    .add(AddInput {
                        workspace,
                        file: req.file.clone(),
                        start_line: req.start,
                        end_line: req.end,
                        source: req.source.clone(),
                        comment: text.to_owned(),
                        lane: self.opts.lane.clone(),
                        author: self.opts.author.clone(),
                        ..AddInput::default()
                    })
                    .map_err(|e| e.to_string())?;
                self.visual = false;
                Ok(format!("queued {} · {}", c.id, c.location()))
            }
            EditKind::Edit => {
                let c = self
                    .opts
                    .service
                    .edit(EditInput {
                        workspace,
                        id: req.id.clone(),
                        comment: text.to_owned(),
                    })
                    .map_err(|e| e.to_string())?;
                Ok(format!("edited {} · {}", c.id, c.location()))
            }
            EditKind::Summary => {
                let r = self
                    .opts
                    .service
                    .submit(SubmitInput {
                        workspace,
                        decision: req.decision.clone(),
                        summary: text.to_owned(),
                        comment_ids: req.ids.clone(),
                        no_comments: req.ids.is_empty(),
                        lane: self.opts.lane.clone(),
                        author: self.opts.author.clone(),
                    })
                    .map_err(|e| e.to_string())?;
                Ok(format!(
                    "submitted {} · {} · {}",
                    r.id,
                    r.decision,
                    plural(r.comment_ids.len(), "comment")
                ))
            }
        }
    }

    // ── submit and compare menus ────────────────────────────────────────────

    fn submit_key(&mut self, s: &str) -> Vec<Effect> {
        let mut cursor = self.submit.as_ref().map_or(0, |menu| menu.cursor);
        match s {
            "esc" | "q" => self.submit = None,
            "j" | "down" | "ctrl+n" => cursor = (cursor + 1).min(DECISIONS.len() - 1),
            "k" | "up" | "ctrl+p" => cursor = cursor.saturating_sub(1),
            "1" | "2" | "3" => {
                cursor = usize::from(s.as_bytes()[0] - b'1');
                return self.choose_decision(cursor);
            }
            "enter" => return self.choose_decision(cursor),
            _ => {}
        }
        if let Some(menu) = self.submit.as_mut() {
            menu.cursor = cursor;
        }
        Vec::new()
    }

    fn choose_decision(&mut self, cursor: usize) -> Vec<Effect> {
        self.submit = None;
        match DECISIONS.get(cursor).copied() {
            Some(decision) => self.start_summary(decision),
            None => Vec::new(),
        }
    }

    fn compare_key(&mut self, s: &str) -> Vec<Effect> {
        let mut cursor = self.compare.as_ref().map_or(0, |menu| menu.cursor);
        match s {
            "esc" | "q" => self.compare = None,
            "j" | "down" | "ctrl+n" => cursor = (cursor + 1).min(COMPARE_ORDER.len() - 1),
            "k" | "up" | "ctrl+p" => cursor = cursor.saturating_sub(1),
            "1" | "2" | "3" => {
                cursor = usize::from(s.as_bytes()[0] - b'1');
                return self.choose_compare(cursor);
            }
            "enter" => return self.choose_compare(cursor),
            _ => {}
        }
        if let Some(menu) = self.compare.as_mut() {
            menu.cursor = cursor;
        }
        Vec::new()
    }

    fn choose_compare(&mut self, cursor: usize) -> Vec<Effect> {
        self.compare = None;
        let Some(c) = COMPARE_ORDER.get(cursor).copied() else {
            return Vec::new();
        };
        if let Err(e) = self.set_compare(c) {
            return self.fail(&e);
        }
        Vec::new()
    }

    // ── picker ──────────────────────────────────────────────────────────────

    /// Opens the fuzzy finder over the files (recent ones first) or over the
    /// open comments.
    fn open_picker(&mut self, kind: PickerKind) {
        let mut p = Picker {
            kind,
            query: String::new(),
            cands: Vec::new(),
            targets: Vec::new(),
            ids: Vec::new(),
            matches: Vec::new(),
            cursor: 0,
            offset: 0,
        };
        match kind {
            PickerKind::Files => {
                let mut order: Vec<&str> = self
                    .recent
                    .iter()
                    .filter(|f| self.files.contains(*f))
                    .map(String::as_str)
                    .collect();
                order.extend(self.files.iter().map(String::as_str));
                let mut seen: HashSet<&str> = HashSet::new();
                for f in order {
                    if seen.insert(f) {
                        p.cands.push(Candidate {
                            label: f.to_owned(),
                            comments: self.counts.get(f).copied().unwrap_or(0),
                        });
                        p.targets.push(Target {
                            file: f.to_owned(),
                            line: 0,
                        });
                    }
                }
            }
            PickerKind::Comments => {
                let width = self
                    .open
                    .iter()
                    .map(|c| c.location().width())
                    .max()
                    .unwrap_or(0)
                    .min(40);
                for c in &self.open {
                    let loc = c.location();
                    let pad = " ".repeat(width.saturating_sub(loc.width()));
                    p.cands.push(Candidate {
                        label: format!("{loc}{pad}  {}", first_line(&c.comment)),
                        comments: 0,
                    });
                    p.targets.push(Target {
                        file: c.file.clone(),
                        line: c.start_line,
                    });
                    p.ids.push(c.id.clone());
                }
            }
        }
        filter_picker(&mut p);
        self.picker = Some(p);
    }

    fn picker_key(&mut self, name: &str) -> Vec<Effect> {
        match name {
            "esc" => {
                self.picker = None;
                return Vec::new();
            }
            "enter" => return self.choose_picker_row(),
            _ => {}
        }
        let Some(p) = self.picker.as_mut() else {
            return Vec::new();
        };
        match name {
            "ctrl+n" | "down" => p.cursor = (p.cursor + 1).min(p.matches.len().saturating_sub(1)),
            "ctrl+p" | "up" => p.cursor = p.cursor.saturating_sub(1),
            "backspace" => {
                if p.query.pop().is_some() {
                    filter_picker(p);
                }
            }
            "ctrl+u" => {
                p.query.clear();
                filter_picker(p);
            }
            "ctrl+w" => {
                // Deletes back to the previous separator, as readline does.
                let trimmed = p.query.trim_end_matches([' ', '/']);
                let cut = trimmed.rfind([' ', '/']).map_or(0, |k| k + 1);
                let kept = trimmed[..cut].to_owned();
                p.query = kept;
                filter_picker(p);
            }
            _ => {
                if let Some(text) = typed_text(name) {
                    p.query.push_str(text);
                    filter_picker(p);
                }
            }
        }
        Vec::new()
    }

    /// Opens the file of the picker's cursor row and moves to the viewer.
    fn choose_picker_row(&mut self) -> Vec<Effect> {
        let Some(p) = self.picker.take() else {
            return Vec::new();
        };
        let Some(hit) = p.matches.get(p.cursor) else {
            return Vec::new();
        };
        let target = p.targets[hit.index].clone();
        if let Err(e) = self.open_file(&target.file, to_u(target.line)) {
            return self.fail(&e);
        }
        self.focus = Pane::Viewer;
        Vec::new()
    }

    // ── mouse ───────────────────────────────────────────────────────────────

    /// Where a mouse event landed: a pane and the body row under it, 0 at the
    /// top of the pane. `None` off the panes: borders, the bar, overlays.
    fn hit(&self, x: usize, y: usize) -> Option<(Pane, usize)> {
        let (tw, vw, body) = (self.tree_width(), self.view_width(), self.body_height());
        if y < 1 || y > body {
            return None;
        }
        if (1..=tw).contains(&x) {
            return Some((Pane::Tree, y - 1));
        }
        if (tw + 2..=tw + 1 + vw).contains(&x) {
            return Some((Pane::Viewer, y - 1));
        }
        None
    }

    /// A click, a drag, a release or a wheel notch. A click on a file in the
    /// tree opens it, on a directory expands or collapses it; a click in the
    /// viewer moves the cursor, and dragging from there selects lines.
    fn mouse(&mut self, e: MouseEvent) -> Vec<Effect> {
        if self.width < 40
            || self.height < 10
            || self.picker.is_some()
            || self.submit.is_some()
            || self.compare.is_some()
            || self.go_line.is_some()
        {
            return Vec::new();
        }
        if self.help {
            if matches!(e.kind, MouseKind::Click(_)) {
                self.help = false;
            }
            return Vec::new();
        }
        match e.kind {
            MouseKind::Release => {
                self.dragging = false;
                return Vec::new();
            }
            MouseKind::Motion(button) => {
                if self.dragging && button == MouseButton::Left {
                    self.drag(to_i(e.y));
                }
                return Vec::new();
            }
            _ => {}
        }
        let Some((pane, row)) = self.hit(e.x, e.y) else {
            return Vec::new();
        };
        match e.kind {
            MouseKind::WheelUp => self.wheel(pane, -WHEEL_STEP),
            MouseKind::WheelDown => self.wheel(pane, WHEEL_STEP),
            MouseKind::Click(MouseButton::Left) => {
                self.count.clear();
                self.prefix.clear();
                self.leader = false;
                if pane == Pane::Tree {
                    return self.click_tree(row);
                }
                self.click_viewer(row);
            }
            _ => {}
        }
        Vec::new()
    }

    fn click_tree(&mut self, row: usize) -> Vec<Effect> {
        self.focus = Pane::Tree;
        let idx = self.tree_off + row;
        if idx >= self.rows.len() {
            return Vec::new();
        }
        self.tree_cur = idx;
        let id = self.rows[idx].node;
        let (path, is_dir, expanded) = self.node_info(id);
        if self.side == Sidebar::Reviews && path != STANDALONE {
            return self.open_review_row(id);
        }
        if is_dir {
            self.tree_mut().set_expanded(id, !expanded);
            self.refresh_rows();
            return Vec::new();
        }
        self.open_from_tree(&path)
    }

    fn click_viewer(&mut self, row: usize) {
        self.focus = Pane::Viewer;
        if self.detail.is_some() {
            return;
        }
        let Some(f) = self.file.as_mut() else {
            return;
        };
        if f.is_empty() || f.offset + row >= f.len() {
            return;
        }
        f.cursor = f.offset + row;
        self.visual = false;
        self.anchor = f.cursor;
        self.dragging = true;
    }

    /// Moves the cursor to the line under row `y`, selecting from where the
    /// drag started. Past the top or bottom of the pane it scrolls one line.
    fn drag(&mut self, y: i64) {
        if self.detail.is_some() {
            return;
        }
        let body = to_i(self.body_height());
        let Some(f) = self.file.as_mut() else {
            return;
        };
        if f.is_empty() {
            return;
        }
        let offset = to_i(f.offset);
        let mut line = offset + y - 1;
        if y < 1 {
            line = offset - 1;
        } else if y > body {
            line = offset + body;
        }
        f.cursor = to_u(clamp(line, 0, to_i(f.len()) - 1));
        self.visual = f.cursor != self.anchor;
    }

    /// Scrolls the pane by `n` rows, carrying its cursor along when it would
    /// leave the screen.
    fn wheel(&mut self, pane: Pane, n: i64) {
        let body = to_i(self.body_height());
        if pane == Pane::Tree {
            let rows = to_i(self.rows.len());
            let off = clamp(to_i(self.tree_off) + n, 0, (rows - body).max(0));
            self.tree_off = to_u(off);
            let hi = (rows - 1).min(off + body - 1);
            self.tree_cur = to_u(clamp(to_i(self.tree_cur), off, hi));
            return;
        }
        let width = self.view_width();
        if let Some(d) = self.detail.as_mut() {
            let len = to_i(d.layout(width).len());
            d.offset = to_u(clamp(to_i(d.offset) + n, 0, (len - body).max(0)));
            return;
        }
        let Some(f) = self.file.as_mut() else {
            return;
        };
        let len = to_i(f.len());
        if len == 0 {
            return;
        }
        f.offset = to_u(clamp(to_i(f.offset) + n, 0, (len - body).max(0)));
        let lo = to_i(f.offset);
        let hi = (len - 1).min(lo + body - 1);
        f.cursor = to_u(clamp(to_i(f.cursor), lo, hi));
    }
}

// ── helpers ─────────────────────────────────────────────────────────────────

/// Re-ranks the picker's candidates for its query and moves it to the top.
fn filter_picker(p: &mut Picker) {
    p.matches = rank(&p.query, &p.cands);
    p.cursor = 0;
    p.offset = 0;
}

/// The text a key types into the picker's query: a space, or the one
/// character a key name is. `None` for any other key.
fn typed_text(name: &str) -> Option<&str> {
    if name == "space" {
        return Some(" ");
    }
    (name.chars().count() == 1).then_some(name)
}

fn is_digit(s: &str) -> bool {
    s.len() == 1 && s.as_bytes()[0].is_ascii_digit()
}

/// The count a pending {count} stands for, and clears it. 0 when there is none.
fn take_count(text: &mut String) -> i64 {
    let n = text
        .bytes()
        .fold(0_i64, |n, b| (n * 10 + i64::from(b - b'0')).min(MAX_COUNT));
    text.clear();
    n
}

/// The compare menu row a comparison is listed on.
fn compare_index(c: Compare) -> usize {
    COMPARE_ORDER.iter().position(|&o| o == c).unwrap_or(0)
}

/// The number of an item as a signed count.
fn to_i(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

/// A signed count or index as an unsigned one; negatives become 0.
fn to_u(n: i64) -> usize {
    usize::try_from(n.max(0)).unwrap_or(usize::MAX)
}

/// `v` moved by `by`, never below 0.
fn shift(v: usize, by: i64) -> usize {
    to_u(to_i(v).saturating_add(by))
}

/// The first line of `s`, trimmed and ready to draw.
fn first_line(s: &str) -> String {
    let t = s.trim();
    expand_tabs(t.split('\n').next().unwrap_or(""))
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use rvw_core::operations::{GetInput, QueryInput};

    use super::*;
    use crate::testing::{keys, screen_text, setup, Fixture};
    use crate::Options;

    /// The column inside the viewer at 100 columns wide.
    const VIEWER_X: usize = 40;

    fn click(x: usize, y: usize) -> Event {
        Event::Mouse(MouseEvent {
            kind: MouseKind::Click(MouseButton::Left),
            x,
            y,
        })
    }

    fn release(x: usize, y: usize) -> Event {
        Event::Mouse(MouseEvent {
            kind: MouseKind::Release,
            x,
            y,
        })
    }

    fn motion(x: usize, y: usize) -> Event {
        Event::Mouse(MouseEvent {
            kind: MouseKind::Motion(MouseButton::Left),
            x,
            y,
        })
    }

    fn wheel_at(kind: MouseKind) -> Event {
        Event::Mouse(MouseEvent {
            kind,
            x: VIEWER_X,
            y: 5,
        })
    }

    /// The names of the visible tree rows, indented by depth, dirs with `/`.
    fn row_names(m: &Model) -> Vec<String> {
        m.rows
            .iter()
            .map(|r| {
                let n = m.tree().node(r.node);
                let mut s = format!("{}{}", "  ".repeat(r.depth), n.name);
                if n.dir {
                    s.push('/');
                }
                s
            })
            .collect()
    }

    /// The comments the Go tests' `commented` fixture adds.
    fn commented(f: &Fixture) {
        f.add(
            "src/api/parse.py",
            5,
            8,
            "extract this branch into parse_x()",
            "gustavo",
        );
        f.add("src/api/parse.py", 12, 12, "typo", "reviewer");
    }

    fn add_request(file: &str, start: i64, end: i64) -> EditRequest {
        EditRequest {
            kind: EditKind::Add,
            file: file.to_owned(),
            source: None,
            start,
            end,
            id: String::new(),
            decision: String::new(),
            ids: Vec::new(),
        }
    }

    /// Feeds the editor's result back as the shell does: the note is written
    /// to `path`, then the editor is reported done.
    fn finish(m: &mut Model, req: EditRequest, path: &Path, text: &str) {
        fs::write(path, format!("{text}\n{}", template("", &[]))).unwrap();
        m.update(Event::EditorDone {
            req,
            path: path.to_path_buf(),
            error: None,
        });
    }

    fn editor_run(effects: &[Effect]) -> (EditRequest, PathBuf) {
        effects
            .iter()
            .find_map(|e| match e {
                Effect::RunEditor { req, path } => Some((req.clone(), path.clone())),
                _ => None,
            })
            .expect("an editor run")
    }

    fn file_rel(m: &Model) -> String {
        m.file.as_ref().map(|f| f.rel.clone()).unwrap_or_default()
    }

    fn cursor(m: &Model) -> usize {
        m.file.as_ref().map_or(0, |f| f.cursor)
    }

    // ── editor and submit ───────────────────────────────────────────────────

    #[test]
    fn editor_adds_edits_and_submits() {
        let f = setup();
        let mut m = f.model();
        keys(&mut m, &["leader", "p", "apipar", "enter", "4G", "V", "3j"]);
        assert_eq!(m.selection(), (4, 7));
        assert_eq!(file_rel(&m), "src/api/parse.py");

        let abs = m.file.as_ref().unwrap().abs.clone();
        let dir = f.dir.path();

        // An empty note cancels, and its file is removed.
        let empty = dir.join("empty.md");
        finish(&mut m, add_request(&abs, 4, 7), &empty, "");
        assert_eq!(m.flash, "cancelled: empty note");
        assert!(m.file.as_ref().unwrap().comments.is_empty());
        assert!(m.visual, "a cancelled note keeps the selection");
        assert!(!empty.exists());

        finish(
            &mut m,
            add_request(&abs, 4, 7),
            &dir.join("add.md"),
            "extract this branch",
        );
        assert_eq!(m.flash, "queued r1 · src/api/parse.py:4-7");
        assert!(!m.visual);
        let comments = &m.file.as_ref().unwrap().comments;
        assert_eq!(comments.len(), 1);
        assert_eq!(m.counts["src/api/parse.py"], 1);
        assert_eq!((comments[0].start_line, comments[0].end_line), (4, 7));
        assert_eq!(comments[0].author.as_deref(), Some("tester"));
        assert_eq!(comments[0].comment, "extract this branch");

        let edit = EditRequest {
            kind: EditKind::Edit,
            id: "r1".to_owned(),
            ..add_request("", 0, 0)
        };
        finish(&mut m, edit, &dir.join("edit.md"), "extract into parse_x()");
        assert_eq!(m.flash, "edited r1 · src/api/parse.py:4-7");
        assert_eq!(
            m.file.as_ref().unwrap().comments[0].comment,
            "extract into parse_x()"
        );

        // A service error is shown with the note's path, and the note is kept.
        let kept = dir.join("kept.md");
        let bad = EditRequest {
            kind: EditKind::Edit,
            id: "r9".to_owned(),
            ..add_request("", 0, 0)
        };
        finish(&mut m, bad, &kept, "nope");
        assert!(m.flash_err && m.flash.starts_with("rvw: "), "{}", m.flash);
        assert!(m
            .flash
            .ends_with(&format!("note kept at {}", kept.display())));
        assert!(kept.exists());

        let summary = m.summary_request("request-changes");
        finish(&mut m, summary, &dir.join("summary.md"), "fix it");
        assert_eq!(m.flash, "submitted rv1 · request-changes · 1 comment");
        let sheet = f
            .svc
            .sheet(GetInput {
                workspace: f.ws.clone(),
                id: "rv1".to_owned(),
            })
            .unwrap();
        assert_eq!(sheet.review.comment_ids, ["r1"]);
        assert_eq!(sheet.review.decision, "request-changes");
    }

    #[test]
    fn c_asks_the_shell_for_an_editor_on_a_quoted_template() {
        let f = setup();
        let mut m = f.model();
        keys(&mut m, &["leader", "p", "apipar", "enter", "4G", "V", "3j"]);
        let effects = keys(&mut m, &["c"]);
        let (req, path) = editor_run(&effects);
        assert_eq!(req.kind, EditKind::Add);
        assert_eq!((req.start, req.end), (4, 7));
        let text = fs::read_to_string(&path).unwrap();
        assert!(
            text.starts_with(&format!("\n\n{}\n", crate::editor::SCISSORS)),
            "an empty body, a blank line, then the scissors: {text:?}"
        );
        assert!(
            text.contains("# New comment on src/api/parse.py:4-7\n"),
            "{text}"
        );
        assert!(
            text.contains("#   4  "),
            "the quoted lines are numbered: {text}"
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_failed_editor_keeps_the_note() {
        let f = setup();
        let mut m = f.model();
        let path = f.dir.path().join("failed.md");
        fs::write(&path, "half a note").unwrap();
        let req = add_request(&m.file.as_ref().unwrap().abs.clone(), 1, 1);
        m.update(Event::EditorDone {
            req,
            path: path.clone(),
            error: Some("exit status 1".to_owned()),
        });
        assert!(m.flash_err);
        assert_eq!(
            m.flash,
            format!(
                "rvw: editor 'true' failed: exit status 1 — note kept at {}",
                path.display()
            )
        );
        assert!(path.exists());
    }

    // The comment snapshots the lines the reviewer chose, as the viewer
    // showed them, even when the file changes while the editor is open.
    #[test]
    fn a_comment_keeps_the_code_the_viewer_showed() {
        let f = setup();
        let mut m = f.model();
        keys(&mut m, &["leader", "p", "apipar", "enter", "4G", "V", "j"]);
        let want = m.file.as_ref().unwrap().plain[3..5].join("\n");
        let req = m.comment_request();

        let abs = m.file.as_ref().unwrap().abs.clone();
        fs::write(&abs, "rewritten\nby\nan\nagent\n").unwrap();
        let path = f.dir.path().join("snap.md");
        fs::write(&path, "look here\n").unwrap();
        m.update(Event::EditorDone {
            req,
            path,
            error: None,
        });

        let out = f
            .svc
            .list(QueryInput {
                workspace: f.ws.clone(),
                ..QueryInput::default()
            })
            .unwrap();
        assert_eq!(out.comments.len(), 1, "flash {:?}", m.flash);
        let c = &out.comments[0];
        assert_eq!(c.code, want);
        assert_eq!((c.start_line, c.end_line), (4, 5));
    }

    // A review links the comments its summary's editor listed, not whatever
    // else the same author queued in the meantime.
    #[test]
    fn a_submit_links_only_the_comments_it_listed() {
        let f = setup();
        f.add("src/api/parse.py", 1, 1, "listed", "tester");
        let mut m = f.model();
        let req = m.summary_request("comment");
        f.add(
            "src/api/parse.py",
            2,
            2,
            "added while the editor was open",
            "tester",
        );
        finish(&mut m, req, &f.dir.path().join("link.md"), "Look at r1.");
        let sheet = f
            .svc
            .sheet(GetInput {
                workspace: f.ws.clone(),
                id: "rv1".to_owned(),
            })
            .unwrap();
        assert_eq!(sheet.review.comment_ids, ["r1"], "flash {:?}", m.flash);
    }

    #[test]
    fn s_opens_the_submit_menu_and_a_number_picks_a_decision() {
        let f = setup();
        f.add("src/api/parse.py", 1, 1, "listed", "tester");
        let mut m = f.model();
        keys(&mut m, &["s"]);
        assert!(m.submit.is_some());
        let effects = keys(&mut m, &["2"]);
        assert!(m.submit.is_none());
        let (req, path) = editor_run(&effects);
        assert_eq!(req.kind, EditKind::Summary);
        assert_eq!(req.decision, "approve");
        assert_eq!(req.ids, ["r1"]);
        fs::remove_file(path).unwrap();

        keys(&mut m, &["s", "esc"]);
        assert!(m.submit.is_none());
    }

    #[test]
    fn b_opens_the_compare_menu_and_a_failed_compare_keeps_the_base() {
        let f = setup();
        let mut m = f.model();
        keys(&mut m, &["b"]);
        assert!(m.compare.is_some());
        keys(&mut m, &["esc"]);
        assert!(m.compare.is_none());

        // There is one commit, so the previous commit cannot be compared with.
        keys(&mut m, &["b", "3"]);
        assert!(m.compare.is_none());
        assert_eq!(m.cmp, Compare::Head);
        assert!(m.flash_err && m.flash.starts_with("rvw: "), "{}", m.flash);
    }

    // ── tree ────────────────────────────────────────────────────────────────

    #[test]
    fn clicking_the_tree_toggles_directories_and_opens_files() {
        let f = setup();
        let mut m = f.model();
        assert_eq!(row_names(&m), ["docs/", "src/", "test/", "README.md"]);

        m.update(click(3, 2));
        assert_eq!(
            row_names(&m),
            ["docs/", "src/", "  api/", "  util.py", "test/", "README.md"]
        );
        assert_eq!(m.focus, Pane::Tree);
        assert_eq!(m.tree_cur, 1);

        m.update(click(3, 4));
        assert_eq!(file_rel(&m), "src/util.py");
        assert_eq!(m.focus, Pane::Viewer);

        m.update(click(3, 2));
        assert_eq!(row_names(&m), ["docs/", "src/", "test/", "README.md"]);

        m.update(click(3, 20));
        assert_eq!(file_rel(&m), "src/util.py");
        assert_eq!(
            m.focus,
            Pane::Tree,
            "a click below the rows only focuses the tree"
        );
    }

    #[test]
    fn tree_keys_move_expand_collapse_and_open() {
        let f = setup();
        let mut m = f.model();
        // The tree starts on README.md, the file open at start.
        keys(&mut m, &["tab", "k", "k", "enter"]);
        assert_eq!(m.focus, Pane::Tree);
        assert_eq!(
            row_names(&m),
            ["docs/", "src/", "  api/", "  util.py", "test/", "README.md"]
        );
        assert_eq!(
            m.tree_cur, 1,
            "enter on src/ expands it; the cursor stays on it"
        );

        keys(&mut m, &["j", "l"]);
        assert_eq!(
            row_names(&m),
            [
                "docs/",
                "src/",
                "  api/",
                "    parse.py",
                "  util.py",
                "test/",
                "README.md"
            ]
        );
        keys(&mut m, &["j", "enter"]);
        assert_eq!(file_rel(&m), "src/api/parse.py");
        assert_eq!(
            m.focus,
            Pane::Viewer,
            "enter on a file opens it in the viewer"
        );

        keys(&mut m, &["tab", "h"]);
        assert_eq!(m.tree_cur, 2, "h on a file moves to its directory, api/");
    }

    // ── viewer and counts ───────────────────────────────────────────────────

    #[test]
    fn counts_and_prefixes_move_the_cursor() {
        let f = setup();
        let mut m = f.model();
        keys(&mut m, &["leader", "p", "parse.py", "enter"]);
        keys(&mut m, &["3", "j"]);
        assert_eq!(cursor(&m), 3);
        keys(&mut m, &["2", "k"]);
        assert_eq!(cursor(&m), 1);
        keys(&mut m, &["5", "G"]);
        assert_eq!(cursor(&m), 4, "5G goes to line 5");
        keys(&mut m, &["3", "g", "g"]);
        assert_eq!(cursor(&m), 2, "3gg goes to line 3");
        keys(&mut m, &["G"]);
        assert_eq!(cursor(&m), 11, "G goes to the last line");
        keys(&mut m, &["g", "g"]);
        assert_eq!(cursor(&m), 0);
        keys(&mut m, &[":", "9", "enter"]);
        assert_eq!(cursor(&m), 8, ":9 goes to line 9");
        keys(&mut m, &[":", "backspace", "backspace", "esc"]);
        assert!(m.go_line.is_none());
    }

    #[test]
    fn reload_picks_up_comments_added_elsewhere() {
        let f = setup();
        let mut m = f.model();
        keys(&mut m, &["leader", "p", "parse.py", "enter"]);
        f.add("src/api/parse.py", 10, 10, "from another shell", "someone");
        assert!(
            m.file.as_ref().unwrap().comments.is_empty(),
            "nothing is watched"
        );
        keys(&mut m, &["r"]);
        assert_eq!(m.file.as_ref().unwrap().comments.len(), 1);
        assert_eq!(m.counts["src/api/parse.py"], 1);
        keys(&mut m, &["]", "c"]);
        assert_eq!(cursor(&m), 9, "]c goes to line 10");
    }

    #[test]
    fn jumps_report_when_there_is_nothing_to_jump_to() {
        let f = setup();
        let mut m = f.model();
        keys(&mut m, &["leader", "p", "parse.py", "enter", "]", "c"]);
        assert_eq!(m.flash, "no next comment in this file");
        keys(&mut m, &["[", "c"]);
        assert_eq!(m.flash, "no previous comment in this file");
        keys(&mut m, &["]", "h"]);
        assert_eq!(m.flash, "no changes in this file");
    }

    #[test]
    fn hunk_jumps_wrap_round_with_a_notice() {
        let f = setup();
        f.write("src/util.py", "def util():\n    return 1\n");
        let mut m = f.model();
        keys(&mut m, &["leader", "p", "util", "enter"]);
        assert_eq!(cursor(&m), 0);
        keys(&mut m, &["]", "h"]);
        assert_eq!(cursor(&m), 1, "the change is on line 2");
        keys(&mut m, &["]", "h"]);
        assert_eq!(m.flash, "wrapped to the first change");
        assert_eq!(cursor(&m), 1);
        keys(&mut m, &["[", "h"]);
        assert_eq!(m.flash, "wrapped to the last change");
        assert_eq!(cursor(&m), 1);
    }

    #[test]
    fn opening_a_file_remembers_where_it_was_left() {
        let f = setup();
        let mut m = f.model();
        keys(
            &mut m,
            &[
                "leader", "p", "parse.py", "enter", "9G", "leader", "p", "util", "enter",
            ],
        );
        assert_eq!(file_rel(&m), "src/util.py");
        assert_eq!(cursor(&m), 0, "a new file opens on line 1");

        keys(&mut m, &["leader", "p"]);
        let first = m.picker.as_ref().map(|p| p.matches[0].index).unwrap();
        assert_eq!(
            m.picker.as_ref().unwrap().cands[first].label,
            "src/util.py",
            "an empty query lists recent files first"
        );
        keys(&mut m, &["down", "enter"]);
        assert_eq!(file_rel(&m), "src/api/parse.py");
        assert_eq!(cursor(&m), 8, "reopened on line 9");
    }

    #[test]
    fn binary_files_cannot_be_commented_on() {
        let f = setup();
        f.write("blob.bin", "a\x00b");
        let mut m = f.model();
        keys(&mut m, &["leader", "p", "blob", "enter", "c"]);
        assert!(m.file.as_ref().unwrap().binary);
        assert!(m.flash_err);
        assert_eq!(m.flash, "rvw: a binary file cannot be commented on");
        assert!(screen_text(&m).contains("binary file"));
    }

    #[test]
    fn e_without_a_comment_on_the_line_says_so() {
        let f = setup();
        let mut m = f.model();
        keys(&mut m, &["leader", "p", "parse.py", "enter", "e"]);
        assert_eq!(m.flash, "no comment on this line");
        assert!(!m.flash_err);
    }

    #[test]
    fn e_edits_the_comment_on_the_line() {
        let f = setup();
        commented(&f);
        let mut m = f.model();
        keys(&mut m, &["leader", "p", "parse.py", "enter", "5G"]);
        let effects = keys(&mut m, &["e"]);
        let (req, path) = editor_run(&effects);
        assert_eq!(req.kind, EditKind::Edit);
        assert_eq!(req.id, "r1");
        let text = fs::read_to_string(&path).unwrap();
        assert!(
            text.starts_with("extract this branch into parse_x()\n"),
            "{text}"
        );
        fs::remove_file(path).unwrap();
    }

    // ── leader and pickers ──────────────────────────────────────────────────

    #[test]
    fn leader_mappings_open_the_pickers() {
        let f = setup();
        commented(&f);
        let mut m = f.model();
        keys(&mut m, &["leader"]);
        assert!(m.leader);
        assert!(screen_text(&m).contains("<leader>  p go to file"));

        keys(&mut m, &["p"]);
        assert!(!m.leader);
        assert_eq!(m.picker.as_ref().unwrap().kind, PickerKind::Files);

        keys(&mut m, &["esc", "leader", "l"]);
        assert_eq!(m.picker.as_ref().unwrap().kind, PickerKind::Comments);
        keys(&mut m, &["esc"]);

        keys(&mut m, &["leader", "j"]);
        assert!(!m.leader && m.picker.is_none());
        assert_eq!(cursor(&m), 0, "another key after the leader does nothing");

        keys(&mut m, &["3", "leader", "p", "esc", "j"]);
        assert_eq!(cursor(&m), 1, "the leader drops a pending count");

        m.update(Event::Key("ctrl+p".to_owned()));
        m.update(Event::Key("ctrl+l".to_owned()));
        assert!(
            m.picker.is_none(),
            "C-p and C-l are left to tmux and the terminal"
        );
    }

    #[test]
    fn the_picker_filters_edits_its_query_and_opens_the_row() {
        let f = setup();
        let mut m = f.model();
        keys(&mut m, &["leader", "p", "zzz"]);
        assert!(m.picker.as_ref().unwrap().matches.is_empty());
        keys(
            &mut m,
            &["backspace", "backspace", "backspace", "u", "t", "il"],
        );
        assert_eq!(m.picker.as_ref().unwrap().query, "util");
        keys(&mut m, &["ctrl+u", "space"]);
        assert_eq!(m.picker.as_ref().unwrap().query, " ");
        keys(&mut m, &["ctrl+u", "src/api", "ctrl+w"]);
        assert_eq!(m.picker.as_ref().unwrap().query, "src/");
        keys(&mut m, &["ctrl+u", "util", "enter"]);
        assert!(m.picker.is_none());
        assert_eq!(file_rel(&m), "src/util.py");
        assert_eq!(m.focus, Pane::Viewer);
    }

    #[test]
    fn leader_key_is_configurable() {
        let f = setup();
        commented(&f);
        let mut m = Model::new(Options {
            service: Arc::clone(&f.svc),
            workspace: f.ws.clone(),
            lane: String::new(),
            author: "tester".to_owned(),
            editor: "true".to_owned(),
            style: String::new(),
            leader: ",".to_owned(),
        })
        .expect("model");
        m.update(Event::Resize {
            width: 100,
            height: 30,
        });
        keys(&mut m, &["leader", "p"]);
        assert!(m.picker.is_none(), "space is no leader when another key is");
        keys(&mut m, &[",", "p"]);
        assert_eq!(m.picker.as_ref().unwrap().kind, PickerKind::Files);
        keys(&mut m, &["esc", "?"]);
        assert!(screen_text(&m).contains("<leader> is ,"));
    }

    // ── mouse ───────────────────────────────────────────────────────────────

    #[test]
    fn dragging_in_the_viewer_selects_lines() {
        let f = setup();
        let mut m = f.model();
        keys(&mut m, &["leader", "p", "parse.py", "enter"]);
        m.update(click(VIEWER_X, 3));
        m.update(release(VIEWER_X, 3));
        assert_eq!(cursor(&m), 2);
        assert!(!m.visual, "a click moves the cursor to line 3");

        m.update(click(VIEWER_X, 8));
        m.update(motion(VIEWER_X, 5));
        m.update(release(VIEWER_X, 5));
        assert!(m.visual);
        assert_eq!(
            m.selection(),
            (5, 8),
            "dragging up from line 8 to 5 selects 5-8"
        );

        m.update(motion(VIEWER_X, 10));
        assert_eq!(
            m.selection(),
            (5, 8),
            "motion after the release changes nothing"
        );
        assert!(screen_text(&m).contains("5-8 (4 lines)"));

        m.update(click(VIEWER_X, 2));
        m.update(motion(VIEWER_X, 29));
        assert!(m.visual);
        assert_eq!(
            cursor(&m),
            m.file.as_ref().unwrap().len() - 1,
            "past the bottom stops at the last line"
        );
    }

    #[test]
    fn wheel_scrolls_the_pane_under_the_pointer() {
        let f = setup();
        f.write("long.txt", &"x\n".repeat(100));
        let mut m = f.model();
        keys(&mut m, &["leader", "p", "long.txt", "enter"]);
        m.update(wheel_at(MouseKind::WheelDown));
        m.update(wheel_at(MouseKind::WheelDown));
        let file = m.file.as_ref().unwrap();
        assert_eq!(
            (file.offset, file.cursor),
            (6, 6),
            "two notches scroll 6 lines"
        );

        m.update(wheel_at(MouseKind::WheelUp));
        let file = m.file.as_ref().unwrap();
        assert_eq!(
            (file.offset, file.cursor),
            (3, 6),
            "scrolling back leaves the cursor"
        );
    }

    #[test]
    fn a_click_on_the_help_overlay_closes_it() {
        let f = setup();
        let mut m = f.model();
        keys(&mut m, &["?"]);
        assert!(m.help);
        m.update(click(VIEWER_X, 3));
        assert!(!m.help);
    }

    // ── flash, resize, background ───────────────────────────────────────────

    #[test]
    fn a_flash_clears_only_when_its_timer_is_the_latest() {
        let f = setup();
        let mut m = f.model();
        let effects = m.set_flash("first", false);
        let Some(Effect::FlashTimer { seq: first }) = effects.first().cloned() else {
            panic!("a flash starts a timer");
        };
        m.set_flash("second", false);
        m.update(Event::FlashDone(first));
        assert_eq!(m.flash, "second", "an older timer leaves a newer flash");
        m.update(Event::FlashDone(m.flash_seq));
        assert!(m.flash.is_empty());
    }

    #[test]
    fn a_background_report_picks_the_theme() {
        let f = setup();
        let mut m = f.model();
        m.update(Event::Background { dark: false });
        assert_eq!(m.theme.cursor, ratatui::style::Color::Indexed(254));
        m.update(Event::Background { dark: true });
        assert_eq!(m.theme.cursor, ratatui::style::Color::Indexed(236));
    }

    #[test]
    fn q_and_ctrl_c_quit() {
        let f = setup();
        let mut m = f.model();
        assert!(matches!(keys(&mut m, &["q"]).as_slice(), [Effect::Quit]));
        assert!(matches!(
            keys(&mut m, &["ctrl+c"]).as_slice(),
            [Effect::Quit]
        ));
    }
}
