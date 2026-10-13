//! The storage the service runs on. Every read-modify-write goes through one
//! `update`, so concurrent editors and agents serialize on it.

use crate::error::Result;
use crate::types::{Comment, Review, ReviewStatus, Status};

pub trait Repository: Send + Sync {
    /// Runs `f` in one write transaction on the workspace, creating the
    /// workspace record when it does not exist yet. An `Err` rolls back.
    fn update(&self, workspace: &str, f: &mut dyn FnMut(&mut dyn Tx) -> Result<()>) -> Result<()>;
    /// Runs `f` in one transaction on the workspace without creating it; an
    /// unknown workspace reads as empty.
    fn view(&self, workspace: &str, f: &mut dyn FnMut(&mut dyn Tx) -> Result<()>) -> Result<()>;
    /// Every workspace ever recorded, ordered by path.
    fn workspaces(&self) -> Result<Vec<String>>;
    /// Where the data lives, for humans.
    fn location(&self) -> String;
}

/// The per-workspace view of the store inside one transaction. Comments and
/// reviews come back in creation order with `review_id` / `comment_ids` filled in.
pub trait Tx {
    fn next_comment_seq(&mut self) -> Result<i64>;
    fn next_review_seq(&mut self) -> Result<i64>;

    /// Empty `statuses` selects every status.
    fn comments(&mut self, statuses: &[Status]) -> Result<Vec<Comment>>;
    /// `Ok(None)` for an id that is malformed or unknown.
    fn comment(&mut self, id: &str) -> Result<Option<Comment>>;
    fn insert_comment(&mut self, c: &Comment) -> Result<()>;
    /// Rewrites the mutable part of a comment: lifecycle and text.
    fn update_comment(&mut self, c: &Comment) -> Result<()>;
    /// Deletes a comment row. Its id is never issued again: the sequence
    /// only moves forward.
    fn delete_comment(&mut self, id: &str) -> Result<()>;

    /// Empty `statuses` selects every status.
    fn reviews(&mut self, statuses: &[ReviewStatus]) -> Result<Vec<Review>>;
    fn review(&mut self, id: &str) -> Result<Option<Review>>;
    /// Stores the review and links its `comment_ids` in order.
    fn insert_review(&mut self, r: &Review) -> Result<()>;
    /// Rewrites a review's lifecycle. Its links never change.
    fn update_review(&mut self, r: &Review) -> Result<()>;
}
