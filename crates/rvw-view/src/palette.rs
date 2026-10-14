//! The action palette behind ⌘K (Ctrl+K off macOS): every action the user can take, whether it
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
    SwitchWorkspace,
    ToggleSplit,
    ToggleFullFile,
    ReloadQueue,
    GoToChanges,
    StageChosen,
    Commit,
    SaveCheckpoint,
    RestoreCheckpoint,
    StashChanges,
    ApplyStash,
    DropStash,
    NewBranch,
    RenameBranch,
    DeleteBranch,
    NewTag,
    PushTag,
    DeleteTag,
    Fetch,
    Pull,
    Push,
    Publish,
    GoToHistory,
    SearchCommits,
    FileHistory,
    CherryPick,
    MergeBranch,
    FinishOperation,
    AbortOperation,
    OpenMergeTool,
    BlameFile,
    BrowseCommitFiles,
    CompareWith,
    EditHistory,
    NewWorktree,
    RemoveWorktree,
    OpenTerminal,
    ToggleTerminalDock,
    NewTerminal,
    OpenDiffTool,
}

/// The reason a network action waits while a fetch, pull or push is running.
const WAIT_FOR_NETWORK: &str = "Wait for the running fetch, pull or push";

/// The reason a history action waits while a merge or cherry-pick is stopped.
const FINISH_OR_ABORT: &str = "Finish or abort the merge or cherry-pick first";

impl Action {
    /// Every action, in palette order.
    pub const ALL: [Action; 52] = [
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
        Action::SwitchWorkspace,
        Action::ToggleSplit,
        Action::ToggleFullFile,
        Action::ReloadQueue,
        Action::GoToChanges,
        Action::StageChosen,
        Action::Commit,
        Action::SaveCheckpoint,
        Action::RestoreCheckpoint,
        Action::StashChanges,
        Action::ApplyStash,
        Action::DropStash,
        Action::NewBranch,
        Action::RenameBranch,
        Action::DeleteBranch,
        Action::NewTag,
        Action::PushTag,
        Action::DeleteTag,
        Action::Fetch,
        Action::Pull,
        Action::Push,
        Action::Publish,
        Action::GoToHistory,
        Action::SearchCommits,
        Action::FileHistory,
        Action::CherryPick,
        Action::MergeBranch,
        Action::FinishOperation,
        Action::AbortOperation,
        Action::OpenMergeTool,
        Action::BlameFile,
        Action::BrowseCommitFiles,
        Action::CompareWith,
        Action::EditHistory,
        Action::NewWorktree,
        Action::RemoveWorktree,
        Action::OpenTerminal,
        Action::ToggleTerminalDock,
        Action::NewTerminal,
        Action::OpenDiffTool,
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
            Action::SwitchWorkspace => "Switch workspace…",
            Action::ToggleSplit => "Toggle split diff",
            Action::ToggleFullFile => "Toggle whole file and changes",
            Action::ReloadQueue => "Reload",
            Action::GoToChanges => "Go to the changes",
            Action::StageChosen => "Stage or unstage the selection",
            Action::Commit => "Commit",
            Action::SaveCheckpoint => "Save a checkpoint",
            Action::RestoreCheckpoint => "Restore a checkpoint…",
            Action::StashChanges => "Stash changes…",
            Action::ApplyStash => "Apply a stash…",
            Action::DropStash => "Drop a stash…",
            Action::NewBranch => "New branch…",
            Action::RenameBranch => "Rename this branch…",
            Action::DeleteBranch => "Delete a branch…",
            Action::NewTag => "New tag…",
            Action::PushTag => "Push a tag…",
            Action::DeleteTag => "Delete a tag…",
            Action::Fetch => "Fetch",
            Action::Pull => "Pull (fast-forward only)",
            Action::Push => "Push",
            Action::Publish => "Publish this branch",
            Action::GoToHistory => "Go to the history",
            Action::SearchCommits => "Search commits",
            Action::FileHistory => "History of this file",
            Action::CherryPick => "Cherry-pick the selected commit",
            Action::MergeBranch => "Merge a branch into this one…",
            Action::FinishOperation => "Finish the merge or cherry-pick",
            Action::AbortOperation => "Abort the merge or cherry-pick",
            Action::OpenMergeTool => "Open the merge tool",
            Action::BlameFile => "Blame this file",
            Action::BrowseCommitFiles => "Open a file as it was in the selected commit…",
            Action::CompareWith => "Compare the selected commit with…",
            Action::EditHistory => "Edit unpushed commits…",
            Action::NewWorktree => "New worktree…",
            Action::RemoveWorktree => "Remove a worktree…",
            Action::OpenTerminal => "Open a terminal in this repository",
            Action::ToggleTerminalDock => "Show or hide the terminal dock",
            Action::NewTerminal => "New terminal in the dock…",
            Action::OpenDiffTool => "Open this file in the diff tool",
        }
    }

    /// The group the palette lists the action under.
    #[must_use]
    pub fn group(self) -> &'static str {
        match self {
            Action::GoToQueue
            | Action::GoToReview
            | Action::GoToFiles
            | Action::GoToChanges
            | Action::GoToHistory => "Go",
            Action::FindFile | Action::SearchCommits => "Find",
            Action::FileHistory
            | Action::CherryPick
            | Action::BlameFile
            | Action::BrowseCommitFiles
            | Action::CompareWith
            | Action::EditHistory => "History",
            Action::NextFile | Action::PreviousFile => "Move",
            Action::Comment | Action::SubmitReview => "Review",
            Action::CompareUncommitted
            | Action::CompareDefaultBranch
            | Action::ComparePreviousCommit
            | Action::ToggleSplit
            | Action::ToggleFullFile => "Diff",
            Action::SwitchBranch
            | Action::SwitchWorkspace
            | Action::ReloadQueue
            | Action::NewWorktree
            | Action::RemoveWorktree => "Repository",
            Action::StageChosen
            | Action::Commit
            | Action::SaveCheckpoint
            | Action::RestoreCheckpoint
            | Action::FinishOperation
            | Action::AbortOperation
            | Action::OpenMergeTool => "Git",
            Action::OpenTerminal
            | Action::ToggleTerminalDock
            | Action::NewTerminal
            | Action::OpenDiffTool => "Tools",
            Action::StashChanges | Action::ApplyStash | Action::DropStash => "Stash",
            Action::NewBranch
            | Action::RenameBranch
            | Action::DeleteBranch
            | Action::NewTag
            | Action::DeleteTag
            | Action::MergeBranch => "Branches",
            Action::PushTag | Action::Fetch | Action::Pull | Action::Push | Action::Publish => {
                "Remote"
            }
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
            Action::StageChosen => Some("S"),
            Action::Commit => Some("⇧⌘C"),
            Action::RestoreCheckpoint => Some("⌘Z"),
            Action::SwitchWorkspace => Some("⌘O"),
            Action::ToggleTerminalDock => Some("⌘J"),
            Action::NewTerminal => Some("⌘T"),
            _ => None,
        }
    }
}

/// What the palette knows about the moment it opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // independent facts, one per rule
pub struct State {
    pub screen: Screen,
    /// Comments in the user's unsubmitted review.
    pub draft_count: usize,
    /// Diff lines are selected.
    pub has_selection: bool,
    /// The changed list or the tree has files to move through.
    pub has_files: bool,
    /// The file shown has changes, so it has both a diff and a whole view.
    pub file_has_changes: bool,
    /// Files are staged.
    pub has_staged: bool,
    /// HEAD is on a branch, not detached.
    pub on_branch: bool,
    /// The branch tracks a remote branch.
    pub has_upstream: bool,
    /// The repository has a remote.
    pub has_remote: bool,
    /// The repository has stashes.
    pub has_stashes: bool,
    /// The repository has tags.
    pub has_tags: bool,
    /// The repository has checkpoints.
    pub has_checkpoints: bool,
    /// A fetch, pull or push is running.
    pub network_busy: bool,
    /// A file is open in the review or the Changes screen.
    pub has_open_file: bool,
    /// The History screen has a commit selected.
    pub has_selected_commit: bool,
    /// A merge or cherry-pick is stopped, waiting to be finished or aborted.
    pub operation_in_progress: bool,
    /// Files still in conflict in the stopped merge or cherry-pick.
    pub conflicted: usize,
}

/// Whether the screen shows a file: the diff or the file list.
fn on_diff_or_files(screen: Screen) -> bool {
    matches!(screen, Screen::Diff | Screen::Files)
}

/// The wait reason while a fetch, pull or push runs, or `None` when idle.
fn busy_reason(state: &State) -> Option<&'static str> {
    state.network_busy.then_some(WAIT_FOR_NETWORK)
}

/// The reason a merge or cherry-pick must finish first, or `None` when none is stopped.
fn operation_reason(state: &State) -> Option<&'static str> {
    state.operation_in_progress.then_some(FINISH_OR_ABORT)
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
        Action::ToggleFullFile => (!(on_diff_or_files(state.screen) && state.file_has_changes))
            .then_some("Open a changed file first"),
        Action::GoToQueue => (state.screen == Screen::Queue).then_some("You are on the queue"),
        Action::GoToReview => (state.screen == Screen::Diff).then_some("You are on the review"),
        Action::GoToFiles => (state.screen == Screen::Files).then_some("You are on the Files tab"),
        Action::GoToChanges => {
            (state.screen == Screen::Changes).then_some("You are on the changes")
        }
        Action::GoToHistory => {
            (state.screen == Screen::History).then_some("You are on the history")
        }
        Action::FileHistory => (!state.has_open_file).then_some("Open a file first"),
        Action::CherryPick => (state.screen != Screen::History || !state.has_selected_commit)
            .then_some("Select a commit in the history first")
            .or_else(|| (!state.on_branch).then_some("HEAD is not on a branch"))
            .or_else(|| operation_reason(state)),
        Action::MergeBranch | Action::EditHistory => (!state.on_branch)
            .then_some("HEAD is not on a branch")
            .or_else(|| operation_reason(state)),
        Action::FinishOperation => (!state.operation_in_progress)
            .then_some("No merge or cherry-pick in progress")
            .or_else(|| (state.conflicted > 0).then_some("Resolve and stage every conflict first")),
        Action::AbortOperation => {
            (!state.operation_in_progress).then_some("No merge or cherry-pick in progress")
        }
        Action::OpenMergeTool => (state.conflicted == 0).then_some("No conflicted files"),
        Action::BlameFile | Action::OpenDiffTool => {
            (!state.has_open_file).then_some("Open a file first")
        }
        Action::BrowseCommitFiles | Action::CompareWith => (state.screen != Screen::History
            || !state.has_selected_commit)
            .then_some("Select a commit in the history first"),
        Action::StageChosen => {
            (state.screen != Screen::Changes).then_some("Open the changes first")
        }
        Action::Commit => (!state.has_staged).then_some("Stage something to commit"),
        Action::RestoreCheckpoint => (!state.has_checkpoints).then_some("No checkpoint yet"),
        Action::ApplyStash | Action::DropStash => (!state.has_stashes).then_some("No stash yet"),
        Action::RenameBranch => (!state.on_branch).then_some("HEAD is not on a branch"),
        Action::DeleteTag => (!state.has_tags).then_some("No tag yet"),
        Action::PushTag => (!state.has_remote)
            .then_some("No remote to push to")
            .or_else(|| (!state.has_tags).then_some("No tag yet"))
            .or_else(|| busy_reason(state)),
        Action::Fetch => (!state.has_remote)
            .then_some("No remote to fetch from")
            .or_else(|| busy_reason(state)),
        Action::Pull | Action::Push => (!state.on_branch)
            .then_some("HEAD is not on a branch")
            .or_else(|| (!state.has_upstream).then_some("Publish the branch first"))
            .or_else(|| busy_reason(state)),
        Action::Publish => (!state.has_remote)
            .then_some("No remote to publish to")
            .or_else(|| (!state.on_branch).then_some("HEAD is not on a branch"))
            .or_else(|| {
                state
                    .has_upstream
                    .then_some("The branch is published already")
            })
            .or_else(|| busy_reason(state)),
        Action::FindFile
        | Action::CompareUncommitted
        | Action::CompareDefaultBranch
        | Action::ComparePreviousCommit
        | Action::SwitchBranch
        | Action::SwitchWorkspace
        | Action::ReloadQueue
        | Action::SaveCheckpoint
        | Action::StashChanges
        | Action::NewBranch
        | Action::DeleteBranch
        | Action::NewTag
        | Action::SearchCommits
        | Action::NewWorktree
        | Action::RemoveWorktree
        | Action::OpenTerminal
        | Action::ToggleTerminalDock
        | Action::NewTerminal => None,
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
            file_has_changes: false,
            has_staged: false,
            on_branch: false,
            has_upstream: false,
            has_remote: false,
            has_stashes: false,
            has_tags: false,
            has_checkpoints: false,
            network_busy: false,
            has_open_file: false,
            has_selected_commit: false,
            operation_in_progress: false,
            conflicted: 0,
        }
    }

    #[test]
    fn all_has_forty_two_distinct_actions_with_titles_and_groups() {
        assert_eq!(Action::ALL.len(), 52);
        let mut seen = std::collections::HashSet::new();
        for action in Action::ALL {
            assert!(seen.insert(action), "duplicate {action:?}");
            assert!(!action.title().is_empty());
            assert!(!action.group().is_empty());
        }
    }

    #[test]
    fn new_actions_are_listed_at_the_end_in_table_order() {
        assert_eq!(
            Action::ALL[16..],
            [
                Action::GoToChanges,
                Action::StageChosen,
                Action::Commit,
                Action::SaveCheckpoint,
                Action::RestoreCheckpoint,
                Action::StashChanges,
                Action::ApplyStash,
                Action::DropStash,
                Action::NewBranch,
                Action::RenameBranch,
                Action::DeleteBranch,
                Action::NewTag,
                Action::PushTag,
                Action::DeleteTag,
                Action::Fetch,
                Action::Pull,
                Action::Push,
                Action::Publish,
                Action::GoToHistory,
                Action::SearchCommits,
                Action::FileHistory,
                Action::CherryPick,
                Action::MergeBranch,
                Action::FinishOperation,
                Action::AbortOperation,
                Action::OpenMergeTool,
                Action::BlameFile,
                Action::BrowseCommitFiles,
                Action::CompareWith,
                Action::EditHistory,
                Action::NewWorktree,
                Action::RemoveWorktree,
                Action::OpenTerminal,
                Action::ToggleTerminalDock,
                Action::NewTerminal,
                Action::OpenDiffTool,
            ]
        );
    }

    #[test]
    fn new_actions_have_their_titles_and_groups() {
        let table = [
            (Action::GoToChanges, "Go to the changes", "Go"),
            (Action::StageChosen, "Stage or unstage the selection", "Git"),
            (Action::Commit, "Commit", "Git"),
            (Action::SaveCheckpoint, "Save a checkpoint", "Git"),
            (Action::RestoreCheckpoint, "Restore a checkpoint…", "Git"),
            (Action::StashChanges, "Stash changes…", "Stash"),
            (Action::ApplyStash, "Apply a stash…", "Stash"),
            (Action::DropStash, "Drop a stash…", "Stash"),
            (Action::NewBranch, "New branch…", "Branches"),
            (Action::RenameBranch, "Rename this branch…", "Branches"),
            (Action::DeleteBranch, "Delete a branch…", "Branches"),
            (Action::NewTag, "New tag…", "Branches"),
            (Action::PushTag, "Push a tag…", "Remote"),
            (Action::DeleteTag, "Delete a tag…", "Branches"),
            (Action::Fetch, "Fetch", "Remote"),
            (Action::Pull, "Pull (fast-forward only)", "Remote"),
            (Action::Push, "Push", "Remote"),
            (Action::Publish, "Publish this branch", "Remote"),
            (Action::GoToHistory, "Go to the history", "Go"),
            (Action::SearchCommits, "Search commits", "Find"),
            (Action::FileHistory, "History of this file", "History"),
            (
                Action::CherryPick,
                "Cherry-pick the selected commit",
                "History",
            ),
            (
                Action::MergeBranch,
                "Merge a branch into this one…",
                "Branches",
            ),
            (
                Action::FinishOperation,
                "Finish the merge or cherry-pick",
                "Git",
            ),
            (
                Action::AbortOperation,
                "Abort the merge or cherry-pick",
                "Git",
            ),
            (Action::OpenMergeTool, "Open the merge tool", "Git"),
            (Action::BlameFile, "Blame this file", "History"),
            (
                Action::BrowseCommitFiles,
                "Open a file as it was in the selected commit…",
                "History",
            ),
            (
                Action::CompareWith,
                "Compare the selected commit with…",
                "History",
            ),
            (Action::EditHistory, "Edit unpushed commits…", "History"),
            (Action::NewWorktree, "New worktree…", "Repository"),
            (Action::RemoveWorktree, "Remove a worktree…", "Repository"),
            (
                Action::OpenTerminal,
                "Open a terminal in this repository",
                "Tools",
            ),
            (
                Action::OpenDiffTool,
                "Open this file in the diff tool",
                "Tools",
            ),
        ];
        for (action, title, group) in table {
            assert_eq!(action.title(), title, "{action:?}");
            assert_eq!(action.group(), group, "{action:?}");
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
        assert_eq!(Action::ToggleFullFile.chord(), None);
        assert_eq!(Action::StageChosen.chord(), Some("S"));
        assert_eq!(Action::Commit.chord(), Some("⇧⌘C"));
        assert_eq!(Action::RestoreCheckpoint.chord(), Some("⌘Z"));
        assert_eq!(Action::GoToChanges.chord(), None);
        assert_eq!(Action::Push.chord(), None);
        assert_eq!(Action::ToggleTerminalDock.chord(), Some("⌘J"));
        assert_eq!(Action::NewTerminal.chord(), Some("⌘T"));
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
        assert_eq!(
            unavailable(Action::GoToChanges, &state(Screen::Changes)),
            Some("You are on the changes")
        );
        assert_eq!(unavailable(Action::GoToChanges, &state(Screen::Diff)), None);
    }

    #[test]
    fn stage_chosen_needs_the_changes_screen() {
        assert_eq!(
            unavailable(Action::StageChosen, &state(Screen::Diff)),
            Some("Open the changes first")
        );
        assert_eq!(
            unavailable(Action::StageChosen, &state(Screen::Changes)),
            None
        );
    }

    #[test]
    fn commit_needs_staged_files() {
        let mut s = state(Screen::Changes);
        assert_eq!(
            unavailable(Action::Commit, &s),
            Some("Stage something to commit")
        );
        s.has_staged = true;
        assert_eq!(unavailable(Action::Commit, &s), None);
    }

    #[test]
    fn restoring_a_checkpoint_needs_one() {
        let mut s = state(Screen::Queue);
        assert_eq!(
            unavailable(Action::RestoreCheckpoint, &s),
            Some("No checkpoint yet")
        );
        s.has_checkpoints = true;
        assert_eq!(unavailable(Action::RestoreCheckpoint, &s), None);
    }

    #[test]
    fn applying_and_dropping_a_stash_need_one() {
        for action in [Action::ApplyStash, Action::DropStash] {
            let mut s = state(Screen::Queue);
            assert_eq!(unavailable(action, &s), Some("No stash yet"));
            s.has_stashes = true;
            assert_eq!(unavailable(action, &s), None);
        }
    }

    #[test]
    fn renaming_a_branch_needs_a_branch() {
        let mut s = state(Screen::Queue);
        assert_eq!(
            unavailable(Action::RenameBranch, &s),
            Some("HEAD is not on a branch")
        );
        s.on_branch = true;
        assert_eq!(unavailable(Action::RenameBranch, &s), None);
    }

    #[test]
    fn deleting_a_tag_needs_one() {
        let mut s = state(Screen::Queue);
        assert_eq!(unavailable(Action::DeleteTag, &s), Some("No tag yet"));
        s.has_tags = true;
        assert_eq!(unavailable(Action::DeleteTag, &s), None);
    }

    #[test]
    fn unconditional_git_actions_are_always_available() {
        for screen in [
            Screen::Queue,
            Screen::Diff,
            Screen::Files,
            Screen::Changes,
            Screen::History,
        ] {
            for action in [
                Action::SaveCheckpoint,
                Action::StashChanges,
                Action::NewBranch,
                Action::DeleteBranch,
                Action::NewTag,
            ] {
                assert_eq!(unavailable(action, &state(screen)), None, "{action:?}");
            }
        }
    }

    #[test]
    fn going_to_the_history_is_off_on_the_history_screen() {
        assert_eq!(
            unavailable(Action::GoToHistory, &state(Screen::History)),
            Some("You are on the history")
        );
        assert_eq!(unavailable(Action::GoToHistory, &state(Screen::Diff)), None);
    }

    #[test]
    fn searching_commits_is_always_available() {
        for screen in [
            Screen::Queue,
            Screen::Diff,
            Screen::Files,
            Screen::Changes,
            Screen::History,
        ] {
            assert_eq!(unavailable(Action::SearchCommits, &state(screen)), None);
        }
    }

    #[test]
    fn file_history_needs_an_open_file() {
        let mut s = state(Screen::Diff);
        assert_eq!(
            unavailable(Action::FileHistory, &s),
            Some("Open a file first")
        );
        s.has_open_file = true;
        assert_eq!(unavailable(Action::FileHistory, &s), None);
    }

    #[test]
    fn cherry_pick_needs_a_selected_commit_then_a_branch_then_no_operation() {
        let mut s = state(Screen::Diff);
        assert_eq!(
            unavailable(Action::CherryPick, &s),
            Some("Select a commit in the history first")
        );
        // A selected commit on another screen does not count.
        s.has_selected_commit = true;
        assert_eq!(
            unavailable(Action::CherryPick, &s),
            Some("Select a commit in the history first")
        );
        let mut s = state(Screen::History);
        s.has_selected_commit = true;
        assert_eq!(
            unavailable(Action::CherryPick, &s),
            Some("HEAD is not on a branch")
        );
        s.on_branch = true;
        assert_eq!(unavailable(Action::CherryPick, &s), None);
        s.operation_in_progress = true;
        assert_eq!(
            unavailable(Action::CherryPick, &s),
            Some("Finish or abort the merge or cherry-pick first")
        );
    }

    #[test]
    fn merge_branch_needs_a_branch_then_no_operation() {
        let mut s = state(Screen::Queue);
        assert_eq!(
            unavailable(Action::MergeBranch, &s),
            Some("HEAD is not on a branch")
        );
        s.on_branch = true;
        assert_eq!(unavailable(Action::MergeBranch, &s), None);
        s.operation_in_progress = true;
        assert_eq!(
            unavailable(Action::MergeBranch, &s),
            Some("Finish or abort the merge or cherry-pick first")
        );
    }

    #[test]
    fn finishing_an_operation_needs_one_stopped_with_no_conflicts_left() {
        let mut s = state(Screen::Changes);
        assert_eq!(
            unavailable(Action::FinishOperation, &s),
            Some("No merge or cherry-pick in progress")
        );
        s.operation_in_progress = true;
        s.conflicted = 2;
        assert_eq!(
            unavailable(Action::FinishOperation, &s),
            Some("Resolve and stage every conflict first")
        );
        s.conflicted = 0;
        assert_eq!(unavailable(Action::FinishOperation, &s), None);
    }

    #[test]
    fn aborting_an_operation_needs_one_stopped() {
        let mut s = state(Screen::Changes);
        assert_eq!(
            unavailable(Action::AbortOperation, &s),
            Some("No merge or cherry-pick in progress")
        );
        s.operation_in_progress = true;
        assert_eq!(unavailable(Action::AbortOperation, &s), None);
    }

    #[test]
    fn the_merge_tool_needs_a_conflicted_file() {
        let mut s = state(Screen::Changes);
        assert_eq!(
            unavailable(Action::OpenMergeTool, &s),
            Some("No conflicted files")
        );
        s.conflicted = 1;
        assert_eq!(unavailable(Action::OpenMergeTool, &s), None);
    }

    #[test]
    fn push_tag_needs_a_remote_then_a_tag_then_an_idle_network() {
        let mut s = state(Screen::Queue);
        assert_eq!(
            unavailable(Action::PushTag, &s),
            Some("No remote to push to")
        );
        s.has_remote = true;
        assert_eq!(unavailable(Action::PushTag, &s), Some("No tag yet"));
        s.has_tags = true;
        assert_eq!(unavailable(Action::PushTag, &s), None);
        s.network_busy = true;
        assert_eq!(
            unavailable(Action::PushTag, &s),
            Some("Wait for the running fetch, pull or push")
        );
    }

    #[test]
    fn fetch_needs_a_remote_then_an_idle_network() {
        let mut s = state(Screen::Queue);
        assert_eq!(
            unavailable(Action::Fetch, &s),
            Some("No remote to fetch from")
        );
        s.has_remote = true;
        assert_eq!(unavailable(Action::Fetch, &s), None);
        s.network_busy = true;
        assert_eq!(
            unavailable(Action::Fetch, &s),
            Some("Wait for the running fetch, pull or push")
        );
    }

    #[test]
    fn pull_and_push_need_a_branch_then_an_upstream_then_an_idle_network() {
        for action in [Action::Pull, Action::Push] {
            let mut s = state(Screen::Queue);
            assert_eq!(unavailable(action, &s), Some("HEAD is not on a branch"));
            s.on_branch = true;
            assert_eq!(unavailable(action, &s), Some("Publish the branch first"));
            s.has_upstream = true;
            assert_eq!(unavailable(action, &s), None);
            s.network_busy = true;
            assert_eq!(
                unavailable(action, &s),
                Some("Wait for the running fetch, pull or push")
            );
        }
    }

    #[test]
    fn publish_needs_a_remote_then_a_branch_then_no_upstream_then_an_idle_network() {
        let mut s = state(Screen::Queue);
        assert_eq!(
            unavailable(Action::Publish, &s),
            Some("No remote to publish to")
        );
        s.has_remote = true;
        assert_eq!(
            unavailable(Action::Publish, &s),
            Some("HEAD is not on a branch")
        );
        s.on_branch = true;
        assert_eq!(unavailable(Action::Publish, &s), None);
        s.has_upstream = true;
        assert_eq!(
            unavailable(Action::Publish, &s),
            Some("The branch is published already")
        );
        s.has_upstream = false;
        s.network_busy = true;
        assert_eq!(
            unavailable(Action::Publish, &s),
            Some("Wait for the running fetch, pull or push")
        );
    }

    #[test]
    fn the_first_failing_reason_wins() {
        // Detached HEAD with no upstream: the branch reason comes before the upstream one.
        let s = state(Screen::Queue);
        assert_eq!(
            unavailable(Action::Pull, &s),
            Some("HEAD is not on a branch")
        );
        // No remote and no tags: the remote reason comes first.
        assert_eq!(
            unavailable(Action::PushTag, &s),
            Some("No remote to push to")
        );
        // Detached and no remote: publish names the remote first.
        assert_eq!(
            unavailable(Action::Publish, &s),
            Some("No remote to publish to")
        );
        // Published and busy: the published reason comes before the wait.
        let mut s = state(Screen::Queue);
        s.has_remote = true;
        s.on_branch = true;
        s.has_upstream = true;
        s.network_busy = true;
        assert_eq!(
            unavailable(Action::Publish, &s),
            Some("The branch is published already")
        );
        // Tagged and busy with a remote: the wait is the only reason left for PushTag.
        s.has_tags = true;
        assert_eq!(
            unavailable(Action::PushTag, &s),
            Some("Wait for the running fetch, pull or push")
        );
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
                Action::SwitchWorkspace,
                Action::ReloadQueue,
                Action::SearchCommits,
            ] {
                assert_eq!(unavailable(action, &state(screen)), None);
            }
        }
    }

    #[test]
    fn empty_query_lists_all_actions_in_order() {
        let got = entries("", &state(Screen::Diff));
        assert_eq!(got.len(), 52);
        let order: Vec<Action> = got.iter().map(|e| e.action).collect();
        assert_eq!(order, Action::ALL);
        assert!(got.iter().all(|e| e.matches.is_empty() && e.score == 0));
    }

    #[test]
    fn push_query_finds_push_and_publish() {
        let got = entries("push", &state(Screen::Diff));
        for action in [Action::Push, Action::Publish] {
            assert!(got.iter().any(|e| e.action == action), "{action:?}");
        }
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
    fn workspace_matches_switch_workspace() {
        let got = entries("workspace", &state(Screen::Queue));
        assert_eq!(got.first().map(|e| e.action), Some(Action::SwitchWorkspace));
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
                "Go",
                "Go",
                "Find",
                "Find",
                "Move",
                "Move",
                "Review",
                "Review",
                "Diff",
                "Diff",
                "Diff",
                "Diff",
                "Diff",
                "Repository",
                "Repository",
                "Repository",
                "Repository",
                "Repository",
                "Git",
                "Git",
                "Git",
                "Git",
                "Git",
                "Git",
                "Git",
                "Stash",
                "Stash",
                "Stash",
                "Branches",
                "Branches",
                "Branches",
                "Branches",
                "Branches",
                "Branches",
                "Remote",
                "Remote",
                "Remote",
                "Remote",
                "Remote",
                "History",
                "History",
                "History",
                "History",
                "History",
                "History",
                "Tools",
                "Tools",
                "Tools",
                "Tools",
            ]
        );
        assert_eq!(got.len(), 52);
    }

    #[test]
    fn grouped_new_actions_keep_their_palette_order_within_each_group() {
        let got = grouped(entries("", &state(Screen::Diff)));
        let remote: Vec<Action> = got
            .iter()
            .filter(|e| e.group == "Remote")
            .map(|e| e.action)
            .collect();
        assert_eq!(
            remote,
            [
                Action::PushTag,
                Action::Fetch,
                Action::Pull,
                Action::Push,
                Action::Publish
            ]
        );
        let branches: Vec<Action> = got
            .iter()
            .filter(|e| e.group == "Branches")
            .map(|e| e.action)
            .collect();
        assert_eq!(
            branches,
            [
                Action::NewBranch,
                Action::RenameBranch,
                Action::DeleteBranch,
                Action::NewTag,
                Action::DeleteTag,
                Action::MergeBranch
            ]
        );
    }

    #[test]
    fn toggling_the_whole_file_needs_a_changed_file_on_the_review() {
        let reason = Some("Open a changed file first");
        let mut s = state(Screen::Diff);
        assert_eq!(unavailable(Action::ToggleFullFile, &s), reason);
        s.file_has_changes = true;
        assert_eq!(unavailable(Action::ToggleFullFile, &s), None);
        s.screen = Screen::Files;
        assert_eq!(unavailable(Action::ToggleFullFile, &s), None);
        s.screen = Screen::Queue;
        assert_eq!(unavailable(Action::ToggleFullFile, &s), reason);
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

    #[test]
    fn history_tools_need_a_file_or_a_selected_commit() {
        let mut s = state(Screen::Diff);
        assert_eq!(
            unavailable(Action::BlameFile, &s),
            Some("Open a file first")
        );
        assert_eq!(
            unavailable(Action::OpenDiffTool, &s),
            Some("Open a file first")
        );
        s.has_open_file = true;
        assert_eq!(unavailable(Action::BlameFile, &s), None);
        assert_eq!(
            unavailable(Action::CompareWith, &s),
            Some("Select a commit in the history first")
        );
        let mut s = state(Screen::History);
        s.has_selected_commit = true;
        assert_eq!(unavailable(Action::CompareWith, &s), None);
        assert_eq!(unavailable(Action::BrowseCommitFiles, &s), None);
        assert_eq!(unavailable(Action::NewWorktree, &s), None);
        assert_eq!(
            unavailable(Action::EditHistory, &s),
            Some("HEAD is not on a branch")
        );
        s.on_branch = true;
        s.operation_in_progress = true;
        assert_eq!(unavailable(Action::EditHistory, &s), Some(FINISH_OR_ABORT));
    }
}
