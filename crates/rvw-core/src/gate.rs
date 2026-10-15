//! The commit gate: what is open or unjudged, and recorded overrides.

use crate::error::{Error, Result};
use crate::operations::{GateInput, GateOutput, OverrideInput};
use crate::service::{Service, str_opt};
use crate::types::{Actor, Comment, GATE_ACTIONS, GateOverride};

/// The comment ids a lane has open and the ones still awaiting a verdict.
/// Lane scoping is strict: a lane never sees another lane's or an unlaned
/// comment; "" means every lane. Input order is kept.
#[must_use]
pub fn undecided(comments: &[Comment], lane: &str) -> (Vec<String>, Vec<String>) {
    let mut open = Vec::new();
    let mut unjudged = Vec::new();
    for c in comments.iter().filter(|c| in_lane(c, lane)) {
        if c.is_open() {
            open.push(c.id.clone());
        }
        if c.awaits_verdict() {
            unjudged.push(c.id.clone());
        }
    }
    (open, unjudged)
}

/// Whether `c` belongs to `lane`; "" is every lane.
fn in_lane(c: &Comment, lane: &str) -> bool {
    lane.is_empty() || c.lane.as_deref() == Some(lane)
}

impl Service {
    /// Reads what stands between the workspace and a commit or push.
    ///
    /// # Errors
    /// Refuses a workspace path that is not absolute, or a store that cannot be read.
    pub fn gate(&self, input: GateInput) -> Result<GateOutput> {
        let GateInput { workspace, lane } = input;
        let (open, unjudged) =
            self.read(&workspace, |tx| Ok(undecided(&tx.comments(&[])?, &lane)))?;
        Ok(GateOutput {
            clear: open.is_empty() && unjudged.is_empty(),
            lane: str_opt(&lane),
            workspace,
            open,
            unjudged,
        })
    }

    /// Records going ahead with an action although the gate is not clear. The
    /// open and unjudged ids are read again inside the write, so the record
    /// holds what the gate saw, never what the caller says.
    ///
    /// # Errors
    /// Refuses an unknown action, a blank reason or author, and a gate that is
    /// already clear.
    pub fn override_gate(&self, input: OverrideInput, actor: Actor) -> Result<GateOverride> {
        let OverrideInput {
            workspace,
            action,
            reason,
            lane,
            author,
        } = input;
        if !GATE_ACTIONS.contains(&action.as_str()) {
            return Err(Error::invalid(format!(
                "action \"{action}\" is not one of {}",
                GATE_ACTIONS.join(", ")
            )));
        }
        let reason = reason.trim().to_string();
        if reason.is_empty() {
            return Err(Error::invalid("an override needs a reason"));
        }
        let author = author.trim().to_string();
        if author.is_empty() {
            return Err(Error::invalid("an override needs an author"));
        }
        self.write(&workspace, |tx| {
            let (open, unjudged) = undecided(&tx.comments(&[])?, &lane);
            if open.is_empty() && unjudged.is_empty() {
                return Err(Error::conflict(
                    "nothing is open or unjudged — no override needed",
                ));
            }
            let o = GateOverride {
                workspace: workspace.clone(),
                lane: str_opt(&lane),
                action,
                reason,
                by: author,
                actor,
                at: self.stamp(),
                open,
                unjudged,
            };
            tx.insert_override(&o)?;
            Ok(o)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operations::{GateInput, OverrideInput, ResolveInput};
    use crate::service::tests::{Fixture, setup};
    use crate::types::{Actor, Status, Verdict, VerdictKind};

    const AT: &str = "2026-10-12T09:00:00+00:00";

    /// A pending comment on app.py, unlaned and by nobody; tests set what they need.
    fn comment(id: &str, lane: Option<&str>) -> Comment {
        Comment {
            id: id.into(),
            status: Status::Pending,
            workspace: "/w".into(),
            lane: lane.map(str::to_string),
            author: None,
            file: "app.py".into(),
            path: "/w/app.py".into(),
            start_line: 1,
            end_line: 1,
            filetype: String::new(),
            code: String::new(),
            comment: String::new(),
            created_at: AT.into(),
            pulled_at: None,
            resolved_at: None,
            resolved_by: None,
            resolution_note: None,
            points_at: None,
            reopens: None,
            verdict: None,
            file_version: String::new(),
            resolved_file_version: String::new(),
            review_id: String::new(),
            edited_at: String::new(),
        }
    }

    /// A comment `author` raised that `by` decided after a handoff.
    fn decided(id: &str, lane: Option<&str>, status: Status, author: &str, by: &str) -> Comment {
        Comment {
            status,
            author: Some(author.into()),
            pulled_at: Some(AT.into()),
            resolved_at: Some(AT.into()),
            resolved_by: Some(by.into()),
            ..comment(id, lane)
        }
    }

    fn strs(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| (*s).to_string()).collect()
    }

    fn gate(f: &Fixture, lane: &str) -> crate::operations::GateOutput {
        f.svc
            .gate(GateInput {
                workspace: f.ws.clone(),
                lane: lane.into(),
            })
            .expect("gate")
    }

    fn resolve_done(f: &Fixture, id: &str) {
        f.svc
            .resolve(ResolveInput {
                workspace: f.ws.clone(),
                id: id.into(),
                outcome: "done".into(),
                author: "claude".into(),
                ..Default::default()
            })
            .expect("resolve");
    }

    fn override_input(
        f: &Fixture,
        action: &str,
        reason: &str,
        lane: &str,
        author: &str,
    ) -> OverrideInput {
        OverrideInput {
            workspace: f.ws.clone(),
            action: action.into(),
            reason: reason.into(),
            lane: lane.into(),
            author: author.into(),
        }
    }

    #[test]
    fn undecided_follows_status_and_verdict() {
        let pulled = Comment {
            status: Status::Pulled,
            pulled_at: Some(AT.into()),
            ..comment("r1", None)
        };
        let judged = Comment {
            verdict: Some(Verdict {
                kind: VerdictKind::Accepted,
                by: "lead".into(),
                actor: Actor::Human,
                at: AT.into(),
                note: None,
                followup: None,
            }),
            ..decided("r1", None, Status::Done, "alice", "claude")
        };
        let withdrawn = Comment {
            status: Status::Rejected,
            resolved_at: Some(AT.into()),
            resolved_by: Some("alice".into()),
            ..comment("r1", None)
        };
        // (case, comment, open, unjudged)
        let cases = [
            (
                "a pending comment is open",
                comment("r1", None),
                true,
                false,
            ),
            (
                "a draft is open",
                Comment {
                    author: Some("gui".into()),
                    ..comment("r1", None)
                },
                true,
                false,
            ),
            ("a pulled comment is open", pulled, true, false),
            (
                "done after a handoff awaits a verdict",
                decided("r1", None, Status::Done, "alice", "claude"),
                false,
                true,
            ),
            (
                "rejected after a handoff awaits a verdict",
                decided("r1", None, Status::Rejected, "alice", "claude"),
                false,
                true,
            ),
            (
                "a decision by its own author does not wait",
                decided("r1", None, Status::Done, "claude", "claude"),
                false,
                false,
            ),
            (
                "a decision with a verdict does not wait",
                judged,
                false,
                false,
            ),
            ("a withdrawal does not wait", withdrawn, false, false),
        ];
        for (case, c, open, unjudged) in cases {
            let (o, u) = undecided(std::slice::from_ref(&c), "");
            assert_eq!(!o.is_empty(), open, "{case}: open");
            assert_eq!(!u.is_empty(), unjudged, "{case}: unjudged");
        }
    }

    #[test]
    fn lane_scoping_is_strict_and_keeps_input_order() {
        let cs = [
            comment("r1", None),
            comment("r2", Some("auth")),
            decided("r3", Some("payments"), Status::Done, "alice", "claude"),
            Comment {
                status: Status::Pulled,
                pulled_at: Some(AT.into()),
                ..comment("r4", Some("auth"))
            },
            decided("r5", None, Status::Rejected, "alice", "claude"),
        ];
        // (lane, open, unjudged)
        let cases = [
            ("", vec!["r1", "r2", "r4"], vec!["r3", "r5"]),
            ("auth", vec!["r2", "r4"], vec![]),
            ("payments", vec![], vec!["r3"]),
            ("billing", vec![], vec![]),
        ];
        for (lane, open, unjudged) in cases {
            let (o, u) = undecided(&cs, lane);
            assert_eq!(o, strs(&open), "lane {lane:?}: open");
            assert_eq!(u, strs(&unjudged), "lane {lane:?}: unjudged");
        }
    }

    #[test]
    fn gate_reads_every_status_and_scopes_to_the_lane() {
        let f = setup();
        f.add_at(1, "unlaned", "", "alice");
        f.add_at(2, "auth one", "auth", "alice");
        f.add_at(3, "auth two", "auth", "alice");
        f.pull("auth");
        resolve_done(&f, "r2");

        let all = gate(&f, "");
        assert_eq!(all.workspace, f.ws);
        assert_eq!(all.lane, None);
        assert_eq!(all.open, strs(&["r1", "r3"]));
        assert_eq!(all.unjudged, strs(&["r2"]));
        assert!(!all.clear);

        let auth = gate(&f, "auth");
        assert_eq!(auth.lane.as_deref(), Some("auth"));
        assert_eq!(auth.open, strs(&["r3"]));
        assert_eq!(auth.unjudged, strs(&["r2"]));
        assert!(!auth.clear);

        let payments = gate(&f, "payments");
        assert_eq!(payments.lane.as_deref(), Some("payments"));
        assert!(payments.open.is_empty());
        assert!(payments.unjudged.is_empty());
        assert!(payments.clear);
    }

    #[test]
    fn an_override_records_what_the_gate_saw_and_accumulates() {
        let f = setup();
        f.add_at(1, "one", "", "alice");
        let first = f
            .svc
            .override_gate(
                override_input(&f, "commit", "  ship the spike  ", "", "gustav"),
                Actor::Human,
            )
            .expect("override");
        assert_eq!(first.workspace, f.ws);
        assert_eq!(first.lane, None);
        assert_eq!(first.action, "commit");
        assert_eq!(first.reason, "ship the spike");
        assert_eq!(first.by, "gustav");
        assert_eq!(first.actor, Actor::Human);
        assert_eq!(first.at, AT);
        assert_eq!(first.open, strs(&["r1"]));
        assert!(first.unjudged.is_empty());

        let second = f
            .svc
            .override_gate(
                override_input(&f, "push", "reviewed by hand", "", "claude"),
                Actor::Agent,
            )
            .expect("second override");
        assert_eq!(second.actor, Actor::Agent);

        let recorded = f.svc.read(&f.ws, |tx| tx.overrides()).expect("overrides");
        assert_eq!(recorded, vec![first, second]);
    }

    #[test]
    fn override_ids_are_read_inside_the_write_not_supplied() {
        let f = setup();
        f.add_at(1, "one", "", "alice");
        let first = f
            .svc
            .override_gate(
                override_input(&f, "commit", "wip", "", "gustav"),
                Actor::Human,
            )
            .expect("first override");
        assert_eq!(first.open, strs(&["r1"]));

        f.pull("");
        resolve_done(&f, "r1");
        let second = f
            .svc
            .override_gate(
                override_input(&f, "push", "shipping", "", "gustav"),
                Actor::Human,
            )
            .expect("second override");
        assert!(second.open.is_empty());
        assert_eq!(second.unjudged, strs(&["r1"]));
    }

    #[test]
    fn an_override_refuses_bad_input_and_records_nothing() {
        let f = setup();
        f.add_at(1, "unlaned", "", "alice");

        let err = f
            .svc
            .override_gate(
                override_input(&f, "merge", "why", "", "gustav"),
                Actor::Human,
            )
            .unwrap_err();
        assert_eq!(err.kind, crate::error::ErrorKind::Invalid);
        assert_eq!(err.message, "action \"merge\" is not one of commit, push");

        let err = f
            .svc
            .override_gate(
                override_input(&f, "commit", "   ", "", "gustav"),
                Actor::Human,
            )
            .unwrap_err();
        assert_eq!(err.kind, crate::error::ErrorKind::Invalid);
        assert_eq!(err.message, "an override needs a reason");

        for author in ["", "   "] {
            let err = f
                .svc
                .override_gate(
                    override_input(&f, "commit", "why", "", author),
                    Actor::Human,
                )
                .unwrap_err();
            assert_eq!(err.kind, crate::error::ErrorKind::Invalid);
            assert_eq!(err.message, "an override needs an author");
        }

        // Lane "auth" is clear: the only undecided comment is unlaned.
        let err = f
            .svc
            .override_gate(
                override_input(&f, "commit", "why", "auth", "gustav"),
                Actor::Human,
            )
            .unwrap_err();
        assert_eq!(err.kind, crate::error::ErrorKind::Conflict);
        assert_eq!(
            err.message,
            "nothing is open or unjudged — no override needed"
        );

        let recorded = f.svc.read(&f.ws, |tx| tx.overrides()).expect("overrides");
        assert!(recorded.is_empty());
    }

    #[test]
    fn an_override_is_refused_when_the_gate_is_clear() {
        let f = setup();
        let err = f
            .svc
            .override_gate(
                override_input(&f, "push", "why", "", "gustav"),
                Actor::Human,
            )
            .unwrap_err();
        assert_eq!(err.kind, crate::error::ErrorKind::Conflict);
        assert_eq!(
            err.message,
            "nothing is open or unjudged — no override needed"
        );
    }
}
