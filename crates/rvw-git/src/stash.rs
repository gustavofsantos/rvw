//! Stashes: list, save, apply and drop them, and put a dropped one back.

use std::path::Path;

use crate::gitx::{self, GitError, owned_args, run};

/// One stash, newest first in a list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StashEntry {
    /// Its place in the list: 0 is `stash@{0}`.
    pub index: usize,
    /// The stash commit's id; it identifies the stash when the list moves.
    pub commit: String,
    /// git's subject, e.g. "On main: try the parser".
    pub message: String,
}

/// Every stash, newest first; empty when there is none.
pub fn list(dir: &Path) -> Result<Vec<StashEntry>, GitError> {
    let out = run(dir, &[], &["stash", "list", "--format=%H%x00%gs"])?;
    Ok(out
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let (commit, message) = line.split_once('\0')?;
            Some(StashEntry {
                index,
                commit: commit.to_owned(),
                message: message.to_owned(),
            })
        })
        .collect())
}

/// Saves the worktree's changes as a stash, with `message` when it is not blank.
/// Returns the new top stash, or None when there was nothing to save.
pub fn push(
    dir: &Path,
    message: &str,
    include_untracked: bool,
) -> Result<Option<StashEntry>, GitError> {
    let before = top_commit(dir)?;
    let message = message.trim();
    let mut args = vec!["stash", "push", "-q"];
    if include_untracked {
        args.push("--include-untracked");
    }
    if !message.is_empty() {
        args.push("-m");
        args.push(message);
    }
    run(dir, &[], &args)?;
    if top_commit(dir)? == before {
        return Ok(None);
    }
    Ok(list(dir)?.into_iter().next())
}

/// Applies a stash by its commit, keeping it in the list. The index is restored too when git can.
pub fn apply(dir: &Path, entry: &StashEntry) -> Result<(), GitError> {
    position(dir, &entry.commit, &["stash", "apply"])?;
    with_index(dir, "apply", Some(&entry.commit))
}

/// Removes a stash from the list by its commit.
pub fn drop(dir: &Path, entry: &StashEntry) -> Result<(), GitError> {
    let index = position(dir, &entry.commit, &["stash", "drop"])?;
    let target = format!("stash@{{{index}}}");
    run(dir, &[], &["stash", "drop", "-q", &target]).map(|_| ())
}

/// Puts a dropped stash back on top of the list; this undoes a drop.
pub fn store(dir: &Path, commit: &str, message: &str) -> Result<(), GitError> {
    if commit.starts_with('-') {
        return Err(GitError {
            args: owned_args(&["stash", "store"]),
            stderr: format!("invalid commit '{commit}'"),
        });
    }
    run(dir, &[], &["stash", "store", "-m", message, commit]).map(|_| ())
}

/// The commit `refs/stash` points at; None when there is no stash.
fn top_commit(dir: &Path) -> Result<Option<String>, GitError> {
    let ran = gitx::exec(dir, &[], &["rev-parse", "-q", "--verify", "refs/stash"])?;
    Ok((ran.code == Some(0)).then(|| ran.stdout.trim().to_owned()))
}

/// The current index of the stash whose commit is `commit`. A stash that is
/// no longer listed is an error, since its index can no longer be found.
fn position(dir: &Path, commit: &str, op: &[&str]) -> Result<usize, GitError> {
    list(dir)?
        .into_iter()
        .position(|entry| entry.commit == commit)
        .ok_or_else(|| GitError {
            args: owned_args(op),
            stderr: "the stash is gone".to_owned(),
        })
}

/// Runs `git stash <verb> -q --index [rev]`. When git refuses because the
/// index cannot be restored (its message names `--index`), runs it once more
/// without `--index`.
fn with_index(dir: &Path, verb: &str, rev: Option<&str>) -> Result<(), GitError> {
    let mut args = vec!["stash", verb, "-q", "--index"];
    args.extend(rev);
    match run(dir, &[], &args) {
        Err(err) if err.stderr.contains("--index") => {
            let mut plain = vec!["stash", verb, "-q"];
            plain.extend(rev);
            run(dir, &[], &plain).map(|_| ())
        }
        result => result.map(|_| ()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testrepo::{commit_all, git, repo, write};
    use std::fs;
    use tempfile::TempDir;

    /// A repository whose `a.txt` is committed as "a".
    fn committed() -> TempDir {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "init");
        r
    }

    #[test]
    fn list_is_empty_in_a_fresh_repo_with_a_commit() {
        let r = committed();
        assert!(list(r.path()).unwrap().is_empty());
    }

    #[test]
    fn push_saves_a_tracked_change_and_cleans_the_worktree() {
        let r = committed();
        write(r.path(), "a.txt", "changed\n");

        let entry = push(r.path(), "try the parser", false).unwrap().unwrap();
        assert_eq!(entry.index, 0);
        assert_eq!(git(r.path(), &["status", "--porcelain"]), "");
        let all = list(r.path()).unwrap();
        assert_eq!(all.len(), 1);
        assert!(all[0].message.contains("try the parser"));
        assert_eq!(all[0], entry);

        assert_eq!(push(r.path(), "", false).unwrap(), None);
        assert_eq!(list(r.path()).unwrap().len(), 1);
    }

    #[test]
    fn push_with_include_untracked_takes_the_new_file_too() {
        let r = committed();
        write(r.path(), "a.txt", "changed\n");
        write(r.path(), "new.txt", "new\n");

        let entry = push(r.path(), "", true).unwrap().unwrap();
        assert!(!r.path().join("new.txt").exists());
        assert_eq!(
            git(r.path(), &["show", &format!("{}^3:new.txt", entry.commit)]),
            "new\n"
        );
    }

    #[test]
    fn push_without_include_untracked_leaves_the_new_file() {
        let r = committed();
        write(r.path(), "a.txt", "changed\n");
        write(r.path(), "new.txt", "new\n");

        assert!(push(r.path(), "keep new", false).unwrap().is_some());
        assert!(r.path().join("new.txt").exists());
        assert_eq!(git(r.path(), &["status", "--porcelain"]), "?? new.txt\n");
    }

    #[test]
    fn apply_restores_the_change_and_keeps_the_stash() {
        let r = committed();
        write(r.path(), "a.txt", "one\n");
        git(r.path(), &["add", "--", "a.txt"]);
        write(r.path(), "a.txt", "two\n");
        let entry = push(r.path(), "staged", false).unwrap().unwrap();

        apply(r.path(), &entry).unwrap();
        assert_eq!(fs::read_to_string(r.path().join("a.txt")).unwrap(), "two\n");
        assert_eq!(
            git(r.path(), &["diff", "--cached", "--name-only"]),
            "a.txt\n"
        );
        assert_eq!(git(r.path(), &["show", ":a.txt"]), "one\n");
        assert_eq!(list(r.path()).unwrap(), vec![entry]);
    }

    #[test]
    fn apply_retries_without_index_when_head_moved_under_the_staged_patch() {
        let r = repo();
        let text = "line 1\nline 2\nline 3\nline 4\nline 5\nline 6\nline 7\nline 8\n";
        write(r.path(), "f.txt", text);
        commit_all(r.path(), "init");
        write(
            r.path(),
            "f.txt",
            &text.replacen("line 1\n", "staged 1\n", 1),
        );
        git(r.path(), &["add", "--", "f.txt"]);
        let entry = push(r.path(), "staged head", false).unwrap().unwrap();

        // HEAD moves to a line within the staged hunk's context, so the
        // staged patch no longer applies to the index.
        write(
            r.path(),
            "f.txt",
            &text.replacen("line 4\n", "moved 4\n", 1),
        );
        commit_all(r.path(), "move line 4");

        apply(r.path(), &entry).unwrap();
        assert_eq!(
            fs::read_to_string(r.path().join("f.txt")).unwrap(),
            text.replacen("line 1\n", "staged 1\n", 1)
                .replacen("line 4\n", "moved 4\n", 1)
        );
        assert_eq!(git(r.path(), &["diff", "--cached", "--name-only"]), "");
        assert_eq!(list(r.path()).unwrap(), vec![entry]);
    }

    #[test]
    fn drop_removes_the_older_stash_by_its_commit_after_a_newer_push() {
        let r = committed();
        write(r.path(), "a.txt", "first\n");
        let older = push(r.path(), "older", false).unwrap().unwrap();
        write(r.path(), "a.txt", "second\n");
        let newer = push(r.path(), "newer", false).unwrap().unwrap();
        // The older entry was index 0 when saved; it has moved to index 1.
        assert_eq!(older.index, 0);
        assert_eq!(newer.index, 0);

        drop(r.path(), &older).unwrap();
        assert_eq!(list(r.path()).unwrap(), vec![newer]);

        let err = drop(r.path(), &older).unwrap_err();
        assert_eq!(err.stderr, "the stash is gone");
        let err = apply(r.path(), &older).unwrap_err();
        assert_eq!(err.stderr, "the stash is gone");
    }

    #[test]
    fn store_puts_a_dropped_stash_back() {
        let r = committed();
        write(r.path(), "a.txt", "first\n");
        let older = push(r.path(), "older", false).unwrap().unwrap();
        drop(r.path(), &older).unwrap();
        assert!(list(r.path()).unwrap().is_empty());

        store(r.path(), &older.commit, "older").unwrap();
        let all = list(r.path()).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].commit, older.commit);
        assert!(all[0].message.contains("older"));
    }

    #[test]
    fn store_rejects_a_commit_that_looks_like_an_option() {
        let r = committed();
        let err = store(r.path(), "--bogus", "m").unwrap_err();
        assert_eq!(err.stderr, "invalid commit '--bogus'");
    }
}
