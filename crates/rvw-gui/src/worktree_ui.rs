//! Worktrees from the window: a new one for a branch, which the window then
//! shows, and removing one. Switching is the workspace picker's job.

use std::path::{Path, PathBuf};

use gpui::{Context, Window};
use rvw_git::GitError;
use rvw_git::worktree::{self, Start, Worktree};
use rvw_view::palette::Action;
use rvw_view::workspaces;

use crate::git_tools::last_line;
use crate::shell::Shell;
use crate::workspace_picker::home;

impl Shell {
    /// Opens the prompt for a new worktree's branch, or the picker of the
    /// worktrees that can be removed. With none to remove, the Changes notice
    /// says so and no picker opens.
    pub(crate) fn open_worktree_ask(
        &mut self,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            Action::NewWorktree => self.open_git_ask(action, Vec::new(), Vec::new(), window, cx),
            Action::RemoveWorktree => {
                let choices = match removable(self.repo_path()) {
                    Ok(choices) => choices,
                    Err(e) => return self.worktree_notice(last_line(&e.stderr), cx),
                };
                if choices.is_empty() {
                    return self.worktree_notice("No other worktree to remove".to_owned(), cx);
                }
                let items = choices.into_iter().map(|(label, _)| label).collect();
                self.open_git_ask(action, items, Vec::new(), window, cx);
            }
            _ => {}
        }
    }

    /// Runs the worktree prompt (`typed`) or picker (`label`). Reports in
    /// the overlay on failure and closes it on success.
    pub(crate) fn submit_worktree_ask(
        &mut self,
        action: Action,
        typed: &str,
        label: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            Action::NewWorktree => self.add_worktree(typed, window, cx),
            Action::RemoveWorktree => self.remove_worktree(label, cx),
            _ => {}
        }
    }

    /// Adds a worktree for branch `typed` beside the main one, then shows it.
    /// The open worktree is not changed, so it gets no checkpoint.
    fn add_worktree(&mut self, typed: &str, window: &mut Window, cx: &mut Context<Self>) {
        if typed.is_empty() {
            return self.git_ask_error("Type a branch name first".to_owned(), cx);
        }
        // Refuse here, not after the add: a window that cannot be rebuilt
        // must not leave a new worktree behind that it did not show.
        if let Some(reason) = self.switch_would_lose(cx) {
            return self.git_ask_error(reason.to_owned(), cx);
        }
        let repo = self.repo_path().to_path_buf();
        let main = match worktree::list(&repo) {
            Ok(list) => match list.into_iter().find(|worktree| worktree.main) {
                Some(main) => main.path,
                None => return self.git_ask_error("git lists no main worktree".to_owned(), cx),
            },
            Err(e) => return self.git_ask_error(last_line(&e.stderr), cx),
        };
        let Some(path) = new_worktree_dir(&main, typed, Path::exists) else {
            let message = format!("No directory beside {}", main.display());
            return self.git_ask_error(message, cx);
        };
        let start = if self.git.facts.branches.iter().any(|name| name == typed) {
            Start::Branch(typed.to_owned())
        } else {
            Start::NewBranch {
                name: typed.to_owned(),
                at: None,
            }
        };
        if let Err(e) = worktree::add(&repo, &path, &start) {
            return self.git_ask_error(last_line(&e.stderr), cx);
        }
        self.close_git_ask(cx);
        self.switch_workspace(&path, window, cx);
    }

    /// Removes the linked worktree shown as `label`, after a checkpoint of the
    /// open one. Refusals from rvw-git (changes in the worktree, a lock) come
    /// back as the reason.
    fn remove_worktree(&mut self, label: &str, cx: &mut Context<Self>) {
        let path = match removable(self.repo_path()) {
            Ok(choices) => choices
                .into_iter()
                .find(|(shown, _)| shown == label)
                .map(|(_, path)| path),
            Err(e) => return self.git_ask_error(last_line(&e.stderr), cx),
        };
        let Some(path) = path else {
            return self.git_ask_error("That worktree is gone".to_owned(), cx);
        };
        if let Err(message) = self.checkpoint_before(&format!("Before removing {label}"), &[], cx) {
            return self.git_ask_error(message, cx);
        }
        let repo = self.repo_path().to_path_buf();
        if let Err(e) = worktree::remove(&repo, &path) {
            return self.git_ask_error(last_line(&e.stderr), cx);
        }
        self.close_git_ask(cx);
        self.git.network_result = Some(format!("Removed the worktree at {label}"));
        self.refresh_facts();
        cx.notify();
    }

    /// Shows one line on the Changes screen's notice line.
    fn worktree_notice(&mut self, message: String, cx: &mut Context<Self>) {
        self.git.network_result = Some(message);
        cx.notify();
    }
}

/// The linked worktrees a remove may take, as (picker label, path). Fails when
/// git cannot list the worktrees of `dir`.
fn removable(dir: &Path) -> Result<Vec<(String, PathBuf)>, GitError> {
    let worktrees = worktree::list(dir)?;
    let home = home();
    Ok(removable_in(&worktrees, home.as_deref()))
}

/// The linked worktrees of `worktrees` that are neither the main one nor the
/// open one, with their labels.
fn removable_in(worktrees: &[Worktree], home: Option<&str>) -> Vec<(String, PathBuf)> {
    worktrees
        .iter()
        .filter(|worktree| !worktree.main && !worktree.current)
        .map(|worktree| {
            let label = worktree_label(&worktree.path, worktree.branch.as_deref(), home);
            (label, worktree.path.clone())
        })
        .collect()
}

/// A worktree as the remove picker shows it: its path as shown, then its
/// branch, or "detached".
fn worktree_label(path: &Path, branch: Option<&str>, home: Option<&str>) -> String {
    let shown = workspaces::label(&path.to_string_lossy(), home);
    format!("{shown} ({})", branch.unwrap_or("detached"))
}

/// The directory for a new worktree of branch `typed`: a sibling of the main
/// worktree `main`, named after it and the branch with every `/` made `-`,
/// then `-2`, `-3`, … while `exists` says the name is taken. None when `main`
/// has no parent directory.
fn new_worktree_dir(main: &Path, typed: &str, exists: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    let parent = main.parent()?;
    let base = format!(
        "{}-{}",
        main.file_name()?.to_string_lossy(),
        typed.replace('/', "-")
    );
    let name = free_name(&base, |name| exists(&parent.join(name)));
    Some(parent.join(name))
}

/// `base`, or the first of `base-2`, `base-3`, … that `taken` does not name.
/// Each step past a taken name is one existing directory, so it ends.
fn free_name(base: &str, taken: impl Fn(&str) -> bool) -> String {
    let mut name = base.to_owned();
    let mut n = 2;
    while taken(&name) {
        name = format!("{base}-{n}");
        n += 1;
    }
    name
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wt(path: &str, branch: Option<&str>, main: bool, current: bool) -> Worktree {
        Worktree {
            path: path.into(),
            head: None,
            branch: branch.map(str::to_owned),
            main,
            current,
            locked: false,
            prunable: false,
        }
    }

    #[test]
    fn a_new_worktree_is_a_sibling_of_the_main_one_named_after_the_branch() {
        let main = Path::new("/home/me/src/rvw");
        let got = new_worktree_dir(main, "feat/login", |_| false);
        assert_eq!(got, Some(PathBuf::from("/home/me/src/rvw-feat-login")));
    }

    #[test]
    fn a_taken_directory_gets_the_next_number() {
        let taken = ["/home/me/src/rvw-x", "/home/me/src/rvw-x-2"];
        let got = new_worktree_dir(Path::new("/home/me/src/rvw"), "x", |path| {
            taken.iter().any(|t| Path::new(t) == path)
        });
        assert_eq!(got, Some(PathBuf::from("/home/me/src/rvw-x-3")));
    }

    #[test]
    fn a_main_worktree_at_the_root_has_no_sibling() {
        assert_eq!(new_worktree_dir(Path::new("/"), "x", |_| false), None);
    }

    #[test]
    fn a_free_name_is_kept_as_it_is() {
        assert_eq!(free_name("rvw-x", |_| false), "rvw-x");
        assert_eq!(free_name("rvw-x", |name| name == "rvw-x"), "rvw-x-2");
    }

    #[test]
    fn a_worktree_is_labelled_by_its_path_and_branch_or_detached() {
        let path = Path::new("/home/me/src/rvw-x");
        assert_eq!(
            worktree_label(path, Some("feat"), Some("/home/me")),
            "~/src/rvw-x (feat)"
        );
        assert_eq!(
            worktree_label(path, None, Some("/home/me")),
            "~/src/rvw-x (detached)"
        );
    }

    #[test]
    fn the_remove_picker_lists_linked_worktrees_but_not_the_main_or_open_one() {
        let worktrees = [
            wt("/w/main", Some("main"), true, false),
            wt("/w/open", Some("open"), false, true),
            wt("/w/feat", Some("feat"), false, false),
            wt("/w/loose", None, false, false),
        ];
        let got = removable_in(&worktrees, None);
        let labels: Vec<&str> = got.iter().map(|(label, _)| label.as_str()).collect();
        assert_eq!(labels, ["/w/feat (feat)", "/w/loose (detached)"]);
        assert_eq!(got[0].1, PathBuf::from("/w/feat"));
    }

    #[test]
    fn with_only_the_main_and_open_worktrees_nothing_can_be_removed() {
        let worktrees = [
            wt("/w/main", Some("main"), true, false),
            wt("/w/open", Some("open"), false, true),
        ];
        assert!(removable_in(&worktrees, None).is_empty());
    }
}
