//! The palette's git actions: stash, branches and tags, and the network. Each
//! action that changes the repository saves a checkpoint first; fetch, pull,
//! push and publish run in the background.

use std::path::Path;
use std::process::Command;

use gpui::{Context, Window};
use rvw_git::checkpoint::Kept;
use rvw_git::stash::{self, StashEntry};
use rvw_git::{GitError, gitx, refs, remote};
use rvw_view::palette::Action;

use crate::gate_card::Gated;
use crate::git_overlay::{self, is_picker};
use crate::git_tools::last_line;
use crate::shell::Shell;

impl Shell {
    /// Runs one of the palette's git actions. The prompts and pickers open
    /// their overlay; the network commands start in the background.
    pub fn run_git_action(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            Action::StashChanges | Action::NewBranch | Action::NewTag | Action::RenameBranch => {
                self.open_git_prompt(action, window, cx);
            }
            Action::ApplyStash | Action::DropStash => {
                let stashes = self.git.facts.stashes.clone();
                let items = stashes.iter().map(git_overlay::stash_item).collect();
                self.open_git_ask(action, items, stashes, window, cx);
            }
            // A picker of the local branches, minus the current one: the one
            // to delete, or the one to merge into this branch.
            Action::DeleteBranch | Action::MergeBranch => {
                let current = self.git.facts.branch.clone();
                let items = self
                    .git
                    .facts
                    .branches
                    .iter()
                    .filter(|name| current.as_deref() != Some(name.as_str()))
                    .cloned()
                    .collect();
                self.open_git_ask(action, items, Vec::new(), window, cx);
            }
            Action::DeleteTag | Action::PushTag => {
                let items = self.git.facts.tags.clone();
                self.open_git_ask(action, items, Vec::new(), window, cx);
            }
            Action::BrowseCommitFiles | Action::CompareWith => {
                self.open_history_ask(action, window, cx);
            }
            Action::NewWorktree | Action::RemoveWorktree => {
                self.open_worktree_ask(action, window, cx);
            }
            Action::Fetch => self.fetch(cx),
            Action::Pull => self.pull(cx),
            Action::Push => self.push(cx),
            Action::Publish => self.publish(cx),
            _ => {}
        }
    }

    /// Enter in the open prompt or picker: runs its action with the typed
    /// name or the chosen item. A failure stays in the overlay.
    pub(crate) fn submit_git_ask(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ask) = self.git.ask.as_ref() else {
            return;
        };
        let action = ask.action;
        let typed = ask.input.read(cx).text().trim().to_owned();
        let index = ask.chosen();
        let label = index
            .and_then(|i| ask.items.get(i).cloned())
            .unwrap_or_default();
        let stash = index.and_then(|i| ask.stashes.get(i).cloned());
        if is_picker(action) && index.is_none() {
            return;
        }
        let outcome = match action {
            Action::BrowseCommitFiles | Action::CompareWith => {
                self.submit_history_ask(action, &label, cx)
            }
            Action::NewWorktree | Action::RemoveWorktree => {
                self.submit_worktree_ask(action, &typed, &label, window, cx);
                return;
            }
            Action::PushTag => {
                match self.push_tag(&label, cx) {
                    Ok(()) => self.close_git_ask(cx),
                    Err(message) => self.git_ask_error(message, cx),
                }
                return;
            }
            // merge_branch reports its own result, so no finish or error here.
            Action::MergeBranch => {
                self.close_git_ask(cx);
                self.merge_branch(&label, cx);
                return;
            }
            Action::StashChanges => self.stash_changes(&typed, cx),
            Action::NewBranch | Action::NewTag | Action::RenameBranch if typed.is_empty() => {
                Err("Type a name first".to_owned())
            }
            Action::NewBranch => self.new_branch(&typed, cx),
            Action::NewTag => self.new_tag(&typed, cx),
            Action::RenameBranch => self.rename_branch(&typed, cx),
            Action::ApplyStash => match stash {
                Some(entry) => self.apply_stash(&label, &entry, cx),
                None => return,
            },
            Action::DropStash => match stash {
                Some(entry) => self.drop_stash(&label, &entry, cx),
                None => return,
            },
            Action::DeleteBranch => self.delete_branch(&label, cx),
            Action::DeleteTag => self.delete_tag(&label, cx),
            _ => return,
        };
        match outcome {
            Ok(done) => self.finish_git_ask(done, cx),
            Err(message) => self.git_ask_error(message, cx),
        }
    }

    /// Shows a failure in the open overlay, or on the Changes screen when the
    /// action had none.
    pub(crate) fn git_ask_error(&mut self, message: String, cx: &mut Context<Self>) {
        match self.git.ask.as_mut() {
            Some(ask) => ask.error = Some(message),
            None => self.changes.error = Some(message),
        }
        cx.notify();
    }

    /// Closes the overlay after a change, says what was done and re-reads
    /// the repository.
    fn finish_git_ask(&mut self, done: String, cx: &mut Context<Self>) {
        self.close_git_ask(cx);
        self.changes.notice = Some(done);
        self.changes.error = None;
        self.refresh_facts();
        self.reload_changes();
        let _ = self.refresh();
        cx.notify();
    }

    fn stash_changes(&mut self, message: &str, cx: &mut Context<Self>) -> Result<String, String> {
        self.checkpoint_before("Before stashing", &[], cx)?;
        let dir = self.repo_path().to_path_buf();
        match stash::push(&dir, message, true).map_err(|e| last_line(&e.stderr))? {
            Some(_) => Ok("Stashed the changes".to_owned()),
            None => Err("Nothing to stash".to_owned()),
        }
    }

    fn apply_stash(
        &mut self,
        label: &str,
        entry: &StashEntry,
        cx: &mut Context<Self>,
    ) -> Result<String, String> {
        self.checkpoint_before(&format!("Before applying {label}"), &[], cx)?;
        let dir = self.repo_path().to_path_buf();
        reply(stash::apply(&dir, entry), format!("Applied {label}"))
    }

    fn drop_stash(
        &mut self,
        label: &str,
        entry: &StashEntry,
        cx: &mut Context<Self>,
    ) -> Result<String, String> {
        let kept = Kept::Stash {
            commit: entry.commit.clone(),
            message: entry.message.clone(),
        };
        self.checkpoint_before(&format!("Before dropping {label}"), &[kept], cx)?;
        let dir = self.repo_path().to_path_buf();
        reply(stash::drop(&dir, entry), format!("Dropped {label}"))
    }

    fn new_branch(&mut self, name: &str, cx: &mut Context<Self>) -> Result<String, String> {
        self.checkpoint_before(&format!("Before creating branch {name}"), &[], cx)?;
        let dir = self.repo_path().to_path_buf();
        refs::create_branch(&dir, name, None).map_err(|e| last_line(&e.stderr))?;
        reply(
            gitx::switch(&dir, name),
            format!("Created and checked out {name}"),
        )
    }

    fn rename_branch(&mut self, name: &str, cx: &mut Context<Self>) -> Result<String, String> {
        let Some(old) = self.git.facts.branch.clone() else {
            return Err("HEAD is not on a branch".to_owned());
        };
        self.checkpoint_before(&format!("Before renaming {old} to {name}"), &[], cx)?;
        let dir = self.repo_path().to_path_buf();
        reply(
            refs::rename_branch(&dir, &old, name),
            format!("Renamed {old} to {name}"),
        )
    }

    fn delete_branch(&mut self, name: &str, cx: &mut Context<Self>) -> Result<String, String> {
        let dir = self.repo_path().to_path_buf();
        let full = format!("refs/heads/{name}");
        let target = gitx::commit(&dir, &full).ok_or_else(|| format!("No branch named {name}"))?;
        let kept = Kept::Ref { name: full, target };
        self.checkpoint_before(&format!("Before deleting branch {name}"), &[kept], cx)?;
        // Forced: the checkpoint keeps the commit, so the branch can come back.
        reply(
            refs::delete_branch(&dir, name, true),
            format!("Deleted branch {name}"),
        )
    }

    fn new_tag(&mut self, name: &str, cx: &mut Context<Self>) -> Result<String, String> {
        self.checkpoint_before(&format!("Before creating tag {name}"), &[], cx)?;
        let dir = self.repo_path().to_path_buf();
        reply(
            refs::create_tag(&dir, name, "HEAD", ""),
            format!("Created tag {name}"),
        )
    }

    fn delete_tag(&mut self, name: &str, cx: &mut Context<Self>) -> Result<String, String> {
        let dir = self.repo_path().to_path_buf();
        let full = format!("refs/tags/{name}");
        // The tag object of an annotated tag, which is what the ref points at.
        let target = git_stdout(&dir, &["rev-parse", "--verify", "--quiet", &full])
            .map(|out| out.trim().to_owned())
            .filter(|id| !id.is_empty())
            .ok_or_else(|| format!("No tag named {name}"))?;
        let kept = Kept::Ref { name: full, target };
        self.checkpoint_before(&format!("Before deleting tag {name}"), &[kept], cx)?;
        reply(refs::delete_tag(&dir, name), format!("Deleted tag {name}"))
    }

    /// Refuses a network command while another one runs.
    fn network_idle(&self) -> Result<(), String> {
        match self.git.busy.as_deref() {
            Some(busy) => Err(format!("Still running: {busy}")),
            None => Ok(()),
        }
    }

    /// Runs `job` in the background and shows its one-line result when it
    /// ends. Refused, with the reason, while another command runs.
    fn start_network<F>(
        &mut self,
        action: Action,
        label: &str,
        job: F,
        cx: &mut Context<Self>,
    ) -> Result<(), String>
    where
        F: FnOnce() -> Result<String, String> + Send + 'static,
    {
        self.network_idle()?;
        self.git.busy = Some(label.to_owned());
        self.git.network_result = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let outcome = cx.background_executor().spawn(async move { job() }).await;
            this.update(cx, |this, cx| this.finish_network(action, outcome, cx))
                .ok();
        })
        .detach();
        Ok(())
    }

    /// Clears the busy label and shows the command's line. A pull re-reads
    /// the worktree, which it changed.
    fn finish_network(
        &mut self,
        action: Action,
        outcome: Result<String, String>,
        cx: &mut Context<Self>,
    ) {
        self.git.busy = None;
        self.git.network_result = Some(match outcome {
            Ok(line) | Err(line) => line,
        });
        self.refresh_facts();
        if action == Action::Pull {
            self.reload_changes();
            let _ = self.refresh();
        }
        cx.notify();
    }

    /// Shows a refusal of a palette network command on the top bar.
    fn network_refused(&mut self, refusal: Result<(), String>, cx: &mut Context<Self>) {
        if let Err(message) = refusal {
            self.git.network_result = Some(message);
            cx.notify();
        }
    }

    fn fetch(&mut self, cx: &mut Context<Self>) {
        let dir = self.repo_path().to_path_buf();
        let remote = self.git.facts.remote.clone();
        let job = move || reply(remote::fetch(&dir, remote.as_deref()), "Fetched".to_owned());
        let refusal = self.start_network(Action::Fetch, "Fetching…", job, cx);
        self.network_refused(refusal, cx);
    }

    fn pull(&mut self, cx: &mut Context<Self>) {
        if let Err(message) = self.network_idle() {
            self.network_refused(Err(message), cx);
            return;
        }
        // A pull changes the worktree, so it is checkpointed first.
        if let Err(message) = self.checkpoint_before("Before pulling", &[], cx) {
            self.changes.error = Some(message);
            cx.notify();
            return;
        }
        let dir = self.repo_path().to_path_buf();
        let job = move || pull_job(&dir);
        let refusal = self.start_network(Action::Pull, "Pulling…", job, cx);
        self.network_refused(refusal, cx);
    }

    /// Pushes, once the commit gate lets it through. A push that cannot start
    /// (another command runs) is refused before the gate is asked.
    fn push(&mut self, cx: &mut Context<Self>) {
        if let Err(message) = self.network_idle() {
            self.network_refused(Err(message), cx);
            return;
        }
        self.gated(Gated::Push, cx);
    }

    /// The push itself, run once the gate is clear.
    pub(crate) fn push_now(&mut self, cx: &mut Context<Self>) {
        let dir = self.repo_path().to_path_buf();
        let job = move || reply(remote::push(&dir), "Pushed".to_owned());
        let refusal = self.start_network(Action::Push, "Pushing…", job, cx);
        self.network_refused(refusal, cx);
    }

    /// Publishes the branch, once the commit gate lets it through: it sends the
    /// branch's commits, so the gate reads it as a push.
    fn publish(&mut self, cx: &mut Context<Self>) {
        if self.git.facts.remote.is_none() {
            self.network_refused(Err("No remote to publish to".to_owned()), cx);
            return;
        }
        if let Err(message) = self.network_idle() {
            self.network_refused(Err(message), cx);
            return;
        }
        self.gated(Gated::Publish, cx);
    }

    /// The publish itself, run once the gate is clear.
    pub(crate) fn publish_now(&mut self, cx: &mut Context<Self>) {
        let Some(remote) = self.git.facts.remote.clone() else {
            self.network_refused(Err("No remote to publish to".to_owned()), cx);
            return;
        };
        let dir = self.repo_path().to_path_buf();
        let branch = self.git.facts.branch.clone().unwrap_or_default();
        let done = published(&branch, &remote);
        let job = move || reply(remote::publish(&dir, &remote), done);
        let refusal = self.start_network(Action::Publish, "Publishing…", job, cx);
        self.network_refused(refusal, cx);
    }

    /// Pushes the tag `name` to the default remote. Refusals are returned so
    /// the overlay can show them.
    fn push_tag(&mut self, name: &str, cx: &mut Context<Self>) -> Result<(), String> {
        self.network_idle()?;
        let remote = self
            .git
            .facts
            .remote
            .clone()
            .ok_or_else(|| "No remote to push to".to_owned())?;
        let dir = self.repo_path().to_path_buf();
        let tag = name.to_owned();
        let done = pushed_tag(name);
        let job = move || reply(remote::push_tag(&dir, &remote, &tag), done);
        self.start_network(Action::PushTag, &format!("Pushing tag {name}…"), job, cx)
    }
}

/// The run's outcome as the line the top bar shows: what it did, or git's reason.
fn reply<T>(result: Result<T, GitError>, done: String) -> Result<String, String> {
    result.map(|_| done).map_err(|e| last_line(&e.stderr))
}

/// The background half of a pull: fast-forwards, then counts what came in.
fn pull_job(dir: &Path) -> Result<String, String> {
    let pulled = remote::pull(dir).map_err(|e| last_line(&e.stderr))?;
    let count = commits_between(dir, &pulled.before, &pulled.after);
    Ok(pulled_line(count))
}

/// The commits in `before..after`, or None when they cannot be counted.
fn commits_between(dir: &Path, before: &str, after: &str) -> Option<usize> {
    if before.is_empty() {
        return None;
    }
    if before == after {
        return Some(0);
    }
    let range = format!("{before}..{after}");
    git_stdout(dir, &["rev-list", "--count", &range])?
        .trim()
        .parse()
        .ok()
}

/// Runs git read-only and returns its stdout when it succeeds.
fn git_stdout(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The top bar's line after a pull. None when the commits could not be
/// counted; the pull itself still happened.
fn pulled_line(count: Option<usize>) -> String {
    match count {
        Some(0) => "Already up to date".to_owned(),
        Some(1) => "Pulled 1 commit".to_owned(),
        Some(n) => format!("Pulled {n} commits"),
        None => "Pulled".to_owned(),
    }
}

/// The top bar's line after a publish.
fn published(branch: &str, remote: &str) -> String {
    format!("Published {branch} to {remote}")
}

/// The top bar's line after a tag push.
fn pushed_tag(name: &str) -> String {
    format!("Pushed tag {name}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pull_says_how_many_commits_came_in() {
        assert_eq!(pulled_line(Some(0)), "Already up to date");
        assert_eq!(pulled_line(Some(1)), "Pulled 1 commit");
        assert_eq!(pulled_line(Some(3)), "Pulled 3 commits");
        assert_eq!(pulled_line(None), "Pulled");
    }

    #[test]
    fn publish_and_tag_push_name_what_they_sent() {
        assert_eq!(published("feat/x", "origin"), "Published feat/x to origin");
        assert_eq!(pushed_tag("v1.2"), "Pushed tag v1.2");
    }

    #[test]
    fn a_reply_is_the_done_line_or_the_last_line_of_git() {
        let ok: Result<(), GitError> = Ok(());
        assert_eq!(reply(ok, "Fetched".to_owned()), Ok("Fetched".to_owned()));
        let failed: Result<(), GitError> = Err(GitError {
            args: vec!["push".to_owned()],
            stderr: "fatal: no\nrejected\n".to_owned(),
        });
        assert_eq!(
            reply(failed, "Pushed".to_owned()),
            Err("rejected".to_owned())
        );
    }
}
