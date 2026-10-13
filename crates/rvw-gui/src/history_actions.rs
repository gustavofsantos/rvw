//! The actions that bring commits in or tidy them: cherry-pick, merge, and
//! finishing or aborting the operation a conflict leaves, and the external
//! merge tool. Each action that changes the repository saves a checkpoint
//! first. A conflict is not an error: the operation stays in progress, and the
//! Changes screen names it and its files.

use std::process::{Command, Stdio};

use gpui::Context;
use rvw_git::merge::{self, FastForward, Operation, Outcome};
use rvw_view::history::short;
use rvw_view::keys::Screen;

use crate::git_tools::last_line;
use crate::shell::Shell;

impl Shell {
    /// Applies the commit selected on the History screen on top of HEAD.
    pub fn cherry_pick_selected(&mut self, cx: &mut Context<Self>) {
        let Some(row) = self.history.history.selected_row() else {
            return;
        };
        let id = row.entry.id.clone();
        let subject = row.entry.subject.clone();
        let label = format!("Before cherry-picking {}", short(&id));
        let outcome = self.checkpoint_before(&label, &[], cx).and_then(|()| {
            merge::cherry_pick(self.repo_path(), &id).map_err(|e| last_line(&e.stderr))
        });
        let line = match outcome {
            Ok(Outcome::Done { .. }) => Ok(format!("Cherry-picked {} “{subject}”", short(&id))),
            Ok(Outcome::UpToDate) => Ok(format!(
                "Nothing to cherry-pick: the change of {} is already here",
                short(&id)
            )),
            Ok(Outcome::Conflicts { files }) => Err(stopped("Cherry-pick", &files)),
            Err(message) => Err(message),
        };
        self.after_history_action(line, cx);
    }

    /// Merges branch `name` into the current branch, fast-forwarding when it
    /// can.
    pub(crate) fn merge_branch(&mut self, name: &str, cx: &mut Context<Self>) {
        let outcome = self
            .checkpoint_before(&format!("Before merging {name}"), &[], cx)
            .and_then(|()| {
                merge::merge(self.repo_path(), name, FastForward::Allow)
                    .map_err(|e| last_line(&e.stderr))
            });
        let line = match outcome {
            Ok(Outcome::Done { .. }) => Ok(format!("Merged {name}")),
            Ok(Outcome::UpToDate) => Ok(format!("Already up to date with {name}")),
            Ok(Outcome::Conflicts { files }) => Err(stopped("Merge", &files)),
            Err(message) => Err(message),
        };
        self.after_history_action(line, cx);
    }

    /// Aborts the merge or cherry-pick in progress.
    pub fn abort_operation(&mut self, cx: &mut Context<Self>) {
        let line = merge::abort(self.repo_path())
            .map(|()| "Aborted; the branch is as it was before".to_owned())
            .map_err(|e| last_line(&e.stderr));
        self.after_history_action(line, cx);
    }

    /// Commits the merge or cherry-pick in progress once every conflict is
    /// resolved and staged.
    pub fn finish_operation(&mut self, cx: &mut Context<Self>) {
        let line = merge::finish(self.repo_path())
            .map(|head| format!("Finished: {}", short(&head)))
            .map_err(|e| last_line(&e.stderr));
        self.after_history_action(line, cx);
    }

    /// Opens git's configured merge tool on the conflicted files, in the
    /// background; the live refresh shows what it resolves.
    pub fn open_merge_tool(&mut self, cx: &mut Context<Self>) {
        let started = Command::new("git")
            .arg("-C")
            .arg(self.repo_path())
            .args(["mergetool", "--no-prompt"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        if let Err(e) = started {
            self.changes.error = Some(format!("Could not start git mergetool: {e}"));
            cx.notify();
        }
    }

    /// Shows how an action ended and re-reads what it changed. A stop on a
    /// conflict opens the Changes screen, where the operation is finished.
    fn after_history_action(&mut self, line: Result<String, String>, cx: &mut Context<Self>) {
        self.refresh_facts();
        self.reload_changes();
        let _ = self.refresh();
        let stopped = merge::in_progress(self.repo_path())
            .ok()
            .flatten()
            .is_some();
        match line {
            Ok(done) => {
                self.history.notice = Some(done.clone());
                self.changes.notice = Some(done);
                self.changes.error = None;
            }
            Err(message) => {
                self.history.notice = Some(message.clone());
                self.changes.error = Some(message);
            }
        }
        if stopped {
            self.show(Screen::Changes, cx);
        } else if self.screen == Screen::History {
            self.reload_history(cx);
        }
        cx.notify();
    }
}

/// "Merge stopped: conflicts in a.txt, b.txt. …"
fn stopped(what: &str, files: &[String]) -> String {
    if files.is_empty() {
        return format!("{what} stopped. Check the staged changes, then Finish or Abort.");
    }
    format!(
        "{what} stopped: conflicts in {}. Resolve them and stage the files, then Finish; or Abort.",
        files.join(", ")
    )
}

/// The name an operation in progress is shown with.
pub fn operation_name(operation: Operation) -> &'static str {
    match operation {
        Operation::Merge => "merge",
        Operation::CherryPick => "cherry-pick",
        Operation::Revert => "revert",
        Operation::Rebase => "rebase",
    }
}
