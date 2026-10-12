//! The UI's state and everything that loads or changes it without an event:
//! port of the `model` struct, loading, side/compare handling and scrolling of
//! internal/tui/tui.go, plus the model parts of reviews.go.
//!
//! Architecture (an Elm-style port of the Bubble Tea program):
//! - `Model` is all state; `Model::update(Event) -> Vec<Effect>` (update.rs)
//!   changes it and says what the shell must do; `Model::screen()` (view.rs)
//!   draws it as styled lines. Neither touches the terminal.
//! - Keys arrive as Bubble Tea key names ("j", "down", "ctrl+d", "enter",
//!   "esc", "tab", "backspace", "pgdown", "space", single characters), so the
//!   Go key tables port one to one.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use rvw_core::operations::{Evidence, GetInput, QueryInput, ReviewSheet, SheetsInput};
use rvw_core::types::{Comment, Status};
use rvw_git::gitx;

use crate::changes::{change_tree, file_changes, Compare};
use crate::fuzzy::{Candidate, Ranked};
use crate::highlight::{highlight, is_binary, source_lines, Segment};
use crate::pages::{evidence_page, sheet_page, short_location, state_mark, status_mark};
use crate::style::{self, piece, Piece, Sign};
use crate::tree::{build_tree, list_files, NodeId, Row, Tree, ROOT};
use crate::Options;

/// Path of the reviews side's node that groups comments no review links.
pub const STANDALONE: &str = "(standalone)";

/// How long a transient message holds the bottom bar.
pub const FLASH_FOR_MS: u64 = 3000;

/// Which pane has focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Tree,
    Viewer,
}

/// What the left pane lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sidebar {
    /// Every file in the workspace.
    Files,
    /// The files that differ from the compared commit.
    Changes,
    /// Submitted reviews and comments, resolved or not.
    Reviews,
}

impl Sidebar {
    /// The next side in the `t` cycle.
    #[must_use]
    pub fn next(self) -> Sidebar {
        match self {
            Sidebar::Files => Sidebar::Changes,
            Sidebar::Changes => Sidebar::Reviews,
            Sidebar::Reviews => Sidebar::Files,
        }
    }
}

/// The uncommitted changes since HEAD, for the status line.
#[derive(Debug, Clone, Default)]
pub struct WdChanges {
    pub added: i64,
    pub deleted: i64,
    /// Any file differs, even one whose change has no lines.
    pub dirty: bool,
    /// Why there are no counts (empty when there are).
    pub err: String,
}

/// The open file. Lines are 0-indexed here, 1-indexed on screen.
#[derive(Debug, Clone, Default)]
pub struct FileView {
    pub rel: String,
    pub abs: String,
    pub lines: Vec<Vec<Segment>>,
    pub plain: Vec<String>,
    /// The text as read, for a comment's snapshot.
    pub source: String,
    pub binary: bool,
    /// Open comments on this file, oldest first.
    pub comments: Vec<Comment>,
    /// Git change per line; empty for none.
    pub signs: Vec<Sign>,
    /// The (1-indexed) line each git hunk starts on.
    pub hunks: Vec<usize>,
    pub cursor: usize,
    pub offset: usize,
}

impl FileView {
    /// Number of lines (0 for a binary file).
    #[must_use]
    pub fn len(&self) -> usize {
        if self.binary {
            0
        } else {
            self.lines.len()
        }
    }
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A read-only page in the viewer about a review or a comment.
#[derive(Debug, Clone, Default)]
pub struct Detail {
    pub id: String,
    pub sheet: Option<ReviewSheet>,
    pub ev: Option<Evidence>,
    pub offset: usize,
    /// The width `lines` was laid out for.
    pub width: usize,
    /// Laid out lazily, per width.
    pub lines: Vec<Vec<Piece>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerKind {
    Files,
    Comments,
}

/// Where choosing a picker row goes.
#[derive(Debug, Clone, Default)]
pub struct Target {
    pub file: String,
    /// 0: where the file was left.
    pub line: i64,
}

/// The fuzzy-finder overlay: quick open over files, or the open comments.
#[derive(Debug, Clone)]
pub struct Picker {
    pub kind: PickerKind,
    pub query: String,
    pub cands: Vec<Candidate>,
    pub targets: Vec<Target>,
    /// Comment ids, shown before the label.
    pub ids: Vec<String>,
    pub matches: Vec<Ranked>,
    pub cursor: usize,
    pub offset: usize,
}

#[derive(Debug, Clone, Default)]
pub struct SubmitMenu {
    pub cursor: usize,
}

#[derive(Debug, Clone, Default)]
pub struct CompareMenu {
    pub cursor: usize,
}

/// What the editor was opened for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditKind {
    Add,
    Edit,
    Summary,
}

#[derive(Debug, Clone)]
pub struct EditRequest {
    pub kind: EditKind,
    /// Add: absolute path.
    pub file: String,
    /// Add: the file as the viewer showed it.
    pub source: Option<String>,
    pub start: i64,
    pub end: i64,
    /// Edit: the comment id.
    pub id: String,
    /// Summary: comment, approve or request-changes.
    pub decision: String,
    /// Summary: the comments it links.
    pub ids: Vec<String>,
}

/// Colors that depend on the terminal's background.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub cursor: ratatui::style::Color,
    pub selection: ratatui::style::Color,
}

impl Theme {
    /// Go's `newTheme`: cursor 254/236 and selection 153/24 for light/dark.
    #[must_use]
    pub fn new(dark: bool) -> Theme {
        use ratatui::style::Color::Indexed;
        if dark {
            Theme {
                cursor: Indexed(236),
                selection: Indexed(24),
            }
        } else {
            Theme {
                cursor: Indexed(254),
                selection: Indexed(153),
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseKind {
    Click(MouseButton),
    Release,
    Motion(MouseButton),
    WheelUp,
    WheelDown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseEvent {
    pub kind: MouseKind,
    pub x: usize,
    pub y: usize,
}

/// Something that happened to the program.
#[derive(Debug, Clone)]
pub enum Event {
    Resize {
        width: usize,
        height: usize,
    },
    /// A key press, named the way Bubble Tea names keys. A one-character name
    /// is that character typed (it is the key's text); the name `space` is
    /// the space key, whose text is " ".
    Key(String),
    Mouse(MouseEvent),
    /// The 3 s flash timer for `seq` fired.
    FlashDone(u64),
    /// The editor exited. `error` is its failure text, if any.
    EditorDone {
        req: EditRequest,
        path: PathBuf,
        error: Option<String>,
    },
    /// The terminal reported whether its background is dark.
    Background {
        dark: bool,
    },
}

/// What the shell must do after an update.
#[derive(Debug, Clone)]
pub enum Effect {
    Quit,
    /// Fire `Event::FlashDone(seq)` after [`FLASH_FOR_MS`].
    FlashTimer {
        seq: u64,
    },
    /// Suspend the UI, run the editor on `path`, then send `Event::EditorDone`.
    RunEditor {
        req: EditRequest,
        path: PathBuf,
    },
}

/// All of the UI's state.
#[allow(clippy::struct_excessive_bools)] // independent UI flags, one per mode or key prefix
pub struct Model {
    pub opts: Options,
    pub theme: Theme,

    pub width: usize,
    pub height: usize,
    pub focus: Pane,

    pub side: Sidebar,
    pub file_tree: Tree,
    /// `None` until the changes view is first shown.
    pub chg_tree: Option<Tree>,
    /// `None` until the reviews side is first shown.
    pub rev_tree: Option<Tree>,
    /// The reviews side's reviews by id.
    pub sheets: HashMap<String, ReviewSheet>,
    /// And its comments by id.
    pub rev_comments: HashMap<String, Comment>,
    /// Git status letter per changed file.
    pub status: HashMap<String, u8>,
    /// Why the changes cannot be listed, if they cannot.
    pub chg_err: String,
    /// Visible rows of the shown tree.
    pub rows: Vec<Row>,
    pub tree_cur: usize,
    pub tree_off: usize,

    /// The checked-out branch, for the status line.
    pub branch: String,
    /// Uncommitted changes, for the status line.
    pub wd: WdChanges,

    pub cmp: Compare,
    /// The revision `cmp` resolved to; empty for none (no gutter signs).
    pub base_id: String,
    /// Its short name: HEAD, main, HEAD~1.
    pub base_name: String,

    /// Every file, relative.
    pub files: Vec<String>,
    /// Every open comment in the workspace.
    pub open: Vec<Comment>,
    /// Open comments per file.
    pub counts: HashMap<String, usize>,

    pub file: Option<FileView>,
    /// A review or comment page shown over the file, if any.
    pub detail: Option<Detail>,
    /// Last cursor line per file this session.
    pub positions: HashMap<String, usize>,
    /// Files opened this session, most recent first.
    pub recent: Vec<String>,

    pub visual: bool,
    pub anchor: usize,
    /// The left button went down in the viewer and is held.
    pub dragging: bool,
    /// A pending {count}.
    pub count: String,
    /// The leader key was pressed; the next key completes it.
    pub leader: bool,
    /// A pending g, ] or [.
    pub prefix: String,
    /// The count before a prefix, for {count}gg.
    pub prefix_count: usize,
    /// The :N prompt, when open.
    pub go_line: Option<String>,

    pub picker: Option<Picker>,
    pub submit: Option<SubmitMenu>,
    pub compare: Option<CompareMenu>,
    pub help: bool,

    pub flash: String,
    pub flash_err: bool,
    pub flash_seq: u64,
}

// ── loading and state changes (owned by the model agent) ─────────────────────

/// `v` clamped into `0..n`: 0 when there is nothing to point at.
fn clamp_index(v: usize, n: usize) -> usize {
    if n == 0 {
        0
    } else {
        v.min(n - 1)
    }
}

/// The lines of a text file, for the status line; a binary or unreadable file
/// has none, and a last line without a newline still counts.
fn text_lines(path: &Path) -> i64 {
    let Ok(data) = fs::read(path) else {
        return 0;
    };
    if data.is_empty() || is_binary(&data) {
        return 0;
    }
    // A split on newlines has one more piece than there are newlines.
    let mut n = data.split(|&b| b == b'\n').count() - 1;
    if data.last() != Some(&b'\n') {
        n += 1;
    }
    i64::try_from(n).unwrap_or(i64::MAX)
}

impl Model {
    /// Builds the model: resolves the compare base, loads status, tree and
    /// comments, and opens the first file (or focuses the tree when the
    /// workspace has none). Errors are the text after `rvw: `.
    pub fn new(opts: Options) -> Result<Model, String> {
        let mut m = Model {
            opts,
            theme: Theme::new(true),
            width: 0,
            height: 0,
            focus: Pane::Viewer,
            side: Sidebar::Files,
            file_tree: build_tree::<&str>(&[]),
            chg_tree: None,
            rev_tree: None,
            sheets: HashMap::new(),
            rev_comments: HashMap::new(),
            status: HashMap::new(),
            chg_err: String::new(),
            rows: Vec::new(),
            tree_cur: 0,
            tree_off: 0,
            branch: String::new(),
            wd: WdChanges::default(),
            cmp: Compare::Head,
            base_id: String::new(),
            base_name: String::new(),
            files: Vec::new(),
            open: Vec::new(),
            counts: HashMap::new(),
            file: None,
            detail: None,
            positions: HashMap::new(),
            recent: Vec::new(),
            visual: false,
            anchor: 0,
            dragging: false,
            count: String::new(),
            leader: false,
            prefix: String::new(),
            prefix_count: 0,
            go_line: None,
            picker: None,
            submit: None,
            compare: None,
            help: false,
            flash: String::new(),
            flash_err: false,
            flash_seq: 0,
        };
        m.resolve_base();
        m.load_status();
        m.load_tree()?;
        m.load_comments()?;
        if let Some(first) = m.file_tree.first_file().map(str::to_owned) {
            if let Err(e) = m.open_file(&first, 0) {
                m.flash = format!("rvw: {e}");
                m.flash_err = true;
            }
        } else {
            m.focus = Pane::Tree;
        }
        Ok(m)
    }

    /// Resolves the compare choice to `base_id` and `base_name`; both are
    /// empty when it cannot be resolved.
    fn resolve_base(&mut self) {
        if let Ok(b) = self.cmp.base(Path::new(&self.opts.workspace)) {
            self.base_id = b.id;
            self.base_name = b.name;
        } else {
            self.base_id.clear();
            self.base_name.clear();
        }
    }

    /// The tree the left pane shows.
    #[must_use]
    pub fn tree(&self) -> &Tree {
        match self.side {
            Sidebar::Changes => self.chg_tree.as_ref().unwrap_or(&self.file_tree),
            Sidebar::Reviews => self.rev_tree.as_ref().unwrap_or(&self.file_tree),
            Sidebar::Files => &self.file_tree,
        }
    }

    /// The same, mutably.
    pub fn tree_mut(&mut self) -> &mut Tree {
        match self.side {
            Sidebar::Changes => self.chg_tree.as_mut().unwrap_or(&mut self.file_tree),
            Sidebar::Reviews => self.rev_tree.as_mut().unwrap_or(&mut self.file_tree),
            Sidebar::Files => &mut self.file_tree,
        }
    }

    /// Re-lists the workspace, keeping expanded directories expanded.
    pub fn load_tree(&mut self) -> Result<(), String> {
        let ws = self.opts.workspace.clone();
        let files = list_files(Path::new(&ws))
            .map_err(|e| format!("cannot list workspace {ws}: {}", e.stderr))?;
        let expanded = self.file_tree.expanded_dirs();
        let mut tree = build_tree(&files);
        for d in &expanded {
            if let Some(n) = tree.find(d).filter(|&n| tree.node(n).dir) {
                tree.set_expanded(n, true);
            }
        }
        if let Some(f) = &self.file {
            tree.reveal(&f.rel);
        }
        self.files = files;
        self.file_tree = tree;
        self.refresh_rows();
        Ok(())
    }

    /// Lists the files that differ from the compared commit; a failure is kept
    /// in `chg_err`, not returned.
    pub fn load_changes(&mut self) {
        let dir = PathBuf::from(&self.opts.workspace);
        let mut err: Option<String> = None;
        if self.base_id.is_empty() {
            // Perhaps there is one by now; else the error says why not.
            match self.cmp.base(&dir) {
                Ok(b) => {
                    self.base_id = b.id;
                    self.base_name = b.name;
                }
                Err(e) => err = Some(e),
            }
        }
        let mut changes = Vec::new();
        if err.is_none() {
            match gitx::changed_files(&dir, &self.base_id) {
                Ok(c) => changes = c,
                Err(e) => err = Some(e.stderr),
            }
        }
        let (tree, status) = change_tree(&changes);
        self.chg_err = err.unwrap_or_default();
        self.chg_tree = Some(tree);
        self.status = status.into_iter().collect();
        self.refresh_rows();
    }

    /// Reads the branch and the uncommitted changes for the status line.
    pub fn load_status(&mut self) {
        let dir = PathBuf::from(&self.opts.workspace);
        self.branch = gitx::branch(&dir);
        self.wd = WdChanges::default();
        if gitx::commit(&dir, "HEAD").is_none() {
            "no commits yet".clone_into(&mut self.wd.err);
            return;
        }
        let changes = match gitx::changed_files(&dir, "HEAD") {
            Ok(c) => c,
            Err(e) => {
                self.wd = WdChanges {
                    err: e.stderr,
                    ..WdChanges::default()
                };
                return;
            }
        };
        match gitx::line_changes(&dir, "HEAD") {
            Ok((added, deleted)) => {
                self.wd.added = added;
                self.wd.deleted = deleted;
            }
            Err(e) => {
                self.wd = WdChanges {
                    err: e.stderr,
                    ..WdChanges::default()
                };
                return;
            }
        }
        self.wd.dirty = !changes.is_empty();
        for c in changes.iter().filter(|c| c.status == b'?') {
            self.wd.added += text_lines(&dir.join(&c.path));
        }
    }

    /// Switches the left pane to `side`, re-listing changes or reviews, and
    /// puts the tree cursor on what the viewer shows.
    pub fn show_side(&mut self, side: Sidebar) -> Result<(), String> {
        self.side = side;
        self.load_status();
        let mut result = Ok(());
        match side {
            Sidebar::Files => {}
            Sidebar::Changes => self.load_changes(),
            Sidebar::Reviews => result = self.load_reviews(),
        }
        self.refresh_rows();
        self.tree_cur = 0;
        self.tree_off = 0;
        self.select_shown();
        result
    }

    /// Puts the tree cursor on the row of what the viewer shows: the review or
    /// comment on the reviews side, else the open file.
    pub fn select_shown(&mut self) {
        if self.side == Sidebar::Reviews {
            if let Some(id) = self.detail.as_ref().map(|d| d.id.clone()) {
                self.select_tree_row(&id);
            }
        } else if let Some(rel) = self.file.as_ref().map(|f| f.rel.clone()) {
            self.select_tree_row(&rel);
        }
    }

    /// Compares the worktree with `c` from now on; when `c` cannot be
    /// resolved nothing changes but the error.
    pub fn set_compare(&mut self, c: Compare) -> Result<(), String> {
        let base = c.base(Path::new(&self.opts.workspace))?;
        self.cmp = c;
        self.base_id = base.id;
        self.base_name = base.name;
        self.show_side(Sidebar::Changes)?;
        self.reload(false)
    }

    /// Re-reads the open comments (workspace and open file); every lane shown.
    pub fn load_comments(&mut self) -> Result<(), String> {
        let ws = self.opts.workspace.clone();
        let all = self
            .opts
            .service
            .list(QueryInput {
                workspace: ws.clone(),
                status: "open".to_owned(),
                ..QueryInput::default()
            })
            .map_err(|e| e.to_string())?;
        self.counts.clear();
        for c in &all.comments {
            *self.counts.entry(c.file.clone()).or_insert(0) += 1;
        }
        self.open = all.comments;
        if let Some(abs) = self.file.as_ref().map(|f| f.abs.clone()) {
            let mine = self
                .opts
                .service
                .list(QueryInput {
                    workspace: ws,
                    file: abs,
                    status: "open".to_owned(),
                    ..QueryInput::default()
                })
                .map_err(|e| e.to_string())?;
            if let Some(f) = self.file.as_mut() {
                f.comments = mine.comments;
            }
        }
        Ok(())
    }

    /// Reads and highlights `rel` (relative to the workspace, or absolute).
    pub fn load_file(&self, rel: &str) -> Result<FileView, String> {
        let abs = if Path::new(rel).is_absolute() {
            rel.to_owned()
        } else {
            Path::new(&self.opts.workspace)
                .join(rel)
                .to_string_lossy()
                .into_owned()
        };
        let data = fs::read(&abs).map_err(|_| format!("cannot read {rel}"))?;
        let mut f = FileView {
            rel: rel.to_owned(),
            abs: abs.clone(),
            ..FileView::default()
        };
        if is_binary(&data) {
            f.binary = true;
            return Ok(f);
        }
        let text = String::from_utf8_lossy(&data).into_owned();
        f.plain = source_lines(&text);
        f.lines = highlight(rel, &text, &self.opts.style);
        f.source = text;
        let (signs, hunks) = file_changes(
            Path::new(&self.opts.workspace),
            &abs,
            &self.base_id,
            f.lines.len(),
        );
        f.signs = signs;
        f.hunks = hunks;
        Ok(f)
    }

    /// Shows a file with the cursor on `line` (1-indexed), or for 0 where it
    /// was left this session, else line 1.
    pub fn open_file(&mut self, rel: &str, line: usize) -> Result<(), String> {
        let mut f = self.load_file(rel)?;
        if let Some((old_rel, old_cursor)) = self.file.as_ref().map(|o| (o.rel.clone(), o.cursor)) {
            self.positions.insert(old_rel, old_cursor);
        }
        if line > 0 {
            f.cursor = line - 1;
        } else if let Some(&p) = self.positions.get(rel) {
            f.cursor = p;
        }
        f.cursor = clamp_index(f.cursor, f.len());
        f.offset = f.cursor.saturating_sub(self.body_height() / 3);
        self.file = Some(f);
        self.detail = None;
        self.visual = false;
        self.recent.retain(|r| r.as_str() != rel);
        self.recent.insert(0, rel.to_owned());
        self.file_tree.reveal(rel);
        self.refresh_rows();
        self.select_tree_row(rel);
        self.load_comments()
    }

    /// Re-reads the tree (when `walk`), the open file and the comments; the
    /// cursor stays on its line.
    pub fn reload(&mut self, walk: bool) -> Result<(), String> {
        if walk {
            self.resolve_base();
            self.load_status();
            self.load_tree()?;
            if self.chg_tree.is_some() {
                self.load_changes();
            }
        }
        if let Some((rel, cursor, offset)) = self
            .file
            .as_ref()
            .map(|f| (f.rel.clone(), f.cursor, f.offset))
        {
            let mut f = self.load_file(&rel)?;
            f.cursor = clamp_index(cursor, f.len());
            f.offset = offset;
            if self.visual {
                self.anchor = clamp_index(self.anchor, f.len());
            }
            self.file = Some(f);
        }
        if self.rev_tree.is_some() {
            self.load_reviews()?;
        }
        if let Some(id) = self.detail.as_ref().map(|d| d.id.clone()) {
            if self.show_detail(&id).is_err() {
                self.detail = None;
            }
        }
        self.load_comments()
    }

    /// Recomputes `rows` from the shown tree and clamps the cursor.
    pub fn refresh_rows(&mut self) {
        self.rows = self.tree().rows();
        self.tree_cur = clamp_index(self.tree_cur, self.rows.len());
    }

    /// Moves the tree cursor to the row whose node path is `path`.
    pub fn select_tree_row(&mut self, path: &str) {
        let tree = self.tree();
        if let Some(i) = self
            .rows
            .iter()
            .position(|r| tree.node(r.node).path == path)
        {
            self.tree_cur = i;
        }
    }

    /// Loads submitted reviews and comments into the reviews tree: newest
    /// first, reviews before the standalone comments, and every review
    /// expanded unless it was collapsed before.
    pub fn load_reviews(&mut self) -> Result<(), String> {
        let ws = self.opts.workspace.clone();
        let sheets = self
            .opts
            .service
            .sheets(SheetsInput {
                workspace: ws.clone(),
                status: "all".to_owned(),
                ..SheetsInput::default()
            })
            .map_err(|e| e.to_string())?
            .sheets;
        let all = self
            .opts
            .service
            .list(QueryInput {
                workspace: ws,
                status: "all".to_owned(),
                ..QueryInput::default()
            })
            .map_err(|e| e.to_string())?
            .comments;

        // The row under the cursor stays under it across the rebuild.
        let mut selected = String::new();
        if self.side == Sidebar::Reviews && self.tree_cur < self.rows.len() {
            selected = self.tree().node(self.rows[self.tree_cur].node).path.clone();
        }
        let mut collapsed = Vec::new();
        if let Some(rev) = &self.rev_tree {
            for &c in rev.children(ROOT) {
                let n = rev.node(c);
                if n.dir && !n.expanded {
                    collapsed.push(n.path.clone());
                }
            }
        }

        self.sheets = HashMap::new();
        self.rev_comments = HashMap::new();
        let mut shown = Vec::with_capacity(sheets.len());
        for s in sheets.into_iter().rev() {
            self.sheets.insert(s.review.id.clone(), s.clone());
            for c in &s.comments {
                self.rev_comments.insert(c.id.clone(), c.clone());
            }
            shown.push(s);
        }
        let mut loose = Vec::new();
        for c in all.into_iter().rev() {
            if c.review_id.is_empty() {
                self.rev_comments.insert(c.id.clone(), c.clone());
                loose.push(c);
            }
        }

        self.rev_tree = Some(reviews_tree(&shown, &loose, &collapsed));
        self.refresh_rows();
        if !selected.is_empty() {
            self.select_tree_row(&selected); // a new review lands on top: keep the cursor on its item
        }
        Ok(())
    }

    /// Puts the page for a review or comment id in the viewer.
    pub fn show_detail(&mut self, id: &str) -> Result<(), String> {
        let mut d = Detail {
            id: id.to_owned(),
            ..Detail::default()
        };
        if let Some(s) = self.sheets.get(id) {
            d.sheet = Some(s.clone());
        } else {
            let ev = self
                .opts
                .service
                .evidence(GetInput {
                    workspace: self.opts.workspace.clone(),
                    id: id.to_owned(),
                })
                .map_err(|e| e.to_string())?;
            d.ev = Some(ev);
        }
        if let Some(old) = &self.detail {
            if old.id == id {
                d.offset = old.offset;
            }
        }
        self.detail = Some(d);
        self.visual = false;
        Ok(())
    }

    /// The 1-indexed line range a comment would cover (`visual` aware).
    #[must_use]
    pub fn selection(&self) -> (usize, usize) {
        let cursor = self.file.as_ref().map_or(0, |f| f.cursor) + 1;
        if !self.visual {
            return (cursor, cursor);
        }
        let anchor = self.anchor + 1;
        (anchor.min(cursor), anchor.max(cursor))
    }

    /// Keeps every cursor on screen after an update, with a few lines of
    /// context when `margins` is set. Does nothing before the first resize.
    pub fn scroll(&mut self, margins: bool) {
        if self.height == 0 {
            return;
        }
        let body = self.body_height();
        let margin = |n: usize| {
            if margins {
                n.min(body.saturating_sub(1) / 2)
            } else {
                0
            }
        };
        let view_w = self.view_width();
        if let Some(d) = self.detail.as_mut() {
            let n = d.layout(view_w).len();
            d.offset = d.offset.min(n.saturating_sub(body));
        } else if let Some(f) = self.file.as_mut() {
            f.offset = follow(f.cursor, f.offset, body, f.len(), margin(3));
        }
        self.tree_off = follow(
            self.tree_cur,
            self.tree_off,
            body,
            self.rows.len(),
            margin(2),
        );
        let picker_rows = self.picker_rows();
        if let Some(p) = self.picker.as_mut() {
            p.cursor = clamp_index(p.cursor, p.matches.len());
            p.offset = follow(p.cursor, p.offset, picker_rows, p.matches.len(), 0);
        }
    }

    /// Shows `text` in the bottom bar for [`FLASH_FOR_MS`]; returns the timer
    /// effect.
    pub fn set_flash(&mut self, text: &str, is_err: bool) -> Vec<Effect> {
        self.flash_seq += 1;
        text.clone_into(&mut self.flash);
        self.flash_err = is_err;
        vec![Effect::FlashTimer {
            seq: self.flash_seq,
        }]
    }

    /// Shows an error the way the CLI prints it: `rvw: <err>`.
    pub fn fail(&mut self, err: &str) -> Vec<Effect> {
        self.set_flash(&format!("rvw: {err}"), true)
    }

    /// What an id-less submit will link: this author's pending, unlinked
    /// comments in this lane.
    #[must_use]
    pub fn my_pending(&self) -> Vec<Comment> {
        self.open
            .iter()
            .filter(|c| {
                c.status == Status::Pending
                    && c.review_id.is_empty()
                    && c.author.as_deref().unwrap_or("") == self.opts.author
                    && c.lane.as_deref().unwrap_or("") == self.opts.lane
            })
            .cloned()
            .collect()
    }

    /// How many rows the picker overlay lists: `clamp(height-12, 3, 15)`.
    #[must_use]
    pub fn picker_rows(&self) -> usize {
        self.height.saturating_sub(12).clamp(3, 15)
    }

    /// The name and mark of a row on the reviews side, a port of `reviewRow`:
    /// a review's arrow, id and decision with its progress mark; the standalone
    /// group's name; or a comment's id and location with its status mark.
    #[allow(dead_code)] // view.rs draws the reviews rows; nothing calls it yet
    pub(crate) fn review_row(&self, node: NodeId) -> (Vec<Piece>, Piece) {
        let Some(tree) = self.rev_tree.as_ref() else {
            return (Vec::new(), piece("", style::plain()));
        };
        let n = tree.node(node);
        let arrow = if n.expanded { "▾ " } else { "▸ " };
        if let Some(s) = self.sheets.get(&n.path) {
            let name = vec![
                piece(arrow, style::dir()),
                piece(s.review.id.clone(), style::id()),
                piece(format!(" {}", s.review.decision), style::plain()),
            ];
            return (name, state_mark(s.state));
        }
        if n.path == STANDALONE {
            return (
                vec![piece(format!("{arrow}comments"), style::dir())],
                piece("", style::plain()),
            );
        }
        match self.rev_comments.get(&n.path) {
            Some(c) => (
                vec![
                    piece("  ", style::plain()),
                    piece(c.id.clone(), style::id()),
                    piece(format!(" {}", short_location(c)), style::plain()),
                ],
                status_mark(c.status),
            ),
            None => (Vec::new(), piece("", style::plain())),
        }
    }
}

/// The reviews side's tree: one directory per review, newest first, holding
/// its linked comments in review order, then a standalone group of the
/// comments no review links. A directory named in `collapsed` starts
/// collapsed; the rest start expanded.
///
/// Needs a `Tree` that adds children in insertion order (`Tree::add`, which
/// `build_tree` uses and then sorts), so it is not built here yet.
fn reviews_tree(shown: &[ReviewSheet], loose: &[Comment], collapsed: &[String]) -> Tree {
    let mut tree = build_tree::<&str>(&[]);
    for s in shown {
        let r = tree.add(ROOT, &s.review.id, &s.review.id, true);
        tree.set_expanded(r, !collapsed.contains(&s.review.id));
        for c in &s.comments {
            tree.add(r, &c.id, &c.id, false);
        }
    }
    if !loose.is_empty() {
        let g = tree.add(ROOT, "comments", STANDALONE, true);
        tree.set_expanded(g, !collapsed.iter().any(|c| c == STANDALONE));
        for c in loose {
            tree.add(g, &c.id, &c.id, false);
        }
    }
    tree
}

impl Detail {
    /// The title shown over the page.
    #[must_use]
    pub fn title(&self) -> String {
        if let Some(s) = &self.sheet {
            return format!("{} · {}", s.review.id, s.review.decision);
        }
        match &self.ev {
            Some(ev) => format!("{} · {}", ev.comment.id, ev.comment.location()),
            None => String::new(),
        }
    }

    /// The page laid out for width `w` (cached per width).
    pub fn layout(&mut self, w: usize) -> &Vec<Vec<Piece>> {
        if self.lines.is_empty() || self.width != w {
            self.width = w;
            self.lines = match (&self.sheet, &self.ev) {
                (Some(s), _) => sheet_page(s, w),
                (None, Some(ev)) => evidence_page(ev, w),
                (None, None) => Vec::new(),
            };
        }
        &self.lines
    }
}

/// Scrolls a window of `height` rows over `n` items so `cur` stays at least
/// `margin` rows from its edges; returns the new offset.
#[must_use]
// The window arithmetic runs in signed space, where a margin can overshoot the top.
#[allow(
    clippy::cast_possible_wrap,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
pub fn follow(cur: usize, off: usize, height: usize, n: usize, margin: usize) -> usize {
    let (cur, off, height, n) = (cur as i64, off as i64, height as i64, n as i64);
    let margin = margin as i64;
    let mut off = off;
    if cur < off + margin {
        off = cur - margin;
    }
    if cur > off + height - 1 - margin {
        off = cur - height + 1 + margin;
    }
    off.clamp(0, (n - height).max(0)) as usize
}

/// Go's `clamp`: `lo` wins when `hi < lo`.
#[must_use]
pub fn clamp(v: i64, lo: i64, hi: i64) -> i64 {
    if hi < lo {
        lo
    } else {
        v.clamp(lo, hi)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Arc;

    use rvw_core::operations::{ResolveInput, SubmitInput};

    use super::*;
    use crate::testing::{setup, Fixture, PARSE_PY};

    /// The model the Go tests build (author `tester`, 100x30), made directly
    /// so no test depends on the update loop.
    fn open_model(f: &Fixture) -> Model {
        let mut m = Model::new(opts(f)).expect("model");
        m.width = 100;
        m.height = 30;
        m
    }

    fn opts(f: &Fixture) -> Options {
        Options {
            service: Arc::clone(&f.svc),
            workspace: f.ws.clone(),
            lane: String::new(),
            author: "tester".to_owned(),
            editor: "true".to_owned(),
            style: String::new(),
            leader: String::new(),
        }
    }

    /// The rows of the shown tree, indented by depth, directories suffixed `/`.
    fn row_names(m: &Model) -> Vec<String> {
        let tree = m.tree();
        m.rows
            .iter()
            .map(|r| {
                let n = tree.node(r.node);
                let mut s = "  ".repeat(r.depth) + &n.name;
                if n.dir {
                    s.push('/');
                }
                s
            })
            .collect()
    }

    fn file(m: &Model) -> &FileView {
        m.file.as_ref().expect("a file is open")
    }

    fn text(lines: &[Vec<Piece>]) -> String {
        lines
            .iter()
            .map(|l| l.iter().map(|p| p.text.as_str()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// parse.py committed with a header, then edited: a line added, one changed,
    /// one deleted mid-file, the first one deleted, and an untracked file.
    fn git_fixture() -> Fixture {
        let f = setup();
        f.write("src/api/parse.py", &format!("# header\n{PARSE_PY}"));
        f.git(&["commit", "-qam", "header"]);
        f.write(
            "src/api/parse.py",
            "import parse\n\n\ndef handle(req):\n    if req.kind == \"y\":\n        body = req.body\n        log(body)\n        return parse(body)\n\n\ndef default(req):\n\treturn None\n",
        );
        f.write("src/new.py", "a = 1\nb = 2\n");
        f
    }

    /// The fixture on a feature branch off main: util.py changed in a commit
    /// on the branch, papers.md in a commit on main after the branch point,
    /// and parse.py edited but not committed.
    fn branched() -> Fixture {
        let f = setup();
        f.git(&["checkout", "-qb", "feature"]);
        f.write("src/util.py", "def util():\n    return 1\n");
        f.git(&["commit", "-qam", "util"]);
        f.git(&["checkout", "-q", "main"]);
        f.write("docs/api/papers.md", "# Papers\n\nmore\n");
        f.git(&["commit", "-qam", "papers"]);
        f.git(&["checkout", "-q", "feature"]);
        f.write("src/api/parse.py", &format!("# edited\n{PARSE_PY}"));
        f
    }

    #[test]
    fn new_opens_the_first_file_and_loads_status_and_comments() {
        let f = setup();
        f.add("src/api/parse.py", 5, 8, "extract", "tester");
        let m = open_model(&f);
        // Directories sort first, so the first top-level file is README.md.
        assert_eq!(file(&m).rel, "README.md");
        assert_eq!(m.branch, "main");
        assert_eq!(m.base_id, "HEAD");
        assert_eq!(m.base_name, "HEAD");
        assert_eq!(m.counts.get("src/api/parse.py"), Some(&1));
        assert_eq!(m.open.len(), 1);
        assert_eq!(m.focus, Pane::Viewer);
    }

    #[test]
    fn new_without_a_worktree_says_why() {
        let f = setup();
        let outside = tempfile::TempDir::new().expect("tempdir");
        let ws = outside.path().to_string_lossy().into_owned();
        let err = Model::new(Options {
            workspace: ws.clone(),
            ..opts(&f)
        })
        .err()
        .expect("no worktree");
        assert!(
            err.starts_with(&format!("cannot list workspace {ws}: ")),
            "{err}"
        );
    }

    #[test]
    fn opening_a_file_remembers_where_it_was_left() {
        let f = setup();
        let mut m = open_model(&f);
        m.open_file("src/api/parse.py", 9).unwrap();
        assert_eq!(file(&m).cursor, 8);
        m.open_file("src/util.py", 0).unwrap();
        assert_eq!(file(&m).cursor, 0, "a new file opens on line 1");
        m.open_file("src/api/parse.py", 0).unwrap();
        assert_eq!(file(&m).cursor, 8, "reopened where it was left");
        assert_eq!(
            m.recent,
            ["src/api/parse.py", "src/util.py", "README.md"],
            "most recent first, including the file opened at start"
        );
        assert_eq!(m.positions.get("src/util.py"), Some(&0));
    }

    #[test]
    fn opening_a_missing_file_says_so() {
        let f = setup();
        let mut m = open_model(&f);
        assert_eq!(
            m.open_file("nope.txt", 0),
            Err("cannot read nope.txt".to_owned())
        );
    }

    #[test]
    fn binary_files_have_no_lines() {
        let f = setup();
        f.write("blob.bin", "a\0b");
        let mut m = open_model(&f);
        m.open_file("blob.bin", 0).unwrap();
        assert!(file(&m).binary);
        assert_eq!(file(&m).len(), 0);
        assert!(file(&m).is_empty());
    }

    #[test]
    fn reload_keeps_the_cursor_and_picks_up_comments_added_elsewhere() {
        let f = setup();
        let mut m = open_model(&f);
        m.open_file("src/api/parse.py", 4).unwrap();
        f.add("src/api/parse.py", 10, 10, "from another shell", "someone");
        assert!(file(&m).comments.is_empty(), "nothing is watched");
        m.reload(false).unwrap();
        assert_eq!(file(&m).comments.len(), 1);
        assert_eq!(m.counts.get("src/api/parse.py"), Some(&1));
        assert_eq!(file(&m).cursor, 3);
    }

    #[test]
    fn reload_with_walk_keeps_expanded_directories() {
        let f = setup();
        let mut m = open_model(&f);
        let docs = m.file_tree.find("docs").unwrap();
        m.file_tree.set_expanded(docs, true);
        let test = m.file_tree.find("test").unwrap();
        m.file_tree.set_expanded(test, false);
        f.write("docs/new.md", "new\n");
        m.reload(true).unwrap();
        assert!(m.file_tree.node(m.file_tree.find("docs").unwrap()).expanded);
        assert!(!m.file_tree.node(m.file_tree.find("test").unwrap()).expanded);
        assert!(m.file_tree.find("docs/new.md").is_some());
    }

    #[test]
    fn opening_a_file_reveals_it_and_selects_its_row() {
        let f = setup();
        let mut m = open_model(&f);
        m.open_file("src/api/parse.py", 0).unwrap();
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
        let row = m.rows[m.tree_cur];
        assert_eq!(m.tree().node(row.node).path, "src/api/parse.py");
    }

    #[test]
    fn gutter_signs_follow_git_changes() {
        let f = git_fixture();
        let mut m = open_model(&f);
        m.open_file("src/api/parse.py", 0).unwrap();
        let (top, add, chg, del, none) = (
            Sign::DeletedTop,
            Sign::Added,
            Sign::Changed,
            Sign::Deleted,
            Sign::None,
        );
        assert_eq!(
            file(&m).signs,
            [top, none, none, none, chg, none, add, del, none, none, none, none]
        );
        assert_eq!(file(&m).hunks, [1, 5, 7, 8]);

        m.open_file("src/new.py", 0).unwrap();
        assert_eq!(file(&m).signs, [add, add], "an untracked file is all added");

        m.open_file("src/util.py", 0).unwrap();
        assert!(file(&m).signs.is_empty(), "an unchanged file has no signs");

        f.write("src/util.py", "def util():\n    return 1\n");
        m.reload(false).unwrap();
        assert_eq!(file(&m).signs, [none, chg], "reload recomputes the signs");
    }

    #[test]
    fn changes_view_lists_uncommitted_changes() {
        let f = git_fixture();
        fs::remove_file(Path::new(&f.ws).join("README.md")).unwrap();
        let mut m = open_model(&f);
        assert_eq!(m.side, Sidebar::Files);
        m.show_side(Sidebar::Changes).unwrap();
        assert_eq!(m.side, Sidebar::Changes);
        assert_eq!(
            row_names(&m),
            ["src/", "  api/", "    parse.py", "  new.py", "README.md"]
        );
        assert_eq!(m.status["src/api/parse.py"], b'M');
        assert_eq!(m.status["src/new.py"], b'?');
        assert_eq!(m.status["README.md"], b'D');
        assert!(m.chg_err.is_empty());
    }

    #[test]
    fn changes_against_the_default_branch_start_at_the_merge_base() {
        let f = branched();
        let mut m = open_model(&f);
        m.open_file("src/util.py", 0).unwrap();
        assert!(
            file(&m).signs.is_empty(),
            "against HEAD util.py is unchanged"
        );

        m.set_compare(Compare::Default).unwrap();
        assert_eq!(m.cmp, Compare::Default);
        assert_eq!(m.base_name, "main");
        assert_eq!(
            row_names(&m),
            ["src/", "  api/", "    parse.py", "  util.py"],
            "not main's own papers.md"
        );
        assert_eq!(file(&m).signs, [Sign::None, Sign::Changed]);

        m.set_compare(Compare::Head).unwrap();
        assert_eq!(row_names(&m), ["src/", "  api/", "    parse.py"]);
        assert!(file(&m).signs.is_empty(), "the gutter goes back with it");
    }

    #[test]
    fn changes_against_the_previous_commit_include_the_last_commit() {
        let f = branched();
        let mut m = open_model(&f);
        m.set_compare(Compare::Prev).unwrap();
        assert_eq!(m.base_name, "HEAD~1");
        assert_eq!(
            row_names(&m),
            ["src/", "  api/", "    parse.py", "  util.py"]
        );
    }

    #[test]
    fn a_compare_that_cannot_resolve_keeps_the_previous_one() {
        let f = setup();
        let mut m = open_model(&f);
        let err = m.set_compare(Compare::Prev).unwrap_err();
        assert_eq!(err, "no previous commit to compare with");
        assert_eq!(m.cmp, Compare::Head);
        assert_eq!(m.side, Sidebar::Files);
        m.set_compare(Compare::Default).unwrap();
        assert_eq!(
            m.cmp,
            Compare::Default,
            "on main itself the default branch resolves"
        );
        assert!(
            row_names(&m).is_empty(),
            "main against itself has no changes"
        );
    }

    #[test]
    fn changes_view_without_a_commit_says_why() {
        let f = setup();
        f.git(&["update-ref", "-d", "HEAD"]);
        let mut m = open_model(&f);
        m.show_side(Sidebar::Changes).unwrap();
        assert!(m.rows.is_empty());
        assert_eq!(m.chg_err, "no commit to compare with yet");
    }

    #[test]
    fn a_commit_made_meanwhile_moves_the_comparison() {
        let f = setup();
        let mut m = open_model(&f);
        f.write("src/util.py", "def util():\n    return 1\n");
        f.git(&["commit", "-qam", "util"]);
        m.open_file("src/util.py", 0).unwrap();
        assert!(
            file(&m).signs.is_empty(),
            "the gutter compares with HEAD as it is now"
        );
        m.show_side(Sidebar::Changes).unwrap();
        assert!(m.rows.is_empty(), "so does the changes view");
    }

    #[test]
    fn the_first_commit_made_meanwhile_is_found_by_reload() {
        let f = setup();
        f.git(&["update-ref", "-d", "HEAD"]);
        let mut m = open_model(&f);
        f.git(&["commit", "-qm", "first"]);
        f.write("README.md", "# Demo\n");
        m.reload(true).unwrap();
        m.show_side(Sidebar::Changes).unwrap();
        assert!(m.chg_err.is_empty());
        assert_eq!(row_names(&m), ["README.md"]);
    }

    #[test]
    fn untracked_files_count_their_text_lines() {
        let f = setup();
        f.write("a.txt", "one\ntwo\nthree"); // no newline at the end
        f.write("b.bin", "\0\x01\x02\n");
        f.write("empty.txt", "");
        let m = open_model(&f);
        assert_eq!(m.wd.added, 3);
        assert_eq!(m.wd.deleted, 0);
        assert!(m.wd.dirty);
        assert!(m.wd.err.is_empty());
    }

    #[test]
    fn status_without_a_commit_says_why() {
        let f = setup();
        f.git(&["update-ref", "-d", "HEAD"]);
        let m = open_model(&f);
        assert_eq!(m.wd.err, "no commits yet");
    }

    #[test]
    fn selection_covers_the_cursor_or_the_visual_range() {
        let f = setup();
        let mut m = open_model(&f);
        m.open_file("src/api/parse.py", 5).unwrap();
        assert_eq!(m.selection(), (5, 5));
        m.visual = true;
        m.anchor = 7;
        assert_eq!(m.selection(), (5, 8));
        m.anchor = 1;
        assert_eq!(m.selection(), (2, 5));
    }

    #[test]
    fn scroll_keeps_the_cursor_on_screen() {
        let f = setup();
        let mut m = open_model(&f);
        m.open_file("src/api/parse.py", 14).unwrap();
        m.scroll(true);
        let body = m.body_height();
        let cur = file(&m).cursor;
        assert!(file(&m).offset <= cur && cur < file(&m).offset + body);
    }

    #[test]
    fn set_flash_and_fail_return_the_timer() {
        let f = setup();
        let mut m = open_model(&f);
        let effects = m.set_flash("saved", false);
        assert_eq!(m.flash, "saved");
        assert!(!m.flash_err);
        assert!(matches!(effects[0], Effect::FlashTimer { seq } if seq == m.flash_seq));
        let before = m.flash_seq;
        let effects = m.fail("boom");
        assert_eq!(m.flash, "rvw: boom");
        assert!(m.flash_err);
        assert!(matches!(effects[0], Effect::FlashTimer { seq } if seq == before + 1));
    }

    #[test]
    fn my_pending_is_this_authors_unlinked_comments_in_this_lane() {
        let f = setup();
        f.add("src/api/parse.py", 1, 1, "mine", "tester");
        f.add("src/api/parse.py", 2, 2, "theirs", "other");
        let m = open_model(&f);
        let mine = m.my_pending();
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].comment, "mine");
    }

    #[test]
    fn picker_rows_fit_the_height() {
        let f = setup();
        let mut m = open_model(&f);
        for (height, rows) in [(0, 3), (12, 3), (20, 8), (30, 15), (99, 15)] {
            m.height = height;
            assert_eq!(m.picker_rows(), rows, "height {height}");
        }
    }

    #[test]
    fn follow_keeps_the_cursor_within_its_margins() {
        assert_eq!(follow(0, 5, 10, 20, 2), 0);
        assert_eq!(follow(15, 0, 10, 20, 2), 8);
        assert_eq!(follow(19, 0, 10, 20, 0), 10);
        assert_eq!(clamp(5, 3, 1), 3, "lo wins when hi < lo");
    }

    #[test]
    fn a_comment_page_shows_its_evidence() {
        let f = setup();
        f.add("src/api/parse.py", 5, 8, "extract this branch", "reviewer");
        let mut m = open_model(&f);
        m.show_detail("r1").unwrap();
        let detail = m.detail.as_mut().expect("a page");
        assert_eq!(detail.title(), "r1 · src/api/parse.py:5-8");
        let page = text(detail.layout(80));
        assert!(page.contains("pending since "), "{page}");
        assert!(page.contains("extract this branch"), "{page}");
        assert!(
            page.contains("o opens src/api/parse.py at line 5"),
            "{page}"
        );
    }

    #[test]
    fn a_review_page_shows_its_summary_and_comments() {
        let f = setup();
        f.add("src/api/parse.py", 5, 8, "extract", "reviewer");
        f.svc
            .submit(SubmitInput {
                workspace: f.ws.clone(),
                decision: "request-changes".to_owned(),
                summary: "Fix both before merging.".to_owned(),
                author: "reviewer".to_owned(),
                ..SubmitInput::default()
            })
            .expect("submit");
        let mut m = open_model(&f);
        let sheet = f
            .svc
            .sheet(GetInput {
                workspace: f.ws.clone(),
                id: "rv1".to_owned(),
            })
            .expect("sheet");
        m.sheets.insert("rv1".to_owned(), sheet);
        m.show_detail("rv1").unwrap();
        let detail = m.detail.as_mut().expect("a page");
        assert_eq!(detail.title(), "rv1 · request-changes");
        let page = text(detail.layout(80));
        assert!(page.contains("Fix both before merging."), "{page}");
        assert!(page.contains("r1 src/api/parse.py:5"), "{page}");
    }

    #[test]
    fn a_detail_page_of_an_unknown_id_is_an_error() {
        let f = setup();
        let mut m = open_model(&f);
        assert!(m.show_detail("r99").is_err());
        assert!(m.detail.is_none());
    }

    /// rv1 asks for two changes, one done and one rejected; rv2 approves with
    /// no comments; r3 is a standalone comment pulled but not decided, and r4
    /// one still pending.
    fn reviewed() -> Fixture {
        let f = setup();
        f.add(
            "src/api/parse.py",
            5,
            8,
            "extract this branch into parse_x()",
            "reviewer",
        );
        f.add("src/util.py", 2, 2, "typo", "reviewer");
        f.svc
            .submit(SubmitInput {
                workspace: f.ws.clone(),
                decision: "request-changes".to_owned(),
                summary: "Fix both before merging.".to_owned(),
                author: "reviewer".to_owned(),
                ..SubmitInput::default()
            })
            .expect("submit");
        f.svc
            .submit(SubmitInput {
                workspace: f.ws.clone(),
                decision: "approve".to_owned(),
                summary: "Docs look good.".to_owned(),
                no_comments: true,
                author: "reviewer".to_owned(),
                ..SubmitInput::default()
            })
            .expect("submit");
        f.add("README.md", 1, 1, "title case", "gustavo");
        f.svc
            .pull(rvw_core::operations::PullInput {
                workspace: f.ws.clone(),
                ..Default::default()
            })
            .expect("pull");
        f.add("src/util.py", 1, 1, "name it better", "gustavo");
        f.write(
            "src/api/parse.py",
            &PARSE_PY.replace(
                "        body = req.body\n        return parse(body)\n",
                "        return parse_x(req)\n",
            ),
        );
        for (id, outcome, note) in [
            ("r1", "done", "extracted parse_x()"),
            ("r2", "rejected", "not a typo: pass is intended"),
        ] {
            f.svc
                .resolve(ResolveInput {
                    workspace: f.ws.clone(),
                    id: id.to_owned(),
                    outcome: outcome.to_owned(),
                    note: note.to_owned(),
                    author: "claude".to_owned(),
                })
                .expect("resolve");
        }
        f
    }

    #[test]
    fn the_reviews_side_lists_reviews_newest_first_then_standalone_comments() {
        let f = reviewed();
        let mut m = open_model(&f);
        m.show_side(Sidebar::Reviews).unwrap();
        assert_eq!(m.side, Sidebar::Reviews);
        assert_eq!(
            row_names(&m),
            ["rv2/", "rv1/", "  r1", "  r2", "comments/", "  r4", "  r3"]
        );
    }

    #[test]
    fn a_review_shows_on_top_and_the_cursor_stays_on_its_item() {
        let f = reviewed();
        let mut m = open_model(&f);
        m.show_side(Sidebar::Reviews).unwrap();
        m.select_tree_row("rv1");
        f.svc
            .submit(SubmitInput {
                workspace: f.ws.clone(),
                decision: "comment".to_owned(),
                summary: "Rename it.".to_owned(),
                no_comments: true,
                author: "reviewer".to_owned(),
                ..SubmitInput::default()
            })
            .expect("submit");
        m.load_reviews().unwrap();
        assert_eq!(row_names(&m)[0], "rv3/");
        assert_eq!(m.rows[m.tree_cur].node, m.tree().find("rv1").unwrap());
    }
}
