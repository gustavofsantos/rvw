//! Commits and the worktree: commit, amend, and discard a file's changes.

use std::fs;
use std::io;
use std::path::Path;

use crate::gitx::{self, os_error_text, owned_args, run, GitError};

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

/// Puts a file's worktree state back to its staged version, or removes it when it
/// is untracked. The content it drops is not kept here: take a checkpoint first.
pub fn discard(dir: &Path, path: &str) -> Result<(), GitError> {
    let untracked = !gitx::tracked(dir, path).unwrap_or(false);
    if untracked {
        return match fs::remove_file(dir.join(path)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(fs_error(&["discard", path], &e)),
        };
    }
    run(
        dir,
        &[],
        &["--literal-pathspecs", "restore", "--worktree", "--", path],
    )
    .map(|_| ())
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
    fn discard_puts_a_modified_binary_file_back_to_its_staged_version() {
        let r = repo();
        fs::write(r.path().join("bin.dat"), [b'a', 0, 0xff, b'\n']).unwrap();
        commit_all(r.path(), "init");

        let staged = [0u8, 0xfe, b'\n', 0xc3];
        fs::write(r.path().join("bin.dat"), staged).unwrap();
        git(r.path(), &["add", "--", "bin.dat"]);
        fs::write(r.path().join("bin.dat"), [0u8, 0xff, 0xff, b'x', 0]).unwrap();

        discard(r.path(), "bin.dat").unwrap();
        assert_eq!(fs::read(r.path().join("bin.dat")).unwrap(), staged);
        assert_eq!(git(r.path(), &["diff", "--name-only"]), "");
        assert_eq!(
            git(r.path(), &["diff", "--cached", "--name-only"]),
            "bin.dat\n"
        );
    }

    #[test]
    fn discard_brings_back_a_deleted_file() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "init");
        fs::remove_file(r.path().join("a.txt")).unwrap();

        discard(r.path(), "a.txt").unwrap();
        assert_eq!(fs::read_to_string(r.path().join("a.txt")).unwrap(), "a\n");
    }

    #[test]
    fn discard_removes_an_untracked_file() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "init");
        write(r.path(), "sub/new.txt", "new\n");

        discard(r.path(), "sub/new.txt").unwrap();
        assert!(!r.path().join("sub/new.txt").exists());
    }

    #[test]
    fn discard_keeps_the_executable_bit() {
        use std::os::unix::fs::PermissionsExt;

        let r = repo();
        write(r.path(), "run.sh", "#!/bin/sh\necho hi\n");
        fs::set_permissions(r.path().join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
        commit_all(r.path(), "init");
        write(r.path(), "run.sh", "#!/bin/sh\necho changed\n");

        discard(r.path(), "run.sh").unwrap();
        assert_eq!(
            fs::read_to_string(r.path().join("run.sh")).unwrap(),
            "#!/bin/sh\necho hi\n"
        );
        let mode = fs::metadata(r.path().join("run.sh"))
            .unwrap()
            .permissions()
            .mode();
        // git writes the file with the umask applied, so only the bit is checked.
        assert_ne!(mode & 0o100, 0);
    }
}
