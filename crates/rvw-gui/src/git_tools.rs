//! The repository facts the palette, the top bar and the Changes screen read
//! (branch, upstream, remotes, stashes, tags, checkpoints), the git overlays'
//! state, and the checkpoint every action that changes the repository takes
//! first.

use std::path::Path;

use gpui::Context;
use rvw_git::checkpoint::{self, Checkpoint, Kept};
use rvw_git::stash::StashEntry;
use rvw_git::{gitx, refs, remote, stash};

use crate::checkpoint_ui::RestoreConfirm;
use crate::git_overlay::GitAsk;
use crate::shell::Shell;

/// How many checkpoints a worktree keeps; older ones are pruned.
const KEEP_CHECKPOINTS: usize = 200;

/// What the repository looks like now, read after every git action.
#[derive(Debug, Clone, Default)]
pub struct RepoFacts {
    /// The checked-out branch; `None` when HEAD is detached.
    pub branch: Option<String>,
    /// The branch's upstream, e.g. "origin/main".
    pub upstream: Option<String>,
    /// Commits ahead of and behind the upstream.
    pub ahead_behind: Option<(usize, usize)>,
    /// The remote fetch, push and publish use.
    pub remote: Option<String>,
    pub branches: Vec<String>,
    pub tags: Vec<String>,
    pub stashes: Vec<StashEntry>,
    /// This worktree's checkpoints, newest first.
    pub checkpoints: Vec<Checkpoint>,
}

impl RepoFacts {
    /// Reads every fact; one that cannot be read is left empty.
    pub fn read(dir: &Path) -> Self {
        let branch =
            Some(gitx::branch(dir)).filter(|b| !b.is_empty() && !b.starts_with("detached"));
        let upstream = branch
            .as_deref()
            .and_then(|b| refs::upstream(dir, b).ok().flatten());
        let ahead_behind = branch
            .as_deref()
            .and_then(|b| refs::ahead_behind(dir, b).ok().flatten());
        Self {
            upstream,
            ahead_behind,
            remote: remote::default_remote(dir),
            branches: gitx::branches(dir)
                .map(|list| list.into_iter().map(|b| b.name).collect())
                .unwrap_or_default(),
            tags: refs::tags(dir).unwrap_or_default(),
            stashes: stash::list(dir).unwrap_or_default(),
            checkpoints: checkpoint::list(dir).unwrap_or_default(),
            branch,
        }
    }
}

/// The git overlays and facts the shell holds in one field.
#[derive(Default)]
pub struct GitTools {
    pub facts: RepoFacts,
    /// The open prompt or picker of a git action.
    pub ask: Option<GitAsk>,
    /// The open restore confirmation.
    pub confirm: Option<RestoreConfirm>,
    /// The network command running, e.g. "Pushing…".
    pub busy: Option<String>,
    /// How the last network command ended, one line.
    pub network_result: Option<String>,
}

impl Shell {
    /// The repository the window shows.
    pub(crate) fn repo_path(&self) -> &Path {
        Path::new(&self.identity.workspace)
    }

    /// Re-reads the repository facts.
    pub(crate) fn refresh_facts(&mut self) {
        self.live_epoch = self.live_epoch.wrapping_add(1);
        self.git.facts = RepoFacts::read(self.repo_path());
    }

    /// Saves a checkpoint before an action that changes the repository,
    /// keeping what the action is about to delete. When the checkpoint cannot
    /// be saved the action must not run: the error is the reason to show.
    pub(crate) fn checkpoint_before(
        &mut self,
        label: &str,
        kept: &[Kept],
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let dir = self.repo_path().to_path_buf();
        let saved = checkpoint::create(&dir, label, kept).map_err(|e| last_line(&e.stderr));
        // Pruning is housekeeping: a failure leaves extra checkpoints only.
        let _ = checkpoint::prune(&dir, KEEP_CHECKPOINTS);
        self.live_epoch = self.live_epoch.wrapping_add(1);
        self.git.facts.checkpoints = checkpoint::list(&dir).unwrap_or_default();
        cx.notify();
        saved.map(|_| ())
    }
}

/// The last non-empty line of git's message, which says what went wrong.
pub fn last_line(text: &str) -> String {
    text.lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("git failed without saying why")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_line_skips_blank_lines() {
        assert_eq!(
            last_line("On branch main\nnothing to commit\n\n"),
            "nothing to commit"
        );
        assert_eq!(last_line(""), "git failed without saying why");
    }
}
