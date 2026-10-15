//! Who made a commit, as far as rvw knows: a reported agent session, a
//! `Claude-Session` marker read from the commit message, or nobody (human).
//! Pure data in, a verdict out. Commit ids are matched by prefix, since a
//! report may carry an abbreviated id.

use rvw_core::types::Session;

/// The shortest commit id prefix that counts as a match.
const MIN_PREFIX: usize = 7;

/// Who made one commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Provenance {
    /// No report and no marker: a person made it.
    Human,
    /// An agent reported the session that made it.
    Agent {
        tool: String,
        model: String,
        session_id: String,
    },
    /// A marker names a session that was never reported.
    Marked { session_id: String },
}

/// The reported session that made a commit, or the one a marker names.
/// A report match wins over a marker.
#[must_use]
pub fn provenance(commit_id: &str, markers: &[String], sessions: &[Session]) -> Provenance {
    if let Some(session) = session_for(commit_id, markers, sessions) {
        return agent(session);
    }
    match markers.first() {
        Some(session_id) => Provenance::Marked {
            session_id: session_id.clone(),
        },
        None => Provenance::Human,
    }
}

/// The reported session behind a commit: the first report that lists it,
/// else the reported session a marker names. Among reports that list the
/// commit, the earliest `reported_at` wins, then the smaller `session_id`.
#[must_use]
pub fn session_for<'a>(
    commit_id: &str,
    markers: &[String],
    sessions: &'a [Session],
) -> Option<&'a Session> {
    let reported = sessions
        .iter()
        .filter(|session| session.commits.iter().any(|c| same_commit(commit_id, c)))
        .min_by(|a, b| {
            a.reported_at
                .cmp(&b.reported_at)
                .then_with(|| a.session_id.cmp(&b.session_id))
        });
    reported.or_else(|| {
        markers.iter().find_map(|marker| {
            sessions
                .iter()
                .find(|session| session.session_id == *marker)
        })
    })
}

/// The ids in `ids` that the session reported, in the order given.
#[must_use]
pub fn commits_of(session: &Session, ids: &[String]) -> Vec<String> {
    ids.iter()
        .filter(|id| session.commits.iter().any(|c| same_commit(id, c)))
        .cloned()
        .collect()
}

fn agent(session: &Session) -> Provenance {
    Provenance::Agent {
        tool: session.tool.clone(),
        model: session.model.clone(),
        session_id: session.session_id.clone(),
    }
}

/// Whether two commit ids name the same commit: one is a prefix of the
/// other, the shorter has at least `MIN_PREFIX` hex digits, case ignored.
fn same_commit(a: &str, b: &str) -> bool {
    let (a, b) = (a.to_ascii_lowercase(), b.to_ascii_lowercase());
    let (short, long) = if a.len() <= b.len() {
        (&a, &b)
    } else {
        (&b, &a)
    };
    short.len() >= MIN_PREFIX
        && short.bytes().all(|byte| byte.is_ascii_hexdigit())
        && long.starts_with(short.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: &str = "0123456789abcdef0123456789abcdef01234567";

    fn session(id: &str, reported_at: &str, commits: &[&str]) -> Session {
        Session {
            session_id: id.to_owned(),
            tool: "claude-code".to_owned(),
            model: "claude-x".to_owned(),
            summary: None,
            commits: commits.iter().map(|c| (*c).to_owned()).collect(),
            reported_at: reported_at.to_owned(),
        }
    }

    fn agent_of(id: &str) -> Provenance {
        Provenance::Agent {
            tool: "claude-code".to_owned(),
            model: "claude-x".to_owned(),
            session_id: id.to_owned(),
        }
    }

    fn markers(list: &[&str]) -> Vec<String> {
        list.iter().map(|m| (*m).to_owned()).collect()
    }

    #[test]
    fn provenance_table() {
        let reported = [session("s1", "2026-10-01T10:00:00Z", &["0123456"])];
        let cases: Vec<(&str, Vec<String>, Provenance)> = vec![
            // a reported abbreviated id matches the full commit
            (FULL, markers(&[]), agent_of("s1")),
            // no report, no marker: human
            ("ffffffffffff", markers(&[]), Provenance::Human),
            // a marker with no report: marked
            (
                "ffffffffffff",
                markers(&["s9"]),
                Provenance::Marked {
                    session_id: "s9".to_owned(),
                },
            ),
            // a marker whose session was reported: agent with that session
            ("ffffffffffff", markers(&["s1"]), agent_of("s1")),
            // a report match wins over a marker naming another session
            ("0123456789ab", markers(&["s9"]), agent_of("s1")),
        ];
        for (commit, markers, expected) in cases {
            assert_eq!(
                provenance(commit, &markers, &reported),
                expected,
                "{commit}"
            );
        }
    }

    #[test]
    fn prefix_shorter_than_seven_never_matches() {
        let reported = [session("s1", "2026-10-01T10:00:00Z", &["012345"])];
        assert_eq!(
            provenance(FULL, &markers(&[]), &reported),
            Provenance::Human
        );
        assert!(session_for("012345", &markers(&[]), &reported).is_none());
    }

    #[test]
    fn uppercase_ids_match() {
        let reported = [session("s1", "2026-10-01T10:00:00Z", &["0123ABC"])];
        assert_eq!(
            provenance("0123abc4567890", &markers(&[]), &reported),
            agent_of("s1")
        );
        assert_eq!(
            provenance("0123ABC4567890", &markers(&[]), &reported),
            agent_of("s1")
        );
        let upper_reported = [session("s2", "2026-10-01T10:00:00Z", &["0123ABCDEF0"])];
        assert_eq!(
            provenance("0123abcdef0123", &markers(&[]), &upper_reported),
            agent_of("s2")
        );
    }

    #[test]
    fn non_hex_or_mismatched_prefix_does_not_match() {
        let reported = [session("s1", "2026-10-01T10:00:00Z", &["0123456"])];
        assert_eq!(
            provenance("0123457", &markers(&[]), &reported),
            Provenance::Human
        );
        let not_hex = [session("s2", "2026-10-01T10:00:00Z", &["ghijklm"])];
        assert_eq!(
            provenance("ghijklm", &markers(&[]), &not_hex),
            Provenance::Human
        );
    }

    #[test]
    fn several_matching_sessions_pick_earliest_report() {
        let sessions = [
            session("late", "2026-10-02T10:00:00Z", &["0123456"]),
            session("early", "2026-10-01T10:00:00Z", &[FULL]),
        ];
        assert_eq!(
            provenance(FULL, &markers(&[]), &sessions),
            agent_of("early")
        );
    }

    #[test]
    fn equal_report_times_break_on_session_id() {
        let sessions = [
            session("b", "2026-10-01T10:00:00Z", &[FULL]),
            session("a", "2026-10-01T10:00:00Z", &["0123456"]),
        ];
        assert_eq!(
            session_for(FULL, &markers(&[]), &sessions).map(|s| s.session_id.as_str()),
            Some("a")
        );
    }

    #[test]
    fn empty_inputs() {
        assert_eq!(provenance("", &markers(&[]), &[]), Provenance::Human);
        assert!(session_for("", &markers(&[]), &[]).is_none());
        let reported = [session("s1", "2026-10-01T10:00:00Z", &[FULL])];
        assert_eq!(provenance("", &markers(&[]), &reported), Provenance::Human);
        assert!(commits_of(&reported[0], &[]).is_empty());
        assert!(commits_of(&reported[0], &markers(&[""])).is_empty());
    }

    #[test]
    fn session_for_table() {
        let sessions = [session("s1", "2026-10-01T10:00:00Z", &["0123456"])];
        assert_eq!(
            session_for(FULL, &markers(&[]), &sessions).map(|s| s.session_id.as_str()),
            Some("s1")
        );
        assert_eq!(
            session_for("ffffffffffff", &markers(&["s1"]), &sessions)
                .map(|s| s.session_id.as_str()),
            Some("s1")
        );
        assert!(session_for("ffffffffffff", &markers(&["s9"]), &sessions).is_none());
    }

    #[test]
    fn commits_of_keeps_matching_ids_in_order() {
        let s = session("s1", "2026-10-01T10:00:00Z", &["0123456", "abcdef1"]);
        let ids = markers(&["ABCDEF1234", "ffffffff", "0123456789", "0123"]);
        assert_eq!(commits_of(&s, &ids), markers(&["ABCDEF1234", "0123456789"]));
        assert!(commits_of(&s, &[]).is_empty());
    }
}
