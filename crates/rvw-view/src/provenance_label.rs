//! The words the History screen shows for who made a commit: the badge beside
//! its row, and the facts under its subject. Pure: a commit id, its trailer
//! markers and the reported sessions in, the text out. The GUI only draws it.

use rvw_core::types::Session;

use crate::provenance::{Provenance, provenance, session_for};

/// The longest badge text, in characters. A longer tool name is cut with an
/// ellipsis, so the badge column stays one width.
pub const BADGE_CHARS: usize = 11;

/// What a badge says about its commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BadgeKind {
    /// An agent reported the session that made the commit.
    Agent,
    /// A marker names a session nobody reported.
    Marked,
}

/// The short label beside a commit row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Badge {
    pub kind: BadgeKind,
    pub text: String,
}

/// The badge of a commit: the agent's tool, "marked", or nothing for a human.
#[must_use]
pub fn badge(commit_id: &str, markers: &[String], sessions: &[Session]) -> Option<Badge> {
    match provenance(commit_id, markers, sessions) {
        Provenance::Human => None,
        Provenance::Agent { tool, .. } => Some(Badge {
            kind: BadgeKind::Agent,
            text: clip(&tool, BADGE_CHARS),
        }),
        Provenance::Marked { .. } => Some(Badge {
            kind: BadgeKind::Marked,
            text: "marked".to_owned(),
        }),
    }
}

/// The facts under a commit's subject, one line each. An agent's commit gives
/// "tool · model", the summary's first non-blank line when the session has
/// one, and "session <id>". A marked commit gives "session <id> (not
/// reported)". A human's commit gives none.
#[must_use]
pub fn detail_facts(commit_id: &str, markers: &[String], sessions: &[Session]) -> Vec<String> {
    match provenance(commit_id, markers, sessions) {
        Provenance::Human => Vec::new(),
        Provenance::Agent {
            tool,
            model,
            session_id,
        } => {
            let summary = session_for(commit_id, markers, sessions)
                .and_then(|session| session.summary.as_deref())
                .and_then(first_line);
            let mut lines = vec![who(&tool, &model)];
            lines.extend(summary);
            lines.push(format!("session {session_id}"));
            lines
        }
        Provenance::Marked { session_id } => {
            vec![format!("session {session_id} (not reported)")]
        }
    }
}

/// "tool · model", or only the part that is not empty.
pub(crate) fn who(tool: &str, model: &str) -> String {
    [tool, model]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" \u{00b7} ")
}

/// The first line of `text` that has anything in it, trimmed.
pub(crate) fn first_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_owned)
}

/// `text` as is when it has at most `max` characters, else its first
/// `max - 1` characters and an ellipsis.
fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut clipped: String = text.chars().take(max.saturating_sub(1)).collect();
    clipped.push('\u{2026}');
    clipped
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: &str = "0123456789abcdef0123456789abcdef01234567";

    fn session(id: &str, tool: &str, model: &str, summary: Option<&str>) -> Session {
        Session {
            session_id: id.to_owned(),
            tool: tool.to_owned(),
            model: model.to_owned(),
            summary: summary.map(str::to_owned),
            commits: vec![FULL[..7].to_owned()],
            reported_at: "2026-10-01T10:00:00Z".to_owned(),
        }
    }

    fn markers(list: &[&str]) -> Vec<String> {
        list.iter().map(|m| (*m).to_owned()).collect()
    }

    #[test]
    fn badge_table() {
        let sessions = [session("s1", "claude-code", "claude-x", None)];
        let cases: Vec<(&str, Vec<String>, Option<Badge>)> = vec![
            // no report, no marker: a person, no badge
            ("ffffffffffff", markers(&[]), None),
            // a reported commit: the tool, in the agent kind
            (
                FULL,
                markers(&[]),
                Some(Badge {
                    kind: BadgeKind::Agent,
                    text: "claude-code".to_owned(),
                }),
            ),
            // a marker that names no report: marked
            (
                "ffffffffffff",
                markers(&["s9"]),
                Some(Badge {
                    kind: BadgeKind::Marked,
                    text: "marked".to_owned(),
                }),
            ),
        ];
        for (commit, markers, expected) in cases {
            assert_eq!(badge(commit, &markers, &sessions), expected, "{commit}");
        }
    }

    #[test]
    fn badge_clips_a_long_tool_name() {
        let sessions = [session("s1", "claude-code-extra", "m", None)];
        let text = badge(FULL, &[], &sessions).expect("agent").text;
        assert_eq!(text, "claude-cod\u{2026}");
        assert_eq!(text.chars().count(), BADGE_CHARS);
    }

    #[test]
    fn clip_keeps_short_and_exact_names() {
        assert_eq!(clip("", 3), "");
        assert_eq!(clip("abc", 3), "abc");
        assert_eq!(clip("abcd", 3), "ab\u{2026}");
        assert_eq!(clip("abcd", 1), "\u{2026}");
        assert_eq!(clip("abcd", 0), "\u{2026}");
    }

    #[test]
    fn detail_facts_table() {
        let cases: Vec<(&str, Vec<String>, Vec<String>)> = vec![
            // a person: nothing to say
            ("ffffffffffff", markers(&[]), vec![]),
            // an agent with no summary: who, then the session
            (
                FULL,
                markers(&[]),
                vec![
                    "claude-code \u{00b7} claude-x".to_owned(),
                    "session s1".to_owned(),
                ],
            ),
            // a marked commit: one line
            (
                "ffffffffffff",
                markers(&["s9"]),
                vec!["session s9 (not reported)".to_owned()],
            ),
        ];
        let sessions = [session("s1", "claude-code", "claude-x", None)];
        for (commit, markers, expected) in cases {
            assert_eq!(
                detail_facts(commit, &markers, &sessions),
                expected,
                "{commit}"
            );
        }
    }

    #[test]
    fn detail_facts_summary_is_its_first_non_blank_line() {
        let cases = [
            (Some("fixed the parser"), "fixed the parser"),
            (Some("   \n  \n"), ""),
            (Some("\n\n   second  \nthird"), "second"),
            (Some("  first  \nsecond"), "first"),
        ];
        for (summary, shown) in cases {
            let sessions = [session("s1", "claude-code", "claude-x", summary)];
            let facts = detail_facts(FULL, &markers(&[]), &sessions);
            if shown.is_empty() {
                assert_eq!(facts.len(), 2, "{summary:?} gives no summary line");
            } else {
                assert_eq!(facts[1], shown, "{summary:?}");
                assert_eq!(facts.len(), 3, "{summary:?}");
            }
            assert_eq!(facts.last().map(String::as_str), Some("session s1"));
        }
    }

    #[test]
    fn detail_facts_skip_an_empty_model() {
        let sessions = [session("s1", "claude-code", "", None)];
        assert_eq!(
            detail_facts(FULL, &markers(&[]), &sessions)[0],
            "claude-code"
        );
    }

    #[test]
    fn detail_facts_follow_a_marker_to_its_report() {
        // a marker naming a reported session shows that session's facts
        let sessions = [session("s1", "claude-code", "claude-x", Some("did it"))];
        assert_eq!(
            detail_facts("ffffffffffff", &markers(&["s1"]), &sessions),
            vec![
                "claude-code \u{00b7} claude-x".to_owned(),
                "did it".to_owned(),
                "session s1".to_owned(),
            ]
        );
    }
}
