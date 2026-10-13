//! The recent-actions list and its undo. Each action records how to reverse it
//! and the state the repository must still be in for that to be safe.

use std::fs;
use std::io;
use std::path::Path;
use std::process::Command;
use std::time::SystemTime;

use rvw_git::commits::{self, Discarded};
use rvw_git::{gitx, index, refs, stash};

/// The most actions the list keeps; the oldest falls off.
const MAX_ACTIONS: usize = 50;

/// How to reverse one action, with the state it left behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inverse {
    /// Stage or unstage: the index trees before and after.
    Index {
        before: String,
        after: String,
    },
    /// Commit or amend: HEAD before (None for the first commit) and after.
    Head {
        before: Option<String>,
        after: String,
    },
    /// A file's changes discarded.
    Discard(Discarded),
    /// A stash saved.
    StashPush {
        commit: String,
    },
    /// A stash dropped.
    StashDrop {
        commit: String,
        message: String,
    },
    BranchCreate {
        name: String,
        commit: String,
    },
    BranchDelete {
        name: String,
        commit: String,
    },
    BranchRename {
        from: String,
        to: String,
    },
    TagCreate {
        name: String,
        target: String,
    },
    TagDelete {
        name: String,
        target: String,
    },
    /// Switched from one branch to another.
    Switch {
        from: String,
        to: String,
    },
    /// A fast-forward pull.
    Pull {
        before: String,
        after: String,
    },
    /// Shown in the list but not undoable (push, fetch, stash apply).
    Final,
}

/// One entry of the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    /// What the list says, e.g. "Staged 2 lines of service.rs".
    pub label: String,
    pub at: SystemTime,
    pub inverse: Inverse,
}

/// The session's actions, oldest first, at most 50 (the oldest falls off).
#[derive(Debug, Clone, Default)]
pub struct Log {
    /// Oldest first.
    entries: Vec<Action>,
}

impl Log {
    pub fn push(&mut self, action: Action) {
        if self.entries.len() == MAX_ACTIONS {
            self.entries.remove(0);
        }
        self.entries.push(action);
    }

    /// Newest first, for the list.
    pub fn newest_first(&self) -> impl Iterator<Item = &Action> {
        self.entries.iter().rev()
    }

    /// Only the newest action can be undone, and only when it is not `Final`:
    /// undoing past a push would rewrite pushed history.
    pub fn can_undo(&self) -> bool {
        self.entries
            .last()
            .is_some_and(|action| !matches!(action.inverse, Inverse::Final))
    }

    /// Undoes the newest action in the worktree at `dir`. On success removes it
    /// and returns its label. On failure keeps it and returns the reason, a
    /// sentence ending in "; nothing was undone".
    pub fn undo(&mut self, dir: &Path) -> Result<String, String> {
        let Some(newest) = self.entries.pop() else {
            return Err("there is nothing to undo; nothing was undone".to_owned());
        };
        match reverse(dir, &newest.inverse) {
            Ok(()) => Ok(newest.label),
            Err(reason) => {
                self.entries.push(newest);
                Err(format!("{reason}; nothing was undone"))
            }
        }
    }
}

/// How long ago, for the list: "now" under a minute, then "2m", "41m", "1h",
/// "23h", "3d". A time in the future reads "now".
pub fn ago(now: SystemTime, at: SystemTime) -> String {
    let secs = now
        .duration_since(at)
        .map_or(0, |elapsed| elapsed.as_secs());
    match secs {
        0..=59 => "now".to_owned(),
        60..=3_599 => format!("{}m", secs / 60),
        3_600..=86_399 => format!("{}h", secs / 3_600),
        _ => format!("{}d", secs / 86_400),
    }
}

/// Reverses one action after checking its precondition.
pub fn reverse(dir: &Path, inverse: &Inverse) -> Result<(), String> {
    match inverse {
        Inverse::Index { before, after } => index_back(dir, before, after),
        Inverse::Head { before, after } => head_back(dir, before.as_deref(), after),
        Inverse::Discard(saved) => discard_back(dir, saved),
        Inverse::StashPush { commit } => stash::pop_top(dir, commit).map_err(|e| e.stderr),
        Inverse::StashDrop { commit, message } => {
            stash::store(dir, commit, message).map_err(|e| e.stderr)
        }
        Inverse::BranchCreate { name, commit } => branch_create_back(dir, name, commit),
        Inverse::BranchDelete { name, commit } => {
            refs::restore_branch(dir, name, commit).map_err(|e| e.stderr)
        }
        Inverse::BranchRename { from, to } => {
            refs::rename_branch(dir, to, from).map_err(|e| e.stderr)
        }
        Inverse::TagCreate { name, target } => tag_create_back(dir, name, target),
        Inverse::TagDelete { name, target } => {
            refs::restore_tag(dir, name, target).map_err(|e| e.stderr)
        }
        Inverse::Switch { from, to } => switch_back(dir, from, to),
        Inverse::Pull { before, after } => pull_back(dir, before, after),
        Inverse::Final => Err("this action cannot be undone".to_owned()),
    }
}

fn index_back(dir: &Path, before: &str, after: &str) -> Result<(), String> {
    let now = index::write_tree(dir).map_err(|e| e.stderr)?;
    if now != after {
        return Err("the index changed since".to_owned());
    }
    index::read_tree(dir, before).map_err(|e| e.stderr)
}

fn head_back(dir: &Path, before: Option<&str>, after: &str) -> Result<(), String> {
    if !head_is(dir, after) {
        return Err("HEAD moved since".to_owned());
    }
    let Some(before) = before else {
        return Err("the first commit cannot be undone".to_owned());
    };
    commits::reset_soft(dir, before).map_err(|e| e.stderr)
}

fn discard_back(dir: &Path, saved: &Discarded) -> Result<(), String> {
    if !left_by_discard(dir, saved) {
        return Err("the file changed since".to_owned());
    }
    commits::restore_discarded(dir, saved).map_err(|e| e.stderr)
}

/// Whether the file is still as `discard` left it: absent for an untracked
/// file, and with no unstaged change for a tracked one.
fn left_by_discard(dir: &Path, saved: &Discarded) -> bool {
    if saved.untracked {
        let full = dir.join(&saved.path);
        return matches!(fs::symlink_metadata(full), Err(e) if e.kind() == io::ErrorKind::NotFound);
    }
    read_only(
        dir,
        &[
            "--literal-pathspecs",
            "diff",
            "--quiet",
            "--",
            saved.path.as_str(),
        ],
    )
    .is_some()
}

fn branch_create_back(dir: &Path, name: &str, commit: &str) -> Result<(), String> {
    let full = format!("refs/heads/{name}");
    if gitx::commit(dir, &full).as_deref() != Some(commit) {
        return Err("the branch moved since".to_owned());
    }
    if gitx::branch(dir) == name {
        return Err("you are on that branch".to_owned());
    }
    refs::delete_branch(dir, name, true)
        .map(|_| ())
        .map_err(|e| e.stderr)
}

fn tag_create_back(dir: &Path, name: &str, target: &str) -> Result<(), String> {
    let full = format!("refs/tags/{name}");
    let now = read_only(dir, &["rev-parse", "--verify", "--quiet", full.as_str()]);
    if now.as_deref() != Some(target) {
        return Err("the tag moved since".to_owned());
    }
    refs::delete_tag(dir, name)
        .map(|_| ())
        .map_err(|e| e.stderr)
}

fn switch_back(dir: &Path, from: &str, to: &str) -> Result<(), String> {
    if gitx::branch(dir) != to {
        return Err("the branch changed since".to_owned());
    }
    gitx::switch(dir, from).map_err(|e| e.stderr)
}

fn pull_back(dir: &Path, before: &str, after: &str) -> Result<(), String> {
    if !head_is(dir, after) {
        return Err("HEAD moved since".to_owned());
    }
    commits::reset_keep(dir, before).map_err(|e| e.stderr)
}

/// Whether HEAD names commit `id`.
fn head_is(dir: &Path, id: &str) -> bool {
    commits::head(dir).as_deref() == Some(id)
}

/// Runs `git -C dir <args>` without taking locks, and returns its trimmed
/// stdout when git exits 0; None otherwise.
fn read_only(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rvw_git::index::{stage, write_tree};
    use std::time::{Duration, UNIX_EPOCH};
    use tempfile::TempDir;

    /// Runs git in `dir` and returns its stdout; panics on failure.
    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    /// A fresh repository on branch main with a local identity and no signing.
    fn repo() -> TempDir {
        let dir = TempDir::new().unwrap();
        git(dir.path(), &["init", "-q", "-b", "main"]);
        git(dir.path(), &["config", "user.email", "t@example.com"]);
        git(dir.path(), &["config", "user.name", "Test"]);
        git(dir.path(), &["config", "commit.gpgsign", "false"]);
        dir
    }

    /// A repository whose `a.txt` is committed as "a" on main.
    fn committed() -> TempDir {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "first");
        r
    }

    fn write(dir: &Path, rel: &str, content: &str) {
        fs::write(dir.join(rel), content).unwrap();
    }

    fn commit_all(dir: &Path, msg: &str) {
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", msg]);
    }

    fn rev_of(dir: &Path, rev: &str) -> String {
        git(dir, &["rev-parse", rev]).trim().to_owned()
    }

    fn action(label: &str, inverse: Inverse) -> Action {
        Action {
            label: label.to_owned(),
            at: UNIX_EPOCH,
            inverse,
        }
    }

    /// The labels of the list, newest first.
    fn labels(log: &Log) -> Vec<String> {
        log.newest_first().map(|a| a.label.clone()).collect()
    }

    #[test]
    fn ago_reads_the_time_in_coarse_steps() {
        let now = UNIX_EPOCH + Duration::from_secs(1_000_000_000);
        let at = |secs: u64| now - Duration::from_secs(secs);

        assert_eq!(ago(now, at(0)), "now");
        assert_eq!(ago(now, at(59)), "now");
        assert_eq!(ago(now, at(60)), "1m");
        assert_eq!(ago(now, at(41 * 60)), "41m");
        assert_eq!(ago(now, at(3_600)), "1h");
        assert_eq!(ago(now, at(23 * 3_600)), "23h");
        assert_eq!(ago(now, at(25 * 3_600)), "1d");
        assert_eq!(ago(now, at(3 * 86_400)), "3d");
        assert_eq!(ago(now, now + Duration::from_secs(10)), "now");
    }

    #[test]
    fn log_keeps_the_newest_fifty_actions() {
        let mut log = Log::default();
        for i in 0..51 {
            log.push(action(&format!("a{i}"), Inverse::Final));
        }

        let all = labels(&log);
        assert_eq!(all.len(), 50);
        assert_eq!(all.first().map(String::as_str), Some("a50"));
        assert_eq!(all.last().map(String::as_str), Some("a1"));
    }

    #[test]
    fn can_undo_is_false_when_empty_or_when_the_newest_is_final() {
        let mut log = Log::default();
        assert!(!log.can_undo());

        log.push(action(
            "switch",
            Inverse::Switch {
                from: "main".to_owned(),
                to: "topic".to_owned(),
            },
        ));
        assert!(log.can_undo());

        log.push(action("pushed", Inverse::Final));
        assert!(!log.can_undo());
    }

    #[test]
    fn undo_of_a_final_action_keeps_it() {
        let dir = repo();
        let mut log = Log::default();
        log.push(action("pushed", Inverse::Final));

        assert_eq!(
            log.undo(dir.path()),
            Err("this action cannot be undone; nothing was undone".to_owned())
        );
        assert_eq!(labels(&log), ["pushed"]);
    }

    #[test]
    fn undo_of_an_empty_log_says_there_is_nothing() {
        let dir = repo();
        let mut log = Log::default();
        assert_eq!(
            log.undo(dir.path()),
            Err("there is nothing to undo; nothing was undone".to_owned())
        );
    }

    #[test]
    fn index_undo_unstages_the_file_and_refuses_after_another_change() {
        let r = committed();
        let before = write_tree(r.path()).unwrap();
        write(r.path(), "b.txt", "b\n");
        stage(r.path(), &["b.txt"]).unwrap();
        let after = write_tree(r.path()).unwrap();
        let mut log = Log::default();
        log.push(action(
            "Staged b.txt",
            Inverse::Index {
                before: before.clone(),
                after: after.clone(),
            },
        ));

        assert_eq!(log.undo(r.path()), Ok("Staged b.txt".to_owned()));
        assert_eq!(git(r.path(), &["diff", "--cached", "--name-only"]), "");
        assert!(log.newest_first().next().is_none());
        assert!(r.path().join("b.txt").exists());

        write(r.path(), "c.txt", "c\n");
        stage(r.path(), &["b.txt", "c.txt"]).unwrap();
        log.push(action("Staged b.txt", Inverse::Index { before, after }));
        assert_eq!(
            log.undo(r.path()),
            Err("the index changed since; nothing was undone".to_owned())
        );
        assert_eq!(labels(&log), ["Staged b.txt"]);
        assert_eq!(
            git(r.path(), &["diff", "--cached", "--name-only"]),
            "b.txt\nc.txt\n"
        );
    }

    #[test]
    fn head_undo_moves_back_and_keeps_the_change_staged() {
        let r = committed();
        let before = rev_of(r.path(), "HEAD");
        write(r.path(), "b.txt", "b\n");
        git(r.path(), &["add", "--", "b.txt"]);
        let after = commits::commit(r.path(), "second").unwrap();
        let mut log = Log::default();
        log.push(action(
            "Committed second",
            Inverse::Head {
                before: Some(before.clone()),
                after,
            },
        ));

        assert_eq!(log.undo(r.path()), Ok("Committed second".to_owned()));
        assert_eq!(rev_of(r.path(), "HEAD"), before);
        assert_eq!(
            git(r.path(), &["diff", "--cached", "--name-only"]),
            "b.txt\n"
        );
    }

    #[test]
    fn head_undo_refuses_when_another_commit_came_after() {
        let r = committed();
        let before = rev_of(r.path(), "HEAD");
        write(r.path(), "b.txt", "b\n");
        git(r.path(), &["add", "--", "b.txt"]);
        let after = commits::commit(r.path(), "second").unwrap();
        write(r.path(), "c.txt", "c\n");
        git(r.path(), &["add", "--", "c.txt"]);
        let third = commits::commit(r.path(), "third").unwrap();
        let mut log = Log::default();
        log.push(action(
            "Committed second",
            Inverse::Head {
                before: Some(before),
                after,
            },
        ));

        assert_eq!(
            log.undo(r.path()),
            Err("HEAD moved since; nothing was undone".to_owned())
        );
        assert_eq!(rev_of(r.path(), "HEAD"), third);
        assert_eq!(labels(&log), ["Committed second"]);
    }

    #[test]
    fn head_undo_of_the_first_commit_is_refused() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        git(r.path(), &["add", "--", "a.txt"]);
        let first = commits::commit(r.path(), "first").unwrap();
        let mut log = Log::default();
        log.push(action(
            "Committed first",
            Inverse::Head {
                before: None,
                after: first,
            },
        ));

        assert_eq!(
            log.undo(r.path()),
            Err("the first commit cannot be undone; nothing was undone".to_owned())
        );
        assert_eq!(labels(&log), ["Committed first"]);
    }

    #[test]
    fn discard_undo_brings_the_change_back_and_refuses_after_an_edit() {
        let r = committed();
        write(r.path(), "a.txt", "changed\n");
        let saved = commits::discard(r.path(), "a.txt").unwrap();
        assert_eq!(fs::read_to_string(r.path().join("a.txt")).unwrap(), "a\n");
        let mut log = Log::default();
        log.push(action("Discarded a.txt", Inverse::Discard(saved.clone())));

        assert_eq!(log.undo(r.path()), Ok("Discarded a.txt".to_owned()));
        assert_eq!(
            fs::read_to_string(r.path().join("a.txt")).unwrap(),
            "changed\n"
        );

        let saved = commits::discard(r.path(), "a.txt").unwrap();
        write(r.path(), "a.txt", "edited again\n");
        log.push(action("Discarded a.txt", Inverse::Discard(saved)));
        assert_eq!(
            log.undo(r.path()),
            Err("the file changed since; nothing was undone".to_owned())
        );
        assert_eq!(
            fs::read_to_string(r.path().join("a.txt")).unwrap(),
            "edited again\n"
        );
        assert_eq!(labels(&log), ["Discarded a.txt"]);
    }

    #[test]
    fn discard_undo_of_an_untracked_file_recreates_it_and_refuses_when_it_is_back() {
        let r = committed();
        write(r.path(), "new.txt", "new\n");
        let saved = commits::discard(r.path(), "new.txt").unwrap();
        assert!(!r.path().join("new.txt").exists());
        let mut log = Log::default();
        log.push(action("Discarded new.txt", Inverse::Discard(saved)));

        assert_eq!(log.undo(r.path()), Ok("Discarded new.txt".to_owned()));
        assert_eq!(
            fs::read_to_string(r.path().join("new.txt")).unwrap(),
            "new\n"
        );

        let saved = commits::discard(r.path(), "new.txt").unwrap();
        write(r.path(), "new.txt", "again\n");
        log.push(action("Discarded new.txt", Inverse::Discard(saved)));
        assert_eq!(
            log.undo(r.path()),
            Err("the file changed since; nothing was undone".to_owned())
        );
        assert_eq!(
            fs::read_to_string(r.path().join("new.txt")).unwrap(),
            "again\n"
        );
    }

    #[test]
    fn stash_push_undo_pops_the_stash_back_into_the_worktree() {
        let r = committed();
        write(r.path(), "a.txt", "changed\n");
        let entry = stash::push(r.path(), "try", false).unwrap().unwrap();
        let mut log = Log::default();
        log.push(action(
            "Stashed try",
            Inverse::StashPush {
                commit: entry.commit,
            },
        ));

        assert_eq!(log.undo(r.path()), Ok("Stashed try".to_owned()));
        assert_eq!(
            fs::read_to_string(r.path().join("a.txt")).unwrap(),
            "changed\n"
        );
        assert!(stash::list(r.path()).unwrap().is_empty());
    }

    #[test]
    fn stash_drop_undo_puts_the_stash_back() {
        let r = committed();
        write(r.path(), "a.txt", "changed\n");
        let entry = stash::push(r.path(), "try", false).unwrap().unwrap();
        stash::drop(r.path(), &entry).unwrap();
        let mut log = Log::default();
        log.push(action(
            "Dropped stash",
            Inverse::StashDrop {
                commit: entry.commit.clone(),
                message: entry.message.clone(),
            },
        ));

        assert_eq!(log.undo(r.path()), Ok("Dropped stash".to_owned()));
        let back = stash::list(r.path()).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].commit, entry.commit);
        assert_eq!(back[0].message, entry.message);
    }

    #[test]
    fn branch_create_undo_deletes_the_branch_and_refuses_when_it_moved() {
        let r = committed();
        let commit = refs::create_branch(r.path(), "topic", None).unwrap();
        let mut log = Log::default();
        log.push(action(
            "Created topic",
            Inverse::BranchCreate {
                name: "topic".to_owned(),
                commit: commit.clone(),
            },
        ));

        git(r.path(), &["switch", "-q", "topic"]);
        write(r.path(), "b.txt", "b\n");
        commit_all(r.path(), "on topic");
        git(r.path(), &["switch", "-q", "main"]);
        assert_eq!(
            log.undo(r.path()),
            Err("the branch moved since; nothing was undone".to_owned())
        );
        assert!(git(r.path(), &["branch", "--list", "topic"]).contains("topic"));

        git(r.path(), &["branch", "-f", "topic", commit.as_str()]);
        assert_eq!(log.undo(r.path()), Ok("Created topic".to_owned()));
        assert_eq!(git(r.path(), &["branch", "--list", "topic"]), "");
    }

    #[test]
    fn branch_create_undo_refuses_when_on_that_branch() {
        let r = committed();
        let commit = refs::create_branch(r.path(), "topic", None).unwrap();
        git(r.path(), &["switch", "-q", "topic"]);
        let mut log = Log::default();
        log.push(action(
            "Created topic",
            Inverse::BranchCreate {
                name: "topic".to_owned(),
                commit,
            },
        ));

        assert_eq!(
            log.undo(r.path()),
            Err("you are on that branch; nothing was undone".to_owned())
        );
        assert_eq!(gitx::branch(r.path()), "topic");
    }

    #[test]
    fn branch_delete_undo_brings_the_branch_back() {
        let r = committed();
        let commit = refs::create_branch(r.path(), "topic", None).unwrap();
        refs::delete_branch(r.path(), "topic", false).unwrap();
        let mut log = Log::default();
        log.push(action(
            "Deleted topic",
            Inverse::BranchDelete {
                name: "topic".to_owned(),
                commit: commit.clone(),
            },
        ));

        assert_eq!(log.undo(r.path()), Ok("Deleted topic".to_owned()));
        assert_eq!(gitx::commit(r.path(), "refs/heads/topic"), Some(commit));
    }

    #[test]
    fn branch_rename_undo_renames_it_back() {
        let r = committed();
        refs::create_branch(r.path(), "old", None).unwrap();
        refs::rename_branch(r.path(), "old", "new").unwrap();
        let mut log = Log::default();
        log.push(action(
            "Renamed old to new",
            Inverse::BranchRename {
                from: "old".to_owned(),
                to: "new".to_owned(),
            },
        ));

        assert_eq!(log.undo(r.path()), Ok("Renamed old to new".to_owned()));
        let branches = git(r.path(), &["branch", "--list"]);
        assert!(
            branches.contains("old") && !branches.contains("new"),
            "{branches}"
        );
    }

    #[test]
    fn tag_create_undo_deletes_the_tag_and_refuses_when_it_moved() {
        let r = committed();
        let target = refs::create_tag(r.path(), "v1", "HEAD", "release one\n").unwrap();
        let mut log = Log::default();
        log.push(action(
            "Tagged v1",
            Inverse::TagCreate {
                name: "v1".to_owned(),
                target: target.clone(),
            },
        ));

        write(r.path(), "b.txt", "b\n");
        commit_all(r.path(), "second");
        git(r.path(), &["tag", "-f", "-a", "-m", "moved", "v1", "HEAD"]);
        assert_eq!(
            log.undo(r.path()),
            Err("the tag moved since; nothing was undone".to_owned())
        );
        assert_ne!(rev_of(r.path(), "refs/tags/v1"), target);

        git(r.path(), &["tag", "-d", "v1"]);
        let light = refs::create_tag(r.path(), "v1", "HEAD", "").unwrap();
        log.push(action(
            "Tagged v1",
            Inverse::TagCreate {
                name: "v1".to_owned(),
                target: light,
            },
        ));
        assert_eq!(log.undo(r.path()), Ok("Tagged v1".to_owned()));
        assert_eq!(git(r.path(), &["tag", "--list"]), "");
    }

    #[test]
    fn tag_delete_undo_restores_an_annotated_tag() {
        let r = committed();
        let target = refs::create_tag(r.path(), "v1", "HEAD", "release one\n").unwrap();
        refs::delete_tag(r.path(), "v1").unwrap();
        let mut log = Log::default();
        log.push(action(
            "Deleted v1",
            Inverse::TagDelete {
                name: "v1".to_owned(),
                target: target.clone(),
            },
        ));

        assert_eq!(log.undo(r.path()), Ok("Deleted v1".to_owned()));
        assert_eq!(rev_of(r.path(), "refs/tags/v1"), target);
        assert_eq!(
            git(r.path(), &["cat-file", "-t", "refs/tags/v1"]).trim(),
            "tag"
        );
    }

    #[test]
    fn switch_undo_goes_back_and_refuses_on_a_third_branch() {
        let r = committed();
        refs::create_branch(r.path(), "topic", None).unwrap();
        gitx::switch(r.path(), "topic").unwrap();
        let mut log = Log::default();
        log.push(action(
            "Switched to topic",
            Inverse::Switch {
                from: "main".to_owned(),
                to: "topic".to_owned(),
            },
        ));

        assert_eq!(log.undo(r.path()), Ok("Switched to topic".to_owned()));
        assert_eq!(gitx::branch(r.path()), "main");

        refs::create_branch(r.path(), "third", None).unwrap();
        gitx::switch(r.path(), "topic").unwrap();
        gitx::switch(r.path(), "third").unwrap();
        log.push(action(
            "Switched to topic",
            Inverse::Switch {
                from: "main".to_owned(),
                to: "topic".to_owned(),
            },
        ));
        assert_eq!(
            log.undo(r.path()),
            Err("the branch changed since; nothing was undone".to_owned())
        );
        assert_eq!(gitx::branch(r.path()), "third");
    }

    #[test]
    fn pull_undo_moves_head_back_and_refuses_when_head_is_not_the_pulled_commit() {
        let r = committed();
        let before = rev_of(r.path(), "HEAD");
        write(r.path(), "b.txt", "b\n");
        commit_all(r.path(), "pulled");
        let after = rev_of(r.path(), "HEAD");
        let mut log = Log::default();
        log.push(action(
            "Pulled",
            Inverse::Pull {
                before: before.clone(),
                after: after.clone(),
            },
        ));

        assert_eq!(log.undo(r.path()), Ok("Pulled".to_owned()));
        assert_eq!(rev_of(r.path(), "HEAD"), before);

        write(r.path(), "c.txt", "c\n");
        commit_all(r.path(), "later");
        let later = rev_of(r.path(), "HEAD");
        log.push(action("Pulled", Inverse::Pull { before, after }));
        assert_eq!(
            log.undo(r.path()),
            Err("HEAD moved since; nothing was undone".to_owned())
        );
        assert_eq!(rev_of(r.path(), "HEAD"), later);
        assert_eq!(labels(&log), ["Pulled"]);
    }
}
