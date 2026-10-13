//! The workspace the window showed last, so a launch from the Dock or a
//! launcher reopens it. The record is a sidecar file next to the queue
//! database, `<db>.last-workspace`, holding one line: the workspace path.
//! The launch order that uses it is `choose`.

use std::fs;
use std::path::{Path, PathBuf};

/// Reads the workspace recorded next to `db`. A missing, empty or unreadable
/// file gives `None`.
pub fn load(db: &Path) -> Option<String> {
    let text = fs::read_to_string(sidecar_path(db)).ok()?;
    let path = text.trim();
    (!path.is_empty()).then(|| path.to_owned())
}

/// Replaces the workspace recorded next to `db` with `workspace`. The new
/// content is written to a temporary file that is then renamed over the
/// sidecar, so a crash never leaves half a path.
///
/// # Errors
///
/// Fails with a message when the workspace holds a tab, a newline or a
/// carriage return, or when the file cannot be written or renamed.
pub fn save(db: &Path, workspace: &str) -> Result<(), String> {
    if workspace.contains(['\t', '\n', '\r']) {
        return Err("cannot record this workspace as the last one".into());
    }
    let sidecar = sidecar_path(db);
    let temporary = temporary_path(&sidecar);
    fs::write(&temporary, format!("{workspace}\n"))
        .map_err(|e| format!("could not record the last workspace: {e}"))?;
    fs::rename(&temporary, &sidecar)
        .map_err(|e| format!("could not record the last workspace: {e}"))
}

/// Picks the workspace to open at launch when no `--workspace` was given:
/// the working directory's repository, then the last workspace when
/// `is_repo` says it still is one, then `busiest`. `is_repo` and `busiest`
/// run git, so each runs only when the choices before it gave nothing.
pub fn choose(
    cwd_repo: Option<String>,
    last: Option<String>,
    is_repo: impl FnOnce(&str) -> bool,
    busiest: impl FnOnce() -> Option<String>,
) -> Option<String> {
    cwd_repo
        .or_else(|| last.filter(|path| is_repo(path)))
        .or_else(busiest)
}

/// The sidecar path: the database path with `.last-workspace` appended.
fn sidecar_path(db: &Path) -> PathBuf {
    let mut name = db.as_os_str().to_owned();
    name.push(".last-workspace");
    PathBuf::from(name)
}

/// The file a save writes before it is renamed over the sidecar. The process
/// id keeps two processes saving at once from mixing their writes.
fn temporary_path(sidecar: &Path) -> PathBuf {
    let mut name = sidecar.as_os_str().to_owned();
    name.push(format!(".{}.tmp", std::process::id()));
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_is_the_database_path_with_last_workspace_appended() {
        assert_eq!(
            sidecar_path(Path::new("/home/me/.cache/queue.db")),
            PathBuf::from("/home/me/.cache/queue.db.last-workspace")
        );
    }

    #[test]
    fn save_then_load_round_trips_the_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("queue.db");
        save(&db, "/ws/rvw").expect("save");
        assert_eq!(load(&db), Some("/ws/rvw".to_owned()));
    }

    #[test]
    fn saving_twice_keeps_only_the_second_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("queue.db");
        save(&db, "/ws/first").expect("first");
        save(&db, "/ws/second").expect("second");
        assert_eq!(load(&db), Some("/ws/second".to_owned()));
        let text = fs::read_to_string(sidecar_path(&db)).expect("read");
        assert_eq!(text, "/ws/second\n");
    }

    #[test]
    fn a_save_leaves_no_temporary_file_behind() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("queue.db");
        save(&db, "/ws/rvw").expect("save");
        let names: Vec<_> = fs::read_dir(dir.path())
            .expect("read dir")
            .map(|entry| entry.expect("entry").file_name())
            .collect();
        assert_eq!(
            names,
            vec![std::ffi::OsString::from("queue.db.last-workspace")]
        );
    }

    #[test]
    fn load_missing_file_is_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(load(&dir.path().join("queue.db")), None);
    }

    #[test]
    fn load_empty_or_blank_file_is_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("queue.db");
        fs::write(sidecar_path(&db), "").expect("write");
        assert_eq!(load(&db), None);
        fs::write(sidecar_path(&db), " \n\t\n").expect("write");
        assert_eq!(load(&db), None);
    }

    #[test]
    fn load_non_utf8_file_is_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("queue.db");
        fs::write(sidecar_path(&db), [0xff, 0xfe, b'\n']).expect("write");
        assert_eq!(load(&db), None);
    }

    #[test]
    fn load_trims_the_line() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("queue.db");
        fs::write(sidecar_path(&db), "  /ws/rvw \r\n").expect("write");
        assert_eq!(load(&db), Some("/ws/rvw".to_owned()));
    }

    #[test]
    fn save_refuses_a_path_that_cannot_be_one_line() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("queue.db");
        save(&db, "/ws/good").expect("first");
        assert!(save(&db, "/ws/bad\tpath").is_err());
        assert!(save(&db, "/ws/bad\npath").is_err());
        assert!(save(&db, "/ws/bad\rpath").is_err());
        assert_eq!(load(&db), Some("/ws/good".to_owned()));
    }

    #[test]
    fn save_reports_a_write_failure() {
        let dir = tempfile::tempdir().expect("tempdir");
        // The sidecar's parent is a missing directory, so the write fails.
        let db = dir.path().join("missing").join("queue.db");
        let error = save(&db, "/ws/rvw").expect_err("no such directory");
        assert!(error.starts_with("could not record the last workspace"));
    }

    #[test]
    fn the_working_directory_beats_the_last_and_the_busiest_without_asking_git() {
        let chosen = choose(
            Some("/cwd".into()),
            Some("/last".into()),
            |_| panic!("the last workspace is not checked"),
            || panic!("the busiest is not looked up"),
        );
        assert_eq!(chosen, Some("/cwd".to_owned()));
    }

    #[test]
    fn the_last_workspace_beats_the_busiest_when_it_is_a_repository() {
        let chosen = choose(
            None,
            Some("/last".into()),
            |path| path == "/last",
            || panic!("the busiest is not looked up"),
        );
        assert_eq!(chosen, Some("/last".to_owned()));
    }

    #[test]
    fn the_last_workspace_is_skipped_when_it_is_not_a_repository() {
        let chosen = choose(
            None,
            Some("/last".into()),
            |_| false,
            || Some("/busy".to_owned()),
        );
        assert_eq!(chosen, Some("/busy".to_owned()));
    }

    #[test]
    fn the_busiest_is_the_fallback() {
        let chosen = choose(None, None, |_| true, || Some("/busy".to_owned()));
        assert_eq!(chosen, Some("/busy".to_owned()));
    }

    #[test]
    fn nothing_to_open_is_none() {
        assert_eq!(choose(None, None, |_| true, || None), None);
        assert_eq!(choose(None, Some("/last".into()), |_| false, || None), None);
    }
}
