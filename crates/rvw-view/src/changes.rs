//! Port of internal/tui/changes.go: what the worktree is compared with (the
//! changes view and the gutter signs), the changed files as a tree, and the
//! per-line change signs of a file.
//!
//! The comparison is always the files on disk against one commit.

use std::collections::BTreeMap;
use std::path::Path;

use rvw_git::gitx::{self, Change, Hunk};

use crate::tree::{build_tree, Tree};

/// What the worktree is compared with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compare {
    /// Uncommitted changes: against HEAD.
    Head,
    /// The branch: against its merge-base with main or master.
    Default,
    /// Against the commit before HEAD.
    Prev,
}

/// A revision to diff against, resolved from a [`Compare`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Base {
    /// What git diffs with: `HEAD`, a branch name, `HEAD~1`, or a commit id
    /// for a merge-base (a merge-base does not move when new commits land).
    pub id: String,
    /// A short name for display: `HEAD`, the branch, `HEAD~1`.
    pub name: String,
}

impl Compare {
    /// The menu label of this comparison.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Head => "uncommitted",
            Self::Default => "default branch",
            Self::Prev => "previous commit",
        }
    }

    /// Resolves this comparison to a revision git can diff with, in the
    /// worktree at `dir`. `HEAD` and `HEAD~1` stay names so a commit made
    /// meanwhile moves them; a merge-base is a commit id.
    ///
    /// # Errors
    ///
    /// Returns the message to show (without the `rvw:` prefix) when the
    /// comparison cannot be resolved, for example when there is no commit
    /// yet, no previous commit, or no main or master branch.
    pub fn base(self, dir: &Path) -> Result<Base, String> {
        match self {
            Self::Default => {
                let branch = gitx::default_branch(dir)
                    .ok_or_else(|| "no main or master branch to compare with".to_owned())?;
                if gitx::commit(dir, "HEAD").is_none() {
                    return Err("no commit to compare with yet".to_owned());
                }
                match gitx::merge_base(dir, &branch, "HEAD") {
                    Ok(id) if !id.is_empty() => Ok(Base { id, name: branch }),
                    _ => Err(format!("the branch shares no history with {branch}")),
                }
            }
            Self::Prev => {
                if gitx::commit(dir, "HEAD~1").is_none() {
                    return Err("no previous commit to compare with".to_owned());
                }
                Ok(Base {
                    id: "HEAD~1".to_owned(),
                    name: "HEAD~1".to_owned(),
                })
            }
            Self::Head => {
                if gitx::commit(dir, "HEAD").is_none() {
                    return Err("no commit to compare with yet".to_owned());
                }
                Ok(Base {
                    id: "HEAD".to_owned(),
                    name: "HEAD".to_owned(),
                })
            }
        }
    }
}

/// The changed files as a tree with every directory expanded, and each
/// path's status letter (git's `A`, `M`, `D`, `T`, `U`, or `?` for untracked).
///
/// A path listed twice takes the status of its last entry.
#[must_use]
pub fn change_tree(changes: &[Change]) -> (Tree, BTreeMap<String, u8>) {
    let mut status = BTreeMap::new();
    for c in changes {
        status.insert(c.path.clone(), c.status);
    }
    // Keys of a BTreeMap come out sorted, which is the order build_tree wants.
    let paths: Vec<&str> = status.keys().map(String::as_str).collect();
    let mut tree = build_tree(&paths);
    tree.expand_all_dirs();
    (tree, status)
}

/// The kind of change a line of a file has against the last commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sign {
    /// No change.
    None,
    /// A new line.
    Added,
    /// A line that replaced another.
    Changed,
    /// Lines were deleted below this one.
    Deleted,
    /// Lines were deleted above this one, the first line.
    DeletedTop,
}

impl Sign {
    /// The gutter glyph of this sign.
    #[must_use]
    pub fn glyph(self) -> &'static str {
        match self {
            Self::None => " ",
            Self::Added | Self::Changed => "▎",
            Self::Deleted => "▁",
            Self::DeletedTop => "▔",
        }
    }
}

/// The change sign of each of a file's `n` lines against the commit `base`,
/// and the line each hunk is marked on first, 1-indexed.
///
/// `path` is passed to git as it is given, so an absolute path works as it
/// does for the Go code. Both results are empty when there is nothing to say:
/// without a base, on any git failure, or when the file has no changes or no
/// lines. An untracked, non-ignored file is all added.
#[must_use]
pub fn file_changes(dir: &Path, path: &str, base: &str, n: usize) -> (Vec<Sign>, Vec<usize>) {
    if base.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let Ok((mut hunks, untracked)) = gitx::changes(dir, path, base) else {
        return (Vec::new(), Vec::new());
    };
    if untracked {
        hunks = vec![Hunk {
            old_start: 0,
            old_lines: 0,
            new_start: 1,
            new_lines: to_i64(n),
        }];
    }
    (signs(&hunks, n), hunk_starts(&hunks, n))
}

/// The line each hunk is marked on first, 1-indexed: a deletion's sign line
/// (clamped into the file), else its first new line. Empty when `n` is 0.
#[must_use]
pub fn hunk_starts(hunks: &[Hunk], n: usize) -> Vec<usize> {
    if n == 0 {
        return Vec::new();
    }
    let n = to_i64(n);
    hunks
        .iter()
        .filter_map(|h| usize::try_from(h.new_start.clamp(1, n)).ok())
        .collect()
}

/// Marks each of `n` lines with the hunks of a zero-context diff. Empty when
/// there are no hunks or no lines.
///
/// A hunk that adds no new lines marks the line before its old position
/// (`Deleted`), or the first line when it sits at the very top (`DeletedTop`).
/// Hunks past the end of the file are dropped.
#[must_use]
pub fn signs(hunks: &[Hunk], n: usize) -> Vec<Sign> {
    if hunks.is_empty() || n == 0 {
        return Vec::new();
    }
    let total = to_i64(n);
    let mut out = vec![Sign::None; n];
    // Sets a 1-indexed line, if it is in the file.
    let mut set = |line: i64, sign: Sign| {
        if (1..=total).contains(&line) {
            if let Ok(idx) = usize::try_from(line - 1) {
                out[idx] = sign;
            }
        }
    };
    for h in hunks {
        match (h.new_lines, h.new_start) {
            (0, 0) => set(1, Sign::DeletedTop),
            (0, start) => set(start, Sign::Deleted),
            (lines, start) => {
                let sign = if h.old_lines == 0 {
                    Sign::Added
                } else {
                    Sign::Changed
                };
                for line in start..start + lines {
                    set(line, sign);
                }
            }
        }
    }
    out
}

/// Converts a count to `i64`, saturating; a file cannot have that many lines.
fn to_i64(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;
    use tempfile::TempDir;

    const N: Sign = Sign::None;
    const A: Sign = Sign::Added;
    const C: Sign = Sign::Changed;
    const D: Sign = Sign::Deleted;
    const T: Sign = Sign::DeletedTop;

    fn hunk(old_start: i64, old_lines: i64, new_start: i64, new_lines: i64) -> Hunk {
        Hunk {
            old_start,
            old_lines,
            new_start,
            new_lines,
        }
    }

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

    /// A fresh repository on branch main with a local identity, no signing.
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
    fn signs_map_zero_context_hunks_to_lines() {
        let cases: [(&str, Vec<Hunk>, Vec<Sign>); 6] = [
            ("none", vec![], vec![]),
            ("added", vec![hunk(2, 0, 3, 2)], vec![N, N, A, A, N]),
            ("changed", vec![hunk(2, 1, 2, 2)], vec![N, C, C, N, N]),
            ("deleted", vec![hunk(3, 2, 2, 0)], vec![N, D, N, N, N]),
            (
                "deleted at top",
                vec![hunk(1, 1, 0, 0)],
                vec![T, N, N, N, N],
            ),
            ("past the end", vec![hunk(9, 0, 5, 4)], vec![N, N, N, N, A]),
        ];
        for (name, hunks, want) in cases {
            assert_eq!(signs(&hunks, 5), want, "{name}");
        }
    }

    #[test]
    fn signs_of_an_empty_file_are_empty() {
        assert!(signs(&[hunk(1, 0, 1, 1)], 0).is_empty());
    }

    #[test]
    fn hunk_starts_clamp_into_the_file() {
        let hunks = [hunk(1, 1, 0, 0), hunk(2, 0, 3, 2), hunk(9, 0, 99, 1)];
        assert_eq!(hunk_starts(&hunks, 5), vec![1, 3, 5]);
        assert!(hunk_starts(&hunks, 0).is_empty());
    }

    #[test]
    fn change_tree_expands_directories_and_keeps_the_last_status() {
        let changes = [
            Change {
                path: "src/b.go".into(),
                status: b'M',
            },
            Change {
                path: "README.md".into(),
                status: b'D',
            },
            Change {
                path: "src/b.go".into(),
                status: b'A',
            },
            Change {
                path: "src/api/a.go".into(),
                status: b'?',
            },
        ];
        let (tree, status) = change_tree(&changes);
        assert_eq!(status["src/b.go"], b'A');
        assert_eq!(status["README.md"], b'D');
        assert_eq!(status.len(), 3);
        let rows: Vec<String> = tree
            .rows()
            .iter()
            .map(|r| tree.node(r.node).path.clone())
            .collect();
        assert_eq!(
            rows,
            ["src", "src/api", "src/api/a.go", "src/b.go", "README.md"]
        );
    }

    #[test]
    fn change_tree_of_no_changes_is_empty() {
        let (tree, status) = change_tree(&[]);
        assert!(tree.rows().is_empty());
        assert!(status.is_empty());
    }

    #[test]
    fn compare_labels() {
        assert_eq!(Compare::Head.label(), "uncommitted");
        assert_eq!(Compare::Default.label(), "default branch");
        assert_eq!(Compare::Prev.label(), "previous commit");
    }

    #[test]
    fn sign_glyphs() {
        assert_eq!(N.glyph(), " ");
        assert_eq!(A.glyph(), "▎");
        assert_eq!(C.glyph(), "▎");
        assert_eq!(D.glyph(), "▁");
        assert_eq!(T.glyph(), "▔");
    }

    #[test]
    fn head_needs_a_commit_and_says_so() {
        let r = repo();
        write(r.path(), "a.txt", "x\n");
        git(r.path(), &["add", "a.txt"]);
        assert_eq!(
            Compare::Head.base(r.path()),
            Err("no commit to compare with yet".to_owned())
        );
        assert_eq!(
            Compare::Prev.base(r.path()),
            Err("no previous commit to compare with".to_owned())
        );
        assert_eq!(
            Compare::Default.base(r.path()),
            Err("no main or master branch to compare with".to_owned())
        );
    }

    #[test]
    fn without_a_commit_there_are_no_signs() {
        let r = repo();
        write(r.path(), "a.txt", "x\n");
        git(r.path(), &["add", "a.txt"]);
        let path = r.path().join("a.txt");
        let path = path.to_str().unwrap();
        assert_eq!(
            file_changes(r.path(), path, "HEAD", 1),
            (Vec::new(), Vec::new())
        );
    }

    #[test]
    fn head_base_is_the_name_head() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "init");
        assert_eq!(
            Compare::Head.base(r.path()),
            Ok(Base {
                id: "HEAD".into(),
                name: "HEAD".into()
            })
        );
        // Only one commit, so there is no previous one.
        assert!(Compare::Prev.base(r.path()).is_err());
    }

    #[test]
    fn prev_base_stays_a_name_and_default_is_the_merge_base() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "base");
        let fork = gitx::commit(r.path(), "HEAD").unwrap();
        git(r.path(), &["checkout", "-q", "-b", "feature"]);
        write(r.path(), "f.txt", "f\n");
        commit_all(r.path(), "feature");
        git(r.path(), &["checkout", "-q", "main"]);
        write(r.path(), "m.txt", "m\n");
        commit_all(r.path(), "main moves on");
        git(r.path(), &["checkout", "-q", "feature"]);

        assert_eq!(
            Compare::Prev.base(r.path()),
            Ok(Base {
                id: "HEAD~1".into(),
                name: "HEAD~1".into()
            })
        );
        assert_eq!(
            Compare::Default.base(r.path()),
            Ok(Base {
                id: fork,
                name: "main".into()
            })
        );
    }

    #[test]
    fn default_base_on_main_itself_is_main() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "base");
        let base = Compare::Default.base(r.path()).unwrap();
        assert_eq!(base.name, "main");
        assert_eq!(base.id, gitx::commit(r.path(), "HEAD").unwrap());
    }

    #[test]
    fn default_base_fails_when_the_branch_shares_no_history() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "base");
        git(r.path(), &["checkout", "-q", "--orphan", "lonely"]);
        git(r.path(), &["rm", "-q", "-rf", "."]);
        write(r.path(), "b.txt", "b\n");
        commit_all(r.path(), "unrelated");
        assert_eq!(
            Compare::Default.base(r.path()),
            Err("the branch shares no history with main".to_owned())
        );
    }

    #[test]
    fn file_changes_track_a_modified_file_against_head() {
        let r = repo();
        write(r.path(), "h.txt", "1\n2\n3\n4\n5\n");
        commit_all(r.path(), "init");
        write(r.path(), "h.txt", "1\nTWO\n4\n5\n6\n");
        let path = r.path().join("h.txt");
        let (signs, starts) = file_changes(r.path(), path.to_str().unwrap(), "HEAD", 5);
        assert_eq!(signs, [N, C, N, N, A]);
        assert_eq!(starts, [2, 5]);
    }

    #[test]
    fn file_changes_of_an_unchanged_file_are_empty() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "init");
        let (signs, starts) = file_changes(r.path(), "a.txt", "HEAD", 1);
        assert!(signs.is_empty());
        assert!(starts.is_empty());
    }

    #[test]
    fn file_changes_of_an_untracked_file_are_all_added() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "init");
        write(r.path(), "new.py", "a = 1\nb = 2\n");
        let (signs, starts) = file_changes(r.path(), "new.py", "HEAD", 2);
        assert_eq!(signs, [A, A]);
        assert_eq!(starts, [1]);
    }

    #[test]
    fn file_changes_of_a_deleted_top_line_mark_line_one() {
        let r = repo();
        write(r.path(), "d.txt", "a\nb\nc\n");
        commit_all(r.path(), "init");
        write(r.path(), "d.txt", "b\nc\n");
        let (signs, starts) = file_changes(r.path(), "d.txt", "HEAD", 2);
        assert_eq!(signs, [T, N]);
        assert_eq!(starts, [1]);
    }

    #[test]
    fn file_changes_without_a_base_are_empty() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "init");
        write(r.path(), "a.txt", "b\n");
        assert_eq!(
            file_changes(r.path(), "a.txt", "", 1),
            (Vec::new(), Vec::new())
        );
    }

    #[test]
    fn file_changes_outside_a_repo_are_empty() {
        let outside = TempDir::new().unwrap();
        write(outside.path(), "a.txt", "a\n");
        assert_eq!(
            file_changes(outside.path(), "a.txt", "HEAD", 1),
            (Vec::new(), Vec::new())
        );
    }

    #[test]
    fn file_changes_for_a_zero_line_file_are_empty() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "init");
        write(r.path(), "new.txt", "");
        assert_eq!(
            file_changes(r.path(), "new.txt", "HEAD", 0),
            (Vec::new(), Vec::new())
        );
    }
}
