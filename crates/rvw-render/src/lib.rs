//! Service outputs as text, markdown, JSON and fixed-width sheets.
//! Port of internal/render.

pub mod display;
pub mod render;
pub mod terminal;

pub use display::{display, sheet};
pub use render::{
    Envelope, attribution, comment_line, first_line, ids, json, markdown, review_line,
    session_line, sessions_markdown, sessions_text, sheet_line, text,
};
pub use terminal::{Terminal, is_control, safe};

#[cfg(test)]
pub(crate) mod fixtures {
    //! Builders shared by the unit tests.

    use rvw_core::types::{Actor, Comment, Review, ReviewStatus, Status, Verdict, VerdictKind};

    pub fn comment(
        id: &str,
        status: Status,
        file: &str,
        start: i64,
        end: i64,
        code: &str,
        note: &str,
    ) -> Comment {
        Comment {
            points_at: None,
            reopens: None,
            verdict: None,
            id: id.into(),
            status,
            workspace: "/ws".into(),
            lane: None,
            author: None,
            file: file.into(),
            path: format!("/ws/{file}"),
            start_line: start,
            end_line: end,
            filetype: String::new(),
            code: code.into(),
            comment: note.into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            pulled_at: None,
            resolved_at: None,
            resolved_by: None,
            resolution_note: None,
            file_version: String::new(),
            resolved_file_version: String::new(),
            review_id: String::new(),
            edited_at: String::new(),
        }
    }

    /// A verdict by a human, at a fixed time.
    pub fn verdict(kind: VerdictKind, note: Option<&str>, followup: Option<&str>) -> Verdict {
        Verdict {
            kind,
            by: "alice".into(),
            actor: Actor::Human,
            at: "2026-01-03T00:00:00Z".into(),
            note: note.map(str::to_owned),
            followup: followup.map(str::to_owned),
        }
    }

    pub fn review(id: &str, decision: &str, summary: &str, comment_ids: &[&str]) -> Review {
        Review {
            id: id.into(),
            status: ReviewStatus::Pending,
            workspace: "/ws".into(),
            lane: None,
            author: None,
            decision: decision.into(),
            summary: summary.into(),
            comment_ids: comment_ids.iter().map(|s| (*s).to_owned()).collect(),
            created_at: "2026-01-01T00:00:00Z".into(),
            pulled_at: None,
        }
    }
}
