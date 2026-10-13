//! Searching commits by message, author or content, a page at a time and
//! stoppable, so a search the next keystroke replaces does not keep running.

use std::path::Path;
use std::sync::atomic::AtomicBool;

use crate::gitx::{self, owned_args, run_stop, GitError};
use crate::history::{parse_log, LogEntry, LOG_FORMAT};

/// What a commit search matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    /// The commit message, case-insensitive, as a fixed string.
    Message,
    /// The author's name or email, case-insensitive, as a fixed string.
    Author,
    /// Commits that add or remove the text in their diff (git's pickaxe, `-S`), as a fixed string, case-sensitive.
    Content,
}

/// One search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    /// The text to find. Matched as given; blank text finds nothing.
    pub text: String,
    /// The part of a commit the text is matched against.
    pub field: Field,
    /// Search every branch, remote branch and tag (`--branches --remotes --tags HEAD`) instead of HEAD only.
    pub all: bool,
    /// Matching commits to skip, newest first.
    pub skip: usize,
    /// At most this many commits; 0 returns empty without running git.
    pub limit: usize,
}

/// A page of the commits matching `query`, newest first in topological order.
/// Blank `text` (after trim) and a `limit` of 0 return empty without running
/// git. An unborn HEAD has no commits to search and returns empty too. Fails
/// when git fails, for example when `dir` is not a repository, and with
/// "context canceled" when `stop` is set.
pub fn commits(dir: &Path, query: &Query, stop: &AtomicBool) -> Result<Vec<LogEntry>, GitError> {
    if query.text.trim().is_empty() || query.limit == 0 {
        return Ok(Vec::new());
    }
    let Some(revs) = revisions(dir, query.all, stop)? else {
        return Ok(Vec::new());
    };

    let skip = format!("--skip={}", query.skip);
    let limit = query.limit.to_string();
    let pattern = match query.field {
        Field::Message => format!("--grep={}", query.text),
        Field::Author => format!("--author={}", query.text),
        Field::Content => format!("-S{}", query.text),
    };
    let mut args = vec![
        "log",
        "--topo-order",
        LOG_FORMAT,
        skip.as_str(),
        "-n",
        limit.as_str(),
    ];
    // Pickaxe `-S` is a fixed string by default and case-sensitive, so only
    // the two pattern options take the case and fixed-string flags.
    if query.field != Field::Content {
        args.extend(["--regexp-ignore-case", "--fixed-strings"]);
    }
    // One argument each, so a text that starts with `-` is a value.
    args.push(pattern.as_str());
    args.extend(revs);
    args.push("--");

    let out = run_stop(dir, &args, stop)?;
    Ok(parse_log(&out))
}

/// The revisions to walk. None when there is nothing to search: HEAD has no
/// commit yet and `all` is false. With `all`, an unborn HEAD is left out, so
/// a repository whose only history is on other branches is still searched.
fn revisions(
    dir: &Path,
    all: bool,
    stop: &AtomicBool,
) -> Result<Option<Vec<&'static str>>, GitError> {
    let args = ["rev-parse", "--verify", "-q", "HEAD"];
    let ran = gitx::exec_stop(dir, &[], &args, &[], Some(stop))?;
    let born = match ran.code {
        Some(0) => true,
        // `-q` makes an unborn HEAD exit 1 without a message.
        Some(1) => false,
        _ => {
            return Err(GitError {
                args: owned_args(&args),
                stderr: ran.stderr,
            })
        }
    };
    Ok(match (born, all) {
        (true, false) => Some(vec!["HEAD"]),
        (true, true) => Some(vec!["--branches", "--remotes", "--tags", "HEAD"]),
        (false, false) => None,
        (false, true) => Some(vec!["--branches", "--remotes", "--tags"]),
    })
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;
    use std::io::Write;
    use std::path::Path;
    use std::process::{Command, Stdio};
    use std::sync::atomic::AtomicBool;

    use tempfile::TempDir;

    use super::{commits, Field, Query};
    use crate::history::LogEntry;
    use crate::testrepo::{commit_all, git, repo, write};

    fn query(text: &str, field: Field) -> Query {
        Query {
            text: text.to_owned(),
            field,
            all: false,
            skip: 0,
            limit: 100,
        }
    }

    fn subjects(entries: &[LogEntry]) -> Vec<&str> {
        entries.iter().map(|e| e.subject.as_str()).collect()
    }

    /// Commits the staged files as `name <email>`, with `msg` as the subject.
    fn commit_as(dir: &Path, name: &str, email: &str, msg: &str) {
        let name = format!("user.name={name}");
        let email = format!("user.email={email}");
        git(dir, &["add", "-A"]);
        git(
            dir,
            &[
                "-c",
                name.as_str(),
                "-c",
                email.as_str(),
                "commit",
                "-q",
                "-m",
                msg,
            ],
        );
    }

    fn search(dir: &Path, query: &Query) -> Vec<LogEntry> {
        commits(dir, query, &AtomicBool::new(false)).unwrap()
    }

    #[test]
    fn message_search_ignores_case_and_matches_a_fixed_string() {
        let dir = repo();
        write(dir.path(), "a.txt", "1");
        commit_all(dir.path(), "Fix Parser");
        write(dir.path(), "a.txt", "2");
        commit_all(dir.path(), "add abc");
        write(dir.path(), "a.txt", "3");
        commit_all(dir.path(), "note a.c");

        let found = search(dir.path(), &query("fix parser", Field::Message));
        assert_eq!(subjects(&found), ["Fix Parser"]);

        let found = search(dir.path(), &query("a.c", Field::Message));
        assert_eq!(subjects(&found), ["note a.c"]);
    }

    #[test]
    fn author_search_matches_name_or_email_ignoring_case() {
        let dir = repo();
        write(dir.path(), "a.txt", "1");
        commit_as(dir.path(), "Ana", "ana@x", "Ana change");
        write(dir.path(), "a.txt", "2");
        commit_as(dir.path(), "Bo", "bo@x", "Bo change");

        let found = search(dir.path(), &query("ANA", Field::Author));
        assert_eq!(subjects(&found), ["Ana change"]);

        let found = search(dir.path(), &query("ana@x", Field::Author));
        assert_eq!(subjects(&found), ["Ana change"]);

        let found = search(dir.path(), &query("bo@x", Field::Author));
        assert_eq!(subjects(&found), ["Bo change"]);
    }

    #[test]
    fn content_search_finds_commits_that_add_or_remove_the_text() {
        let dir = repo();
        write(dir.path(), "src.rs", "let total = 42;\n");
        commit_all(dir.path(), "add total");
        write(dir.path(), "other.txt", "hello\n");
        commit_all(dir.path(), "unrelated");
        write(dir.path(), "src.rs", "let count = 1;\n");
        commit_all(dir.path(), "remove total");

        let found = search(dir.path(), &query("total = 42", Field::Content));
        assert_eq!(subjects(&found), ["remove total", "add total"]);

        // Pickaxe is case-sensitive.
        let found = search(dir.path(), &query("TOTAL = 42", Field::Content));
        assert!(found.is_empty());
    }

    #[test]
    fn text_starting_with_a_dash_is_a_value_not_an_option() {
        let dir = repo();
        write(dir.path(), "flags.txt", "--all\n");
        commit_all(dir.path(), "fix --all flag");
        write(dir.path(), "flags.txt", "");
        commit_all(dir.path(), "plain");

        let found = search(dir.path(), &query("--all", Field::Message));
        assert_eq!(subjects(&found), ["fix --all flag"]);

        let found = search(dir.path(), &query("--all", Field::Content));
        assert_eq!(subjects(&found), ["plain", "fix --all flag"]);
    }

    #[test]
    fn log_pages_with_skip_and_limit() {
        let dir = repo();
        for i in 1..=5 {
            write(dir.path(), "a.txt", &i.to_string());
            commit_all(dir.path(), &format!("page {i}"));
        }
        let mut q = query("page", Field::Message);
        q.limit = 2;
        assert_eq!(subjects(&search(dir.path(), &q)), ["page 5", "page 4"]);

        q.skip = 2;
        assert_eq!(subjects(&search(dir.path(), &q)), ["page 3", "page 2"]);

        q.skip = 4;
        assert_eq!(subjects(&search(dir.path(), &q)), ["page 1"]);

        q.skip = 0;
        q.limit = 0;
        assert!(search(dir.path(), &q).is_empty());
    }

    #[test]
    fn blank_text_and_zero_limit_return_empty_without_running_git() {
        // Not a repository: git would fail, so an empty Ok proves git never ran.
        let dir = TempDir::new().unwrap();
        let stop = AtomicBool::new(false);
        let blank = commits(dir.path(), &query("  \t", Field::Message), &stop);
        assert_eq!(blank, Ok(Vec::new()));
        let mut zero = query("fix", Field::Message);
        zero.limit = 0;
        assert_eq!(commits(dir.path(), &zero, &stop), Ok(Vec::new()));
    }

    #[test]
    fn all_searches_branches_that_head_does_not_reach() {
        let dir = repo();
        write(dir.path(), "a.txt", "1");
        commit_all(dir.path(), "base");
        git(dir.path(), &["checkout", "-q", "-b", "side"]);
        write(dir.path(), "a.txt", "2");
        commit_all(dir.path(), "side needle");
        git(dir.path(), &["checkout", "-q", "main"]);

        let mut q = query("needle", Field::Message);
        assert!(search(dir.path(), &q).is_empty());

        q.all = true;
        assert_eq!(subjects(&search(dir.path(), &q)), ["side needle"]);
    }

    #[test]
    fn unborn_head_returns_empty() {
        let dir = repo();
        let q = query("x", Field::Message);
        assert!(search(dir.path(), &q).is_empty());

        let mut all = q;
        all.all = true;
        assert!(search(dir.path(), &all).is_empty());
    }

    #[test]
    fn stopped_search_fails_with_context_canceled() {
        let dir = repo();
        write(dir.path(), "a.txt", "1");
        commit_all(dir.path(), "fix");
        let stop = AtomicBool::new(true);
        let err = commits(dir.path(), &query("fix", Field::Message), &stop).unwrap_err();
        assert_eq!(err.stderr, "context canceled");
    }

    #[test]
    fn message_search_stops_at_the_limit_on_a_long_history() {
        let dir = repo();
        let mut stream = String::new();
        for i in 1..=2000 {
            let msg = format!("commit {i}\n");
            let _ = write!(
                stream,
                "commit refs/heads/main\nmark :{i}\ncommitter Test <t@example.com> {} +0000\ndata {}\n{msg}",
                1_700_000_000 + i,
                msg.len(),
            );
            if i > 1 {
                let _ = writeln!(stream, "from :{}", i - 1);
            }
            stream.push('\n');
        }
        let mut child = Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(["fast-import", "--quiet"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stream.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );

        let found = search(
            dir.path(),
            &Query {
                limit: 50,
                ..query("commit", Field::Message)
            },
        );
        assert_eq!(found.len(), 50);
        assert_eq!(found[0].subject, "commit 2000");
    }
}
