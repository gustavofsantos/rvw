//! The action palette behind ⌘K: every action the user can take, whether it
//! is available on the current screen, and the list ranked by a typed query.
//! Pure logic; the view draws the entries and dims the unavailable ones.

use crate::fuzzy::fuzzy_match;
use crate::keys::Screen;

/// One thing the user can do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    GoToQueue,
    GoToReview,
    GoToFiles,
    FindFile,
    NextFile,
    PreviousFile,
    Comment,
    SubmitReview,
    CompareUncommitted,
    CompareDefaultBranch,
    ComparePreviousCommit,
    SwitchBranch,
    ToggleSplit,
    ReloadQueue,
}

impl Action {
    /// Every action, in palette order.
    pub const ALL: [Action; 14] = [
        Action::GoToQueue,
        Action::GoToReview,
        Action::GoToFiles,
        Action::FindFile,
        Action::NextFile,
        Action::PreviousFile,
        Action::Comment,
        Action::SubmitReview,
        Action::CompareUncommitted,
        Action::CompareDefaultBranch,
        Action::ComparePreviousCommit,
        Action::SwitchBranch,
        Action::ToggleSplit,
        Action::ReloadQueue,
    ];

    /// The name shown in the palette.
    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            Action::GoToQueue => "Go to the queue",
            Action::GoToReview => "Go to the review",
            Action::GoToFiles => "Browse all files",
            Action::FindFile => "Find a file",
            Action::NextFile => "Next file",
            Action::PreviousFile => "Previous file",
            Action::Comment => "Comment on the selection",
            Action::SubmitReview => "Submit review",
            Action::CompareUncommitted => "Compare with: uncommitted changes",
            Action::CompareDefaultBranch => "Compare with: the default branch",
            Action::ComparePreviousCommit => "Compare with: the previous commit",
            Action::SwitchBranch => "Switch branch",
            Action::ToggleSplit => "Toggle split diff",
            Action::ReloadQueue => "Reload",
        }
    }

    /// The group the palette lists the action under.
    #[must_use]
    pub fn group(self) -> &'static str {
        match self {
            Action::GoToQueue | Action::GoToReview | Action::GoToFiles => "Go",
            Action::FindFile => "Find",
            Action::NextFile | Action::PreviousFile => "Move",
            Action::Comment | Action::SubmitReview => "Review",
            Action::CompareUncommitted
            | Action::CompareDefaultBranch
            | Action::ComparePreviousCommit
            | Action::ToggleSplit => "Diff",
            Action::SwitchBranch | Action::ReloadQueue => "Repository",
        }
    }

    /// The key that does the action directly, as shown on its cap, if any.
    #[must_use]
    pub fn chord(self) -> Option<&'static str> {
        match self {
            Action::FindFile => Some("⌘P"),
            Action::NextFile => Some("J"),
            Action::PreviousFile => Some("K"),
            Action::Comment => Some("C"),
            Action::SubmitReview => Some("⇧⌘R"),
            _ => None,
        }
    }
}

/// What the palette knows about the moment it opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct State {
    pub screen: Screen,
    /// Comments in the user's unsubmitted review.
    pub draft_count: usize,
    /// Diff lines are selected.
    pub has_selection: bool,
    /// The changed list or the tree has files to move through.
    pub has_files: bool,
}

/// Whether the screen shows a file: the diff or the file list.
fn on_diff_or_files(screen: Screen) -> bool {
    matches!(screen, Screen::Diff | Screen::Files)
}

/// Why an action is unavailable, or `None` when it is available.
#[must_use]
pub fn unavailable(action: Action, state: &State) -> Option<&'static str> {
    match action {
        Action::Comment => {
            if !on_diff_or_files(state.screen) {
                Some("Open a file first")
            } else if !state.has_selection {
                Some("Select some lines first")
            } else {
                None
            }
        }
        Action::SubmitReview => {
            (state.draft_count == 0).then_some("Add a comment to your review first")
        }
        Action::NextFile | Action::PreviousFile => (!(on_diff_or_files(state.screen)
            && state.has_files))
            .then_some("Open the review to move through files"),
        Action::ToggleSplit => (state.screen != Screen::Diff).then_some("Open a diff first"),
        Action::GoToQueue => (state.screen == Screen::Queue).then_some("You are on the queue"),
        Action::GoToReview => (state.screen == Screen::Diff).then_some("You are on the review"),
        Action::GoToFiles => (state.screen == Screen::Files).then_some("You are on the Files tab"),
        Action::FindFile
        | Action::CompareUncommitted
        | Action::CompareDefaultBranch
        | Action::ComparePreviousCommit
        | Action::SwitchBranch
        | Action::ReloadQueue => None,
    }
}

/// One row of the palette.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub action: Action,
    pub title: &'static str,
    pub group: &'static str,
    pub chord: Option<&'static str>,
    /// Char indices in `title` that the query matched, ascending.
    pub matches: Vec<usize>,
    /// Why the action cannot run now, shown dimmed beside it.
    pub unavailable: Option<&'static str>,
    pub score: i64,
}

/// The palette rows for `query`. An empty query lists every action in
/// [`Action::ALL`] order. Otherwise the rows are the actions whose title, or
/// `"<group> <title>"`, matches the query, best score first and ties in
/// `ALL` order. Unavailable actions are listed too.
#[must_use]
pub fn entries(query: &str, state: &State) -> Vec<Entry> {
    let mut out: Vec<Entry> = Action::ALL
        .iter()
        .filter_map(|&action| {
            let title = action.title();
            let group = action.group();
            let (score, matches) = if query.is_empty() {
                (0, Vec::new())
            } else {
                score_of(query, action)?
            };
            Some(Entry {
                action,
                title,
                group,
                chord: action.chord(),
                matches,
                unavailable: unavailable(action, state),
                score,
            })
        })
        .collect();
    // A stable sort keeps ALL order among equal scores.
    out.sort_by_key(|entry| std::cmp::Reverse(entry.score));
    out
}

/// The score of `action` for a non-empty `query`, and the title chars it
/// matched. The title and `"<group> <title>"` are both tried; the better match
/// wins, ties going to the title. `None` when neither matches.
fn score_of(query: &str, action: Action) -> Option<(i64, Vec<usize>)> {
    let title = action.title();
    let title_match = fuzzy_match(query, title);
    let prefix = action.group().chars().count() + 1;
    let grouped = fuzzy_match(query, &format!("{} {title}", action.group())).map(|found| {
        // Keep only the positions that fall inside the title.
        let inside: Vec<usize> = found
            .positions
            .into_iter()
            .filter(|&at| at >= prefix)
            .map(|at| at - prefix)
            .collect();
        (found.score, inside)
    });
    match (title_match, grouped) {
        (Some(t), Some(g)) if g.0 > t.score => Some(g),
        (Some(t), _) => Some((t.score, t.positions)),
        (None, g) => g,
    }
}

/// `entries` reordered so that each group's rows sit together, groups in the
/// order they first appear and rows in their given order. For the grouped
/// list shown when the query is empty.
#[must_use]
pub fn grouped(entries: Vec<Entry>) -> Vec<Entry> {
    let mut groups: Vec<&'static str> = Vec::new();
    for entry in &entries {
        if !groups.contains(&entry.group) {
            groups.push(entry.group);
        }
    }
    let rank = |entry: &Entry| groups.iter().position(|g| *g == entry.group);
    let mut keyed: Vec<(Option<usize>, Entry)> = entries
        .into_iter()
        .map(|entry| (rank(&entry), entry))
        .collect();
    keyed.sort_by_key(|(group, _)| *group);
    keyed.into_iter().map(|(_, entry)| entry).collect()
}

/// The index of the first runnable entry, or 0 when none is.
#[must_use]
pub fn first_available(entries: &[Entry]) -> usize {
    entries
        .iter()
        .position(|entry| entry.unavailable.is_none())
        .unwrap_or(0)
}

/// The next runnable entry from `from` going down or up, or `from` itself
/// when there is none in that direction.
#[must_use]
pub fn step(entries: &[Entry], from: usize, down: bool) -> usize {
    let found = if down {
        (from + 1..entries.len()).find(|&at| entries[at].unavailable.is_none())
    } else {
        (0..from.min(entries.len()))
            .rev()
            .find(|&at| entries[at].unavailable.is_none())
    };
    found.unwrap_or(from)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(screen: Screen) -> State {
        State {
            screen,
            draft_count: 0,
            has_selection: false,
            has_files: false,
        }
    }

    #[test]
    fn all_has_fourteen_distinct_actions_with_titles_and_groups() {
        assert_eq!(Action::ALL.len(), 14);
        let mut seen = std::collections::HashSet::new();
        for action in Action::ALL {
            assert!(seen.insert(action), "duplicate {action:?}");
            assert!(!action.title().is_empty());
            assert!(!action.group().is_empty());
        }
    }

    #[test]
    fn chords_are_the_directly_bound_keys() {
        assert_eq!(Action::FindFile.chord(), Some("⌘P"));
        assert_eq!(Action::NextFile.chord(), Some("J"));
        assert_eq!(Action::PreviousFile.chord(), Some("K"));
        assert_eq!(Action::Comment.chord(), Some("C"));
        assert_eq!(Action::SubmitReview.chord(), Some("⇧⌘R"));
        assert_eq!(Action::SwitchBranch.chord(), None);
        assert_eq!(Action::ReloadQueue.chord(), None);
    }

    #[test]
    fn comment_needs_a_file_and_a_selection() {
        let mut s = state(Screen::Queue);
        s.has_selection = true;
        assert_eq!(unavailable(Action::Comment, &s), Some("Open a file first"));

        let mut s = state(Screen::Diff);
        assert_eq!(
            unavailable(Action::Comment, &s),
            Some("Select some lines first")
        );
        s.has_selection = true;
        assert_eq!(unavailable(Action::Comment, &s), None);

        let mut s = state(Screen::Files);
        s.has_selection = true;
        assert_eq!(unavailable(Action::Comment, &s), None);
    }

    #[test]
    fn submit_review_needs_a_draft() {
        let mut s = state(Screen::Diff);
        assert_eq!(
            unavailable(Action::SubmitReview, &s),
            Some("Add a comment to your review first")
        );
        s.draft_count = 2;
        assert_eq!(unavailable(Action::SubmitReview, &s), None);
    }

    #[test]
    fn moving_through_files_needs_a_file_list_on_a_diff_or_files_screen() {
        for action in [Action::NextFile, Action::PreviousFile] {
            let mut s = state(Screen::Diff);
            assert_eq!(
                unavailable(action, &s),
                Some("Open the review to move through files")
            );
            s.has_files = true;
            assert_eq!(unavailable(action, &s), None);

            let mut s = state(Screen::Queue);
            s.has_files = true;
            assert!(unavailable(action, &s).is_some());

            let mut s = state(Screen::Files);
            s.has_files = true;
            assert_eq!(unavailable(action, &s), None);
        }
    }

    #[test]
    fn toggle_split_needs_the_diff_screen() {
        assert_eq!(
            unavailable(Action::ToggleSplit, &state(Screen::Files)),
            Some("Open a diff first")
        );
        assert_eq!(unavailable(Action::ToggleSplit, &state(Screen::Diff)), None);
    }

    #[test]
    fn go_to_actions_are_off_on_their_own_screen() {
        assert_eq!(
            unavailable(Action::GoToQueue, &state(Screen::Queue)),
            Some("You are on the queue")
        );
        assert_eq!(unavailable(Action::GoToQueue, &state(Screen::Diff)), None);
        assert_eq!(
            unavailable(Action::GoToReview, &state(Screen::Diff)),
            Some("You are on the review")
        );
        assert_eq!(unavailable(Action::GoToReview, &state(Screen::Files)), None);
        assert_eq!(
            unavailable(Action::GoToFiles, &state(Screen::Files)),
            Some("You are on the Files tab")
        );
        assert_eq!(unavailable(Action::GoToFiles, &state(Screen::Queue)), None);
    }

    #[test]
    fn always_available_actions_have_no_reason() {
        for screen in [Screen::Queue, Screen::Diff, Screen::Files, Screen::History] {
            for action in [
                Action::FindFile,
                Action::CompareUncommitted,
                Action::CompareDefaultBranch,
                Action::ComparePreviousCommit,
                Action::SwitchBranch,
                Action::ReloadQueue,
            ] {
                assert_eq!(unavailable(action, &state(screen)), None);
            }
        }
    }

    #[test]
    fn empty_query_lists_all_actions_in_order() {
        let got = entries("", &state(Screen::Diff));
        assert_eq!(got.len(), 14);
        let order: Vec<Action> = got.iter().map(|e| e.action).collect();
        assert_eq!(order, Action::ALL);
        assert!(got.iter().all(|e| e.matches.is_empty() && e.score == 0));
    }

    #[test]
    fn sub_ranks_submit_review_first() {
        let got = entries("sub", &state(Screen::Diff));
        assert_eq!(got.first().map(|e| e.action), Some(Action::SubmitReview));
    }

    #[test]
    fn branch_matches_switch_branch() {
        let got = entries("branch", &state(Screen::Diff));
        assert!(got.iter().any(|e| e.action == Action::SwitchBranch));
    }

    #[test]
    fn nonsense_query_matches_nothing() {
        assert!(entries("xyzzy", &state(Screen::Diff)).is_empty());
    }

    #[test]
    fn matches_are_valid_char_indices_into_the_title() {
        for query in ["sub", "branch", "cmp", "diff", "go", "⌘", "é", "r"] {
            for entry in entries(query, &state(Screen::Diff)) {
                let len = entry.title.chars().count();
                assert!(
                    entry.matches.iter().all(|&at| at < len),
                    "{query}: {:?} out of {}",
                    entry.matches,
                    entry.title
                );
                assert!(entry.matches.windows(2).all(|w| w[0] < w[1]));
            }
        }
    }

    #[test]
    fn group_prefix_matches_stay_inside_the_title() {
        // "review" matches only through the group name for Comment-free titles;
        // every returned index must still point into the title.
        for entry in entries("review", &state(Screen::Diff)) {
            let len = entry.title.chars().count();
            assert!(entry.matches.iter().all(|&at| at < len));
        }
    }

    #[test]
    fn ties_keep_all_order() {
        // "e" matches several titles at the same score; they must keep ALL order.
        let got = entries("re", &state(Screen::Diff));
        let positions: Vec<usize> = got
            .iter()
            .filter(|e| e.score == got[0].score)
            .filter_map(|e| Action::ALL.iter().position(|&a| a == e.action))
            .collect();
        assert!(positions.windows(2).all(|w| w[0] < w[1]), "{positions:?}");
    }

    #[test]
    fn unavailable_actions_are_still_listed() {
        let got = entries("submit", &state(Screen::Diff));
        let submit = got.iter().find(|e| e.action == Action::SubmitReview);
        assert_eq!(
            submit.and_then(|e| e.unavailable),
            Some("Add a comment to your review first")
        );
    }

    #[test]
    fn grouping_keeps_each_group_together_in_first_seen_order() {
        let got = grouped(entries("", &state(Screen::Diff)));
        let groups: Vec<&str> = got.iter().map(|e| e.group).collect();
        assert_eq!(
            groups,
            [
                "Go",
                "Go",
                "Go",
                "Find",
                "Move",
                "Move",
                "Review",
                "Review",
                "Diff",
                "Diff",
                "Diff",
                "Diff",
                "Repository",
                "Repository"
            ]
        );
        assert_eq!(got.len(), 14);
    }

    #[test]
    fn stepping_skips_unavailable_entries_and_stops_at_the_ends() {
        let got = entries("", &state(Screen::Queue));
        let first = first_available(&got);
        assert_eq!(got[first].unavailable, None);
        let down = step(&got, first, true);
        assert!(down > first && got[down].unavailable.is_none());
        assert_eq!(step(&got, down, false), first);
        assert_eq!(step(&got, first, false), first);
        let last = got.len() - 1;
        assert_eq!(step(&got, last, true), last);
        assert_eq!(first_available(&[]), 0);
    }
}
