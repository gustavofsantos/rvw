//! Commits and the worktree: commit, amend, move HEAD back, and discard a file's
//! changes so they can be put back.

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use crate::gitx::{self, os_error_text, owned_args, run, store_file, GitError};

/// The commit HEAD points at; None on a branch with no commit yet.
pub fn head(dir: &Path) -> Option<String> {
    gitx::commit(dir, "HEAD")
}

/// Commits the index with `message`; returns the new HEAD.
pub fn commit(dir: &Path, message: &str) -> Result<String, GitError> {
    record(
        dir,
        message,
        &["commit", "-q", "--cleanup=strip", "-F", "-"],
    )
}

/// Replaces the last commit with the index and `message`; returns the new HEAD.
pub fn amend(dir: &Path, message: &str) -> Result<String, GitError> {
    record(
        dir,
        message,
        &["commit", "-q", "--amend", "--cleanup=strip", "-F", "-"],
    )
}

/// Runs a `git commit` that reads its message from stdin, then returns HEAD.
fn record(dir: &Path, message: &str, args: &[&str]) -> Result<String, GitError> {
    let message = message.trim_end();
    if message.trim().is_empty() {
        return Err(GitError {
            args: owned_args(args),
            stderr: "empty commit message".to_owned(),
        });
    }
    let ran = gitx::exec(dir, message.as_bytes(), args)?;
    if ran.code != Some(0) {
        // git says "nothing to commit" on stdout, so fall back to its last
        // line: a failure is shown as one line.
        let stderr = if ran.stderr.is_empty() {
            ran.stdout
                .trim()
                .lines()
                .last()
                .unwrap_or_default()
                .to_owned()
        } else {
            ran.stderr
        };
        return Err(GitError {
            args: owned_args(args),
            stderr,
        });
    }
    head(dir).ok_or_else(|| GitError {
        args: owned_args(args),
        stderr: "no commit after git commit".to_owned(),
    })
}

/// The full message of HEAD's commit.
pub fn head_message(dir: &Path) -> Result<String, GitError> {
    let out = run(dir, &[], &["log", "-1", "--format=%B"])?;
    Ok(out.trim_end_matches('\n').to_owned())
}

/// Whether HEAD's commit is on any remote-tracking branch.
pub fn head_pushed(dir: &Path) -> Result<bool, GitError> {
    if head(dir).is_none() {
        return Ok(false);
    }
    let out = run(
        dir,
        &[],
        &[
            "for-each-ref",
            "--contains",
            "HEAD",
            "--format=%(refname)",
            "refs/remotes",
        ],
    )?;
    Ok(!out.trim().is_empty())
}

/// Moves the current branch to `rev`, keeping the index and the worktree.
pub fn reset_soft(dir: &Path, rev: &str) -> Result<(), GitError> {
    if rev.starts_with('-') {
        return Err(GitError {
            args: owned_args(&["reset", "--soft", rev]),
            stderr: format!("invalid revision '{rev}'"),
        });
    }
    run(dir, &[], &["reset", "-q", "--soft", rev]).map(|_| ())
}

/// A file's worktree state saved before its changes were discarded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discarded {
    pub path: String,
    /// The blob holding the file as it was; None when the file did not exist in the
    /// worktree (it was deleted).
    pub blob: Option<String>,
    /// The file was executable.
    pub executable: bool,
    /// The file was untracked, so discarding removed it.
    pub untracked: bool,
}

/// Saves the file as a blob, then puts the worktree file back to its staged version
/// (or removes it when untracked).
pub fn discard(dir: &Path, path: &str) -> Result<Discarded, GitError> {
    let untracked = !gitx::tracked(dir, path).unwrap_or(false);
    let full = dir.join(path);
    let (blob, executable) = match fs::symlink_metadata(&full) {
        Ok(meta) => (
            Some(store_file(dir, path)?),
            (meta.permissions().mode() & 0o111) != 0,
        ),
        Err(e) if e.kind() == io::ErrorKind::NotFound => (None, false),
        Err(e) => return Err(fs_error(&["discard", path], &e)),
    };
    if untracked {
        if blob.is_some() {
            fs::remove_file(&full).map_err(|e| fs_error(&["discard", path], &e))?;
        }
    } else {
        run(
            dir,
            &[],
            &["--literal-pathspecs", "restore", "--worktree", "--", path],
        )?;
    }
    Ok(Discarded {
        path: path.to_owned(),
        blob,
        executable,
        untracked,
    })
}

/// Writes a discarded file back exactly as it was saved.
pub fn restore_discarded(dir: &Path, saved: &Discarded) -> Result<(), GitError> {
    let full = dir.join(&saved.path);
    let op = ["restore", saved.path.as_str()];
    let Some(id) = saved.blob.as_deref() else {
        return match fs::remove_file(&full) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(fs_error(&op, &e)),
        };
    };
    let bytes = blob_bytes(dir, id)?;
    if let Some(parent) = full.parent() {
        fs::create_dir_all(parent).map_err(|e| fs_error(&op, &e))?;
    }
    fs::write(&full, bytes).map_err(|e| fs_error(&op, &e))?;
    let mode = if saved.executable { 0o755 } else { 0o644 };
    fs::set_permissions(&full, fs::Permissions::from_mode(mode)).map_err(|e| fs_error(&op, &e))
}

/// The exact bytes of a blob. `run` decodes UTF-8 lossily, so this reads the raw output.
fn blob_bytes(dir: &Path, id: &str) -> Result<Vec<u8>, GitError> {
    let args = ["cat-file", "blob", id];
    let out = Command::new("git")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| GitError {
            args: owned_args(&args),
            stderr: os_error_text(&e),
        })?;
    if out.status.success() {
        Ok(out.stdout)
    } else {
        Err(GitError {
            args: owned_args(&args),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        })
    }
}

/// A file-system failure as a `GitError` for the operation `op` on a path.
fn fs_error(op: &[&str], e: &io::Error) -> GitError {
    GitError {
        args: owned_args(op),
        stderr: os_error_text(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testrepo::{commit_all, git, repo, write};
    use tempfile::TempDir;

    #[test]
    fn commit_records_the_index_as_head_with_its_message() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        git(r.path(), &["add", "--", "a.txt"]);

        let id = commit(r.path(), "first\n\n").unwrap();
        assert_eq!(head(r.path()), Some(id));
        assert_eq!(head_message(r.path()).unwrap(), "first");
    }

    #[test]
    fn commit_keeps_a_multi_line_message() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        git(r.path(), &["add", "--", "a.txt"]);

        let msg = "Subject line\n\nFirst paragraph.\n\n- a bullet";
        commit(r.path(), msg).unwrap();
        assert_eq!(head_message(r.path()).unwrap(), msg);
    }

    #[test]
    fn commit_rejects_an_empty_message_and_leaves_head() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "init");
        let first = head(r.path());
        write(r.path(), "b.txt", "b\n");
        git(r.path(), &["add", "--", "b.txt"]);

        let err = commit(r.path(), " \n\t\n").unwrap_err();
        assert_eq!(err.stderr, "empty commit message");
        assert_eq!(head(r.path()), first);
    }

    #[test]
    fn commit_with_nothing_staged_is_an_error_and_leaves_head() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "init");
        let first = head(r.path());

        let err = commit(r.path(), "nothing new").unwrap_err();
        assert!(err.stderr.contains("nothing to commit"), "{err}");
        assert_eq!(head(r.path()), first);
    }

    #[test]
    fn amend_replaces_the_message_and_keeps_the_parent() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "first");
        write(r.path(), "b.txt", "b\n");
        commit_all(r.path(), "second");
        let parent = git(r.path(), &["rev-parse", "HEAD~1"]).trim().to_owned();
        let old = head(r.path()).unwrap();

        write(r.path(), "c.txt", "c\n");
        git(r.path(), &["add", "--", "c.txt"]);
        let new = amend(r.path(), "second, amended\n\nWith a body.").unwrap();

        assert_ne!(new, old);
        assert_eq!(head(r.path()), Some(new.clone()));
        assert_eq!(git(r.path(), &["rev-parse", "HEAD~1"]).trim(), parent);
        assert_eq!(
            head_message(r.path()).unwrap(),
            "second, amended\n\nWith a body."
        );
        assert_eq!(
            git(r.path(), &["show", "--name-only", "--format=", "HEAD"]).trim(),
            "b.txt\nc.txt"
        );

        assert!(amend(r.path(), " ").is_err());
        assert_eq!(head(r.path()), Some(new));
    }

    #[test]
    fn reset_soft_moves_head_back_and_keeps_the_change_staged() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "first");
        let first = head(r.path()).unwrap();
        write(r.path(), "b.txt", "b\n");
        commit_all(r.path(), "second");

        reset_soft(r.path(), &first).unwrap();
        assert_eq!(head(r.path()), Some(first));
        assert_eq!(
            git(r.path(), &["diff", "--cached", "--name-only"]),
            "b.txt\n"
        );
    }

    #[test]
    fn reset_soft_rejects_an_option_like_rev() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "first");
        let before = head(r.path());

        let err = reset_soft(r.path(), "--hard").unwrap_err();
        assert_eq!(err.args, vec!["reset", "--soft", "--hard"]);
        assert_eq!(head(r.path()), before);
    }

    #[test]
    fn head_is_none_without_a_commit() {
        let r = repo();
        assert_eq!(head(r.path()), None);
        assert_eq!(head_pushed(r.path()), Ok(false));
    }

    #[test]
    fn head_pushed_follows_the_remote_tracking_refs() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "init");
        assert_eq!(head_pushed(r.path()), Ok(false));

        let remote = TempDir::new().unwrap();
        git(remote.path(), &["init", "-q", "--bare"]);
        let url = remote.path().to_str().unwrap();
        git(r.path(), &["remote", "add", "origin", url]);
        git(r.path(), &["push", "-q", "origin", "main"]);
        assert_eq!(head_pushed(r.path()), Ok(true));

        write(r.path(), "b.txt", "b\n");
        commit_all(r.path(), "second");
        assert_eq!(head_pushed(r.path()), Ok(false));
    }

    #[test]
    fn discard_saves_a_modified_binary_file_and_keeps_the_staged_version() {
        let r = repo();
        fs::write(r.path().join("bin.dat"), [b'a', 0, 0xff, b'\n']).unwrap();
        commit_all(r.path(), "init");

        let staged = [0u8, 0xfe, b'\n', 0xc3];
        let worktree = [0u8, 0xff, 0xff, b'x', 0];
        fs::write(r.path().join("bin.dat"), staged).unwrap();
        git(r.path(), &["add", "--", "bin.dat"]);
        fs::write(r.path().join("bin.dat"), worktree).unwrap();

        let saved = discard(r.path(), "bin.dat").unwrap();
        assert!(saved.blob.is_some());
        assert!(!saved.executable);
        assert!(!saved.untracked);
        assert_eq!(fs::read(r.path().join("bin.dat")).unwrap(), staged);
        assert_eq!(
            git(r.path(), &["diff", "--cached", "--name-only"]),
            "bin.dat\n"
        );

        restore_discarded(r.path(), &saved).unwrap();
        assert_eq!(fs::read(r.path().join("bin.dat")).unwrap(), worktree);
        assert_eq!(
            git(r.path(), &["diff", "--cached", "--name-only"]),
            "bin.dat\n"
        );
    }

    #[test]
    fn discard_brings_back_a_deleted_file_and_restore_deletes_it_again() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "init");
        fs::remove_file(r.path().join("a.txt")).unwrap();

        let saved = discard(r.path(), "a.txt").unwrap();
        assert_eq!(saved.blob, None);
        assert!(!saved.untracked);
        assert_eq!(fs::read_to_string(r.path().join("a.txt")).unwrap(), "a\n");

        restore_discarded(r.path(), &saved).unwrap();
        assert!(!r.path().join("a.txt").exists());
    }

    #[test]
    fn discard_removes_an_untracked_file_and_restore_recreates_it() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "init");
        write(r.path(), "sub/new.txt", "new\n");

        let saved = discard(r.path(), "sub/new.txt").unwrap();
        assert!(saved.untracked);
        assert!(saved.blob.is_some());
        assert!(!r.path().join("sub/new.txt").exists());

        fs::remove_dir(r.path().join("sub")).unwrap();
        restore_discarded(r.path(), &saved).unwrap();
        assert_eq!(
            fs::read_to_string(r.path().join("sub/new.txt")).unwrap(),
            "new\n"
        );
        assert!(!saved.executable);
    }

    #[test]
    fn discard_and_restore_keep_the_executable_bit() {
        let r = repo();
        write(r.path(), "run.sh", "#!/bin/sh\necho hi\n");
        fs::set_permissions(r.path().join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
        commit_all(r.path(), "init");
        write(r.path(), "run.sh", "#!/bin/sh\necho changed\n");

        let saved = discard(r.path(), "run.sh").unwrap();
        assert!(saved.executable);
        assert_eq!(
            fs::read_to_string(r.path().join("run.sh")).unwrap(),
            "#!/bin/sh\necho hi\n"
        );

        restore_discarded(r.path(), &saved).unwrap();
        assert_eq!(
            fs::read_to_string(r.path().join("run.sh")).unwrap(),
            "#!/bin/sh\necho changed\n"
        );
        let mode = fs::metadata(r.path().join("run.sh"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o755);
    }
}
