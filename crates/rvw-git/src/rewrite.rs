//! The narrow history edit: reword, squash and drop commits that are on no
//! remote. Planning writes the new commits as loose objects and touches no ref,
//! index or file, so the confirmation can show the exact result and a conflict
//! leaves nothing behind. Applying moves the branch with a compare-and-swap.
//!
//! Reword and squash keep every later commit's tree, so the new HEAD has the
//! same tree and applying never touches the index or the files an agent may be
//! editing. A drop replays the later commits with `merge-tree`; when the new
//! HEAD's tree differs, the index and files move from the old tree to the new
//! one with `read-tree -m -u`, which refuses to overwrite a local change.

use std::path::Path;

use crate::gitx::{exec, owned_args, run, run_env, GitError};
use crate::history::{parse_log, LogEntry, LOG_FORMAT};

/// Why the list of editable commits ends where it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    /// The next older commit is on a remote branch.
    Pushed,
    /// The next older commit is a merge.
    Merge,
    /// The oldest editable commit is the repository's first.
    Root,
    /// The list was cut at the limit asked for.
    Limit,
}

/// The commits that may be edited, newest first, and why the list ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Editable {
    pub branch: String,
    pub commits: Vec<LogEntry>,
    pub end: End,
}

/// What to do with one commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Replace its message.
    Reword(String),
    /// Fold it into the next older commit that is kept, which takes this
    /// message.
    Squash(String),
    /// Remove its change; later commits are replayed without it.
    Drop,
}

/// The new history, written as objects but not applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    pub branch: String,
    pub old_head: String,
    pub new_head: String,
    /// The edited range before, newest first.
    pub before: Vec<LogEntry>,
    /// The same range after, newest first.
    pub after: Vec<LogEntry>,
    /// The new HEAD's tree differs from the old one, so applying changes files.
    pub changes_files: bool,
}

/// The commits on the current branch that may be edited: its first-parent
/// chain back to the first commit that is on a remote branch or is a merge,
/// at most `limit` of them. Refuses a detached HEAD and an operation in
/// progress.
pub fn editable(dir: &Path, limit: usize) -> Result<Editable, GitError> {
    let branch = current_branch(dir)?;
    refuse_in_progress(dir)?;
    // git reads `-n` as an int.
    let take = limit.saturating_add(1).min(i32::MAX as usize).to_string();
    let out = run(
        dir,
        &[],
        &[
            "log",
            "--first-parent",
            LOG_FORMAT,
            "-n",
            &take,
            "HEAD",
            "--not",
            "--remotes",
            "--",
        ],
    )?;
    let mut commits = parse_log(&out);
    let mut end = if commits.len() > limit {
        commits.truncate(limit);
        End::Limit
    } else if commits.last().is_some_and(|c| c.parents.is_empty()) {
        End::Root
    } else {
        End::Pushed
    };
    if let Some(merge) = commits.iter().position(|c| c.parents.len() > 1) {
        commits.truncate(merge);
        end = End::Merge;
    }
    Ok(Editable {
        branch,
        commits,
        end,
    })
}

/// Writes the history that `edits` (commit id and action) make of the
/// editable commits, without moving any ref. Commits not named are kept. Fails
/// when an id is not editable, when a squash has no kept commit to fold into,
/// when the first commit would be dropped, and when a drop makes a later
/// commit conflict; the error names the commit and the files.
pub fn plan(dir: &Path, edits: &[(String, Action)]) -> Result<Planned, GitError> {
    let fail = |stderr: String| GitError {
        args: owned_args(&["rewrite"]),
        stderr,
    };
    let editable = editable(dir, usize::MAX)?;
    let oldest_first: Vec<&LogEntry> = editable.commits.iter().rev().collect();
    let mut actions: Vec<Option<&Action>> = vec![None; oldest_first.len()];
    for (id, action) in edits {
        let Some(i) = oldest_first.iter().position(|c| &c.id == id) else {
            return Err(fail(format!("commit {} cannot be edited", short(id))));
        };
        actions[i] = Some(action);
    }
    // A squash folds into an older commit, so the rewrite starts one earlier.
    let Some(start) = actions
        .iter()
        .enumerate()
        .filter_map(|(i, a)| match a {
            Some(Action::Squash(_)) => Some(i.saturating_sub(1)),
            Some(_) => Some(i),
            None => None,
        })
        .min()
    else {
        return Err(fail("nothing to edit".to_owned()));
    };

    let old_head = oldest_first
        .last()
        .map(|c| c.id.clone())
        .unwrap_or_default();
    let base = oldest_first[start].parents.first().cloned();
    let Some(new_head) = write_commits(dir, &oldest_first[start..], &actions[start..], base)?
    else {
        return Err(fail("the edit would leave the branch empty".to_owned()));
    };
    let before: Vec<LogEntry> = oldest_first[start..]
        .iter()
        .rev()
        .map(|c| (*c).clone())
        .collect();
    let after = match oldest_first[start].parents.first() {
        Some(base) => range(dir, base, &new_head)?,
        None => range_from_root(dir, &new_head)?,
    };
    let changes_files = tree_of(dir, &old_head)? != tree_of(dir, &new_head)?;
    Ok(Planned {
        branch: editable.branch,
        old_head,
        new_head,
        before,
        after,
        changes_files,
    })
}

/// Writes the new commits for `commits` (oldest first, starting on `parent`)
/// under `actions`, and returns the new head; None when nothing is left.
fn write_commits(
    dir: &Path,
    commits: &[&LogEntry],
    actions: &[Option<&Action>],
    mut parent: Option<String>,
) -> Result<Option<String>, GitError> {
    let fail = |stderr: String| GitError {
        args: owned_args(&["rewrite"]),
        stderr,
    };
    let sign = signs_commits(dir);
    // The commits written so far in the range, oldest first.
    let mut written: Vec<Written> = Vec::new();
    let mut replaying = false;
    for (commit, &action) in commits.iter().zip(actions) {
        if action == Some(&Action::Drop) {
            if commit.parents.is_empty() {
                return Err(fail("the first commit cannot be dropped".to_owned()));
            }
            replaying = true;
            continue;
        }
        let meta = read_commit(dir, &commit.id)?;
        let tree = if replaying {
            replay(dir, commit, parent.as_deref())?
        } else {
            meta.tree.clone()
        };
        if let Some(Action::Squash(message)) = action {
            let Some(into) = written.pop() else {
                return Err(fail(format!(
                    "{} has no kept commit to squash into",
                    short(&commit.id)
                )));
            };
            let id = commit_tree(
                dir,
                &tree,
                into.parent.as_deref(),
                &into.author,
                message,
                sign,
            )?;
            parent = Some(id);
            written.push(Written {
                parent: into.parent,
                author: into.author,
            });
        } else {
            let message = match action {
                Some(Action::Reword(message)) => message.as_str(),
                _ => meta.message.as_str(),
            };
            let unchanged = action.is_none()
                && !replaying
                && parent.as_deref() == commit.parents.first().map(String::as_str);
            let id = if unchanged {
                commit.id.clone()
            } else {
                commit_tree(dir, &tree, parent.as_deref(), &meta.author, message, sign)?
            };
            written.push(Written {
                parent: parent.clone(),
                author: meta.author,
            });
            parent = Some(id);
        }
    }
    Ok(parent)
}

/// Moves the branch to the planned history. Refuses when the branch or HEAD
/// moved since the plan, and when a file the new history changes has a local
/// change; in both cases nothing is changed.
pub fn apply(dir: &Path, planned: &Planned) -> Result<(), GitError> {
    let fail = |stderr: &str| GitError {
        args: owned_args(&["rewrite"]),
        stderr: stderr.to_owned(),
    };
    refuse_in_progress(dir)?;
    let moved = "the branch moved since the edit was planned; plan it again";
    if current_branch(dir)? != planned.branch
        || crate::commits::head(dir).as_deref() != Some(&planned.old_head)
    {
        return Err(fail(moved));
    }
    let old_tree = tree_of(dir, &planned.old_head)?;
    let new_tree = tree_of(dir, &planned.new_head)?;
    if planned.changes_files {
        run(dir, &[], &["read-tree", "-m", "-u", &old_tree, &new_tree])?;
    }
    let reference = format!("refs/heads/{}", planned.branch);
    let moved_ref = run(
        dir,
        &[],
        &[
            "update-ref",
            "-m",
            "rvw: edit history",
            &reference,
            &planned.new_head,
            &planned.old_head,
        ],
    );
    if moved_ref.is_err() {
        if planned.changes_files {
            // Put the index and files back; the branch never moved.
            run(dir, &[], &["read-tree", "-m", "-u", &new_tree, &old_tree])?;
        }
        return Err(fail(moved));
    }
    Ok(())
}

/// A commit written in the new history, enough to fold a later one into it.
struct Written {
    parent: Option<String>,
    author: Author,
}

#[derive(Clone)]
struct Author {
    name: String,
    email: String,
    /// `%ad` in raw form: seconds and zone, e.g. "1700000000 +0100".
    date: String,
}

struct Meta {
    tree: String,
    author: Author,
    message: String,
}

fn read_commit(dir: &Path, id: &str) -> Result<Meta, GitError> {
    let out = run(
        dir,
        &[],
        &[
            "log",
            "-1",
            "--date=raw",
            "--format=%T%x1f%an%x1f%ae%x1f%ad%x1f%B",
            id,
            "--",
        ],
    )?;
    let mut fields = out.splitn(5, '\x1f');
    let mut next = || fields.next().unwrap_or_default().to_owned();
    let (tree, name, email, date, message) = (next(), next(), next(), next(), next());
    Ok(Meta {
        tree,
        author: Author { name, email, date },
        message: message.trim_end_matches('\n').to_owned(),
    })
}

/// The tree of `commit`'s change put on top of `onto`: a cherry-pick done in
/// the object store. A conflict is an error naming the commit and the files.
fn replay(dir: &Path, commit: &LogEntry, onto: Option<&str>) -> Result<String, GitError> {
    let (Some(base), Some(onto)) = (commit.parents.first(), onto) else {
        return Err(GitError {
            args: owned_args(&["merge-tree"]),
            stderr: format!("{} cannot be replayed without a parent", short(&commit.id)),
        });
    };
    let merge_base = format!("--merge-base={base}");
    let args = [
        "merge-tree",
        "--write-tree",
        "--name-only",
        "--no-messages",
        merge_base.as_str(),
        onto,
        commit.id.as_str(),
    ];
    let ran = exec(dir, &[], &args)?;
    let mut lines = ran.stdout.lines();
    let tree = lines.next().unwrap_or_default().to_owned();
    match ran.code {
        Some(0) => Ok(tree),
        Some(1) => {
            let files: Vec<&str> = lines.filter(|l| !l.is_empty()).collect();
            Err(GitError {
                args: owned_args(&args),
                stderr: format!(
                    "without the dropped commit, {} \"{}\" conflicts in {}",
                    short(&commit.id),
                    commit.subject,
                    files.join(", ")
                ),
            })
        }
        _ => Err(GitError {
            args: owned_args(&args),
            stderr: ran.stderr,
        }),
    }
}

fn commit_tree(
    dir: &Path,
    tree: &str,
    parent: Option<&str>,
    author: &Author,
    message: &str,
    sign: bool,
) -> Result<String, GitError> {
    let mut args = vec!["commit-tree", tree];
    if let Some(parent) = parent {
        args.extend(["-p", parent]);
    }
    if sign {
        args.push("-S");
    }
    args.extend(["-F", "-"]);
    let env = [
        ("GIT_AUTHOR_NAME", author.name.as_str()),
        ("GIT_AUTHOR_EMAIL", author.email.as_str()),
        ("GIT_AUTHOR_DATE", author.date.as_str()),
    ];
    let mut message = message.trim_end_matches('\n').to_owned();
    message.push('\n');
    let out = run_env(dir, message.as_bytes(), &args, &env)?;
    Ok(out.trim().to_owned())
}

/// `commit-tree` does not read `commit.gpgSign`, so it is passed on when set.
fn signs_commits(dir: &Path) -> bool {
    run(dir, &[], &["config", "--bool", "commit.gpgSign"]).is_ok_and(|v| v.trim() == "true")
}

fn range(dir: &Path, base: &str, head: &str) -> Result<Vec<LogEntry>, GitError> {
    let spec = format!("{base}..{head}");
    let out = run(
        dir,
        &[],
        &["log", "--first-parent", LOG_FORMAT, &spec, "--"],
    )?;
    Ok(parse_log(&out))
}

fn range_from_root(dir: &Path, head: &str) -> Result<Vec<LogEntry>, GitError> {
    let out = run(dir, &[], &["log", "--first-parent", LOG_FORMAT, head, "--"])?;
    Ok(parse_log(&out))
}

fn tree_of(dir: &Path, commit: &str) -> Result<String, GitError> {
    let spec = format!("{commit}^{{tree}}");
    Ok(run(dir, &[], &["rev-parse", "--verify", &spec])?
        .trim()
        .to_owned())
}

fn current_branch(dir: &Path) -> Result<String, GitError> {
    let args = ["symbolic-ref", "-q", "--short", "HEAD"];
    let ran = exec(dir, &[], &args)?;
    let name = ran.stdout.trim();
    if ran.code != Some(0) || name.is_empty() {
        return Err(GitError {
            args: owned_args(&args),
            stderr: "history can only be edited on a branch; HEAD is detached".to_owned(),
        });
    }
    Ok(name.to_owned())
}

fn refuse_in_progress(dir: &Path) -> Result<(), GitError> {
    let args = ["rev-parse", "--absolute-git-dir"];
    let git_dir = run(dir, &[], &args)?;
    let git_dir = Path::new(git_dir.trim());
    let busy = [
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "rebase-merge",
        "rebase-apply",
    ]
    .iter()
    .any(|f| git_dir.join(f).exists());
    if busy {
        return Err(GitError {
            args: owned_args(&args),
            stderr: "finish or abort the operation in progress first".to_owned(),
        });
    }
    Ok(())
}

fn short(id: &str) -> &str {
    id.get(..7).unwrap_or(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testrepo::{commit_all, git, repo, write};
    use tempfile::TempDir;

    /// main with commits c1..c3 changing `a.txt`, `b.txt`, `c.txt`, and no remote.
    fn three() -> (TempDir, Vec<String>) {
        let r = repo();
        let mut ids = Vec::new();
        for (file, msg) in [("a.txt", "c1"), ("b.txt", "c2"), ("c.txt", "c3")] {
            write(r.path(), file, &format!("{msg}\n"));
            commit_all(r.path(), msg);
            ids.push(git(r.path(), &["rev-parse", "HEAD"]).trim().to_owned());
        }
        (r, ids)
    }

    fn subjects(entries: &[LogEntry]) -> Vec<&str> {
        entries.iter().map(|e| e.subject.as_str()).collect()
    }

    fn head(dir: &Path) -> String {
        git(dir, &["rev-parse", "HEAD"]).trim().to_owned()
    }

    #[test]
    fn editable_lists_unpushed_commits_newest_first_and_stops_at_a_pushed_one() {
        let (r, ids) = three();
        let e = editable(r.path(), 50).unwrap();
        assert_eq!(subjects(&e.commits), ["c3", "c2", "c1"]);
        assert_eq!(e.end, End::Root);
        assert_eq!(e.branch, "main");

        git(
            r.path(),
            &["update-ref", "refs/remotes/origin/main", &ids[0]],
        );
        let e = editable(r.path(), 50).unwrap();
        assert_eq!(subjects(&e.commits), ["c3", "c2"]);
        assert_eq!(e.end, End::Pushed);

        let e = editable(r.path(), 1).unwrap();
        assert_eq!(subjects(&e.commits), ["c3"]);
        assert_eq!(e.end, End::Limit);
    }

    #[test]
    fn editable_stops_below_a_merge() {
        let (r, _) = three();
        git(r.path(), &["checkout", "-q", "-b", "side", "HEAD~1"]);
        write(r.path(), "side.txt", "s\n");
        commit_all(r.path(), "side");
        git(r.path(), &["checkout", "-q", "main"]);
        git(r.path(), &["merge", "-q", "--no-ff", "--no-edit", "side"]);
        write(r.path(), "d.txt", "d\n");
        commit_all(r.path(), "c4");
        let e = editable(r.path(), 50).unwrap();
        assert_eq!(subjects(&e.commits), ["c4"]);
        assert_eq!(e.end, End::Merge);
    }

    #[test]
    fn editable_refuses_a_detached_head() {
        let (r, ids) = three();
        git(r.path(), &["checkout", "-q", "--detach", &ids[1]]);
        let err = editable(r.path(), 50).unwrap_err();
        assert!(err.stderr.contains("detached"), "{err}");
    }

    #[test]
    fn reword_keeps_trees_authors_and_leaves_local_changes_alone() {
        let (r, ids) = three();
        git(
            r.path(),
            &[
                "commit",
                "-q",
                "--amend",
                "--no-edit",
                "--author=Ana <ana@x>",
                "--date=1700000000 +0100",
            ],
        );
        let ids = [ids[0].clone(), ids[1].clone(), head(r.path())];
        write(r.path(), "a.txt", "agent is editing\n");
        write(r.path(), "new.txt", "untracked\n");
        git(r.path(), &["add", "new.txt"]);
        let status = git(r.path(), &["status", "--porcelain"]);

        let planned = plan(
            r.path(),
            &[(ids[1].clone(), Action::Reword("c2 better\n\nbody".into()))],
        )
        .unwrap();
        assert!(!planned.changes_files);
        assert_eq!(subjects(&planned.before), ["c3", "c2"]);
        assert_eq!(subjects(&planned.after), ["c3", "c2 better"]);
        assert_eq!(head(r.path()), ids[2], "planning moves nothing");

        apply(r.path(), &planned).unwrap();
        assert_eq!(head(r.path()), planned.new_head);
        assert_eq!(git(r.path(), &["status", "--porcelain"]), status);
        assert_eq!(
            git(
                r.path(),
                &["log", "-1", "--format=%an %ae %ad", "--date=raw"]
            )
            .trim(),
            "Ana ana@x 1700000000 +0100"
        );
        assert_eq!(
            git(r.path(), &["log", "-1", "--format=%B", "HEAD~1"]).trim(),
            "c2 better\n\nbody"
        );
        assert_eq!(
            git(r.path(), &["rev-parse", "HEAD~2"]).trim(),
            ids[0],
            "c1 is not rewritten"
        );
        assert_eq!(
            git(r.path(), &["rev-parse", "HEAD^{tree}"]),
            git(r.path(), &["rev-parse", &format!("{}^{{tree}}", ids[2])])
        );
    }

    #[test]
    fn squash_folds_into_the_older_commit_with_the_given_message() {
        let (r, ids) = three();
        let planned = plan(
            r.path(),
            &[(ids[2].clone(), Action::Squash("c2 and c3".into()))],
        )
        .unwrap();
        assert_eq!(subjects(&planned.before), ["c3", "c2"]);
        assert_eq!(subjects(&planned.after), ["c2 and c3"]);
        assert!(!planned.changes_files);
        apply(r.path(), &planned).unwrap();
        assert_eq!(git(r.path(), &["rev-list", "--count", "HEAD"]).trim(), "2");
        assert_eq!(
            git(r.path(), &["show", "--format=", "--name-only", "HEAD"]).trim(),
            "b.txt\nc.txt"
        );
    }

    #[test]
    fn squash_of_the_oldest_commit_is_refused() {
        let (r, ids) = three();
        let err = plan(r.path(), &[(ids[0].clone(), Action::Squash("x".into()))]).unwrap_err();
        assert!(err.stderr.contains("no kept commit"), "{err}");
    }

    #[test]
    fn drop_replays_later_commits_and_updates_the_files_keeping_unrelated_changes() {
        let (r, ids) = three();
        write(r.path(), "c.txt", "agent is editing c\n");
        let planned = plan(r.path(), &[(ids[1].clone(), Action::Drop)]).unwrap();
        assert!(planned.changes_files);
        assert_eq!(subjects(&planned.after), ["c3"]);
        apply(r.path(), &planned).unwrap();
        assert!(!r.path().join("b.txt").exists());
        assert_eq!(
            std::fs::read_to_string(r.path().join("c.txt")).unwrap(),
            "agent is editing c\n"
        );
        assert_eq!(git(r.path(), &["status", "--porcelain"]), " M c.txt\n");
        assert_eq!(git(r.path(), &["log", "--format=%s"]).trim(), "c3\nc1");
    }

    #[test]
    fn drop_that_conflicts_is_refused_and_writes_no_ref() {
        let r = repo();
        write(r.path(), "a.txt", "one\n");
        commit_all(r.path(), "c1");
        write(r.path(), "a.txt", "two\n");
        commit_all(r.path(), "c2");
        let c2 = head(r.path());
        write(r.path(), "a.txt", "three\n");
        commit_all(r.path(), "c3");
        let before = git(r.path(), &["for-each-ref"]);
        let err = plan(r.path(), &[(c2, Action::Drop)]).unwrap_err();
        assert!(err.stderr.contains("conflicts in a.txt"), "{err}");
        assert_eq!(git(r.path(), &["for-each-ref"]), before);
        assert_eq!(git(r.path(), &["status", "--porcelain"]), "");
    }

    #[test]
    fn drop_refuses_when_a_local_change_would_be_overwritten() {
        let (r, ids) = three();
        write(r.path(), "b.txt", "agent is editing b\n");
        let planned = plan(r.path(), &[(ids[1].clone(), Action::Drop)]).unwrap();
        let old = head(r.path());
        assert!(apply(r.path(), &planned).is_err());
        assert_eq!(head(r.path()), old);
        assert_eq!(
            std::fs::read_to_string(r.path().join("b.txt")).unwrap(),
            "agent is editing b\n"
        );
    }

    #[test]
    fn apply_refuses_when_the_branch_moved_since_the_plan() {
        let (r, ids) = three();
        let planned = plan(r.path(), &[(ids[1].clone(), Action::Reword("x".into()))]).unwrap();
        write(r.path(), "d.txt", "d\n");
        commit_all(r.path(), "c4 by an agent");
        let moved = head(r.path());
        let err = apply(r.path(), &planned).unwrap_err();
        assert!(err.stderr.contains("moved"), "{err}");
        assert_eq!(head(r.path()), moved);
    }

    #[test]
    fn plan_refuses_a_commit_that_is_not_editable_and_the_first_commits_drop() {
        let (r, ids) = three();
        git(
            r.path(),
            &["update-ref", "refs/remotes/origin/main", &ids[0]],
        );
        assert!(plan(r.path(), &[(ids[0].clone(), Action::Drop)])
            .unwrap_err()
            .stderr
            .contains("cannot be edited"));
        git(r.path(), &["update-ref", "-d", "refs/remotes/origin/main"]);
        assert!(plan(r.path(), &[(ids[0].clone(), Action::Drop)])
            .unwrap_err()
            .stderr
            .contains("first commit"));
    }

    #[test]
    fn several_edits_at_once() {
        let r = repo();
        let mut ids = Vec::new();
        for i in 1..=5 {
            write(r.path(), &format!("f{i}.txt"), "x\n");
            commit_all(r.path(), &format!("c{i}"));
            ids.push(head(r.path()));
        }
        let planned = plan(
            r.path(),
            &[
                (ids[1].clone(), Action::Reword("c2'".into())),
                (ids[2].clone(), Action::Squash("c2+c3".into())),
                (ids[3].clone(), Action::Drop),
            ],
        )
        .unwrap();
        assert_eq!(subjects(&planned.before), ["c5", "c4", "c3", "c2"]);
        assert_eq!(subjects(&planned.after), ["c5", "c2+c3"]);
        apply(r.path(), &planned).unwrap();
        assert_eq!(
            git(r.path(), &["log", "--format=%s"]).trim(),
            "c5\nc2+c3\nc1"
        );
        assert!(!r.path().join("f4.txt").exists());
    }
}
