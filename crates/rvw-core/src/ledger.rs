//! The audit ledger: the commits of a range, the decided comments that landed
//! in them, the comments that matched none, the gate overrides and the
//! sessions behind the commits. A read: nothing is written, and every link is
//! computed here from git and the store, never recorded. The matching rules
//! are in `ledger_match`.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::AtomicBool;

use rvw_git::history::{self, LogEntry, LogQuery};
use rvw_git::{GitError, gitx, patch_id};
use time::{Date, Month, OffsetDateTime, PrimitiveDateTime, Time};

use crate::error::{Error, Result};
use crate::evidence::snapshot_diff;
use crate::ledger_match::{CommitFiles, FileFact, exact, link_all, same_commit, unlinked_reason};
use crate::operations::{LedgerCommit, LedgerInput, LedgerOutput, LinkedComment, UnlinkedComment};
use crate::service::{Service, format_time};
use crate::types::{Comment, Session, Status};

/// The most commits one ledger reads; a larger range is refused by name.
const MAX_COMMITS: usize = 5000;
/// Commits per page of the log.
const PAGE: usize = 500;
/// Seconds in a UTC day.
const DAY: i64 = 86_400;

impl Service {
    /// The audit ledger of a range: its commits oldest first, with the decided
    /// comments that landed in each, the comments that matched none, the gate
    /// overrides of the lane and the sessions behind the commits.
    ///
    /// # Errors
    /// Refuses a range that starts with `-`, a since or until that is neither
    /// RFC 3339 nor YYYY-MM-DD, the default branch with no range and no since,
    /// and a range of more than 5000 commits. A git failure is internal.
    pub fn ledger(&self, input: LedgerInput) -> Result<LedgerOutput> {
        let dir = Path::new(&input.workspace);
        let since = bound(&input.since, false)?;
        let until = bound(&input.until, true)?;
        let branch = gitx::branch(dir);
        let range = resolve_range(dir, &input.range, &branch, !input.since.is_empty())?;

        let mut entries = read_range(dir, &range)?;
        entries.retain(|e| since.is_none_or(|s| e.time >= s) && until.is_none_or(|u| e.time <= u));

        // Store reads first, then git: a slow git never holds the database lock.
        let (comments, overrides, mut sessions) = self.read(&input.workspace, |tx| {
            Ok((
                tx.comments(&[Status::Done])?,
                tx.overrides()?,
                tx.sessions()?,
            ))
        })?;
        let comments: Vec<Comment> = comments
            .into_iter()
            .filter(|c| in_lane(c, &input.lane))
            .collect();

        // Only the files a comment in scope names are read from git.
        let watched: HashSet<&str> = comments.iter().map(|c| c.file.as_str()).collect();
        let mut files = entries
            .iter()
            .map(|e| commit_files(dir, &e.id, &watched))
            .collect::<Result<Vec<_>>>()?;

        // The earliest-reported session owns a commit first, so sort oldest first.
        sessions.sort_by(|a, b| {
            a.reported_at
                .cmp(&b.reported_at)
                .then_with(|| a.session_id.cmp(&b.session_id))
        });

        let (linked, unlinked) = match_comments(dir, comments, &mut files)?;

        let mut commits = Vec::with_capacity(entries.len());
        for ((entry, facts), comments) in entries.into_iter().zip(&files).zip(linked) {
            let owner = sessions.iter().find(|s| owns(s, &facts.id));
            commits.push(LedgerCommit {
                id: facts.id.clone(),
                subject: entry.subject,
                author: entry.author,
                time: rfc3339(entry.time),
                session_id: owner.map(|s| s.session_id.clone()),
                marked: owner.is_none() && !entry.session_markers.is_empty(),
                comments,
            });
        }

        let lane = if input.lane.is_empty() {
            branch
        } else {
            input.lane.clone()
        };
        let overrides_out = overrides
            .iter()
            .filter(|o| o.lane.as_deref() == Some(lane.as_str()))
            .cloned()
            .collect();
        // Overrides are never cut by time: that would be an inference.
        let unscoped_overrides = overrides.into_iter().filter(|o| o.lane.is_none()).collect();
        let sessions_out = sessions
            .into_iter()
            .filter(|s| files.iter().any(|f| owns(s, &f.id)))
            .collect();

        Ok(LedgerOutput {
            workspace: input.workspace,
            range,
            commits,
            unlinked,
            overrides: overrides_out,
            unscoped_overrides,
            sessions: sessions_out,
        })
    }
}

/// The range as resolved. An explicit range is taken as given. Otherwise HEAD
/// against the merge base with the default branch. The default branch, or no
/// default branch at all, has no base, so it needs a since bound (then HEAD).
fn resolve_range(dir: &Path, range: &str, branch: &str, has_since: bool) -> Result<String> {
    if range.starts_with('-') {
        return Err(Error::invalid(format!("invalid revision '{range}'")));
    }
    if !range.is_empty() {
        return Ok(range.to_owned());
    }
    let default = gitx::default_branch(dir);
    let on_default = default
        .as_deref()
        .is_some_and(|d| d == branch || d.strip_prefix("origin/") == Some(branch));
    match default {
        Some(d) if !on_default => {
            let base = gitx::merge_base(dir, &d, "HEAD").map_err(|e| git_error(&e))?;
            Ok(format!("{base}..HEAD"))
        }
        _ if has_since => Ok("HEAD".to_owned()),
        Some(_) => Err(Error::invalid(
            "the ledger of the default branch needs --range or --since",
        )),
        None => Err(Error::invalid(
            "the repository has no default branch to compare HEAD with; pass --range or --since",
        )),
    }
}

/// The commits of `range`, oldest first, paged until the log is exhausted.
fn read_range(dir: &Path, range: &str) -> Result<Vec<LogEntry>> {
    let stop = AtomicBool::new(false);
    let mut entries: Vec<LogEntry> = Vec::new();
    loop {
        let query = LogQuery {
            revs: vec![range.to_owned()],
            all: false,
            skip: entries.len(),
            limit: PAGE,
        };
        let page = history::log(dir, &query, &stop).map_err(|e| git_error(&e))?;
        let full = page.len() == PAGE;
        entries.extend(page);
        if entries.len() > MAX_COMMITS {
            return Err(Error::invalid(format!(
                "the range has more than {MAX_COMMITS} commits; name a smaller --range"
            )));
        }
        if !full {
            break;
        }
    }
    entries.reverse();
    Ok(entries)
}

#[cfg(test)]
thread_local! {
    /// The git reads the ledger made, in order, for the tests that count them.
    static GIT_READS: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// What one commit did to each watched file it changed: the blob it left. The
/// patch-id of the change is read later, and only where matching needs it.
/// With no watched file, git is not read at all: the commit is its id alone.
fn commit_files(dir: &Path, id: &str, watched: &HashSet<&str>) -> Result<CommitFiles> {
    if watched.is_empty() {
        return Ok(CommitFiles {
            id: id.to_owned(),
            files: Vec::new(),
        });
    }
    #[cfg(test)]
    GIT_READS.with(|r| r.borrow_mut().push(format!("show {id}")));
    let detail = history::show(dir, id).map_err(|e| git_error(&e))?;
    let mut files = Vec::new();
    for change in detail
        .files
        .iter()
        .filter(|change| watched.contains(change.path.as_str()))
    {
        #[cfg(test)]
        GIT_READS.with(|r| r.borrow_mut().push(format!("blob {id} {}", change.path)));
        let blob_after = patch_id::blob_at(dir, id, &change.path).map_err(|e| git_error(&e))?;
        files.push(FileFact {
            path: change.path.clone(),
            blob_after,
            patch_id: None,
        });
    }
    Ok(CommitFiles {
        id: id.to_owned(),
        files,
    })
}

/// Links the done comments in lane scope to the commits of the range. Each
/// comment lands in its oldest linking commit only, so a fix copied by a
/// rebase is shown once. A comment that links nowhere is unlinked when a
/// commit of the range touches its file; a comment on an untouched file is
/// left out, which is a content scope, not a time scope.
///
/// The patch-id of a commit's change to a file is read only for a commit
/// before the comment's exact link, because only such a commit can link it
/// first by patch-id. Reads are cached per (commit, file), so a commit is
/// read once however many comments name its file.
fn match_comments(
    dir: &Path,
    comments: Vec<Comment>,
    files: &mut [CommitFiles],
) -> Result<(Vec<Vec<LinkedComment>>, Vec<UnlinkedComment>)> {
    let index: HashMap<String, usize> = files
        .iter()
        .enumerate()
        .map(|(i, f)| (f.id.clone(), i))
        .collect();
    let mut patched: HashSet<(usize, String)> = HashSet::new();
    let mut linked: Vec<Vec<LinkedComment>> = vec![Vec::new(); files.len()];
    let mut unlinked = Vec::new();
    for c in comments {
        let touching: Vec<usize> = (0..files.len())
            .filter(|&i| touches(&files[i], &c.file))
            .collect();
        if touching.is_empty() {
            // Not in the range's content scope.
            continue;
        }
        let exact_at = touching.iter().copied().find(|&i| exact(&c, &files[i]));
        let before = exact_at.unwrap_or(usize::MAX);
        let patch = if touching.iter().any(|&i| i < before) {
            resolution_patch_id(dir, &c)
        } else {
            None
        };
        if patch.is_some() {
            for &i in touching.iter().take_while(|&&i| i < before) {
                fill_patch_id(dir, files, &mut patched, i, &c.file)?;
            }
        }
        // Every commit before the exact one now has its patch-id, and the
        // exact one needs none, so the first link is the oldest one.
        let oldest = link_all(&c, patch.as_deref(), files)
            .first()
            .map(|&(commit, kind)| (index[&commit.id], kind));
        if let Some((i, link)) = oldest {
            let (diff, _) = snapshot_diff(&c);
            linked[i].push(LinkedComment {
                comment: c,
                link,
                diff,
            });
        } else {
            let reason = unlinked_reason(&c, patch.as_deref(), files);
            unlinked.push(UnlinkedComment { comment: c, reason });
        }
    }
    Ok((linked, unlinked))
}

/// Reads the patch-id of commit `i`'s change to `path` into its fact, once.
fn fill_patch_id(
    dir: &Path,
    files: &mut [CommitFiles],
    patched: &mut HashSet<(usize, String)>,
    i: usize,
    path: &str,
) -> Result<()> {
    if !patched.insert((i, path.to_owned())) {
        return Ok(());
    }
    #[cfg(test)]
    GIT_READS.with(|r| r.borrow_mut().push(format!("patch {} {path}", files[i].id)));
    let id = files[i].id.clone();
    let read = patch_id::commit_file_patch_id(dir, &id, path).map_err(|e| git_error(&e))?;
    if let Some(fact) = files[i].files.iter_mut().find(|f| f.path == path) {
        fact.patch_id = read;
    }
    Ok(())
}

/// The patch-id of the change from the reviewed blob to the resolved one.
/// A snapshot git can no longer read cannot match by patch-id, so the comment
/// stays unlinked instead of failing the whole ledger.
fn resolution_patch_id(dir: &Path, c: &Comment) -> Option<String> {
    if c.file_version.is_empty() || c.resolved_file_version.is_empty() {
        return None;
    }
    #[cfg(test)]
    GIT_READS.with(|r| r.borrow_mut().push(format!("resolution {}", c.file)));
    patch_id::blobs_patch_id(dir, &c.file_version, &c.resolved_file_version, &c.file)
        .ok()
        .flatten()
}

fn in_lane(c: &Comment, lane: &str) -> bool {
    lane.is_empty() || c.lane.as_deref() == Some(lane)
}

fn touches(files: &CommitFiles, path: &str) -> bool {
    files.files.iter().any(|fact| fact.path == path)
}

/// Whether the reported session names one of the commit's ids.
fn owns(session: &Session, commit: &str) -> bool {
    session
        .commits
        .iter()
        .any(|reported| same_commit(reported, commit))
}

/// The seconds a since or until names. RFC 3339 names an instant. YYYY-MM-DD
/// names a UTC day: `since` starts at its first second, `until` ends at its
/// last. Empty is no bound.
fn bound(text: &str, is_until: bool) -> Result<Option<i64>> {
    if text.is_empty() {
        return Ok(None);
    }
    if let Some(day) = parse_day(text) {
        let start = PrimitiveDateTime::new(day, Time::MIDNIGHT)
            .assume_utc()
            .unix_timestamp();
        return Ok(Some(if is_until { start + DAY - 1 } else { start }));
    }
    parse_rfc3339(text).map(Some).ok_or_else(|| {
        Error::invalid(format!(
            "'{text}' is neither an RFC 3339 time nor a YYYY-MM-DD date"
        ))
    })
}

/// The date of `YYYY-MM-DD`, exactly that shape.
fn parse_day(text: &str) -> Option<Date> {
    let [y1, y2, y3, y4, b'-', m1, m2, b'-', d1, d2] = text.as_bytes() else {
        return None;
    };
    let year = i32::try_from(number(&[*y1, *y2, *y3, *y4])?).ok()?;
    let month = Month::try_from(u8::try_from(number(&[*m1, *m2])?).ok()?).ok()?;
    let day = u8::try_from(number(&[*d1, *d2])?).ok()?;
    Date::from_calendar_date(year, month, day).ok()
}

/// The instant of an RFC 3339 time with an explicit offset, in seconds since
/// the epoch. The `time` crate's parser is not enabled for this crate, so the
/// grammar is read here: `YYYY-MM-DDTHH:MM:SS[.fraction](Z|+HH:MM|-HH:MM)`.
fn parse_rfc3339(text: &str) -> Option<i64> {
    let (day, rest) = text.split_at_checked(10)?;
    let day = parse_day(day)?;
    let rest = rest.strip_prefix(['T', 't'])?;
    let (clock, rest) = rest.split_at_checked(8)?;
    let [h1, h2, b':', m1, m2, b':', s1, s2] = clock.as_bytes() else {
        return None;
    };
    let time = Time::from_hms(
        u8::try_from(number(&[*h1, *h2])?).ok()?,
        u8::try_from(number(&[*m1, *m2])?).ok()?,
        u8::try_from(number(&[*s1, *s2])?).ok()?,
    )
    .ok()?;
    let rest = match rest.strip_prefix('.') {
        Some(fraction) => {
            let digits = fraction.bytes().take_while(u8::is_ascii_digit).count();
            if digits == 0 {
                return None;
            }
            &fraction[digits..]
        }
        None => rest,
    };
    let offset = match rest {
        "Z" | "z" => 0,
        _ => {
            let [sign, h1, h2, b':', m1, m2] = rest.as_bytes() else {
                return None;
            };
            let sign = match *sign {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            sign * (number(&[*h1, *h2])? * 3600 + number(&[*m1, *m2])? * 60)
        }
    };
    let instant = PrimitiveDateTime::new(day, time)
        .assume_utc()
        .unix_timestamp();
    Some(instant - offset)
}

/// The value of ASCII digits, every byte of them.
fn number(digits: &[u8]) -> Option<i64> {
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    digits
        .iter()
        .try_fold(0i64, |n, b| Some(n * 10 + i64::from(b - b'0')))
}

/// A commit time in RFC 3339, the way the store writes its own.
fn rfc3339(seconds: i64) -> String {
    format_time(OffsetDateTime::from_unix_timestamp(seconds).unwrap_or(OffsetDateTime::UNIX_EPOCH))
}

fn git_error(e: &GitError) -> Error {
    Error::internal(e.to_string())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use super::*;
    use crate::operations::{
        AddInput, LedgerCommit, LinkKind, OverrideInput, PullInput, ReportSessionInput,
        ResolveInput,
    };
    use crate::service::testing::{MemRepo, fixed_clock};
    use crate::types::{Actor, Comment, OUTCOME_DONE};

    struct Fixture {
        svc: Service,
        ws: String,
        root: PathBuf,
        _dir: tempfile::TempDir,
    }

    /// Runs git in `dir` with `env` and returns its stdout; panics on failure.
    fn git_env(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .envs(env.iter().copied())
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).expect("utf-8 output")
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        git_env(dir, args, &[])
    }

    /// A repository on main with one commit, app.py tracked, and the store
    /// in memory. The commit is "base".
    fn setup() -> Fixture {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("proj");
        fs::create_dir_all(&root).expect("mkdir");
        git(&root, &["init", "-q", "-b", "main"]);
        git(&root, &["config", "user.name", "Test"]);
        git(&root, &["config", "user.email", "test@example.com"]);
        git(&root, &["config", "commit.gpgsign", "false"]);
        fs::write(root.join("app.py"), "one\ntwo\nthree\nfour\n").expect("write");
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-q", "-m", "base"]);
        let ws = rvw_git::workspace::resolve(&root)
            .expect("resolves")
            .to_string_lossy()
            .into_owned();
        Fixture {
            svc: Service::with_clock(Box::new(MemRepo::default()), fixed_clock()),
            ws,
            root,
            _dir: dir,
        }
    }

    impl Fixture {
        /// Replaces the whole of app.py.
        fn write(&self, text: &str) {
            fs::write(self.root.join("app.py"), text).expect("write");
        }

        /// Stages everything and commits it with `msg`.
        fn commit(&self, msg: &str) {
            git(&self.root, &["add", "-A"]);
            git(&self.root, &["commit", "-q", "-m", msg]);
        }

        /// Commits everything with both dates set to `date`.
        fn commit_dated(&self, msg: &str, date: &str) {
            git(&self.root, &["add", "-A"]);
            git_env(
                &self.root,
                &["commit", "-q", "-m", msg],
                &[("GIT_AUTHOR_DATE", date), ("GIT_COMMITTER_DATE", date)],
            );
        }

        fn rev(&self, rev: &str) -> String {
            git(&self.root, &["rev-parse", rev]).trim().to_owned()
        }

        /// A review comment on line `line` of app.py.
        fn add(&self, at: i64, text: &str, lane: &str) -> Comment {
            self.svc
                .add(AddInput {
                    workspace: self.ws.clone(),
                    file: "app.py".into(),
                    start_line: at,
                    comment: text.into(),
                    lane: lane.into(),
                    ..Default::default()
                })
                .expect("add")
        }

        /// Pulls the queue, as an agent does before it works, then marks the
        /// comment done, snapshotting app.py as it is now.
        fn resolve(&self, id: &str) {
            self.svc
                .pull(PullInput {
                    workspace: self.ws.clone(),
                    ..Default::default()
                })
                .expect("pull");
            self.svc
                .resolve(ResolveInput {
                    workspace: self.ws.clone(),
                    id: id.into(),
                    outcome: OUTCOME_DONE.into(),
                    note: "done".into(),
                    author: "agent".into(),
                })
                .expect("resolve");
        }

        /// The ledger of the range, or of the default branch to HEAD when empty.
        fn ledger(&self, range: &str) -> LedgerOutput {
            self.svc
                .ledger(LedgerInput {
                    workspace: self.ws.clone(),
                    range: range.into(),
                    ..Default::default()
                })
                .expect("ledger")
        }

        /// The ledger of the default branch to HEAD, narrowed by `since`.
        fn ledger_since(&self, since: &str) -> LedgerOutput {
            self.svc
                .ledger(LedgerInput {
                    workspace: self.ws.clone(),
                    since: since.into(),
                    ..Default::default()
                })
                .expect("ledger")
        }

        fn subjects(out: &LedgerOutput) -> Vec<String> {
            out.commits.iter().map(|c| c.subject.clone()).collect()
        }
    }

    /// The commit with `subject` in the ledger.
    fn commit_named<'a>(out: &'a LedgerOutput, subject: &str) -> &'a LedgerCommit {
        out.commits
            .iter()
            .find(|c| c.subject == subject)
            .unwrap_or_else(|| panic!("no commit {subject} in {:?}", Fixture::subjects(out)))
    }

    #[test]
    fn a_resolved_comment_links_exactly_to_its_commit() {
        let f = setup();
        let base = f.rev("main");
        git(&f.root, &["checkout", "-q", "-b", "feature"]);
        let c = f.add(2, "say two", "");
        f.write("one\nTWO\nthree\nfour\n");
        f.resolve(&c.id);
        f.commit("fix two");

        let out = f.ledger("");
        assert_eq!(out.range, format!("{base}..HEAD"));
        assert_eq!(Fixture::subjects(&out), ["fix two"]);
        let linked = &out.commits[0].comments;
        assert_eq!(linked.len(), 1);
        assert_eq!(linked[0].comment.id, c.id);
        assert_eq!(linked[0].link, LinkKind::Exact);
        assert!(!linked[0].diff.is_empty(), "the resolution diff is shown");
        assert!(out.unlinked.is_empty());
    }

    #[test]
    fn a_rebased_copy_of_the_fix_still_links_by_content() {
        let f = setup();
        let base = f.rev("main");
        git(&f.root, &["checkout", "-q", "-b", "feature"]);
        let c = f.add(2, "say two", "");
        f.write("one\nTWO\nthree\nfour\n");
        f.resolve(&c.id);
        f.commit("fix two");
        let original = f.rev("HEAD");

        // the same fix lands on another base, after an unrelated commit
        git(&f.root, &["checkout", "-q", "-b", "rebased", &base]);
        fs::write(f.root.join("other.txt"), "unrelated\n").expect("write");
        f.commit("unrelated");
        git(&f.root, &["cherry-pick", &original]);

        let out = f.ledger("");
        assert_eq!(Fixture::subjects(&out), ["unrelated", "fix two"]);
        let copy = commit_named(&out, "fix two");
        assert_ne!(copy.id, original, "the copy has another commit id");
        assert_eq!(copy.comments.len(), 1);
        assert_eq!(copy.comments[0].link, LinkKind::Exact);
        assert_eq!(copy.comments[0].comment.id, c.id);
        assert!(commit_named(&out, "unrelated").comments.is_empty());
    }

    #[test]
    fn a_fix_rebased_onto_a_changed_line_links_by_patch_id() {
        let f = setup();
        let lines: String = (1..=12)
            .map(|n| format!("line {n}\n"))
            .collect::<Vec<_>>()
            .concat();
        f.write(&lines);
        f.commit("longer");
        let base = f.rev("main");
        git(&f.root, &["checkout", "-q", "-b", "feature"]);
        let c = f.add(2, "say two", "");
        f.write(&lines.replace("line 2\n", "LINE 2\n"));
        f.resolve(&c.id);
        f.commit("fix two");
        let original = f.rev("HEAD");

        // the base changed far from the fix, so the copy's blob differs
        git(&f.root, &["checkout", "-q", "-b", "rebased", &base]);
        f.write(&lines.replace("line 11\n", "LINE 11\n"));
        f.commit("edit eleven");
        git(&f.root, &["cherry-pick", &original]);

        let out = f.ledger("");
        let copy = commit_named(&out, "fix two");
        assert_eq!(copy.comments.len(), 1);
        assert_eq!(copy.comments[0].link, LinkKind::SameChange);
        assert!(out.unlinked.is_empty());
    }

    #[test]
    fn a_squashed_fix_is_unlinked_with_no_content_match() {
        let f = setup();
        let base = f.rev("main");
        git(&f.root, &["checkout", "-q", "-b", "feature"]);
        let c = f.add(2, "say two", "");
        f.write("one\nTWO\nthree\nfour\n");
        f.resolve(&c.id);
        f.commit("fix two");
        f.write("one\nTWO\nthree\nFOUR\n");
        f.commit("tidy four");
        git(&f.root, &["reset", "-q", "--soft", &base]);
        f.commit("squash");

        let out = f.ledger("");
        assert_eq!(Fixture::subjects(&out), ["squash"]);
        assert!(out.commits[0].comments.is_empty());
        assert_eq!(out.unlinked.len(), 1);
        assert_eq!(out.unlinked[0].comment.id, c.id);
        assert_eq!(out.unlinked[0].reason, "no content match");
    }

    #[test]
    fn the_default_branch_needs_a_range_or_a_since() {
        let f = setup();
        let refused = f
            .svc
            .ledger(LedgerInput {
                workspace: f.ws.clone(),
                ..Default::default()
            })
            .expect_err("refused");
        assert_eq!(refused.kind, crate::error::ErrorKind::Invalid);
        assert!(
            refused
                .message
                .contains("the ledger of the default branch needs --range or --since"),
            "{}",
            refused.message
        );

        let since = f.ledger_since("2000-01-01");
        assert_eq!(since.range, "HEAD");
        assert_eq!(Fixture::subjects(&since), ["base"]);
    }

    #[test]
    fn a_range_starting_with_a_dash_is_refused() {
        let f = setup();
        let err = f
            .svc
            .ledger(LedgerInput {
                workspace: f.ws.clone(),
                range: "--output=/tmp/x".into(),
                ..Default::default()
            })
            .expect_err("refused");
        assert_eq!(err.kind, crate::error::ErrorKind::Invalid);
    }

    #[test]
    fn since_and_until_filter_commits_by_author_date() {
        let f = setup();
        git(&f.root, &["checkout", "-q", "-b", "feature"]);
        fs::write(f.root.join("app.py"), "v1\n").expect("write");
        f.commit_dated("early", "2026-10-01T10:00:00+00:00");
        fs::write(f.root.join("app.py"), "v2\n").expect("write");
        f.commit_dated("middle", "2026-10-05T12:00:00+00:00");
        fs::write(f.root.join("app.py"), "v3\n").expect("write");
        f.commit_dated("late", "2026-10-10T08:00:00+00:00");

        let subjects = |since: &str, until: &str| {
            let out = f
                .svc
                .ledger(LedgerInput {
                    workspace: f.ws.clone(),
                    since: since.into(),
                    until: until.into(),
                    ..Default::default()
                })
                .expect("ledger");
            Fixture::subjects(&out)
        };
        assert_eq!(subjects("", ""), ["early", "middle", "late"]);
        assert_eq!(subjects("2026-10-05", ""), ["middle", "late"]);
        assert_eq!(
            subjects("", "2026-10-05"),
            ["early", "middle"],
            "a date is a whole day"
        );
        assert_eq!(subjects("2026-10-05T13:00:00Z", ""), ["late"]);
        assert_eq!(
            subjects("2026-10-05T14:00:00+02:00", ""),
            ["middle", "late"]
        );
        assert_eq!(subjects("", "2026-10-05T11:59:59Z"), ["early"]);

        let err = f
            .svc
            .ledger(LedgerInput {
                workspace: f.ws.clone(),
                since: "yesterday".into(),
                ..Default::default()
            })
            .expect_err("refused");
        assert_eq!(err.kind, crate::error::ErrorKind::Invalid);
    }

    #[test]
    fn overrides_split_by_lane_and_are_never_cut_by_time() {
        let f = setup();
        git(&f.root, &["checkout", "-q", "-b", "feature"]);
        // every override needs something open in its lane
        f.add(1, "in feature", "feature");
        f.add(2, "in payments", "payments");
        f.add(3, "unlaned", "");
        let override_gate = |lane: &str, reason: &str| {
            f.svc
                .override_gate(
                    OverrideInput {
                        workspace: f.ws.clone(),
                        action: "commit".into(),
                        reason: reason.into(),
                        lane: lane.into(),
                        author: "gustavo".into(),
                    },
                    Actor::Human,
                )
                .expect("override")
        };
        override_gate("feature", "ship feature");
        override_gate("", "ship everything");
        override_gate("payments", "ship payments");

        let mine = f.ledger("");
        let reasons = |os: &[crate::types::GateOverride]| {
            os.iter().map(|o| o.reason.clone()).collect::<Vec<_>>()
        };
        assert_eq!(reasons(&mine.overrides), ["ship feature"]);
        assert_eq!(reasons(&mine.unscoped_overrides), ["ship everything"]);

        let pinned = f
            .svc
            .ledger(LedgerInput {
                workspace: f.ws.clone(),
                lane: "payments".into(),
                ..Default::default()
            })
            .expect("ledger");
        assert_eq!(reasons(&pinned.overrides), ["ship payments"]);
        assert_eq!(reasons(&pinned.unscoped_overrides), ["ship everything"]);
    }

    #[test]
    fn an_empty_range_gives_empty_arrays() {
        let f = setup();
        git(&f.root, &["checkout", "-q", "-b", "feature"]);
        let base = f.rev("main");

        let out = f.ledger("");
        assert_eq!(out.range, format!("{base}..HEAD"));
        assert!(out.commits.is_empty());
        assert!(out.unlinked.is_empty());
        assert!(out.overrides.is_empty());
        assert!(out.unscoped_overrides.is_empty());
        assert!(out.sessions.is_empty());

        let explicit = f.ledger("HEAD..HEAD");
        assert!(explicit.commits.is_empty());
    }

    #[test]
    fn sessions_own_their_commits_and_a_marker_without_a_report_says_so() {
        let f = setup();
        git(&f.root, &["checkout", "-q", "-b", "feature"]);
        f.write("one\nTWO\nthree\nfour\n");
        f.commit("fix two");
        let fix = f.rev("HEAD");
        f.svc
            .report_session(ReportSessionInput {
                workspace: f.ws.clone(),
                session_id: "s1".into(),
                tool: "claude-code".into(),
                model: "model-x".into(),
                summary: String::new(),
                // an abbreviated id, as an agent reports it
                commits: vec![fix[..12].to_owned()],
            })
            .expect("report");
        f.write("one\nTWO\nthree\nFOUR\n");
        f.commit("tidy four\n\nClaude-Session: s9");

        let out = f.ledger("");
        let fixed = commit_named(&out, "fix two");
        assert_eq!(fixed.session_id.as_deref(), Some("s1"));
        assert!(!fixed.marked);
        let tidy = commit_named(&out, "tidy four");
        assert_eq!(tidy.session_id, None);
        assert!(tidy.marked, "a marker with no report is marked");
        let sessions: Vec<&str> = out.sessions.iter().map(|s| s.session_id.as_str()).collect();
        assert_eq!(sessions, ["s1"]);
    }

    /// The git reads the last ledger made, clearing the log first.
    fn reads_of(f: impl FnOnce()) -> Vec<String> {
        GIT_READS.with(|r| r.borrow_mut().clear());
        f();
        GIT_READS.with(|r| r.borrow().clone())
    }

    #[test]
    fn a_comment_on_one_file_reads_no_other_file() {
        let f = setup();
        git(&f.root, &["checkout", "-q", "-b", "feature"]);
        fs::write(f.root.join("other.txt"), "x\n").expect("write");
        f.commit("touch other");
        let c = f.add(2, "say two", "");
        f.write("one\nTWO\nthree\nfour\n");
        f.resolve(&c.id);
        f.commit("fix two");
        fs::write(f.root.join("other.txt"), "y\n").expect("write");
        f.commit("touch other again");

        let mut out = None;
        let reads = reads_of(|| out = Some(f.ledger("")));
        let out = out.expect("ledger ran");
        assert_eq!(
            Fixture::subjects(&out),
            ["touch other", "fix two", "touch other again"]
        );
        assert_eq!(
            commit_named(&out, "fix two").comments[0].link,
            LinkKind::Exact
        );
        assert!(!reads.is_empty(), "the ledger reads git");
        assert!(
            reads.iter().all(|read| !read.contains("other.txt")),
            "no read names the other file: {reads:?}"
        );
        // The exact link is the first commit touching app.py, so nothing is
        // compared by patch-id.
        assert!(
            reads
                .iter()
                .all(|read| !read.starts_with("patch") && !read.starts_with("resolution")),
            "{reads:?}"
        );
    }

    #[test]
    fn no_done_comment_in_scope_reads_no_commit_detail() {
        let f = setup();
        git(&f.root, &["checkout", "-q", "-b", "feature"]);
        f.write("one\nTWO\nthree\nfour\n");
        f.commit("fix two");
        f.add(1, "pending only", "");

        let mut out = None;
        let reads = reads_of(|| out = Some(f.ledger("")));
        let out = out.expect("ledger ran");
        assert_eq!(Fixture::subjects(&out), ["fix two"]);
        assert!(out.commits[0].comments.is_empty());
        assert!(
            reads.is_empty(),
            "no done comment, no commit detail: {reads:?}"
        );
    }
}
