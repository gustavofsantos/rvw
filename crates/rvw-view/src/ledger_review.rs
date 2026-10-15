//! The Review section under a commit in the History screen: the decided
//! comments that landed in the commit, and the ones on its files that matched
//! no commit. Pure: the ledger's entries in, the lines out. The GUI only draws
//! them, and a comment id is plain text.

use rvw_core::operations::{LinkKind, LinkedComment, UnlinkedComment};
use rvw_core::types::VerdictKind;

use crate::provenance_label::first_line;

/// What a line of the Review section says, so the GUI can colour it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewKind {
    /// A linked comment: its id, location, who decided it and how it matched.
    Head,
    /// The first line of the note that decided a linked comment.
    Note,
    /// A reviewer accepted the decision.
    Accepted,
    /// A reviewer reopened the decision.
    Reopened,
    /// A comment on a file of the commit that matched no commit.
    Unlinked,
}

/// One line of the Review section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewLine {
    pub kind: ReviewKind,
    pub text: String,
}

/// The Review section's lines for one commit: each linked comment with its
/// note and verdict, then each unlinked comment with its reason. Empty when
/// there is nothing to show, and then the section is left out.
#[must_use]
pub fn review_lines(linked: &[LinkedComment], unlinked: &[UnlinkedComment]) -> Vec<ReviewLine> {
    let mut lines = Vec::new();
    for item in linked {
        let c = &item.comment;
        lines.push(line(
            ReviewKind::Head,
            format!(
                "{} \u{00b7} {}:{} \u{00b7} {} \u{00b7} {}",
                c.id,
                c.file,
                span(c.start_line, c.end_line),
                done_by(c.resolved_by.as_deref()),
                link_text(item.link),
            ),
        ));
        if let Some(note) = c.resolution_note.as_deref().and_then(first_line) {
            lines.push(line(ReviewKind::Note, note));
        }
        if let Some(verdict) = &c.verdict {
            lines.push(match verdict.kind {
                VerdictKind::Accepted => ReviewLine {
                    kind: ReviewKind::Accepted,
                    text: format!("accepted by @{} ({})", verdict.by, verdict.actor.as_str()),
                },
                VerdictKind::Reopened => ReviewLine {
                    kind: ReviewKind::Reopened,
                    text: format!("reopened by @{}", verdict.by),
                },
            });
        }
    }
    for item in unlinked {
        let c = &item.comment;
        lines.push(line(
            ReviewKind::Unlinked,
            format!(
                "{} \u{00b7} {} \u{00b7} unlinked: {}",
                c.id, c.file, item.reason
            ),
        ));
    }
    lines
}

fn line(kind: ReviewKind, text: String) -> ReviewLine {
    ReviewLine { kind, text }
}

/// "3" for one line, "3-9" for a range.
fn span(start: i64, end: i64) -> String {
    if start == end {
        start.to_string()
    } else {
        format!("{start}-{end}")
    }
}

fn done_by(who: Option<&str>) -> String {
    match who {
        Some(who) if !who.is_empty() => format!("done by @{who}"),
        _ => "done".to_owned(),
    }
}

fn link_text(link: LinkKind) -> &'static str {
    match link {
        LinkKind::Exact => "exact",
        LinkKind::SameChange => "same-change",
    }
}

#[cfg(test)]
mod tests {
    use rvw_core::operations::{LinkKind, LinkedComment, UnlinkedComment};
    use rvw_core::types::{Actor, Comment, Status, Verdict, VerdictKind};

    use super::*;

    fn comment(id: &str) -> Comment {
        Comment {
            id: id.into(),
            status: Status::Done,
            workspace: "/ws".into(),
            lane: Some("feat".into()),
            author: None,
            file: "src/a.rs".into(),
            path: "/ws/src/a.rs".into(),
            start_line: 3,
            end_line: 9,
            filetype: "rust".into(),
            code: "let x = 1;".into(),
            comment: "fix this".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            pulled_at: None,
            resolved_at: Some("2026-01-02T00:00:00Z".into()),
            resolved_by: Some("ana".into()),
            resolution_note: Some("moved the check\nand the test".into()),
            points_at: None,
            reopens: None,
            verdict: None,
            file_version: String::new(),
            resolved_file_version: String::new(),
            review_id: String::new(),
            edited_at: String::new(),
        }
    }

    fn linked(c: Comment, link: LinkKind) -> LinkedComment {
        LinkedComment {
            comment: c,
            link,
            diff: Vec::new(),
        }
    }

    fn verdict(kind: VerdictKind, actor: Actor) -> Verdict {
        Verdict {
            kind,
            by: "bruno".into(),
            actor,
            at: "2026-01-03T00:00:00Z".into(),
            note: None,
            followup: None,
        }
    }

    fn texts(lines: &[ReviewLine]) -> Vec<&str> {
        lines.iter().map(|l| l.text.as_str()).collect()
    }

    #[test]
    fn nothing_to_show_gives_no_lines() {
        assert!(review_lines(&[], &[]).is_empty());
    }

    #[test]
    fn a_linked_comment_gives_its_head_and_note_first_line() {
        let lines = review_lines(&[linked(comment("r12"), LinkKind::Exact)], &[]);
        assert_eq!(
            texts(&lines),
            [
                "r12 \u{00b7} src/a.rs:3-9 \u{00b7} done by @ana \u{00b7} exact",
                "moved the check",
            ]
        );
        assert_eq!(lines[0].kind, ReviewKind::Head);
        assert_eq!(lines[1].kind, ReviewKind::Note);
    }

    #[test]
    fn a_same_change_link_is_named_and_a_single_line_has_no_range() {
        let mut c = comment("r14");
        c.start_line = 5;
        c.end_line = 5;
        c.resolution_note = None;
        let lines = review_lines(&[linked(c, LinkKind::SameChange)], &[]);
        assert_eq!(
            texts(&lines),
            ["r14 \u{00b7} src/a.rs:5 \u{00b7} done by @ana \u{00b7} same-change"]
        );
    }

    #[test]
    fn a_missing_resolver_or_blank_note_is_left_out() {
        let mut c = comment("r15");
        c.resolved_by = None;
        c.resolution_note = Some("  \n \n".into());
        let lines = review_lines(&[linked(c, LinkKind::Exact)], &[]);
        assert_eq!(
            texts(&lines),
            ["r15 \u{00b7} src/a.rs:3-9 \u{00b7} done \u{00b7} exact"]
        );
    }

    #[test]
    fn an_accepted_verdict_names_the_actor_and_a_reopened_one_does_not() {
        let mut accepted = comment("r16");
        accepted.verdict = Some(verdict(VerdictKind::Accepted, Actor::Human));
        let mut reopened = comment("r17");
        reopened.verdict = Some(verdict(VerdictKind::Reopened, Actor::Agent));
        let lines = review_lines(
            &[
                linked(accepted, LinkKind::Exact),
                linked(reopened, LinkKind::Exact),
            ],
            &[],
        );
        let verdicts: Vec<_> = lines
            .iter()
            .filter(|l| matches!(l.kind, ReviewKind::Accepted | ReviewKind::Reopened))
            .map(|l| (l.kind, l.text.as_str()))
            .collect();
        assert_eq!(
            verdicts,
            [
                (ReviewKind::Accepted, "accepted by @bruno (human)"),
                (ReviewKind::Reopened, "reopened by @bruno"),
            ]
        );
    }

    #[test]
    fn an_unlinked_comment_gives_its_reason() {
        let unlinked = UnlinkedComment {
            comment: comment("r13"),
            reason: "no content match".into(),
        };
        let lines = review_lines(&[], &[unlinked]);
        assert_eq!(
            texts(&lines),
            ["r13 \u{00b7} src/a.rs \u{00b7} unlinked: no content match"]
        );
        assert_eq!(lines[0].kind, ReviewKind::Unlinked);
    }

    #[test]
    fn linked_lines_come_before_unlinked_ones() {
        let unlinked = UnlinkedComment {
            comment: comment("r13"),
            reason: "no snapshot of the resolved file".into(),
        };
        let lines = review_lines(&[linked(comment("r12"), LinkKind::Exact)], &[unlinked]);
        let kinds: Vec<_> = lines.iter().map(|l| l.kind).collect();
        assert_eq!(
            kinds,
            [ReviewKind::Head, ReviewKind::Note, ReviewKind::Unlinked]
        );
    }
}
