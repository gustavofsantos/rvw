//! Changing a draft comment: opening its composer to edit it, deleting it,
//! and the undo that follows a delete. Every step is one service call and a
//! reload; the rules are in the service and in `rvw_view::queue::is_draft`.

use gpui::{AppContext, Context, Window};
use rvw_core::types::{Comment, Location};
use rvw_view::queue::is_draft;
use rvw_view::selection;

use crate::composer::{self, Composer, PointsField};
use crate::data;
use crate::shell::Shell;
use crate::text_area::{TextArea, TextAreaEvent};

/// A deleted comment, with what a re-add needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removed {
    pub id: String,
    pub file: String,
    pub lines: (i64, i64),
    pub text: String,
    /// The "points at" reference, so an undo keeps it.
    pub points_at: Option<Location>,
}

impl Removed {
    pub fn from_comment(comment: &Comment) -> Self {
        Self {
            id: comment.id.clone(),
            file: comment.file.clone(),
            lines: (comment.start_line, comment.end_line),
            text: comment.comment.clone(),
            points_at: comment.points_at.clone(),
        }
    }
}

/// The line at the top of the draft list after a delete or its undo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Removal {
    /// The comment is gone; Undo adds it back.
    Removed(Removed),
    /// A delete or undo failed, with the message.
    Failed(String),
}

impl Shell {
    /// Puts a composer under the selection (or under the comment it edits),
    /// with focus in its field. `editing` is the id of the comment it edits.
    pub(crate) fn install_composer(
        &mut self,
        editing: Option<String>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The composer belongs to the file on screen, so a read still running
        // for another file is dropped rather than replacing it under the text.
        self.file_pending = None;
        // A new comment on a file outside the change gets the context example
        // in its "Points at" field. An edit keeps the reference, so has none.
        let outside = self
            .selected
            .as_ref()
            .is_some_and(|selected| selected.outside);
        let points = editing.is_none().then(|| PointsField::new(outside, cx));
        let input = cx.new(|cx| {
            let mut area = TextArea::new("Write a comment", cx).with_lines(2, 8);
            if !text.is_empty() {
                area.set_text(text, cx);
            }
            area
        });
        let events = cx.subscribe(
            &input,
            |this: &mut Self, _, event: &TextAreaEvent, cx: &mut Context<Self>| {
                match event {
                    TextAreaEvent::Cancel => this.composer = None,
                    TextAreaEvent::Submit => this.submit_comment(cx),
                }
                cx.notify();
            },
        );
        input.update(cx, |area, cx| area.focus(window, cx));
        self.composer = Some(Composer::new(input, events, editing, points));
        self.load_why_line(cx);
        cx.notify();
    }

    /// Adds the typed comment on the selection, or replaces the text of the
    /// comment being edited. A blank new comment keeps the composer open; a
    /// refused write shows its message in the composer.
    pub(crate) fn submit_comment(&mut self, cx: &mut Context<Self>) {
        let Some((text, points_text, editing)) = self.composer.as_ref().map(|composer| {
            (
                composer.input.read(cx).text().trim_end().to_owned(),
                composer
                    .points
                    .as_ref()
                    .map(|points| points.input.read(cx).text().to_owned())
                    .unwrap_or_default(),
                composer.editing.clone(),
            )
        }) else {
            return;
        };
        let result = match editing {
            Some(id) => data::edit_comment(&self.service, &self.identity, &id, &text).map(|_| ()),
            None if text.is_empty() => return,
            // The "Points at" field shows its own problem; nothing is added.
            None => match composer::points_spec(&points_text) {
                Ok(points_at) => self.add_selected(&text, points_at),
                Err(_) => return,
            },
        };
        match result {
            Ok(()) => self.reload(cx),
            Err(message) => {
                if let Some(composer) = self.composer.as_mut() {
                    composer.error = Some(message);
                }
                cx.notify();
            }
        }
    }

    fn add_selected(&self, text: &str, points_at: Option<Location>) -> Result<(), String> {
        let selected = self.selected.as_ref().ok_or("no file is open")?;
        let diff = selected.diff.as_ref().map_err(Clone::clone)?;
        let (lo, hi) = selected
            .selection
            .lines(diff)
            .ok_or("select a line first")?;
        let lines = (i64::from(lo), i64::from(hi));
        data::add_comment(
            &self.service,
            &self.identity,
            &selected.path,
            lines,
            text,
            points_at,
        )
    }

    /// Opens the composer on draft `id`, prefilled, under the comment's last
    /// line. The comment's lines are selected so the composer sits where a
    /// new one would. Does nothing for a comment that is not a draft.
    pub fn start_edit(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let editing_now = self
            .composer
            .as_ref()
            .and_then(|composer| composer.editing.as_deref());
        if editing_now == Some(id) {
            return;
        }
        let Some(comment) = self
            .snapshot
            .comments
            .iter()
            .find(|comment| comment.id == id && is_draft(comment))
            .cloned()
        else {
            return;
        };
        self.removal = None;
        self.composer = None;
        if let Some(selected) = self.selected.as_mut() {
            selected.selection.clear();
            if let Ok(diff) = selected.diff.clone() {
                let start = selection::anchor(&diff, comment.start_line);
                let end = selection::anchor(&diff, comment.end_line);
                if let Some(end) = end {
                    selected.selection.click(&diff, start.unwrap_or(end));
                    selected.selection.extend(&diff, end);
                }
            }
        }
        self.install_composer(Some(comment.id), &comment.comment, window, cx);
    }

    /// Deletes draft `id`. The right panel then offers to undo it.
    pub fn delete_comment(&mut self, id: &str, cx: &mut Context<Self>) {
        self.removal = None;
        match data::remove_comment(&self.service, &self.identity, id) {
            Ok(comment) => {
                let editing_it = self
                    .composer
                    .as_ref()
                    .is_some_and(|composer| composer.editing.as_deref() == Some(id));
                if editing_it {
                    self.composer = None;
                }
                self.removal = Some(Removal::Removed(Removed::from_comment(&comment)));
                if let Err(message) = self.refresh() {
                    self.removal = Some(Removal::Failed(format!(
                        "Removed, but the view did not reload: {message}"
                    )));
                }
            }
            Err(message) => {
                self.removal = Some(Removal::Failed(message));
                // The comment may have been pulled since the snapshot.
                self.refresh().ok();
            }
        }
        cx.notify();
    }

    /// Adds the last deleted comment back, on its file and lines.
    pub fn undo_removal(&mut self, cx: &mut Context<Self>) {
        let Some(Removal::Removed(removed)) = self.removal.take() else {
            return;
        };
        let added = data::add_comment(
            &self.service,
            &self.identity,
            &removed.file,
            removed.lines,
            &removed.text,
            removed.points_at.clone(),
        );
        match added {
            Ok(()) => {
                if let Err(message) = self.refresh() {
                    self.removal = Some(Removal::Failed(format!(
                        "Restored, but the view did not reload: {message}"
                    )));
                }
            }
            Err(message) => {
                self.removal = Some(Removal::Failed(format!("Could not undo: {message}")));
            }
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_re_add_request_carries_file_range_and_text() {
        let comment = Comment {
            points_at: Some(Location {
                file: "src/service.rs".into(),
                start_line: 90,
                end_line: 96,
            }),
            reopens: None,
            verdict: None,
            id: "r4".into(),
            status: rvw_core::types::Status::Pending,
            workspace: "/ws".into(),
            lane: Some("feat".into()),
            author: Some("gus".into()),
            file: "src/a.rs".into(),
            path: "/ws/src/a.rs".into(),
            start_line: 7,
            end_line: 9,
            filetype: "rust".into(),
            code: "x".into(),
            comment: "rename this".into(),
            created_at: String::new(),
            pulled_at: None,
            resolved_at: None,
            resolved_by: None,
            resolution_note: None,
            file_version: String::new(),
            resolved_file_version: String::new(),
            review_id: String::new(),
            edited_at: String::new(),
        };
        assert_eq!(
            Removed::from_comment(&comment),
            Removed {
                id: "r4".into(),
                file: "src/a.rs".into(),
                lines: (7, 9),
                text: "rename this".into(),
                points_at: Some(Location {
                    file: "src/service.rs".into(),
                    start_line: 90,
                    end_line: 96,
                }),
            }
        );
    }
}
