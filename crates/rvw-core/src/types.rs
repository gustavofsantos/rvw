//! The domain of rvw: a per-workspace queue of review comments, raised by a
//! human or an agent and pulled by whichever agent does the work.
//!
//! The JSON shape of [`Comment`], [`Review`] and [`Handoff`] is the public
//! contract read by editors and agents: nullable fields are always present
//! (`null`), optional ones are omitted when empty. Field order is output order.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Where a comment stands: pending → pulled → done | rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Pending,
    Pulled,
    Done,
    Rejected,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Pending => "pending",
            Status::Pulled => "pulled",
            Status::Done => "done",
            Status::Rejected => "rejected",
        }
    }
    pub fn parse(s: &str) -> Option<Status> {
        Some(match s {
            "pending" => Status::Pending,
            "pulled" => Status::Pulled,
            "done" => Status::Done,
            "rejected" => Status::Rejected,
            _ => return None,
        })
    }
    /// Whether the comment carries a final decision.
    pub fn resolved(self) -> bool {
        matches!(self, Status::Done | Status::Rejected)
    }
}

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Every accepted status filter, in help order. Inputs carry the filter as a
/// plain string so a bad value yields `status "x" is not one of ...`.
pub const STATUS_FILTERS: [&str; 6] = ["pending", "pulled", "done", "rejected", "open", "all"];

/// Expands a status filter ("" means pending) to the statuses it matches.
pub fn statuses_of(filter: &str) -> Result<Vec<Status>> {
    Ok(match filter {
        "" | "pending" => vec![Status::Pending],
        "pulled" => vec![Status::Pulled],
        "done" => vec![Status::Done],
        "rejected" => vec![Status::Rejected],
        "open" => vec![Status::Pending, Status::Pulled],
        "all" => vec![
            Status::Pending,
            Status::Pulled,
            Status::Done,
            Status::Rejected,
        ],
        other => {
            return Err(Error::invalid(format!(
                "status \"{other}\" is not one of {}",
                STATUS_FILTERS.join(", ")
            )));
        }
    })
}

/// The verdicts a submitted review may carry.
pub const DECISIONS: [&str; 3] = ["comment", "approve", "request-changes"];

pub fn validate_decision(d: &str) -> Result<()> {
    if DECISIONS.contains(&d) {
        Ok(())
    } else {
        Err(Error::invalid(format!(
            "decision \"{d}\" is not one of {}",
            DECISIONS.join(", ")
        )))
    }
}

/// The decision recorded on a single comment: `done` or `rejected`.
pub const OUTCOME_DONE: &str = "done";
pub const OUTCOME_REJECTED: &str = "rejected";

/// One note on a line range of one file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Comment {
    /// stable id, e.g. r3; never reused within a workspace
    pub id: String,
    #[schemars(with = "String")]
    /// pending, pulled, done or rejected
    pub status: Status,
    /// absolute path of the workspace the comment belongs to
    pub workspace: String,
    #[schemars(required, schema_with = "nullable_string")]
    /// branch or lane the comment belongs to, null when unscoped
    pub lane: Option<String>,
    #[schemars(required, schema_with = "nullable_string")]
    /// who raised the comment
    pub author: Option<String>,
    /// path relative to the workspace (absolute only on comments recorded outside it by older versions)
    pub file: String,
    /// absolute path of the file
    pub path: String,
    /// first line of the range, 1-indexed
    pub start_line: i64,
    /// last line of the range, inclusive
    pub end_line: i64,
    /// language of the code snapshot, for fencing
    pub filetype: String,
    /// the reviewed lines as they stood when the comment was written
    pub code: String,
    /// the reviewer's note
    pub comment: String,
    /// RFC 3339 UTC timestamp
    pub created_at: String,

    #[schemars(required, schema_with = "nullable_string")]
    /// when the comment was handed over
    pub pulled_at: Option<String>,
    #[schemars(required, schema_with = "nullable_string")]
    /// when a decision was recorded
    pub resolved_at: Option<String>,
    #[schemars(required, schema_with = "nullable_string")]
    /// who recorded the decision
    pub resolved_by: Option<String>,
    #[schemars(required, schema_with = "nullable_string")]
    /// what was done, or why not
    pub resolution_note: Option<String>,
    #[schemars(required, schema_with = "nullable_location")]
    /// another file range this comment refers to, null when it points nowhere
    pub points_at: Option<Location>,
    #[schemars(required, schema_with = "nullable_string")]
    /// id of the decided comment this one reopens, null when it reopens none
    pub reopens: Option<String>,
    #[schemars(required, schema_with = "nullable_verdict")]
    /// the reviewer's judgement of the decision, null until one is given
    pub verdict: Option<Verdict>,

    #[serde(default, skip_serializing_if = "String::is_empty")]
    /// git blob of the whole file as reviewed
    pub file_version: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    /// git blob of the whole file when resolved
    pub resolved_file_version: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    /// submitted review this comment is linked to
    pub review_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    /// when the note was last edited
    pub edited_at: String,
}

impl Comment {
    /// Whether the comment still waits for an agent: pending or pulled.
    pub fn is_open(&self) -> bool {
        matches!(self.status, Status::Pending | Status::Pulled)
    }

    /// Whether a reviewer still owes a verdict: an agent decided it after a
    /// handoff and nobody has judged that decision. A comment its own author
    /// decided, and a pending comment withdrawn before any handoff, never wait.
    pub fn awaits_verdict(&self) -> bool {
        self.status.resolved()
            && self.pulled_at.is_some()
            && self.verdict.is_none()
            && !(self.author.is_some() && self.author == self.resolved_by)
    }

    /// `file:N` or `file:N-M`.
    pub fn location(&self) -> String {
        if self.start_line == self.end_line {
            format!("{}:{}", self.file, self.start_line)
        } else {
            format!("{}:{}-{}", self.file, self.start_line, self.end_line)
        }
    }
}

/// A reviewer's judgement of a decision: accepted, or reopened as a follow-up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum VerdictKind {
    Accepted,
    Reopened,
}

impl VerdictKind {
    pub fn as_str(self) -> &'static str {
        match self {
            VerdictKind::Accepted => "accepted",
            VerdictKind::Reopened => "reopened",
        }
    }
    pub fn parse(s: &str) -> Option<VerdictKind> {
        Some(match s {
            "accepted" => VerdictKind::Accepted,
            "reopened" => VerdictKind::Reopened,
            _ => return None,
        })
    }
}

/// The verdicts a reviewer may give.
pub const VERDICTS: [&str; 2] = ["accepted", "reopened"];

/// The kind of party behind a write: the surface it came through, not an
/// authenticated identity. MCP is an agent; the CLI, TUI and GUI are a human.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Actor {
    Human,
    Agent,
}

impl Actor {
    pub fn as_str(self) -> &'static str {
        match self {
            Actor::Human => "human",
            Actor::Agent => "agent",
        }
    }
    pub fn parse(s: &str) -> Option<Actor> {
        Some(match s {
            "human" => Actor::Human,
            "agent" => Actor::Agent,
            _ => return None,
        })
    }
}

/// A reviewer's verdict on a decided comment. Final once given.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Verdict {
    #[schemars(with = "String")]
    /// accepted or reopened
    pub kind: VerdictKind,
    /// who gave the verdict
    pub by: String,
    #[schemars(with = "String")]
    /// human or agent, by the surface the verdict came through
    pub actor: Actor,
    /// RFC 3339 UTC timestamp
    pub at: String,
    #[schemars(required, schema_with = "nullable_string")]
    /// the reviewer's note; the reason a comment was reopened
    pub note: Option<String>,
    #[schemars(required, schema_with = "nullable_string")]
    /// id of the follow-up comment a reopen created
    pub followup: Option<String>,
}

/// A file range, as another comment's "points at" reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Location {
    /// path relative to the workspace
    pub file: String,
    /// first line of the range, 1-indexed
    pub start_line: i64,
    /// last line of the range, inclusive
    pub end_line: i64,
}

/// The actions the commit gate guards.
pub const GATE_ACTIONS: [&str; 2] = ["commit", "push"];

/// A recorded decision to go ahead past the gate, with the reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct GateOverride {
    /// absolute workspace path
    pub workspace: String,
    #[schemars(required, schema_with = "nullable_string")]
    /// the lane the gate looked at, null for every lane
    pub lane: Option<String>,
    /// commit or push
    pub action: String,
    /// why the action went ahead
    pub reason: String,
    /// who overrode the gate
    pub by: String,
    #[schemars(with = "String")]
    /// human or agent
    pub actor: Actor,
    /// RFC 3339 UTC timestamp
    pub at: String,
    /// open comment ids when the gate was overridden
    pub open: Vec<String>,
    /// decided comments still waiting for a verdict
    pub unjudged: Vec<String>,
}

/// Where a submitted review stands; its comments carry their own resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ReviewStatus {
    Pending,
    Pulled,
}

impl ReviewStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ReviewStatus::Pending => "pending",
            ReviewStatus::Pulled => "pulled",
        }
    }
}

/// Groups zero or more comments under one decision and summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Review {
    /// stable id, e.g. rv2
    pub id: String,
    #[schemars(with = "String")]
    /// pending or pulled
    pub status: ReviewStatus,
    /// absolute path of the workspace
    pub workspace: String,
    #[schemars(required, schema_with = "nullable_string")]
    /// lane the review belongs to
    pub lane: Option<String>,
    #[schemars(required, schema_with = "nullable_string")]
    /// who submitted the review
    pub author: Option<String>,
    /// comment, approve or request-changes
    pub decision: String,
    /// the reviewer's overall assessment
    pub summary: String,
    /// linked comment ids, in review order
    pub comment_ids: Vec<String>,
    /// RFC 3339 UTC timestamp
    pub created_at: String,
    #[schemars(required, schema_with = "nullable_string")]
    /// when the review was handed over
    pub pulled_at: Option<String>,
}

/// A review as it is pulled: the review plus its linked comments that still exist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Handoff {
    #[serde(flatten)]
    pub review: Review,
    /// the linked comments that still exist, in review order
    pub comments: Vec<Comment>,
}

/// The schema of a nullable string the Go contract always lists as required:
/// present, and `null` when unset.
fn nullable_string(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({ "type": ["string", "null"] })
}

/// `Location` or `null`: the schema of `points_at`, which is present on every
/// comment and `null` when it points nowhere.
fn nullable_location(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({ "anyOf": [generator.subschema_for::<Location>(), { "type": "null" }] })
}

/// `Verdict` or `null`: the schema of `verdict`, which is present on every
/// comment and `null` until a reviewer judges the decision.
fn nullable_verdict(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({ "anyOf": [generator.subschema_for::<Verdict>(), { "type": "null" }] })
}

// ── ids ──────────────────────────────────────────────────────────────────────

pub fn comment_id(seq: i64) -> String {
    format!("r{seq}")
}

pub fn review_id(seq: i64) -> String {
    format!("rv{seq}")
}

/// Whether `id` names a submitted review rather than a comment.
pub fn is_review_id(id: &str) -> bool {
    id.starts_with("rv")
}

/// Reads the number out of an id like `r12` or `rv3`; the prefix must match
/// exactly, so `r12` is not a review and `rv3` is not a comment.
pub fn parse_seq(id: &str, prefix: &str) -> Option<i64> {
    let digits = id.strip_prefix(prefix)?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

// ── line ranges ──────────────────────────────────────────────────────────────

/// A 1-indexed inclusive range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct LineRange {
    pub start_line: i64,
    pub end_line: i64,
}

impl LineRange {
    /// Accepts `N` or `N-M` (also `N:M`, `N,M`), swapping a reversed range.
    pub fn parse(spec: &str) -> Result<LineRange> {
        let bad = || Error::invalid(format!("--lines '{spec}' is not N or N-M"));
        let s = spec.trim();
        let (first, second) = match s.find(['-', ':', ',']) {
            Some(i) => (s[..i].trim(), Some(s[i + 1..].trim())),
            None => (s, None),
        };
        let start = parse_line(first).ok_or_else(bad)?;
        let end = match second {
            Some(t) => parse_line(t).ok_or_else(bad)?,
            None => start,
        };
        LineRange {
            start_line: start,
            end_line: end,
        }
        .normalized()
    }

    pub fn normalized(mut self) -> Result<LineRange> {
        if self.start_line < 1 || self.end_line < 1 {
            return Err(Error::invalid("--lines is 1-indexed; N must be >= 1"));
        }
        if self.start_line > self.end_line {
            std::mem::swap(&mut self.start_line, &mut self.end_line);
        }
        Ok(self)
    }

    /// `N` or `N-M`.
    pub fn label(&self) -> String {
        if self.start_line == self.end_line {
            self.start_line.to_string()
        } else {
            format!("{}-{}", self.start_line, self.end_line)
        }
    }
}

fn parse_line(s: &str) -> Option<i64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}
