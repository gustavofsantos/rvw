//! Verdicts: accepting a decision, or reopening a comment as a follow-up.
//! rvw's domain: types, operations, the storage trait and the service.

use crate::error::{Error, ErrorKind, Result};
use crate::operations::{VerdictInput, VerdictOutput};
use crate::service::{
    find, read_source, snapshot_reviewed, split_source, to_i64, to_index, trim_newlines, Service,
};
use crate::types::{comment_id, Actor, Comment, LineRange, Status, Verdict, VerdictKind, VERDICTS};

#[allow(clippy::needless_pass_by_value, clippy::missing_errors_doc)] // operations take their input by value, as the adapters build it per call
impl Service {
    /// Judges a decided comment, as the reviewer named in `input.author`.
    /// Accepting agrees with the decision. Reopening disagrees: it records the
    /// verdict on the original and creates a pending follow-up comment with the
    /// original's placement, which the next pull hands over.
    pub fn judge(&self, input: VerdictInput, actor: Actor) -> Result<VerdictOutput> {
        let kind = VerdictKind::parse(&input.verdict).ok_or_else(|| {
            Error::invalid(format!(
                "verdict \"{}\" is not one of {}",
                input.verdict,
                VERDICTS.join(", ")
            ))
        })?;
        let by = input.author.trim().to_string();
        if by.is_empty() {
            return Err(Error::invalid("a verdict needs an author"));
        }
        let reopening = kind == VerdictKind::Reopened;
        if reopening && input.note.trim().is_empty() {
            return Err(Error::invalid("reopening a comment needs a note"));
        }
        let range = override_range(&input)?;
        if range.is_some() && !reopening {
            return Err(Error::invalid("a range only applies to a reopen"));
        }

        // Git and file reads run before the write transaction, so a slow git
        // never holds the database lock; the checks run again inside it.
        let placement = if reopening {
            let original = self.read(&input.workspace, |tx| {
                let c = find(tx, &input.id)?;
                check_judgeable(&c, &by)?;
                Ok(c)
            })?;
            Some(followup_placement(&original, range)?)
        } else {
            None
        };

        let note = remark(&input.note);
        let text = trim_newlines(&input.note).to_string();
        self.write(&input.workspace, |tx| {
            let mut original = find(tx, &input.id)?;
            check_judgeable(&original, &by)?;
            let at = self.stamp();
            let Some(placement) = placement else {
                original.verdict = Some(new_verdict(kind, &by, actor, &at, note, None));
                tx.update_comment(&original)?;
                return Ok(VerdictOutput {
                    comment: original,
                    followup: None,
                });
            };
            let seq = tx.next_comment_seq()?;
            let id = comment_id(seq);
            let followup = Comment {
                points_at: original.points_at.clone(),
                reopens: Some(original.id.clone()),
                verdict: None,
                id: id.clone(),
                status: Status::Pending,
                workspace: original.workspace.clone(),
                lane: original.lane.clone(),
                author: Some(by.clone()),
                file: original.file.clone(),
                path: original.path.clone(),
                start_line: placement.start_line,
                end_line: placement.end_line,
                filetype: original.filetype.clone(),
                code: placement.code,
                comment: text,
                created_at: at.clone(),
                pulled_at: None,
                resolved_at: None,
                resolved_by: None,
                resolution_note: None,
                file_version: placement.file_version,
                resolved_file_version: String::new(),
                review_id: String::new(),
                edited_at: String::new(),
            };
            tx.insert_comment(&followup)?;
            original.verdict = Some(new_verdict(kind, &by, actor, &at, note, Some(id)));
            tx.update_comment(&original)?;
            Ok(VerdictOutput {
                comment: original,
                followup: Some(followup),
            })
        })
    }
}

/// The range a reopen gives its follow-up: both lines, or neither to keep the
/// original range. A swapped range is swapped.
fn override_range(input: &VerdictInput) -> Result<Option<LineRange>> {
    match (input.start_line, input.end_line) {
        (0, 0) => Ok(None),
        (0, _) | (_, 0) => Err(Error::invalid(
            "a range needs both start_line and end_line, or neither to keep the original range",
        )),
        (start_line, end_line) => LineRange {
            start_line,
            end_line,
        }
        .normalized()
        .map(Some),
    }
}

/// Whether `by` may give a verdict on `c`. Only a decision made after a
/// handoff waits for one, and a verdict is given once, by someone other than
/// the decider.
fn check_judgeable(c: &Comment, by: &str) -> Result<()> {
    if !c.status.resolved() {
        return Err(Error::conflict(format!(
            "{} is {} — only a decided comment can be judged",
            c.id, c.status
        )));
    }
    if c.pulled_at.is_none() {
        return Err(Error::conflict(format!(
            "{} was withdrawn before a handoff — it needs no verdict",
            c.id
        )));
    }
    if c.verdict.is_some() {
        return Err(Error::conflict(format!(
            "{} already has a verdict — it is final",
            c.id
        )));
    }
    if c.resolved_by.as_deref() == Some(by) {
        return Err(Error::conflict(format!(
            "{} was decided by {by} — a second reviewer must judge it",
            c.id
        )));
    }
    if !c.awaits_verdict() {
        return Err(Error::conflict(format!("{} needs no verdict", c.id)));
    }
    Ok(())
}

/// Where a reopen's follow-up sits and what it holds.
struct Placement {
    start_line: i64,
    end_line: i64,
    code: String,
    file_version: String,
}

/// Reads the original's file as it is now, for the follow-up. When the file is
/// gone, the follow-up keeps the original's code and range with no version,
/// and a range cannot be moved, since nothing checks it.
fn followup_placement(c: &Comment, range: Option<LineRange>) -> Result<Placement> {
    match read_source(&c.path, None) {
        Ok(source) => {
            let lines = split_source(&source);
            let total = to_i64(lines.len());
            let want = range.unwrap_or(LineRange {
                start_line: c.start_line,
                end_line: c.end_line,
            });
            if want.start_line > total {
                return Err(Error::invalid(format!(
                    "lines {} is past the end of {} ({total} lines)",
                    want.label(),
                    c.path
                )));
            }
            let end_line = want.end_line.min(total);
            let code = lines[to_index(want.start_line - 1)..to_index(end_line)].join("\n");
            Ok(Placement {
                start_line: want.start_line,
                end_line,
                code,
                file_version: snapshot_reviewed(&c.workspace, &c.file, &source)?,
            })
        }
        Err(e) if e.kind == ErrorKind::NotFound => {
            if range.is_some() {
                return Err(Error::invalid(format!(
                    "cannot move the range: {} is gone, so there is nothing to check it against",
                    c.path
                )));
            }
            Ok(Placement {
                start_line: c.start_line,
                end_line: c.end_line,
                code: c.code.clone(),
                file_version: String::new(),
            })
        }
        Err(e) => Err(e),
    }
}

/// A note as stored: newlines trimmed, and a blank note is no note.
fn remark(note: &str) -> Option<String> {
    let text = trim_newlines(note);
    if text.trim().is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

fn new_verdict(
    kind: VerdictKind,
    by: &str,
    actor: Actor,
    at: &str,
    note: Option<String>,
    followup: Option<String>,
) -> Verdict {
    Verdict {
        kind,
        by: by.to_string(),
        actor,
        at: at.to_string(),
        note,
        followup,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::error::{ErrorKind, Result};
    use crate::operations::{GetInput, ResolveInput};
    use crate::service::find;
    use crate::service::tests::{git, setup, Fixture};
    use crate::types::{Location, Status, Verdict, OUTCOME_DONE, OUTCOME_REJECTED};

    fn input(f: &Fixture, id: &str, verdict: &str, note: &str, author: &str) -> VerdictInput {
        VerdictInput {
            workspace: f.ws.clone(),
            id: id.into(),
            verdict: verdict.into(),
            note: note.into(),
            author: author.into(),
            ..Default::default()
        }
    }

    fn give(
        f: &Fixture,
        id: &str,
        verdict: &str,
        note: &str,
        author: &str,
        actor: Actor,
    ) -> Result<VerdictOutput> {
        f.svc.judge(input(f, id, verdict, note, author), actor)
    }

    /// Decides a handed-over comment; the decider is always claude.
    fn decide(f: &Fixture, id: &str, outcome: &str, note: &str) {
        f.svc
            .resolve(ResolveInput {
                workspace: f.ws.clone(),
                id: id.into(),
                outcome: outcome.into(),
                note: note.into(),
                author: "claude".into(),
            })
            .expect("resolve");
    }

    fn stored(f: &Fixture, id: &str) -> Comment {
        f.svc
            .get(GetInput {
                workspace: f.ws.clone(),
                id: id.into(),
            })
            .expect("get")
    }

    /// Sets a comment's points-at, which `add` does not record in this slice.
    fn point(f: &Fixture, id: &str, at: Location) {
        f.svc
            .write(&f.ws, |tx| {
                let mut c = find(tx, id)?;
                c.points_at = Some(at);
                tx.update_comment(&c)
            })
            .expect("point");
    }

    fn ids(cs: &[Comment]) -> Vec<String> {
        cs.iter().map(|c| c.id.clone()).collect()
    }

    #[test]
    fn accepting_a_done_comment_records_the_verdict() {
        let f = setup();
        f.add_at(1, "x", "", "alice");
        f.pull("");
        decide(&f, "r1", OUTCOME_DONE, "");
        let out = give(&f, "r1", "accepted", "agreed", "alice", Actor::Human).expect("accept");
        assert!(out.followup.is_none());
        let v = out.comment.verdict.clone().expect("verdict");
        assert_eq!(v.kind, VerdictKind::Accepted);
        assert_eq!(v.by, "alice");
        assert_eq!(v.actor, Actor::Human);
        assert_eq!(v.at, "2026-10-12T09:00:00+00:00");
        assert_eq!(v.note.as_deref(), Some("agreed"));
        assert_eq!(v.followup, None);
        assert_eq!(out.comment.status, Status::Done);
        assert!(!out.comment.awaits_verdict());
        assert_eq!(stored(&f, "r1"), out.comment);
        assert!(f
            .svc
            .get(GetInput {
                workspace: f.ws.clone(),
                id: "r2".into()
            })
            .is_err());
    }

    #[test]
    fn accepting_a_rejected_comment_agrees_it_should_not_be_done() {
        let f = setup();
        f.add_at(1, "x", "", "alice");
        f.pull("");
        decide(&f, "r1", OUTCOME_REJECTED, "not needed");
        let out = give(&f, "r1", "accepted", "  ", "alice", Actor::Human).expect("accept");
        assert_eq!(out.comment.status, Status::Rejected);
        assert_eq!(out.comment.resolution_note.as_deref(), Some("not needed"));
        let v = out.comment.verdict.expect("verdict");
        assert_eq!(v.kind, VerdictKind::Accepted);
        assert_eq!(v.note, None, "a blank remark is no remark");
    }

    #[test]
    fn reopening_creates_a_linked_follow_up_with_the_original_placement() {
        let f = setup();
        git(&f.root, &["add", "app.py"]);
        f.add_at(2, "fix this", "feat", "alice");
        point(
            &f,
            "r1",
            Location {
                file: "app.py".into(),
                start_line: 4,
                end_line: 4,
            },
        );
        f.pull("feat");
        decide(&f, "r1", OUTCOME_DONE, "");
        let before = stored(&f, "r1");
        fs::write(f.root.join("app.py"), "one\nTWO\nthree\nfour\n").expect("write");

        let out = give(
            &f,
            "r1",
            "reopened",
            "\nstill wrong\n",
            "alice",
            Actor::Human,
        )
        .expect("reopen");
        let fu = out.followup.clone().expect("follow-up");
        assert_eq!(fu.id, "r2");
        assert_eq!(fu.status, Status::Pending);
        assert_eq!(fu.lane.as_deref(), Some("feat"));
        assert_eq!(fu.author.as_deref(), Some("alice"));
        assert_eq!(fu.file, before.file);
        assert_eq!(fu.path, before.path);
        assert_eq!(fu.filetype, before.filetype);
        assert_eq!((fu.start_line, fu.end_line), (2, 2));
        assert_eq!(
            fu.code, "TWO",
            "the follow-up snapshots the file as it is now"
        );
        assert_eq!(fu.comment, "still wrong");
        assert_eq!(
            fu.points_at,
            Some(Location {
                file: "app.py".into(),
                start_line: 4,
                end_line: 4
            })
        );
        assert_eq!(fu.reopens.as_deref(), Some("r1"));
        assert!(fu.pulled_at.is_none());
        assert!(fu.resolved_at.is_none());
        assert!(fu.resolved_by.is_none());
        assert!(fu.verdict.is_none());
        assert!(
            !fu.file_version.is_empty(),
            "a tracked file keeps its version"
        );

        let original = out.comment.clone();
        let v = original.verdict.clone().expect("verdict");
        assert_eq!(v.kind, VerdictKind::Reopened);
        assert_eq!(v.by, "alice");
        assert_eq!(v.note.as_deref(), Some("still wrong"));
        assert_eq!(v.followup.as_deref(), Some("r2"));
        assert_eq!(original.status, Status::Done);
        assert_eq!(original.resolved_by, before.resolved_by);
        assert_eq!(original.resolved_at, before.resolved_at);
        assert_eq!(original.resolution_note, before.resolution_note);
        assert_eq!(original.resolved_file_version, before.resolved_file_version);
        assert_eq!(stored(&f, "r1"), original);
        assert_eq!(stored(&f, "r2"), fu);
    }

    #[test]
    fn reopening_a_rejected_comment_is_allowed() {
        let f = setup();
        f.add_at(1, "x", "", "alice");
        f.pull("");
        decide(&f, "r1", OUTCOME_REJECTED, "no");
        let out =
            give(&f, "r1", "reopened", "do it anyway", "alice", Actor::Human).expect("reopen");
        assert_eq!(out.followup.expect("follow-up").comment, "do it anyway");
        assert_eq!(out.comment.status, Status::Rejected);
    }

    #[test]
    fn a_range_override_is_clamped_to_the_file_and_swapped_when_reversed() {
        let f = setup();
        f.add_at(2, "a", "", "alice");
        f.add_at(1, "b", "", "alice");
        f.add_at(3, "c", "", "alice");
        f.pull("");
        decide(&f, "r1", OUTCOME_DONE, "");
        decide(&f, "r2", OUTCOME_DONE, "");
        decide(&f, "r3", OUTCOME_DONE, "");

        let mut reversed = input(&f, "r1", "reopened", "more", "alice");
        reversed.start_line = 4;
        reversed.end_line = 3;
        let fu = f
            .svc
            .judge(reversed, Actor::Human)
            .expect("reopen")
            .followup
            .expect("fu");
        assert_eq!((fu.start_line, fu.end_line), (3, 4));
        assert_eq!(fu.code, "three\nfour");

        let mut clamped = input(&f, "r2", "reopened", "to the end", "alice");
        clamped.start_line = 4;
        clamped.end_line = 99;
        let fu = f
            .svc
            .judge(clamped, Actor::Human)
            .expect("reopen")
            .followup
            .expect("fu");
        assert_eq!((fu.start_line, fu.end_line), (4, 4));
        assert_eq!(fu.code, "four");

        let mut past = input(&f, "r3", "reopened", "gone", "alice");
        past.start_line = 9;
        past.end_line = 9;
        let err = f.svc.judge(past, Actor::Human).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Invalid);
        assert!(err.message.contains("past the end"), "{}", err.message);
        assert!(
            stored(&f, "r3").awaits_verdict(),
            "a refused reopen changes nothing"
        );
        assert_eq!(stored(&f, "r3").verdict, None);
    }

    #[test]
    fn a_pinned_pull_takes_the_follow_up_once_and_only_in_its_lane() {
        let f = setup();
        f.add_at(2, "fix", "feat", "alice");
        f.pull("feat");
        decide(&f, "r1", OUTCOME_DONE, "");
        give(&f, "r1", "reopened", "again", "alice", Actor::Human).expect("reopen");

        assert!(
            f.pull("other").comments.is_empty(),
            "another lane never takes it"
        );
        assert_eq!(ids(&f.pull("feat").comments), vec!["r2".to_string()]);
        assert_eq!(stored(&f, "r2").status, Status::Pulled);
        assert!(f.pull("feat").comments.is_empty(), "handed over once");
    }

    #[test]
    fn when_the_file_is_gone_the_follow_up_keeps_the_original_code() {
        let f = setup();
        let orig = f.add_at(2, "fix", "", "alice");
        f.pull("");
        decide(&f, "r1", OUTCOME_DONE, "");
        fs::remove_file(f.root.join("app.py")).expect("remove");

        let mut moved = input(&f, "r1", "reopened", "bring it back", "alice");
        moved.start_line = 1;
        moved.end_line = 1;
        let err = f.svc.judge(moved, Actor::Human).unwrap_err();
        assert_eq!(
            err.kind,
            ErrorKind::Invalid,
            "a range cannot be checked without the file"
        );
        assert!(stored(&f, "r1").awaits_verdict());

        let out =
            give(&f, "r1", "reopened", "bring it back", "alice", Actor::Human).expect("reopen");
        let fu = out.followup.expect("follow-up");
        assert_eq!(fu.file, orig.file);
        assert_eq!(fu.code, "two");
        assert_eq!((fu.start_line, fu.end_line), (2, 2));
        assert_eq!(fu.filetype, orig.filetype);
        assert!(fu.file_version.is_empty(), "no file, no version");
    }

    #[test]
    fn a_comment_that_is_undecided_or_withdrawn_cannot_be_judged() {
        let f = setup();
        f.add_at(1, "pending", "", "alice");
        f.add_at(2, "withdrawn", "", "alice");
        decide(&f, "r2", OUTCOME_REJECTED, "not needed");

        let err = give(&f, "r1", "accepted", "", "alice", Actor::Human).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Conflict);
        assert_eq!(
            err.message,
            "r1 is pending — only a decided comment can be judged"
        );

        let err = give(&f, "r2", "accepted", "", "alice", Actor::Human).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Conflict);
        assert_eq!(
            err.message,
            "r2 was withdrawn before a handoff — it needs no verdict"
        );
        assert_eq!(stored(&f, "r1").verdict, None);
        assert_eq!(stored(&f, "r2").verdict, None);
    }

    #[test]
    fn a_verdict_is_final() {
        let f = setup();
        f.add_at(1, "x", "", "alice");
        f.pull("");
        decide(&f, "r1", OUTCOME_DONE, "");
        give(&f, "r1", "accepted", "", "alice", Actor::Human).expect("accept");

        let err = give(&f, "r1", "accepted", "", "alice", Actor::Human).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Conflict);
        assert_eq!(err.message, "r1 already has a verdict — it is final");
        let err = give(&f, "r1", "reopened", "again", "alice", Actor::Human).unwrap_err();
        assert_eq!(err.message, "r1 already has a verdict — it is final");

        assert_eq!(
            stored(&f, "r1").verdict.expect("verdict").kind,
            VerdictKind::Accepted
        );
        assert!(f
            .svc
            .get(GetInput {
                workspace: f.ws.clone(),
                id: "r2".into()
            })
            .is_err());
    }

    #[test]
    fn a_decision_is_judged_by_a_second_reviewer() {
        let f = setup();
        f.add_at(1, "x", "", "alice");
        f.pull("");
        decide(&f, "r1", OUTCOME_DONE, "");

        let err = give(&f, "r1", "accepted", "", "claude", Actor::Agent).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Conflict);
        assert_eq!(
            err.message,
            "r1 was decided by claude — a second reviewer must judge it"
        );
        assert!(stored(&f, "r1").awaits_verdict());
    }

    #[test]
    fn a_comment_its_own_author_decided_needs_no_verdict() {
        let f = setup();
        f.add_at(1, "x", "", "claude");
        f.pull("");
        decide(&f, "r1", OUTCOME_DONE, "");

        let err = give(&f, "r1", "accepted", "", "alice", Actor::Human).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Conflict);
        assert_eq!(err.message, "r1 needs no verdict");
    }

    #[test]
    fn a_verdict_needs_a_valid_kind_an_author_a_note_and_a_sane_range() {
        let f = setup();
        f.add_at(1, "x", "", "alice");
        f.pull("");
        decide(&f, "r1", OUTCOME_DONE, "");

        let err = give(&f, "r1", "maybe", "", "alice", Actor::Human).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Invalid);
        assert_eq!(
            err.message,
            "verdict \"maybe\" is not one of accepted, reopened"
        );

        let err = give(&f, "r1", "accepted", "", "  ", Actor::Human).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Invalid);
        assert_eq!(err.message, "a verdict needs an author");

        let err = give(&f, "r1", "reopened", " \n ", "alice", Actor::Human).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Invalid);
        assert_eq!(err.message, "reopening a comment needs a note");

        let mut accept_range = input(&f, "r1", "accepted", "", "alice");
        accept_range.start_line = 1;
        accept_range.end_line = 2;
        let err = f.svc.judge(accept_range, Actor::Human).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Invalid);
        assert_eq!(err.message, "a range only applies to a reopen");

        let mut half = input(&f, "r1", "reopened", "more", "alice");
        half.start_line = 3;
        let err = f.svc.judge(half, Actor::Human).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Invalid);

        assert_eq!(
            stored(&f, "r1").verdict,
            None,
            "every refusal leaves it untouched"
        );
    }

    #[test]
    fn the_actor_is_recorded_as_the_surface_it_came_through() {
        let f = setup();
        f.add_at(1, "a", "", "alice");
        f.add_at(2, "b", "", "alice");
        f.pull("");
        decide(&f, "r1", OUTCOME_DONE, "");
        decide(&f, "r2", OUTCOME_REJECTED, "no");

        let out = give(&f, "r1", "accepted", "", "alice", Actor::Agent).expect("accept");
        assert_eq!(out.comment.verdict.expect("verdict").actor, Actor::Agent);

        let out = give(&f, "r2", "reopened", "do it", "alice", Actor::Human).expect("reopen");
        let v: Verdict = out.comment.verdict.expect("verdict");
        assert_eq!(v.actor, Actor::Human);
        assert_eq!(v.kind, VerdictKind::Reopened);
        assert_eq!(
            out.followup.expect("follow-up").reopens.as_deref(),
            Some("r2")
        );
    }
}
