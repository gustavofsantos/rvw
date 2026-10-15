//! Lane worktrees (M7): a lane is one linked worktree of a repository, on a branch of its own.
//! This module creates them under the rvw data directory, archives them with a checkpoint, and
//! counts their uncommitted work. It knows no database and no GUI state.
//!
//! Archiving keeps the branch and leaves the checkpoint refs of the removed worktree under
//! `refs/rvw/checkpoints/linked/<id>/` (see the checkpoint module). It does not use
//! `worktree::remove`: that deletes those refs, which would delete the archive checkpoint.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::checkpoint::{self, Checkpoint};
use crate::gitx::{GitError, exec, os_error_text, owned_args, run};
use crate::worktree::{self, Start, Worktree};

/// The directory of a lane: `data_dir/lanes/<project>/<repo>/<lane>`. The caller passes the rvw
/// data directory; nothing here reads the environment. The names are not checked: use
/// [`lane_path_checked`] for names that come from a file or a user.
pub fn lane_path(data_dir: &Path, project: &str, repo: &str, lane: &str) -> PathBuf {
    data_dir.join("lanes").join(project).join(repo).join(lane)
}

/// [`lane_path`], refusing a name that is empty, `.` or `..`, or that contains a path separator
/// or a NUL, so the result cannot leave `data_dir/lanes/<project>/<repo>/`.
pub fn lane_path_checked(
    data_dir: &Path,
    project: &str,
    repo: &str,
    lane: &str,
) -> Result<PathBuf, GitError> {
    for name in [project, repo, lane] {
        check_component(name)?;
    }
    Ok(lane_path(data_dir, project, repo, lane))
}

/// Refuses a path component that is not a single plain name.
fn check_component(name: &str) -> Result<(), GitError> {
    let bad = name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\', '\0']);
    if bad {
        return Err(GitError {
            args: owned_args(&["lane"]),
            stderr: format!("invalid lane path name '{name}'"),
        });
    }
    Ok(())
}

/// Adds a worktree at `path` on a new branch `name`, created at `at` (the repository's HEAD when
/// None). Parent directories of `path` are created. Refuses when a branch `name` already exists.
pub fn create(
    repo: &Path,
    path: &Path,
    name: &str,
    at: Option<&str>,
) -> Result<Worktree, GitError> {
    const OP: [&str; 2] = ["worktree", "add"];
    let full = format!("refs/heads/{name}");
    if exec(repo, &[], &["rev-parse", "--verify", "--quiet", &full])?.code == Some(0) {
        return Err(GitError {
            args: owned_args(&OP),
            stderr: format!("branch '{name}' already exists"),
        });
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| io_error(&OP, &e))?;
    }
    worktree::add(
        repo,
        path,
        &Start::NewBranch {
            name: name.to_owned(),
            at: at.map(str::to_owned),
        },
    )
}

/// The result of archiving a lane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Archived {
    /// The directory the lane had, as git reports it. The queue is found by this path.
    pub path: PathBuf,
    /// The branch the lane had; None when it was detached. The branch is never deleted.
    pub branch: Option<String>,
    /// The checkpoint taken before removal, labelled "archive <name>". Always set on success.
    pub checkpoint: Option<Checkpoint>,
}

/// Archives the lane at `path`: takes a checkpoint, then removes the worktree and keeps its
/// branch. A lane with changed or untracked files is refused unless `consent` is true; the
/// refusal counts them. With consent the worktree is removed even when dirty, the checkpoint
/// holding its files. Refuses the main worktree, the worktree holding `repo`, a locked one, and
/// one whose directory is gone.
pub fn archive(repo: &Path, path: &Path, consent: bool) -> Result<Archived, GitError> {
    const OP: [&str; 2] = ["worktree", "remove"];
    let target = repo.join(path);
    let listed = worktree::list(repo)?
        .into_iter()
        .find(|w| same_path(&w.path, &target))
        .ok_or_else(|| refuse(&OP, format!("{} is not a worktree", path.display())))?;
    if listed.main {
        return Err(refuse(&OP, "cannot archive the main worktree".to_owned()));
    }
    if listed.current {
        return Err(refuse(
            &OP,
            "cannot archive the worktree you are in".to_owned(),
        ));
    }
    if listed.locked {
        return Err(refuse(
            &OP,
            format!("worktree is locked: {}", path.display()),
        ));
    }
    if listed.prunable {
        return Err(refuse(
            &OP,
            format!("worktree directory is gone: {}", path.display()),
        ));
    }

    let (changed, untracked) = dirty(&listed.path)?;
    let dirty_count = changed + untracked;
    if dirty_count > 0 && !consent {
        return Err(refuse(
            &OP,
            format!(
                "uncommitted work would be lost: {changed} changed and {untracked} untracked \
                 file(s); archive with consent to checkpoint and remove them"
            ),
        ));
    }

    let name = listed
        .branch
        .clone()
        .or_else(|| {
            listed
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
        })
        .unwrap_or_default();
    let checkpoint = checkpoint::create(&listed.path, &format!("archive {name}"), &[])?;

    let text = listed
        .path
        .to_str()
        .ok_or_else(|| refuse(&OP, format!("path is not UTF-8: {}", listed.path.display())))?;
    let mut args = vec!["worktree", "remove"];
    if dirty_count > 0 {
        args.push("--force");
    }
    args.push(text);
    run(repo, &[], &args)?;

    Ok(Archived {
        path: listed.path,
        branch: listed.branch,
        checkpoint: Some(checkpoint),
    })
}

/// The number of changed (tracked, staged or not) and untracked files of the worktree at `path`.
/// Ignored files are not counted.
pub fn dirty(path: &Path) -> Result<(usize, usize), GitError> {
    let out = run(
        path,
        &[],
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--no-renames",
            "--untracked-files=all",
        ],
    )?;
    let mut changed = 0;
    let mut untracked = 0;
    for entry in out.split('\0').filter(|entry| !entry.is_empty()) {
        if entry.starts_with("??") {
            untracked += 1;
        } else {
            changed += 1;
        }
    }
    Ok((changed, untracked))
}

fn refuse(op: &[&str], stderr: String) -> GitError {
    GitError {
        args: owned_args(op),
        stderr,
    }
}

fn io_error(op: &[&str], e: &io::Error) -> GitError {
    GitError {
        args: owned_args(op),
        stderr: os_error_text(e),
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    canonical(a) == canonical(b)
}

/// The path with symlinks resolved, or the path itself when that fails (a missing directory).
fn canonical(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use tempfile::TempDir;

    use super::*;
    use crate::testrepo::{commit_all, git, repo, write};

    /// A repository with one commit on main, `a.txt` = "a".
    fn committed() -> TempDir {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "init");
        r
    }

    fn head_of(dir: &Path) -> String {
        git(dir, &["rev-parse", "HEAD"]).trim().to_owned()
    }

    /// The ids of the checkpoint refs under `refs/rvw/checkpoints/linked/`.
    fn linked_refs(dir: &Path) -> Vec<String> {
        git(
            dir,
            &[
                "for-each-ref",
                "--format=%(refname)",
                "refs/rvw/checkpoints/linked/",
            ],
        )
        .lines()
        .map(str::to_owned)
        .collect()
    }

    #[test]
    fn lane_path_joins_the_data_directory_and_checks_names() {
        let data = Path::new("/data/rvw");
        assert_eq!(
            lane_path(data, "proj", "repo", "feat"),
            PathBuf::from("/data/rvw/lanes/proj/repo/feat")
        );
        assert_eq!(
            lane_path_checked(data, "proj", "repo", "feat").unwrap(),
            PathBuf::from("/data/rvw/lanes/proj/repo/feat")
        );
    }

    #[test]
    fn lane_path_checked_refuses_a_name_that_escapes() {
        let data = Path::new("/data/rvw");
        let bad = [
            ("..", "repo", "lane"),
            ("proj", "..", "lane"),
            ("proj", "repo", ".."),
            ("proj", "repo", "."),
            ("proj", "", "lane"),
            ("proj", "re/po", "lane"),
            ("proj", "repo", "../../etc"),
            ("/abs", "repo", "lane"),
            ("proj", "re\\po", "lane"),
            ("proj", "repo", "bad\0name"),
        ];
        for (project, repo_name, lane) in bad {
            let err = lane_path_checked(data, project, repo_name, lane).unwrap_err();
            assert!(err.stderr.starts_with("invalid lane path name"), "{err}");
        }
    }

    #[test]
    fn create_adds_a_branch_at_head_and_makes_the_parents() {
        let r = committed();
        let parent = TempDir::new().unwrap();
        let path = parent.path().join("lanes/proj/repo/feat");
        let head = head_of(r.path());

        let wt = create(r.path(), &path, "feat", None).unwrap();

        assert!(path.join("a.txt").exists());
        assert_eq!(wt.branch.as_deref(), Some("feat"));
        assert_eq!(wt.head.as_deref(), Some(head.as_str()));
        assert_eq!(
            git(r.path(), &["rev-parse", "refs/heads/feat"]).trim(),
            head
        );
    }

    #[test]
    fn create_at_a_branch_starts_the_new_branch_there() {
        let r = committed();
        git(r.path(), &["branch", "base"]);
        let base = head_of(r.path());
        write(r.path(), "b.txt", "b\n");
        commit_all(r.path(), "two");
        let parent = TempDir::new().unwrap();
        let path = parent.path().join("feat");

        let wt = create(r.path(), &path, "feat", Some("base")).unwrap();

        assert_eq!(wt.branch.as_deref(), Some("feat"));
        assert_eq!(wt.head.as_deref(), Some(base.as_str()));
        assert!(!path.join("b.txt").exists());
    }

    #[test]
    fn create_refuses_an_existing_branch_and_makes_nothing() {
        let r = committed();
        let parent = TempDir::new().unwrap();
        let path = parent.path().join("lane");

        let err = create(r.path(), &path, "main", None).unwrap_err();

        assert!(err.stderr.contains("already exists"), "{err}");
        assert!(!path.exists());
        assert_eq!(worktree::list(r.path()).unwrap().len(), 1);
    }

    #[test]
    fn dirty_counts_changed_and_untracked_files() {
        let r = committed();
        assert_eq!(dirty(r.path()).unwrap(), (0, 0));

        write(r.path(), "a.txt", "changed\n");
        write(r.path(), "new/one.txt", "1\n");
        write(r.path(), "new/two.txt", "2\n");
        assert_eq!(dirty(r.path()).unwrap(), (1, 2));
    }

    #[test]
    fn archive_a_clean_lane_checkpoints_removes_it_and_keeps_the_branch() {
        let r = committed();
        let parent = TempDir::new().unwrap();
        let path = parent.path().join("feat");
        create(r.path(), &path, "feat", None).unwrap();

        let archived = archive(r.path(), &path, false).unwrap();

        assert!(!path.exists());
        assert_eq!(archived.branch.as_deref(), Some("feat"));
        let cp = archived.checkpoint.expect("a checkpoint");
        assert_eq!(cp.label, "archive feat");
        assert_eq!(worktree::list(r.path()).unwrap().len(), 1);
        assert_eq!(
            git(r.path(), &["rev-parse", "--verify", "refs/heads/feat"]).trim(),
            head_of(r.path())
        );
        assert_eq!(linked_refs(r.path()).len(), 1);
    }

    #[test]
    fn archive_refuses_a_dirty_lane_without_consent_and_names_the_counts() {
        let r = committed();
        let parent = TempDir::new().unwrap();
        let path = parent.path().join("feat");
        create(r.path(), &path, "feat", None).unwrap();
        write(&path, "a.txt", "changed\n");
        write(&path, "u.txt", "u\n");

        let err = archive(r.path(), &path, false).unwrap_err();

        assert!(err.stderr.contains("1 changed"), "{err}");
        assert!(err.stderr.contains("1 untracked"), "{err}");
        assert!(path.join("u.txt").exists());
        assert_eq!(worktree::list(r.path()).unwrap().len(), 2);
        assert!(linked_refs(r.path()).is_empty());
    }

    #[test]
    fn archive_a_dirty_lane_with_consent_checkpoints_its_files_and_keeps_the_branch() {
        let r = committed();
        let parent = TempDir::new().unwrap();
        let path = parent.path().join("feat");
        create(r.path(), &path, "feat", None).unwrap();
        write(&path, "a.txt", "changed\n");
        write(&path, "u.txt", "u\n");

        let archived = archive(r.path(), &path, true).unwrap();

        assert!(!path.exists());
        assert_eq!(worktree::list(r.path()).unwrap().len(), 1);
        assert_eq!(archived.branch.as_deref(), Some("feat"));
        assert_eq!(
            git(r.path(), &["rev-parse", "--verify", "refs/heads/feat"]).trim(),
            head_of(r.path())
        );
        let cp = archived.checkpoint.expect("a checkpoint");
        assert_eq!(cp.label, "archive feat");
        assert_eq!(
            git(r.path(), &["show", &format!("{}:a.txt", cp.worktree_tree)]),
            "changed\n"
        );
        assert_eq!(
            git(r.path(), &["show", &format!("{}:u.txt", cp.worktree_tree)]),
            "u\n"
        );
        // The checkpoint ref outlives the removed worktree.
        assert_eq!(linked_refs(r.path()).len(), 1);
    }

    #[test]
    fn archive_refuses_the_main_worktree_and_a_path_that_is_not_one() {
        let r = committed();
        let parent = TempDir::new().unwrap();

        let main_err = archive(r.path(), r.path(), true).unwrap_err();
        assert!(main_err.stderr.contains("main worktree"), "{main_err}");
        let missing = archive(r.path(), &parent.path().join("nope"), true).unwrap_err();
        assert!(missing.stderr.contains("not a worktree"), "{missing}");
        assert_eq!(worktree::list(r.path()).unwrap().len(), 1);
    }
}
