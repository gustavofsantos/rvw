//! The queue as the reviewer sees it: groups, states and counters.

use std::collections::{HashMap, HashSet};
use std::hash::BuildHasher;

use rvw_core::operations::ReviewSheet;
use rvw_core::types::{parse_seq, Comment, Review, Status};

/// The title length limit, in characters, ellipsis included. A title longer
/// than this is cut to `TITLE_MAX_CHARS - 1` characters (59) followed by `…`,
/// so a cut title is exactly 60 characters. A title of exactly 60 is not cut.
const TITLE_MAX_CHARS: usize = 60;

/// Where a comment stands from the reviewer's side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Draft,
    NotYetTaken,
    WithAgent,
    WaitingOnYou,
    Closed,
}

impl State {
    pub fn label(self) -> &'static str {
        match self {
            State::Draft => "In your draft",
            State::NotYetTaken => "Not yet taken",
            State::WithAgent => "With an agent",
            State::WaitingOnYou => "Waiting on you",
            State::Closed => "Closed",
        }
    }

    pub fn glyph(self) -> &'static str {
        match self {
            State::Draft | State::NotYetTaken => "○",
            State::WithAgent => "◐",
            State::WaitingOnYou | State::Closed => "✓",
        }
    }
}

/// The glyph for a comment, which shows a rejection apart from a done one.
pub fn glyph_of(comment: &Comment, state: State) -> &'static str {
    if comment.status == Status::Rejected {
        "✗"
    } else {
        state.glyph()
    }
}

/// One comment as a row of the queue home screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// comment id, e.g. "r9"
    pub id: String,
    /// first non-empty line of the comment text, trimmed. A line over 60
    /// characters is cut to 59 characters plus `…`, so 60 in total (see
    /// `TITLE_MAX_CHARS`); a line of exactly 60 characters is kept whole.
    pub title: String,
    /// `file:N` or `file:N-M`
    pub location: String,
    /// author, or "" when none
    pub author: String,
    /// lane, or "" when none
    pub lane: String,
    pub state: State,
    /// who recorded the decision, or ""
    pub resolved_by: String,
}

/// Rows under one review; `review` is None for the group of loose comments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub review: Option<Review>,
    pub rows: Vec<Row>,
}

/// Comment counts per state, after filtering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Counters {
    pub waiting_on_you: usize,
    pub with_agent: usize,
    pub draft: usize,
    pub not_yet_taken: usize,
    pub closed: usize,
}

/// Narrows the queue; a `None` field matches everything.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    pub state: Option<State>,
    pub lane: Option<String>,
    pub author: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Queue {
    pub groups: Vec<Group>,
    pub counters: Counters,
}

/// The state of a comment, given the ids the reviewer has acknowledged.
pub fn state_of<S: BuildHasher>(comment: &Comment, acknowledged: &HashSet<String, S>) -> State {
    match comment.status {
        Status::Pending if comment.review_id.is_empty() => State::Draft,
        Status::Pending => State::NotYetTaken,
        Status::Pulled => State::WithAgent,
        Status::Done if acknowledged.contains(&comment.id) => State::Closed,
        Status::Done => State::WaitingOnYou,
        Status::Rejected => State::Closed,
    }
}

/// Builds the queue home view from every comment and submitted review sheet.
///
/// A comment belongs to the first sheet (newest review first) that links it.
/// Comments no sheet links form the trailing group without a review, in input
/// order. Groups the filter empties are dropped; counters see filtered rows only.
pub fn build<S: BuildHasher>(
    comments: &[Comment],
    sheets: &[ReviewSheet],
    acknowledged: &HashSet<String, S>,
    filter: &Filter,
) -> Queue {
    let by_id: HashMap<&str, &Comment> = comments.iter().map(|c| (c.id.as_str(), c)).collect();
    let mut claimed: HashSet<&str> = HashSet::new();
    let mut counters = Counters {
        waiting_on_you: 0,
        with_agent: 0,
        draft: 0,
        not_yet_taken: 0,
        closed: 0,
    };

    let mut ordered: Vec<&ReviewSheet> = sheets.iter().collect();
    ordered.sort_by(|a, b| {
        b.review
            .created_at
            .cmp(&a.review.created_at)
            .then_with(|| review_seq(&b.review.id).cmp(&review_seq(&a.review.id)))
    });

    let mut groups = Vec::new();
    for sheet in ordered {
        // the review's own order first, then any sheet comment it did not list
        let listed_ids: HashSet<&str> = sheet
            .review
            .comment_ids
            .iter()
            .map(String::as_str)
            .collect();
        let listed = sheet.review.comment_ids.iter().map(String::as_str);
        let extra = sheet
            .comments
            .iter()
            .map(|c| c.id.as_str())
            .filter(|id| !listed_ids.contains(id));

        let mut rows = Vec::new();
        for id in listed.chain(extra) {
            let found = by_id
                .get(id)
                .copied()
                .or_else(|| sheet.comments.iter().find(|c| c.id == id));
            let Some(comment) = found else { continue };
            if !claimed.insert(comment.id.as_str()) {
                continue;
            }
            let state = state_of(comment, acknowledged);
            if passes(filter, comment, state) {
                tally(&mut counters, state);
                rows.push(row_of(comment, state));
            }
        }
        if !rows.is_empty() {
            groups.push(Group {
                review: Some(sheet.review.clone()),
                rows,
            });
        }
    }

    let mut loose = Vec::new();
    for comment in comments {
        if claimed.contains(comment.id.as_str()) {
            continue;
        }
        let state = state_of(comment, acknowledged);
        if passes(filter, comment, state) {
            tally(&mut counters, state);
            loose.push(row_of(comment, state));
        }
    }
    if !loose.is_empty() {
        groups.push(Group {
            review: None,
            rows: loose,
        });
    }

    Queue { groups, counters }
}

/// The number in a review id like `rv3`; ids without one sort lowest.
fn review_seq(id: &str) -> i64 {
    parse_seq(id, "rv").unwrap_or(0)
}

fn tally(counters: &mut Counters, state: State) {
    match state {
        State::WaitingOnYou => counters.waiting_on_you += 1,
        State::WithAgent => counters.with_agent += 1,
        State::Draft => counters.draft += 1,
        State::NotYetTaken => counters.not_yet_taken += 1,
        State::Closed => counters.closed += 1,
    }
}

fn passes(filter: &Filter, comment: &Comment, state: State) -> bool {
    filter.state.is_none_or(|wanted| wanted == state)
        && filter
            .lane
            .as_ref()
            .is_none_or(|lane| comment.lane.as_ref() == Some(lane))
        && filter
            .author
            .as_ref()
            .is_none_or(|author| comment.author.as_ref() == Some(author))
}

fn row_of(comment: &Comment, state: State) -> Row {
    Row {
        id: comment.id.clone(),
        title: title_of(&comment.comment),
        location: comment.location(),
        author: comment.author.clone().unwrap_or_default(),
        lane: comment.lane.clone().unwrap_or_default(),
        state,
        resolved_by: comment.resolved_by.clone().unwrap_or_default(),
    }
}

/// The first non-empty line, trimmed, cut to `TITLE_MAX_CHARS` characters
/// including the ellipsis that marks a cut.
fn title_of(text: &str) -> String {
    let Some(line) = text.lines().map(str::trim).find(|l| !l.is_empty()) else {
        return "(empty)".to_string();
    };
    if line.chars().count() <= TITLE_MAX_CHARS {
        return line.to_string();
    }
    let mut title: String = line.chars().take(TITLE_MAX_CHARS - 1).collect();
    title.push('…');
    title
}

#[cfg(test)]
mod tests {
    use super::*;
    use rvw_core::operations::SheetState;
    use rvw_core::types::{ReviewStatus, Status};

    fn comment(id: &str, status: Status) -> Comment {
        Comment {
            id: id.into(),
            status,
            workspace: "/ws".into(),
            lane: None,
            author: None,
            file: "src/a.rs".into(),
            path: "/ws/src/a.rs".into(),
            start_line: 4,
            end_line: 4,
            filetype: "rust".into(),
            code: "let x = 1;".into(),
            comment: "fix this".into(),
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

    fn in_review(mut c: Comment, review_id: &str) -> Comment {
        c.review_id = review_id.into();
        c
    }

    fn sheet(id: &str, created: &str, ids: &[&str], comments: Vec<Comment>) -> ReviewSheet {
        ReviewSheet {
            review: Review {
                id: id.into(),
                status: ReviewStatus::Pending,
                workspace: "/ws".into(),
                lane: None,
                author: None,
                decision: "comment".into(),
                summary: "summary".into(),
                comment_ids: ids.iter().map(ToString::to_string).collect(),
                created_at: created.into(),
                pulled_at: None,
            },
            state: SheetState::Pending,
            comments,
        }
    }

    fn none() -> HashSet<String> {
        HashSet::new()
    }

    fn ids(group: &Group) -> Vec<&str> {
        group.rows.iter().map(|r| r.id.as_str()).collect()
    }

    #[test]
    fn state_of_pending_without_review_is_draft() {
        assert_eq!(
            state_of(&comment("r1", Status::Pending), &none()),
            State::Draft
        );
    }

    #[test]
    fn state_of_pending_in_review_is_not_yet_taken() {
        let c = in_review(comment("r1", Status::Pending), "rv1");
        assert_eq!(state_of(&c, &none()), State::NotYetTaken);
    }

    #[test]
    fn state_of_pulled_is_with_agent() {
        let c = in_review(comment("r1", Status::Pulled), "rv1");
        assert_eq!(state_of(&c, &none()), State::WithAgent);
    }

    #[test]
    fn state_of_done_unacknowledged_is_waiting_on_you() {
        let c = comment("r1", Status::Done);
        assert_eq!(state_of(&c, &none()), State::WaitingOnYou);
    }

    #[test]
    fn state_of_done_acknowledged_is_closed() {
        let c = comment("r1", Status::Done);
        let acked: HashSet<String> = ["r1".to_string()].into();
        assert_eq!(state_of(&c, &acked), State::Closed);
    }

    #[test]
    fn state_of_rejected_is_closed_even_unacknowledged() {
        let c = comment("r1", Status::Rejected);
        assert_eq!(state_of(&c, &none()), State::Closed);
    }

    #[test]
    fn glyph_of_shows_rejection_apart_from_done() {
        let done = comment("r1", Status::Done);
        let rejected = comment("r2", Status::Rejected);
        assert_eq!(glyph_of(&done, State::Closed), "✓");
        assert_eq!(glyph_of(&rejected, State::Closed), "✗");
        assert_eq!(glyph_of(&rejected, State::WaitingOnYou), "✗");
        assert_eq!(glyph_of(&done, State::WaitingOnYou), "✓");
    }

    #[test]
    fn labels_and_glyphs_match_the_spec() {
        assert_eq!(State::Draft.label(), "In your draft");
        assert_eq!(State::NotYetTaken.label(), "Not yet taken");
        assert_eq!(State::WithAgent.label(), "With an agent");
        assert_eq!(State::WaitingOnYou.label(), "Waiting on you");
        assert_eq!(State::Closed.label(), "Closed");
        assert_eq!(State::Draft.glyph(), "○");
        assert_eq!(State::NotYetTaken.glyph(), "○");
        assert_eq!(State::WithAgent.glyph(), "◐");
        assert_eq!(State::WaitingOnYou.glyph(), "✓");
        assert_eq!(State::Closed.glyph(), "✓");
    }

    #[test]
    fn groups_are_newest_review_first_and_loose_comments_last() {
        let comments = vec![
            in_review(comment("r1", Status::Pending), "rv1"),
            in_review(comment("r3", Status::Pending), "rv2"),
            in_review(comment("r7", Status::Pending), "rv2"),
            comment("r9", Status::Pending),
        ];
        let sheets = vec![
            sheet("rv1", "2026-01-01T00:00:00Z", &["r1"], vec![]),
            // comment_ids order is the row order, not the id order
            sheet("rv2", "2026-01-02T00:00:00Z", &["r7", "r3"], vec![]),
        ];
        let q = build(&comments, &sheets, &none(), &Filter::default());

        assert_eq!(q.groups.len(), 3);
        assert_eq!(q.groups[0].review.as_ref().unwrap().id, "rv2");
        assert_eq!(ids(&q.groups[0]), vec!["r7", "r3"]);
        assert_eq!(q.groups[1].review.as_ref().unwrap().id, "rv1");
        assert_eq!(ids(&q.groups[1]), vec!["r1"]);
        assert!(q.groups[2].review.is_none());
        assert_eq!(ids(&q.groups[2]), vec!["r9"]);
    }

    #[test]
    fn same_timestamp_ties_break_by_review_number_descending() {
        let comments = vec![
            in_review(comment("r1", Status::Pending), "rv3"),
            in_review(comment("r2", Status::Pending), "rv10"),
        ];
        let sheets = vec![
            sheet("rv3", "2026-01-01T00:00:00Z", &["r1"], vec![]),
            sheet("rv10", "2026-01-01T00:00:00Z", &["r2"], vec![]),
        ];
        let q = build(&comments, &sheets, &none(), &Filter::default());
        assert_eq!(q.groups[0].review.as_ref().unwrap().id, "rv10");
        assert_eq!(q.groups[1].review.as_ref().unwrap().id, "rv3");
    }

    #[test]
    fn loose_comments_keep_input_order() {
        let comments = vec![
            comment("r9", Status::Pending),
            comment("r2", Status::Pending),
        ];
        let q = build(&comments, &[], &none(), &Filter::default());
        assert_eq!(q.groups.len(), 1);
        assert!(q.groups[0].review.is_none());
        assert_eq!(ids(&q.groups[0]), vec!["r9", "r2"]);
    }

    #[test]
    fn a_comment_in_both_inputs_appears_once_with_the_comments_version() {
        let fresh = in_review(comment("r1", Status::Pulled), "rv1");
        let stale = in_review(comment("r1", Status::Pending), "rv1");
        let sheets = vec![sheet("rv1", "2026-01-01T00:00:00Z", &["r1"], vec![stale])];
        let q = build(&[fresh], &sheets, &none(), &Filter::default());

        assert_eq!(q.groups.len(), 1);
        assert_eq!(ids(&q.groups[0]), vec!["r1"]);
        assert_eq!(q.groups[0].rows[0].state, State::WithAgent);
        assert_eq!(q.counters.with_agent, 1);
        assert_eq!(q.counters.not_yet_taken, 0);
    }

    #[test]
    fn a_sheet_comment_missing_from_the_comments_input_is_still_shown() {
        let linked = in_review(comment("r4", Status::Pending), "rv1");
        let sheets = vec![sheet("rv1", "2026-01-01T00:00:00Z", &["r4"], vec![linked])];
        let q = build(&[], &sheets, &none(), &Filter::default());
        assert_eq!(ids(&q.groups[0]), vec!["r4"]);
        assert_eq!(q.counters.not_yet_taken, 1);
    }

    #[test]
    fn a_sheet_id_with_no_comment_anywhere_shows_no_row() {
        // a sheet listing an id that is not in `comments` and not in its own
        // comments has nothing to show for that id
        let sheets = vec![sheet("rv1", "2026-01-01T00:00:00Z", &["r99"], vec![])];
        let q = build(&[], &sheets, &none(), &Filter::default());
        assert!(q.groups.is_empty());
        assert_eq!(q.counters.draft, 0);
    }

    #[test]
    fn filter_by_state_keeps_only_that_state() {
        let comments = vec![
            comment("r1", Status::Pending),
            in_review(comment("r2", Status::Pulled), "rv1"),
        ];
        let sheets = vec![sheet("rv1", "2026-01-01T00:00:00Z", &["r2"], vec![])];
        let filter = Filter {
            state: Some(State::WithAgent),
            ..Filter::default()
        };
        let q = build(&comments, &sheets, &none(), &filter);
        assert_eq!(q.groups.len(), 1);
        assert_eq!(ids(&q.groups[0]), vec!["r2"]);
    }

    #[test]
    fn filter_by_lane_never_matches_an_unlaned_comment() {
        let mut laned = comment("r1", Status::Pending);
        laned.lane = Some("feat".into());
        let unlaned = comment("r2", Status::Pending);
        let filter = Filter {
            lane: Some("feat".into()),
            ..Filter::default()
        };
        let q = build(&[laned, unlaned], &[], &none(), &filter);
        assert_eq!(q.groups.len(), 1);
        assert_eq!(ids(&q.groups[0]), vec!["r1"]);
        assert_eq!(q.groups[0].rows[0].lane, "feat");
    }

    #[test]
    fn filter_by_author_never_matches_an_anonymous_comment() {
        let mut ada = comment("r1", Status::Pending);
        ada.author = Some("ada".into());
        let anonymous = comment("r2", Status::Pending);
        let filter = Filter {
            author: Some("ada".into()),
            ..Filter::default()
        };
        let q = build(&[ada, anonymous], &[], &none(), &filter);
        assert_eq!(ids(&q.groups[0]), vec!["r1"]);
        assert_eq!(q.groups[0].rows[0].author, "ada");
    }

    #[test]
    fn groups_left_empty_by_the_filter_are_dropped() {
        let comments = vec![
            in_review(comment("r1", Status::Pending), "rv1"),
            in_review(comment("r2", Status::Done), "rv2"),
            comment("r3", Status::Pending),
        ];
        let sheets = vec![
            sheet("rv1", "2026-01-01T00:00:00Z", &["r1"], vec![]),
            sheet("rv2", "2026-01-02T00:00:00Z", &["r2"], vec![]),
        ];
        let filter = Filter {
            state: Some(State::WaitingOnYou),
            ..Filter::default()
        };
        let q = build(&comments, &sheets, &none(), &filter);
        assert_eq!(q.groups.len(), 1);
        assert_eq!(q.groups[0].review.as_ref().unwrap().id, "rv2");
    }

    #[test]
    fn counters_count_the_filtered_rows_only() {
        let mut laned = comment("r1", Status::Pending);
        laned.lane = Some("feat".into());
        let comments = vec![
            laned,
            comment("r2", Status::Pending),
            comment("r3", Status::Rejected),
        ];
        let filter = Filter {
            lane: Some("feat".into()),
            ..Filter::default()
        };
        let q = build(&comments, &[], &none(), &filter);
        assert_eq!(
            q.counters,
            Counters {
                waiting_on_you: 0,
                with_agent: 0,
                draft: 1,
                not_yet_taken: 0,
                closed: 0,
            }
        );
        let all = build(&comments, &[], &none(), &Filter::default());
        assert_eq!(all.counters.draft, 2);
        assert_eq!(all.counters.closed, 1);
    }

    #[test]
    fn row_carries_location_resolved_by_and_empty_strings_for_missing_fields() {
        let mut c = comment("r1", Status::Done);
        c.resolved_by = Some("ada".into());
        c.start_line = 3;
        c.end_line = 5;
        let q = build(&[c], &[], &none(), &Filter::default());
        let row = &q.groups[0].rows[0];
        assert_eq!(row.location, "src/a.rs:3-5");
        assert_eq!(row.resolved_by, "ada");
        assert_eq!(row.author, "");
        assert_eq!(row.lane, "");
    }

    #[test]
    fn title_is_the_first_non_empty_trimmed_line() {
        let mut c = comment("r1", Status::Pending);
        c.comment = "\n   \n  hello  \nworld".into();
        let q = build(&[c], &[], &none(), &Filter::default());
        assert_eq!(q.groups[0].rows[0].title, "hello");
    }

    #[test]
    fn title_of_whitespace_only_text_is_empty_marker() {
        let mut c = comment("r1", Status::Pending);
        c.comment = "  \n\t\n   ".into();
        let q = build(&[c], &[], &none(), &Filter::default());
        assert_eq!(q.groups[0].rows[0].title, "(empty)");
    }

    #[test]
    fn title_of_sixty_chars_is_not_cut() {
        let text = "a".repeat(60);
        let mut c = comment("r1", Status::Pending);
        c.comment = text.clone();
        let q = build(&[c], &[], &none(), &Filter::default());
        assert_eq!(q.groups[0].rows[0].title, text);
    }

    #[test]
    fn title_longer_than_sixty_chars_is_cut_with_an_ellipsis_counting_chars() {
        // multibyte chars: the cut must count chars, not bytes
        let text = "é".repeat(70);
        let mut c = comment("r1", Status::Pending);
        c.comment = text;
        let q = build(&[c], &[], &none(), &Filter::default());
        let title = &q.groups[0].rows[0].title;
        assert_eq!(title.chars().count(), 60);
        assert!(title.ends_with('…'));
        assert_eq!(title.chars().filter(|&ch| ch == 'é').count(), 59);
    }
}
