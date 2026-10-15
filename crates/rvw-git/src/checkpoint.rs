//! Checkpoints: the whole state of a worktree (its branch and commit, the index,
//! and every file that is not ignored) saved as git objects and put back exactly.
//! Restoring first saves the current state, so a restore can be reversed too. Ignored
//! files are not saved and not touched, except ones a restore overwrites: the safety
//! checkpoint keeps those.
//!
//! Checkpoint refs live in the common namespace, `refs/rvw/checkpoints/`: under `main/`
//! for the main worktree and `linked/<id>/` for a linked worktree, so each worktree
//! lists only its own. They are deliberately not `refs/worktree/`: git does not treat
//! another worktree's per-worktree refs as reachable, so `gc --prune=now` run in one
//! worktree deleted the checkpoints of the others.
//!
//! The refs of a removed linked worktree stay under `linked/<id>/` until something
//! deletes them with `git update-ref -d`. There is no orphan cleanup, and `prune` only
//! runs in a worktree that still exists.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::gitx::{self, GitError, exec, owned_args, run, run_env};
use crate::stash;

/// The common root of every checkpoint ref; see the module docs for the layout.
const ROOT: &str = "refs/rvw/checkpoints/";

/// The identity of every object rvw writes, so checkpoints do not depend on the
/// user's configuration.
const IDENTITY: [(&str, &str); 4] = [
    ("GIT_AUTHOR_NAME", "rvw"),
    ("GIT_AUTHOR_EMAIL", "rvw@localhost"),
    ("GIT_COMMITTER_NAME", "rvw"),
    ("GIT_COMMITTER_EMAIL", "rvw@localhost"),
];

/// Git state files that mean an operation is half done. Their presence refuses
/// both saving and restoring, since the index cannot be written as a tree.
const IN_PROGRESS: [&str; 5] = [
    "MERGE_HEAD",
    "CHERRY_PICK_HEAD",
    "REVERT_HEAD",
    "rebase-merge",
    "rebase-apply",
];

/// The directories of one worktree: its top, its own git directory (`.git/worktrees/<id>` for
/// a linked worktree, where the index and the merge state live), and the common git directory
/// that holds the objects and the refs. One git process answers for all three.
struct Dirs {
    top: PathBuf,
    git: PathBuf,
    common: PathBuf,
}

impl Dirs {
    fn of(dir: &Path) -> Result<Self, GitError> {
        let args = [
            "rev-parse",
            "--absolute-git-dir",
            "--git-common-dir",
            "--show-toplevel",
        ];
        let out = run(dir, &[], &args)?;
        let mut lines = out.lines();
        match (lines.next(), lines.next(), lines.next()) {
            (Some(git), Some(common), Some(top)) => Ok(Self {
                top: PathBuf::from(top),
                git: PathBuf::from(git),
                common: dir.join(common),
            }),
            _ => Err(GitError {
                args: owned_args(&args),
                stderr: format!("unexpected output: {out}"),
            }),
        }
    }
}

/// Something the action after a checkpoint deletes, kept so restore can recreate it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kept {
    /// A branch or tag: its full ref name (`refs/heads/x`, `refs/tags/v1`) and what it pointed at.
    Ref { name: String, target: String },
    /// A stash entry: its commit and git's subject.
    Stash { commit: String, message: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkpoint {
    /// The checkpoint commit's id.
    pub id: String,
    pub label: String,
    /// When it was taken: seconds since the Unix epoch (the commit's committer time).
    pub at: i64,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub index_tree: String,
    pub worktree_tree: String,
    pub kept: Vec<Kept>,
}

/// What restoring `checkpoint` would change, for the confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preview {
    /// Files whose content differs between the worktree now and the checkpoint: git's
    /// status letter and path, from `git diff --name-status --no-renames <now_tree>
    /// <checkpoint_tree>`.
    pub files: Vec<(char, String)>,
    /// The staged state differs too.
    pub index_differs: bool,
    /// The branch now and the branch restore checks out (None = detached).
    pub branch_now: Option<String>,
    pub branch_then: Option<String>,
    /// Commits the restored branch has now that the checkpoint's commit does not: they
    /// leave the branch (they stay in the safety checkpoint).
    pub commits_left_out: usize,
    /// Kept refs and stashes that are missing now and will be recreated.
    pub recreated: Vec<String>,
    /// Ignored files the restore overwrites; their current content is kept in the safety
    /// checkpoint.
    pub overwritten_ignored: Vec<String>,
}

/// Saves the current state as a checkpoint labelled `label`, keeping `kept`. When nothing
/// differs from the newest checkpoint (same branch, head, both trees, and `kept` is empty),
/// returns that one and writes nothing.
pub fn create(dir: &Path, label: &str, kept: &[Kept]) -> Result<Checkpoint, GitError> {
    let dirs = Dirs::of(dir)?;
    create_with(dir, &dirs, label, kept, &[])
}

/// `create`, also forcing the ignored `forced` paths into the worktree tree. Restore uses it
/// so the safety checkpoint keeps the ignored files that the restore overwrites.
fn create_with(
    dir: &Path,
    dirs: &Dirs,
    label: &str,
    kept: &[Kept],
    forced: &[String],
) -> Result<Checkpoint, GitError> {
    refuse_in_progress(dirs, "write-tree")?;
    let (prefix, refs) = own_refs(dir, dirs)?;
    let worktree_tree = working_tree(dirs, forced)?;
    let index_tree = staged_tree(dirs)?;
    let branch = current_branch(dir)?;
    let head = gitx::commit(dir, "HEAD");

    let newest = if kept.is_empty() {
        newest_of(dir, &refs)?
    } else {
        None
    };
    if let Some(same) = newest.filter(|c| {
        c.branch == branch
            && c.head == head
            && c.index_tree == index_tree
            && c.worktree_tree == worktree_tree
    }) {
        return Ok(same);
    }

    let label = one_line(label);
    let index_commit = run_env(
        dir,
        &[],
        &[
            "commit-tree",
            "--no-gpg-sign",
            &index_tree,
            "-m",
            "rvw checkpoint index",
        ],
        &IDENTITY,
    )?
    .trim()
    .to_owned();

    let mut parents = vec![index_commit];
    if let Some(head) = &head {
        parents.push(head.clone());
    }
    let mut lines = vec![
        format!("rvw checkpoint: {label}"),
        String::new(),
        format!("branch: {}", branch.as_deref().unwrap_or("-")),
        format!("head: {}", head.as_deref().unwrap_or("-")),
        format!("index: {index_tree}"),
    ];
    for entry in kept {
        // A kept commit becomes a parent so gc keeps it. A tag's peeled commit
        // is the parent; the tag object itself is only named in the message.
        let (target, line) = match entry {
            Kept::Ref { name, target } => (target, format!("keep: ref {name} {target}")),
            Kept::Stash { commit, message } => (
                commit,
                format!("keep: stash {commit} {}", one_line(message)),
            ),
        };
        let parent = gitx::commit(dir, target).ok_or_else(|| GitError {
            args: owned_args(&["commit-tree"]),
            stderr: format!("cannot keep {target}: it is gone"),
        })?;
        if !parents.contains(&parent) {
            parents.push(parent);
        }
        lines.push(line);
    }
    let message = format!("{}\n", lines.join("\n"));

    let mut args = vec!["commit-tree", "--no-gpg-sign", worktree_tree.as_str()];
    for parent in &parents {
        args.push("-p");
        args.push(parent);
    }
    args.extend(["-F", "-"]);
    let id = run_env(dir, message.as_bytes(), &args, &IDENTITY)?
        .trim()
        .to_owned();

    // Names sort by time, and a clock that stands still or runs back still
    // gives each new checkpoint a higher number than the newest one.
    let last = refs
        .first()
        .and_then(|(name, _)| name.strip_prefix(prefix.as_str())?.parse::<u64>().ok())
        .unwrap_or(0);
    let name = format!("{prefix}{:020}", now_nanos().max(last + 1));
    run(dir, &[], &["update-ref", &name, &id, ""])?;
    read(dir, &id)
}

/// The worktree's checkpoints, newest first.
pub fn list(dir: &Path) -> Result<Vec<Checkpoint>, GitError> {
    let dirs = Dirs::of(dir)?;
    let (_, refs) = own_refs(dir, &dirs)?;
    let ids: Vec<&str> = refs.iter().map(|(_, id)| id.as_str()).collect();
    read_all(dir, &ids)
}

/// Reads many checkpoints with one git process, in the order given: the list
/// holds up to the pruning cap, and it is read on the UI thread.
fn read_all(dir: &Path, ids: &[&str]) -> Result<Vec<Checkpoint>, GitError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut args = vec![
        "log",
        "--no-walk=unsorted",
        "--format=%H%x00%T%x00%ct%x00%B%x1e",
    ];
    args.extend_from_slice(ids);
    let out = run(dir, &[], &args)?;
    out.split('\x1e')
        .map(|record| record.trim_start_matches('\n'))
        .filter(|record| !record.is_empty())
        .map(|record| {
            let mut fields = record.splitn(4, '\0');
            let (id, tree, at, body) = (
                fields.next().unwrap_or_default(),
                fields.next().unwrap_or_default(),
                fields.next().unwrap_or_default(),
                fields.next().unwrap_or_default(),
            );
            // The same shape `cat-file commit` gives, so one parser reads both.
            let text = format!("tree {tree}\ncommitter rvw <rvw@localhost> {at} +0000\n\n{body}");
            parse(id, &text).ok_or_else(|| GitError {
                args: owned_args(&args),
                stderr: format!("not a checkpoint: {id}"),
            })
        })
        .collect()
}

/// What `restore` would change.
pub fn preview(dir: &Path, checkpoint: &Checkpoint) -> Result<Preview, GitError> {
    let dirs = Dirs::of(dir)?;
    refuse_in_progress(&dirs, "write-tree")?;
    let now_tree = working_tree(&dirs, &[])?;
    let index_now = staged_tree(&dirs)?;
    let files = name_status(dir, &now_tree, &checkpoint.worktree_tree)?;
    let recreated = missing(dir, &checkpoint.kept)?
        .into_iter()
        .map(|entry| match entry {
            Kept::Ref { name, .. } => name.clone(),
            Kept::Stash { message, .. } => format!("stash {message}"),
        })
        .collect();
    Ok(Preview {
        files,
        index_differs: index_now != checkpoint.index_tree,
        branch_now: current_branch(dir)?,
        branch_then: checkpoint.branch.clone(),
        commits_left_out: commits_left_out(dir, checkpoint)?,
        recreated,
        overwritten_ignored: overwritten(&dirs, checkpoint)?,
    })
}

/// Puts the worktree back exactly as `checkpoint` saved it, after saving the current state
/// as "Before restoring “<label>”". Returns that safety checkpoint.
pub fn restore(dir: &Path, checkpoint: &Checkpoint) -> Result<Checkpoint, GitError> {
    let dirs = Dirs::of(dir)?;
    refuse_in_progress(&dirs, "read-tree")?;
    if dirs.git.join("index.lock").exists() {
        return Err(GitError {
            args: owned_args(&["read-tree"]),
            stderr: "git is busy in this repository; try again".to_owned(),
        });
    }
    // Checked before anything moves: a kept object that is gone cannot be
    // recreated, and failing later would leave the restore half done.
    for entry in missing(dir, &checkpoint.kept)? {
        let (what, id) = match entry {
            Kept::Ref { name, target } => (name, target),
            Kept::Stash { message, commit } => (message, commit),
        };
        if exec(dir, &[], &["cat-file", "-e", id])?.code != Some(0) {
            return Err(GitError {
                args: owned_args(&["cat-file", "-e"]),
                stderr: format!("cannot recreate {what}: {id} is gone"),
            });
        }
    }

    // The ignored files the restore overwrites go into the safety checkpoint first.
    let forced = overwritten(&dirs, checkpoint)?;
    let label = format!("Before restoring “{}”", checkpoint.label);
    let safety = create_with(dir, &dirs, &label, &[], &forced)?;
    restore_head(dir, checkpoint)?;
    run(dir, &[], &["add", "-A"])?;
    run(
        dir,
        &[],
        &["read-tree", "--reset", "-u", &checkpoint.worktree_tree],
    )?;
    run(dir, &[], &["read-tree", &checkpoint.index_tree])?;
    // Oldest first, so the stash list keeps the order it was kept in.
    for entry in missing(dir, &checkpoint.kept)?.into_iter().rev() {
        match entry {
            Kept::Ref { name, target } => {
                run(dir, &[], &["update-ref", name, target, ""])?;
            }
            Kept::Stash { commit, message } => stash::store(dir, commit, message)?,
        }
    }
    Ok(safety)
}

/// Deletes all but the newest `keep` checkpoints of this worktree.
pub fn prune(dir: &Path, keep: usize) -> Result<(), GitError> {
    let dirs = Dirs::of(dir)?;
    let (_, refs) = own_refs(dir, &dirs)?;
    for (name, _) in refs.into_iter().skip(keep) {
        run(dir, &[], &["update-ref", "-d", &name])?;
    }
    Ok(())
}

/// This worktree's ref prefix, and its checkpoint refs as (ref name, commit id), newest first.
fn own_refs(dir: &Path, dirs: &Dirs) -> Result<(String, Vec<(String, String)>), GitError> {
    let prefix = refs_prefix(dirs)?;
    let template = "--format=%(refname)%00%(objectname)";
    let args = ["for-each-ref", "--sort=-refname", template, prefix.as_str()];
    let out = run(dir, &[], &args)?;
    let refs = out
        .lines()
        .filter_map(|line| {
            let (name, id) = line.split_once('\0')?;
            Some((name.to_owned(), id.to_owned()))
        })
        .collect();
    Ok((prefix, refs))
}

/// The ref prefix of the worktree at `dir`: `main/` for the main worktree, and
/// `linked/<id>/` for a linked one, where `<id>` is its name under `.git/worktrees/`.
fn refs_prefix(dirs: &Dirs) -> Result<String, GitError> {
    if canonical(&dirs.git)? == canonical(&dirs.common)? {
        return Ok(format!("{ROOT}main/"));
    }
    let id = dirs
        .git
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| GitError {
            args: owned_args(&["rev-parse", "--absolute-git-dir"]),
            stderr: format!("worktree name is not UTF-8: {}", dirs.git.display()),
        })?;
    Ok(format!("{ROOT}linked/{id}/"))
}

fn canonical(path: &Path) -> Result<PathBuf, GitError> {
    fs::canonicalize(path).map_err(|e| io_error(&["rev-parse"], &e))
}

fn newest_of(dir: &Path, refs: &[(String, String)]) -> Result<Option<Checkpoint>, GitError> {
    match refs.first() {
        Some((_, id)) => read(dir, id).map(Some),
        None => Ok(None),
    }
}

/// Reads the checkpoint commit `id`.
fn read(dir: &Path, id: &str) -> Result<Checkpoint, GitError> {
    let text = run(dir, &[], &["cat-file", "commit", id])?;
    parse(id, &text).ok_or_else(|| GitError {
        args: owned_args(&["cat-file", "commit", id]),
        stderr: format!("not a checkpoint: {id}"),
    })
}

/// Reads a checkpoint from its commit's raw text; None when it is not one.
fn parse(id: &str, text: &str) -> Option<Checkpoint> {
    let (header, body) = text.split_once("\n\n")?;
    let mut tree = None;
    let mut at = None;
    for line in header.lines() {
        if let Some(value) = line.strip_prefix("tree ") {
            tree = Some(value.to_owned());
        } else if let Some(value) = line.strip_prefix("committer ") {
            // "name <email> <seconds> <zone>": the seconds are second from the end.
            at = value.rsplit(' ').nth(1).and_then(|s| s.parse::<i64>().ok());
        }
    }

    let mut lines = body.lines();
    let label = lines
        .next()?
        .strip_prefix("rvw checkpoint:")?
        .trim_start()
        .to_owned();
    let mut branch = None;
    let mut head = None;
    let mut index_tree = None;
    let mut kept = Vec::new();
    for line in lines {
        if let Some(value) = line.strip_prefix("branch: ") {
            branch = dash_is_none(value);
        } else if let Some(value) = line.strip_prefix("head: ") {
            head = dash_is_none(value);
        } else if let Some(value) = line.strip_prefix("index: ") {
            index_tree = Some(value.to_owned());
        } else if let Some(value) = line.strip_prefix("keep: ") {
            if let Some(rest) = value.strip_prefix("ref ") {
                let (name, target) = rest.split_once(' ')?;
                kept.push(Kept::Ref {
                    name: name.to_owned(),
                    target: target.to_owned(),
                });
            } else if let Some(rest) = value.strip_prefix("stash ") {
                let (commit, message) = rest.split_once(' ').unwrap_or((rest, ""));
                kept.push(Kept::Stash {
                    commit: commit.to_owned(),
                    message: message.to_owned(),
                });
            }
        }
    }

    Some(Checkpoint {
        id: id.to_owned(),
        label,
        at: at?,
        branch,
        head,
        index_tree: index_tree?,
        worktree_tree: tree?,
        kept,
    })
}

/// The message's "-" for "none", otherwise the value.
fn dash_is_none(value: &str) -> Option<String> {
    (value != "-").then(|| value.to_owned())
}

/// A subject or stash message as one line: a newline would end the field.
fn one_line(text: &str) -> String {
    text.replace(['\r', '\n'], " ").trim().to_owned()
}

fn now_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
}

/// The tree of the worktree as `git add -A` would stage it, plus the ignored `forced` paths.
fn working_tree(dirs: &Dirs, forced: &[String]) -> Result<String, GitError> {
    scratch_tree(dirs, true, forced)
}

/// The tree of the real index. `write-tree` would rewrite the index file's cache tree, so
/// this reads a copy instead and the real index is never written.
fn staged_tree(dirs: &Dirs) -> Result<String, GitError> {
    scratch_tree(dirs, false, &[])
}

/// Writes a tree from a temporary copy of the real index, staging every non-ignored file
/// first when `stage_all`. The copy is deleted afterwards.
fn scratch_tree(dirs: &Dirs, stage_all: bool, forced: &[String]) -> Result<String, GitError> {
    let real = dirs.git.join("index");
    let temp = dirs.git.join("rvw-checkpoint.index");
    let result = write_scratch(&dirs.top, &real, &temp, stage_all, forced);
    let cleanup = remove_if_exists(&temp).map_err(|e| io_error(&["write-tree"], &e));
    let tree = result?;
    cleanup?;
    Ok(tree)
}

fn write_scratch(
    dir: &Path,
    real: &Path,
    temp: &Path,
    stage_all: bool,
    forced: &[String],
) -> Result<String, GitError> {
    // A stale file from an interrupted run must not be the starting point.
    remove_if_exists(temp).map_err(|e| io_error(&["write-tree"], &e))?;
    if real.exists() {
        copy_index(real, temp).map_err(|e| io_error(&["write-tree"], &e))?;
    }
    let temp = temp.to_str().ok_or_else(|| GitError {
        args: owned_args(&["write-tree"]),
        stderr: format!("index path is not UTF-8: {}", temp.display()),
    })?;
    let env = [("GIT_INDEX_FILE", temp)];
    if stage_all {
        run_env(dir, &[], &["add", "-A"], &env)?;
        if !forced.is_empty() {
            // Literal paths, NUL-separated on stdin, so no name is read as a pattern.
            let mut stdin = Vec::new();
            for path in forced {
                stdin.extend_from_slice(path.as_bytes());
                stdin.push(0);
            }
            let args = [
                "--literal-pathspecs",
                "add",
                "-f",
                "--pathspec-from-file=-",
                "--pathspec-file-nul",
            ];
            run_env(dir, &stdin, &args, &env)?;
        }
    }
    Ok(run_env(dir, &[], &["write-tree"], &env)?.trim().to_owned())
}

/// Copies the index with its modification time. Git trusts a cached file
/// size and time only when they are older than the index file; an entry
/// written in the same tick as the index is re-read. A copy made later
/// would look newer, so a file rewritten at the same size in that tick would
/// be saved with its old content. The time is read first: should git rewrite
/// the index meanwhile, the copy looks older, which only means more re-reads.
fn copy_index(real: &Path, temp: &Path) -> io::Result<()> {
    let modified = fs::metadata(real)?.modified()?;
    fs::copy(real, temp)?;
    fs::File::options()
        .write(true)
        .open(temp)?
        .set_modified(modified)
}

fn remove_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

fn io_error(args: &[&str], e: &io::Error) -> GitError {
    GitError {
        args: owned_args(args),
        stderr: gitx::os_error_text(e),
    }
}

/// Refuses while a merge, cherry-pick, revert or rebase is half done.
fn refuse_in_progress(dirs: &Dirs, op: &str) -> Result<(), GitError> {
    for name in IN_PROGRESS {
        if dirs.git.join(name).exists() {
            return Err(GitError {
                args: owned_args(&[op]),
                stderr: "finish or abort the merge or rebase first".to_owned(),
            });
        }
    }
    Ok(())
}

/// Ignored files that restoring `checkpoint` would overwrite: the checkpoint has a file at
/// the same path, a directory of its files there, or a file above it. Git lists them from
/// the worktree root, and only the ignored ones can be missing from the safety checkpoint.
fn overwritten(dirs: &Dirs, checkpoint: &Checkpoint) -> Result<Vec<String>, GitError> {
    let listed = run(
        &dirs.top,
        &[],
        &[
            "ls-files",
            "-z",
            "--others",
            "--ignored",
            "--exclude-standard",
        ],
    )?;
    let ignored: Vec<&str> = listed.split('\0').filter(|p| !p.is_empty()).collect();
    if ignored.is_empty() {
        return Ok(Vec::new());
    }
    let tree = run(
        &dirs.top,
        &[],
        &[
            "ls-tree",
            "-r",
            "-z",
            "--name-only",
            &checkpoint.worktree_tree,
        ],
    )?;
    let mut files: HashSet<&str> = HashSet::new();
    let mut parents: HashSet<&str> = HashSet::new();
    for path in tree.split('\0').filter(|p| !p.is_empty()) {
        files.insert(path);
        let mut rest = path;
        while let Some((parent, _)) = rest.rsplit_once('/') {
            parents.insert(parent);
            rest = parent;
        }
    }
    Ok(ignored
        .into_iter()
        .filter(|path| in_the_way(path, &files, &parents))
        .map(str::to_owned)
        .collect())
}

/// Whether the ignored file `path` is a file of the checkpoint, a directory of one, or lies
/// under one of its files.
fn in_the_way(path: &str, files: &HashSet<&str>, parents: &HashSet<&str>) -> bool {
    if files.contains(path) || parents.contains(path) {
        return true;
    }
    let mut rest = path;
    while let Some((parent, _)) = rest.rsplit_once('/') {
        if files.contains(parent) {
            return true;
        }
        rest = parent;
    }
    false
}

/// The branch HEAD is on, None when detached.
fn current_branch(dir: &Path) -> Result<Option<String>, GitError> {
    let ran = exec(dir, &[], &["symbolic-ref", "-q", "HEAD"])?;
    match ran.code {
        Some(0) => {
            let full = ran.stdout.trim();
            Ok(Some(
                full.strip_prefix("refs/heads/").unwrap_or(full).to_owned(),
            ))
        }
        Some(1) => Ok(None),
        _ => Err(GitError {
            args: owned_args(&["symbolic-ref", "-q", "HEAD"]),
            stderr: ran.stderr,
        }),
    }
}

fn ref_exists(dir: &Path, name: &str) -> Result<bool, GitError> {
    Ok(exec(dir, &[], &["rev-parse", "--verify", "--quiet", name])?.code == Some(0))
}

/// The kept refs and stashes that are gone now, in the order they were kept.
fn missing<'a>(dir: &Path, kept: &'a [Kept]) -> Result<Vec<&'a Kept>, GitError> {
    let stashes = stash::list(dir)?;
    let mut gone = Vec::new();
    for entry in kept {
        let absent = match entry {
            Kept::Ref { name, .. } => !ref_exists(dir, name)?,
            Kept::Stash { commit, .. } => !stashes.iter().any(|s| &s.commit == commit),
        };
        if absent {
            gone.push(entry);
        }
    }
    Ok(gone)
}

/// The tree of `from` against `to`, as git's status letters and paths.
fn name_status(dir: &Path, from: &str, to: &str) -> Result<Vec<(char, String)>, GitError> {
    let args = ["diff", "--name-status", "--no-renames", "-z", from, to];
    let out = run(dir, &[], &args)?;
    // With -z each entry is "<status>\0<path>\0", paths unquoted.
    let mut fields = out.split('\0');
    let mut files = Vec::new();
    while let Some(status) = fields.next() {
        let Some(path) = fields.next() else { break };
        if let Some(letter) = status.chars().next() {
            files.push((letter, path.to_owned()));
        }
    }
    Ok(files)
}

fn commits_left_out(dir: &Path, checkpoint: &Checkpoint) -> Result<usize, GitError> {
    let Some(branch) = &checkpoint.branch else {
        return Ok(0);
    };
    let Some(tip) = gitx::commit(dir, &format!("refs/heads/{branch}")) else {
        return Ok(0);
    };
    let range = match &checkpoint.head {
        Some(head) => format!("{head}..{tip}"),
        None => tip,
    };
    let out = run(dir, &[], &["rev-list", "--count", &range])?;
    out.trim().parse().map_err(|_| GitError {
        args: owned_args(&["rev-list", "--count"]),
        stderr: format!("unexpected count '{}'", out.trim()),
    })
}

/// Points HEAD and its branch back at the checkpoint's, as restore step 2 says.
fn restore_head(dir: &Path, checkpoint: &Checkpoint) -> Result<(), GitError> {
    match (&checkpoint.branch, &checkpoint.head) {
        (Some(branch), Some(head)) => {
            let full = format!("refs/heads/{branch}");
            run(dir, &[], &["update-ref", &full, head])?;
            run(dir, &[], &["symbolic-ref", "HEAD", &full])?;
        }
        (Some(branch), None) => {
            let full = format!("refs/heads/{branch}");
            run(dir, &[], &["symbolic-ref", "HEAD", &full])?;
            if ref_exists(dir, &full)? {
                run(dir, &[], &["update-ref", "-d", &full])?;
            }
        }
        (None, Some(head)) => {
            run(dir, &[], &["update-ref", "--no-deref", "HEAD", head])?;
        }
        (None, None) => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testrepo::{commit_all, git, repo, write};
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::process::Command;
    use std::time::{Duration, SystemTime};
    use tempfile::TempDir;

    /// A repository whose `a.txt` is committed as "a".
    fn committed() -> TempDir {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "init");
        r
    }

    /// Whether git exits 0 in `dir`; for the calls that are expected to fail.
    fn git_ok(dir: &Path, args: &[&str]) -> bool {
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap()
            .status
            .success()
    }

    fn head_of(dir: &Path) -> String {
        git(dir, &["rev-parse", "HEAD"]).trim().to_owned()
    }

    fn status(dir: &Path) -> String {
        git(dir, &["status", "--porcelain"])
    }

    fn ids(dir: &Path) -> Vec<String> {
        list(dir).unwrap().into_iter().map(|c| c.id).collect()
    }

    fn labels(dir: &Path) -> Vec<String> {
        list(dir).unwrap().into_iter().map(|c| c.label).collect()
    }

    #[test]
    fn create_and_list_round_trip_kept_refs_and_stashes() {
        let r = committed();
        git(r.path(), &["branch", "old"]);
        let head = head_of(r.path());
        write(r.path(), "a.txt", "changed\n");
        let entry = stash::push(r.path(), "try the parser", false)
            .unwrap()
            .unwrap();
        let kept = [
            Kept::Ref {
                name: "refs/heads/old".to_owned(),
                target: head.clone(),
            },
            Kept::Stash {
                commit: entry.commit.clone(),
                message: entry.message.clone(),
            },
        ];

        let first = create(r.path(), "first", &kept).unwrap();
        assert_eq!(first.label, "first");
        assert_eq!(first.branch.as_deref(), Some("main"));
        assert_eq!(first.head.as_deref(), Some(head.as_str()));
        assert_eq!(first.index_tree, git(r.path(), &["write-tree"]).trim());
        assert_eq!(
            first.worktree_tree,
            git(r.path(), &["rev-parse", &format!("{}^{{tree}}", first.id)]).trim()
        );
        assert_eq!(
            git(
                r.path(),
                &["show", &format!("{}:a.txt", first.worktree_tree)]
            ),
            "a\n"
        );
        assert_eq!(first.kept, kept.to_vec());
        let now = i64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        )
        .unwrap();
        assert!((now - first.at).abs() < 60);

        write(r.path(), "a.txt", "second\n");
        let second = create(r.path(), "second", &[]).unwrap();
        let all = list(r.path()).unwrap();
        assert_eq!(all, vec![second, first]);
    }

    #[test]
    fn create_twice_with_nothing_changed_returns_the_same_checkpoint() {
        let r = committed();
        let first = create(r.path(), "one", &[]).unwrap();
        let again = create(r.path(), "two", &[]).unwrap();
        assert_eq!(again, first);
        let refs = "refs/rvw/checkpoints/main/";
        assert_eq!(git(r.path(), &["for-each-ref", refs]).lines().count(), 1);

        write(r.path(), "a.txt", "changed\n");
        let third = create(r.path(), "three", &[]).unwrap();
        assert_ne!(third.id, first.id);
        assert_eq!(git(r.path(), &["for-each-ref", refs]).lines().count(), 2);
    }

    #[test]
    fn create_leaves_the_real_index_and_status_untouched() {
        let r = committed();
        write(r.path(), "a.txt", "staged\n");
        git(r.path(), &["add", "--", "a.txt"]);
        write(r.path(), "a.txt", "edited\n");
        write(r.path(), "new.txt", "new\n");
        // git's own write-tree and status settle the index file (its cache tree
        // and stat data) before the bytes are compared.
        let tree_before = git(r.path(), &["write-tree"]);
        let status_before = status(r.path());
        let index_before = fs::read(r.path().join(".git/index")).unwrap();

        create(r.path(), "check", &[]).unwrap();

        assert_eq!(fs::read(r.path().join(".git/index")).unwrap(), index_before);
        assert_eq!(git(r.path(), &["write-tree"]), tree_before);
        assert_eq!(status(r.path()), status_before);
        assert!(!r.path().join(".git/rvw-checkpoint.index").exists());
    }

    #[test]
    fn restore_brings_back_a_modified_and_a_deleted_file_and_removes_a_new_one() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        write(r.path(), "b.txt", "b\n");
        commit_all(r.path(), "init");
        let cp = create(r.path(), "before", &[]).unwrap();

        write(r.path(), "a.txt", "changed\n");
        fs::remove_file(r.path().join("b.txt")).unwrap();
        write(r.path(), "new.txt", "new\n");
        let safety = restore(r.path(), &cp).unwrap();

        assert_eq!(fs::read_to_string(r.path().join("a.txt")).unwrap(), "a\n");
        assert_eq!(fs::read_to_string(r.path().join("b.txt")).unwrap(), "b\n");
        assert!(!r.path().join("new.txt").exists());
        assert_eq!(status(r.path()), "");
        assert_eq!(safety.label, "Before restoring “before”");
        assert_eq!(
            git(
                r.path(),
                &["show", &format!("{}:new.txt", safety.worktree_tree)]
            ),
            "new\n"
        );

        restore(r.path(), &safety).unwrap();
        assert_eq!(
            fs::read_to_string(r.path().join("a.txt")).unwrap(),
            "changed\n"
        );
        assert!(!r.path().join("b.txt").exists());
        assert_eq!(
            fs::read_to_string(r.path().join("new.txt")).unwrap(),
            "new\n"
        );
    }

    #[test]
    fn an_untracked_file_in_the_checkpoint_comes_back_untracked() {
        let r = committed();
        write(r.path(), "u.txt", "u\n");
        let cp = create(r.path(), "with untracked", &[]).unwrap();
        fs::remove_file(r.path().join("u.txt")).unwrap();

        restore(r.path(), &cp).unwrap();
        assert_eq!(fs::read_to_string(r.path().join("u.txt")).unwrap(), "u\n");
        assert_eq!(status(r.path()), "?? u.txt\n");
    }

    #[test]
    fn a_same_size_edit_in_the_second_the_index_was_written_is_saved() {
        // Git may compare file times to the second only, and then re-reads a
        // file whose time is not older than the index. Set that state up a
        // minute back: "one" staged, the index written, then "two" written,
        // all within one second. Ctime cannot be set, so it is not trusted.
        let r = committed();
        git(r.path(), &["config", "core.trustctime", "false"]);
        let file = r.path().join("a.txt");
        let back = SystemTime::now() - Duration::from_secs(60);
        let set = |path: &Path| {
            fs::File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_modified(back)
                .unwrap();
        };
        write(r.path(), "a.txt", "one\n");
        set(&file);
        git(r.path(), &["add", "--", "a.txt"]);
        set(&r.path().join(".git/index"));
        write(r.path(), "a.txt", "two\n");
        set(&file);

        let cp = create(r.path(), "racy", &[]).unwrap();

        let saved = git(
            r.path(),
            &["cat-file", "-p", &format!("{}:a.txt", cp.worktree_tree)],
        );
        assert_eq!(saved, "two\n");
        assert_eq!(git(r.path(), &["diff", "--stat"]).lines().count(), 2);
    }

    #[test]
    fn staged_only_state_comes_back_in_the_index_and_the_worktree() {
        let r = committed();
        write(r.path(), "a.txt", "one\n");
        git(r.path(), &["add", "--", "a.txt"]);
        write(r.path(), "a.txt", "two\n");
        let cached = git(r.path(), &["diff", "--cached"]);
        let unstaged = git(r.path(), &["diff"]);
        let cp = create(r.path(), "staged", &[]).unwrap();

        commit_all(r.path(), "messy");
        write(r.path(), "a.txt", "three\n");
        write(r.path(), "c.txt", "c\n");
        git(r.path(), &["add", "--", "c.txt"]);

        restore(r.path(), &cp).unwrap();
        assert_eq!(git(r.path(), &["diff", "--cached"]), cached);
        assert_eq!(git(r.path(), &["diff"]), unstaged);
        assert_eq!(head_of(r.path()), cp.head.clone().unwrap());
    }

    #[test]
    fn the_executable_bit_and_a_symlink_come_back() {
        let r = committed();
        write(r.path(), "run.sh", "#!/bin/sh\n");
        fs::set_permissions(r.path().join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
        symlink("a.txt", r.path().join("link")).unwrap();
        commit_all(r.path(), "scripts");
        let cp = create(r.path(), "modes", &[]).unwrap();

        fs::set_permissions(r.path().join("run.sh"), fs::Permissions::from_mode(0o644)).unwrap();
        fs::remove_file(r.path().join("link")).unwrap();
        write(r.path(), "link", "not a link\n");
        restore(r.path(), &cp).unwrap();

        let mode = fs::metadata(r.path().join("run.sh"))
            .unwrap()
            .permissions()
            .mode();
        // Git keeps only the executable bit; its value is masked by the umask.
        assert_ne!(mode & 0o111, 0);
        assert_eq!(
            fs::read_link(r.path().join("link")).unwrap(),
            Path::new("a.txt")
        );
        let listed = git(r.path(), &["ls-files", "-s"]);
        assert!(listed.contains("100755"), "{listed}");
        assert!(listed.contains("120000"), "{listed}");
    }

    #[test]
    fn ignored_files_are_never_touched_by_restore() {
        let r = repo();
        write(r.path(), ".gitignore", "*.log\n");
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "init");
        write(r.path(), "old.log", "old\n");
        let cp = create(r.path(), "ignored", &[]).unwrap();

        write(r.path(), "old.log", "newer\n");
        write(r.path(), "new.log", "x\n");
        write(r.path(), "a.txt", "changed\n");
        restore(r.path(), &cp).unwrap();

        assert_eq!(
            fs::read_to_string(r.path().join("old.log")).unwrap(),
            "newer\n"
        );
        assert_eq!(fs::read_to_string(r.path().join("new.log")).unwrap(), "x\n");
        assert_eq!(fs::read_to_string(r.path().join("a.txt")).unwrap(), "a\n");
    }

    #[test]
    fn commits_made_after_the_checkpoint_leave_the_branch_and_come_back_from_the_safety() {
        let r = committed();
        let start = head_of(r.path());
        let cp = create(r.path(), "start", &[]).unwrap();
        write(r.path(), "b.txt", "b\n");
        commit_all(r.path(), "two");
        write(r.path(), "c.txt", "c\n");
        commit_all(r.path(), "three");
        let tip = head_of(r.path());

        assert_eq!(preview(r.path(), &cp).unwrap().commits_left_out, 2);
        let safety = restore(r.path(), &cp).unwrap();
        assert_eq!(head_of(r.path()), start);
        assert_eq!(
            git(r.path(), &["rev-parse", "refs/heads/main"]).trim(),
            start
        );
        assert!(!r.path().join("b.txt").exists());

        restore(r.path(), &safety).unwrap();
        assert_eq!(head_of(r.path()), tip);
        assert_eq!(fs::read_to_string(r.path().join("c.txt")).unwrap(), "c\n");
    }

    #[test]
    fn restore_switches_back_to_the_checkpoint_branch_and_keeps_the_other() {
        let r = committed();
        let main_head = head_of(r.path());
        let cp = create(r.path(), "on main", &[]).unwrap();
        git(r.path(), &["switch", "-c", "other"]);
        write(r.path(), "x.txt", "x\n");
        commit_all(r.path(), "other work");
        let other = head_of(r.path());

        let before = preview(r.path(), &cp).unwrap();
        assert_eq!(before.branch_now.as_deref(), Some("other"));
        assert_eq!(before.branch_then.as_deref(), Some("main"));
        // The restored branch is main, which has no commit that the checkpoint lacks.
        assert_eq!(before.commits_left_out, 0);

        restore(r.path(), &cp).unwrap();
        assert_eq!(
            git(r.path(), &["symbolic-ref", "--short", "HEAD"]).trim(),
            "main"
        );
        assert_eq!(head_of(r.path()), main_head);
        assert_eq!(
            git(r.path(), &["rev-parse", "refs/heads/other"]).trim(),
            other
        );
    }

    #[test]
    fn an_unborn_branch_restores_without_a_commit_and_with_the_staged_file() {
        let r = repo();
        write(r.path(), "s.txt", "staged\n");
        git(r.path(), &["add", "--", "s.txt"]);
        let cp = create(r.path(), "unborn", &[]).unwrap();
        assert_eq!(cp.head, None);
        assert_eq!(cp.branch.as_deref(), Some("main"));

        git(r.path(), &["commit", "-q", "-m", "first"]);
        restore(r.path(), &cp).unwrap();

        assert!(!git_ok(r.path(), &["rev-parse", "--verify", "HEAD"]));
        assert!(!git_ok(
            r.path(),
            &["show-ref", "--verify", "--quiet", "refs/heads/main"]
        ));
        assert_eq!(
            git(r.path(), &["symbolic-ref", "--short", "HEAD"]).trim(),
            "main"
        );
        assert_eq!(
            git(r.path(), &["diff", "--cached", "--name-only"]),
            "s.txt\n"
        );
    }

    #[test]
    fn a_detached_head_is_saved_and_restored_detached() {
        let r = committed();
        let start = head_of(r.path());
        git(r.path(), &["checkout", "-q", "--detach"]);
        let cp = create(r.path(), "detached", &[]).unwrap();
        assert_eq!(cp.branch, None);
        assert_eq!(cp.head.as_deref(), Some(start.as_str()));

        git(r.path(), &["switch", "-c", "other"]);
        write(r.path(), "x.txt", "x\n");
        commit_all(r.path(), "other work");
        let before = preview(r.path(), &cp).unwrap();
        assert_eq!(before.branch_then, None);
        assert_eq!(before.commits_left_out, 0);

        restore(r.path(), &cp).unwrap();
        assert!(!git_ok(r.path(), &["symbolic-ref", "-q", "HEAD"]));
        assert_eq!(head_of(r.path()), start);
    }

    #[test]
    fn a_kept_branch_is_recreated_and_an_existing_one_is_not_moved() {
        let r = committed();
        git(r.path(), &["branch", "gone"]);
        let target = head_of(r.path());
        let cp = create(
            r.path(),
            "keeps",
            &[Kept::Ref {
                name: "refs/heads/gone".to_owned(),
                target: target.clone(),
            }],
        )
        .unwrap();
        git(r.path(), &["branch", "-D", "gone"]);
        assert_eq!(
            preview(r.path(), &cp).unwrap().recreated,
            vec!["refs/heads/gone".to_owned()]
        );
        restore(r.path(), &cp).unwrap();
        assert_eq!(
            git(r.path(), &["rev-parse", "refs/heads/gone"]).trim(),
            target
        );

        // Moved since the checkpoint: restore leaves it where it is.
        write(r.path(), "b.txt", "b\n");
        commit_all(r.path(), "two");
        let moved = head_of(r.path());
        let cp = create(
            r.path(),
            "keeps again",
            &[Kept::Ref {
                name: "refs/heads/gone".to_owned(),
                target: target.clone(),
            }],
        )
        .unwrap();
        git(r.path(), &["branch", "-f", "gone", &moved]);
        restore(r.path(), &cp).unwrap();
        assert_eq!(
            git(r.path(), &["rev-parse", "refs/heads/gone"]).trim(),
            moved
        );
    }

    #[test]
    fn a_dropped_stash_is_stored_back() {
        let r = committed();
        write(r.path(), "a.txt", "changed\n");
        let entry = stash::push(r.path(), "keep me", false).unwrap().unwrap();
        let cp = create(
            r.path(),
            "stashed",
            &[Kept::Stash {
                commit: entry.commit.clone(),
                message: entry.message.clone(),
            }],
        )
        .unwrap();
        stash::drop(r.path(), &entry).unwrap();
        assert!(stash::list(r.path()).unwrap().is_empty());
        assert_eq!(
            preview(r.path(), &cp).unwrap().recreated,
            vec![format!("stash {}", entry.message)]
        );

        restore(r.path(), &cp).unwrap();
        let all = stash::list(r.path()).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].commit, entry.commit);
        assert_eq!(all[0].message, entry.message);
    }

    #[test]
    fn kept_stashes_come_back_in_the_order_they_were_kept() {
        let r = committed();
        write(r.path(), "a.txt", "first\n");
        let older = stash::push(r.path(), "older", false).unwrap().unwrap();
        write(r.path(), "a.txt", "second\n");
        let newer = stash::push(r.path(), "newer", false).unwrap().unwrap();
        let kept: Vec<Kept> = [&newer, &older]
            .iter()
            .map(|e| Kept::Stash {
                commit: e.commit.clone(),
                message: e.message.clone(),
            })
            .collect();
        let cp = create(r.path(), "two stashes", &kept).unwrap();
        stash::drop(r.path(), &older).unwrap();
        stash::drop(r.path(), &newer).unwrap();

        restore(r.path(), &cp).unwrap();
        let commits: Vec<String> = stash::list(r.path())
            .unwrap()
            .into_iter()
            .map(|e| e.commit)
            .collect();
        assert_eq!(commits, vec![newer.commit, older.commit]);
    }

    #[test]
    fn a_kept_tag_object_that_is_gone_refuses_before_anything_changes() {
        let r = committed();
        git(r.path(), &["tag", "-a", "v1", "-m", "release"]);
        let tag = git(r.path(), &["rev-parse", "refs/tags/v1"])
            .trim()
            .to_owned();
        let cp = create(
            r.path(),
            "tagged",
            &[Kept::Ref {
                name: "refs/tags/v1".to_owned(),
                target: tag.clone(),
            }],
        )
        .unwrap();
        // Only the peeled commit is a parent, so gc prunes the tag object.
        git(r.path(), &["tag", "-d", "v1"]);
        git(r.path(), &["gc", "--prune=now", "-q"]);
        assert!(!git_ok(r.path(), &["cat-file", "-e", &tag]));

        write(r.path(), "a.txt", "changed\n");
        let err = restore(r.path(), &cp).unwrap_err();
        assert!(
            err.stderr.starts_with("cannot recreate refs/tags/v1"),
            "{err}"
        );
        assert_eq!(
            fs::read_to_string(r.path().join("a.txt")).unwrap(),
            "changed\n"
        );
        assert_eq!(list(r.path()).unwrap().len(), 1);
    }

    #[test]
    fn create_and_restore_refuse_mid_merge_and_change_nothing() {
        let r = committed();
        git(r.path(), &["switch", "-c", "other"]);
        write(r.path(), "a.txt", "other\n");
        commit_all(r.path(), "other");
        git(r.path(), &["switch", "main"]);
        write(r.path(), "a.txt", "main\n");
        commit_all(r.path(), "main");
        let cp = create(r.path(), "before merge", &[]).unwrap();

        let merge = Command::new("git")
            .arg("-C")
            .arg(r.path())
            .args(["merge", "other"])
            .output()
            .unwrap();
        assert!(!merge.status.success());
        let status_before = status(r.path());
        let head_before = head_of(r.path());

        let msg = "finish or abort the merge or rebase first";
        assert_eq!(create(r.path(), "during", &[]).unwrap_err().stderr, msg);
        assert_eq!(restore(r.path(), &cp).unwrap_err().stderr, msg);
        assert!(preview(r.path(), &cp).is_err());
        assert_eq!(status(r.path()), status_before);
        assert_eq!(head_of(r.path()), head_before);
        assert_eq!(ids(r.path()), vec![cp.id]);
        assert!(r.path().join(".git/MERGE_HEAD").exists());
    }

    #[test]
    fn restore_refuses_while_index_lock_exists_and_changes_nothing() {
        let r = committed();
        let cp = create(r.path(), "locked", &[]).unwrap();
        write(r.path(), "a.txt", "changed\n");
        fs::write(r.path().join(".git/index.lock"), "").unwrap();

        let err = restore(r.path(), &cp).unwrap_err();
        assert_eq!(err.stderr, "git is busy in this repository; try again");
        assert_eq!(
            fs::read_to_string(r.path().join("a.txt")).unwrap(),
            "changed\n"
        );
        assert_eq!(ids(r.path()), vec![cp.id]);
        fs::remove_file(r.path().join(".git/index.lock")).unwrap();
    }

    #[test]
    fn a_checkpoint_is_listed_only_in_the_worktree_it_was_taken_in() {
        let r = committed();
        let parent = TempDir::new().unwrap();
        let linked = parent.path().join("linked");
        git(
            r.path(),
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "linked",
                linked.to_str().unwrap(),
            ],
        );
        write(&linked, "l.txt", "linked\n");
        write(r.path(), "m.txt", "main\n");

        let in_linked = create(&linked, "in linked", &[]).unwrap();
        let in_main = create(r.path(), "in main", &[]).unwrap();
        assert_eq!(ids(&linked), vec![in_linked.id.clone()]);
        assert_eq!(ids(r.path()), vec![in_main.id.clone()]);
        assert_eq!(in_linked.branch.as_deref(), Some("linked"));
        assert_eq!(in_main.branch.as_deref(), Some("main"));
        // Both live in the common namespace, under each worktree's own prefix.
        let common = git(r.path(), &["for-each-ref", "--format=%(refname)", ROOT]);
        let names: Vec<&str> = common.lines().collect();
        assert_eq!(names.len(), 2, "{common}");
        assert!(
            names
                .iter()
                .any(|n| n.starts_with("refs/rvw/checkpoints/main/"))
        );
        assert!(
            names
                .iter()
                .any(|n| n.starts_with("refs/rvw/checkpoints/linked/linked/"))
        );
    }

    /// A repository with a linked worktree at `parent/linked`, which has a file `l.txt`.
    fn with_linked_worktree(parent: &TempDir) -> (TempDir, PathBuf) {
        let r = committed();
        let linked = parent.path().join("linked");
        git(
            r.path(),
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "linked",
                linked.to_str().unwrap(),
            ],
        );
        write(&linked, "l.txt", "linked\n");
        (r, linked)
    }

    #[test]
    fn gc_in_a_worktree_keeps_the_checkpoints_taken_in_it() {
        let parent = TempDir::new().unwrap();
        let (r, linked) = with_linked_worktree(&parent);
        let cp = create(&linked, "in linked", &[]).unwrap();
        git(&linked, &["gc", "--prune=now", "-q"]);
        for id in [&cp.id, &cp.index_tree, &cp.worktree_tree] {
            assert!(git_ok(&linked, &["cat-file", "-e", id]), "gc pruned {id}");
        }
        restore(&linked, &cp).unwrap();
        assert_eq!(
            fs::read_to_string(linked.join("l.txt")).unwrap(),
            "linked\n"
        );

        // The main worktree, with no other worktree's checkpoints around.
        write(r.path(), "m.txt", "main\n");
        let main_cp = create(r.path(), "in main", &[]).unwrap();
        git(r.path(), &["gc", "--prune=now", "-q"]);
        for id in [&main_cp.id, &main_cp.index_tree, &main_cp.worktree_tree] {
            assert!(git_ok(r.path(), &["cat-file", "-e", id]), "gc pruned {id}");
        }
        restore(r.path(), &main_cp).unwrap();
        assert_eq!(
            fs::read_to_string(r.path().join("m.txt")).unwrap(),
            "main\n"
        );
    }

    /// Each worktree's gc, with `gc --prune=now`, keeps the checkpoints of both
    /// worktrees, and each worktree still lists only its own.
    #[test]
    fn gc_from_either_worktree_keeps_the_checkpoints_of_both() {
        let parent = TempDir::new().unwrap();
        let (r, linked) = with_linked_worktree(&parent);
        let in_linked = create(&linked, "in linked", &[]).unwrap();
        write(r.path(), "m.txt", "main\n");
        let in_main = create(r.path(), "in main", &[]).unwrap();

        for gc_dir in [r.path(), linked.as_path()] {
            git(gc_dir, &["gc", "--prune=now", "-q"]);
            for cp in [&in_linked, &in_main] {
                for id in [&cp.id, &cp.index_tree, &cp.worktree_tree] {
                    assert!(
                        git_ok(gc_dir, &["cat-file", "-e", id]),
                        "gc in {} pruned {id} of checkpoint '{}'",
                        gc_dir.display(),
                        cp.label
                    );
                }
            }
        }
        assert_eq!(ids(&linked), vec![in_linked.id.clone()]);
        assert_eq!(ids(r.path()), vec![in_main.id.clone()]);

        restore(&linked, &in_linked).unwrap();
        assert_eq!(
            fs::read_to_string(linked.join("l.txt")).unwrap(),
            "linked\n"
        );
        restore(r.path(), &in_main).unwrap();
        assert_eq!(
            fs::read_to_string(r.path().join("m.txt")).unwrap(),
            "main\n"
        );
    }

    #[test]
    fn prune_keeps_the_newest_checkpoints() {
        let r = committed();
        for (n, label) in ["one", "two", "three"].into_iter().enumerate() {
            write(r.path(), "a.txt", &format!("{n}\n"));
            create(r.path(), label, &[]).unwrap();
        }
        prune(r.path(), 2).unwrap();
        assert_eq!(labels(r.path()), vec!["three", "two"]);
        prune(r.path(), 0).unwrap();
        assert!(list(r.path()).unwrap().is_empty());
    }

    /// A measurement, not a check: run with `--ignored --nocapture`.
    #[test]
    #[ignore = "timing measurement"]
    fn timing_with_two_hundred_checkpoints() {
        let r = committed();
        for n in 0..200 {
            write(r.path(), "a.txt", &format!("{n}\n"));
            create(r.path(), &format!("cp {n}"), &[]).unwrap();
        }
        let t = std::time::Instant::now();
        assert_eq!(list(r.path()).unwrap().len(), 200);
        eprintln!("list of 200: {:?}", t.elapsed());
        let t = std::time::Instant::now();
        create(r.path(), "same", &[]).unwrap();
        eprintln!("create, deduped: {:?}", t.elapsed());
        write(r.path(), "a.txt", "new\n");
        let t = std::time::Instant::now();
        create(r.path(), "new", &[]).unwrap();
        eprintln!("create, new: {:?}", t.elapsed());
    }

    #[test]
    fn restore_keeps_an_ignored_file_it_overwrites_in_the_safety_checkpoint() {
        let r = committed();
        write(r.path(), "x.txt", "old\n");
        let cp = create(r.path(), "with x untracked", &[]).unwrap();
        write(r.path(), ".gitignore", "x.txt\n");
        write(r.path(), "x.txt", "precious\n");

        let before = preview(r.path(), &cp).unwrap();
        assert_eq!(before.overwritten_ignored, vec!["x.txt".to_owned()]);

        let safety = restore(r.path(), &cp).unwrap();
        assert_eq!(fs::read_to_string(r.path().join("x.txt")).unwrap(), "old\n");
        restore(r.path(), &safety).unwrap();
        assert_eq!(
            fs::read_to_string(r.path().join("x.txt")).unwrap(),
            "precious\n"
        );
    }

    #[test]
    fn an_ignored_file_outside_the_checkpoint_is_untouched_and_not_listed() {
        let r = repo();
        write(r.path(), ".gitignore", "target/\n");
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "init");
        let cp = create(r.path(), "before", &[]).unwrap();
        write(r.path(), "target/out.bin", "bin\n");
        write(r.path(), "a.txt", "changed\n");

        let p = preview(r.path(), &cp).unwrap();
        assert!(p.overwritten_ignored.is_empty(), "{p:?}");
        restore(r.path(), &cp).unwrap();
        assert_eq!(
            fs::read_to_string(r.path().join("target/out.bin")).unwrap(),
            "bin\n"
        );
    }

    #[test]
    fn an_ignored_file_in_the_way_of_a_checkpoint_directory_is_kept() {
        let r = repo();
        write(r.path(), ".gitignore", "build/\n");
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "init");
        write(r.path(), "build", "file\n");
        let cp = create(r.path(), "build is a file", &[]).unwrap();
        fs::remove_file(r.path().join("build")).unwrap();
        write(r.path(), "build/precious.txt", "precious\n");

        let safety = restore(r.path(), &cp).unwrap();
        assert_eq!(
            fs::read_to_string(r.path().join("build")).unwrap(),
            "file\n"
        );
        restore(r.path(), &safety).unwrap();
        assert_eq!(
            fs::read_to_string(r.path().join("build/precious.txt")).unwrap(),
            "precious\n"
        );
    }
}
