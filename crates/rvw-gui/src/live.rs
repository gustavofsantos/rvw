//! The live working tree: changes made outside the window, by an agent or a
//! terminal, show within about a second, and the reviewer keeps their place.
//! Once a second a background task reads a `Fingerprint` of the repository
//! and the database. A change of it starts a `LiveJob`, which re-reads what
//! the window shows off the UI thread; `Shell::live_apply` shows it.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui::Context;
use rvw_core::Service;
use rvw_core::types::Comment;
use rvw_view::changes::Compare;
use rvw_view::commit_screen::Side;
use rvw_view::viewmode::ViewMode;

use crate::changes_state::{ChangesRead, read_changes};
use crate::data::{self, Snapshot};
use crate::file_view::load_selected;
use crate::git_tools::RepoFacts;
use crate::review_screen::Selected;
use crate::shell::Shell;

/// How often the window reads the fingerprint.
const INTERVAL: Duration = Duration::from_secs(1);

/// The modification time and length of a file; `None` when it is missing.
type Stamp = Option<(SystemTime, u64)>;

/// What the window last showed, as two parts that compare cheaply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fingerprint {
    /// A hash of git's status, the refs, and the size and time of every
    /// listed file. `None` when git could not run; see `keep_git`.
    pub git: Option<u64>,
    /// The database file, then its write-ahead log.
    pub db: [Stamp; 2],
}

impl Fingerprint {
    /// Reads the repository at `dir` and the database at `db`. Git is run
    /// without taking the index lock, so reading never blocks an agent.
    pub fn read(dir: &Path, db: &Path) -> Self {
        Self {
            git: git_hash(dir),
            db: [stamp(db), stamp(&wal_path(db))],
        }
    }

    /// The same fingerprint with the git part of `previous` when git failed
    /// to run, so a failure reads as no change.
    pub fn keep_git(mut self, previous: Option<&Self>) -> Self {
        if self.git.is_none() {
            self.git = previous.and_then(|last| last.git);
        }
        self
    }
}

/// Polls the fingerprint once a second for the life of the window. Each read
/// finishes before the next timer starts, so ticks never overlap. The paths
/// are read from the shell on every tick. The loop ends when the shell is gone.
///
/// Git and the database are read on the background executor, both for the
/// fingerprint and, when it changed, for what the window shows; the UI thread
/// only compares and applies. See `LiveJob`.
pub fn start(cx: &mut Context<Shell>) {
    cx.spawn(async move |this, cx| {
        loop {
            cx.background_executor().timer(INTERVAL).await;
            let Ok((dir, db)) = this.read_with(cx, |shell, _| {
                (
                    PathBuf::from(&shell.identity.workspace),
                    shell.db_path.clone(),
                )
            }) else {
                break;
            };
            let fingerprint = cx
                .background_executor()
                .spawn(async move { Fingerprint::read(&dir, &db) })
                .await;
            let Ok(job) = this.update(cx, |this, _| this.live_job(fingerprint)) else {
                break;
            };
            let Some(job) = job else {
                continue;
            };
            let read = cx
                .background_executor()
                .spawn(async move { job.read() })
                .await;
            if this
                .update(cx, |this, cx| this.live_apply(read, cx))
                .is_err()
            {
                break;
            }
        }
    })
    .detach();
}

/// What the window showed when a live read started. The read is applied only
/// when the window still shows the same, so a click or an action taken while
/// git ran is never undone by an older read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveKey {
    /// `Shell::live_epoch`: bumped by every re-read made on the UI thread.
    pub epoch: u64,
    /// The review screen's open file and its view.
    pub selected: Option<(String, ViewMode)>,
    /// The Changes screen's open file.
    pub open: Option<(Side, String)>,
    pub compare: Option<Compare>,
}

/// One live read: what to read, captured on the UI thread, and read on the
/// background executor by `read`.
pub struct LiveJob {
    fingerprint: Fingerprint,
    key: LiveKey,
    /// Git changed, so the Changes screen and the repository facts are read
    /// too.
    git_changed: bool,
    service: Arc<Service>,
    workspace: String,
    lane: String,
    /// The comments shown now, which the Changes lists count when the queue
    /// cannot be read.
    comments: Vec<Comment>,
    /// `Shell::open_change_place`.
    open_place: Option<(Side, String, usize)>,
}

/// What a `LiveJob` read.
pub struct LiveRead {
    fingerprint: Fingerprint,
    key: LiveKey,
    snapshot: Result<Snapshot, String>,
    /// The review screen's open file, read against `snapshot`.
    selected: Option<Selected>,
    /// Read only when git changed.
    changes: Option<ChangesRead>,
    facts: Option<RepoFacts>,
}

impl LiveJob {
    /// Reads the queue, the open file and, when git changed, the Changes
    /// screen and the repository facts. Runs off the UI thread.
    fn read(self) -> LiveRead {
        let snapshot = data::load(&self.service, &self.workspace, &self.lane, self.key.compare);
        let selected = match (&snapshot, &self.key.selected) {
            (Ok(snapshot), Some((path, mode))) => Some(load_selected(snapshot, path, *mode)),
            _ => None,
        };
        let (changes, facts) = if self.git_changed {
            let dir = Path::new(&self.workspace);
            let comments = snapshot
                .as_ref()
                .map_or(&self.comments[..], |snapshot| &snapshot.comments[..]);
            (
                Some(read_changes(dir, comments, self.open_place.as_ref())),
                Some(RepoFacts::read(dir)),
            )
        } else {
            (None, None)
        };
        LiveRead {
            fingerprint: self.fingerprint,
            key: self.key,
            snapshot,
            selected,
            changes,
            facts,
        }
    }
}

impl Shell {
    /// What the window shows now, to tell a stale live read; see `LiveKey`.
    pub(crate) fn live_key(&self) -> LiveKey {
        LiveKey {
            epoch: self.live_epoch,
            selected: self
                .selected
                .as_ref()
                .map(|selected| (selected.path.clone(), selected.mode)),
            open: self
                .changes
                .open
                .as_ref()
                .map(|open| (open.side, open.path.clone())),
            compare: self.tools.compare,
        }
    }

    /// Handles one fingerprint. The first only records it. A changed one
    /// starts a read of what the window shows.
    pub(crate) fn live_job(&mut self, fingerprint: Fingerprint) -> Option<LiveJob> {
        let fingerprint = fingerprint.keep_git(self.live_last.as_ref());
        let Some(last) = self.live_last else {
            self.live_last = Some(fingerprint);
            return None;
        };
        if last == fingerprint {
            return None;
        }
        Some(LiveJob {
            fingerprint,
            key: self.live_key(),
            git_changed: last.git != fingerprint.git,
            service: Arc::clone(&self.service),
            workspace: self.identity.workspace.clone(),
            lane: self.identity.lane.clone(),
            comments: self.snapshot.comments.clone(),
            open_place: self.open_change_place(),
        })
    }

    /// Shows a live read, keeping the reviewer's place. The fingerprint is
    /// kept only once the window has caught up, so a stale or failed read, or
    /// an open file that waits for its composer to close, is tried again on
    /// the next tick. A failed read changes nothing on screen.
    pub(crate) fn live_apply(&mut self, read: LiveRead, cx: &mut Context<Self>) {
        if read.key != self.live_key() {
            return;
        }
        let Ok(snapshot) = read.snapshot else {
            return;
        };
        let settled = self.apply_snapshot(snapshot, read.selected);
        if let Some(changes) = read.changes {
            self.apply_changes(changes);
        }
        if let Some(facts) = read.facts {
            self.git.facts = facts;
        }
        cx.notify();
        if settled {
            self.live_last = Some(read.fingerprint);
        }
    }
}

/// The fingerprint of the repository at `dir`, or `None` when git fails.
fn git_hash(dir: &Path) -> Option<u64> {
    let status = git(
        dir,
        &[
            "status",
            "--porcelain=v2",
            "-z",
            "--branch",
            "--untracked-files=all",
        ],
    )?;
    let refs = git(
        dir,
        &["for-each-ref", "--format=%(refname)%00%(objectname)"],
    )?;
    let mut hasher = DefaultHasher::new();
    status.hash(&mut hasher);
    refs.hash(&mut hasher);
    // A file edited again while already listed keeps its status line, so its
    // size and time are part of the hash too.
    for path in status_paths(&status) {
        path.hash(&mut hasher);
        stamp(&dir.join(&path)).hash(&mut hasher);
    }
    Some(hasher.finish())
}

/// The stdout of `git -C dir args`, or `None` when git fails to run or exits
/// with an error.
fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The paths `git status --porcelain=v2 -z` lists, in record order. A rename
/// or copy is followed by its original path, which is not listed.
pub fn status_paths(out: &str) -> Vec<String> {
    let mut paths = Vec::new();
    let mut records = out.split('\0');
    while let Some(record) = records.next() {
        let Some((kind, _)) = record.split_once(' ') else {
            continue;
        };
        let (path, has_origin) = match kind {
            // 1 XY sub mH mI mW hH hI path
            "1" => (field(record, 9), false),
            // 2 XY sub mH mI mW hH hI Xscore path, then the origin record
            "2" => (field(record, 10), true),
            // u XY sub m1 m2 m3 mW h1 h2 h3 path
            "u" => (field(record, 11), false),
            "?" | "!" => (record.get(2..), false),
            _ => (None, false),
        };
        paths.extend(path.filter(|path| !path.is_empty()).map(str::to_owned));
        if has_origin {
            records.next();
        }
    }
    paths
}

/// The text after the first `count - 1` spaces of a record. Paths may hold
/// spaces, so the last field is the rest of the line.
fn field(record: &str, count: usize) -> Option<&str> {
    record.splitn(count, ' ').nth(count - 1)
}

/// The size and time of `path`, without following a symbolic link.
fn stamp(path: &Path) -> Stamp {
    let meta = std::fs::symlink_metadata(path).ok()?;
    Some((meta.modified().unwrap_or(UNIX_EPOCH), meta.len()))
}

/// The write-ahead log that sits beside the database.
fn wal_path(db: &Path) -> PathBuf {
    let mut name = db.as_os_str().to_owned();
    name.push("-wal");
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn status_paths_reads_ordinary_records() {
        let out = "# branch.oid abc\0# branch.head main\0\
                   1 .M N... 100644 100644 100644 abc def src/main.rs\0";
        assert_eq!(status_paths(out), ["src/main.rs"]);
    }

    #[test]
    fn status_paths_keeps_spaces_in_a_path() {
        let out = "1 .M N... 100644 100644 100644 abc def docs/my notes.md\0";
        assert_eq!(status_paths(out), ["docs/my notes.md"]);
    }

    #[test]
    fn status_paths_skips_the_origin_of_a_rename() {
        let out = "2 R. N... 100644 100644 100644 abc def R100 new name.rs\0\
                   old name.rs\0? untracked.txt\0";
        assert_eq!(status_paths(out), ["new name.rs", "untracked.txt"]);
    }

    #[test]
    fn status_paths_reads_unmerged_untracked_and_ignored_records() {
        let out = "u UU N... 100644 100644 100644 100644 a b c conflict.rs\0\
                   ? new file.txt\0! target/\0";
        assert_eq!(
            status_paths(out),
            ["conflict.rs", "new file.txt", "target/"]
        );
    }

    #[test]
    fn status_paths_is_empty_for_a_clean_tree() {
        let out = "# branch.oid abc\0# branch.head main\0";
        assert!(status_paths(out).is_empty());
    }

    /// A repository with one commit, and a database path beside it.
    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        run_git(dir.path(), &["init", "-q", "-b", "main"]).unwrap();
        run_git(dir.path(), &["config", "user.email", "test@example.com"]).unwrap();
        run_git(dir.path(), &["config", "user.name", "Test"]).unwrap();
        run_git(dir.path(), &["config", "commit.gpgsign", "false"]).unwrap();
        fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        run_git(dir.path(), &["add", "-A"]).unwrap();
        run_git(dir.path(), &["commit", "-q", "-m", "init"]).unwrap();
        dir
    }

    /// The fingerprint of `repo`, with no database: its path is in `.git`,
    /// where nothing is written.
    fn read(repo: &tempfile::TempDir) -> Fingerprint {
        Fingerprint::read(repo.path(), &repo.path().join(".git").join("unused.db"))
    }

    #[test]
    fn fingerprint_is_equal_twice_without_a_change() {
        let repo = repo();
        let first = read(&repo);
        assert!(first.git.is_some());
        assert_eq!(first, read(&repo));
    }

    #[test]
    fn fingerprint_changes_when_a_tracked_file_is_edited() {
        let repo = repo();
        let before = read(&repo);
        fs::write(repo.path().join("a.txt"), "two\n").unwrap();
        assert_ne!(before.git, read(&repo).git);
    }

    #[test]
    fn fingerprint_changes_again_when_a_modified_file_is_edited_again() {
        let repo = repo();
        fs::write(repo.path().join("a.txt"), "two\n").unwrap();
        let first_edit = read(&repo);
        // The status line of a modified file is the same after each edit, so
        // only the file's size and time tell the two apart.
        fs::write(repo.path().join("a.txt"), "three three three\n").unwrap();
        assert_ne!(first_edit.git, read(&repo).git);
    }

    #[test]
    fn fingerprint_changes_when_an_untracked_file_appears() {
        let repo = repo();
        let before = read(&repo);
        fs::write(repo.path().join("new.txt"), "hello\n").unwrap();
        assert_ne!(before.git, read(&repo).git);
    }

    #[test]
    fn fingerprint_changes_when_a_branch_is_created() {
        let repo = repo();
        let before = read(&repo);
        run_git(repo.path(), &["branch", "feature"]).unwrap();
        assert_ne!(before.git, read(&repo).git);
    }

    #[test]
    fn fingerprint_db_part_changes_when_the_database_is_rewritten() {
        let repo = repo();
        let scratch = tempfile::tempdir().unwrap();
        let db = scratch.path().join("rvw.db");
        fs::write(&db, "short").unwrap();
        let before = Fingerprint::read(repo.path(), &db);
        fs::write(&db, "much longer contents").unwrap();
        let after = Fingerprint::read(repo.path(), &db);
        assert_eq!(before.git, after.git);
        assert_ne!(before.db, after.db);
    }

    #[test]
    fn a_git_failure_keeps_the_previous_git_part() {
        let previous = Fingerprint {
            git: Some(7),
            db: [None, None],
        };
        let failed = Fingerprint {
            git: None,
            db: [None, None],
        };
        assert_eq!(failed.keep_git(Some(&previous)).git, Some(7));
        assert_eq!(failed.keep_git(None).git, None);
    }

    /// Runs git in `dir`; the error is its stderr, for a failed command.
    /// A job as the window would start it after git changed, with nothing
    /// open on either screen.
    fn job(repo: &tempfile::TempDir, db: &tempfile::TempDir) -> LiveJob {
        let store = rvw_store::Store::open(&db.path().join("rvw.db")).unwrap();
        LiveJob {
            fingerprint: Fingerprint {
                git: Some(1),
                db: [None, None],
            },
            key: LiveKey {
                epoch: 0,
                selected: None,
                open: None,
                compare: None,
            },
            git_changed: true,
            service: Arc::new(Service::new(Box::new(store))),
            workspace: repo.path().to_string_lossy().into_owned(),
            lane: String::new(),
            comments: Vec::new(),
            open_place: None,
        }
    }

    fn open_path(read: &LiveRead) -> Option<(Side, String)> {
        match read.changes.as_ref()?.open.as_ref()? {
            Ok(open) => Some((open.side, open.path.clone())),
            Err(message) => panic!("{message}"),
        }
    }

    #[test]
    fn a_live_read_reads_the_queue_the_open_file_the_changes_and_the_facts() {
        let repo = repo();
        let db = tempfile::tempdir().unwrap();
        fs::write(repo.path().join("a.txt"), "two\n").unwrap();
        let mut job = job(&repo, &db);
        job.key.selected = Some(("a.txt".to_owned(), ViewMode::Diff));

        let read = job.read();

        let snapshot = read.snapshot.as_ref().unwrap();
        assert!(snapshot.changes.iter().any(|file| file.path == "a.txt"));
        let selected = read.selected.as_ref().unwrap();
        assert_eq!(selected.path, "a.txt");
        assert!(selected.diff.is_ok());
        let lists = read.changes.as_ref().unwrap().lists.as_ref().unwrap();
        assert_eq!(lists.side(Side::Unstaged)[0].path, "a.txt");
        assert_eq!(open_path(&read), Some((Side::Unstaged, "a.txt".to_owned())));
        assert_eq!(read.facts.unwrap().branch.as_deref(), Some("main"));
    }

    #[test]
    fn a_live_read_follows_the_open_file_to_the_other_list() {
        let repo = repo();
        let db = tempfile::tempdir().unwrap();
        fs::write(repo.path().join("a.txt"), "two\n").unwrap();
        run_git(repo.path(), &["add", "a.txt"]).unwrap();
        let mut job = job(&repo, &db);
        job.open_place = Some((Side::Unstaged, "a.txt".to_owned(), 0));

        let read = job.read();

        assert_eq!(open_path(&read), Some((Side::Staged, "a.txt".to_owned())));
    }

    #[test]
    fn a_live_read_after_a_queue_change_alone_leaves_git_unread() {
        let repo = repo();
        let db = tempfile::tempdir().unwrap();
        let mut job = job(&repo, &db);
        job.git_changed = false;

        let read = job.read();

        assert!(read.snapshot.is_ok());
        assert!(read.changes.is_none());
        assert!(read.facts.is_none());
    }

    fn run_git(dir: &Path, args: &[&str]) -> Result<(), String> {
        let output = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .map_err(|e| e.to_string())?;
        if output.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).into_owned())
        }
    }
}
