//! The Changes screen: the staged and unstaged lists, the lines chosen to
//! stage, the commit button's label, and what open comments touch.

use std::collections::BTreeSet;

use rvw_core::types::{Comment, Status};
use rvw_git::index::FileStatus;

use crate::diff::{FileDiff, LineKind};
use crate::selection::Pos;

/// Which list a file sits in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Staged,
    Unstaged,
}

impl Side {
    fn other(self) -> Self {
        match self {
            Side::Staged => Side::Unstaged,
            Side::Unstaged => Side::Staged,
        }
    }
}

/// One row of a list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    /// The path before a staged rename or copy.
    pub orig_path: Option<String>,
    /// git's letter as a char: 'M', 'A', 'D', 'R', 'C', 'T', 'U', or '?' for
    /// untracked.
    pub letter: char,
    /// Open review comments (pending or pulled) on this file.
    pub open_comments: usize,
    /// The file moves only as a whole: anything but a plain modification ('M').
    pub whole_file_only: bool,
}

/// The two lists, each sorted by path.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Lists {
    pub staged: Vec<Entry>,
    pub unstaged: Vec<Entry>,
}

/// Splits the changed paths into the staged and unstaged lists. A path with
/// changes on both sides, or unmerged, appears in both lists.
#[must_use]
pub fn lists(status: &[FileStatus], comments: &[Comment]) -> Lists {
    let mut lists = Lists::default();
    for file in status {
        if let Some(letter) = file.staged {
            lists.staged.push(entry(file, letter, comments));
        }
        if let Some(letter) = file.unstaged {
            lists.unstaged.push(entry(file, letter, comments));
        }
    }
    lists.staged.sort_by(|a, b| a.path.cmp(&b.path));
    lists.unstaged.sort_by(|a, b| a.path.cmp(&b.path));
    lists
}

fn entry(file: &FileStatus, letter: u8, comments: &[Comment]) -> Entry {
    let letter = char::from(letter);
    Entry {
        path: file.path.clone(),
        orig_path: file.orig_path.clone(),
        letter,
        open_comments: comments
            .iter()
            .filter(|c| is_open(c) && c.file == file.path)
            .count(),
        whole_file_only: letter != 'M',
    }
}

/// A comment that still needs a decision: pending or pulled.
fn is_open(comment: &Comment) -> bool {
    matches!(comment.status, Status::Pending | Status::Pulled)
}

impl Lists {
    /// The list on `side`.
    #[must_use]
    pub fn side(&self, side: Side) -> &[Entry] {
        match side {
            Side::Staged => &self.staged,
            Side::Unstaged => &self.unstaged,
        }
    }

    /// The entry for `path` on `side`, with its index.
    #[must_use]
    pub fn find(&self, side: Side, path: &str) -> Option<(usize, &Entry)> {
        self.side(side)
            .iter()
            .enumerate()
            .find(|(_, entry)| entry.path == path)
    }
}

/// The lines the user chose in the open diff: an anchor and a head, both set
/// or both unset. Any line can be an end, removed lines included.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LineChoice {
    anchor: Option<Pos>,
    head: Option<Pos>,
}

impl LineChoice {
    /// Whether no line is chosen.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.anchor.is_none()
    }

    /// Selects `pos` alone; a pos outside the diff changes nothing.
    pub fn click(&mut self, diff: &FileDiff, pos: Pos) {
        if in_diff(diff, pos) {
            *self = LineChoice {
                anchor: Some(pos),
                head: Some(pos),
            };
        }
    }

    /// Moves the head to `pos`, keeping the anchor; with no anchor, as `click`.
    /// A pos outside the diff changes nothing.
    pub fn extend(&mut self, diff: &FileDiff, pos: Pos) {
        if !in_diff(diff, pos) {
            return;
        }
        if self.anchor.is_none() {
            self.click(diff, pos);
        } else {
            self.head = Some(pos);
        }
    }

    /// Drops the choice.
    pub fn clear(&mut self) {
        *self = LineChoice::default();
    }

    /// Whether `pos` lies between the anchor and the head, inclusive, in diff order.
    #[must_use]
    pub fn contains(&self, pos: Pos) -> bool {
        self.bounds().is_some_and(|(lo, hi)| lo <= pos && pos <= hi)
    }

    /// The added and removed lines in the range as (hunk, line) pairs; context
    /// lines are left out.
    #[must_use]
    pub fn chosen(&self, diff: &FileDiff) -> BTreeSet<(usize, usize)> {
        let mut chosen = BTreeSet::new();
        for (hunk, h) in diff.hunks.iter().enumerate() {
            for (line, l) in h.lines.iter().enumerate() {
                if l.kind != LineKind::Context && self.contains(Pos { hunk, line }) {
                    chosen.insert((hunk, line));
                }
            }
        }
        chosen
    }

    /// The anchor and head in diff order.
    fn bounds(&self) -> Option<(Pos, Pos)> {
        let (anchor, head) = (self.anchor?, self.head?);
        Some(if anchor <= head {
            (anchor, head)
        } else {
            (head, anchor)
        })
    }
}

/// Whether `pos` names a line of the diff, removed lines included.
fn in_diff(diff: &FileDiff, pos: Pos) -> bool {
    diff.hunks
        .get(pos.hunk)
        .is_some_and(|hunk| pos.line < hunk.lines.len())
}

/// Open comments on `path` whose lines overlap hunk `hunk`'s new-side lines
/// (the smallest to the largest `new_no` in the hunk). 0 when the hunk has no
/// new-side line or does not exist.
#[must_use]
pub fn hunk_open_comments(diff: &FileDiff, hunk: usize, path: &str, comments: &[Comment]) -> usize {
    let Some(hunk) = diff.hunks.get(hunk) else {
        return 0;
    };
    let new_nos = || hunk.lines.iter().filter_map(|line| line.new_no);
    let (Some(lo), Some(hi)) = (new_nos().min(), new_nos().max()) else {
        return 0;
    };
    let (lo, hi) = (i64::from(lo), i64::from(hi));
    comments
        .iter()
        .filter(|c| is_open(c) && c.file == path && c.start_line <= hi && c.end_line >= lo)
        .count()
}

/// The commit button's text: "Commit 1 file", "Commit 3 files"; "Nothing staged" for 0.
#[must_use]
pub fn commit_label(staged_files: usize) -> String {
    match staged_files {
        0 => "Nothing staged".to_owned(),
        1 => "Commit 1 file".to_owned(),
        n => format!("Commit {n} files"),
    }
}

/// Notices above the commit button, about open comments on staged files: the
/// pulled ones first, then the pending ones. A line is left out when its count
/// is 0. These never block a commit.
#[must_use]
pub fn before_commit(lists: &Lists, comments: &[Comment]) -> Vec<String> {
    let staged: BTreeSet<&str> = lists.staged.iter().map(|e| e.path.as_str()).collect();
    let on_staged = |c: &&Comment| staged.contains(c.file.as_str());
    let pulled = comments
        .iter()
        .filter(|c| c.status == Status::Pulled)
        .filter(on_staged)
        .count();
    let pending = comments
        .iter()
        .filter(|c| c.status == Status::Pending)
        .filter(on_staged)
        .count();

    let mut lines = Vec::new();
    match pulled {
        0 => {}
        1 => lines.push("1 comment is with an agent and touches a staged file.".to_owned()),
        n => lines.push(format!(
            "{n} comments are with an agent and touch staged files."
        )),
    }
    match pending {
        0 => {}
        1 => lines.push("1 comment waiting for a pull touches a staged file.".to_owned()),
        n => lines.push(format!(
            "{n} comments waiting for a pull touch staged files."
        )),
    }
    lines
}

/// The git action a commit gate guards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateAction {
    Commit,
    Amend,
    Push,
}

impl GateAction {
    /// The action name the core operation accepts: "commit" or "push".
    #[must_use]
    pub fn core_action(self) -> &'static str {
        match self {
            GateAction::Commit | GateAction::Amend => "commit",
            GateAction::Push => "push",
        }
    }

    /// The button verb: "Commit", "Amend" or "Push".
    #[must_use]
    pub fn verb(self) -> &'static str {
        match self {
            GateAction::Commit => "Commit",
            GateAction::Amend => "Amend",
            GateAction::Push => "Push",
        }
    }

    /// The lowercase noun used in the confirmation question.
    fn noun(self) -> &'static str {
        match self {
            GateAction::Commit => "commit",
            GateAction::Amend => "amend",
            GateAction::Push => "push",
        }
    }
}

/// What still needs a decision before a commit or push goes ahead: the open
/// comments (pending or pulled) and the decided ones with no verdict, in one
/// lane. An empty `lane` means every lane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gate {
    pub open: usize,
    pub unjudged: usize,
    pub lane: String,
}

impl Gate {
    /// Builds the gate from the ids of the open comments and of the decided
    /// comments without a verdict.
    #[must_use]
    pub fn from_ids(open: &[String], unjudged: &[String], lane: &str) -> Gate {
        Gate {
            open: open.len(),
            unjudged: unjudged.len(),
            lane: lane.to_owned(),
        }
    }

    /// Whether nothing needs a decision.
    #[must_use]
    pub fn is_clear(&self) -> bool {
        self.open == 0 && self.unjudged == 0
    }
}

/// The confirmation sentence for a gate that is not clear, or `None` when it
/// is clear. The zero clause is left out; the question follows the action.
#[must_use]
pub fn gate_notice(gate: &Gate, action: GateAction) -> Option<String> {
    if gate.is_clear() {
        return None;
    }
    let mut clauses = Vec::new();
    match gate.open {
        0 => {}
        1 => clauses.push("1 comment is still open".to_owned()),
        n => clauses.push(format!("{n} comments are still open")),
    }
    match gate.unjudged {
        0 => {}
        1 => clauses.push("1 decided comment has no verdict".to_owned()),
        n => clauses.push(format!("{n} decided comments have no verdict")),
    }
    let lane = gate.lane.trim();
    let scope = if lane.is_empty() {
        "in every lane".to_owned()
    } else {
        format!("in lane {lane}")
    };
    Some(format!(
        "{} {scope}. Go ahead with the {} anyway?",
        clauses.join(" and "),
        action.noun()
    ))
}

/// Whether a reason typed to get past a gate is usable: not blank.
#[must_use]
pub fn gate_reason_ok(reason: &str) -> bool {
    !reason.trim().is_empty()
}

/// The button that goes ahead past a gate: "Commit anyway", "Amend anyway",
/// "Push anyway".
#[must_use]
pub fn gate_confirm_label(action: GateAction) -> String {
    format!("{} anyway", action.verb())
}

/// J and K: the next (`delta` 1) or previous (-1) file, staged list first, then
/// unstaged, clamped at the ends. With no current file, or one that is not in
/// the lists, the first file. `None` when both lists are empty.
#[must_use]
pub fn step(lists: &Lists, current: Option<(Side, &str)>, delta: isize) -> Option<(Side, String)> {
    let files: Vec<(Side, &str)> = lists
        .staged
        .iter()
        .map(|e| (Side::Staged, e.path.as_str()))
        .chain(
            lists
                .unstaged
                .iter()
                .map(|e| (Side::Unstaged, e.path.as_str())),
        )
        .collect();
    let last = files.len().checked_sub(1)?;
    let here =
        current.and_then(|(side, path)| files.iter().position(|&(s, p)| s == side && p == path));
    let next = match here {
        None => 0,
        Some(i) => match usize::try_from(delta) {
            Ok(forward) => i.saturating_add(forward).min(last),
            Err(_) => i.saturating_sub(delta.unsigned_abs()),
        },
    };
    let &(side, path) = files.get(next)?;
    Some((side, path.to_owned()))
}

/// The file to show after the lists were re-read: the same side and path when
/// still there; else the same path on the other side; else the entry at
/// `index` on the same side, clamped to the last one; else the first entry of
/// the other side; else `None`.
#[must_use]
pub fn reselect(lists: &Lists, side: Side, path: &str, index: usize) -> Option<(Side, String)> {
    if lists.find(side, path).is_some() {
        return Some((side, path.to_owned()));
    }
    if lists.find(side.other(), path).is_some() {
        return Some((side.other(), path.to_owned()));
    }
    let same = lists.side(side);
    match same.len().checked_sub(1) {
        Some(last) => Some((side, same[index.min(last)].path.clone())),
        None => lists
            .side(side.other())
            .first()
            .map(|e| (side.other(), e.path.clone())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::parse;

    /// Two hunks. Hunk 0: ctx "one", removed "two", added "TWO", ctx "three"
    /// (new lines 1..=3). Hunk 1: ctx "ten", added "eleven", ctx "twelve"
    /// (new lines 10..=12).
    const DIFF: &str = "diff --git a/f.rs b/f.rs\n--- a/f.rs\n+++ b/f.rs\n@@ -1,3 +1,3 @@\n one\n-two\n+TWO\n three\n@@ -10,2 +10,3 @@\n ten\n+eleven\n twelve\n";

    fn file(path: &str, staged: Option<u8>, unstaged: Option<u8>) -> FileStatus {
        FileStatus {
            path: path.into(),
            orig_path: None,
            staged,
            unstaged,
        }
    }

    fn comment(file: &str, start: i64, end: i64, status: Status) -> Comment {
        Comment {
            points_at: None,
            reopens: None,
            verdict: None,
            id: "r1".into(),
            status,
            workspace: "/ws".into(),
            lane: None,
            author: None,
            file: file.into(),
            path: format!("/ws/{file}"),
            start_line: start,
            end_line: end,
            filetype: "rust".into(),
            code: String::new(),
            comment: "note".into(),
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

    fn pos(hunk: usize, line: usize) -> Pos {
        Pos { hunk, line }
    }

    #[test]
    fn a_file_with_both_sides_appears_in_both_lists() {
        let lists = lists(&[file("a.rs", Some(b'M'), Some(b'M'))], &[]);
        assert_eq!(lists.staged.len(), 1);
        assert_eq!(lists.unstaged.len(), 1);
        assert_eq!(lists.staged[0].letter, 'M');
        assert_eq!(lists.unstaged[0].letter, 'M');
    }

    #[test]
    fn an_untracked_file_appears_only_in_unstaged() {
        let lists = lists(&[file("new.rs", None, Some(b'?'))], &[]);
        assert!(lists.staged.is_empty());
        assert_eq!(lists.unstaged.len(), 1);
        assert_eq!(lists.unstaged[0].letter, '?');
        assert!(lists.unstaged[0].whole_file_only);
    }

    #[test]
    fn a_rename_keeps_the_original_path_on_both_entries() {
        let status = FileStatus {
            path: "new.rs".into(),
            orig_path: Some("old.rs".into()),
            staged: Some(b'R'),
            unstaged: Some(b'M'),
        };
        let lists = lists(&[status], &[]);
        assert_eq!(lists.staged[0].orig_path.as_deref(), Some("old.rs"));
        assert_eq!(lists.unstaged[0].orig_path.as_deref(), Some("old.rs"));
        assert_eq!(lists.staged[0].letter, 'R');
        assert!(lists.staged[0].whole_file_only);
        assert!(!lists.unstaged[0].whole_file_only);
    }

    #[test]
    fn only_a_plain_modification_moves_by_line() {
        for letter in *b"MADRCT?" {
            let lists = lists(&[file("f.rs", None, Some(letter))], &[]);
            assert_eq!(
                lists.unstaged[0].whole_file_only,
                letter != b'M',
                "letter {}",
                char::from(letter)
            );
        }
        let staged = lists(&[file("f.rs", Some(b'D'), None)], &[]);
        assert!(staged.staged[0].whole_file_only);
    }

    #[test]
    fn an_unmerged_file_appears_on_both_sides_and_moves_whole() {
        let lists = lists(&[file("u.rs", Some(b'U'), Some(b'U'))], &[]);
        assert_eq!(lists.staged[0].letter, 'U');
        assert_eq!(lists.unstaged[0].letter, 'U');
        assert!(lists.staged[0].whole_file_only);
        assert!(lists.unstaged[0].whole_file_only);
    }

    #[test]
    fn each_list_is_sorted_by_path() {
        let lists = lists(
            &[
                file("z.rs", Some(b'M'), None),
                file("a.rs", Some(b'M'), None),
                file("m.rs", None, Some(b'M')),
                file("b.rs", None, Some(b'M')),
            ],
            &[],
        );
        let staged: Vec<&str> = lists.staged.iter().map(|e| e.path.as_str()).collect();
        let unstaged: Vec<&str> = lists.unstaged.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(staged, ["a.rs", "z.rs"]);
        assert_eq!(unstaged, ["b.rs", "m.rs"]);
    }

    #[test]
    fn open_comments_count_pending_and_pulled_on_that_file_only() {
        let comments = [
            comment("a.rs", 1, 1, Status::Pending),
            comment("a.rs", 2, 2, Status::Pulled),
            comment("a.rs", 3, 3, Status::Done),
            comment("a.rs", 4, 4, Status::Rejected),
            comment("b.rs", 1, 1, Status::Pending),
        ];
        let lists = lists(
            &[
                file("a.rs", Some(b'M'), None),
                file("b.rs", None, Some(b'M')),
            ],
            &comments,
        );
        assert_eq!(lists.staged[0].open_comments, 2);
        assert_eq!(
            lists.find(Side::Unstaged, "b.rs").unwrap().1.open_comments,
            1
        );
    }

    #[test]
    fn find_gives_the_index_on_the_named_side() {
        let lists = lists(
            &[
                file("a.rs", Some(b'M'), None),
                file("b.rs", Some(b'M'), Some(b'M')),
            ],
            &[],
        );
        let (index, entry) = lists.find(Side::Staged, "b.rs").unwrap();
        assert_eq!((index, entry.path.as_str()), (1, "b.rs"));
        assert!(lists.find(Side::Unstaged, "a.rs").is_none());
    }

    #[test]
    fn click_selects_one_line_and_removed_lines_can_be_chosen() {
        let diff = parse(DIFF);
        let mut choice = LineChoice::default();
        assert!(choice.is_empty());
        choice.click(&diff, pos(0, 1));
        assert!(!choice.is_empty());
        assert!(choice.contains(pos(0, 1)));
        assert!(!choice.contains(pos(0, 2)));
        assert_eq!(choice.chosen(&diff), BTreeSet::from([(0, 1)]));
    }

    #[test]
    fn a_click_outside_the_diff_changes_nothing() {
        let diff = parse(DIFF);
        let mut choice = LineChoice::default();
        choice.click(&diff, pos(5, 0));
        choice.click(&diff, pos(0, 99));
        assert!(choice.is_empty());

        choice.click(&diff, pos(1, 1));
        choice.extend(&diff, pos(9, 0));
        assert!(choice.contains(pos(1, 1)));
        assert!(!choice.contains(pos(1, 2)));
    }

    #[test]
    fn extend_backwards_puts_the_head_before_the_anchor() {
        let diff = parse(DIFF);
        let mut choice = LineChoice::default();
        choice.click(&diff, pos(0, 3));
        choice.extend(&diff, pos(0, 1));
        assert!(choice.contains(pos(0, 1)));
        assert!(choice.contains(pos(0, 2)));
        assert!(choice.contains(pos(0, 3)));
        assert!(!choice.contains(pos(0, 0)));
        assert_eq!(choice.chosen(&diff), BTreeSet::from([(0, 1), (0, 2)]));
    }

    #[test]
    fn extend_without_an_anchor_acts_as_click() {
        let diff = parse(DIFF);
        let mut choice = LineChoice::default();
        choice.extend(&diff, pos(1, 1));
        assert!(choice.contains(pos(1, 1)));
        assert!(!choice.contains(pos(0, 2)));
    }

    #[test]
    fn chosen_skips_context_and_spans_two_hunks() {
        let diff = parse(DIFF);
        let mut choice = LineChoice::default();
        choice.click(&diff, pos(0, 2));
        choice.extend(&diff, pos(1, 1));
        assert_eq!(choice.chosen(&diff), BTreeSet::from([(0, 2), (1, 1)]));
    }

    #[test]
    fn clear_drops_the_choice() {
        let diff = parse(DIFF);
        let mut choice = LineChoice::default();
        choice.click(&diff, pos(0, 0));
        choice.clear();
        assert!(choice.is_empty());
        assert!(!choice.contains(pos(0, 0)));
    }

    /// A hunk with no new-side line: two removed lines.
    const PURE_REMOVAL: &str = "--- a/g.rs\n+++ b/g.rs\n@@ -5,2 +4,0 @@\n-a\n-b\n";

    #[test]
    fn hunk_open_comments_counts_overlap_at_the_edges() {
        let diff = parse(DIFF);
        // Hunk 1's new lines are 10..=12.
        let comments = [
            comment("f.rs", 9, 9, Status::Pending),
            comment("f.rs", 10, 10, Status::Pulled),
            comment("f.rs", 12, 12, Status::Pending),
            comment("f.rs", 13, 14, Status::Pending),
            comment("f.rs", 1, 1, Status::Pending),
        ];
        assert_eq!(hunk_open_comments(&diff, 1, "f.rs", &comments), 2);
        assert_eq!(hunk_open_comments(&diff, 0, "f.rs", &comments), 1);
    }

    #[test]
    fn hunk_open_comments_ignores_closed_comments_and_other_files() {
        let diff = parse(DIFF);
        let comments = [
            comment("f.rs", 10, 10, Status::Done),
            comment("f.rs", 11, 11, Status::Rejected),
            comment("g.rs", 11, 11, Status::Pending),
        ];
        assert_eq!(hunk_open_comments(&diff, 1, "f.rs", &comments), 0);
    }

    #[test]
    fn hunk_open_comments_is_zero_for_a_pure_removal_or_a_missing_hunk() {
        let diff = parse(PURE_REMOVAL);
        let comments = [comment("g.rs", 4, 4, Status::Pending)];
        assert_eq!(hunk_open_comments(&diff, 0, "g.rs", &comments), 0);
        assert_eq!(hunk_open_comments(&diff, 3, "g.rs", &comments), 0);
    }

    #[test]
    fn commit_label_for_none_one_and_many() {
        assert_eq!(commit_label(0), "Nothing staged");
        assert_eq!(commit_label(1), "Commit 1 file");
        assert_eq!(commit_label(2), "Commit 2 files");
    }

    #[test]
    fn before_commit_is_empty_without_open_comments_on_staged_files() {
        let lists = lists(&[file("a.rs", Some(b'M'), None)], &[]);
        assert!(before_commit(&lists, &[]).is_empty());

        let other = [
            comment("b.rs", 1, 1, Status::Pulled),
            comment("a.rs", 1, 1, Status::Done),
        ];
        assert!(before_commit(&lists, &other).is_empty());
    }

    #[test]
    fn before_commit_gives_singular_and_plural_lines_in_order() {
        let lists = lists(
            &[
                file("a.rs", Some(b'M'), None),
                file("b.rs", Some(b'M'), None),
            ],
            &[],
        );
        let one_each = [
            comment("a.rs", 1, 1, Status::Pulled),
            comment("b.rs", 1, 1, Status::Pending),
        ];
        assert_eq!(
            before_commit(&lists, &one_each),
            [
                "1 comment is with an agent and touches a staged file.",
                "1 comment waiting for a pull touches a staged file.",
            ]
        );

        let several = [
            comment("a.rs", 1, 1, Status::Pulled),
            comment("b.rs", 1, 1, Status::Pulled),
            comment("b.rs", 2, 2, Status::Pending),
            comment("a.rs", 3, 3, Status::Pending),
        ];
        assert_eq!(
            before_commit(&lists, &several),
            [
                "2 comments are with an agent and touch staged files.",
                "2 comments waiting for a pull touch staged files.",
            ]
        );
    }

    #[test]
    fn before_commit_leaves_out_a_zero_line() {
        let lists = lists(&[file("a.rs", Some(b'M'), None)], &[]);
        let pending_only = [comment("a.rs", 1, 1, Status::Pending)];
        assert_eq!(
            before_commit(&lists, &pending_only),
            ["1 comment waiting for a pull touches a staged file."]
        );
    }

    fn two_lists() -> Lists {
        lists(
            &[
                file("a.rs", Some(b'M'), None),
                file("b.rs", Some(b'M'), None),
                file("c.rs", None, Some(b'M')),
                file("d.rs", None, Some(b'M')),
            ],
            &[],
        )
    }

    #[test]
    fn step_crosses_from_the_last_staged_file_to_the_first_unstaged() {
        let lists = two_lists();
        assert_eq!(
            step(&lists, Some((Side::Staged, "b.rs")), 1),
            Some((Side::Unstaged, "c.rs".to_owned()))
        );
        assert_eq!(
            step(&lists, Some((Side::Unstaged, "c.rs")), -1),
            Some((Side::Staged, "b.rs".to_owned()))
        );
    }

    #[test]
    fn step_clamps_at_both_ends() {
        let lists = two_lists();
        assert_eq!(
            step(&lists, Some((Side::Unstaged, "d.rs")), 1),
            Some((Side::Unstaged, "d.rs".to_owned()))
        );
        assert_eq!(
            step(&lists, Some((Side::Staged, "a.rs")), -1),
            Some((Side::Staged, "a.rs".to_owned()))
        );
        assert_eq!(
            step(&lists, Some((Side::Staged, "a.rs")), 10),
            Some((Side::Unstaged, "d.rs".to_owned()))
        );
    }

    #[test]
    fn step_without_a_current_file_gives_the_first_file() {
        let lists = two_lists();
        assert_eq!(
            step(&lists, None, 1),
            Some((Side::Staged, "a.rs".to_owned()))
        );
        assert_eq!(
            step(&lists, None, -1),
            Some((Side::Staged, "a.rs".to_owned()))
        );
        assert_eq!(
            step(&lists, Some((Side::Staged, "gone.rs")), 1),
            Some((Side::Staged, "a.rs".to_owned()))
        );
    }

    #[test]
    fn step_is_none_when_both_lists_are_empty() {
        let lists = Lists::default();
        assert_eq!(step(&lists, None, 1), None);
        assert_eq!(step(&lists, Some((Side::Staged, "a.rs")), -1), None);
    }

    #[test]
    fn reselect_keeps_the_same_side_and_path_when_still_there() {
        let lists = two_lists();
        assert_eq!(
            reselect(&lists, Side::Staged, "a.rs", 0),
            Some((Side::Staged, "a.rs".to_owned()))
        );
    }

    #[test]
    fn reselect_moves_to_the_other_side_for_the_same_path() {
        let lists = two_lists();
        assert_eq!(
            reselect(&lists, Side::Staged, "c.rs", 0),
            Some((Side::Unstaged, "c.rs".to_owned()))
        );
    }

    #[test]
    fn reselect_takes_the_entry_at_the_index_clamped_to_the_last() {
        let lists = lists(
            &[
                file("a.rs", Some(b'M'), None),
                file("c.rs", Some(b'M'), None),
            ],
            &[],
        );
        assert_eq!(
            reselect(&lists, Side::Staged, "b.rs", 1),
            Some((Side::Staged, "c.rs".to_owned()))
        );
        assert_eq!(
            reselect(&lists, Side::Staged, "x.rs", 9),
            Some((Side::Staged, "c.rs".to_owned()))
        );
    }

    #[test]
    fn reselect_falls_back_to_the_first_of_the_other_side() {
        let lists = lists(&[file("c.rs", None, Some(b'M'))], &[]);
        assert_eq!(
            reselect(&lists, Side::Staged, "x.rs", 0),
            Some((Side::Unstaged, "c.rs".to_owned()))
        );
    }

    #[test]
    fn reselect_is_none_when_both_lists_are_empty() {
        assert_eq!(reselect(&Lists::default(), Side::Unstaged, "x.rs", 3), None);
    }

    fn ids(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| (*v).to_owned()).collect()
    }

    fn gate(open: usize, unjudged: usize, lane: &str) -> Gate {
        Gate {
            open,
            unjudged,
            lane: lane.to_owned(),
        }
    }

    #[test]
    fn a_gate_with_nothing_open_or_unjudged_is_clear() {
        assert!(Gate::from_ids(&[], &[], "main").is_clear());
        assert!(!gate(1, 0, "main").is_clear());
        assert!(!gate(0, 1, "main").is_clear());
    }

    #[test]
    fn from_ids_counts_each_list_and_keeps_the_lane() {
        let gate = Gate::from_ids(&ids(&["a", "b", "c"]), &ids(&["d", "e"]), "main");
        assert_eq!(gate, self::gate(3, 2, "main"));
    }

    #[test]
    fn gate_notice_is_none_when_clear() {
        assert_eq!(
            gate_notice(&self::gate(0, 0, "main"), GateAction::Commit),
            None
        );
    }

    #[test]
    fn gate_notice_names_only_open_comments() {
        assert_eq!(
            gate_notice(&self::gate(3, 0, "main"), GateAction::Commit).as_deref(),
            Some("3 comments are still open in lane main. Go ahead with the commit anyway?")
        );
    }

    #[test]
    fn gate_notice_names_only_unjudged_comments() {
        assert_eq!(
            gate_notice(&self::gate(0, 2, "main"), GateAction::Commit).as_deref(),
            Some(
                "2 decided comments have no verdict in lane main. Go ahead with the commit anyway?"
            )
        );
    }

    #[test]
    fn gate_notice_names_both_clauses() {
        assert_eq!(
            gate_notice(&self::gate(3, 2, "main"), GateAction::Commit).as_deref(),
            Some(
                "3 comments are still open and 2 decided comments have no verdict in lane main. Go ahead with the commit anyway?"
            )
        );
    }

    #[test]
    fn gate_notice_uses_singular_wording_for_one() {
        let notice = |g: Gate| gate_notice(&g, GateAction::Commit).unwrap();
        assert_eq!(
            notice(self::gate(1, 0, "main")),
            "1 comment is still open in lane main. Go ahead with the commit anyway?"
        );
        assert_eq!(
            notice(self::gate(0, 1, "main")),
            "1 decided comment has no verdict in lane main. Go ahead with the commit anyway?"
        );
        assert_eq!(
            notice(self::gate(1, 1, "main")),
            "1 comment is still open and 1 decided comment has no verdict in lane main. Go ahead with the commit anyway?"
        );
    }

    #[test]
    fn gate_notice_says_every_lane_when_no_lane_is_named() {
        assert_eq!(
            gate_notice(&self::gate(3, 2, ""), GateAction::Commit).as_deref(),
            Some("3 comments are still open and 2 decided comments have no verdict in every lane. Go ahead with the commit anyway?")
        );
        assert_eq!(
            gate_notice(&self::gate(1, 0, "  "), GateAction::Commit).as_deref(),
            Some("1 comment is still open in every lane. Go ahead with the commit anyway?")
        );
    }

    #[test]
    fn gate_notice_question_follows_the_action() {
        let g = self::gate(1, 0, "main");
        assert!(gate_notice(&g, GateAction::Commit)
            .unwrap()
            .ends_with("Go ahead with the commit anyway?"));
        assert!(gate_notice(&g, GateAction::Amend)
            .unwrap()
            .ends_with("Go ahead with the amend anyway?"));
        assert!(gate_notice(&g, GateAction::Push)
            .unwrap()
            .ends_with("Go ahead with the push anyway?"));
    }

    #[test]
    fn each_action_has_its_verb_and_core_action() {
        assert_eq!(GateAction::Commit.verb(), "Commit");
        assert_eq!(GateAction::Commit.core_action(), "commit");
        assert_eq!(GateAction::Amend.verb(), "Amend");
        assert_eq!(GateAction::Amend.core_action(), "commit");
        assert_eq!(GateAction::Push.verb(), "Push");
        assert_eq!(GateAction::Push.core_action(), "push");
    }

    #[test]
    fn confirm_label_follows_the_verb() {
        assert_eq!(gate_confirm_label(GateAction::Commit), "Commit anyway");
        assert_eq!(gate_confirm_label(GateAction::Amend), "Amend anyway");
        assert_eq!(gate_confirm_label(GateAction::Push), "Push anyway");
    }

    #[test]
    fn a_reason_is_ok_only_when_it_is_not_blank() {
        assert!(!gate_reason_ok(""));
        assert!(!gate_reason_ok("   "));
        assert!(!gate_reason_ok("\t\n"));
        assert!(gate_reason_ok("checked with the author"));
        assert!(gate_reason_ok("  ok  "));
    }
}
