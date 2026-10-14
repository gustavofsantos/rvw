//! The git log: every git process the window runs, kept in memory for the
//! dock's read-only git tab. Entries stay structured so the tab can filter or
//! lay them out later; `lines` is only its first, plain view.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use rvw_git::GitLog;

/// The most entries kept: the oldest go first.
const KEPT: usize = 1000;

static ENTRIES: Mutex<VecDeque<GitLog>> = Mutex::new(VecDeque::new());
/// Counts the entries ever recorded, so a view learns of news cheaply.
static RECORDED: AtomicU64 = AtomicU64::new(0);

/// Records every git process from now on.
pub fn start() {
    rvw_git::set_logger(record);
}

fn record(entry: &GitLog) {
    if let Ok(mut entries) = ENTRIES.lock() {
        if entries.len() == KEPT {
            entries.pop_front();
        }
        entries.push_back(entry.clone());
        RECORDED.fetch_add(1, Ordering::Release);
    }
}

/// How many entries were ever recorded; it changes when the log does.
pub fn recorded() -> u64 {
    RECORDED.load(Ordering::Acquire)
}

/// The entries, oldest first.
pub fn entries() -> Vec<GitLog> {
    ENTRIES
        .lock()
        .map(|entries| entries.iter().cloned().collect())
        .unwrap_or_default()
}

/// What an entry shows: whether it failed, and its text lines.
pub struct Shown {
    pub failed: bool,
    pub command: String,
    pub detail: Vec<String>,
}

/// An entry as the tab lists it: the command, how it ended, and git's own
/// words when it said any, a commit hook's report included.
pub fn lines(entry: &GitLog) -> Shown {
    let ended = match entry.code {
        Some(0) => "ok".to_owned(),
        Some(code) => format!("exit {code}"),
        None => "killed by a signal".to_owned(),
    };
    let mut detail = vec![format!(
        "{ended}, {} ms, {} bytes out",
        entry.took.as_millis(),
        entry.stdout_bytes
    )];
    detail.extend(
        entry
            .stdout
            .lines()
            .chain(entry.stderr.lines())
            .map(str::to_owned),
    );
    Shown {
        failed: entry.code != Some(0),
        command: format!("git -C {} {}", entry.dir.display(), entry.args.join(" ")),
        detail,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use super::*;

    fn entry(code: Option<i32>, stdout: &str, stderr: &str) -> GitLog {
        GitLog {
            dir: PathBuf::from("/w"),
            args: vec!["commit".into(), "-q".into()],
            code,
            stdout_bytes: stdout.len(),
            stdout: stdout.into(),
            stderr: stderr.into(),
            took: Duration::from_millis(12),
        }
    }

    #[test]
    fn a_failed_commit_shows_the_hooks_whole_report() {
        let shown = lines(&entry(Some(1), "", "lint failed\nbad.clj:3\nusage: hook"));
        assert!(shown.failed);
        assert_eq!(shown.command, "git -C /w commit -q");
        assert_eq!(
            shown.detail,
            [
                "exit 1, 12 ms, 0 bytes out",
                "lint failed",
                "bad.clj:3",
                "usage: hook"
            ]
        );
    }

    #[test]
    fn a_read_shows_how_it_ended_and_its_size() {
        let shown = lines(&entry(Some(0), "", ""));
        assert!(!shown.failed);
        assert_eq!(shown.detail, ["ok, 12 ms, 0 bytes out"]);
    }

    #[test]
    fn the_log_keeps_the_newest_entries_and_counts_them() {
        let before = recorded();
        for _ in 0..KEPT + 5 {
            record(&entry(Some(0), "", ""));
        }
        assert!(entries().len() <= KEPT);
        assert!(recorded() >= before + (KEPT as u64) + 5);
    }
}
