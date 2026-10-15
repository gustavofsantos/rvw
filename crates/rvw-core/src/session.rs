//! Sessions: an agent reports the session it ran and the commits it made.
//! Reports are append-only: a repeat report for a known session adds commits,
//! and its tool or model must match the first report's.

use crate::error::{Error, Result};
use crate::operations::{ReportSessionInput, ReportSessionOutput, SessionsInput, SessionsOutput};
use crate::service::Service;
use crate::types::Session;

/// Commit ids are abbreviated or full hex names: 7 to 40 characters.
const MIN_COMMIT_LEN: usize = 7;
const MAX_COMMIT_LEN: usize = 40;

#[allow(clippy::needless_pass_by_value, clippy::missing_errors_doc)] // operations take their input by value, as the adapters build it per call
impl Service {
    /// Records a session an agent ran, or adds commits to a session already
    /// reported. The first report fixes the tool, model and summary; a later
    /// report with another tool or model is refused and changes nothing.
    pub fn report_session(&self, input: ReportSessionInput) -> Result<ReportSessionOutput> {
        let session_id = required("session id", &input.session_id)?;
        let tool = required("tool", &input.tool)?;
        let model = required("model", &input.model)?;
        let commits = commit_ids(&input.commits)?;
        let summary = input.summary.trim().to_string();

        let session = self.write(&input.workspace, |tx| {
            match tx.session(&session_id)? {
                None => tx.insert_session(&Session {
                    session_id: session_id.clone(),
                    tool,
                    model,
                    summary: (!summary.is_empty()).then_some(summary),
                    commits,
                    reported_at: self.stamp(),
                })?,
                Some(known) => {
                    if known.tool != tool || known.model != model {
                        return Err(Error::invalid(format!(
                            "session {session_id} was reported by {}/{}, not {tool}/{model}",
                            known.tool, known.model
                        )));
                    }
                    tx.add_session_commits(&session_id, &commits)?;
                }
            }
            tx.session(&session_id)?
                .ok_or_else(|| Error::internal(format!("session {session_id} was not stored")))
        })?;
        Ok(ReportSessionOutput { session })
    }

    /// The workspace's reported sessions, newest first.
    pub fn sessions(&self, input: SessionsInput) -> Result<SessionsOutput> {
        let sessions = self.read(&input.workspace, |tx| tx.sessions())?;
        Ok(SessionsOutput { sessions })
    }
}

/// A required text field, trimmed; blank is refused by name.
fn required(field: &str, value: &str) -> Result<String> {
    let text = value.trim();
    if text.is_empty() {
        return Err(Error::invalid(format!("a session report needs a {field}")));
    }
    Ok(text.to_string())
}

/// The reported commits as stored: lowercased, duplicates collapsed, in the
/// order first given. An id that is not 7 to 40 hex characters is refused.
fn commit_ids(commits: &[String]) -> Result<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for commit in commits {
        let well_formed = (MIN_COMMIT_LEN..=MAX_COMMIT_LEN).contains(&commit.len())
            && commit.bytes().all(|b| b.is_ascii_hexdigit());
        if !well_formed {
            return Err(Error::invalid(format!(
                "commit {commit} is not a commit id: want {MIN_COMMIT_LEN} to {MAX_COMMIT_LEN} hex characters"
            )));
        }
        let id = commit.to_ascii_lowercase();
        if !out.contains(&id) {
            out.push(id);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicI64, Ordering};

    use time::{Date, Duration, Month};

    use super::*;
    use crate::error::ErrorKind;
    use crate::service::testing::MemRepo;

    const WS: &str = "/work/proj";
    const FIRST_AT: &str = "2026-10-12T09:00:00+00:00";

    /// A service whose clock moves one second on every stamp, so each report
    /// gets a later `reported_at` than the one before it.
    fn service() -> Service {
        let base = Date::from_calendar_date(2026, Month::October, 12)
            .expect("valid date")
            .with_hms(9, 0, 0)
            .expect("valid time")
            .assume_utc();
        let ticks = Arc::new(AtomicI64::new(0));
        Service::with_clock(
            Box::new(MemRepo::default()),
            Box::new(move || {
                let n = ticks.fetch_add(1, Ordering::SeqCst);
                base + Duration::seconds(n)
            }),
        )
    }

    fn report(
        svc: &Service,
        session_id: &str,
        model: &str,
        summary: &str,
        commits: &[&str],
    ) -> Result<ReportSessionOutput> {
        svc.report_session(ReportSessionInput {
            workspace: WS.into(),
            session_id: session_id.into(),
            tool: "claude-code".into(),
            model: model.into(),
            summary: summary.into(),
            commits: commits.iter().map(|c| (*c).to_string()).collect(),
        })
    }

    fn all(svc: &Service) -> Vec<crate::types::Session> {
        svc.sessions(SessionsInput {
            workspace: WS.into(),
        })
        .expect("sessions")
        .sessions
    }

    #[test]
    fn a_first_report_stores_the_session_with_its_commits() {
        let svc = service();
        let out = report(&svc, "s1", "opus", "  fixed the parser  ", &["abcdef1"]).expect("report");
        let s = out.session;
        assert_eq!(s.session_id, "s1");
        assert_eq!(s.tool, "claude-code");
        assert_eq!(s.model, "opus");
        assert_eq!(s.summary.as_deref(), Some("fixed the parser"));
        assert_eq!(s.commits, vec!["abcdef1".to_string()]);
        assert_eq!(s.reported_at, FIRST_AT);
        assert_eq!(all(&svc), vec![s]);
    }

    #[test]
    fn a_blank_summary_is_stored_as_none() {
        let svc = service();
        let s = report(&svc, "s1", "opus", "   ", &[])
            .expect("report")
            .session;
        assert_eq!(s.summary, None);
        assert!(s.commits.is_empty());
    }

    #[test]
    fn a_repeat_report_adds_commits_and_keeps_the_first_summary() {
        let svc = service();
        let first = report(&svc, "s1", "opus", "first words", &["abcdef1"])
            .expect("first")
            .session;
        let again = report(&svc, "s1", "opus", "later words", &["1234567"])
            .expect("repeat")
            .session;
        assert_eq!(
            again.commits,
            vec!["abcdef1".to_string(), "1234567".to_string()]
        );
        assert_eq!(again.summary, first.summary);
        assert_eq!(again.tool, first.tool);
        assert_eq!(again.model, first.model);
        assert_eq!(again.reported_at, first.reported_at);
        assert_eq!(all(&svc), vec![again]);
    }

    #[test]
    fn a_repeat_with_another_model_is_refused_and_changes_nothing() {
        let svc = service();
        report(&svc, "s1", "opus", "", &["abcdef1"]).expect("first");
        let before = all(&svc);

        let err = report(&svc, "s1", "sonnet", "", &["1234567"]).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Invalid);
        assert!(!err.message.is_empty());
        assert_eq!(all(&svc), before);
    }

    #[test]
    fn a_repeat_with_another_tool_is_refused_and_changes_nothing() {
        let svc = service();
        report(&svc, "s1", "opus", "", &["abcdef1"]).expect("first");
        let before = all(&svc);

        let err = svc
            .report_session(ReportSessionInput {
                workspace: WS.into(),
                session_id: "s1".into(),
                tool: "codex".into(),
                model: "opus".into(),
                summary: String::new(),
                commits: vec!["1234567".into()],
            })
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::Invalid);
        assert_eq!(all(&svc), before);
    }

    #[test]
    fn an_empty_session_tool_or_model_is_invalid() {
        let svc = service();
        for (id, model) in [("  ", "opus"), ("s1", " ")] {
            let err = report(&svc, id, model, "", &[]).unwrap_err();
            assert_eq!(err.kind, ErrorKind::Invalid, "id {id:?} model {model:?}");
            assert!(!err.message.is_empty());
        }
        let err = svc
            .report_session(ReportSessionInput {
                workspace: WS.into(),
                session_id: "s1".into(),
                tool: String::new(),
                model: "opus".into(),
                summary: String::new(),
                commits: Vec::new(),
            })
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::Invalid);
        assert!(all(&svc).is_empty(), "a refused report stores nothing");
    }

    #[test]
    fn a_short_or_non_hex_or_long_commit_is_invalid_and_named() {
        let svc = service();
        for bad in [
            "abc123",
            "zzzzzzz",
            "abcdef1234567890abcdef1234567890abcdef123",
        ] {
            let err = report(&svc, "s1", "opus", "", &[bad]).unwrap_err();
            assert_eq!(err.kind, ErrorKind::Invalid, "{bad}");
            assert!(err.message.contains(bad), "names the id: {}", err.message);
        }
        assert!(all(&svc).is_empty(), "a refused report stores nothing");
    }

    #[test]
    fn commit_ids_are_stored_lowercased() {
        let svc = service();
        let s = report(
            &svc,
            "s1",
            "opus",
            "",
            &["ABCDEF1", "ABCDEF1234567890ABCDEF1234567890ABCDEF12"],
        )
        .expect("report")
        .session;
        assert_eq!(
            s.commits,
            vec![
                "abcdef1".to_string(),
                "abcdef1234567890abcdef1234567890abcdef12".to_string()
            ]
        );
    }

    #[test]
    fn duplicate_commits_collapse_within_and_across_reports() {
        let svc = service();
        let s = report(
            &svc,
            "s1",
            "opus",
            "",
            &["abcdef1", "ABCDEF1", "abcdef1234"],
        )
        .expect("first")
        .session;
        assert_eq!(
            s.commits,
            vec!["abcdef1".to_string(), "abcdef1234".to_string()]
        );

        let s = report(&svc, "s1", "opus", "", &["abcdef1234", "1234567"])
            .expect("repeat")
            .session;
        assert_eq!(
            s.commits,
            vec![
                "abcdef1".to_string(),
                "abcdef1234".to_string(),
                "1234567".to_string()
            ]
        );
    }

    #[test]
    fn sessions_are_listed_newest_first_and_an_empty_workspace_has_none() {
        let svc = service();
        assert!(all(&svc).is_empty());

        report(&svc, "old", "opus", "", &[]).expect("old");
        report(&svc, "mid", "opus", "", &[]).expect("mid");
        report(&svc, "new", "opus", "", &[]).expect("new");
        let ids: Vec<String> = all(&svc).into_iter().map(|s| s.session_id).collect();
        assert_eq!(ids, vec!["new", "mid", "old"]);
    }
}
