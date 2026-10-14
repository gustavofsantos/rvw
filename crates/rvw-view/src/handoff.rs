//! The handoff preview (A8): what the next pull would take, grouped by lane.

use std::collections::BTreeMap;

use rvw_core::types::{Comment, Handoff};

use crate::queue::is_draft;

/// The title length limit, in characters, ellipsis included, as in the queue.
const TITLE_MAX_CHARS: usize = 60;

/// The name shown for the group of handoffs with no lane.
const UNLANED: &str = "(no lane)";

/// One submitted review a pull would hand over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewItem {
    /// review id, e.g. "rv2"
    pub id: String,
    /// the review's decision, e.g. "comment", "approve" or "request-changes"
    pub decision: String,
    /// first non-empty line of the summary, cut like queue titles
    pub title: String,
    /// how many linked comments the review carries
    pub comments: usize,
}

/// One standalone comment a pull would hand over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentItem {
    /// comment id, e.g. "r9"
    pub id: String,
    /// `file:N` or `file:N-M`
    pub location: String,
    /// first non-empty line of the comment text, cut like queue titles
    pub title: String,
    /// a pending comment outside any review: the reviewer's own draft
    pub draft: bool,
    /// the comment this one reopens, when it is a follow-up
    pub reopens: Option<String>,
}

/// The handoffs of one lane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanePreview {
    /// the lane, or "" for handoffs with no lane
    pub lane: String,
    pub reviews: Vec<ReviewItem>,
    pub comments: Vec<CommentItem>,
}

impl LanePreview {
    fn new(lane: &str) -> Self {
        Self {
            lane: lane.to_owned(),
            reviews: Vec::new(),
            comments: Vec::new(),
        }
    }

    /// The lane's name for people; "(no lane)" for the unlaned group.
    pub fn label(&self) -> String {
        if self.lane.is_empty() {
            UNLANED.to_owned()
        } else {
            self.lane.clone()
        }
    }

    /// Whether the lane holds no handoff at all.
    pub fn is_empty(&self) -> bool {
        self.reviews.is_empty() && self.comments.is_empty()
    }

    /// The reviews and standalone comments in the lane.
    pub fn handoffs(&self) -> usize {
        self.reviews.len() + self.comments.len()
    }
}

/// What a pull would take, grouped by lane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preview {
    /// the lane the pull is pinned to, or "" when unpinned
    pub pinned: String,
    /// the pinned lane first (even when empty), then the other lanes sorted by
    /// name, with the unlaned group last; each lane keeps its input order
    pub lanes: Vec<LanePreview>,
}

impl Preview {
    /// The pinned lane's group; None when unpinned.
    pub fn pinned_lane(&self) -> Option<&LanePreview> {
        if self.pinned.is_empty() {
            return None;
        }
        self.lanes.iter().find(|lane| lane.lane == self.pinned)
    }

    /// The handoffs outside the pinned lane. Always 0 when unpinned, because a
    /// pull with no lane takes everything.
    pub fn elsewhere(&self) -> usize {
        self.others().map(LanePreview::handoffs).sum()
    }

    /// The labels of the other lanes that hold handoffs, in preview order.
    pub fn elsewhere_lanes(&self) -> Vec<String> {
        self.others()
            .filter(|lane| !lane.is_empty())
            .map(LanePreview::label)
            .collect()
    }

    /// The handoffs a pull pinned like this takes: the pinned lane's, or all
    /// of them when unpinned.
    pub fn will_take(&self) -> usize {
        self.taken().map(LanePreview::handoffs).sum()
    }

    /// The reviewer's own draft comments a pull would take.
    pub fn drafts(&self) -> usize {
        self.taken()
            .flat_map(|lane| lane.comments.iter())
            .filter(|comment| comment.draft)
            .count()
    }

    /// One sentence on what a pull takes and what waits elsewhere.
    pub fn notice(&self) -> String {
        let total: usize = self.lanes.iter().map(LanePreview::handoffs).sum();
        if total == 0 {
            return "Nothing to hand over.".to_owned();
        }

        let take = self.will_take();
        let head = if self.pinned.is_empty() {
            format!("A pull takes {}.", handoff_count(take))
        } else if take == 0 {
            format!("A pull takes nothing in lane {}.", self.pinned)
        } else {
            format!(
                "A pull takes {} in lane {}.",
                handoff_count(take),
                self.pinned
            )
        };
        let mut parts = vec![head];

        let other = self.elsewhere();
        if other > 0 {
            let lanes = self.elsewhere_lanes();
            let scope = if lanes.len() == 1 {
                "another lane"
            } else {
                "other lanes"
            };
            let listed = lanes.join(", ");
            parts.push(if other == 1 {
                format!("1 more handoff waits in {scope}: {listed}.")
            } else {
                format!("{other} more wait in {scope}: {listed}.")
            });
        }

        match self.drafts() {
            0 => {}
            1 => parts.push("1 is your draft — a pull takes it too.".to_owned()),
            n => parts.push(format!("{n} are your drafts — a pull takes them too.")),
        }
        parts.join(" ")
    }

    /// The lanes a pull pinned like this takes: the pinned lane, or all.
    fn taken(&self) -> impl Iterator<Item = &LanePreview> {
        let pinned = self.pinned.as_str();
        self.lanes
            .iter()
            .filter(move |lane| pinned.is_empty() || lane.lane == pinned)
    }

    /// The lanes outside the pin. Empty when unpinned.
    fn others(&self) -> impl Iterator<Item = &LanePreview> {
        let pinned = self.pinned.as_str();
        self.lanes
            .iter()
            .filter(move |lane| !pinned.is_empty() && lane.lane != pinned)
    }
}

/// Groups what a peek with lane "" returns by lane, for a pull pinned to
/// `pinned` ("" for no pin). A handoff takes its own lane, so an unlaned item
/// and another lane's item are never taken by a pin.
pub fn preview(reviews: &[Handoff], comments: &[Comment], pinned: &str) -> Preview {
    let mut by_lane: BTreeMap<String, LanePreview> = BTreeMap::new();
    for handoff in reviews {
        let item = review_item(handoff);
        lane_entry(&mut by_lane, handoff.review.lane.as_deref())
            .reviews
            .push(item);
    }
    for comment in comments {
        let item = comment_item(comment);
        lane_entry(&mut by_lane, comment.lane.as_deref())
            .comments
            .push(item);
    }

    // the pinned lane first, even when empty; then the named lanes in name
    // order; the unlaned group last (it sorts first in the map, so take it out)
    let mut lanes = Vec::new();
    if !pinned.is_empty() {
        lanes.push(
            by_lane
                .remove(pinned)
                .unwrap_or_else(|| LanePreview::new(pinned)),
        );
    }
    let unlaned = by_lane.remove("");
    lanes.extend(by_lane.into_values());
    lanes.extend(unlaned);

    Preview {
        pinned: pinned.to_owned(),
        lanes,
    }
}

/// The lane a handoff belongs to: a missing lane and an empty one are both
/// the unlaned group.
fn lane_entry<'a>(
    by_lane: &'a mut BTreeMap<String, LanePreview>,
    lane: Option<&str>,
) -> &'a mut LanePreview {
    let name = lane.unwrap_or("");
    by_lane
        .entry(name.to_owned())
        .or_insert_with(|| LanePreview::new(name))
}

fn review_item(handoff: &Handoff) -> ReviewItem {
    ReviewItem {
        id: handoff.review.id.clone(),
        decision: handoff.review.decision.clone(),
        title: title_of(&handoff.review.summary),
        comments: handoff.comments.len(),
    }
}

fn comment_item(comment: &Comment) -> CommentItem {
    CommentItem {
        id: comment.id.clone(),
        location: comment.location(),
        title: title_of(&comment.comment),
        draft: is_draft(comment),
        reopens: comment.reopens.clone(),
    }
}

fn handoff_count(n: usize) -> String {
    if n == 1 {
        "1 handoff".to_owned()
    } else {
        format!("{n} handoffs")
    }
}

/// The first non-empty line, trimmed, cut to `TITLE_MAX_CHARS` characters
/// including the ellipsis that marks a cut. Mirrors `queue`'s titles.
fn title_of(text: &str) -> String {
    let Some(line) = text.lines().map(str::trim).find(|l| !l.is_empty()) else {
        return "(empty)".to_owned();
    };
    if line.chars().count() <= TITLE_MAX_CHARS {
        return line.to_owned();
    }
    let mut title: String = line.chars().take(TITLE_MAX_CHARS - 1).collect();
    title.push('…');
    title
}

#[cfg(test)]
mod tests {
    use super::*;
    use rvw_core::types::{Review, ReviewStatus, Status};

    fn comment(id: &str, lane: Option<&str>) -> Comment {
        Comment {
            points_at: None,
            reopens: None,
            verdict: None,
            id: id.into(),
            status: Status::Pending,
            workspace: "/ws".into(),
            lane: lane.map(Into::into),
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

    /// A pending comment submitted in a review, so it is not a draft.
    fn in_review(mut c: Comment, review_id: &str) -> Comment {
        c.review_id = review_id.into();
        c
    }

    fn handoff(id: &str, lane: Option<&str>, summary: &str, linked: Vec<Comment>) -> Handoff {
        Handoff {
            review: Review {
                id: id.into(),
                status: ReviewStatus::Pending,
                workspace: "/ws".into(),
                lane: lane.map(Into::into),
                author: None,
                decision: "comment".into(),
                summary: summary.into(),
                comment_ids: linked.iter().map(|c| c.id.clone()).collect(),
                created_at: "2026-01-01T00:00:00Z".into(),
                pulled_at: None,
            },
            comments: linked,
        }
    }

    fn lane_names(p: &Preview) -> Vec<&str> {
        p.lanes.iter().map(|l| l.lane.as_str()).collect()
    }

    fn comment_ids(lane: &LanePreview) -> Vec<&str> {
        lane.comments.iter().map(|c| c.id.as_str()).collect()
    }

    #[test]
    fn a_review_groups_under_its_own_lane() {
        let reviews = vec![handoff("rv1", Some("main"), "ship it", vec![])];
        let p = preview(&reviews, &[], "");
        assert_eq!(lane_names(&p), vec!["main"]);
        assert_eq!(p.lanes[0].reviews.len(), 1);
        assert_eq!(p.lanes[0].reviews[0].id, "rv1");
    }

    #[test]
    fn a_standalone_comment_groups_under_its_own_lane_not_a_review_lane() {
        // the review is in "main", its linked comment in "feat": the comment
        // is counted inside the review, so no "feat" group appears
        let linked = comment("r1", Some("feat"));
        let reviews = vec![handoff("rv1", Some("main"), "s", vec![linked])];
        let standalone = vec![comment("r2", Some("dev"))];
        let p = preview(&reviews, &standalone, "");
        assert_eq!(lane_names(&p), vec!["dev", "main"]);
        let dev = &p.lanes[0];
        assert!(dev.reviews.is_empty());
        assert_eq!(comment_ids(dev), vec!["r2"]);
        assert_eq!(p.lanes[1].reviews[0].comments, 1);
        assert!(p.lanes[1].comments.is_empty());
    }

    #[test]
    fn pinned_lane_comes_first_and_is_present_when_empty() {
        let standalone = vec![comment("r1", Some("dev"))];
        let p = preview(&[], &standalone, "main");
        assert_eq!(lane_names(&p), vec!["main", "dev"]);
        assert!(p.lanes[0].is_empty());
        assert_eq!(p.pinned_lane().unwrap().lane, "main");
        assert_eq!(p.pinned_lane().unwrap().handoffs(), 0);
    }

    #[test]
    fn other_lanes_sort_by_name_with_the_unlaned_group_last() {
        let standalone = vec![
            comment("r1", Some("zeta")),
            comment("r2", None),
            comment("r3", Some("alpha")),
            comment("r4", Some("")),
            comment("r5", Some("beta")),
        ];
        let p = preview(&[], &standalone, "");
        assert_eq!(lane_names(&p), vec!["alpha", "beta", "zeta", ""]);
        assert_eq!(p.lanes[3].label(), "(no lane)");
        assert_eq!(comment_ids(&p.lanes[3]), vec!["r2", "r4"]);
    }

    #[test]
    fn the_pinned_lane_is_first_and_the_rest_stay_sorted() {
        let standalone = vec![
            comment("r1", Some("zeta")),
            comment("r2", None),
            comment("r3", Some("main")),
            comment("r4", Some("alpha")),
        ];
        let p = preview(&[], &standalone, "main");
        assert_eq!(lane_names(&p), vec!["main", "alpha", "zeta", ""]);
    }

    #[test]
    fn inside_a_lane_handoffs_keep_input_order() {
        let reviews = vec![
            handoff("rv1", Some("main"), "first", vec![]),
            handoff("rv2", Some("main"), "second", vec![]),
        ];
        let standalone = vec![comment("r9", Some("main")), comment("r2", Some("main"))];
        let p = preview(&reviews, &standalone, "main");
        let ids: Vec<&str> = p.lanes[0].reviews.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["rv1", "rv2"]);
        assert_eq!(comment_ids(&p.lanes[0]), vec!["r9", "r2"]);
    }

    #[test]
    fn elsewhere_counts_other_lanes_even_when_the_pinned_lane_is_not_empty() {
        let reviews = vec![handoff("rv1", Some("main"), "s", vec![])];
        let standalone = vec![
            comment("r1", Some("main")),
            comment("r2", Some("dev")),
            comment("r3", None),
        ];
        let p = preview(&reviews, &standalone, "main");
        assert_eq!(p.will_take(), 2);
        assert_eq!(p.elsewhere(), 2);
        assert_eq!(p.elsewhere_lanes(), vec!["dev", "(no lane)"]);
    }

    #[test]
    fn elsewhere_is_zero_when_nothing_waits_outside_the_pin() {
        let standalone = vec![comment("r1", Some("main"))];
        let p = preview(&[], &standalone, "main");
        assert_eq!(p.elsewhere(), 0);
        assert!(p.elsewhere_lanes().is_empty());
    }

    #[test]
    fn unpinned_takes_everything_and_has_no_elsewhere() {
        let reviews = vec![handoff("rv1", Some("main"), "s", vec![])];
        let standalone = vec![comment("r1", Some("dev")), comment("r2", None)];
        let p = preview(&reviews, &standalone, "");
        assert_eq!(p.will_take(), 3);
        assert_eq!(p.elsewhere(), 0);
        assert!(p.elsewhere_lanes().is_empty());
        assert!(p.pinned_lane().is_none());
    }

    #[test]
    fn a_pinned_lane_takes_only_its_own_handoffs() {
        let standalone = vec![comment("r1", Some("dev")), comment("r2", None)];
        let p = preview(&[], &standalone, "main");
        assert_eq!(p.will_take(), 0);
        assert_eq!(p.elsewhere(), 2);
    }

    #[test]
    fn drafts_counts_pending_comments_outside_reviews_in_the_taken_lanes() {
        let standalone = vec![
            comment("r1", Some("main")),
            comment("r2", Some("dev")),
            in_review(comment("r3", Some("main")), "rv1"),
        ];
        let pinned = preview(&[], &standalone, "main");
        assert_eq!(pinned.drafts(), 1);
        let unpinned = preview(&[], &standalone, "");
        assert_eq!(unpinned.drafts(), 2);
    }

    #[test]
    fn a_comment_item_marks_its_draft_and_carries_its_reopens() {
        let mut follow_up = comment("r7", Some("main"));
        follow_up.reopens = Some("r3".into());
        follow_up.start_line = 3;
        follow_up.end_line = 5;
        let p = preview(&[], &[follow_up], "main");
        let item = &p.lanes[0].comments[0];
        assert_eq!(item.id, "r7");
        assert_eq!(item.location, "src/a.rs:3-5");
        assert!(item.draft);
        assert_eq!(item.reopens.as_deref(), Some("r3"));
    }

    #[test]
    fn a_review_item_carries_decision_linked_count_and_its_title() {
        let linked = vec![comment("r1", None), comment("r2", None)];
        let reviews = vec![handoff("rv4", None, "  \n  Looks good  \nmore", linked)];
        let p = preview(&reviews, &[], "");
        let item = &p.lanes[0].reviews[0];
        assert_eq!(item.id, "rv4");
        assert_eq!(item.decision, "comment");
        assert_eq!(item.title, "Looks good");
        assert_eq!(item.comments, 2);
    }

    #[test]
    fn titles_are_cut_like_queue_titles() {
        let long = "é".repeat(70);
        let reviews = vec![handoff("rv1", None, &long, vec![])];
        let p = preview(&reviews, &[], "");
        let title = &p.lanes[0].reviews[0].title;
        assert_eq!(title.chars().count(), 60);
        assert!(title.ends_with('…'));

        let blank = vec![handoff("rv2", None, "  \n\t", vec![])];
        let p = preview(&blank, &[], "");
        assert_eq!(p.lanes[0].reviews[0].title, "(empty)");
    }

    #[test]
    fn a_lane_of_empty_text_is_the_unlaned_group() {
        let standalone = vec![comment("r1", Some(""))];
        let p = preview(&[], &standalone, "");
        assert_eq!(p.lanes[0].lane, "");
        assert_eq!(p.lanes[0].label(), "(no lane)");
    }

    #[test]
    fn label_and_counts_of_a_lane() {
        let standalone = vec![comment("r1", Some("dev")), comment("r2", Some("dev"))];
        let reviews = vec![handoff("rv1", Some("dev"), "s", vec![])];
        let p = preview(&reviews, &standalone, "");
        assert_eq!(p.lanes[0].label(), "dev");
        assert_eq!(p.lanes[0].handoffs(), 3);
        assert!(!p.lanes[0].is_empty());
    }

    #[test]
    fn notice_for_nothing_to_hand_over() {
        assert_eq!(preview(&[], &[], "").notice(), "Nothing to hand over.");
        assert_eq!(preview(&[], &[], "main").notice(), "Nothing to hand over.");
    }

    #[test]
    fn notice_for_a_single_lane() {
        let standalone = vec![
            in_review(comment("r1", Some("main")), "rv2"),
            in_review(comment("r2", Some("main")), "rv2"),
        ];
        let reviews = vec![handoff("rv1", Some("main"), "s", vec![])];
        let p = preview(&reviews, &standalone, "main");
        assert_eq!(p.notice(), "A pull takes 3 handoffs in lane main.");
    }

    #[test]
    fn notice_for_a_single_handoff_and_for_an_unpinned_pull() {
        let submitted = vec![in_review(comment("r1", Some("main")), "rv1")];
        let p = preview(&[], &submitted, "main");
        assert_eq!(p.notice(), "A pull takes 1 handoff in lane main.");
        let unpinned = preview(&[], &submitted, "");
        assert_eq!(unpinned.notice(), "A pull takes 1 handoff.");
    }

    #[test]
    fn notice_with_elsewhere() {
        let standalone = vec![
            in_review(comment("r1", Some("main")), "rv1"),
            in_review(comment("r2", Some("main")), "rv1"),
            in_review(comment("r3", Some("main")), "rv1"),
            in_review(comment("r4", Some("dev")), "rv1"),
            in_review(comment("r5", Some("dev")), "rv1"),
            in_review(comment("r6", None), "rv1"),
        ];
        let p = preview(&[], &standalone, "main");
        assert_eq!(
            p.notice(),
            "A pull takes 3 handoffs in lane main. 3 more wait in other lanes: dev, (no lane)."
        );
    }

    #[test]
    fn notice_with_one_elsewhere_and_an_empty_pin() {
        let standalone = vec![comment("r1", Some("dev"))];
        let p = preview(&[], &standalone, "main");
        assert_eq!(
            p.notice(),
            "A pull takes nothing in lane main. 1 more handoff waits in another lane: dev."
        );
    }

    #[test]
    fn notice_mentions_a_draft_the_pull_takes() {
        let standalone = vec![comment("r1", Some("main"))];
        let p = preview(&[], &standalone, "main");
        assert_eq!(
            p.notice(),
            "A pull takes 1 handoff in lane main. 1 is your draft — a pull takes it too."
        );
    }

    #[test]
    fn notice_mentions_several_drafts_in_the_plural() {
        let standalone = vec![comment("r1", Some("main")), comment("r2", Some("main"))];
        let p = preview(&[], &standalone, "main");
        assert_eq!(
            p.notice(),
            "A pull takes 2 handoffs in lane main. 2 are your drafts — a pull takes them too."
        );
    }

    #[test]
    fn notice_does_not_count_a_draft_that_waits_elsewhere() {
        let standalone = vec![
            in_review(comment("r1", Some("main")), "rv1"),
            comment("r2", Some("dev")),
        ];
        let p = preview(&[], &standalone, "main");
        assert_eq!(p.drafts(), 0);
        assert_eq!(
            p.notice(),
            "A pull takes 1 handoff in lane main. 1 more handoff waits in another lane: dev."
        );
    }
}
