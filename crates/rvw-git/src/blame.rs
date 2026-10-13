//! The past of one file: the tree at a revision, a file's content at a
//! revision, its history across renames, and blame.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::AtomicBool;

use crate::gitx::{self, owned_args, run, run_stop, GitError};
use crate::history::{parse_log, LogEntry};

/// The `--format` of [`file_history`]: each record starts with 0x1e and holds
/// the seven fields of `LOG_FORMAT`, so the name-status lines that follow a
/// record belong to it.
const HISTORY_FORMAT: &str = "--format=%x1e%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%D%x1f%s";

/// Every file path in the tree of `rev`, sorted as git lists them. Submodules (gitlinks) are left out.
pub fn tree(dir: &Path, rev: &str) -> Result<Vec<String>, GitError> {
    check_rev(&["ls-tree"], rev)?;
    let out = run(dir, &[], &["ls-tree", "-r", "-z", "--full-tree", rev])?;
    Ok(out
        .split('\0')
        .filter_map(|record| {
            // `<mode> <type> <id>\t<path>`
            let (meta, path) = record.split_once('\t')?;
            let kind = meta.split(' ').nth(1)?;
            (kind == "blob").then(|| path.to_owned())
        })
        .collect())
}

/// The content of `path` at `rev`; None when `rev`'s tree has no such file. Non-UTF-8 bytes are replaced (lossy).
/// Fails when `rev` does not name a commit.
pub fn file_at(dir: &Path, rev: &str, path: &str) -> Result<Option<String>, GitError> {
    check_rev(&["cat-file"], rev)?;
    let spec = format!("{rev}:{path}");
    let ran = gitx::exec(dir, &[], &["cat-file", "-e", spec.as_str()])?;
    if ran.code != Some(0) {
        // A missing path and an unknown revision both exit non-zero. Only the
        // unknown revision is an error.
        let tree = format!("{rev}^{{tree}}");
        let known = gitx::exec(
            dir,
            &[],
            &["rev-parse", "--verify", "--quiet", tree.as_str()],
        )?;
        if known.code == Some(0) {
            return Ok(None);
        }
        return Err(GitError {
            args: owned_args(&["cat-file", "-e", spec.as_str()]),
            stderr: ran.stderr,
        });
    }
    run(dir, &[], &["cat-file", "blob", spec.as_str()]).map(Some)
}

/// One commit in a file's history, with the file's path in that commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRevision {
    pub entry: LogEntry,
    /// The file's path in this commit; differs from the asked path before a rename.
    pub path: String,
}

/// A page of the commits that changed `path`, newest first, following renames (`--follow`), starting at HEAD.
/// Fails when `stop` is set or git fails.
pub fn file_history(
    dir: &Path,
    path: &str,
    skip: usize,
    limit: usize,
    stop: &AtomicBool,
) -> Result<Vec<FileRevision>, GitError> {
    // git 2.53 ignores `--skip` with `--follow`, so read the first
    // `skip + limit` commits and drop the first `skip` of them.
    let max_count = format!("--max-count={}", skip.saturating_add(limit));
    let args = [
        "--literal-pathspecs",
        "log",
        "--follow",
        "--no-color",
        "--name-status",
        "-z",
        HISTORY_FORMAT,
        max_count.as_str(),
        "--",
        path,
    ];
    let out = run_stop(dir, &args, stop)?;
    Ok(parse_history(&out, path).into_iter().skip(skip).collect())
}

/// Parses the output of [`file_history`]. Walking from HEAD, the file's name
/// changes at each rename, so a commit without a status line keeps the
/// name the newer commit left it under.
fn parse_history(out: &str, asked: &str) -> Vec<FileRevision> {
    let mut revisions = Vec::new();
    let mut path = asked.to_owned();
    for record in out.split('\x1e').skip(1) {
        let (header, names) = record.split_once('\0').unwrap_or((record, ""));
        let Some(entry) = parse_log(&format!("{header}\x1e")).pop() else {
            continue;
        };
        // `<status>\0<path>\0` or, for a rename or copy, `<status>\0<old>\0<new>\0`.
        let names = names.trim_start_matches('\n');
        let tokens: Vec<&str> = names.split('\0').filter(|t| !t.is_empty()).collect();
        let (here, before) = match tokens.as_slice() {
            [status, old, new, ..] if matches!(status.as_bytes().first(), Some(b'R' | b'C')) => {
                ((*new).to_owned(), (*old).to_owned())
            }
            [_, only, ..] => ((*only).to_owned(), (*only).to_owned()),
            _ => (path.clone(), path.clone()),
        };
        revisions.push(FileRevision { entry, path: here });
        path = before;
    }
    revisions
}

/// One line of blame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlameLine {
    /// 1-based line number in the blamed version.
    pub line: usize,
    /// The commit that last changed the line; all zeros for a line not committed yet.
    pub id: String,
    pub author: String,
    /// Author time, seconds since the Unix epoch.
    pub time: i64,
    /// The commit's subject.
    pub summary: String,
    /// The line's text without its newline.
    pub text: String,
}

/// Blame of `path` at `rev`, or of the file on disk (uncommitted lines included) when `rev` is None.
/// Fails when the path is not in the revision, `stop` is set, or git fails.
pub fn blame(
    dir: &Path,
    rev: Option<&str>,
    path: &str,
    stop: &AtomicBool,
) -> Result<Vec<BlameLine>, GitError> {
    let mut args = vec!["blame", "--porcelain"];
    if let Some(rev) = rev {
        check_rev(&["blame"], rev)?;
        args.push(rev);
    }
    args.extend(["--", path]);
    let out = run_stop(dir, &args, stop)?;
    parse_blame(&out).ok_or_else(|| GitError {
        args: owned_args(&args),
        stderr: "unreadable blame output".to_owned(),
    })
}

/// What the porcelain format says about one commit. It is printed the first
/// time the commit appears and never again.
#[derive(Debug, Clone, Default)]
struct CommitMeta {
    author: String,
    time: i64,
    summary: String,
}

/// Parses `git blame --porcelain`. None when the output is not in that format.
fn parse_blame(out: &str) -> Option<Vec<BlameLine>> {
    let body = out.strip_suffix('\n').unwrap_or(out);
    let mut lines = Vec::new();
    if body.is_empty() {
        return Some(lines);
    }
    let mut metas: HashMap<&str, CommitMeta> = HashMap::new();
    // The commit id and final line number of the line being described.
    let mut current: Option<(&str, usize)> = None;
    for raw in body.split('\n') {
        if let Some(text) = raw.strip_prefix('\t') {
            // The content line closes the group of one blamed line.
            let (id, line) = current.take()?;
            let meta = metas.get(id).cloned().unwrap_or_default();
            lines.push(BlameLine {
                line,
                id: id.to_owned(),
                author: meta.author,
                time: meta.time,
                summary: meta.summary,
                text: text.to_owned(),
            });
        } else if let Some((id, _)) = current {
            // A `key value` line of the commit's metadata.
            let (key, value) = raw.split_once(' ').unwrap_or((raw, ""));
            let meta = metas.entry(id).or_default();
            match key {
                "author" => value.clone_into(&mut meta.author),
                "author-time" => meta.time = value.parse().unwrap_or(0),
                "summary" => value.clone_into(&mut meta.summary),
                _ => {}
            }
        } else {
            // The header: `<id> <original line> <final line> [<lines in group>]`.
            let mut fields = raw.split(' ');
            let id = fields.next()?;
            let _original = fields.next()?;
            let line: usize = fields.next()?.parse().ok()?;
            metas.entry(id).or_default();
            current = Some((id, line));
        }
    }
    current.is_none().then_some(lines)
}

/// Rejects a revision that git would read as an option.
fn check_rev(op: &[&str], rev: &str) -> Result<(), GitError> {
    if rev.starts_with('-') {
        return Err(GitError {
            args: owned_args(op),
            stderr: format!("invalid revision '{rev}'"),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::AtomicBool;

    use super::*;
    use crate::testrepo::{commit_all, git, repo, write};

    fn head(dir: &Path) -> String {
        git(dir, &["rev-parse", "HEAD"]).trim().to_owned()
    }

    #[test]
    fn tree_lists_nested_paths_at_an_older_revision() {
        let dir = repo();
        write(dir.path(), "src/lib.rs", "a\n");
        write(dir.path(), "README.md", "r\n");
        commit_all(dir.path(), "c1");
        let first = head(dir.path());
        write(dir.path(), "docs/new.md", "n\n");
        commit_all(dir.path(), "c2");

        assert_eq!(
            tree(dir.path(), &first).unwrap(),
            ["README.md", "src/lib.rs"]
        );
        assert_eq!(
            tree(dir.path(), "HEAD").unwrap(),
            ["README.md", "docs/new.md", "src/lib.rs"]
        );
    }

    #[test]
    fn a_revision_that_starts_with_a_dash_is_rejected() {
        let dir = repo();
        write(dir.path(), "a.txt", "a\n");
        commit_all(dir.path(), "c1");
        let stop = AtomicBool::new(false);

        assert!(tree(dir.path(), "--output=x").is_err());
        assert!(file_at(dir.path(), "-p", "a.txt").is_err());
        assert!(blame(dir.path(), Some("--all"), "a.txt", &stop).is_err());
    }

    #[test]
    fn file_at_returns_content_at_a_revision_and_none_when_missing() {
        let dir = repo();
        write(dir.path(), "a.txt", "old\n");
        commit_all(dir.path(), "c1");
        let first = head(dir.path());
        write(dir.path(), "a.txt", "new\n");
        commit_all(dir.path(), "c2");

        assert_eq!(
            file_at(dir.path(), &first, "a.txt").unwrap().as_deref(),
            Some("old\n")
        );
        assert_eq!(
            file_at(dir.path(), "HEAD", "a.txt").unwrap().as_deref(),
            Some("new\n")
        );
        assert_eq!(file_at(dir.path(), "HEAD", "missing.txt").unwrap(), None);
        assert!(file_at(dir.path(), "no-such-rev", "a.txt").is_err());
    }

    #[test]
    fn file_at_replaces_invalid_utf8_bytes() {
        let dir = repo();
        fs::write(dir.path().join("bin.txt"), b"ok \xff end\n").unwrap();
        commit_all(dir.path(), "c1");

        let text = file_at(dir.path(), "HEAD", "bin.txt").unwrap().unwrap();
        assert_eq!(text, "ok \u{fffd} end\n");
    }

    #[test]
    fn file_history_follows_a_rename_and_pages_with_skip_and_limit() {
        let dir = repo();
        write(dir.path(), "a.txt", "one\n");
        commit_all(dir.path(), "c1");
        write(dir.path(), "a.txt", "one\ntwo\n");
        commit_all(dir.path(), "c2");
        git(dir.path(), &["mv", "a.txt", "b.txt"]);
        write(dir.path(), "b.txt", "one\ntwo\nthree\n");
        commit_all(dir.path(), "c3");
        write(dir.path(), "other.txt", "x\n");
        commit_all(dir.path(), "c4");
        let stop = AtomicBool::new(false);

        let all = file_history(dir.path(), "b.txt", 0, 10, &stop).unwrap();
        let got: Vec<(&str, &str)> = all
            .iter()
            .map(|r| (r.entry.subject.as_str(), r.path.as_str()))
            .collect();
        assert_eq!(got, [("c3", "b.txt"), ("c2", "a.txt"), ("c1", "a.txt")]);

        let page = file_history(dir.path(), "b.txt", 1, 1, &stop).unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].entry.subject, "c2");
        assert_eq!(page[0].path, "a.txt");
    }

    #[test]
    fn blame_attributes_each_line_to_the_commit_that_wrote_it() {
        let dir = repo();
        write(dir.path(), "a.txt", "first\nsecond\n");
        commit_all(dir.path(), "add a");
        write(dir.path(), "a.txt", "first\nchanged\nthird\n");
        git(dir.path(), &["add", "-A"]);
        git(
            dir.path(),
            &[
                "-c",
                "user.name=Bo",
                "-c",
                "user.email=bo@example.com",
                "commit",
                "-q",
                "-m",
                "edit a",
            ],
        );
        let stop = AtomicBool::new(false);

        let lines = blame(dir.path(), Some("HEAD"), "a.txt", &stop).unwrap();
        let got: Vec<(usize, &str, &str, &str)> = lines
            .iter()
            .map(|l| {
                (
                    l.line,
                    l.author.as_str(),
                    l.summary.as_str(),
                    l.text.as_str(),
                )
            })
            .collect();
        assert_eq!(
            got,
            [
                (1, "Test", "add a", "first"),
                (2, "Bo", "edit a", "changed"),
                (3, "Bo", "edit a", "third"),
            ]
        );
        assert_eq!(lines[0].id.len(), 40);
        assert_ne!(lines[0].id, lines[1].id);
        assert_eq!(lines[1].id, lines[2].id);
        assert!(lines[0].time > 0);
        assert!(blame(dir.path(), Some("HEAD"), "nope.txt", &stop).is_err());
    }

    #[test]
    fn blame_of_the_worktree_marks_uncommitted_lines_with_zero_ids() {
        let dir = repo();
        write(dir.path(), "a.txt", "one\ntwo\n");
        commit_all(dir.path(), "c1");
        let committed = head(dir.path());
        write(dir.path(), "a.txt", "one\n2\n");
        let stop = AtomicBool::new(false);

        let lines = blame(dir.path(), None, "a.txt", &stop).unwrap();
        assert_eq!(lines[0].id, committed);
        assert_eq!(lines[1].id, "0".repeat(40));
        assert_eq!(lines[1].line, 2);
        assert_eq!(lines[1].text, "2");
    }

    #[test]
    fn stopped_blame_and_history_fail_with_context_canceled() {
        let dir = repo();
        write(dir.path(), "a.txt", "one\n");
        commit_all(dir.path(), "c1");
        let stop = AtomicBool::new(true);

        let err = blame(dir.path(), Some("HEAD"), "a.txt", &stop).unwrap_err();
        assert_eq!(err.stderr, "context canceled");
        let err = file_history(dir.path(), "a.txt", 0, 10, &stop).unwrap_err();
        assert_eq!(err.stderr, "context canceled");
    }
}
