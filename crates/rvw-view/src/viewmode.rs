//! The two ways to look at a file of the change: its diff, or the whole file
//! as it is on disk with the changed lines marked. Which one opens by default,
//! and which lines of a file the change touched.

use std::collections::BTreeSet;
use std::path::Path;

use crate::changes::{self, Sign};
use crate::keys::Screen;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    /// Only what differs, with a few lines of context.
    Diff,
    /// The whole file, the changed lines marked.
    File,
}

impl ViewMode {
    /// The other mode.
    #[must_use]
    pub fn other(self) -> Self {
        match self {
            Self::Diff => Self::File,
            Self::File => Self::Diff,
        }
    }
}

/// The mode a file opens in when the reviewer has not chosen: a click in the
/// Files tab wants the file, anywhere else the change.
#[must_use]
pub fn default_for(screen: Screen) -> ViewMode {
    if screen == Screen::Files {
        ViewMode::File
    } else {
        ViewMode::Diff
    }
}

/// The mode a file can show: a file the change does not touch has no diff.
#[must_use]
pub fn effective(wanted: ViewMode, has_changes: bool) -> ViewMode {
    if has_changes {
        wanted
    } else {
        ViewMode::File
    }
}

/// The 1-indexed lines a change added or modified; deletions leave no line.
#[must_use]
pub fn added_lines(signs: &[Sign]) -> BTreeSet<u32> {
    signs
        .iter()
        .enumerate()
        .filter(|(_, sign)| matches!(sign, Sign::Added | Sign::Changed))
        .filter_map(|(index, _)| u32::try_from(index + 1).ok())
        .collect()
}

/// The lines of the file at `path` (relative to `dir`) that differ from the
/// commit `base`. `bytes` is the file's content, which sizes the search.
/// Empty when git cannot say, which shows the file unmarked.
#[must_use]
pub fn marked_lines(dir: &Path, path: &str, base: &str, bytes: &[u8]) -> BTreeSet<u32> {
    // The count `str::lines` gives: a last line may lack its newline.
    let unterminated = bytes.last().is_some_and(|&b| b != b'\n');
    let lines = bytes.split(|&b| b == b'\n').count() - usize::from(!unterminated);
    let (signs, _) = changes::file_changes(dir, path, base, lines);
    added_lines(&signs)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::process::Command;

    use super::*;

    #[test]
    fn the_files_tab_wants_the_file_and_everywhere_else_the_change() {
        assert_eq!(default_for(Screen::Files), ViewMode::File);
        for screen in [Screen::Diff, Screen::Queue, Screen::Outcome, Screen::Search] {
            assert_eq!(default_for(screen), ViewMode::Diff);
        }
    }

    #[test]
    fn an_unchanged_file_only_has_the_file_view() {
        assert_eq!(effective(ViewMode::Diff, false), ViewMode::File);
        assert_eq!(effective(ViewMode::Diff, true), ViewMode::Diff);
        assert_eq!(effective(ViewMode::File, true), ViewMode::File);
        assert_eq!(ViewMode::Diff.other(), ViewMode::File);
    }

    #[test]
    fn added_and_changed_signs_are_lines_and_deletions_are_not() {
        let signs = [
            Sign::None,
            Sign::Added,
            Sign::Deleted,
            Sign::Changed,
            Sign::DeletedTop,
        ];
        assert_eq!(added_lines(&signs), BTreeSet::from([2, 4]));
        assert!(added_lines(&[]).is_empty());
    }

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .status()
            .expect("git runs");
        assert!(status.success(), "git {args:?}");
    }

    #[test]
    fn a_modified_file_marks_its_changed_and_new_lines() {
        let dir = tempfile::tempdir().expect("tempdir");
        git(dir.path(), &["init", "-q"]);
        fs::write(dir.path().join("h.txt"), "1\n2\n3\n4\n5\n").expect("write");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-qm", "init"]);
        let text = "1\nTWO\n4\n5\n6\n";
        fs::write(dir.path().join("h.txt"), text).expect("write");
        let marks = marked_lines(dir.path(), "h.txt", "HEAD", text.as_bytes());
        assert_eq!(marks, BTreeSet::from([2, 5]));
    }

    #[test]
    fn an_untracked_file_is_all_added() {
        let dir = tempfile::tempdir().expect("tempdir");
        git(dir.path(), &["init", "-q"]);
        fs::write(dir.path().join("a.txt"), "a\n").expect("write");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-qm", "init"]);
        let text = "x\ny\n";
        fs::write(dir.path().join("new.txt"), text).expect("write");
        let marks = marked_lines(dir.path(), "new.txt", "HEAD", text.as_bytes());
        assert_eq!(marks, BTreeSet::from([1, 2]));
    }
}
