//! Inputs and outputs of every `Service` operation. Each one is a JSON object so
//! it can double as an MCP tool's input or structured output. A field without
//! `skip_serializing_if` is required; its doc comment is its description.
//!
//! `workspace` is always the canonical absolute path an adapter resolved. Lane
//! "" means every lane; author "" means anonymous.

use serde::{Deserialize, Serialize};

use crate::types::{Comment, Handoff, Location, Review};

#[allow(clippy::trivially_copy_pass_by_ref)] // serde's skip_serializing_if takes a reference
fn is_false(b: &bool) -> bool {
    !*b
}
#[allow(clippy::trivially_copy_pass_by_ref)] // serde's skip_serializing_if takes a reference
fn is_zero(n: &i64) -> bool {
    *n == 0
}

/// Enqueues one comment on a line range of one file.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AddInput {
    /// absolute workspace path
    pub workspace: String,
    /// file the comment is about, inside the workspace; absolute or relative to it
    pub file: String,
    /// first line of the range, 1-indexed
    pub start_line: i64,
    /// last line of the range, inclusive; defaults to start_line
    #[allow(clippy::doc_markdown)] // the description is the Go contract text, verbatim
    #[serde(default, skip_serializing_if = "is_zero")]
    pub end_line: i64,
    /// the review note, markdown allowed
    pub comment: String,
    /// whole-file content to snapshot instead of reading the file from disk (e.g. an unsaved editor buffer)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// language for fencing the code; guessed from the extension when empty
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub filetype: String,
    /// branch or lane the comment belongs to
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub lane: String,
    /// who raises the comment
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub author: String,
    /// another file range this comment refers to, e.g. the changed code a note on an unchanged file is about
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub points_at: Option<Location>,
}

/// Groups pending comments under one review.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SubmitInput {
    /// absolute workspace path
    pub workspace: String,
    /// comment, approve or request-changes
    pub decision: String,
    /// overall assessment
    pub summary: String,
    /// comments to link, in order; empty takes this author's pending comments in this lane
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub comment_ids: Vec<String>,
    /// submit a summary-only review, linking nothing
    #[serde(default, skip_serializing_if = "is_false")]
    pub no_comments: bool,
    /// lane of the review
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub lane: String,
    /// reviewer
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub author: String,
}

/// Scopes a read of the queue.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct QueryInput {
    /// absolute workspace path
    pub workspace: String,
    /// pending (default), pulled, done, rejected, open or all
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub status: String,
    /// only comments on this file, absolute or relative to the workspace
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub file: String,
    /// only this lane's comments; empty means every lane
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub lane: String,
}

/// The comments matching a [`QueryInput`], oldest first.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ListOutput {
    /// absolute workspace path
    pub workspace: String,
    /// matching comments, oldest first
    pub comments: Vec<Comment>,
}

/// Counts pending handoffs: a submitted review counts once, with its linked
/// comments; for any other status it counts comments.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CountOutput {
    /// absolute workspace path
    pub workspace: String,
    /// number of handoffs (pending) or comments (other statuses)
    pub count: i64,
}

/// Dequeues handoffs for an agent to act on.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PullInput {
    /// absolute workspace path
    pub workspace: String,
    /// only this lane; a pinned pull never takes another lane's or an unlaned comment
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub lane: String,
    /// only handoffs touching this file
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub file: String,
    /// only these comment or review ids; a linked comment brings its whole review
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ids: Vec<String>,
    /// at most this many handoffs, oldest first; 0 means no limit
    #[serde(default, skip_serializing_if = "is_zero")]
    pub limit: i64,
    /// read without dequeuing
    #[serde(default, skip_serializing_if = "is_false")]
    pub peek: bool,
}

/// What was handed over.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PullOutput {
    /// absolute workspace path
    pub workspace: String,
    /// true when the handoffs left the queue (not a peek)
    pub drained: bool,
    /// submitted reviews, each with its linked comments
    pub reviews: Vec<Handoff>,
    /// standalone comments
    pub comments: Vec<Comment>,
    /// handoffs waiting in other lanes, when this pinned pull took nothing
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elsewhere: Option<Elsewhere>,
}

impl PullOutput {
    /// The number of handoffs taken.
    pub fn count(&self) -> usize {
        self.reviews.len() + self.comments.len()
    }
}

/// Counts handoffs pending outside the pinned lane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Elsewhere {
    /// handoffs pending in other lanes
    pub count: i64,
    /// those lanes, sorted; (no lane) for unlaned comments
    pub lanes: Vec<String>,
}

/// Records what became of a comment.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ResolveInput {
    /// absolute workspace path
    pub workspace: String,
    /// comment id, e.g. r3
    pub id: String,
    /// done, or rejected (which needs a note)
    pub outcome: String,
    /// what was done, or why not
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// who decides
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub author: String,
}

/// Names one comment or review.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct GetInput {
    /// absolute workspace path
    pub workspace: String,
    /// comment id (r3) or review id (rv2)
    pub id: String,
}

/// A comment with what changed while resolving it.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Evidence {
    pub comment: Comment,
    /// unified diff lines between the reviewed and resolved file versions
    pub diff: Vec<String>,
    /// whether both file snapshots exist
    pub diff_available: bool,
}

/// pending, pulled, or complete once every linked comment is decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SheetState {
    Pending,
    Pulled,
    Complete,
}

impl SheetState {
    pub fn as_str(self) -> &'static str {
        match self {
            SheetState::Pending => "pending",
            SheetState::Pulled => "pulled",
            SheetState::Complete => "complete",
        }
    }
}

/// A submitted review with the comments it links.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ReviewSheet {
    pub review: Review,
    /// pending, pulled, or complete once every linked comment is decided
    #[schemars(with = "String")]
    pub state: SheetState,
    /// linked comments that still exist, in review order
    pub comments: Vec<Comment>,
}

/// Every accepted sheet filter, in help order.
pub const SHEET_FILTERS: [&str; 5] = ["pending", "pulled", "complete", "open", "all"];

/// Scopes a read of submitted reviews.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SheetsInput {
    /// absolute workspace path
    pub workspace: String,
    /// pending (default), pulled, complete, open or all
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub status: String,
    /// only reviews linking a comment on this file, absolute or relative to the workspace
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub file: String,
    /// only this lane's reviews; empty means every lane
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub lane: String,
}

/// The review sheets matching a [`SheetsInput`], oldest first.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SheetsOutput {
    /// absolute workspace path
    pub workspace: String,
    /// number of matching review sheets
    pub count: i64,
    /// matching review sheets, oldest first
    pub sheets: Vec<ReviewSheet>,
}

/// Replaces a comment's text.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct EditInput {
    /// absolute workspace path
    pub workspace: String,
    /// comment id
    pub id: String,
    /// new text
    pub comment: String,
}

/// Deletes a comment from the queue before it is handed over.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RemoveInput {
    /// absolute workspace path
    pub workspace: String,
    /// comment id
    pub id: String,
}

/// Lists workspaces in the store.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkspacesInput {
    /// include workspaces with nothing pending
    #[serde(default, skip_serializing_if = "is_false")]
    pub all: bool,
}

/// Every workspace holding handoffs.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkspacesOutput {
    pub workspaces: Vec<WorkspaceSummary>,
}

/// One workspace's queue at a glance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkspaceSummary {
    /// absolute workspace path
    pub workspace: String,
    /// pending handoffs
    pub pending: i64,
    /// comments handed over and not yet decided
    pub pulled: i64,
}

/// Judges a decided comment: accepts the decision, or reopens the comment.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct VerdictInput {
    /// absolute workspace path
    pub workspace: String,
    /// comment id, e.g. r3, decided as done or rejected after a handoff
    pub id: String,
    /// accepted, or reopened (which needs a note)
    pub verdict: String,
    /// accept: an optional remark; reopen: what is still wrong, which becomes the follow-up comment's text
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// reopen only: first line of the follow-up's range; 0 keeps the original range
    #[serde(default, skip_serializing_if = "is_zero")]
    pub start_line: i64,
    /// reopen only: last line of the follow-up's range; 0 keeps the original range
    #[serde(default, skip_serializing_if = "is_zero")]
    pub end_line: i64,
    /// who judges; must not be the one who decided the comment
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub author: String,
}

/// The judged comment, and the follow-up a reopen created.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct VerdictOutput {
    /// the comment with its verdict
    pub comment: Comment,
    /// the new pending comment a reopen created, null for an accept
    #[schemars(required, schema_with = "nullable_comment_schema")]
    pub followup: Option<Comment>,
}

/// Asks what stands between the workspace and a commit or push.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct GateInput {
    /// absolute workspace path
    pub workspace: String,
    /// only this lane's comments; empty means every lane
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub lane: String,
}

/// Comments that still need a decision or a verdict.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct GateOutput {
    /// absolute workspace path
    pub workspace: String,
    /// the lane looked at, null for every lane
    #[schemars(required, schema_with = "nullable_string_schema")]
    pub lane: Option<String>,
    /// pending or pulled comment ids (drafts included)
    pub open: Vec<String>,
    /// decided comment ids still waiting for a verdict
    pub unjudged: Vec<String>,
    /// true when nothing is open or unjudged
    pub clear: bool,
}

/// Records going ahead with an action although the gate was not clear.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct OverrideInput {
    /// absolute workspace path
    pub workspace: String,
    /// commit or push
    pub action: String,
    /// why the action goes ahead
    pub reason: String,
    /// only this lane's comments; empty means every lane
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub lane: String,
    /// who overrides
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub author: String,
}

fn nullable_comment_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({ "anyOf": [generator.subschema_for::<Comment>(), { "type": "null" }] })
}

fn nullable_string_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({ "type": ["string", "null"] })
}
