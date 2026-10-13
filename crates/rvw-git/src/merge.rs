//! Merge and cherry-pick, and the operation a conflict leaves in progress:
//! which one it is, the conflicted files, abort and continue.

use std::path::{Path, PathBuf};

use crate::gitx::{exec, exec_env, owned_args, run, run_env, GitError};

/// How a merge may fast-forward.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FastForward {
    /// Fast-forward when possible, else a merge commit (git's default).
    Allow,
    /// Always a merge commit (`--no-ff`).
    Never,
}

/// What a merge or a cherry-pick did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// HEAD moved to this commit.
    Done { head: String },
    /// Nothing to do: already merged, or the cherry-pick's change is already there (empty).
    UpToDate,
    /// Stopped with conflicts; the operation is in progress. Paths sorted.
    Conflicts { files: Vec<String> },
}

/// An operation git has left half done.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    /// A merge, from `git merge`.
    Merge,
    /// A cherry-pick, from `git cherry-pick`.
    CherryPick,
    /// A revert, from `git revert`.
    Revert,
    /// A rebase, interactive or not.
    Rebase,
}

/// Environment for git calls that could open an editor: none ever does.
const NO_EDITOR: [(&str, &str); 1] = [("GIT_EDITOR", "true")];

/// Merges `rev` into the current branch with git's default message. Returns `Done`
/// with the new HEAD, `UpToDate` when HEAD did not move, and `Conflicts` when the merge
/// stopped on conflicts. Any other failure, such as a dirty file the merge would
/// overwrite, is an error and leaves nothing in progress.
pub fn merge(dir: &Path, rev: &str, ff: FastForward) -> Result<Outcome, GitError> {
    reject_option(&["merge"], rev)?;
    ensure_idle(dir, &["merge"])?;
    let before = head(dir)?;
    let policy = match ff {
        FastForward::Allow => "--ff",
        FastForward::Never => "--no-ff",
    };
    let args = ["merge", "--no-edit", policy, rev];
    let ran = exec_env(dir, &[], &args, &NO_EDITOR)?;
    if ran.code == Some(0) {
        return moved(dir, &before);
    }
    let files = conflicted(dir)?;
    if !files.is_empty() {
        return Ok(Outcome::Conflicts { files });
    }
    if in_progress(dir)? == Some(Operation::Merge) {
        // Rerere staged every resolution: the merge waits for `finish`.
        return Ok(Outcome::Conflicts { files: Vec::new() });
    }
    Err(refuse(&args, &ran.stderr))
}

/// Applies commit `id` on top of HEAD with its message and author. Returns the same
/// outcomes as [`merge`]; a change HEAD already has is `UpToDate` and leaves nothing in
/// progress. A merge commit is refused, as is any other failure.
pub fn cherry_pick(dir: &Path, id: &str) -> Result<Outcome, GitError> {
    reject_option(&["cherry-pick"], id)?;
    ensure_idle(dir, &["cherry-pick"])?;
    let parents = run(dir, &[], &["rev-list", "--parents", "-n", "1", id])?;
    // The first word is the commit itself; a second and third word are parents.
    if parents.split_whitespace().count() > 2 {
        return Err(refuse(
            &["cherry-pick", id],
            "cannot cherry-pick a merge commit",
        ));
    }
    let before = head(dir)?;
    let args = ["cherry-pick", id];
    let ran = exec_env(dir, &[], &args, &NO_EDITOR)?;
    if ran.code == Some(0) {
        return moved(dir, &before);
    }
    let files = conflicted(dir)?;
    if !files.is_empty() {
        return Ok(Outcome::Conflicts { files });
    }
    if in_progress(dir)? != Some(Operation::CherryPick) {
        return Err(refuse(&args, &ran.stderr));
    }
    if index_matches_head(dir)? {
        // The change is already in HEAD, so git stopped with nothing to commit.
        run(dir, &[], &["cherry-pick", "--skip"])?;
        return Ok(Outcome::UpToDate);
    }
    // Rerere staged a resolution: the cherry-pick waits for `finish`.
    Ok(Outcome::Conflicts { files: Vec::new() })
}

/// The operation in progress in the worktree holding `dir`, if any. A linked worktree
/// has its own state, so this answers for `dir`'s worktree only.
pub fn in_progress(dir: &Path) -> Result<Option<Operation>, GitError> {
    let git = git_dir(dir)?;
    if git.join("rebase-merge").is_dir() || git.join("rebase-apply").is_dir() {
        return Ok(Some(Operation::Rebase));
    }
    let markers = [
        ("MERGE_HEAD", Operation::Merge),
        ("CHERRY_PICK_HEAD", Operation::CherryPick),
        ("REVERT_HEAD", Operation::Revert),
    ];
    Ok(markers
        .into_iter()
        .find(|(file, _)| git.join(file).exists())
        .map(|(_, op)| op))
}

/// Paths with unresolved conflicts, sorted; empty when there is none.
pub fn conflicted(dir: &Path) -> Result<Vec<String>, GitError> {
    let out = run(dir, &[], &["diff", "--name-only", "--diff-filter=U", "-z"])?;
    let mut files: Vec<String> = out
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
        .collect();
    files.sort();
    Ok(files)
}

/// Aborts the merge or cherry-pick in progress, restoring the state before it. Errors
/// when none is in progress, or when the one in progress is a revert or a rebase, which
/// rvw did not start.
pub fn abort(dir: &Path) -> Result<(), GitError> {
    let args: &[&str] = match in_progress(dir)? {
        Some(Operation::Merge) => &["merge", "--abort"],
        Some(Operation::CherryPick) => &["cherry-pick", "--abort"],
        Some(Operation::Revert | Operation::Rebase) => {
            return Err(refuse(
                &["status"],
                "a revert or rebase was not started by rvw; abort it with git",
            ))
        }
        None => return Err(refuse(&["status"], "no merge or cherry-pick in progress")),
    };
    run(dir, &[], args)?;
    Ok(())
}

/// Finishes the merge or cherry-pick in progress once every conflict is resolved and
/// staged, and returns the new HEAD. Errors while any path is still conflicted, when
/// nothing is in progress, or when the one in progress is a revert or a rebase.
pub fn finish(dir: &Path) -> Result<String, GitError> {
    let args: &[&str] = match in_progress(dir)? {
        Some(Operation::Merge) => &["commit", "--no-edit"],
        Some(Operation::CherryPick) => &["cherry-pick", "--continue"],
        Some(Operation::Revert | Operation::Rebase) => {
            return Err(refuse(
                &["status"],
                "only a merge or cherry-pick is finished here",
            ))
        }
        None => return Err(refuse(&["status"], "no merge or cherry-pick in progress")),
    };
    if !conflicted(dir)?.is_empty() {
        return Err(refuse(args, "resolve and stage every conflict first"));
    }
    run_env(dir, &[], args, &NO_EDITOR)?;
    head(dir)
}

/// The git directory of the worktree holding `dir`: `.git` in the main worktree,
/// `.git/worktrees/<id>` in a linked one.
fn git_dir(dir: &Path) -> Result<PathBuf, GitError> {
    let out = run(dir, &[], &["rev-parse", "--absolute-git-dir"])?;
    Ok(PathBuf::from(out.trim()))
}

/// The commit HEAD points at.
fn head(dir: &Path) -> Result<String, GitError> {
    Ok(run(dir, &[], &["rev-parse", "HEAD"])?.trim().to_owned())
}

/// `Done` with the new HEAD when HEAD moved from `before`, else `UpToDate`.
fn moved(dir: &Path, before: &str) -> Result<Outcome, GitError> {
    let head = head(dir)?;
    Ok(if head == before {
        Outcome::UpToDate
    } else {
        Outcome::Done { head }
    })
}

/// Whether the index equals HEAD, so a stopped cherry-pick would commit nothing.
fn index_matches_head(dir: &Path) -> Result<bool, GitError> {
    let args = ["diff", "--cached", "--quiet", "HEAD"];
    let ran = exec(dir, &[], &args)?;
    match ran.code {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(refuse(&args, &ran.stderr)),
    }
}

/// Errors when a merge, cherry-pick, revert or rebase is already in progress: git
/// would refuse, and the result of a call could not be told from the one in progress.
fn ensure_idle(dir: &Path, args: &[&str]) -> Result<(), GitError> {
    match in_progress(dir)? {
        None => Ok(()),
        Some(_) => Err(refuse(
            args,
            "a merge, cherry-pick, revert or rebase is already in progress",
        )),
    }
}

/// Refuses a revision that would be read as a git option.
fn reject_option(op: &[&str], rev: &str) -> Result<(), GitError> {
    if rev.starts_with('-') {
        let mut args = owned_args(op);
        args.push(rev.to_owned());
        return Err(GitError {
            args,
            stderr: format!("invalid revision '{rev}'"),
        });
    }
    Ok(())
}

/// A failure of the git command `args`, with git's message.
fn refuse(args: &[&str], stderr: &str) -> GitError {
    GitError {
        args: owned_args(args),
        stderr: stderr.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use tempfile::TempDir;

    use super::*;
    use crate::testrepo::{commit_all, git, repo, write};

    /// A repository with one commit, a.txt = "base". rerere is off: a global
    /// `rerere.autoupdate` would resolve the conflicts these tests make.
    fn base_repo() -> TempDir {
        let dir = repo();
        git(dir.path(), &["config", "rerere.enabled", "false"]);
        write(dir.path(), "a.txt", "base\n");
        commit_all(dir.path(), "base");
        dir
    }

    /// Commits `content` to `rel` on a new branch from HEAD, then checks out main.
    /// Returns the new commit.
    fn branch_with(dir: &Path, branch: &str, rel: &str, content: &str, msg: &str) -> String {
        git(dir, &["checkout", "-q", "-b", branch]);
        write(dir, rel, content);
        commit_all(dir, msg);
        let id = head_of(dir);
        git(dir, &["checkout", "-q", "main"]);
        id
    }

    /// Branch `feature` sets a.txt to "feature" and main sets it to "main", so a merge
    /// or cherry-pick of `feature` conflicts. Returns the feature commit.
    fn conflicting(dir: &Path) -> String {
        let id = branch_with(dir, "feature", "a.txt", "feature\n", "feature change");
        write(dir, "a.txt", "main\n");
        commit_all(dir, "main change");
        id
    }

    fn head_of(dir: &Path) -> String {
        git(dir, &["rev-parse", "HEAD"]).trim().to_owned()
    }

    fn parent_count(dir: &Path) -> usize {
        git(dir, &["rev-list", "--parents", "-n", "1", "HEAD"])
            .split_whitespace()
            .count()
            - 1
    }

    fn is_clean(dir: &Path) -> bool {
        git(dir, &["status", "--porcelain"]).is_empty()
    }

    fn names(files: &[&str]) -> Vec<String> {
        files.iter().map(|f| (*f).to_owned()).collect()
    }

    #[test]
    fn merge_fast_forwards_to_the_branch_tip() {
        let dir = base_repo();
        let tip = branch_with(dir.path(), "feature", "b.txt", "b\n", "add b");

        let outcome = merge(dir.path(), "feature", FastForward::Allow).unwrap();

        assert_eq!(outcome, Outcome::Done { head: tip.clone() });
        assert_eq!(head_of(dir.path()), tip);
    }

    #[test]
    fn merge_never_fast_forward_makes_a_merge_commit() {
        let dir = base_repo();
        let tip = branch_with(dir.path(), "feature", "b.txt", "b\n", "add b");

        let outcome = merge(dir.path(), "feature", FastForward::Never).unwrap();

        let head = head_of(dir.path());
        assert_eq!(outcome, Outcome::Done { head: head.clone() });
        assert_ne!(head, tip);
        assert_eq!(parent_count(dir.path()), 2);
    }

    #[test]
    fn merge_of_a_branch_already_merged_is_up_to_date() {
        let dir = base_repo();
        git(dir.path(), &["branch", "same"]);
        let before = head_of(dir.path());

        let outcome = merge(dir.path(), "same", FastForward::Allow).unwrap();

        assert_eq!(outcome, Outcome::UpToDate);
        assert_eq!(head_of(dir.path()), before);
        assert_eq!(in_progress(dir.path()).unwrap(), None);
    }

    #[test]
    fn merge_conflict_stops_and_abort_restores_the_branch() {
        let dir = base_repo();
        conflicting(dir.path());
        let before = head_of(dir.path());

        let outcome = merge(dir.path(), "feature", FastForward::Allow).unwrap();

        assert_eq!(
            outcome,
            Outcome::Conflicts {
                files: names(&["a.txt"])
            }
        );
        assert_eq!(in_progress(dir.path()).unwrap(), Some(Operation::Merge));
        assert_eq!(conflicted(dir.path()).unwrap(), names(&["a.txt"]));

        abort(dir.path()).unwrap();

        assert!(is_clean(dir.path()));
        assert_eq!(in_progress(dir.path()).unwrap(), None);
        assert_eq!(head_of(dir.path()), before);
    }

    #[test]
    fn resolved_and_staged_merge_finishes_with_two_parents() {
        let dir = base_repo();
        let before = head_of(dir.path());
        conflicting(dir.path());
        merge(dir.path(), "feature", FastForward::Allow).unwrap();
        write(dir.path(), "a.txt", "resolved\n");
        git(dir.path(), &["add", "a.txt"]);

        let head = finish(dir.path()).unwrap();

        assert_eq!(head, head_of(dir.path()));
        assert_ne!(head, before);
        assert_eq!(parent_count(dir.path()), 2);
        assert_eq!(in_progress(dir.path()).unwrap(), None);
        assert!(conflicted(dir.path()).unwrap().is_empty());
    }

    #[test]
    fn finish_refuses_while_a_conflict_is_unresolved() {
        let dir = base_repo();
        conflicting(dir.path());
        merge(dir.path(), "feature", FastForward::Allow).unwrap();

        let err = finish(dir.path()).unwrap_err();

        assert!(err
            .stderr
            .contains("resolve and stage every conflict first"));
        assert_eq!(in_progress(dir.path()).unwrap(), Some(Operation::Merge));
        abort(dir.path()).unwrap();
    }

    #[test]
    fn merge_refused_by_a_dirty_file_leaves_nothing_in_progress() {
        let dir = base_repo();
        let before = head_of(dir.path());
        branch_with(
            dir.path(),
            "feature",
            "a.txt",
            "feature\n",
            "feature change",
        );
        write(dir.path(), "a.txt", "dirty\n");

        let err = merge(dir.path(), "feature", FastForward::Allow).unwrap_err();

        assert!(err.stderr.contains("would be overwritten"));
        assert_eq!(in_progress(dir.path()).unwrap(), None);
        assert_eq!(head_of(dir.path()), before);
        assert_eq!(
            fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "dirty\n"
        );
    }

    #[test]
    fn a_revision_that_starts_with_a_dash_is_refused() {
        let dir = base_repo();

        let err = merge(dir.path(), "--all", FastForward::Allow).unwrap_err();
        assert!(err.stderr.contains("invalid revision"));

        let err = cherry_pick(dir.path(), "--all").unwrap_err();
        assert!(err.stderr.contains("invalid revision"));
    }

    #[test]
    fn cherry_pick_applies_the_commit_with_its_author_and_subject() {
        let dir = base_repo();
        let before = head_of(dir.path());
        git(dir.path(), &["checkout", "-q", "-b", "feature"]);
        write(dir.path(), "c.txt", "c\n");
        git(dir.path(), &["add", "-A"]);
        git(
            dir.path(),
            &[
                "-c",
                "user.name=Other",
                "-c",
                "user.email=other@example.com",
                "commit",
                "-q",
                "-m",
                "add c",
            ],
        );
        let id = head_of(dir.path());
        git(dir.path(), &["checkout", "-q", "main"]);

        let outcome = cherry_pick(dir.path(), &id).unwrap();

        let head = head_of(dir.path());
        assert_eq!(outcome, Outcome::Done { head: head.clone() });
        assert_ne!(head, before);
        assert_eq!(
            git(dir.path(), &["log", "-1", "--format=%an|%ae|%s"]).trim(),
            "Other|other@example.com|add c"
        );
        assert!(dir.path().join("c.txt").exists());
    }

    #[test]
    fn cherry_pick_of_an_applied_change_is_up_to_date() {
        let dir = base_repo();
        let id = branch_with(dir.path(), "feature", "c.txt", "c\n", "add c");
        assert!(matches!(
            cherry_pick(dir.path(), &id).unwrap(),
            Outcome::Done { .. }
        ));
        let before = head_of(dir.path());

        let outcome = cherry_pick(dir.path(), &id).unwrap();

        assert_eq!(outcome, Outcome::UpToDate);
        assert_eq!(head_of(dir.path()), before);
        assert_eq!(in_progress(dir.path()).unwrap(), None);
        assert!(is_clean(dir.path()));
    }

    #[test]
    fn cherry_pick_of_a_merge_commit_is_refused() {
        let dir = base_repo();
        branch_with(dir.path(), "feature", "b.txt", "b\n", "add b");
        write(dir.path(), "d.txt", "d\n");
        commit_all(dir.path(), "main d");
        git(
            dir.path(),
            &["merge", "--no-ff", "-q", "-m", "merge feature", "feature"],
        );
        let merge_commit = head_of(dir.path());

        let err = cherry_pick(dir.path(), &merge_commit).unwrap_err();

        assert!(err.stderr.contains("cannot cherry-pick a merge commit"));
        assert_eq!(in_progress(dir.path()).unwrap(), None);
    }

    #[test]
    fn cherry_pick_conflict_can_be_aborted() {
        let dir = base_repo();
        let id = conflicting(dir.path());
        let before = head_of(dir.path());

        let outcome = cherry_pick(dir.path(), &id).unwrap();

        assert_eq!(
            outcome,
            Outcome::Conflicts {
                files: names(&["a.txt"])
            }
        );
        assert_eq!(
            in_progress(dir.path()).unwrap(),
            Some(Operation::CherryPick)
        );
        assert_eq!(conflicted(dir.path()).unwrap(), names(&["a.txt"]));

        abort(dir.path()).unwrap();

        assert_eq!(in_progress(dir.path()).unwrap(), None);
        assert!(is_clean(dir.path()));
        assert_eq!(head_of(dir.path()), before);
    }

    #[test]
    fn cherry_pick_conflict_can_be_finished_after_resolving() {
        let dir = base_repo();
        let before = head_of(dir.path());
        let id = conflicting(dir.path());
        cherry_pick(dir.path(), &id).unwrap();
        write(dir.path(), "a.txt", "resolved\n");
        git(dir.path(), &["add", "a.txt"]);

        let head = finish(dir.path()).unwrap();

        assert_eq!(head, head_of(dir.path()));
        assert_ne!(head, before);
        assert_eq!(
            git(dir.path(), &["log", "-1", "--format=%s"]).trim(),
            "feature change"
        );
        assert_eq!(in_progress(dir.path()).unwrap(), None);
    }

    #[test]
    fn finish_without_an_operation_is_an_error() {
        let dir = base_repo();

        assert!(finish(dir.path()).is_err());
        assert!(abort(dir.path()).is_err());
    }

    #[test]
    fn a_rebase_is_reported_before_a_leftover_head_file() {
        let dir = base_repo();
        let git_dir = PathBuf::from(git(dir.path(), &["rev-parse", "--absolute-git-dir"]).trim());
        fs::create_dir(git_dir.join("rebase-merge")).unwrap();
        fs::write(
            git_dir.join("MERGE_HEAD"),
            "0000000000000000000000000000000000000000\n",
        )
        .unwrap();

        assert_eq!(in_progress(dir.path()).unwrap(), Some(Operation::Rebase));
        assert!(abort(dir.path()).is_err());
    }

    #[test]
    fn in_progress_and_conflicted_follow_a_linked_worktree() {
        let dir = base_repo();
        let parent = TempDir::new().unwrap();
        let wt = parent.path().join("side");
        git(
            dir.path(),
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "side",
                wt.to_str().unwrap(),
                "main",
            ],
        );
        write(&wt, "a.txt", "side\n");
        commit_all(&wt, "side change");
        write(dir.path(), "a.txt", "main\n");
        commit_all(dir.path(), "main change");

        let outcome = merge(&wt, "main", FastForward::Allow).unwrap();

        assert_eq!(
            outcome,
            Outcome::Conflicts {
                files: names(&["a.txt"])
            }
        );
        assert_eq!(in_progress(&wt).unwrap(), Some(Operation::Merge));
        assert_eq!(conflicted(&wt).unwrap(), names(&["a.txt"]));
        // The main worktree has no operation of its own.
        assert_eq!(in_progress(dir.path()).unwrap(), None);
        assert!(conflicted(dir.path()).unwrap().is_empty());

        abort(&wt).unwrap();

        assert_eq!(in_progress(&wt).unwrap(), None);
    }
}
