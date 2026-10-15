//! "Why this line": the reported session that last changed the lines a
//! comment is written on, and the summary it gave. Pure: commits, their
//! trailer markers and the reported sessions in, the line out. The GUI only
//! draws it.

use rvw_core::types::Session;

use crate::provenance::session_for;
use crate::provenance_label::{first_line, who};

/// The reported session behind the most recent blamed commit of a range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhyLine {
    /// The agent tool that ran the session, e.g. claude-code; may be empty.
    pub tool: String,
    /// The model the agent reported; may be empty.
    pub model: String,
    /// The summary's first non-blank line; None when the session gave none.
    pub summary_first_line: Option<String>,
    pub session_id: String,
    /// The blamed commit the session was chosen by, as the caller gave it.
    pub commit_id: String,
}

/// The session of the most recent blamed commit that a reported session owns.
///
/// `blamed_commits` is ordered oldest first, so the last commit in the list
/// that a reported session owns wins; the caller sorts by time and breaks ties
/// by the later line. `markers` is parallel to `blamed_commits`: the trailer
/// markers of each commit, empty when missing. None when no blamed commit is
/// owned by a reported session.
#[must_use]
pub fn why_line(
    blamed_commits: &[String],
    markers: &[Vec<String>],
    sessions: &[Session],
) -> Option<WhyLine> {
    blamed_commits
        .iter()
        .enumerate()
        .rev()
        .find_map(|(index, commit_id)| {
            let markers = markers.get(index).map_or(&[][..], Vec::as_slice);
            let session = session_for(commit_id, markers, sessions)?;
            Some(WhyLine {
                tool: session.tool.clone(),
                model: session.model.clone(),
                summary_first_line: session.summary.as_deref().and_then(first_line),
                session_id: session.session_id.clone(),
                commit_id: commit_id.clone(),
            })
        })
}

/// The one line the composer shows: `tool · model — "summary first line"`.
/// The summary part is left out when the session gave none; the tool and
/// model are only the parts that are not empty.
#[must_use]
pub fn text(line: &WhyLine) -> String {
    let who = who(&line.tool, &line.model);
    match (who.is_empty(), line.summary_first_line.as_deref()) {
        (_, None) => who,
        (true, Some(summary)) => format!("\"{summary}\""),
        (false, Some(summary)) => format!("{who} \u{2014} \"{summary}\""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "aaaaaaaa11111111111111111111111111111111";
    const B: &str = "bbbbbbbb22222222222222222222222222222222";
    const C: &str = "cccccccc33333333333333333333333333333333";
    const D: &str = "dddddddd44444444444444444444444444444444";

    fn session(id: &str, summary: Option<&str>, commits: &[&str]) -> Session {
        Session {
            session_id: id.to_owned(),
            tool: "claude-code".to_owned(),
            model: "claude-x".to_owned(),
            summary: summary.map(str::to_owned),
            commits: commits.iter().map(|c| (*c).to_owned()).collect(),
            reported_at: "2026-10-01T10:00:00Z".to_owned(),
        }
    }

    type Case<'a> = (&'a str, Vec<&'a str>, Option<(&'a str, Option<&'a str>)>);

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|c| (*c).to_owned()).collect()
    }

    /// The session id and summary of the result, or None.
    fn picked(result: Option<WhyLine>) -> Option<(String, Option<String>)> {
        result.map(|line| (line.session_id, line.summary_first_line))
    }

    #[test]
    fn why_line_table() {
        let sessions = [
            session("s1", Some("Fix the parser\nlonger detail"), &[A]),
            session("s2", Some("Rename the module"), &[C]),
            session("s3", None, &[B]),
            session("s4", Some("   \n  "), &[D]),
        ];
        let cases: Vec<Case<'_>> = vec![
            // no blamed commit is owned by a reported session
            ("no owner", vec!["ffffffffffffffff"], None),
            ("nothing blamed", vec![], None),
            // one owner: its first summary line
            ("one owner", vec![A], Some(("s1", Some("Fix the parser")))),
            // the most recent owned commit wins, the later one in the list
            (
                "later wins",
                vec![A, C],
                Some(("s2", Some("Rename the module"))),
            ),
            // a human commit after the agent's does not hide it
            (
                "human after",
                vec![A, "dddddddddddddddd"],
                Some(("s1", Some("Fix the parser"))),
            ),
            // a session with no summary shows with none
            ("no summary", vec![A, B], Some(("s3", None))),
            // a blank summary is none, not an empty line
            ("blank summary", vec![D], Some(("s4", None))),
            // an abbreviated report id matches the full blamed id
            ("abbreviated", vec!["bbbbbbbb2222"], Some(("s3", None))),
        ];
        for (name, commits, expected) in cases {
            let commits = ids(&commits);
            let expected =
                expected.map(|(id, summary)| (id.to_owned(), summary.map(str::to_owned)));
            assert_eq!(
                picked(why_line(&commits, &[], &sessions)),
                expected,
                "{name}"
            );
        }
    }

    #[test]
    fn a_marker_naming_a_reported_session_owns_the_commit() {
        let sessions = [session("s1", Some("Fix"), &[])];
        let markers = vec![vec!["s1".to_owned()]];
        let line = why_line(&ids(&[A]), &markers, &sessions).expect("marker owns A");
        assert_eq!(line.session_id, "s1");
        assert_eq!(line.commit_id, A);
        assert!(why_line(&ids(&[A]), &[], &sessions).is_none());
    }

    #[test]
    fn missing_markers_count_as_none() {
        let sessions = [session("s2", Some("Rename"), &[C])];
        assert_eq!(
            picked(why_line(&ids(&[A, C]), &[], &sessions)),
            Some(("s2".to_owned(), Some("Rename".to_owned())))
        );
    }

    #[test]
    fn text_table() {
        let line = |tool: &str, model: &str, summary: Option<&str>| WhyLine {
            tool: tool.to_owned(),
            model: model.to_owned(),
            summary_first_line: summary.map(str::to_owned),
            session_id: "s1".to_owned(),
            commit_id: A.to_owned(),
        };
        let cases = [
            (
                line("claude-code", "claude-x", Some("Fix")),
                "claude-code \u{b7} claude-x \u{2014} \"Fix\"",
            ),
            (
                line("claude-code", "claude-x", None),
                "claude-code \u{b7} claude-x",
            ),
            (
                line("claude-code", "", Some("Fix")),
                "claude-code \u{2014} \"Fix\"",
            ),
            (line("", "", Some("Fix")), "\"Fix\""),
            (line("", "", None), ""),
        ];
        for (input, expected) in cases {
            assert_eq!(text(&input), expected);
        }
    }
}
