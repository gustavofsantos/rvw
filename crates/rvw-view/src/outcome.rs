//! The outcome of one comment as the outcome screen shows it: the story of the
//! comment in four steps, and the timeline of its moments. No drawing and no
//! I/O; the times are shown in short form.

use std::collections::HashSet;

use rvw_core::operations::Evidence;
use rvw_core::types::{Comment, Status};

use crate::diff::{self, FileDiff};
use crate::queue::{self, Filter, Group, Queue};

/// What the outcome screen shows of one comment and its resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Story {
    /// comment id, e.g. "r9"
    pub id: String,
    /// first line of the text, cut like a queue title
    pub title: String,
    /// `file:N` or `file:N-M`
    pub location: String,
    pub status: Status,
    /// lane, or "" when none
    pub lane: String,
    /// author, or "someone" when none
    pub asked_by: String,
    /// creation time in short form, "HH:MM"
    pub asked_at: String,
    /// the comment text, whole
    pub text: String,
    /// the code as it was reviewed, lines `start_line..=end_line`
    pub code: String,
    /// the first line number of `code`
    pub start_line: i64,
    /// who recorded the decision, or "an agent" when none
    pub resolved_by: String,
    /// decision time in short form, or "" when not decided
    pub resolved_at: String,
    /// the agent's note, or ""
    pub note: String,
    /// what changed while resolving; None when no diff was kept
    pub diff: Option<FileDiff>,
}

/// One moment in a comment's life, as a timeline row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Moment {
    pub glyph: &'static str,
    pub label: String,
    /// time in short form, or "" when the moment has none
    pub at: String,
}

/// The story of the comment in `ev`, with its resolution diff parsed.
pub fn story(ev: &Evidence) -> Story {
    let c = &ev.comment;
    let diff = (ev.diff_available && !ev.diff.is_empty()).then(|| diff::parse(&ev.diff.join("\n")));
    Story {
        id: c.id.clone(),
        title: title_of(c),
        location: c.location(),
        status: c.status,
        lane: c.lane.clone().unwrap_or_default(),
        asked_by: named(c.author.as_deref(), "someone"),
        asked_at: short_time(&c.created_at),
        text: c.comment.clone(),
        code: c.code.clone(),
        start_line: c.start_line,
        resolved_by: named(c.resolved_by.as_deref(), "an agent"),
        resolved_at: c.resolved_at.as_deref().map(short_time).unwrap_or_default(),
        note: c.resolution_note.clone().unwrap_or_default(),
        diff,
    }
}

/// The moments of a comment, oldest first: raised, pulled when it was, then
/// done or rejected. A comment not yet decided has no decision moment.
pub fn timeline(c: &Comment) -> Vec<Moment> {
    let mut moments = vec![Moment {
        glyph: "○",
        label: format!("raised by {}", named(c.author.as_deref(), "someone")),
        at: short_time(&c.created_at),
    }];
    if let Some(at) = c.pulled_at.as_deref() {
        moments.push(Moment {
            glyph: "◐",
            label: "pulled by an agent".into(),
            at: short_time(at),
        });
    }
    let resolved_at = c.resolved_at.as_deref().map(short_time).unwrap_or_default();
    let who = named(c.resolved_by.as_deref(), "an agent");
    match c.status {
        Status::Done => moments.push(Moment {
            glyph: "✓",
            label: format!("done by {who}"),
            at: resolved_at,
        }),
        Status::Rejected => moments.push(Moment {
            glyph: "✗",
            label: format!("rejected by {who}"),
            at: resolved_at,
        }),
        Status::Pending | Status::Pulled => {}
    }
    moments
}

/// The time as "HH:MM" when `at` has an ISO-8601 time part, such as
/// "2026-10-12T10:41:00+00:00"; otherwise `at` unchanged.
#[must_use]
pub fn short_time(at: &str) -> String {
    at.split_once('T')
        .and_then(|(_, time)| time.get(..5))
        .filter(|hm| is_hh_mm(hm))
        .map_or_else(|| at.to_owned(), str::to_owned)
}

fn is_hh_mm(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 5 && bytes[2] == b':' && [0, 1, 3, 4].iter().all(|&i| bytes[i].is_ascii_digit())
}

/// The queue group that holds `id`: its review's sheet, or the loose comments.
pub fn group_of<'a>(queue: &'a Queue, id: &str) -> Option<&'a Group> {
    queue
        .groups
        .iter()
        .find(|group| group.rows.iter().any(|row| row.id == id))
}

/// The name, or `fallback` when the name is absent or blank.
fn named(name: Option<&str>, fallback: &str) -> String {
    name.map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or(fallback)
        .to_owned()
}

/// The title the queue shows for this comment. The queue builds its rows
/// from comments, so one comment alone gives the same title.
fn title_of(c: &Comment) -> String {
    queue::build(
        std::slice::from_ref(c),
        &[],
        &HashSet::<String>::new(),
        &Filter::default(),
    )
    .groups
    .into_iter()
    .next()
    .and_then(|group| group.rows.into_iter().next())
    .map_or_else(|| "(empty)".to_owned(), |row| row.title)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comment(status: Status) -> Comment {
        Comment {
            id: "r9".into(),
            status,
            workspace: "/ws".into(),
            lane: Some("feat".into()),
            author: Some("gustavo".into()),
            file: "src/tx.rs".into(),
            path: "/ws/src/tx.rs".into(),
            start_line: 40,
            end_line: 41,
            filetype: "rust".into(),
            code: "fn a() {}\nfn b() {}".into(),
            comment: "stamp every row\nwith one instant".into(),
            created_at: "2026-10-12T10:41:00+00:00".into(),
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

    fn evidence(c: Comment, diff: &[&str], available: bool) -> Evidence {
        Evidence {
            comment: c,
            diff: diff.iter().map(ToString::to_string).collect(),
            diff_available: available,
        }
    }

    #[test]
    fn short_time_keeps_hours_and_minutes() {
        assert_eq!(short_time("2026-10-12T10:41:00+00:00"), "10:41");
        assert_eq!(short_time("2026-10-12T09:05:59Z"), "09:05");
    }

    #[test]
    fn short_time_returns_unparsed_input_unchanged() {
        assert_eq!(short_time("yesterday"), "yesterday");
        assert_eq!(short_time("2026-10-12T1041"), "2026-10-12T1041");
        assert_eq!(short_time("2026-10-12T1a:41:00"), "2026-10-12T1a:41:00");
        assert_eq!(short_time(""), "");
    }

    #[test]
    fn story_takes_the_comment_fields() {
        let mut c = comment(Status::Done);
        c.resolved_by = Some("claude".into());
        c.resolved_at = Some("2026-10-12T11:03:00+00:00".into());
        c.resolution_note = Some("Added a now parameter.".into());
        let s = story(&evidence(c, &[], false));
        assert_eq!(s.id, "r9");
        assert_eq!(s.title, "stamp every row");
        assert_eq!(s.location, "src/tx.rs:40-41");
        assert_eq!(s.status, Status::Done);
        assert_eq!(s.lane, "feat");
        assert_eq!(s.asked_by, "gustavo");
        assert_eq!(s.asked_at, "10:41");
        assert_eq!(s.text, "stamp every row\nwith one instant");
        assert_eq!(s.code, "fn a() {}\nfn b() {}");
        assert_eq!(s.start_line, 40);
        assert_eq!(s.resolved_by, "claude");
        assert_eq!(s.resolved_at, "11:03");
        assert_eq!(s.note, "Added a now parameter.");
        assert_eq!(s.diff, None);
    }

    #[test]
    fn story_names_missing_people_and_leaves_missing_text_empty() {
        let mut c = comment(Status::Pending);
        c.author = None;
        c.lane = None;
        let s = story(&evidence(c, &[], false));
        assert_eq!(s.asked_by, "someone");
        assert_eq!(s.lane, "");
        assert_eq!(s.resolved_by, "an agent");
        assert_eq!(s.resolved_at, "");
        assert_eq!(s.note, "");
    }

    #[test]
    fn story_parses_the_diff_when_one_was_kept() {
        let ev = evidence(
            comment(Status::Done),
            &["@@ -1,1 +1,1 @@", "-let at = 1;", "+let at = 2;"],
            true,
        );
        let diff = story(&ev).diff.expect("a diff");
        assert_eq!((diff.added, diff.removed), (1, 1));
        assert_eq!(diff.hunks.len(), 1);
    }

    #[test]
    fn story_has_no_diff_when_unavailable_or_empty() {
        let unavailable = evidence(comment(Status::Done), &["+x"], false);
        assert_eq!(story(&unavailable).diff, None);
        let empty = evidence(comment(Status::Done), &[], true);
        assert_eq!(story(&empty).diff, None);
    }

    #[test]
    fn story_title_is_cut_like_the_queue_title() {
        let mut c = comment(Status::Pending);
        c.comment = format!("\n  {}\nsecond", "x".repeat(70));
        let title = story(&evidence(c, &[], false)).title;
        assert_eq!(title.chars().count(), 60);
        assert!(title.ends_with('…'));
    }

    #[test]
    fn timeline_of_a_done_comment_has_four_moments_in_order() {
        let mut c = comment(Status::Done);
        c.pulled_at = Some("2026-10-12T10:52:00+00:00".into());
        c.resolved_at = Some("2026-10-12T11:03:00+00:00".into());
        c.resolved_by = Some("claude".into());
        let moments = timeline(&c);
        let labels: Vec<(&str, &str, &str)> = moments
            .iter()
            .map(|m| (m.glyph, m.label.as_str(), m.at.as_str()))
            .collect();
        assert_eq!(
            labels,
            vec![
                ("○", "raised by gustavo", "10:41"),
                ("◐", "pulled by an agent", "10:52"),
                ("✓", "done by claude", "11:03"),
            ]
        );
    }

    #[test]
    fn timeline_of_a_rejected_comment_ends_with_the_rejection() {
        let mut c = comment(Status::Rejected);
        c.resolved_at = Some("2026-10-12T11:05:00+00:00".into());
        let moments = timeline(&c);
        let last = moments.last().expect("a last moment");
        assert_eq!(last.glyph, "✗");
        assert_eq!(last.label, "rejected by an agent");
        assert_eq!(last.at, "11:05");
        assert_eq!(moments.len(), 2);
    }

    #[test]
    fn timeline_of_a_pending_comment_is_only_raised() {
        let c = comment(Status::Pending);
        let moments = timeline(&c);
        assert_eq!(moments.len(), 1);
        assert_eq!(moments[0].glyph, "○");
    }

    #[test]
    fn timeline_names_an_unknown_author() {
        let mut c = comment(Status::Pending);
        c.author = None;
        assert_eq!(timeline(&c)[0].label, "raised by someone");
    }

    #[test]
    fn group_of_finds_the_group_holding_the_comment() {
        let loose = comment(Status::Done);
        let mut other = comment(Status::Done);
        other.id = "r10".into();
        let queue = queue::build(
            &[loose, other],
            &[],
            &HashSet::<String>::new(),
            &Filter::default(),
        );
        let group = group_of(&queue, "r10").expect("a group");
        assert!(group.review.is_none());
        assert_eq!(group.rows.len(), 2);
        assert!(group_of(&queue, "r99").is_none());
    }
}
