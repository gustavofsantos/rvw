//! The diff's virtualised list: its rows, and when they are rebuilt.
//!
//! The list measures and draws only the rows in view, so a selection click or
//! a caret blink repaints a screenful whatever the length of the file. Its
//! rows are rebuilt (and their measured heights dropped) only when the file,
//! its comments, the composer's place or the composer's error change; the
//! current selection is read from a shared cell by the visible rows.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::rc::Rc;

use gpui::{list, px, Entity, EntityId, List, ListAlignment, ListState, WeakEntity, Window};
use rvw_core::types::Comment;
use rvw_view::diff::FileDiff;
use rvw_view::diffrows::{self, DiffRow};
use rvw_view::selection::{self, Pos, Selection};

use crate::diff_view;
use crate::highlights::Shared;
use crate::shell::Shell;
use crate::text_area::TextArea;

/// How far past the viewport rows are measured, so a scroll does not pop.
const OVERDRAW: f32 = 800.;

/// The composer's field and the error of its last failed add.
pub struct ComposerSlot {
    pub input: Entity<TextArea>,
    pub error: Option<String>,
    /// The id of the comment the composer edits, if it edits one.
    pub editing: Option<String>,
}

/// What the rows are drawn from; replaced whole when the list is rebuilt.
pub struct Model {
    pub diff: Rc<FileDiff>,
    /// The comments of the open file; thread rows index into this.
    pub comments: Vec<Comment>,
    pub rows: Vec<DiffRow>,
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
        list(self.state.clone(), move |index, _, _| {
            diff_view::row(&model, index, selection.get(), &shell)
        })
    }
}

/// The file's comments and the rows of its diff. `outside_edit` is the id of
/// a comment with no line in view that the composer edits.
fn layout(
    diff: &FileDiff,
    all: &[Comment],
    path: &str,
    composer_after: Option<Pos>,
    outside_edit: Option<&str>,
) -> (Vec<Comment>, Vec<DiffRow>) {
    let comments: Vec<Comment> = all
        .iter()
        .filter(|comment| comment.file == path)
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
    let rows = diffrows::rows(diff, &under, composer_after, &outside, composer_outside);
    (comments, rows)
}

impl Shell {
    /// Brings the diff list in line with the open file, building its rows
    /// only when the key changed. Called by every render; a selection change
    /// or a caret blink finds the key equal and costs one comparison.
    pub(crate) fn sync_diff_list(&mut self, window: &Window) {
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
        };
        if let Some(current) = self.diff_list.as_ref() {
            if current.key == key && current.path == selected.path {
                current.selection.set(selected.selection);
                return;
            }
        }

        let outside_edit = editing.filter(|_| edits_outside).map(|c| c.id.as_str());
        let (comments, rows) = layout(
            diff,
            &self.snapshot.comments,
            &selected.path,
            composer_after,
            outside_edit,
        );
        let composer_row = rows
            .iter()
            .position(|row| matches!(row, DiffRow::Composer | DiffRow::OutsideComposer));
        let model = Rc::new(Model {
            diff: Rc::clone(diff),
            comments,
            rows,
            outside_change: selected.outside,
            composer: self.composer.as_ref().map(|composer| ComposerSlot {
                input: composer.input.clone(),
                error: composer.error.clone(),
                editing: composer.editing.clone(),
            }),
            spans: Rc::clone(&self.highlights.shared),
        });

        let previous = self.diff_list.take();
        let opened = key.composer_input.is_some()
            && previous
                .as_ref()
                .is_none_or(|list| list.key.composer_input != key.composer_input);
        // The same file keeps its scroll position through a rebuild; a new
        // file starts at its top.
        let state = match previous {
            Some(list) if list.path == selected.path => {
                let top = list.state.logical_scroll_top();
                list.state.reset(model.rows.len());
                list.state.scroll_to(gpui::ListOffset {
                    item_ix: top.item_ix.min(model.rows.len().saturating_sub(1)),
                    offset_in_item: top.offset_in_item,
                });
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
