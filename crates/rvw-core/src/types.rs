//! The domain of rvw: a per-workspace queue of review comments, raised by a
//! human or an agent and pulled by whichever agent does the work.
//!
//! The JSON shape of [`Comment`], [`Review`] and [`Handoff`] is the public
//! contract read by editors and agents: nullable fields are always present
//! (`null`), optional ones are omitted when empty. Field order is output order.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Where a comment stands: pending → pulled → done | rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
            )))
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comment {
    pub id: String,
    pub status: Status,
    pub workspace: String,
    pub lane: Option<String>,
    pub author: Option<String>,
    pub file: String,
    pub path: String,
    pub start_line: i64,
    pub end_line: i64,
    pub filetype: String,
    pub code: String,
    pub comment: String,
    pub created_at: String,

    pub pulled_at: Option<String>,
    pub resolved_at: Option<String>,
    pub resolved_by: Option<String>,
    pub resolution_note: Option<String>,

    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub file_version: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub resolved_file_version: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub review_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub edited_at: String,
}

impl Comment {
    /// `file:N` or `file:N-M`.
    pub fn location(&self) -> String {
        if self.start_line == self.end_line {
            format!("{}:{}", self.file, self.start_line)
        } else {
            format!("{}:{}-{}", self.file, self.start_line, self.end_line)
        }
    }
}

/// Where a submitted review stands; its comments carry their own resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Review {
    pub id: String,
    pub status: ReviewStatus,
    pub workspace: String,
    pub lane: Option<String>,
    pub author: Option<String>,
    pub decision: String,
    pub summary: String,
    pub comment_ids: Vec<String>,
    pub created_at: String,
    pub pulled_at: Option<String>,
}

/// A review as it is pulled: the review plus its linked comments that still exist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Handoff {
    #[serde(flatten)]
    pub review: Review,
    pub comments: Vec<Comment>,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
