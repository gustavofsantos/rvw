//! Linked worktrees: list, add and remove. A worktree is rvw's unit of queue,
//! so each one is a workspace of its own.

use std::fs;
use std::path::{Path, PathBuf};

use crate::gitx::{self, os_error_text, owned_args, run, GitError};

/// The common root of a linked worktree's checkpoint refs; see the checkpoint module.
const LINKED: &str = "refs/rvw/checkpoints/linked/";

/// One worktree of the repository.
// reason: each bool is one independent git flag of the entry; the public shape is fixed.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    /// Its top directory, as git reports it.
    pub path: PathBuf,
    /// The commit checked out; None in a repository with no commits yet.
    pub head: Option<String>,
    /// The branch checked out, short name ("main"); None when detached.
    pub branch: Option<String>,
    /// This is the main worktree (the first git lists).
    pub main: bool,
    /// The worktree holding `dir` in the call that listed it.
    pub current: bool,
    pub locked: bool,
    /// git says its directory is gone and `git worktree prune` would remove it.
    pub prunable: bool,
}

/// What a new worktree checks out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Start {
    /// An existing local branch not checked out elsewhere.
    Branch(String),
    /// A new branch `name` created at `at` (HEAD when None).
    NewBranch { name: String, at: Option<String> },
    /// A detached HEAD at a revision.
    Detached(String),
}

/// Every worktree, the main one first, in git's order. Bare entries are skipped.
/// Fails when git cannot list the worktrees of the repository at `dir`.
pub fn list(dir: &Path) -> Result<Vec<Worktree>, GitError> {
    let out = run(dir, &[], &["worktree", "list", "--porcelain", "-z"])?;
    let here = gitx::toplevel(dir).map(|top| canonical(&top));
    let worktrees = parse(&out)
        .into_iter()
        .enumerate()
        .filter(|(_, record)| !record.bare)
        .map(|(index, record)| {
            let current = here.as_deref() == Some(canonical(&record.path).as_path());
            Worktree {
                path: record.path,
                head: record.head,
                branch: record.branch,
                main: index == 0,
                current,
                locked: record.locked,
                prunable: record.prunable,
            }
        })
        .collect();
    Ok(worktrees)
}

/// Adds a worktree at `path` (which must not exist or be an empty directory) and returns it as
/// `list` would. Fails when a name or revision starts with `-`, or when git refuses the add (a
/// branch already checked out elsewhere, a path in use).
pub fn add(dir: &Path, path: &Path, start: &Start) -> Result<Worktree, GitError> {
    const OP: [&str; 2] = ["worktree", "add"];
    let target = path_arg(&OP, path)?;
    let mut args = vec!["worktree", "add"];
    match start {
        Start::Branch(name) => {
            check_name(&OP, name)?;
            args.extend([target, name.as_str()]);
        }
        Start::NewBranch { name, at } => {
            check_name(&OP, name)?;
            args.extend(["-b", name.as_str(), target]);
            if let Some(at) = at {
                check_rev(&OP, at)?;
                args.push(at.as_str());
            }
        }
        Start::Detached(rev) => {
            check_rev(&OP, rev)?;
            args.extend(["--detach", target, rev.as_str()]);
        }
    }
    run(dir, &[], &args)?;
    let wanted = dir.join(path);
    list(dir)?
        .into_iter()
        .find(|worktree| same_path(&worktree.path, &wanted))
        .ok_or_else(|| {
            refuse(
                &OP,
                format!("worktree at {} is not listed after add", path.display()),
            )
        })
}

/// Removes the linked worktree at `path` and deletes its checkpoint refs. Refuses the main
/// worktree, the worktree holding `dir`, and a locked one. A worktree git cannot remove (one
/// with changes or untracked files) keeps its refs and git's error comes back. A prunable
/// worktree (its directory already gone) is pruned instead. Fails when `path` is not a worktree.
pub fn remove(dir: &Path, path: &Path) -> Result<(), GitError> {
    const OP: [&str; 2] = ["worktree", "remove"];
    let wanted = dir.join(path);
    let worktree = list(dir)?
        .into_iter()
        .find(|worktree| same_path(&worktree.path, &wanted))
        .ok_or_else(|| refuse(&OP, format!("{} is not a worktree", path.display())))?;
    if worktree.main {
        return Err(refuse(&OP, "cannot remove the main worktree".to_owned()));
    }
    if worktree.current {
        return Err(refuse(
            &OP,
            "cannot remove the worktree you are in".to_owned(),
        ));
    }
    if worktree.locked {
        return Err(refuse(
            &OP,
            format!("worktree is locked: {}", path.display()),
        ));
    }
    // The id names the checkpoint refs, so find it before git removes the worktree.
    let id = if worktree.prunable {
        id_from_gitdir_files(&common_dir(dir)?, &worktree.path)?
    } else {
        linked_id(&worktree.path)?
    };
    if worktree.prunable {
        run(dir, &[], &["worktree", "prune"])?;
    } else {
        let target = path_arg(&OP, &worktree.path)?;
        run(dir, &[], &["worktree", "remove", target])?;
    }
    if let Some(id) = id {
        delete_linked_refs(dir, &id)?;
    }
    Ok(())
}

/// One record of `git worktree list --porcelain -z`, bare entries included.
#[derive(Default)]
struct Record {
    path: PathBuf,
    head: Option<String>,
    branch: Option<String>,
    bare: bool,
    locked: bool,
    prunable: bool,
}

/// Splits the NUL-separated fields of `git worktree list --porcelain -z`. An empty field
/// ends a record.
fn parse(out: &str) -> Vec<Record> {
    let mut records = Vec::new();
    let mut record: Option<Record> = None;
    for field in out.split('\0') {
        if field.is_empty() {
            records.extend(record.take());
            continue;
        }
        let rec = record.get_or_insert_with(Record::default);
        if let Some(path) = field.strip_prefix("worktree ") {
            rec.path = PathBuf::from(path);
        } else if let Some(head) = field.strip_prefix("HEAD ") {
            // All zeros is an unborn branch: no commit yet.
            rec.head = Some(head.to_owned()).filter(|id| !id.bytes().all(|b| b == b'0'));
        } else if let Some(branch) = field.strip_prefix("branch ") {
            rec.branch = Some(
                branch
                    .strip_prefix("refs/heads/")
                    .unwrap_or(branch)
                    .to_owned(),
            );
        } else if field == "bare" {
            rec.bare = true;
        } else if field == "locked" || field.starts_with("locked ") {
            rec.locked = true;
        } else if field == "prunable" || field.starts_with("prunable ") {
            rec.prunable = true;
        }
    }
    records.extend(record.take());
    records
}

/// The id of the linked worktree whose git directory is `worktree/.git` (`.git/worktrees/<id>`).
fn linked_id(worktree: &Path) -> Result<Option<String>, GitError> {
    let git_dir = run(worktree, &[], &["rev-parse", "--absolute-git-dir"])?;
    Ok(id_from_git_dir(Path::new(git_dir.trim_end())))
}

/// The name of a git directory under `<common>/worktrees/`, or None for any other directory.
fn id_from_git_dir(git_dir: &Path) -> Option<String> {
    let under_worktrees = git_dir
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        == Some("worktrees");
    if !under_worktrees {
        return None;
    }
    git_dir.file_name()?.to_str().map(str::to_owned)
}

/// The id of a worktree whose directory is gone: the entry under `<common>/worktrees/` whose
/// `gitdir` file points at `<worktree>/.git`.
fn id_from_gitdir_files(common: &Path, worktree: &Path) -> Result<Option<String>, GitError> {
    let root = common.join("worktrees");
    for entry in fs::read_dir(&root).map_err(|e| fs_error(&e))? {
        let entry = entry.map_err(|e| fs_error(&e))?;
        let Ok(text) = fs::read_to_string(entry.path().join("gitdir")) else {
            continue;
        };
        let dot_git = Path::new(text.trim_end());
        if dot_git
            .parent()
            .is_some_and(|parent| same_path(parent, worktree))
        {
            return Ok(entry.file_name().into_string().ok());
        }
    }
    Ok(None)
}

/// The common git directory of the repository at `dir`.
fn common_dir(dir: &Path) -> Result<PathBuf, GitError> {
    let out = run(dir, &[], &["rev-parse", "--git-common-dir"])?;
    Ok(dir.join(out.trim_end()))
}

/// Deletes every checkpoint ref of the linked worktree `id`, in one git process.
fn delete_linked_refs(dir: &Path, id: &str) -> Result<(), GitError> {
    let prefix = format!("{LINKED}{id}/");
    let out = run(
        dir,
        &[],
        &["for-each-ref", "--format=%(refname)", prefix.as_str()],
    )?;
    let mut input = String::new();
    for name in out.lines().filter(|name| name.starts_with(prefix.as_str())) {
        input.push_str("delete ");
        input.push_str(name);
        input.push('\n');
    }
    if !input.is_empty() {
        run(dir, input.as_bytes(), &["update-ref", "--stdin"])?;
    }
    Ok(())
}

/// Rejects a branch name git would read as an option, or an empty one. Git checks the rest.
fn check_name(op: &[&str], name: &str) -> Result<(), GitError> {
    if name.is_empty() || name.starts_with('-') {
        return Err(refuse(op, format!("invalid branch name '{name}'")));
    }
    Ok(())
}

/// Rejects a revision git would read as an option, or an empty one.
fn check_rev(op: &[&str], rev: &str) -> Result<(), GitError> {
    if rev.is_empty() || rev.starts_with('-') {
        return Err(refuse(op, format!("invalid revision '{rev}'")));
    }
    Ok(())
}

/// The path as an argument for git: UTF-8, and not read as an option.
fn path_arg<'a>(op: &[&str], path: &'a Path) -> Result<&'a str, GitError> {
    match path.to_str() {
        Some(text) if !text.is_empty() && !text.starts_with('-') => Ok(text),
        _ => Err(refuse(
            op,
            format!("invalid worktree path '{}'", path.display()),
        )),
    }
}

/// A refusal made before git runs, reported like a git failure.
fn refuse(op: &[&str], stderr: String) -> GitError {
    GitError {
        args: owned_args(op),
        stderr,
    }
}

/// A file error while reading git's own state, reported as a git failure.
fn fs_error(e: &std::io::Error) -> GitError {
    GitError {
        args: owned_args(&["worktree", "remove"]),
        stderr: os_error_text(e),
    }
}

/// The path with symlinks resolved, or the path itself when that fails (a missing directory).
fn canonical(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn same_path(a: &Path, b: &Path) -> bool {
    canonical(a) == canonical(b)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use tempfile::TempDir;

    use super::*;
    use crate::testrepo::{commit_all, git, repo, write};

    /// A repository with one commit on main.
    fn committed() -> TempDir {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "init");
        r
    }

    /// The checkpoint id of the linked worktree at `worktree`.
    fn id_of(worktree: &Path) -> String {
        let git_dir = git(worktree, &["rev-parse", "--absolute-git-dir"]);
        id_from_git_dir(Path::new(git_dir.trim_end())).unwrap()
    }

    /// The full names of the refs under `prefix`.
    fn refs_under(dir: &Path, prefix: &str) -> Vec<String> {
        git(dir, &["for-each-ref", "--format=%(refname)", prefix])
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn find<'a>(all: &'a [Worktree], path: &Path) -> &'a Worktree {
        all.iter()
            .find(|w| same_path(&w.path, path))
            .unwrap_or_else(|| panic!("no worktree at {}", path.display()))
    }

    #[test]
    fn a_fresh_repository_lists_only_the_main_worktree() {
        let r = committed();
        let all = list(r.path()).unwrap();
        assert_eq!(all.len(), 1, "{all:?}");
        let main = &all[0];
        assert!(main.main && main.current && !main.locked && !main.prunable);
        assert_eq!(main.branch.as_deref(), Some("main"));
        let head = git(r.path(), &["rev-parse", "HEAD"]);
        assert_eq!(main.head.as_deref(), Some(head.trim()));
        assert!(same_path(&main.path, r.path()));
    }

    #[test]
    fn a_repository_without_commits_lists_its_main_worktree_with_no_head() {
        let r = repo();
        let all = list(r.path()).unwrap();
        assert_eq!(all.len(), 1, "{all:?}");
        assert_eq!(all[0].head, None);
        assert_eq!(all[0].branch.as_deref(), Some("main"));
    }

    #[test]
    fn add_creates_a_worktree_for_each_kind_of_start() {
        let r = committed();
        git(r.path(), &["branch", "feature"]);
        let parent = TempDir::new().unwrap();
        let new_path = parent.path().join("new");
        let feature_path = parent.path().join("feature");
        let detached_path = parent.path().join("detached");
        let added = add(
            r.path(),
            &new_path,
            &Start::NewBranch {
                name: "new".to_owned(),
                at: None,
            },
        )
        .unwrap();
        add(
            r.path(),
            &feature_path,
            &Start::Branch("feature".to_owned()),
        )
        .unwrap();
        add(
            r.path(),
            &detached_path,
            &Start::Detached("HEAD".to_owned()),
        )
        .unwrap();

        let all = list(r.path()).unwrap();
        assert_eq!(all.len(), 4, "{all:?}");
        assert_eq!(find(&all, &new_path).branch.as_deref(), Some("new"));
        assert_eq!(find(&all, &feature_path).branch.as_deref(), Some("feature"));
        let detached = find(&all, &detached_path);
        assert_eq!(detached.branch, None);
        assert!(!detached.main);
        assert_eq!(added, *find(&all, &new_path));
        assert!(!added.current);

        let inside = list(&new_path).unwrap();
        assert!(find(&inside, &new_path).current);
        assert_eq!(inside.iter().filter(|w| w.current).count(), 1);
        assert!(!find(&inside, r.path()).current);
    }

    #[test]
    fn remove_deletes_a_clean_linked_worktree_and_its_checkpoint_refs() {
        let r = committed();
        let parent = TempDir::new().unwrap();
        let path = parent.path().join("linked");
        add(
            r.path(),
            &path,
            &Start::NewBranch {
                name: "linked".to_owned(),
                at: None,
            },
        )
        .unwrap();
        let id = id_of(&path);
        let linked_ref = format!("refs/rvw/checkpoints/linked/{id}/x");
        git(r.path(), &["update-ref", linked_ref.as_str(), "HEAD"]);
        git(
            r.path(),
            &["update-ref", "refs/rvw/checkpoints/main/y", "HEAD"],
        );

        remove(r.path(), &path).unwrap();

        assert_eq!(list(r.path()).unwrap().len(), 1);
        assert!(!path.exists());
        assert!(refs_under(r.path(), &format!("refs/rvw/checkpoints/linked/{id}/")).is_empty());
        assert_eq!(
            refs_under(r.path(), "refs/rvw/checkpoints/main/"),
            vec!["refs/rvw/checkpoints/main/y".to_owned()]
        );
    }

    #[test]
    fn remove_refuses_the_main_the_current_a_locked_and_a_dirty_worktree() {
        let r = committed();
        let parent = TempDir::new().unwrap();
        let path = parent.path().join("linked");
        add(
            r.path(),
            &path,
            &Start::NewBranch {
                name: "linked".to_owned(),
                at: None,
            },
        )
        .unwrap();
        let id = id_of(&path);
        let linked_ref = format!("refs/rvw/checkpoints/linked/{id}/x");
        git(r.path(), &["update-ref", linked_ref.as_str(), "HEAD"]);
        let path_text = path.to_str().unwrap();

        let main_err = remove(r.path(), r.path()).unwrap_err();
        assert!(main_err.stderr.contains("main worktree"), "{main_err}");
        let current_err = remove(&path, &path).unwrap_err();
        assert!(current_err.stderr.contains("you are in"), "{current_err}");

        git(r.path(), &["worktree", "lock", path_text]);
        let locked_err = remove(r.path(), &path).unwrap_err();
        assert!(locked_err.stderr.contains("locked"), "{locked_err}");
        git(r.path(), &["worktree", "unlock", path_text]);

        write(&path, "untracked.txt", "left behind\n");
        let dirty_err = remove(r.path(), &path).unwrap_err();
        assert!(dirty_err.stderr.contains("untracked"), "{dirty_err}");

        assert_eq!(list(r.path()).unwrap().len(), 2);
        assert!(path.exists());
        assert_eq!(
            refs_under(r.path(), &format!("refs/rvw/checkpoints/linked/{id}/")),
            vec![linked_ref]
        );
    }

    #[test]
    fn remove_prunes_a_worktree_whose_directory_is_gone_and_deletes_its_refs() {
        let r = committed();
        let parent = TempDir::new().unwrap();
        let path = parent.path().join("linked");
        add(
            r.path(),
            &path,
            &Start::NewBranch {
                name: "linked".to_owned(),
                at: None,
            },
        )
        .unwrap();
        let id = id_of(&path);
        let linked_ref = format!("refs/rvw/checkpoints/linked/{id}/x");
        git(r.path(), &["update-ref", linked_ref.as_str(), "HEAD"]);
        fs::remove_dir_all(&path).unwrap();
        let gone = list(r.path())
            .unwrap()
            .into_iter()
            .find(|w| w.prunable)
            .unwrap();
        assert!(same_path(&gone.path, &path));

        remove(r.path(), &path).unwrap();

        assert_eq!(list(r.path()).unwrap().len(), 1);
        assert!(refs_under(r.path(), &format!("refs/rvw/checkpoints/linked/{id}/")).is_empty());
    }

    #[test]
    fn add_refuses_a_name_or_revision_starting_with_a_dash() {
        let r = committed();
        let parent = TempDir::new().unwrap();
        let path = parent.path().join("wt");
        let starts = [
            Start::Branch("-x".to_owned()),
            Start::NewBranch {
                name: "-x".to_owned(),
                at: None,
            },
            Start::NewBranch {
                name: "ok".to_owned(),
                at: Some("-x".to_owned()),
            },
            Start::Detached("-x".to_owned()),
        ];
        for start in &starts {
            let err = add(r.path(), &path, start).unwrap_err();
            assert!(err.stderr.contains("invalid"), "{err}");
        }
        assert!(!path.exists());
    }

    #[test]
    fn parse_reads_a_zero_head_locked_bare_and_prunable_entries() {
        let out = "worktree /a\0HEAD 0000000000000000000000000000000000000000\0branch refs/heads/main\0\0\
                   worktree /b\0HEAD abc\0detached\0locked because\0\0\
                   worktree /c\0bare\0\0\
                   worktree /d\0HEAD def\0branch refs/heads/x\0prunable gitdir file points nowhere\0\0";
        let records = parse(out);
        assert_eq!(records.len(), 4);
        assert_eq!(records[0].head, None);
        assert_eq!(records[0].branch.as_deref(), Some("main"));
        assert_eq!(records[1].head.as_deref(), Some("abc"));
        assert_eq!(records[1].branch, None);
        assert!(records[1].locked);
        assert!(records[2].bare);
        assert!(records[3].prunable);
        assert_eq!(records[3].path, PathBuf::from("/d"));
    }
}
