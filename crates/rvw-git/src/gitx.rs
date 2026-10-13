//! Port of internal/gitx/gitx.go: git plumbing run as `git -C <dir> ...`.

use std::fmt;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, OnceLock};
use std::time::Duration;

/// A git command that failed. `stderr` is git's trimmed message (or the OS
/// error text when git could not be started).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitError {
    pub args: Vec<String>,
    pub stderr: String,
}

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "git {}: {}", self.args.join(" "), self.stderr)
    }
}

impl std::error::Error for GitError {}

/// One hunk header of a zero-context diff: the old file's lines
/// `old_start..old_start+old_lines-1` became the new file's `new_start..`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hunk {
    pub old_start: i64,
    pub old_lines: i64,
    pub new_start: i64,
    pub new_lines: i64,
}

/// A file that differs between a commit and the worktree. `status` is git's
/// letter (A, M, D, T, U) or `b'?'` for an untracked file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub path: String,
    pub status: u8,
}

/// Outcome of one git process that could be started.
pub(crate) struct Ran {
    pub(crate) stdout: String,
    /// `None` when git was killed by a signal.
    pub(crate) code: Option<i32>,
    pub(crate) stderr: String,
}

pub(crate) fn owned_args(args: &[&str]) -> Vec<String> {
    args.iter().map(|a| (*a).to_owned()).collect()
}

/// Set by the process's signal handlers; a git call in flight is killed and
/// reports `context canceled`, as the Go code's cancelled context did.
static CANCELLED: OnceLock<Arc<AtomicBool>> = OnceLock::new();

/// The shared cancellation flag. Register it with a signal handler.
pub fn cancel_flag() -> Arc<AtomicBool> {
    CANCELLED
        .get_or_init(|| Arc::new(AtomicBool::new(false)))
        .clone()
}

fn cancelled() -> bool {
    cancel_flag().load(Ordering::Relaxed)
}

/// How long a killed git may keep its pipes open (a grandchild can hold them).
const WAIT_DELAY: Duration = Duration::from_secs(1);

/// The text of an OS error the way Go prints it: lower case, no `(os error N)`.
pub fn os_error_text(e: &std::io::Error) -> String {
    let text = e.to_string();
    let text = text
        .rsplit_once(" (os error ")
        .map_or(text.as_str(), |(head, _)| head);
    let mut chars = text.chars();
    match chars.next() {
        Some(c) => c.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Runs `git -C dir args...` with `stdin` piped in and closed. Only a failure
/// to start git, or a cancellation, is an `Err`; a non-zero exit is reported
/// through `Ran::code`.
pub(crate) fn exec(dir: &Path, stdin: &[u8], args: &[&str]) -> Result<Ran, GitError> {
    let fail = |stderr: String| GitError {
        args: owned_args(args),
        stderr,
    };
    if cancelled() {
        return Err(fail("context canceled".to_owned()));
    }
    // No credential prompt can open on a terminal nobody sees, and a status
    // read never takes the index lock an agent's git command may need.
    let mut child = Command::new("git")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| fail(os_error_text(&e)))?;
    // Feed stdin and drain both output pipes on threads, so a large payload
    // cannot deadlock against git's pipes; a broken pipe just means git
    // stopped reading.
    if let Some(mut pipe) = child.stdin.take() {
        let input = stdin.to_vec();
        std::thread::spawn(move || {
            let _ = pipe.write_all(&input);
        });
    }
    let reader = |pipe: Option<Box<dyn Read + Send>>| {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_end(&mut buf);
            }
            let _ = tx.send(buf);
        });
        rx
    };
    let out_rx = reader(
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let err_rx = reader(
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );

    let mut killed = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(e) => return Err(fail(os_error_text(&e))),
        }
        if !killed && cancelled() {
            let _ = child.kill();
            killed = true;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let collect = |rx: mpsc::Receiver<Vec<u8>>| {
        if killed {
            rx.recv_timeout(WAIT_DELAY).unwrap_or_default()
        } else {
            rx.recv().unwrap_or_default()
        }
    };
    let stdout = collect(out_rx);
    let stderr = collect(err_rx);
    if killed {
        return Err(fail("context canceled".to_owned()));
    }
    Ok(Ran {
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        code: status.code(),
        stderr: String::from_utf8_lossy(&stderr).trim().to_owned(),
    })
}

/// Runs git and returns its stdout; any exit other than 0 is a `GitError`
/// carrying git's trimmed stderr.
pub(crate) fn run(dir: &Path, stdin: &[u8], args: &[&str]) -> Result<String, GitError> {
    let ran = exec(dir, stdin, args)?;
    if ran.code == Some(0) {
        Ok(ran.stdout)
    } else {
        Err(GitError {
            args: owned_args(args),
            stderr: ran.stderr,
        })
    }
}

/// The root of the worktree holding `dir`, if `dir` is inside one.
pub fn toplevel(dir: &Path) -> Option<PathBuf> {
    let out = run(dir, &[], &["rev-parse", "--show-toplevel"]).ok()?;
    let top = out.trim();
    if top.is_empty() {
        None
    } else {
        Some(PathBuf::from(top))
    }
}

/// Whether `rel` is in the index of the repository at `dir`. A git failure
/// other than "not tracked" is an error.
pub fn tracked(dir: &Path, rel: &str) -> Result<bool, GitError> {
    let args = [
        "--literal-pathspecs",
        "ls-files",
        "--error-unmatch",
        "--",
        rel,
    ];
    let ran = exec(dir, &[], &args)?;
    match ran.code {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(GitError {
            args: owned_args(&args),
            stderr: ran.stderr,
        }),
    }
}

/// Writes `content` to the object store and returns its blob id.
pub fn store_content(dir: &Path, content: &str) -> Result<String, GitError> {
    let out = run(dir, content.as_bytes(), &["hash-object", "-w", "--stdin"])?;
    blob_id(&out)
}

/// Writes the file at `rel` (relative to `dir`) to the object store.
pub fn store_file(dir: &Path, rel: &str) -> Result<String, GitError> {
    let out = run(dir, &[], &["hash-object", "-w", "--", rel])?;
    blob_id(&out)
}

fn blob_id(out: &str) -> Result<String, GitError> {
    let id = out.trim();
    if id.is_empty() {
        return Err(GitError {
            args: owned_args(&["hash-object"]),
            stderr: "no object id returned".to_owned(),
        });
    }
    Ok(id.to_owned())
}

/// Reads a blob back, if the repository at `dir` still has it. Non-UTF-8
/// content is converted lossily.
pub fn blob(dir: &Path, id: &str) -> Option<String> {
    run(dir, &[], &["cat-file", "blob", id]).ok()
}

/// Files git sees in the worktree: tracked plus untracked-not-ignored,
/// relative and slash-separated.
pub fn files(dir: &Path) -> Result<Vec<String>, GitError> {
    let out = run(
        dir,
        &[],
        &[
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ],
    )?;
    Ok(out
        .split('\0')
        .filter(|f| !f.is_empty())
        .map(str::to_owned)
        .collect())
}

/// Diffs `path` against commit `base`. The bool is `true` (with no hunks) for
/// an untracked, non-ignored file.
pub fn changes(dir: &Path, path: &str, base: &str) -> Result<(Vec<Hunk>, bool), GitError> {
    let others = run(
        dir,
        &[],
        &[
            "--literal-pathspecs",
            "ls-files",
            "--others",
            "--exclude-standard",
            "--",
            path,
        ],
    )?;
    if !others.trim().is_empty() {
        return Ok((Vec::new(), true));
    }
    let out = run(
        dir,
        &[],
        &[
            "--literal-pathspecs",
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "-U0",
            base,
            "--",
            path,
        ],
    )?;
    Ok((parse_hunks(&out), false))
}

/// The unified diff of `path` against commit `base`, with `context` lines of
/// context. A file git does not track yet is shown as entirely added.
pub fn unified_diff(
    dir: &Path,
    path: &str,
    base: &str,
    context: usize,
) -> Result<String, GitError> {
    let others = run(
        dir,
        &[],
        &[
            "--literal-pathspecs",
            "ls-files",
            "--others",
            "--exclude-standard",
            "--",
            path,
        ],
    )?;
    if !others.trim().is_empty() {
        return added_file_diff(dir, path);
    }
    let context_arg = format!("-U{context}");
    run(
        dir,
        &[],
        &[
            "--literal-pathspecs",
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            context_arg.as_str(),
            base,
            "--",
            path,
        ],
    )
}

/// The diff git would print for an untracked file: every line added. A binary
/// file (a NUL in its first 8000 bytes, as git tests) gets git's own notice.
fn added_file_diff(dir: &Path, path: &str) -> Result<String, GitError> {
    let bytes = std::fs::read(dir.join(path)).map_err(|e| GitError {
        args: owned_args(&["diff", "--", path]),
        stderr: os_error_text(&e),
    })?;
    if bytes.is_empty() {
        return Ok(String::new());
    }
    if bytes[..bytes.len().min(8000)].contains(&0) {
        return Ok(format!("Binary files /dev/null and b/{path} differ\n"));
    }
    let text = String::from_utf8_lossy(&bytes);
    let ends_with_newline = text.ends_with('\n');
    let body = if ends_with_newline {
        &text[..text.len() - 1]
    } else {
        &text[..]
    };
    let lines: Vec<&str> = body.split('\n').collect();
    let mut out = format!(
        "--- /dev/null\n+++ b/{path}\n@@ -0,0 +1,{} @@\n",
        lines.len()
    );
    for line in lines {
        out.push('+');
        out.push_str(line);
        out.push('\n');
    }
    if !ends_with_newline {
        out.push_str("\\ No newline at end of file\n");
    }
    Ok(out)
}

/// Reads the "@@ -a,b +c,d @@" headers of a unified diff; a count left out
/// is 1.
fn parse_hunks(diff: &str) -> Vec<Hunk> {
    let mut hunks = Vec::new();
    for line in diff.split('\n') {
        let Some(rest) = line.strip_prefix("@@ -") else {
            continue;
        };
        let (old, rest) = rest.split_once(" +").unwrap_or((rest, ""));
        let (new, _) = rest.split_once(" @@").unwrap_or((rest, ""));
        let (old_start, old_lines) = span(old);
        let (new_start, new_lines) = span(new);
        hunks.push(Hunk {
            old_start,
            old_lines,
            new_start,
            new_lines,
        });
    }
    hunks
}

/// Parses "start" or "start,count" as Go's strconv.Atoi would, with a bad
/// number read as 0; a count left out is 1.
fn span(s: &str) -> (i64, i64) {
    match s.split_once(',') {
        Some((a, b)) => (atoi(a), atoi(b)),
        None => (atoi(s), 1),
    }
}

fn atoi(s: &str) -> i64 {
    s.parse().unwrap_or(0)
}

/// What differs between `base` and the worktree, then untracked files last.
pub fn changed_files(dir: &Path, base: &str) -> Result<Vec<Change>, GitError> {
    let diff = run(
        dir,
        &[],
        &[
            "diff",
            "--name-status",
            "-z",
            "--no-renames",
            "--no-ext-diff",
            base,
            "--",
        ],
    )?;
    let mut out = Vec::new();
    let fields: Vec<&str> = diff.split('\0').collect();
    let mut i = 0;
    while i + 1 < fields.len() {
        if !fields[i].is_empty() {
            out.push(Change {
                path: fields[i + 1].to_owned(),
                status: fields[i].as_bytes()[0],
            });
        }
        i += 2;
    }
    let others = run(
        dir,
        &[],
        &["ls-files", "-z", "--others", "--exclude-standard"],
    )?;
    for f in others.split('\0').filter(|f| !f.is_empty()) {
        out.push(Change {
            path: f.to_owned(),
            status: b'?',
        });
    }
    Ok(out)
}

/// Lines added and deleted between `base` and the worktree (`--numstat`).
/// Binary files count for nothing.
pub fn line_changes(dir: &Path, base: &str) -> Result<(i64, i64), GitError> {
    let out = run(
        dir,
        &[],
        &[
            "diff",
            "--numstat",
            "--no-renames",
            "--no-ext-diff",
            base,
            "--",
        ],
    )?;
    let mut added = 0;
    let mut deleted = 0;
    for line in out.split('\n') {
        let (a, rest) = line.split_once('\t').unwrap_or((line, ""));
        let (d, _) = rest.split_once('\t').unwrap_or((rest, ""));
        added += atoi(a); // "-" for a binary file
        deleted += atoi(d);
    }
    Ok((added, deleted))
}

/// Resolves `rev` to a commit id.
pub fn commit(dir: &Path, rev: &str) -> Option<String> {
    let spec = format!("{rev}^{{commit}}");
    let out = run(
        dir,
        &[],
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &spec,
        ],
    )
    .ok()?;
    let id = out.trim();
    if id.is_empty() {
        None
    } else {
        Some(id.to_owned())
    }
}

/// The best common ancestor of commits `a` and `b`.
pub fn merge_base(dir: &Path, a: &str, b: &str) -> Result<String, GitError> {
    let out = run(dir, &[], &["merge-base", a, b])?;
    Ok(out.trim().to_owned())
}

/// The checked-out branch, `"detached <short id>"`, or `""` when unknown.
pub fn branch(dir: &Path) -> String {
    if let Ok(out) = run(dir, &[], &["symbolic-ref", "--short", "--quiet", "HEAD"]) {
        return out.trim().to_owned();
    }
    if let Ok(out) = run(dir, &[], &["rev-parse", "--short", "HEAD"]) {
        return format!("detached {}", out.trim());
    }
    String::new()
}

/// The repository's main line, if any.
pub fn default_branch(dir: &Path) -> Option<String> {
    if let Ok(out) = run(
        dir,
        &[],
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    ) {
        let reference = out.trim();
        if !reference.is_empty() && commit(dir, reference).is_some() {
            return Some(reference.to_owned());
        }
    }
    ["main", "master", "origin/main", "origin/master"]
        .into_iter()
        .find(|reference| commit(dir, reference).is_some())
        .map(str::to_owned)
}

/// A local branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchInfo {
    pub name: String,
    pub current: bool,
    pub upstream: Option<String>,
    /// The subject of the branch's tip commit.
    pub subject: String,
    /// The relative commit date from git, e.g. `3 days ago`.
    pub when: String,
}

/// Parses one `for-each-ref` line; `None` when the line does not have the
/// five NUL-separated fields.
fn parse_branch(line: &str) -> Option<BranchInfo> {
    let fields: Vec<&str> = line.split('\0').collect();
    let [head, name, upstream, subject, when] = fields.as_slice() else {
        return None;
    };
    Some(BranchInfo {
        name: (*name).to_owned(),
        current: *head == "*",
        upstream: (!upstream.is_empty()).then(|| (*upstream).to_owned()),
        subject: (*subject).to_owned(),
        when: (*when).to_owned(),
    })
}

/// Local branches, the current one first, then by most recent commit. A
/// detached or unborn HEAD has no current entry.
pub fn branches(dir: &Path) -> Result<Vec<BranchInfo>, GitError> {
    // NUL cannot occur in a ref name or a subject, and a subject has no
    // newline, so one line per branch and NUL between fields is unambiguous.
    let out = run(
        dir,
        &[],
        &[
            "for-each-ref",
            "--sort=-committerdate",
            "--format=%(HEAD)%00%(refname:lstrip=2)%00%(upstream:short)%00%(contents:subject)%00%(committerdate:relative)",
            "refs/heads",
        ],
    )?;
    let mut list: Vec<BranchInfo> = out.lines().filter_map(parse_branch).collect();
    // A stable sort keeps the recency order among the rest.
    list.sort_by_key(|b| !b.current);
    Ok(list)
}

/// Switches the working tree to the local branch `name`. Git's refusal (for
/// example, local changes that would be overwritten) is returned as is.
pub fn switch(dir: &Path, name: &str) -> Result<(), GitError> {
    if name.is_empty() || name.starts_with('-') {
        return Err(GitError {
            args: owned_args(&["switch", name]),
            stderr: format!("invalid branch name '{name}'"),
        });
    }
    if !branches(dir)?.iter().any(|b| b.name == name) {
        return Err(GitError {
            args: owned_args(&["switch", name]),
            stderr: format!("no local branch named '{name}'"),
        });
    }
    run(dir, &[], &["switch", name]).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Runs git in `dir` and returns its stdout; panics on failure.
    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    /// A fresh repository on branch main with a local identity and no signing.
    fn repo() -> TempDir {
        let dir = TempDir::new().unwrap();
        git(dir.path(), &["init", "-q", "-b", "main"]);
        git(dir.path(), &["config", "user.email", "t@example.com"]);
        git(dir.path(), &["config", "user.name", "Test"]);
        git(dir.path(), &["config", "commit.gpgsign", "false"]);
        dir
    }

    fn write(dir: &Path, rel: &str, content: &str) {
        fs::write(dir.join(rel), content).unwrap();
    }

    fn commit_all(dir: &Path, msg: &str) {
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", msg]);
    }

    #[test]
    fn toplevel_inside_and_outside_a_repo() {
        let r = repo();
        let sub = r.path().join("sub");
        fs::create_dir(&sub).unwrap();
        assert_eq!(
            toplevel(&sub).map(|p| fs::canonicalize(p).unwrap()),
            Some(fs::canonicalize(r.path()).unwrap())
        );

        let outside = TempDir::new().unwrap();
        assert_eq!(toplevel(outside.path()), None);
        assert_eq!(toplevel(&outside.path().join("missing")), None);
    }

    #[test]
    fn tracked_true_false_and_error_outside_repo() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "init");
        write(r.path(), "u.txt", "u\n");

        assert_eq!(tracked(r.path(), "a.txt"), Ok(true));
        assert_eq!(tracked(r.path(), "u.txt"), Ok(false));

        let outside = TempDir::new().unwrap();
        assert!(tracked(outside.path(), "a.txt").is_err());
    }

    #[test]
    fn store_content_round_trip_and_empty_content() {
        let r = repo();
        let id = store_content(r.path(), "hello\n").unwrap();
        assert_eq!(id.len(), 40);
        assert_eq!(blob(r.path(), &id).as_deref(), Some("hello\n"));

        let empty = store_content(r.path(), "").unwrap();
        assert_eq!(empty, "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391");
        assert_eq!(blob(r.path(), &empty).as_deref(), Some(""));

        assert_eq!(
            blob(r.path(), "0123456789012345678901234567890123456789"),
            None
        );
    }

    #[test]
    fn store_file_round_trip() {
        let r = repo();
        write(r.path(), "f.txt", "file body\n");
        let id = store_file(r.path(), "f.txt").unwrap();
        assert_eq!(blob(r.path(), &id).as_deref(), Some("file body\n"));
    }

    #[test]
    fn files_lists_tracked_and_untracked_honoring_gitignore() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        write(r.path(), ".gitignore", "*.log\n");
        commit_all(r.path(), "init");
        write(r.path(), "u.txt", "u\n");
        write(r.path(), "ignored.log", "x\n");

        let mut got = files(r.path()).unwrap();
        got.sort();
        assert_eq!(got, vec![".gitignore", "a.txt", "u.txt"]);
    }

    #[test]
    fn changes_parses_hunks_with_counts_insertions_and_deletions() {
        let r = repo();
        write(r.path(), "h.txt", "1\n2\n3\n4\n5\n");
        write(r.path(), "d.txt", "a\nb\nc\n");
        write(r.path(), "x.txt", "a\n");
        commit_all(r.path(), "init");

        // Two old lines become one, and a line is appended after line 5.
        write(r.path(), "h.txt", "1\nTWO\n4\n5\n6\n");
        let (hunks, untracked) = changes(r.path(), "h.txt", "HEAD").unwrap();
        assert!(!untracked);
        assert_eq!(
            hunks,
            vec![
                Hunk {
                    old_start: 2,
                    old_lines: 2,
                    new_start: 2,
                    new_lines: 1
                },
                Hunk {
                    old_start: 5,
                    old_lines: 0,
                    new_start: 5,
                    new_lines: 1
                },
            ]
        );

        // Pure deletion: nothing new, so the start sits after new line 1.
        write(r.path(), "d.txt", "a\nc\n");
        let (hunks, _) = changes(r.path(), "d.txt", "HEAD").unwrap();
        assert_eq!(
            hunks,
            vec![Hunk {
                old_start: 2,
                old_lines: 1,
                new_start: 1,
                new_lines: 0
            }]
        );

        // Single-line hunk header with omitted counts.
        write(r.path(), "x.txt", "b\n");
        let (hunks, _) = changes(r.path(), "x.txt", "HEAD").unwrap();
        assert_eq!(
            hunks,
            vec![Hunk {
                old_start: 1,
                old_lines: 1,
                new_start: 1,
                new_lines: 1
            }]
        );
    }

    #[test]
    fn changes_reports_untracked_file_with_no_hunks() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "init");
        write(r.path(), "u.txt", "u\n");

        assert_eq!(
            changes(r.path(), "u.txt", "HEAD").unwrap(),
            (Vec::new(), true)
        );
    }

    #[test]
    fn unified_diff_shows_modified_lines_with_context() {
        let r = repo();
        write(r.path(), "h.txt", "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n");
        commit_all(r.path(), "init");
        write(r.path(), "h.txt", "1\n2\n3\n4\nFIVE\n6\n7\n8\n9\n10\n");

        let text = unified_diff(r.path(), "h.txt", "HEAD", 3).unwrap();
        let (header, body) = text.split_once("@@ -2,7 +2,7 @@\n").expect("hunk header");
        assert!(header.starts_with("diff --git a/h.txt b/h.txt\n"));
        assert_eq!(body, " 2\n 3\n 4\n-5\n+FIVE\n 6\n 7\n 8\n");
    }

    #[test]
    fn unified_diff_shows_a_deleted_line_without_context() {
        let r = repo();
        write(r.path(), "d.txt", "a\nb\nc\n");
        commit_all(r.path(), "init");
        write(r.path(), "d.txt", "a\nc\n");

        let text = unified_diff(r.path(), "d.txt", "HEAD", 0).unwrap();
        // Git may append the enclosing line as context after the header.
        assert!(text.contains("@@ -2 +1,0 @@"), "{text:?}");
        assert!(text.ends_with("-b\n"), "{text:?}");
    }

    #[test]
    fn unified_diff_shows_an_untracked_file_as_added() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "init");
        write(r.path(), "new.txt", "x\ny\n");
        write(r.path(), "no_eol.txt", "p\nq");

        assert_eq!(
            unified_diff(r.path(), "new.txt", "HEAD", 3).unwrap(),
            "--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1,2 @@\n+x\n+y\n"
        );
        assert_eq!(
            unified_diff(r.path(), "no_eol.txt", "HEAD", 3).unwrap(),
            "--- /dev/null\n+++ b/no_eol.txt\n@@ -0,0 +1,2 @@\n+p\n+q\n\\ No newline at end of file\n"
        );
    }

    #[test]
    fn unified_diff_of_empty_or_binary_untracked_file() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "init");
        write(r.path(), "empty.txt", "");
        write(r.path(), "bin.dat", "a\0b");

        assert_eq!(unified_diff(r.path(), "empty.txt", "HEAD", 3).unwrap(), "");
        assert_eq!(
            unified_diff(r.path(), "bin.dat", "HEAD", 3).unwrap(),
            "Binary files /dev/null and b/bin.dat differ\n"
        );
    }

    #[test]
    fn unified_diff_is_empty_when_nothing_changed() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "init");

        assert_eq!(unified_diff(r.path(), "a.txt", "HEAD", 3).unwrap(), "");
    }

    #[test]
    fn changed_files_lists_modified_deleted_added_then_untracked() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        write(r.path(), "b.txt", "b\n");
        commit_all(r.path(), "init");

        write(r.path(), "a.txt", "changed\n");
        fs::remove_file(r.path().join("b.txt")).unwrap();
        write(r.path(), "c.txt", "c\n");
        git(r.path(), &["add", "c.txt"]);
        write(r.path(), "u.txt", "u\n");

        let got = changed_files(r.path(), "HEAD").unwrap();
        let want = vec![
            Change {
                path: "a.txt".into(),
                status: b'M',
            },
            Change {
                path: "b.txt".into(),
                status: b'D',
            },
            Change {
                path: "c.txt".into(),
                status: b'A',
            },
            Change {
                path: "u.txt".into(),
                status: b'?',
            },
        ];
        assert_eq!(got, want);
    }

    #[test]
    fn line_changes_counts_text_and_ignores_binary_and_untracked() {
        let r = repo();
        write(r.path(), "x.txt", "x\n");
        fs::write(r.path().join("bin.dat"), [0u8, 1, 2]).unwrap();
        commit_all(r.path(), "init");

        write(r.path(), "x.txt", "y\nz\n"); // +2 -1
        fs::write(r.path().join("bin.dat"), [0u8, 9, 9, 9]).unwrap(); // binary: 0
        write(r.path(), "u.txt", "many\nlines\nuntracked\n"); // not counted

        assert_eq!(line_changes(r.path(), "HEAD").unwrap(), (2, 1));
    }

    #[test]
    fn commit_resolves_revs_and_rejects_unknown() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "init");

        let id = commit(r.path(), "HEAD").unwrap();
        assert_eq!(id.len(), 40);
        assert_eq!(commit(r.path(), "no-such-rev"), None);
    }

    #[test]
    fn merge_base_finds_fork_point() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "base");
        let fork = commit(r.path(), "HEAD").unwrap();

        git(r.path(), &["branch", "feature"]);
        write(r.path(), "main.txt", "m\n");
        commit_all(r.path(), "main work");
        git(r.path(), &["checkout", "-q", "feature"]);
        write(r.path(), "f.txt", "f\n");
        commit_all(r.path(), "feature work");

        assert_eq!(merge_base(r.path(), "main", "feature").unwrap(), fork);
    }

    #[test]
    fn branch_named_unborn_detached_and_unknown() {
        let r = repo();
        // Unborn: no commit yet, the symbolic ref still names the branch.
        assert_eq!(branch(r.path()), "main");

        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "init");
        assert_eq!(branch(r.path()), "main");

        git(r.path(), &["checkout", "-q", "--detach"]);
        let short = branch(r.path());
        assert!(short.starts_with("detached "), "got {short:?}");
        assert!(short.len() > "detached ".len());

        let outside = TempDir::new().unwrap();
        assert_eq!(branch(outside.path()), "");
    }

    #[test]
    fn default_branch_from_local_names_and_origin_head() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "init");
        assert_eq!(default_branch(r.path()).as_deref(), Some("main"));

        let trunk = TempDir::new().unwrap();
        git(trunk.path(), &["init", "-q", "-b", "trunk"]);
        git(trunk.path(), &["config", "user.email", "t@example.com"]);
        git(trunk.path(), &["config", "user.name", "Test"]);
        git(trunk.path(), &["config", "commit.gpgsign", "false"]);
        write(trunk.path(), "a.txt", "a\n");
        commit_all(trunk.path(), "init");
        assert_eq!(default_branch(trunk.path()), None);

        // A clone has origin/HEAD, which wins over the local name.
        let parent = TempDir::new().unwrap();
        let clone = parent.path().join("clone");
        git(
            parent.path(),
            &[
                "clone",
                "-q",
                r.path().to_str().unwrap(),
                clone.to_str().unwrap(),
            ],
        );
        assert_eq!(default_branch(&clone).as_deref(), Some("origin/main"));
    }

    /// Commits everything with the given author and committer date.
    fn commit_at(dir: &Path, msg: &str, date: &str) {
        git(dir, &["add", "-A"]);
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["commit", "-q", "-m", msg])
            .env("GIT_AUTHOR_DATE", date)
            .env("GIT_COMMITTER_DATE", date)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "commit: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// A repository on main (2020), with `middle` (2022) and `newer` (2024)
    /// branches, and main checked out.
    fn three_branches() -> TempDir {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_at(r.path(), "init", "2020-01-01T00:00:00Z");
        git(r.path(), &["switch", "-q", "-c", "middle"]);
        write(r.path(), "a.txt", "middle\n");
        commit_at(r.path(), "middle work", "2022-01-01T00:00:00Z");
        git(r.path(), &["switch", "-q", "main"]);
        git(r.path(), &["switch", "-q", "-c", "newer"]);
        write(r.path(), "a.txt", "newer\n");
        commit_at(r.path(), "tab\there, café ✓", "2024-01-01T00:00:00Z");
        git(r.path(), &["switch", "-q", "main"]);
        r
    }

    #[test]
    fn branches_lists_current_first_then_newest() {
        let r = three_branches();
        let got = branches(r.path()).unwrap();
        let names: Vec<&str> = got.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, vec!["main", "newer", "middle"]);
        assert!(got[0].current);
        assert!(!got[1].current && !got[2].current);
        assert_eq!(got[0].upstream, None);
        assert!(!got[0].when.is_empty());
    }

    #[test]
    fn branches_keeps_subject_with_tab_and_non_ascii() {
        let r = three_branches();
        let got = branches(r.path()).unwrap();
        let newer = got.iter().find(|b| b.name == "newer").unwrap();
        assert_eq!(newer.subject, "tab\there, café ✓");
        assert!(!newer.current);
    }

    #[test]
    fn branches_on_detached_head_has_no_current() {
        let r = three_branches();
        git(r.path(), &["checkout", "-q", "--detach"]);
        let got = branches(r.path()).unwrap();
        assert_eq!(got.len(), 3);
        assert!(got.iter().all(|b| !b.current));
    }

    #[test]
    fn switch_changes_the_checked_out_branch() {
        let r = three_branches();
        switch(r.path(), "newer").unwrap();
        assert_eq!(branch(r.path()), "newer");
        assert!(branches(r.path()).unwrap()[0].current);
    }

    #[test]
    fn switch_to_missing_branch_errors() {
        let r = three_branches();
        let err = switch(r.path(), "nope").unwrap_err();
        assert!(err.stderr.contains("nope"));
        assert_eq!(branch(r.path()), "main");
    }

    #[test]
    fn switch_rejects_option_like_name_without_running_git() {
        let r = three_branches();
        let err = switch(r.path(), "--force").unwrap_err();
        assert_eq!(err.args, vec!["switch", "--force"]);
        assert!(switch(r.path(), "").is_err());
        assert_eq!(branch(r.path()), "main");
    }

    #[test]
    fn switch_refused_when_local_changes_would_be_overwritten() {
        let r = three_branches();
        // main's a.txt is "a"; newer's is "newer". Dirty it on main.
        write(r.path(), "a.txt", "dirty\n");
        let err = switch(r.path(), "newer").unwrap_err();
        assert!(!err.stderr.is_empty());
        assert_eq!(branch(r.path()), "main");
        assert_eq!(
            fs::read_to_string(r.path().join("a.txt")).unwrap(),
            "dirty\n"
        );
    }
}
