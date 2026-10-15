//! Patch ids of one file's change, so that the same edit made in two commits,
//! or a commit and a blob pair, compares equal. Shells out to `git` like the
//! rest of the crate.

use std::fmt::Write as _;
use std::path::Path;

use crate::gitx::{self, GitError, exec, owned_args, run};

/// The blob id of `path` at commit `rev` (`git rev-parse rev:path`); None when
/// the path does not exist there or names a directory. Fails when `rev` does
/// not name a commit.
pub fn blob_at(dir: &Path, rev: &str, path: &str) -> Result<Option<String>, GitError> {
    let commit = resolve(dir, "rev-parse", rev)?;
    check_path("rev-parse", path)?;
    // The commit id is spelled out in full, so `rev` cannot add a `:` of its own.
    let spec = format!("{commit}:{path}");
    let ran = exec(
        dir,
        &[],
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &spec,
        ],
    )?;
    match ran.code {
        Some(0) => {}
        // A missing path exits 1 with no message.
        Some(1) => return Ok(None),
        _ => {
            return Err(GitError {
                args: owned_args(&["rev-parse", &spec]),
                stderr: ran.stderr,
            });
        }
    }
    let id = ran.stdout.trim().to_owned();
    // `rev:dir` names a tree; only a blob is a file's blob id.
    let kind = run(dir, &[], &["cat-file", "-t", &id])?;
    Ok((kind.trim() == "blob").then_some(id))
}

/// The patch-id of the change commit `commit` makes to `path`, against its
/// first parent (or the empty tree for a root commit). None when the commit
/// does not touch `path`.
pub fn commit_file_patch_id(
    dir: &Path,
    commit: &str,
    path: &str,
) -> Result<Option<String>, GitError> {
    let id = resolve(dir, "diff-tree", commit)?;
    check_path("diff-tree", path)?;
    let mut args = vec![
        "--literal-pathspecs",
        "diff-tree",
        "-p",
        "-r",
        "--no-renames",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--no-commit-id",
        "--src-prefix=a/",
        "--dst-prefix=b/",
    ];
    // A merge is compared with its first parent. A root commit has none, so
    // `--root` diffs it against the empty tree.
    let parent = gitx::commit(dir, &format!("{id}^1"));
    let parent_id;
    match &parent {
        Some(p) => {
            parent_id = p.clone();
            args.extend([parent_id.as_str(), id.as_str()]);
        }
        None => args.extend(["--root", id.as_str()]),
    }
    args.extend(["--", path]);
    let diff = run(dir, &[], &args)?;
    patch_id_of(dir, &diff)
}

/// The patch-id of the change from blob `old_blob` to blob `new_blob`, with the
/// diff labelled as `path`, so it equals [`commit_file_patch_id`] for the same
/// change. None when the blobs are equal.
///
/// A blob carries no mode, so the change is taken as a regular file (100644).
/// An empty side reads as an added or deleted file, as git labels it.
pub fn blobs_patch_id(
    dir: &Path,
    old_blob: &str,
    new_blob: &str,
    path: &str,
) -> Result<Option<String>, GitError> {
    check_rev("diff", old_blob)?;
    check_rev("diff", new_blob)?;
    check_path("diff", path)?;
    let raw = run(
        dir,
        &[],
        &[
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            old_blob,
            new_blob,
            "--",
        ],
    )?;
    patch_id_of(dir, &relabel(&raw, path))
}

/// The patch-id git computes for a diff, or None for an empty diff.
fn patch_id_of(dir: &Path, diff: &str) -> Result<Option<String>, GitError> {
    if diff.is_empty() {
        return Ok(None);
    }
    // `git patch-id` prints `<patch-id> <commit-id>`; only the first field is
    // the id, and the second is absent or zeros for a bare diff.
    let out = run(dir, diff.as_bytes(), &["patch-id", "--stable"])?;
    Ok(out.split_whitespace().next().map(str::to_owned))
}

/// Rewrites the header of a one-file diff so its labels name `path`: the
/// `diff --git`, `---` and `+++` lines before the first hunk, and adds the mode
/// line of an absent side. Hunk lines are not touched.
fn relabel(diff: &str, path: &str) -> String {
    // A side with no lines is an absent file, labelled `/dev/null` as git
    // labels a commit's added or deleted file. The first hunk says which side is
    // empty: `-0,0` for the old side, `+0,0` for the new one.
    let first_hunk = diff.lines().find(|line| line.starts_with("@@ "));
    let old_label = if first_hunk.is_some_and(|h| h.starts_with("@@ -0,0 ")) {
        "/dev/null".to_owned()
    } else {
        format!("a/{path}")
    };
    let new_label = if first_hunk.is_some_and(|h| h.contains(" +0,0 @@")) {
        "/dev/null".to_owned()
    } else {
        format!("b/{path}")
    };
    let mut out = String::with_capacity(diff.len());
    let mut in_header = true;
    for line in diff.split_inclusive('\n') {
        if line.starts_with("@@") {
            in_header = false;
        }
        let body = line.trim_end_matches('\n');
        if in_header && body.starts_with("diff --git ") {
            let _ = writeln!(out, "diff --git a/{path} b/{path}");
            // Patch ids count the mode line of an added or deleted file, which a
            // blob diff lacks. A blob has no mode here, so a regular file is assumed.
            if old_label == "/dev/null" {
                out.push_str("new file mode 100644\n");
            } else if new_label == "/dev/null" {
                out.push_str("deleted file mode 100644\n");
            }
        } else if in_header && body.starts_with("--- ") {
            let _ = writeln!(out, "--- {old_label}");
        } else if in_header && body.starts_with("+++ ") {
            let _ = writeln!(out, "+++ {new_label}");
        } else {
            out.push_str(line);
        }
    }
    out
}

/// Resolves `rev` to a full commit id; an error when it names no commit.
fn resolve(dir: &Path, op: &str, rev: &str) -> Result<String, GitError> {
    check_rev(op, rev)?;
    gitx::commit(dir, rev).ok_or_else(|| GitError {
        args: owned_args(&[op, rev]),
        stderr: format!("unknown revision '{rev}'"),
    })
}

/// Rejects a revision that git would read as an option.
fn check_rev(op: &str, rev: &str) -> Result<(), GitError> {
    if rev.starts_with('-') {
        return Err(GitError {
            args: owned_args(&[op, rev]),
            stderr: format!("invalid revision '{rev}'"),
        });
    }
    Ok(())
}

/// Rejects a path git would resolve against its working directory rather than
/// the repository root, or that is empty.
fn check_path(op: &str, path: &str) -> Result<(), GitError> {
    if path.is_empty() || path.starts_with('/') || path.starts_with("./") || path.starts_with("../")
    {
        return Err(GitError {
            args: owned_args(&[op, path]),
            stderr: format!("invalid path '{path}'"),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gitx;
    use crate::testrepo::{commit_all, git, repo, write};

    /// Twelve numbered lines, `line 1` to `line 12`.
    fn numbered() -> String {
        (1..=12).fold(String::new(), |mut text, i| {
            let _ = writeln!(text, "line {i}");
            text
        })
    }

    #[test]
    fn same_edit_at_different_positions_gives_one_id() {
        let r = repo();
        write(r.path(), "f.txt", &numbered());
        commit_all(r.path(), "init");

        // Branch one edits line 9 of the base.
        git(r.path(), &["switch", "-q", "-c", "one"]);
        write(r.path(), "f.txt", &numbered().replace("line 9\n", "EDIT\n"));
        commit_all(r.path(), "one");
        let one = gitx::commit(r.path(), "HEAD").unwrap();

        // Branch two first adds two lines at the top, so the same edit lands
        // at line 11. Its surrounding context (lines 6 to 8 and 10 to 12) is
        // unchanged, since patch-ids hash context lines.
        git(r.path(), &["switch", "-q", "main"]);
        let prefixed = format!("top a\ntop b\n{}", numbered());
        write(r.path(), "f.txt", &prefixed);
        commit_all(r.path(), "prefix");
        git(r.path(), &["switch", "-q", "-c", "two"]);
        write(r.path(), "f.txt", &prefixed.replace("line 9\n", "EDIT\n"));
        commit_all(r.path(), "two");
        let two = gitx::commit(r.path(), "HEAD").unwrap();

        let a = commit_file_patch_id(r.path(), &one, "f.txt").unwrap();
        let b = commit_file_patch_id(r.path(), &two, "f.txt").unwrap();
        assert!(a.is_some());
        assert_eq!(a, b);
    }

    #[test]
    fn whitespace_only_difference_in_the_edit_gives_one_id() {
        let r = repo();
        write(r.path(), "f.txt", &numbered());
        commit_all(r.path(), "init");

        write(r.path(), "f.txt", &numbered().replace("line 3\n", "EDIT\n"));
        commit_all(r.path(), "plain");
        let plain = gitx::commit(r.path(), "HEAD").unwrap();

        git(r.path(), &["reset", "-q", "--hard", "HEAD^"]);
        write(
            r.path(),
            "f.txt",
            &numbered().replace("line 3\n", "  EDIT \n"),
        );
        commit_all(r.path(), "indented");
        let indented = gitx::commit(r.path(), "HEAD").unwrap();

        let a = commit_file_patch_id(r.path(), &plain, "f.txt").unwrap();
        let b = commit_file_patch_id(r.path(), &indented, "f.txt").unwrap();
        assert!(a.is_some());
        assert_eq!(a, b);
    }

    #[test]
    fn a_different_edit_gives_a_different_id() {
        let r = repo();
        write(r.path(), "f.txt", &numbered());
        commit_all(r.path(), "init");

        write(r.path(), "f.txt", &numbered().replace("line 3\n", "EDIT\n"));
        commit_all(r.path(), "edit");
        let edit = gitx::commit(r.path(), "HEAD").unwrap();

        git(r.path(), &["reset", "-q", "--hard", "HEAD^"]);
        write(
            r.path(),
            "f.txt",
            &numbered().replace("line 3\n", "OTHER\n"),
        );
        commit_all(r.path(), "other");
        let other = gitx::commit(r.path(), "HEAD").unwrap();

        let a = commit_file_patch_id(r.path(), &edit, "f.txt").unwrap();
        let b = commit_file_patch_id(r.path(), &other, "f.txt").unwrap();
        assert!(a.is_some() && b.is_some());
        assert_ne!(a, b);
    }

    #[test]
    fn blobs_patch_id_equals_the_commit_patch_id() {
        let r = repo();
        write(r.path(), "f.txt", &numbered());
        commit_all(r.path(), "init");
        write(r.path(), "f.txt", &numbered().replace("line 4\n", "EDIT\n"));
        commit_all(r.path(), "edit");

        let old = blob_at(r.path(), "HEAD^", "f.txt").unwrap().unwrap();
        let new = blob_at(r.path(), "HEAD", "f.txt").unwrap().unwrap();
        let from_blobs = blobs_patch_id(r.path(), &old, &new, "f.txt").unwrap();
        let from_commit = commit_file_patch_id(r.path(), "HEAD", "f.txt").unwrap();
        assert!(from_commit.is_some());
        assert_eq!(from_blobs, from_commit);
    }

    #[test]
    fn blobs_of_a_different_edit_differ_from_the_commit() {
        let r = repo();
        write(r.path(), "f.txt", &numbered());
        commit_all(r.path(), "init");
        write(r.path(), "f.txt", &numbered().replace("line 4\n", "EDIT\n"));
        commit_all(r.path(), "edit");

        let old = blob_at(r.path(), "HEAD^", "f.txt").unwrap().unwrap();
        let other =
            gitx::store_content(r.path(), &numbered().replace("line 6\n", "EDIT\n")).unwrap();
        let from_blobs = blobs_patch_id(r.path(), &old, &other, "f.txt").unwrap();
        let from_commit = commit_file_patch_id(r.path(), "HEAD", "f.txt").unwrap();
        assert!(from_blobs.is_some());
        assert_ne!(from_blobs, from_commit);
    }

    #[test]
    fn blobs_patch_id_of_equal_blobs_is_none() {
        let r = repo();
        write(r.path(), "f.txt", &numbered());
        commit_all(r.path(), "init");
        let id = blob_at(r.path(), "HEAD", "f.txt").unwrap().unwrap();

        assert_eq!(blobs_patch_id(r.path(), &id, &id, "f.txt").unwrap(), None);
    }

    #[test]
    fn commit_that_does_not_touch_the_path_is_none() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        write(r.path(), "b.txt", "b\n");
        commit_all(r.path(), "init");
        write(r.path(), "a.txt", "changed\n");
        commit_all(r.path(), "touch a only");

        assert_eq!(
            commit_file_patch_id(r.path(), "HEAD", "b.txt").unwrap(),
            None
        );
        assert!(
            commit_file_patch_id(r.path(), "HEAD", "a.txt")
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn blob_at_is_none_for_a_missing_path_and_a_directory() {
        let r = repo();
        write(r.path(), "dir/f.txt", "x\n");
        commit_all(r.path(), "init");

        assert_eq!(blob_at(r.path(), "HEAD", "missing.txt").unwrap(), None);
        assert_eq!(blob_at(r.path(), "HEAD", "dir").unwrap(), None);
        let id = blob_at(r.path(), "HEAD", "dir/f.txt").unwrap().unwrap();
        assert_eq!(id, gitx::store_content(r.path(), "x\n").unwrap());
    }

    #[test]
    fn blob_at_fails_for_an_unknown_revision() {
        let r = repo();
        write(r.path(), "f.txt", "x\n");
        commit_all(r.path(), "init");

        assert!(blob_at(r.path(), "no-such-rev", "f.txt").is_err());
        assert!(blob_at(r.path(), "--all", "f.txt").is_err());
    }

    #[test]
    fn root_commit_diffs_against_the_empty_tree() {
        let r = repo();
        write(r.path(), "f.txt", &numbered());
        commit_all(r.path(), "root");
        let root = gitx::commit(r.path(), "HEAD").unwrap();

        let from_commit = commit_file_patch_id(r.path(), &root, "f.txt").unwrap();
        assert!(from_commit.is_some());

        let empty = gitx::store_content(r.path(), "").unwrap();
        let new = blob_at(r.path(), &root, "f.txt").unwrap().unwrap();
        assert_eq!(
            blobs_patch_id(r.path(), &empty, &new, "f.txt").unwrap(),
            from_commit
        );
    }
}
