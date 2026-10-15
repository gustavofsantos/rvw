//! The diff's virtualised list: its rows, and when they are rebuilt.
//!
//! The list measures and draws only the rows in view, so a selection click or
//! a caret blink repaints a screenful whatever the length of the file. Its
//! rows are rebuilt (and their measured heights dropped) only when the file,
//! its comments, the composer's place or the composer's error change; the
//! current selection is read from a shared cell by the visible rows.

pub(crate) mod drag;

use std::cell::Cell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    Bounds, Entity, EntityId, List, ListAlignment, ListState, Pixels, Point, WeakEntity, Window,
    list, px,
};
use rvw_core::types::Comment;
use rvw_view::diff::FileDiff;
use rvw_view::diffrows::{self, DiffRow};
use rvw_view::queue;
use rvw_view::selection::{self, Pos, Selection};

use crate::code_scroll;
use crate::diff_view;
use crate::highlights::Shared;
use crate::shell::Shell;
use crate::text_area::TextArea;
use crate::text_input::TextInput;

/// The width before a line's code: the number, the marker and the padding.
const CODE_GUTTER: Pixels = px(62.);

/// How far past the viewport rows are measured, so a scroll does not pop.
const OVERDRAW: f32 = 800.;

/// The composer's field, the error of its last failed add, and its "Points
/// at" field when it has one.
pub struct ComposerSlot {
    pub input: Entity<TextArea>,
    pub error: Option<String>,
    /// The id of the comment the composer edits, if it edits one.
    pub editing: Option<String>,
    pub points: Option<Entity<TextInput>>,
}

/// What the rows are drawn from; replaced whole when the list is rebuilt.
pub struct Model {
    pub diff: Arc<FileDiff>,
    /// How far the code text is moved left; see `code_scroll`. Kept by the
    /// rebuilds of the same file.
    pub scroll_x: Rc<Cell<Pixels>>,
    /// The width of the longest line's text.
    pub code_width: Pixels,
    /// The comments of the open file; thread rows index into this.
    pub comments: Vec<Comment>,
    pub rows: Vec<DiffRow>,
    /// The rows are the split view's pairs.
    pub split: bool,
    /// The file is outside the change; the composer's caption says so.
    pub outside_change: bool,
    pub composer: Option<ComposerSlot>,
    /// Syntax spans of the file, filled in by the background task. Read when
    /// a row is drawn, so their arrival needs a repaint and no rebuild.
    pub spans: Shared,
}

/// What the rows were built for. A different key rebuilds them.
#[derive(Clone, PartialEq, Eq)]
struct Key {
    /// Bumped by the shell when the open file or its snapshot is replaced.
    generation: u64,
    composer_after: Option<Pos>,
    /// The composer's field; another field is another composer.
    composer_input: Option<EntityId>,
    error: bool,
    split: bool,
}

pub struct DiffList {
    state: ListState,
    model: Rc<Model>,
    /// The reviewer's selection, read by the rows when they are drawn.
    selection: Rc<Cell<Selection>>,
    path: String,
    key: Key,
}

impl DiffList {
    /// The list element; it draws rows from the model at layout time.
    pub fn element(&self, shell: WeakEntity<Shell>) -> List {
        let model = Rc::clone(&self.model);
        let selection = Rc::clone(&self.selection);
        list(self.state.clone(), move |index, _, cx| {
            diff_view::row(&model, index, selection.get(), &shell, cx)
        })
    }

    /// Where the list was last drawn, in window coordinates.
    pub fn viewport(&self) -> Bounds<Pixels> {
        self.state.viewport_bounds()
    }

    pub fn scroll_by(&self, distance: Pixels) {
        self.state.scroll_by(distance);
    }

    /// Moves the code sideways by a wheel's `delta`, within the longest
    /// line. True when it moved.
    pub fn scroll_code(&self, delta: Pixels) -> bool {
        let view = self.state.viewport_bounds().size.width;
        // A split row has two columns, each with its own gutter.
        let column = if self.model.split { view / 2. } else { view };
        let room = column - CODE_GUTTER;
        let max = code_scroll::max_offset(self.model.code_width, room);
        let old = self.model.scroll_x.get();
        let new = code_scroll::scrolled(old, delta, max);
        self.model.scroll_x.set(new);
        new != old
    }

    /// The line row under `point`, or the nearest visible one when the
    /// point is on a thread or beyond the list's ends; in a split row the
    /// column the point is in. `None` when no line row is in view.
    pub fn pos_near(&self, point: Point<Pixels>) -> Option<Pos> {
        let view = self.state.viewport_bounds();
        let mut before = None;
        for (index, row) in self
            .model
            .rows
            .iter()
            .enumerate()
            .skip(self.state.logical_scroll_top().item_ix)
        {
            let Some(bounds) = self.state.bounds_for_item(index) else {
                break;
            };
            if bounds.bottom() <= view.top() {
                continue;
            }
            if bounds.top() >= view.bottom() {
                break;
            }
            let here = match *row {
                DiffRow::Line { pos } => Some(pos),
                DiffRow::Pair { left, right } if point.x < view.center().x => left.or(right),
                DiffRow::Pair { left, right } => right.or(left),
                _ => None,
            };
            if bounds.bottom() > point.y {
                return here.or(before).or_else(|| self.next_line(index));
            }
            before = here.or(before);
        }
        before
    }

    /// The first line row after row `index` that is in the model.
    fn next_line(&self, index: usize) -> Option<Pos> {
        self.model
            .rows
            .iter()
            .skip(index + 1)
            .find_map(|row| match *row {
                DiffRow::Line { pos } => Some(pos),
                DiffRow::Pair { left, right } => right.or(left),
                _ => None,
            })
    }
}

/// The file's comments and the rows of its diff. Done and rejected comments
/// are left out unless `show_resolved`. `outside_edit` is the id of a comment
/// with no line in view that the composer edits.
fn layout(
    diff: &FileDiff,
    all: &[Comment],
    path: &str,
    show_resolved: bool,
    composer_after: Option<Pos>,
    outside_edit: Option<&str>,
    split: bool,
) -> (Vec<Comment>, Vec<DiffRow>) {
    let comments: Vec<Comment> = all
        .iter()
        .filter(|comment| comment.file == path)
        .filter(|comment| show_resolved || !queue::is_resolved(comment))
        .cloned()
        .collect();
    let mut under: BTreeMap<Pos, Vec<usize>> = BTreeMap::new();
    let mut outside = Vec::new();
    for (index, comment) in comments.iter().enumerate() {
        match selection::anchor(diff, comment.end_line) {
            Some(pos) => under.entry(pos).or_default().push(index),
            None => outside.push(index),
        }
    }
    let composer_outside =
        outside_edit.and_then(|id| comments.iter().position(|comment| comment.id == id));
    let build = if split {
        diffrows::split_rows
    } else {
        diffrows::rows
    };
    let rows = build(diff, &under, composer_after, &outside, composer_outside);
    (comments, rows)
}

/// Where a rebuilt list of the same file starts: the old position when the
/// mode is unchanged, else the first row of the hunk that was at the top.
fn scroll_offset(old: &DiffList, model: &Model) -> gpui::ListOffset {
    let top = old.state.logical_scroll_top();
    let last = model.rows.len().saturating_sub(1);
    if old.model.split == model.split {
        return gpui::ListOffset {
            item_ix: top.item_ix.min(last),
            offset_in_item: top.offset_in_item,
        };
    }
    let row = diffrows::hunk_of_row(&old.model.rows, top.item_ix)
        .and_then(|hunk| diffrows::first_row_of_hunk(&model.rows, hunk));
    gpui::ListOffset {
        item_ix: row.unwrap_or(0).min(last),
        offset_in_item: px(0.),
    }
}

impl Shell {
    /// Brings the diff list in line with the open file, building its rows
    /// only when the key changed. Called by every render; a selection change
    /// or a caret blink finds the key equal and costs one comparison.
    /// The sideways offset of the list's file when it is `path`, which a
    /// rebuild keeps; a new one for another file.
    fn kept_scroll_x(&self, path: &str) -> Rc<Cell<Pixels>> {
        self.diff_list
            .as_ref()
            .filter(|list| list.path == path)
            .map_or_else(Rc::default, |list| Rc::clone(&list.model.scroll_x))
    }

    pub(crate) fn sync_diff_list(&mut self, window: &Window) {
        let split = self.split_active();
        let Some(selected) = self.selected.as_ref() else {
            self.diff_list = None;
            return;
        };
        let diff = match &selected.diff {
            Ok(diff) if !diff.binary && !diff.hunks.is_empty() => diff,
            _ => {
                self.diff_list = None;
                return;
            }
        };
        // A composer editing a comment sits under that comment's last line,
        // or under the comment in the outside strip; a new one, under the
        // selection.
        let editing = self
            .composer
            .as_ref()
            .and_then(|composer| composer.editing.as_deref())
            .and_then(|id| self.snapshot.comments.iter().find(|c| c.id == id));
        let (composer_after, edits_outside) = match (self.composer.as_ref(), editing) {
            (None, _) => (None, false),
            (Some(_), Some(comment)) => {
                let anchor = selection::anchor(diff, comment.end_line);
                (anchor, anchor.is_none())
            }
            (Some(_), None) => (selected.selection.last(), false),
        };
        let key = Key {
            generation: self.diff_generation,
            composer_after,
            composer_input: self.composer.as_ref().map(|c| c.input.entity_id()),
            error: self
                .composer
                .as_ref()
                .is_some_and(|composer| composer.error.is_some()),
            split,
        };
        if let Some(current) = self.diff_list.as_ref()
            && current.key == key
            && current.path == selected.path
        {
            current.selection.set(selected.selection);
            return;
        }

        let outside_edit = editing.filter(|_| edits_outside).map(|c| c.id.as_str());
        let (comments, rows) = layout(
            diff,
            &self.snapshot.comments,
            &selected.path,
            self.show_resolved,
            composer_after,
            outside_edit,
            split,
        );
        let composer_row = rows
            .iter()
            .position(|row| matches!(row, DiffRow::Composer | DiffRow::OutsideComposer));
        let model = Rc::new(Model {
            diff: Arc::clone(diff),
            scroll_x: self.kept_scroll_x(&selected.path),
            code_width: code_scroll::width(self.code_advance, code_scroll::longest_line(diff)),
            comments,
            rows,
            split,
            outside_change: selected.outside,
            composer: self.composer.as_ref().map(|composer| ComposerSlot {
                input: composer.input.clone(),
                error: composer.error.clone(),
                editing: composer.editing.clone(),
                points: composer.points.as_ref().map(|points| points.input.clone()),
            }),
            spans: Rc::clone(&self.highlights.shared),
        });

        let previous = self.diff_list.take();
        let opened = key.composer_input.is_some()
            && previous
                .as_ref()
                .is_none_or(|list| list.key.composer_input != key.composer_input);
        // The same file keeps its scroll position through a rebuild, or, when
        // the mode changed, the place of the hunk at its top; a new file
        // starts at its top.
        let state = match previous {
            Some(list) if list.path == selected.path => {
                let offset = scroll_offset(&list, &model);
                list.state.reset(model.rows.len());
                list.state.scroll_to(offset);
                list.state
            }
            _ => ListState::new(model.rows.len(), ListAlignment::Top, px(OVERDRAW)),
        };
        if let (true, Some(index)) = (opened, composer_row) {
            // The composer's height is known only after it is drawn once.
            let state = state.clone();
            window.on_next_frame(move |window, _| {
                state.scroll_to_reveal_item(index);
                window.refresh();
            });
        }
        self.diff_list = Some(DiffList {
            state,
            model,
            selection: Rc::new(Cell::new(selected.selection)),
            path: selected.path.clone(),
            key,
        });
    }
}
