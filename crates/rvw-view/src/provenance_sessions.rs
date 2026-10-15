//! The Sessions timeline and a session's commit list, as the History screen
//! shows them: the row text, the order, the empty state, what a session's
//! list is missing, and the order of its commits. Pure: sessions and commits
//! in, text and order out. The GUI only draws it.

use rvw_core::types::Session;
use rvw_git::history::LogEntry;

use crate::history::ago;
use crate::provenance::same_commit;
use crate::provenance_label::{first_line, who};

/// What the Sessions timeline says when no session was reported.
pub const EMPTY: &str =
    "No sessions reported yet. Agents report them with the report_session tool.";

/// One row of the Sessions timeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub session_id: String,
    /// "tool · model", or only the part that is not empty.
    pub who: String,
    /// The summary's first non-blank line; empty when there is none.
    pub summary: String,
    /// "1 commit" or "N commits", as reported.
    pub commits: String,
    /// How long ago the first report came, as the list shows ages; empty
    /// when the report's time cannot be read.
    pub age: String,
}

/// The rows of the timeline, newest first, at `now` (seconds since the epoch).
#[must_use]
pub fn rows(sessions: &[Session], now: i64) -> Vec<Row> {
    ordered(sessions)
        .into_iter()
        .map(|session| Row {
            session_id: session.session_id.clone(),
            who: who(&session.tool, &session.model),
            summary: session
                .summary
                .as_deref()
                .and_then(first_line)
                .unwrap_or_default(),
            commits: commit_count(session.commits.len()),
            age: parse_time(&session.reported_at)
                .map(|at| ago(now, at))
                .unwrap_or_default(),
        })
        .collect()
}

/// The sessions newest first: by when the first report came, then by id.
/// A report time that cannot be read sorts last.
#[must_use]
pub fn ordered(sessions: &[Session]) -> Vec<&Session> {
    let mut list: Vec<&Session> = sessions.iter().collect();
    list.sort_by(|a, b| {
        parse_time(&b.reported_at)
            .cmp(&parse_time(&a.reported_at))
            .then_with(|| a.session_id.cmp(&b.session_id))
    });
    list
}

/// The empty state of the timeline, or None when there is a session to list.
#[must_use]
pub fn empty_state(sessions: &[Session]) -> Option<&'static str> {
    sessions.is_empty().then_some(EMPTY)
}

/// "1 commit" or "N commits".
#[must_use]
pub fn commit_count(count: usize) -> String {
    if count == 1 {
        "1 commit".to_owned()
    } else {
        format!("{count} commits")
    }
}

/// How many of a session's reported commits none of `found` is: the commits
/// that are not in this repository. `found` holds the ids of the commits the
/// list shows; an abbreviated report matches its full id.
#[must_use]
pub fn missing(session: &Session, found: &[&str]) -> usize {
    session
        .commits
        .iter()
        .filter(|commit| !found.iter().any(|id| same_commit(commit, id)))
        .count()
}

/// The line that says a session's commits are missing, or None when none are.
#[must_use]
pub fn missing_note(missing: usize) -> Option<String> {
    match missing {
        0 => None,
        1 => Some("1 commit of this session is not in this repository.".to_owned()),
        n => Some(format!(
            "{n} commits of this session are not in this repository."
        )),
    }
}

/// Puts a session's commits newest first by their author time. The sort is
/// stable, so commits at the same time keep the order they came in.
pub fn sort_newest_first(entries: &mut [LogEntry]) {
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.time));
}

/// Seconds since the epoch of an RFC 3339 stamp such as the ones rvw writes,
/// `2026-10-12T09:00:00+00:00`. A fraction of a second is allowed, and `Z`
/// or a `±HH:MM` offset. None for anything else.
#[must_use]
pub fn parse_time(stamp: &str) -> Option<i64> {
    let (date, rest) = stamp.split_once('T')?;
    let mut parts = date.split('-');
    let year = parts.next()?.parse::<i64>().ok()?;
    let month = parts.next()?.parse::<i64>().ok()?;
    let day = parts.next()?.parse::<i64>().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let cut = rest.find(['Z', '+', '-']).unwrap_or(rest.len());
    let (clock, zone) = rest.split_at(cut);
    let clock = clock.split_once('.').map_or(clock, |(whole, _)| whole);
    let mut fields = clock.split(':');
    let hour = fields.next()?.parse::<i64>().ok()?;
    let minute = fields.next()?.parse::<i64>().ok()?;
    let second = fields.next()?.parse::<i64>().ok()?;
    if fields.next().is_some()
        || !(0..24).contains(&hour)
        || !(0..60).contains(&minute)
        || !(0..=60).contains(&second)
    {
        return None;
    }
    let offset = zone_offset(zone)?;
    let days = days_from_civil(year, month, day);
    Some(days * 86_400 + hour * 3_600 + minute * 60 + second - offset)
}

/// The seconds east of UTC that `zone` names: "Z", or "+HH:MM" or "-HH:MM".
fn zone_offset(zone: &str) -> Option<i64> {
    if zone == "Z" {
        return Some(0);
    }
    let (sign, digits) = match zone.as_bytes().first()? {
        b'+' => (1, &zone[1..]),
        b'-' => (-1, &zone[1..]),
        _ => return None,
    };
    let (hours, minutes) = digits.split_once(':')?;
    let hours = hours.parse::<i64>().ok()?;
    let minutes = minutes.parse::<i64>().ok()?;
    if !(0..24).contains(&hours) || !(0..60).contains(&minutes) {
        return None;
    }
    Some(sign * (hours * 3_600 + minutes * 60))
}

/// The day number of a civil date, counted from 1970-01-01 (Howard Hinnant's
/// algorithm, valid for every year this program can see).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let year_of_era = y - era * 400;
    let month_from_march = (month + 9) % 12;
    let day_of_year = (153 * month_from_march + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: &str = "0123456789abcdef0123456789abcdef01234567";

    fn session(id: &str, reported_at: &str, commits: &[&str], summary: Option<&str>) -> Session {
        Session {
            session_id: id.to_owned(),
            tool: "claude-code".to_owned(),
            model: "opus".to_owned(),
            summary: summary.map(str::to_owned),
            commits: commits.iter().map(|c| (*c).to_owned()).collect(),
            reported_at: reported_at.to_owned(),
        }
    }

    fn entry(id: &str, time: i64) -> LogEntry {
        LogEntry {
            id: id.to_owned(),
            parents: Vec::new(),
            author: String::new(),
            email: String::new(),
            time,
            refs: Vec::new(),
            subject: String::new(),
            session_markers: Vec::new(),
        }
    }

    #[test]
    fn parse_time_reads_the_stamps_rvw_writes() {
        assert_eq!(parse_time("1970-01-01T00:00:00+00:00"), Some(0));
        assert_eq!(parse_time("2027-01-15T08:00:00+00:00"), Some(1_800_000_000));
        // a fraction, Z, and an offset name the same instant
        assert_eq!(parse_time("2027-01-15T08:00:00.999Z"), Some(1_800_000_000));
        assert_eq!(parse_time("2027-01-15T10:00:00+02:00"), Some(1_800_000_000));
        assert_eq!(parse_time("2027-01-15T05:00:00-03:00"), Some(1_800_000_000));
        // leap day
        assert_eq!(parse_time("2028-02-29T00:00:00Z"), Some(1_835_395_200));
    }

    #[test]
    fn parse_time_refuses_what_it_cannot_read() {
        for bad in [
            "",
            "2027-01-15",
            "garbage",
            "2027-13-01T00:00:00Z",
            "2027-01-15T24:00:00Z",
            "2027-01-15T08:00:00+25:00",
            "2027-01-15T08:00:00 +00:00",
            "2027-01-15T08:00",
            "2027-01-15T08:00:00+0000",
        ] {
            assert_eq!(parse_time(bad), None, "{bad}");
        }
    }

    #[test]
    fn ordered_is_newest_first_then_by_id_and_unreadable_last() {
        let sessions = [
            session("old", "2026-10-01T10:00:00+00:00", &[], None),
            session("junk", "not a time", &[], None),
            session("b", "2026-10-02T10:00:00+00:00", &[], None),
            session("a", "2026-10-02T10:00:00+00:00", &[], None),
        ];
        let ids: Vec<&str> = ordered(&sessions)
            .into_iter()
            .map(|s| s.session_id.as_str())
            .collect();
        assert_eq!(ids, ["a", "b", "old", "junk"]);
    }

    #[test]
    fn rows_say_who_what_how_many_and_how_old() {
        let now = 1_800_000_000 + 3 * 86_400;
        let sessions = [
            session(
                "s1",
                "2027-01-15T08:00:00+00:00",
                &[FULL],
                Some("  \nFixed the parser\nmore detail"),
            ),
            session("s2", "2027-01-14T08:00:00+00:00", &[FULL, "abcdef1"], None),
        ];
        let rows = rows(&sessions, now);
        assert_eq!(
            rows[0],
            Row {
                session_id: "s1".to_owned(),
                who: "claude-code \u{00b7} opus".to_owned(),
                summary: "Fixed the parser".to_owned(),
                commits: "1 commit".to_owned(),
                age: "3d".to_owned(),
            }
        );
        assert_eq!(rows[1].summary, "");
        assert_eq!(rows[1].commits, "2 commits");
        assert_eq!(rows[1].age, "4d");
    }

    #[test]
    fn a_row_without_a_readable_time_has_no_age() {
        let rows = rows(&[session("s1", "junk", &[], None)], 0);
        assert_eq!(rows[0].age, "");
        assert_eq!(rows[0].commits, "0 commits");
    }

    #[test]
    fn empty_state_only_for_no_sessions() {
        assert_eq!(empty_state(&[]), Some(EMPTY));
        assert_eq!(
            EMPTY,
            "No sessions reported yet. Agents report them with the report_session tool."
        );
        let one = [session("s1", "2027-01-15T08:00:00+00:00", &[], None)];
        assert_eq!(empty_state(&one), None);
    }

    #[test]
    fn commit_count_is_singular_only_for_one() {
        assert_eq!(commit_count(0), "0 commits");
        assert_eq!(commit_count(1), "1 commit");
        assert_eq!(commit_count(2), "2 commits");
    }

    #[test]
    fn missing_counts_reported_commits_no_listed_commit_matches() {
        let s = session(
            "s1",
            "2027-01-15T08:00:00+00:00",
            &["0123456", "abcdef1", "fedcba9"],
            None,
        );
        // the abbreviated report matches its full id; the others are absent
        assert_eq!(missing(&s, &[FULL]), 2);
        assert_eq!(
            missing(
                &s,
                &[
                    FULL,
                    "abcdef1234567890abcdef1234567890abcdef12",
                    "fedcba9876"
                ]
            ),
            0
        );
        assert_eq!(missing(&s, &[]), 3);
        // a prefix shorter than seven characters never matches
        assert_eq!(
            missing(&session("s2", "x", &["0123456"], None), &["012345"]),
            1
        );
    }

    #[test]
    fn missing_note_only_when_something_is_missing() {
        assert_eq!(missing_note(0), None);
        assert_eq!(
            missing_note(1).as_deref(),
            Some("1 commit of this session is not in this repository.")
        );
        assert_eq!(
            missing_note(3).as_deref(),
            Some("3 commits of this session are not in this repository.")
        );
    }

    #[test]
    fn sort_newest_first_keeps_equal_times_in_order() {
        let mut entries = vec![
            entry("a", 10),
            entry("b", 30),
            entry("c", 10),
            entry("d", 20),
        ];
        sort_newest_first(&mut entries);
        let ids: Vec<&str> = entries.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["b", "d", "a", "c"]);
        sort_newest_first(&mut []);
    }
}
