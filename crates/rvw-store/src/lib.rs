//! rvw's queue in one local SQLite database, shared by every workspace on the
//! machine. Port of internal/store.

mod tx;

use std::fs::{self, OpenOptions};
use std::io::ErrorKind;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior};
use rvw_core::{Error, Repository, Result, Tx};

use tx::SqlTx;

const SCHEMA: &str = include_str!("schema.sql");
const SCHEMA_V2: &str = include_str!("v2.sql");
const SCHEMA_VERSION: i64 = 2;

/// Maps a SQLite failure to the internal error every store failure becomes.
fn internal(e: impl std::fmt::Display) -> Error {
    Error::internal(e.to_string())
}

/// Where the database lives: `$XDG_DATA_HOME/rvw/rvw.db`, else
/// `~/.local/share/rvw/rvw.db`. The XDG spec applies on every OS, and a relative
/// `$XDG_DATA_HOME` is ignored, as the spec requires.
///
/// # Errors
///
/// Fails when `$XDG_DATA_HOME` is unusable and `$HOME` is not set.
pub fn default_path() -> Result<PathBuf> {
    let data = match std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
    {
        Some(data) => data,
        None => home_dir()?.join(".local").join("share"),
    };
    Ok(data.join("rvw").join("rvw.db"))
}

fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| Error::internal("cannot locate the data directory: $HOME is not defined"))
}

/// A [`Repository`] backed by one SQLite connection. Each `Store` owns its
/// connection; separate processes or threads open their own.
pub struct Store {
    conn: Mutex<Connection>,
    path: PathBuf,
}

impl Store {
    /// Opens, creating when needed, the database at `path`, and brings its
    /// schema up to date. Every transaction takes the write lock up front, so
    /// concurrent processes queue behind each other instead of failing on
    /// upgrade. The journal stays in rollback mode: switching a fresh file to
    /// WAL does not wait on the busy handler, so concurrent first opens would
    /// fail with `SQLITE_BUSY`.
    ///
    /// # Errors
    ///
    /// Fails when the directory or file cannot be created, or the schema cannot
    /// be prepared (including a database written by a newer rvw).
    pub fn open(path: &Path) -> Result<Store> {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)
                .map_err(|e| internal(format!("cannot create {}: {e}", dir.display())))?;
        }
        // Code snapshots are private to the user: create the file 0600 before
        // SQLite would create it with the umask's mode.
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(path)
            .map(drop)
            .map_err(|e| internal(format!("cannot open {}: {e}", path.display())))?;
        let mut conn = Connection::open(path)
            .map_err(|e| internal(format!("cannot open {}: {e}", path.display())))?;
        migrate(&mut conn, path).map_err(|e| {
            Error::internal(format!("cannot prepare {}: {}", path.display(), e.message))
        })?;
        Ok(Store {
            conn: Mutex::new(conn),
            path: path.to_path_buf(),
        })
    }

    fn lock(&self) -> Result<MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|_| Error::internal("store connection poisoned by an earlier panic"))
    }

    /// Runs `f` in one immediate write transaction on the workspace. Only a
    /// successful `f` commits; any error rolls back and is returned unchanged.
    fn run(
        &self,
        workspace: &str,
        create: bool,
        f: &mut dyn FnMut(&mut dyn Tx) -> Result<()>,
    ) -> Result<()> {
        let mut conn = self.lock()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(internal)?;
        let ws_id = workspace_id(&tx, workspace, create)?;
        let mut t = SqlTx {
            tx,
            workspace: workspace.to_owned(),
            ws_id,
        };
        f(&mut t)?;
        t.tx.commit().map_err(internal)
    }
}

/// The id of `workspace`, inserting its row when `create` is set. An unknown
/// workspace without `create` gets id 0, which no row carries, so it reads empty.
fn workspace_id(tx: &Transaction<'_>, workspace: &str, create: bool) -> Result<i64> {
    let found: Option<i64> = tx
        .query_row(
            "SELECT id FROM workspaces WHERE path = ?",
            [workspace],
            |row| row.get(0),
        )
        .optional()
        .map_err(internal)?;
    match found {
        Some(id) => Ok(id),
        None if create => {
            tx.execute("INSERT INTO workspaces (path) VALUES (?)", [workspace])
                .map_err(internal)?;
            Ok(tx.last_insert_rowid())
        }
        None => Ok(0),
    }
}

/// Sets the connection's pragmas and brings the schema up to date, all in one
/// immediate transaction. A fresh database (version 0) gets `schema.sql` and
/// then `v2.sql`; a version 1 database gets `v2.sql` alone.
fn migrate(conn: &mut Connection, path: &Path) -> Result<()> {
    conn.pragma_update(None, "busy_timeout", 10_000)
        .map_err(internal)?;
    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(internal)?;
    // VACUUM INTO cannot run inside a transaction, so the backup is taken
    // before the write lock. The version is read again under the lock: another
    // process may have migrated in the meantime.
    if read_version(conn)? == 1 && path != Path::new(":memory:") {
        back_up_v1(conn, path)?;
    }
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(internal)?;
    let version = read_version(&tx)?;
    if version == SCHEMA_VERSION {
        return Ok(());
    }
    if version > SCHEMA_VERSION {
        return Err(Error::internal(format!(
            "schema version {version} is newer than this rvw understands ({SCHEMA_VERSION})"
        )));
    }
    if version < 1 {
        tx.execute_batch(SCHEMA).map_err(internal)?;
    }
    tx.execute_batch(SCHEMA_V2).map_err(internal)?;
    tx.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}"))
        .map_err(internal)?;
    tx.commit().map_err(internal)
}

fn read_version(conn: &Connection) -> Result<i64> {
    conn.query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(internal)
}

/// Copies a version 1 database to `<path>.v1.bak`, once, before the upgrade. The
/// copy is written under a temporary name and hard-linked into place, so the
/// backup appears whole or not at all, and a concurrent upgrader that loses the
/// race keeps the winner's copy. The copy is private like the database.
fn back_up_v1(conn: &Connection, path: &Path) -> Result<()> {
    let backup = suffixed(path, ".v1.bak");
    if fs::symlink_metadata(&backup).is_ok() {
        return Ok(());
    }
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let temp = suffixed(path, &format!(".v1.bak.{}.{nanos}.tmp", std::process::id()));
    // VACUUM INTO writes into an empty file it finds, so the file is created
    // here with the private mode first.
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temp)
        .map_err(|e| internal(format!("cannot back up {}: {e}", path.display())))?;
    let copied = conn
        .execute("VACUUM INTO ?", [temp.to_string_lossy().into_owned()])
        .map_err(internal)
        .and_then(|_| match fs::hard_link(&temp, &backup) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => Ok(()),
            // a filesystem without hard links still gets its backup
            Err(_) => fs::rename(&temp, &backup)
                .map_err(|e| internal(format!("cannot back up {}: {e}", path.display()))),
        });
    let _ = fs::remove_file(&temp);
    copied
}

/// `path` with `suffix` appended to its file name.
fn suffixed(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

impl Repository for Store {
    fn update(&self, workspace: &str, f: &mut dyn FnMut(&mut dyn Tx) -> Result<()>) -> Result<()> {
        self.run(workspace, true, f)
    }

    fn view(&self, workspace: &str, f: &mut dyn FnMut(&mut dyn Tx) -> Result<()>) -> Result<()> {
        self.run(workspace, false, f)
    }

    fn workspaces(&self) -> Result<Vec<String>> {
        let conn = self.lock()?;
        let mut stmt = conn
            .prepare("SELECT path FROM workspaces ORDER BY path")
            .map_err(internal)?;
        let paths = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(internal)?
            .collect::<rusqlite::Result<Vec<String>>>()
            .map_err(internal)?;
        Ok(paths)
    }

    fn location(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::thread;

    use rusqlite::Connection;
    use rvw_core::types::{
        comment_id, review_id, Actor, Comment, GateOverride, Location, Review, ReviewStatus,
        Status, Verdict, VerdictKind,
    };
    use rvw_core::{Error, ErrorKind, Repository, Result, Tx};
    use tempfile::TempDir;

    use super::Store;

    const WS: &str = "/work/proj";
    const N: usize = 8;

    fn fresh() -> (TempDir, Store) {
        let dir = TempDir::new().unwrap();
        let store = Store::open(&dir.path().join("rvw.db")).unwrap();
        (dir, store)
    }

    fn comment(seq: i64, lane: Option<&str>) -> Comment {
        Comment {
            points_at: None,
            reopens: None,
            verdict: None,
            id: comment_id(seq),
            status: Status::Pending,
            workspace: WS.to_owned(),
            lane: lane.map(str::to_owned),
            author: None,
            file: "a.go".to_owned(),
            path: format!("{WS}/a.go"),
            start_line: 1,
            end_line: 2,
            filetype: String::new(),
            code: "x".to_owned(),
            comment: "note".to_owned(),
            created_at: "2026-01-01T00:00:00+00:00".to_owned(),
            pulled_at: None,
            resolved_at: None,
            resolved_by: None,
            resolution_note: None,
            file_version: String::new(),
            resolved_file_version: String::new(),
            review_id: String::new(),
            edited_at: String::new(),
        }
    }

    fn comments_of(store: &Store, ws: &str) -> Vec<Comment> {
        let mut out = Vec::new();
        store
            .view(ws, &mut |tx: &mut dyn Tx| {
                out = tx.comments(&[])?;
                Ok(())
            })
            .unwrap();
        out
    }

    #[test]
    fn file_is_created_0600_inside_a_0700_directory() {
        let base = TempDir::new().unwrap();
        let dir = base.path().join("data");
        let path = dir.join("rvw.db");
        let _store = Store::open(&path).unwrap();

        let file_mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        let dir_mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(file_mode, 0o600);
        assert_eq!(dir_mode, 0o700);
    }

    #[test]
    fn reopening_is_idempotent() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("rvw.db");
        {
            let store = Store::open(&path).unwrap();
            store
                .update(WS, &mut |tx: &mut dyn Tx| {
                    let seq = tx.next_comment_seq()?;
                    tx.insert_comment(&comment(seq, None))
                })
                .unwrap();
        }
        let store = Store::open(&path).unwrap();
        assert_eq!(comments_of(&store, WS).len(), 1);
        let conn = Connection::open(&path).unwrap();
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 2);
    }

    #[test]
    fn newer_schema_is_refused_with_the_exact_message() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("rvw.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.pragma_update(None, "user_version", 3).unwrap();
        }
        let err = Store::open(&path).err().expect("open must fail");
        assert_eq!(
            err,
            Error::internal(format!(
                "cannot prepare {}: schema version 3 is newer than this rvw understands (2)",
                path.display()
            ))
        );
    }

    /// Builds a version 1 database the way the v1 binary left it: schema.sql
    /// alone, with comments, a submitted review and the id counters in use.
    fn build_v1(path: &Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(super::SCHEMA).unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
        conn.execute_batch(
            "INSERT INTO workspaces (path, next_comment_seq, next_review_seq) \
                 VALUES ('/work/proj', 4, 2);
             INSERT INTO comments (workspace_id, seq, status, lane, author, file, path, \
                 start_line, end_line, filetype, code, comment, created_at, pulled_at, \
                 resolved_at, resolved_by, resolution_note, file_version, \
                 resolved_file_version, edited_at) VALUES
               (1, 1, 'pending', 'auth', 'ana', 'a.go', '/work/proj/a.go', 1, 2, 'go', 'x', \
                 'note one', '2026-01-01T00:00:00+00:00', NULL, NULL, NULL, NULL, 'abc', NULL, NULL),
               (1, 2, 'done', NULL, NULL, 'b.go', '/work/proj/b.go', 3, 5, '', 'y', \
                 'note two', '2026-01-01T00:00:01+00:00', '2026-01-02T00:00:00+00:00', \
                 '2026-01-03T00:00:00+00:00', 'bo', 'fixed', NULL, 'def', NULL),
               (1, 3, 'pulled', 'auth', NULL, 'a.go', '/work/proj/a.go', 4, 4, 'go', 'z', \
                 'note three', '2026-01-01T00:00:02+00:00', '2026-01-02T00:00:00+00:00', \
                 NULL, NULL, NULL, 'abc', NULL, '2026-01-04T00:00:00+00:00');
             INSERT INTO reviews (workspace_id, seq, status, lane, author, decision, summary, \
                 created_at, pulled_at) VALUES \
                 (1, 1, 'pending', NULL, 'bo', 'request-changes', 'two notes', \
                 '2026-01-05T00:00:00+00:00', NULL);
             INSERT INTO review_comments (workspace_id, review_seq, position, comment_seq) \
                 VALUES (1, 1, 0, 3), (1, 1, 1, 1);",
        )
        .unwrap();
    }

    /// The v1 comments as the v2 store reads them: the same rows, the new
    /// fields empty.
    fn v1_comments() -> Vec<Comment> {
        let mut one = comment(1, Some("auth"));
        one.author = Some("ana".to_owned());
        one.filetype = "go".to_owned();
        one.comment = "note one".to_owned();
        one.file_version = "abc".to_owned();
        one.review_id = "rv1".to_owned();

        let mut two = comment(2, None);
        two.status = Status::Done;
        two.file = "b.go".to_owned();
        two.path = format!("{WS}/b.go");
        two.start_line = 3;
        two.end_line = 5;
        two.code = "y".to_owned();
        two.comment = "note two".to_owned();
        two.created_at = "2026-01-01T00:00:01+00:00".to_owned();
        two.pulled_at = Some("2026-01-02T00:00:00+00:00".to_owned());
        two.resolved_at = Some("2026-01-03T00:00:00+00:00".to_owned());
        two.resolved_by = Some("bo".to_owned());
        two.resolution_note = Some("fixed".to_owned());
        two.resolved_file_version = "def".to_owned();

        let mut three = comment(3, Some("auth"));
        three.status = Status::Pulled;
        three.filetype = "go".to_owned();
        three.start_line = 4;
        three.end_line = 4;
        three.code = "z".to_owned();
        three.comment = "note three".to_owned();
        three.created_at = "2026-01-01T00:00:02+00:00".to_owned();
        three.pulled_at = Some("2026-01-02T00:00:00+00:00".to_owned());
        three.file_version = "abc".to_owned();
        three.review_id = "rv1".to_owned();
        three.edited_at = "2026-01-04T00:00:00+00:00".to_owned();

        vec![one, two, three]
    }

    /// Every column of `comments` and the names of all tables, as SQLite
    /// describes them.
    fn schema_of(path: &Path) -> (Vec<String>, Vec<String>) {
        let conn = Connection::open(path).unwrap();
        let mut stmt = conn.prepare("PRAGMA table_info(comments)").unwrap();
        let columns = stmt
            .query_map([], |row| {
                Ok(format!(
                    "{}|{}|{}|{}|{}|{}",
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                    row.get::<_, i64>(5)?
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<String>>>()
            .unwrap();
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .unwrap();
        let tables = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<String>>>()
            .unwrap();
        (columns, tables)
    }

    #[test]
    fn upgrading_v1_keeps_every_row_and_backs_the_file_up() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("rvw.db");
        build_v1(&path);

        let store = Store::open(&path).unwrap();
        assert_eq!(comments_of(&store, WS), v1_comments());
        store
            .view(WS, &mut |tx: &mut dyn Tx| {
                let rv = tx.review("rv1")?.expect("rv1 survives the upgrade");
                assert_eq!(rv.comment_ids, vec!["r3", "r1"]);
                assert_eq!(rv.summary, "two notes");
                Ok(())
            })
            .unwrap();
        store
            .update(WS, &mut |tx: &mut dyn Tx| {
                assert_eq!(tx.next_comment_seq()?, 4, "comment counter kept");
                assert_eq!(tx.next_review_seq()?, 2, "review counter kept");
                Ok(())
            })
            .unwrap();
        drop(store);

        let backup = dir.path().join("rvw.db.v1.bak");
        let conn = Connection::open(&backup).unwrap();
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 1, "the backup is the v1 database");
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM comments", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rows, 3);
    }

    #[test]
    fn upgraded_schema_matches_a_fresh_one_and_fresh_gets_no_backup() {
        let dir = TempDir::new().unwrap();
        let fresh = dir.path().join("fresh.db");
        let upgraded = dir.path().join("upgraded.db");
        drop(Store::open(&fresh).unwrap());
        build_v1(&upgraded);
        drop(Store::open(&upgraded).unwrap());

        assert_eq!(schema_of(&fresh), schema_of(&upgraded));
        assert!(schema_of(&fresh).1.contains(&"gate_overrides".to_owned()));
        assert!(!dir.path().join("fresh.db.v1.bak").exists());
    }

    #[test]
    fn concurrent_upgrades_of_one_v1_database_all_succeed() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("rvw.db");
        build_v1(&path);
        let opened: Vec<Result<()>> = thread::scope(|scope| {
            let handles: Vec<_> = (0..N)
                .map(|_| scope.spawn(|| Store::open(&path).map(drop)))
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        assert!(opened.iter().all(Result::is_ok), "{opened:?}");

        let store = Store::open(&path).unwrap();
        assert_eq!(comments_of(&store, WS), v1_comments());
        assert!(dir.path().join("rvw.db.v1.bak").exists());
    }

    #[test]
    fn points_at_and_reopens_round_trip() {
        let (_dir, store) = fresh();
        let mut reopened = comment(2, None);
        reopened.points_at = Some(Location {
            file: "src/b.rs".to_owned(),
            start_line: 7,
            end_line: 9,
        });
        reopened.reopens = Some("r1".to_owned());
        store
            .update(WS, &mut |tx: &mut dyn Tx| {
                for seq in [1, 2] {
                    assert_eq!(tx.next_comment_seq()?, seq);
                    let c = if seq == 1 {
                        comment(1, None)
                    } else {
                        reopened.clone()
                    };
                    tx.insert_comment(&c)?;
                }
                Ok(())
            })
            .unwrap();
        assert_eq!(comments_of(&store, WS), vec![comment(1, None), reopened]);
    }

    #[test]
    fn verdicts_round_trip_through_update_and_keep_points_at() {
        let (_dir, store) = fresh();
        let mut decided = comment(1, Some("auth"));
        decided.status = Status::Done;
        decided.author = Some("ana".to_owned());
        decided.pulled_at = Some("2026-01-02T00:00:00+00:00".to_owned());
        decided.resolved_at = Some("2026-01-03T00:00:00+00:00".to_owned());
        decided.resolved_by = Some("bo".to_owned());
        decided.points_at = Some(Location {
            file: "a.go".to_owned(),
            start_line: 1,
            end_line: 1,
        });
        store
            .update(WS, &mut |tx: &mut dyn Tx| {
                tx.next_comment_seq()?;
                tx.insert_comment(&decided)
            })
            .unwrap();

        let mut reopened = decided.clone();
        reopened.verdict = Some(Verdict {
            kind: VerdictKind::Reopened,
            by: "cy".to_owned(),
            actor: Actor::Human,
            at: "2026-01-04T00:00:00+00:00".to_owned(),
            note: Some("do it anyway".to_owned()),
            followup: Some("r2".to_owned()),
        });
        // The write does not rewrite points_at, so a stale copy here changes nothing.
        reopened.points_at = None;
        store
            .update(WS, &mut |tx: &mut dyn Tx| tx.update_comment(&reopened))
            .unwrap();
        decided.verdict = reopened.verdict.clone();
        assert_eq!(comments_of(&store, WS), vec![decided.clone()]);

        let mut accepted = comment(2, None);
        accepted.status = Status::Rejected;
        accepted.pulled_at = Some("2026-01-02T00:00:00+00:00".to_owned());
        accepted.resolved_by = Some("bo".to_owned());
        store
            .update(WS, &mut |tx: &mut dyn Tx| {
                tx.next_comment_seq()?;
                tx.insert_comment(&accepted)
            })
            .unwrap();
        accepted.verdict = Some(Verdict {
            kind: VerdictKind::Accepted,
            by: "cy".to_owned(),
            actor: Actor::Agent,
            at: "2026-01-05T00:00:00+00:00".to_owned(),
            note: None,
            followup: None,
        });
        store
            .update(WS, &mut |tx: &mut dyn Tx| tx.update_comment(&accepted))
            .unwrap();
        assert_eq!(comments_of(&store, WS), vec![decided, accepted]);
    }

    #[test]
    fn gate_overrides_keep_order_lane_and_empty_lists() {
        let (_dir, store) = fresh();
        let first = GateOverride {
            workspace: WS.to_owned(),
            lane: None,
            action: "commit".to_owned(),
            reason: "shipping anyway".to_owned(),
            by: "ana".to_owned(),
            actor: Actor::Human,
            at: "2026-01-06T00:00:00+00:00".to_owned(),
            open: vec!["r1".to_owned(), "r2".to_owned()],
            unjudged: Vec::new(),
        };
        let second = GateOverride {
            workspace: WS.to_owned(),
            lane: Some("auth".to_owned()),
            action: "push".to_owned(),
            reason: "hotfix".to_owned(),
            by: "bo".to_owned(),
            actor: Actor::Agent,
            at: "2026-01-07T00:00:00+00:00".to_owned(),
            open: Vec::new(),
            unjudged: vec!["r3".to_owned()],
        };
        store
            .update(WS, &mut |tx: &mut dyn Tx| {
                tx.insert_override(&first)?;
                tx.insert_override(&second)
            })
            .unwrap();
        store
            .view(WS, &mut |tx: &mut dyn Tx| {
                assert_eq!(tx.overrides()?, vec![first.clone(), second.clone()]);
                Ok(())
            })
            .unwrap();
        store
            .view("/elsewhere", &mut |tx: &mut dyn Tx| {
                assert!(tx.overrides()?.is_empty());
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn unknown_workspace_reads_empty_and_is_not_created() {
        let (_dir, store) = fresh();
        store
            .view("/nowhere", &mut |tx: &mut dyn Tx| {
                assert_eq!(tx.comments(&[])?.len(), 0);
                assert_eq!(tx.reviews(&[])?.len(), 0);
                assert!(tx.comment("r1")?.is_none());
                Ok(())
            })
            .unwrap();
        assert_eq!(store.workspaces().unwrap().len(), 0);
    }

    #[test]
    fn a_deleted_comment_does_not_free_its_id() {
        let (_dir, store) = fresh();
        store
            .update(WS, &mut |tx: &mut dyn Tx| {
                for _ in 0..2 {
                    let seq = tx.next_comment_seq()?;
                    tx.insert_comment(&comment(seq, None))?;
                }
                tx.delete_comment("r2")?;
                assert!(tx.comment("r2")?.is_none());
                assert_eq!(tx.next_comment_seq()?, 3);
                Ok(())
            })
            .unwrap();
        assert_eq!(comments_of(&store, WS).len(), 1);
    }

    #[test]
    fn ids_are_monotonic_and_never_reused() {
        let (_dir, store) = fresh();
        store
            .update(WS, &mut |tx: &mut dyn Tx| {
                assert_eq!(tx.next_comment_seq()?, 1);
                assert_eq!(tx.next_comment_seq()?, 2);
                assert_eq!(tx.next_review_seq()?, 1);
                let third = tx.next_comment_seq()?;
                assert_eq!(third, 3);
                tx.insert_comment(&comment(third, None))?;
                assert_eq!(tx.next_comment_seq()?, 4);
                assert_eq!(tx.next_review_seq()?, 2);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn every_comment_field_round_trips() {
        let (_dir, store) = fresh();
        let original = Comment {
            points_at: None,
            reopens: None,
            verdict: None,
            id: "r7".to_owned(),
            status: Status::Pulled,
            workspace: WS.to_owned(),
            lane: Some("auth".to_owned()),
            author: Some("ana".to_owned()),
            file: "src/a.rs".to_owned(),
            path: format!("{WS}/src/a.rs"),
            start_line: 4,
            end_line: 9,
            filetype: "rust".to_owned(),
            code: "fn a() {}".to_owned(),
            comment: "rename".to_owned(),
            created_at: "2026-01-01T00:00:00+00:00".to_owned(),
            pulled_at: Some("2026-01-02T00:00:00+00:00".to_owned()),
            resolved_at: None,
            resolved_by: None,
            resolution_note: None,
            file_version: "abc123".to_owned(),
            resolved_file_version: String::new(),
            review_id: String::new(),
            edited_at: "2026-01-03T00:00:00+00:00".to_owned(),
        };
        store
            .update(WS, &mut |tx: &mut dyn Tx| {
                tx.next_comment_seq()?;
                tx.insert_comment(&original)
            })
            .unwrap();
        assert_eq!(comments_of(&store, WS), vec![original.clone()]);

        let mut resolved = original.clone();
        resolved.status = Status::Done;
        resolved.resolved_at = Some("2026-01-04T00:00:00+00:00".to_owned());
        resolved.resolved_by = Some("bo".to_owned());
        resolved.resolution_note = Some("fixed".to_owned());
        resolved.resolved_file_version = "def456".to_owned();
        resolved.comment = "rename it".to_owned();
        store
            .update(WS, &mut |tx: &mut dyn Tx| tx.update_comment(&resolved))
            .unwrap();
        assert_eq!(comments_of(&store, WS), vec![resolved]);
    }

    #[test]
    fn review_links_keep_order_and_fill_review_ids() {
        let (_dir, store) = fresh();
        store
            .update(WS, &mut |tx: &mut dyn Tx| {
                for _ in 0..3 {
                    let seq = tx.next_comment_seq()?;
                    tx.insert_comment(&comment(seq, Some("auth")))?;
                }
                let seq = tx.next_review_seq()?;
                tx.insert_review(&Review {
                    id: review_id(seq),
                    status: ReviewStatus::Pending,
                    workspace: WS.to_owned(),
                    lane: None,
                    author: None,
                    decision: "comment".to_owned(),
                    summary: "s".to_owned(),
                    comment_ids: vec!["r3".to_owned(), "r1".to_owned()],
                    created_at: "2026-01-01T00:00:00+00:00".to_owned(),
                    pulled_at: None,
                })
            })
            .unwrap();

        store
            .view(WS, &mut |tx: &mut dyn Tx| {
                let cs = tx.comments(&[Status::Pending])?;
                assert_eq!(cs.len(), 3);
                assert_eq!(cs[0].id, "r1");
                assert_eq!(cs[2].id, "r3");
                assert_eq!(cs[0].review_id, "rv1");
                assert_eq!(cs[1].review_id, "");
                assert_eq!(cs[0].author, None);
                assert_eq!(cs[0].lane.as_deref(), Some("auth"));
                assert_eq!(cs[0].workspace, WS);

                let rv = tx.review("rv1")?.expect("rv1 exists");
                assert_eq!(rv.comment_ids, vec!["r3", "r1"]);
                assert_eq!(tx.reviews(&[ReviewStatus::Pulled])?.len(), 0);
                assert_eq!(tx.reviews(&[])?.len(), 1);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn malformed_and_unknown_ids_are_not_found() {
        let (_dir, store) = fresh();
        store
            .update(WS, &mut |tx: &mut dyn Tx| {
                tx.next_comment_seq()?;
                tx.insert_comment(&comment(1, None))?;
                assert!(tx.comment("r99")?.is_none());
                assert!(tx.comment("rv1")?.is_none());
                assert!(tx.comment("r")?.is_none());
                assert!(tx.comment("rx")?.is_none());
                assert!(tx.review("r1")?.is_none());
                assert!(tx.review("rv1")?.is_none());
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn insert_rejects_malformed_ids() {
        let (_dir, store) = fresh();
        let err = store
            .update(WS, &mut |tx: &mut dyn Tx| {
                let mut c = comment(1, None);
                c.id = "x1".to_owned();
                tx.insert_comment(&c)
            })
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::Internal);
    }

    #[test]
    fn error_rolls_back_and_keeps_its_kind() {
        let (_dir, store) = fresh();
        let err = store
            .update(WS, &mut |tx: &mut dyn Tx| {
                let seq = tx.next_comment_seq()?;
                tx.insert_comment(&comment(seq, None))?;
                Err(Error::conflict("boom"))
            })
            .unwrap_err();
        assert_eq!(err, Error::conflict("boom"));
        assert_eq!(comments_of(&store, WS).len(), 0);
        assert!(
            store.workspaces().unwrap().is_empty(),
            "the workspace insert rolled back too"
        );
    }

    #[test]
    fn workspaces_are_listed_sorted_and_location_is_the_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("rvw.db");
        let store = Store::open(&path).unwrap();
        for ws in ["/b", "/a"] {
            store.update(ws, &mut |_tx: &mut dyn Tx| Ok(())).unwrap();
        }
        assert_eq!(store.workspaces().unwrap(), vec!["/a", "/b"]);
        assert_eq!(store.location(), path.to_string_lossy());
    }

    #[test]
    fn concurrent_writers_do_not_lose_rows() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("rvw.db");
        let seqs: Vec<Result<i64>> = thread::scope(|scope| {
            let handles: Vec<_> = (0..N)
                .map(|_| {
                    scope.spawn(|| -> Result<i64> {
                        let store = Store::open(&path)?;
                        let mut seq = 0;
                        store.update(WS, &mut |tx: &mut dyn Tx| {
                            seq = tx.next_comment_seq()?;
                            tx.insert_comment(&comment(seq, None))
                        })?;
                        Ok(seq)
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        let mut got: Vec<i64> = seqs.into_iter().map(|r| r.unwrap()).collect();
        got.sort_unstable();
        assert_eq!(got, (1..=i64::try_from(N).unwrap()).collect::<Vec<_>>());

        let store = Store::open(&path).unwrap();
        assert_eq!(comments_of(&store, WS).len(), N);
    }
}
