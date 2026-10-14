//! The commit gate: what is open or unjudged, and recorded overrides.

use crate::types::Comment;

/// The comment ids a lane has open and the ones still awaiting a verdict.
/// Lane scoping is strict: a lane never sees another lane's or an unlaned
/// comment; "" means every lane. Input order is kept.
#[must_use]
pub fn undecided(comments: &[Comment], lane: &str) -> (Vec<String>, Vec<String>) {
    let _ = (comments, lane);
    todo!("slice 4")
}
