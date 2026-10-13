//! Which done comments the reviewer has looked at. The record is a sidecar
//! file next to the queue database, `<db>.seen`, with one line per comment:
//! the workspace, a tab, then the comment id. A done comment counts as waiting
//! on the reviewer until its id is here for its workspace.

use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

/// The seen record of one database, loaded once and kept in step with the file.
pub struct Seen {
    path: PathBuf,
    /// One key per recorded comment, in the file's line format.
    set: HashSet<String>,
}

impl Seen {
    /// Reads the record next to `db`. A missing file is an empty record; a
    /// line that is not `workspace<TAB>id` is skipped.
    pub fn load(db: &Path) -> Seen {
        let path = sidecar_path(db);
        let set: HashSet<String> = fs::read(&path)
            .map(|bytes| {
                String::from_utf8_lossy(&bytes)
                    .lines()
                    .filter_map(parse_line)
                    .collect()
            })
            .unwrap_or_default();
        Seen { path, set }
    }

    pub fn contains(&self, workspace: &str, id: &str) -> bool {
        self.set.contains(&format_line(workspace, id))
    }

    /// Records that the reviewer has seen `id` in `workspace`, appending one
    /// line to the file. Recording it twice writes nothing the second time.
    ///
    /// # Errors
    ///
    /// Fails with a message when the workspace or id cannot be a line, or
    /// when the file cannot be written.
    pub fn mark(&mut self, workspace: &str, id: &str) -> Result<(), String> {
        if workspace.contains(['\t', '\n', '\r']) || id.contains(['\t', '\n', '\r']) {
            return Err("cannot record this outcome as seen".into());
        }
        let line = format_line(workspace, id);
        if self.set.contains(&line) {
            return Ok(());
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|e| format!("could not record the outcome as seen: {e}"))?;
        writeln!(file, "{line}")
            .map_err(|e| format!("could not record the outcome as seen: {e}"))?;
        self.set.insert(line);
        Ok(())
    }

    /// The ids recorded for `workspace`: the acknowledged set the queue takes.
    pub fn ids_for(&self, workspace: &str) -> HashSet<String> {
        let prefix = format!("{workspace}\t");
        self.set
            .iter()
            .filter_map(|line| line.strip_prefix(&prefix))
            .map(str::to_owned)
            .collect()
    }
}

/// The sidecar path: the database path with `.seen` appended.
fn sidecar_path(db: &Path) -> PathBuf {
    let mut name = db.as_os_str().to_owned();
    name.push(".seen");
    PathBuf::from(name)
}

/// The line that records one comment. The id never holds a tab, so the
/// workspace is everything before the last tab of a line.
fn format_line(workspace: &str, id: &str) -> String {
    format!("{workspace}\t{id}")
}

/// The record key of a line, or `None` when the line has no tab or an empty
/// workspace or id.
fn parse_line(line: &str) -> Option<String> {
    let (workspace, id) = line.rsplit_once('\t')?;
    if workspace.is_empty() || id.is_empty() {
        return None;
    }
    Some(line.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_is_the_database_path_with_seen_appended() {
        assert_eq!(
            sidecar_path(Path::new("/home/me/.cache/queue.db")),
            PathBuf::from("/home/me/.cache/queue.db.seen")
        );
    }

    #[test]
    fn a_line_is_workspace_tab_id() {
        assert_eq!(format_line("/ws/rvw", "r9"), "/ws/rvw\tr9");
    }

    #[test]
    fn parse_line_accepts_a_workspace_and_an_id() {
        assert_eq!(parse_line("/ws/rvw\tr9"), Some("/ws/rvw\tr9".into()));
        assert_eq!(
            parse_line("/ws\twith tab\tr9"),
            Some("/ws\twith tab\tr9".into())
        );
    }

    #[test]
    fn parse_line_skips_lines_without_both_parts() {
        assert_eq!(parse_line(""), None);
        assert_eq!(parse_line("no tab here"), None);
        assert_eq!(parse_line("\tr9"), None);
        assert_eq!(parse_line("/ws\t"), None);
    }

    #[test]
    fn load_missing_file_is_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let seen = Seen::load(&dir.path().join("queue.db"));
        assert!(!seen.contains("/ws", "r1"));
        assert!(seen.ids_for("/ws").is_empty());
    }

    #[test]
    fn mark_then_load_again_remembers_the_id() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("queue.db");
        let mut seen = Seen::load(&db);
        seen.mark("/ws", "r1").expect("mark");
        assert!(seen.contains("/ws", "r1"));
        assert!(!seen.contains("/ws", "r2"));

        let reloaded = Seen::load(&db);
        assert!(reloaded.contains("/ws", "r1"));
        assert_eq!(reloaded.ids_for("/ws"), HashSet::from(["r1".to_owned()]));
    }

    #[test]
    fn marking_twice_writes_one_line() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("queue.db");
        let mut seen = Seen::load(&db);
        seen.mark("/ws", "r1").expect("first");
        seen.mark("/ws", "r1").expect("second");
        let text = fs::read_to_string(sidecar_path(&db)).expect("read");
        assert_eq!(text, "/ws\tr1\n");
    }

    #[test]
    fn ids_are_kept_per_workspace() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut seen = Seen::load(&dir.path().join("queue.db"));
        seen.mark("/a", "r1").expect("mark a");
        seen.mark("/b", "r2").expect("mark b");
        assert_eq!(seen.ids_for("/a"), HashSet::from(["r1".to_owned()]));
        assert_eq!(seen.ids_for("/b"), HashSet::from(["r2".to_owned()]));
        assert!(!seen.contains("/a", "r2"));
    }

    #[test]
    fn unreadable_lines_are_skipped() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("queue.db");
        let mut bytes = b"garbage line\n/ws\tr1\n\tr2\n/ws\t\n".to_vec();
        bytes.extend_from_slice(&[0xff, 0xfe, b'\n']);
        bytes.extend_from_slice(b"/ws\tr3\n");
        fs::write(sidecar_path(&db), bytes).expect("write");
        let seen = Seen::load(&db);
        assert_eq!(
            seen.ids_for("/ws"),
            HashSet::from(["r1".to_owned(), "r3".to_owned()])
        );
    }

    #[test]
    fn mark_refuses_a_workspace_that_cannot_be_a_line() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut seen = Seen::load(&dir.path().join("queue.db"));
        assert!(seen.mark("/bad\tpath", "r1").is_err());
        assert!(seen.mark("/ws", "r1\nr2").is_err());
        assert!(!seen.contains("/bad\tpath", "r1"));
    }

    #[test]
    fn mark_reports_a_write_failure() {
        let dir = tempfile::tempdir().expect("tempdir");
        // The sidecar's parent is a missing directory, so the append fails.
        let db = dir.path().join("missing").join("queue.db");
        let mut seen = Seen::load(&db);
        let error = seen.mark("/ws", "r1").expect_err("no such directory");
        assert!(error.starts_with("could not record the outcome as seen"));
        assert!(!seen.contains("/ws", "r1"));
    }
}
