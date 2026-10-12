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
struct Ran {
    stdout: String,
    /// `None` when git was killed by a signal.
    code: Option<i32>,
    stderr: String,
}

fn owned_args(args: &[&str]) -> Vec<String> {
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
fn exec(dir: &Path, stdin: &[u8], args: &[&str]) -> Result<Ran, GitError> {
    let fail = |stderr: String| GitError {
        args: owned_args(args),
        stderr,
    };
    if cancelled() {
        return Err(fail("context canceled".to_owned()));
    }
    let mut child = Command::new("git")
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
fn run(dir: &Path, stdin: &[u8], args: &[&str]) -> Result<String, GitError> {
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
}
