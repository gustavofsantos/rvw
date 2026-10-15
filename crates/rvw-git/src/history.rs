//! Commit history: a page of the log, one commit's details and diffs, and the
//! comparison of two revisions. Every read that can be slow takes a stop flag.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::AtomicBool;

use crate::gitx::{GitError, exec_stop, owned_args, run, run_stop};

/// One commit as a log line shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    pub id: String,
    /// Parent ids, first parent first; empty for a root commit.
    pub parents: Vec<String>,
    pub author: String,
    pub email: String,
    /// Author time, seconds since the Unix epoch.
    pub time: i64,
    /// Full ref names pointing at the commit as `%D` prints them with
    /// [`DECORATE`], e.g. `["HEAD -> refs/heads/main",
    /// "refs/remotes/origin/main", "tag: refs/tags/v1"]`.
    pub refs: Vec<String>,
    pub subject: String,
}

/// The `--format` that [`parse_log`] reads: fields split by 0x1f, records
/// ended by 0x1e.
/// Passed with every log format: full ref names in `%D`, so a branch named
/// `feat/x` is told apart from a remote branch.
pub(crate) const DECORATE: &str = "--decorate=full";

pub(crate) const LOG_FORMAT: &str = "--format=%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%D%x1f%s%x1e";

/// The `--format` of one commit in full: the seven [`LOG_FORMAT`] fields, then
/// the committer name, email and time, then the whole message, all split by
/// 0x1f. The message is last, so it may hold any character.
const COMMIT_FORMAT: &str =
    "--format=%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%D%x1f%s%x1f%cn%x1f%ce%x1f%ct%x1f%B";

/// The entries of `git log` output written with [`LOG_FORMAT`]. A record that
/// does not have seven fields is skipped.
pub(crate) fn parse_log(out: &str) -> Vec<LogEntry> {
    out.split('\x1e')
        .filter_map(|record| {
            let record = record.trim_start_matches('\n');
            let fields: Vec<&str> = record.split('\x1f').collect();
            let fields: [&str; 7] = fields.try_into().ok()?;
            Some(log_entry(fields))
        })
        .collect()
}

/// The entry for the seven fields of [`LOG_FORMAT`], in order.
fn log_entry([id, parents, author, email, time, refs, subject]: [&str; 7]) -> LogEntry {
    LogEntry {
        id: id.to_owned(),
        parents: parents.split_whitespace().map(str::to_owned).collect(),
        author: author.to_owned(),
        email: email.to_owned(),
        time: time.parse().unwrap_or(0),
        refs: refs
            .split(", ")
            .filter(|r| !r.is_empty())
            .map(str::to_owned)
            .collect(),
        subject: subject.to_owned(),
    }
}

/// Which commits a log page reads.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LogQuery {
    /// Revisions to start from; empty means HEAD. `["--all"]` is NOT accepted; use `all`.
    pub revs: Vec<String>,
    /// Every local branch, remote branch and tag (git's `--branches --remotes --tags`), plus HEAD.
    pub all: bool,
    /// Commits to skip from the top before the page.
    pub skip: usize,
    /// At most this many commits; 0 means none (return empty without running git).
    pub limit: usize,
}

/// A page of the log in topological order (`--topo-order`), newest first.
/// A repository with no commits yet gives an empty page. Fails when a revision
/// starts with `-`, when git fails, or when `stop` is set ("context canceled").
pub fn log(dir: &Path, query: &LogQuery, stop: &AtomicBool) -> Result<Vec<LogEntry>, GitError> {
    if query.limit == 0 {
        return Ok(Vec::new());
    }
    for rev in &query.revs {
        check_rev("log", rev)?;
    }
    let head_born = (query.revs.is_empty() || query.all) && has_head(dir, stop)?;
    if query.revs.is_empty() && !query.all && !head_born {
        return Ok(Vec::new());
    }
    let skip = format!("--skip={}", query.skip);
    let max = format!("--max-count={}", query.limit);
    let mut args = vec![
        "log",
        "--no-color",
        "--topo-order",
        DECORATE,
        LOG_FORMAT,
        skip.as_str(),
        max.as_str(),
    ];
    if query.all {
        args.extend(["--branches", "--remotes", "--tags"]);
        if head_born {
            args.push("HEAD");
        }
    }
    args.extend(query.revs.iter().map(String::as_str));
    args.push("--");
    let out = run_stop(dir, &args, stop)?;
    Ok(parse_log(&out))
}

/// Whether HEAD names a commit; false in a repository with no commits yet.
fn has_head(dir: &Path, stop: &AtomicBool) -> Result<bool, GitError> {
    let ran = exec_stop(
        dir,
        &[],
        &["rev-parse", "--verify", "-q", "HEAD"],
        &[],
        Some(stop),
    )?;
    Ok(ran.code == Some(0))
}

/// A file a commit (or a comparison) changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    pub path: String,
    /// The path before a rename or copy; None otherwise.
    pub old_path: Option<String>,
    /// git's status letter: b'A', b'M', b'D', b'R', b'C', b'T'.
    pub status: u8,
    /// Lines added and deleted; None for a binary file.
    pub added: Option<u64>,
    pub deleted: Option<u64>,
}

/// One commit in full.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitDetail {
    pub entry: LogEntry,
    pub committer: String,
    pub committer_email: String,
    /// Committer time, seconds since the Unix epoch.
    pub committer_time: i64,
    /// The whole message, subject and body, with the trailing newline trimmed.
    pub message: String,
    /// Files changed against the first parent (against the empty tree for a root commit), in git's order.
    pub files: Vec<FileChange>,
}

/// The commit `rev` with its message and changed files. Renames are detected (`-M`).
/// Fails when `rev` does not name a commit or starts with `-`.
pub fn show(dir: &Path, rev: &str) -> Result<CommitDetail, GitError> {
    check_rev("show", rev)?;
    let args = [
        "log",
        "-1",
        "--no-color",
        DECORATE,
        COMMIT_FORMAT,
        rev,
        "--",
    ];
    let out = run(dir, &[], &args)?;
    let fields: Vec<&str> = out.splitn(11, '\x1f').collect();
    let [
        id,
        parents,
        author,
        email,
        time,
        refs,
        subject,
        committer,
        committer_email,
        committer_time,
        message,
    ]: [&str; 11] = fields.try_into().map_err(|_| malformed(&args))?;
    let committer_time: i64 = committer_time.parse().map_err(|_| malformed(&args))?;
    let entry = log_entry([id, parents, author, email, time, refs, subject]);
    let base = base_of(dir, &entry.parents)?;
    let files = changes(dir, &base, &entry.id)?;
    Ok(CommitDetail {
        entry,
        committer: committer.to_owned(),
        committer_email: committer_email.to_owned(),
        committer_time,
        message: message.trim_end_matches('\n').to_owned(),
        files,
    })
}

/// The unified diff of `path` in commit `rev` against its first parent (the empty tree for a root commit), with `context` lines of context. For a renamed file pass the new path; the diff follows the rename. Empty string when the commit did not touch `path`.
pub fn commit_diff(dir: &Path, rev: &str, path: &str, context: usize) -> Result<String, GitError> {
    let detail = show(dir, rev)?;
    let Some(change) = find_change(&detail.files, path) else {
        return Ok(String::new());
    };
    let base = base_of(dir, &detail.entry.parents)?;
    file_diff(dir, &base, &detail.entry.id, change, context)
}

/// Files that differ between revisions `from` and `to` (both must name commits), renames detected.
/// Fails when either revision does not name a commit.
pub fn compare(dir: &Path, from: &str, to: &str) -> Result<Vec<FileChange>, GitError> {
    let from = commit_id(dir, from)?;
    let to = commit_id(dir, to)?;
    changes(dir, &from, &to)
}

/// The unified diff of `path` between revisions `from` and `to`, with `context` lines of context.
/// Empty string when the two revisions do not differ for `path`.
pub fn compare_diff(
    dir: &Path,
    from: &str,
    to: &str,
    path: &str,
    context: usize,
) -> Result<String, GitError> {
    let from = commit_id(dir, from)?;
    let to = commit_id(dir, to)?;
    let files = changes(dir, &from, &to)?;
    match find_change(&files, path) {
        Some(change) => file_diff(dir, &from, &to, change, context),
        None => Ok(String::new()),
    }
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

/// The error for git output this module cannot read.
fn malformed(args: &[&str]) -> GitError {
    GitError {
        args: owned_args(args),
        stderr: "unexpected output".to_owned(),
    }
}

/// The id of the commit `rev` names; an error when it names no commit.
fn commit_id(dir: &Path, rev: &str) -> Result<String, GitError> {
    check_rev("rev-parse", rev)?;
    let spec = format!("{rev}^{{commit}}");
    let ran = exec_stop(
        dir,
        &[],
        &["rev-parse", "--verify", "--quiet", spec.as_str()],
        &[],
        None,
    )?;
    let id = ran.stdout.trim();
    if ran.code == Some(0) && !id.is_empty() {
        Ok(id.to_owned())
    } else {
        Err(GitError {
            args: owned_args(&["rev-parse", "--verify", rev]),
            stderr: format!("{rev} does not name a commit"),
        })
    }
}

/// The tree a commit's diff starts from: its first parent, or the empty tree
/// for a root commit.
fn base_of(dir: &Path, parents: &[String]) -> Result<String, GitError> {
    match parents.first() {
        Some(parent) => Ok(parent.clone()),
        None => empty_tree(dir),
    }
}

/// The id of git's empty tree in this repository's hash algorithm.
fn empty_tree(dir: &Path) -> Result<String, GitError> {
    let out = run(dir, &[], &["hash-object", "-t", "tree", "--stdin"])?;
    Ok(out.trim().to_owned())
}

/// The files that differ between the trees of commits `base` and `rev`, with
/// renames detected. Two `diff-tree` runs share git's order: one gives the
/// status and paths, one the line counts, joined on the new path.
fn changes(dir: &Path, base: &str, rev: &str) -> Result<Vec<FileChange>, GitError> {
    let names = run(
        dir,
        &[],
        &[
            "diff-tree",
            "-r",
            "-z",
            "-M",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--name-status",
            base,
            rev,
        ],
    )?;
    let stats = run(
        dir,
        &[],
        &[
            "diff-tree",
            "-r",
            "-z",
            "-M",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--numstat",
            base,
            rev,
        ],
    )?;
    let bad = || malformed(&["diff-tree", base, rev]);

    // With -z a numstat record is "added\tdeleted\tpath\0"; a rename has an
    // empty path there and its old and new paths as the next two fields.
    let mut counts: HashMap<&str, (Option<u64>, Option<u64>)> = HashMap::new();
    let mut tokens = stats.split('\0').filter(|t| !t.is_empty());
    while let Some(token) = tokens.next() {
        let mut cols = token.splitn(3, '\t');
        let (Some(added), Some(deleted), Some(path)) = (cols.next(), cols.next(), cols.next())
        else {
            return Err(bad());
        };
        let path = if path.is_empty() {
            tokens.next();
            tokens.next().ok_or_else(bad)?
        } else {
            path
        };
        // "-" for a binary file parses to None.
        counts.insert(path, (added.parse().ok(), deleted.parse().ok()));
    }

    // A name-status record is "status\0path\0", or "status\0old\0new\0" for a
    // rename or copy.
    let mut tokens = names.split('\0').filter(|t| !t.is_empty());
    let mut files = Vec::new();
    while let Some(status) = tokens.next() {
        let code = status.as_bytes().first().copied().ok_or_else(bad)?;
        let (old_path, path) = if matches!(code, b'R' | b'C') {
            let old = tokens.next().ok_or_else(bad)?;
            (Some(old.to_owned()), tokens.next())
        } else {
            (None, tokens.next())
        };
        let path = path.ok_or_else(bad)?;
        let (added, deleted) = counts.get(path).copied().unwrap_or((None, None));
        files.push(FileChange {
            path: path.to_owned(),
            old_path,
            status: code,
            added,
            deleted,
        });
    }
    Ok(files)
}

/// The change to `path` in `files`, matched by its new path or, for a rename,
/// its old path.
fn find_change<'a>(files: &'a [FileChange], path: &str) -> Option<&'a FileChange> {
    files
        .iter()
        .find(|c| c.path == path || c.old_path.as_deref() == Some(path))
}

/// The unified diff of one file between trees `from` and `to`. Both paths of a
/// rename are given, so git pairs them and shows the rename.
fn file_diff(
    dir: &Path,
    from: &str,
    to: &str,
    change: &FileChange,
    context: usize,
) -> Result<String, GitError> {
    let context = format!("-U{context}");
    let mut args = vec![
        "--literal-pathspecs",
        "diff",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "-M",
        context.as_str(),
        from,
        to,
        "--",
    ];
    if let Some(old) = change.old_path.as_deref() {
        args.push(old);
    }
    args.push(change.path.as_str());
    run(dir, &[], &args)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use super::*;
    use crate::testrepo::{commit_all, git, repo, write};

    fn page(limit: usize) -> LogQuery {
        LogQuery {
            limit,
            ..LogQuery::default()
        }
    }

    fn subjects(entries: &[LogEntry]) -> Vec<&str> {
        entries.iter().map(|e| e.subject.as_str()).collect()
    }

    fn commit_each(dir: &Path, messages: &[&str]) {
        for msg in messages {
            write(dir, &format!("{msg}.txt"), msg);
            commit_all(dir, msg);
        }
    }

    fn change(path: &str, status: u8, added: Option<u64>, deleted: Option<u64>) -> FileChange {
        FileChange {
            path: path.to_owned(),
            old_path: None,
            status,
            added,
            deleted,
        }
    }

    #[test]
    fn log_lists_linear_commits_newest_first() {
        let dir = repo();
        commit_each(dir.path(), &["one", "two", "three"]);
        let stop = AtomicBool::new(false);
        let entries = log(dir.path(), &page(10), &stop).unwrap();
        assert_eq!(subjects(&entries), ["three", "two", "one"]);
        assert_eq!(entries[0].parents, [entries[1].id.as_str()]);
        assert_eq!(entries[1].parents, [entries[2].id.as_str()]);
        assert!(entries[2].parents.is_empty());
        assert_eq!(entries[0].author, "Test");
        assert_eq!(entries[0].email, "t@example.com");
        assert!(entries[0].time > 0);
    }

    #[test]
    fn log_pages_with_skip_and_limit() {
        let dir = repo();
        commit_each(dir.path(), &["one", "two", "three"]);
        let stop = AtomicBool::new(false);
        let middle = LogQuery {
            skip: 1,
            limit: 1,
            ..LogQuery::default()
        };
        let entries = log(dir.path(), &middle, &stop).unwrap();
        assert_eq!(subjects(&entries), ["two"]);
        let past_end = LogQuery {
            skip: 5,
            limit: 10,
            ..LogQuery::default()
        };
        assert!(log(dir.path(), &past_end, &stop).unwrap().is_empty());
        assert!(log(dir.path(), &page(0), &stop).unwrap().is_empty());
    }

    #[test]
    fn log_of_unborn_repository_is_empty() {
        let dir = repo();
        let stop = AtomicBool::new(false);
        assert!(log(dir.path(), &page(10), &stop).unwrap().is_empty());
        let all = LogQuery {
            all: true,
            limit: 10,
            ..LogQuery::default()
        };
        assert!(log(dir.path(), &all, &stop).unwrap().is_empty());
    }

    #[test]
    fn log_of_a_merge_has_both_parents_and_all_reaches_other_branches() {
        let dir = repo();
        let d = dir.path();
        commit_each(d, &["base"]);
        git(d, &["checkout", "-q", "-b", "feature"]);
        commit_each(d, &["feature side"]);
        git(d, &["checkout", "-q", "main"]);
        commit_each(d, &["main side"]);
        git(d, &["merge", "-q", "--no-ff", "feature", "-m", "merge it"]);
        git(d, &["checkout", "-q", "-b", "other"]);
        commit_each(d, &["other work"]);
        git(d, &["checkout", "-q", "main"]);

        let stop = AtomicBool::new(false);
        let head = log(d, &page(10), &stop).unwrap();
        let head_subjects = subjects(&head);
        assert!(head_subjects.contains(&"feature side"));
        assert!(head_subjects.contains(&"main side"));
        assert!(!head_subjects.contains(&"other work"));
        let merge = head.iter().find(|e| e.subject == "merge it").unwrap();
        assert_eq!(merge.parents.len(), 2);

        let all = LogQuery {
            all: true,
            limit: 10,
            ..LogQuery::default()
        };
        let everything = log(d, &all, &stop).unwrap();
        assert!(subjects(&everything).contains(&"other work"));
    }

    #[test]
    fn log_entries_carry_branch_and_tag_refs() {
        let dir = repo();
        commit_each(dir.path(), &["one"]);
        git(dir.path(), &["tag", "v1"]);
        let stop = AtomicBool::new(false);
        let entries = log(dir.path(), &page(1), &stop).unwrap();
        assert!(
            entries[0]
                .refs
                .iter()
                .any(|r| r == "HEAD -> refs/heads/main")
        );
        assert!(entries[0].refs.iter().any(|r| r == "tag: refs/tags/v1"));
    }

    #[test]
    fn stopped_log_fails_with_context_canceled() {
        let dir = repo();
        commit_each(dir.path(), &["one"]);
        let stop = AtomicBool::new(true);
        let err = log(dir.path(), &page(10), &stop).unwrap_err();
        assert_eq!(err.stderr, "context canceled");
    }

    #[test]
    fn log_and_show_reject_option_like_revisions() {
        let dir = repo();
        commit_each(dir.path(), &["one"]);
        let stop = AtomicBool::new(false);
        let query = LogQuery {
            revs: vec!["--all".to_owned()],
            limit: 10,
            ..LogQuery::default()
        };
        assert!(log(dir.path(), &query, &stop).is_err());
        assert!(show(dir.path(), "--output=x").is_err());
    }

    #[test]
    fn show_lists_root_files_with_counts_and_binary_without_counts() {
        let dir = repo();
        let d = dir.path();
        write(d, "a.txt", "one\ntwo\n");
        write(d, "b.bin", "bin\0ary\n");
        commit_all(d, "subject here\n\nbody text");
        let detail = show(d, "HEAD").unwrap();
        assert!(detail.entry.parents.is_empty());
        assert_eq!(detail.entry.subject, "subject here");
        assert_eq!(detail.message, "subject here\n\nbody text");
        assert_eq!(detail.committer, "Test");
        assert_eq!(detail.committer_email, "t@example.com");
        assert!(detail.committer_time > 0);
        assert_eq!(
            detail.files,
            [
                change("a.txt", b'A', Some(2), Some(0)),
                change("b.bin", b'A', None, None),
            ]
        );
    }

    #[test]
    fn show_reports_a_rename_with_its_old_path() {
        let dir = repo();
        let d = dir.path();
        write(d, "a.txt", "same content\n");
        commit_all(d, "root");
        git(d, &["mv", "a.txt", "c.txt"]);
        commit_all(d, "rename");
        let detail = show(d, "HEAD").unwrap();
        assert_eq!(
            detail.files,
            [FileChange {
                path: "c.txt".to_owned(),
                old_path: Some("a.txt".to_owned()),
                status: b'R',
                added: Some(0),
                deleted: Some(0),
            }]
        );
    }

    #[test]
    fn commit_diff_follows_a_rename_and_shows_the_edit() {
        let dir = repo();
        let d = dir.path();
        let lines: Vec<String> = (1..=10).map(|n| format!("line {n}")).collect();
        let body = lines.join("\n") + "\n";
        write(d, "a.txt", &body);
        commit_all(d, "root");
        git(d, &["mv", "a.txt", "b.txt"]);
        write(d, "b.txt", &body.replace("line 5\n", "line five\n"));
        commit_all(d, "rename and edit");
        let diff = commit_diff(d, "HEAD", "b.txt", 3).unwrap();
        assert!(diff.contains("rename from a.txt"), "{diff}");
        assert!(diff.contains("+line five"), "{diff}");
        assert_eq!(commit_diff(d, "HEAD", "other.txt", 3).unwrap(), "");
    }

    #[test]
    fn compare_lists_exactly_the_differing_files() {
        let dir = repo();
        let d = dir.path();
        write(d, "a.txt", "old a\n");
        write(d, "b.txt", "b\n");
        write(d, "kept.txt", "kept\n");
        commit_all(d, "base");
        git(d, &["checkout", "-q", "-b", "feature"]);
        write(d, "a.txt", "new a\n");
        git(d, &["rm", "-q", "b.txt"]);
        write(d, "c.txt", "something else entirely\n");
        commit_all(d, "feature");
        git(d, &["checkout", "-q", "main"]);

        assert_eq!(
            compare(d, "main", "feature").unwrap(),
            [
                change("a.txt", b'M', Some(1), Some(1)),
                change("b.txt", b'D', Some(0), Some(1)),
                change("c.txt", b'A', Some(1), Some(0)),
            ]
        );
        let diff = compare_diff(d, "main", "feature", "a.txt", 0).unwrap();
        assert!(diff.contains("-old a"), "{diff}");
        assert!(diff.contains("+new a"), "{diff}");
        assert_eq!(
            compare_diff(d, "main", "feature", "kept.txt", 0).unwrap(),
            ""
        );
        assert!(compare(d, "no-such-rev", "feature").is_err());
    }

    #[test]
    fn paths_with_spaces_and_unicode_survive_in_files() {
        let dir = repo();
        let d = dir.path();
        write(d, "with space.txt", "x\n");
        write(d, "ünï/codé.txt", "y\n");
        write(d, "two\nlines.txt", "z\n");
        commit_all(d, "names");
        let detail = show(d, "HEAD").unwrap();
        let paths: Vec<&str> = detail.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["two\nlines.txt", "with space.txt", "ünï/codé.txt"]);
        let diff = commit_diff(d, "HEAD", "with space.txt", 0).unwrap();
        assert!(diff.contains("+x"), "{diff}");
    }

    #[test]
    fn parses_records_with_and_without_parents_and_refs() {
        let out = "a1\x1fb2 c3\x1fAna\x1fana@x\x1f100\x1fHEAD -> main, tag: v1\x1fmerge it\x1e\n\
                   b2\x1f\x1fBo\x1fbo@x\x1f50\x1f\x1froot\x1e\n";
        let entries = parse_log(out);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].parents, ["b2", "c3"]);
        assert_eq!(entries[0].refs, ["HEAD -> main", "tag: v1"]);
        assert_eq!(entries[0].time, 100);
        assert!(entries[1].parents.is_empty());
        assert!(entries[1].refs.is_empty());
        assert_eq!(entries[1].subject, "root");
    }
}
