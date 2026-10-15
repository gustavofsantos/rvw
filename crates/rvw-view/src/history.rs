//! The History screen's view model: the commit rows with their graph lanes
//! and ref labels, the selection, paging, and short relative times.

use std::collections::HashSet;

use rvw_git::history::LogEntry;

use crate::graph::{Graph, Row};

/// Commits read per page.
pub const PAGE: usize = 200;
/// Rows left below the last one in view when the next page is asked for.
pub const PREFETCH: usize = 50;
/// Lanes the graph column draws; a commit in a lane beyond this is drawn at the last lane.
pub const MAX_LANES: usize = 12;

/// What kind of ref a label names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelKind {
    Branch,
    Remote,
    Tag,
    DetachedHead,
}

/// A ref shown on a commit row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    pub kind: LabelKind,
    /// The short name: "main", "feat/x", "origin/main", "v1", or "HEAD" for a detached HEAD.
    pub name: String,
    /// HEAD points here: the checked-out branch, or the detached HEAD itself.
    pub head: bool,
}

/// The labels of a commit, in this order: the checked-out branch (or detached HEAD) first,
/// then other local branches, then remote branches, then tags; each group keeps git's order.
/// `refs/remotes/<remote>/HEAD` is left out, and so is any ref outside refs/heads, refs/remotes and refs/tags (e.g. refs/stash).
pub fn labels(refs: &[String]) -> Vec<Label> {
    let mut head = None;
    let mut branches = Vec::new();
    let mut remotes = Vec::new();
    let mut tags = Vec::new();
    for name in refs.iter().map(String::as_str) {
        if name == "HEAD" {
            head = Some(Label {
                kind: LabelKind::DetachedHead,
                name: "HEAD".to_owned(),
                head: true,
            });
        } else if let Some(target) = name.strip_prefix("HEAD -> ") {
            if let Some(branch) = target.strip_prefix("refs/heads/") {
                head = Some(label(LabelKind::Branch, branch, true));
            }
        } else if let Some(branch) = name.strip_prefix("refs/heads/") {
            branches.push(label(LabelKind::Branch, branch, false));
        } else if let Some(remote) = name.strip_prefix("refs/remotes/") {
            if !is_remote_head(remote) {
                remotes.push(label(LabelKind::Remote, remote, false));
            }
        } else if let Some(tag) = name
            .strip_prefix("tag: ")
            .unwrap_or(name)
            .strip_prefix("refs/tags/")
        {
            tags.push(label(LabelKind::Tag, tag, false));
        }
    }
    head.into_iter()
        .chain(branches)
        .chain(remotes)
        .chain(tags)
        .collect()
}

fn label(kind: LabelKind, name: &str, head: bool) -> Label {
    Label {
        kind,
        name: name.to_owned(),
        head,
    }
}

/// Whether `remote` (the part of a ref after `refs/remotes/`) is `<remote>/HEAD`.
fn is_remote_head(remote: &str) -> bool {
    let mut parts = remote.split('/');
    parts.next();
    matches!((parts.next(), parts.next()), (Some("HEAD"), None))
}

const MINUTE: i64 = 60;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;
const WEEK: i64 = 7 * DAY;
const MONTH: i64 = 30 * DAY;
const YEAR: i64 = 365 * DAY;

/// How long ago `at` was, in the steps the list shows: "now" (under a minute, or in the
/// future), "41m", "23h", "13d" (under 14 days), "7w" (under 60 days, whole weeks),
/// "11mo" (under 365 days, 30-day months), then "2y" (365-day years).
pub fn ago(now: i64, at: i64) -> String {
    let secs = now.saturating_sub(at);
    if secs < MINUTE {
        "now".to_owned()
    } else if secs < HOUR {
        format!("{}m", secs / MINUTE)
    } else if secs < DAY {
        format!("{}h", secs / HOUR)
    } else if secs < 14 * DAY {
        format!("{}d", secs / DAY)
    } else if secs < 60 * DAY {
        format!("{}w", secs / WEEK)
    } else if secs < YEAR {
        format!("{}mo", secs / MONTH)
    } else {
        format!("{}y", secs / YEAR)
    }
}

/// The first 7 characters of an id (all of it when shorter).
pub fn short(id: &str) -> &str {
    match id.char_indices().nth(7) {
        Some((end, _)) => &id[..end],
        None => id,
    }
}

/// One commit row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryRow {
    pub entry: LogEntry,
    pub graph: Row,
    pub labels: Vec<Label>,
}

/// The commits read so far, laid out, with the selection.
#[derive(Debug, Clone, Default)]
pub struct History {
    rows: Vec<HistoryRow>,
    graph: Graph,
    selected: Option<usize>,
    complete: bool,
    /// Ids of the rows kept, so a commit read twice is skipped.
    ids: HashSet<String>,
    /// Entries handed to `append`, skipped ones included.
    read: usize,
    /// The widest graph row so far, in lanes.
    lanes: usize,
}

impl History {
    /// An empty history: no rows, nothing selected, not complete.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds the next page. A commit already present is skipped (no row, no graph push).
    /// `complete` becomes true when the page has fewer than `asked` entries.
    /// Selects row 0 when nothing was selected and there is now a row.
    pub fn append(&mut self, page: Vec<LogEntry>, asked: usize) {
        let got = page.len();
        self.read += got;
        if got < asked {
            self.complete = true;
        }
        for entry in page {
            if !self.ids.insert(entry.id.clone()) {
                continue;
            }
            let graph = self.graph.push(&entry.id, &entry.parents);
            self.lanes = self.lanes.max(graph.width);
            let row_labels = labels(&entry.refs);
            self.rows.push(HistoryRow {
                entry,
                graph,
                labels: row_labels,
            });
        }
        if self.selected.is_none() && !self.rows.is_empty() {
            self.selected = Some(0);
        }
    }

    /// The lanes the graph column draws so every row lines up: the widest
    /// row read so far, at least 1 and at most [`MAX_LANES`].
    pub fn lanes(&self) -> usize {
        self.lanes.clamp(1, MAX_LANES)
    }

    /// The rows read so far, newest first.
    pub fn rows(&self) -> &[HistoryRow] {
        &self.rows
    }

    /// The number of rows.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Whether no row has been read yet.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Every page has been read.
    pub fn complete(&self) -> bool {
        self.complete
    }

    /// The `--skip` of the next page: the number of commits read so far, skipped ones included
    /// (count every entry handed to `append`, not only the rows kept).
    pub fn next_skip(&self) -> usize {
        self.read
    }

    /// Whether the next page should be read now that row `last_visible` is in view:
    /// not complete, and fewer than `PREFETCH` rows lie below `last_visible`.
    pub fn wants_more(&self, last_visible: usize) -> bool {
        !self.complete && last_visible.saturating_add(PREFETCH + 1) >= self.rows.len()
    }

    /// The selected row's index, or None when there are no rows.
    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    /// The selected row, or None when nothing is selected.
    pub fn selected_row(&self) -> Option<&HistoryRow> {
        self.selected.and_then(|index| self.rows.get(index))
    }

    /// Selects row `index`, clamped to the last row; nothing when empty.
    pub fn select(&mut self, index: usize) {
        if let Some(last) = self.rows.len().checked_sub(1) {
            self.selected = Some(index.min(last));
        }
    }

    /// Moves the selection by `delta` rows, clamped to the first and last; from nothing it selects row 0.
    pub fn step(&mut self, delta: isize) {
        let Some(last) = self.rows.len().checked_sub(1) else {
            return;
        };
        self.selected = Some(match self.selected {
            None => 0,
            Some(current) => current.saturating_add_signed(delta).min(last),
        });
    }

    /// Selects the row of commit `id` (a full id or a prefix of at least 7 characters); false when it is not read yet.
    pub fn select_id(&mut self, id: &str) -> bool {
        if id.chars().count() < 7 {
            return false;
        }
        match self
            .rows
            .iter()
            .position(|row| row.entry.id.starts_with(id))
        {
            Some(index) => {
                self.selected = Some(index);
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, parents: &[&str]) -> LogEntry {
        LogEntry {
            id: id.to_owned(),
            parents: parents.iter().map(|p| (*p).to_owned()).collect(),
            author: "Ada".to_owned(),
            email: "ada@example.com".to_owned(),
            time: 1_000,
            refs: Vec::new(),
            subject: "subject".to_owned(),
            session_markers: Vec::new(),
        }
    }

    fn refs(list: &[&str]) -> Vec<String> {
        list.iter().map(|r| (*r).to_owned()).collect()
    }

    /// A 40-character id for commit `n`, so ids in one history differ from the first character.
    fn full_id(n: usize) -> String {
        format!("{n:040x}")
    }

    /// Commits 0..count, each with the next one as its parent.
    fn chain(count: usize) -> Vec<LogEntry> {
        (0..count)
            .map(|n| entry(&full_id(n), &[&full_id(n + 1)]))
            .collect()
    }

    #[test]
    fn labels_put_the_checked_out_branch_first_then_branches_remotes_and_tags() {
        let got = labels(&refs(&[
            "HEAD -> refs/heads/main",
            "refs/remotes/origin/main",
            "refs/remotes/origin/HEAD",
            "tag: refs/tags/v1",
            "refs/heads/dev",
        ]));

        assert_eq!(
            got,
            vec![
                label(LabelKind::Branch, "main", true),
                label(LabelKind::Branch, "dev", false),
                label(LabelKind::Remote, "origin/main", false),
                label(LabelKind::Tag, "v1", false),
            ]
        );
    }

    #[test]
    fn labels_show_a_detached_head_first() {
        assert_eq!(
            labels(&refs(&["HEAD", "refs/heads/x"])),
            vec![
                label(LabelKind::DetachedHead, "HEAD", true),
                label(LabelKind::Branch, "x", false),
            ]
        );
    }

    #[test]
    fn labels_keep_the_slashes_in_a_branch_name() {
        assert_eq!(
            labels(&refs(&["HEAD -> refs/heads/feat/x"])),
            vec![label(LabelKind::Branch, "feat/x", true)]
        );
    }

    #[test]
    fn labels_name_a_remote_branch_without_its_refs_remotes_prefix() {
        assert_eq!(
            labels(&refs(&["refs/remotes/up/feat/x"])),
            vec![label(LabelKind::Remote, "up/feat/x", false)]
        );
    }

    #[test]
    fn labels_leave_out_stash_and_other_refs() {
        assert_eq!(labels(&refs(&["refs/stash"])), Vec::new());
    }

    #[test]
    fn ago_says_now_under_a_minute_and_for_a_future_time() {
        assert_eq!(ago(1000, 1000), "now");
        assert_eq!(ago(1000, 2000), "now");
        assert_eq!(ago(100, 41), "now");
    }

    #[test]
    fn ago_counts_in_the_steps_the_list_shows() {
        let day = 86_400;
        let cases = [
            (60, "1m"),
            (41 * 60, "41m"),
            (23 * 3600, "23h"),
            (13 * day, "13d"),
            (14 * day, "2w"),
            (59 * day, "8w"),
            (60 * day, "2mo"),
            (364 * day, "12mo"),
            (365 * day, "1y"),
            (800 * day, "2y"),
        ];
        for (secs, want) in cases {
            assert_eq!(ago(10_000_000, 10_000_000 - secs), want, "{secs} seconds");
        }
    }

    #[test]
    fn short_is_the_first_seven_characters() {
        assert_eq!(short("0123456789abcdef"), "0123456");
        assert_eq!(short("abc"), "abc");
    }

    #[test]
    fn append_lays_out_rows_like_the_graph_does() {
        let mut h = History::new();
        h.append(
            vec![entry("c3", &["c2"]), entry("c2", &["c1"]), entry("c1", &[])],
            3,
        );

        let mut g = Graph::new();
        let want = vec![
            g.push("c3", &["c2".to_string()]),
            g.push("c2", &["c1".to_string()]),
            g.push("c1", &[]),
        ];
        let got: Vec<Row> = h.rows().iter().map(|row| row.graph.clone()).collect();
        assert_eq!(got, want);
        assert!(h.rows().iter().all(|row| row.graph.lane == 0));
    }

    #[test]
    fn paged_append_gives_the_same_graph_as_one_page() {
        let mut whole = History::new();
        whole.append(
            vec![entry("c3", &["c2"]), entry("c2", &["c1"]), entry("c1", &[])],
            3,
        );

        let mut paged = History::new();
        paged.append(vec![entry("c3", &["c2"])], 1);
        paged.append(vec![entry("c2", &["c1"]), entry("c1", &[])], 2);

        assert_eq!(paged.rows(), whole.rows());
    }

    #[test]
    fn append_is_complete_once_a_page_is_short() {
        let mut h = History::new();
        h.append(
            vec![entry("1", &["2"]), entry("2", &["3"]), entry("3", &[])],
            3,
        );
        assert!(!h.complete());

        h.append(vec![entry("4", &[])], 3);
        assert!(h.complete());
        assert_eq!(h.next_skip(), 4);
    }

    #[test]
    fn append_skips_a_commit_already_read_and_counts_it_for_the_skip() {
        let mut h = History::new();
        h.append(vec![entry("a", &["b"]), entry("b", &["c"])], 2);
        h.append(vec![entry("b", &["c"]), entry("c", &[])], 2);

        let ids: Vec<&str> = h.rows().iter().map(|row| row.entry.id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "c"]);
        assert_eq!(h.next_skip(), 4);
    }

    #[test]
    fn first_append_selects_the_first_row() {
        let mut h = History::new();
        assert_eq!(h.selected(), None);
        h.append(vec![entry("a", &[])], 1);
        assert_eq!(h.selected(), Some(0));
        assert_eq!(h.selected_row().map(|row| row.entry.id.as_str()), Some("a"));
    }

    #[test]
    fn step_moves_the_selection_and_clamps_at_both_ends() {
        let mut h = History::new();
        h.append(chain(3), 3);

        h.step(5);
        assert_eq!(h.selected(), Some(2));
        h.step(-10);
        assert_eq!(h.selected(), Some(0));
    }

    #[test]
    fn step_from_nothing_selects_the_first_row() {
        let mut h = History::new();
        h.step(3);
        assert_eq!(h.selected(), None);

        h.append(chain(3), 3);
        h.selected = None;
        h.step(3);
        assert_eq!(h.selected(), Some(0));
    }

    #[test]
    fn select_clamps_to_the_last_row_and_does_nothing_when_empty() {
        let mut empty = History::new();
        empty.select(4);
        assert_eq!(empty.selected(), None);

        let mut h = History::new();
        h.append(chain(3), 3);
        h.select(99);
        assert_eq!(h.selected(), Some(2));
    }

    #[test]
    fn select_id_takes_a_full_id_or_a_prefix_of_seven_characters() {
        let (a, b, c) = ("a".repeat(40), "b".repeat(40), "c".repeat(40));
        let mut h = History::new();
        h.append(vec![entry(&a, &[&b]), entry(&b, &[&c]), entry(&c, &[])], 3);

        assert!(h.select_id("bbbbbbb"));
        assert_eq!(h.selected(), Some(1));
        assert!(h.select_id(&c));
        assert_eq!(h.selected(), Some(2));
    }

    #[test]
    fn select_id_refuses_a_prefix_shorter_than_seven_characters() {
        let (a, b) = ("a".repeat(40), "b".repeat(40));
        let mut h = History::new();
        h.append(vec![entry(&a, &[&b]), entry(&b, &[])], 2);

        assert!(!h.select_id("bbbbbb"));
        assert_eq!(h.selected(), Some(0));
    }

    #[test]
    fn select_id_is_false_for_a_commit_not_read_yet() {
        let mut h = History::new();
        h.append(chain(2), 2);
        assert!(!h.select_id(&full_id(9)));
    }

    #[test]
    fn wants_more_fetches_when_fewer_than_prefetch_rows_are_below_the_view() {
        let mut h = History::new();
        h.append(chain(200), 200);

        assert!(!h.complete());
        assert!(h.wants_more(149));
        assert!(!h.wants_more(100));
    }

    #[test]
    fn wants_more_is_false_once_every_page_is_read() {
        let mut h = History::new();
        h.append(chain(200), 200);
        h.append(Vec::new(), 200);

        assert!(h.complete());
        assert!(!h.wants_more(149));
    }

    #[test]
    fn lanes_is_the_widest_row_clamped() {
        let mut history = History::new();
        assert_eq!(history.lanes(), 1);
        history.append(vec![entry("m", &["a", "b"]), entry("b", &["r"])], PAGE);
        assert_eq!(history.lanes(), 2);
    }
}
