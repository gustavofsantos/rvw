//! The index: what is staged and what is not, the two diffs that separate them, and
//! the commands that move changes between them.

use std::fs;
use std::path::Path;

use crate::gitx::{self, GitError};

/// One changed path as `git status` sees it. A letter is git's: M, A, D, R, C, T,
/// U, or b'?' for untracked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStatus {
    pub path: String,
    /// The path before a rename or copy staged in the index. Unstaging only `path`
    /// leaves the deletion of this path staged, so both need unstaging.
    pub orig_path: Option<String>,
    /// The change between HEAD and the index, if any.
    pub staged: Option<u8>,
    /// The change between the index and the worktree, if any. b'?' when untracked.
    pub unstaged: Option<u8>,
}

/// Every changed path in the repository at `dir`, sorted by path. Paths are
/// relative to the top of the worktree.
pub fn status(dir: &Path) -> Result<Vec<FileStatus>, GitError> {
    let out = gitx::run(
        dir,
        &[],
        &["status", "--porcelain=v2", "-z", "--untracked-files=all"],
    )?;
    Ok(parse_status(&out))
}

/// Reads `git status --porcelain=v2 -z` output. Header lines (`#`) and the
/// records of ignored files are skipped.
fn parse_status(out: &str) -> Vec<FileStatus> {
    let mut files = Vec::new();
    let mut records = out.split('\0');
    while let Some(record) = records.next() {
        if let Some(entry) = parse_record(record, &mut records) {
            files.push(entry);
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    files
}

/// Parses one record. A rename or copy takes the next record as its original
/// path, so `next` is the rest of the records.
fn parse_record<'a>(record: &str, next: &mut impl Iterator<Item = &'a str>) -> Option<FileStatus> {
    let (kind, rest) = record.split_once(' ')?;
    match kind {
        // 1 XY sub mH mI mW hH hI path
        "1" => {
            let fields: Vec<&str> = rest.splitn(8, ' ').collect();
            let [xy, .., path] = fields.as_slice() else {
                return None;
            };
            let (staged, unstaged) = letters(xy);
            Some(FileStatus {
                path: (*path).to_owned(),
                orig_path: None,
                staged,
                unstaged,
            })
        }
        // 2 XY sub mH mI mW hH hI score path, then the original path
        "2" => {
            let fields: Vec<&str> = rest.splitn(9, ' ').collect();
            let [xy, .., path] = fields.as_slice() else {
                return None;
            };
            let orig = next.next()?;
            let (staged, unstaged) = letters(xy);
            Some(FileStatus {
                path: (*path).to_owned(),
                orig_path: Some(orig.to_owned()),
                staged,
                unstaged,
            })
        }
        // u XY sub m1 m2 m3 mW h1 h2 h3 path
        "u" => {
            let fields: Vec<&str> = rest.splitn(10, ' ').collect();
            let [_, .., path] = fields.as_slice() else {
                return None;
            };
            Some(FileStatus {
                path: (*path).to_owned(),
                orig_path: None,
                staged: Some(b'U'),
                unstaged: Some(b'U'),
            })
        }
        // ? path
        "?" => Some(FileStatus {
            path: rest.to_owned(),
            orig_path: None,
            staged: None,
            unstaged: Some(b'?'),
        }),
        _ => None,
    }
}

/// The index and worktree letters of a porcelain XY field; `.` means no change.
fn letters(xy: &str) -> (Option<u8>, Option<u8>) {
    let [x, y] = xy.as_bytes() else {
        return (None, None);
    };
    (letter(*x), letter(*y))
}

fn letter(c: u8) -> Option<u8> {
    (c != b'.').then_some(c)
}

/// The patch from the index to the worktree for `path`, with git's prefixes. An
/// untracked file is shown as entirely added.
pub fn unstaged_diff(dir: &Path, path: &str) -> Result<String, GitError> {
    if is_untracked(dir, path)? {
        return untracked_diff(dir, path);
    }
    gitx::run(
        dir,
        &[],
        &[
            "--literal-pathspecs",
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            "--",
            path,
        ],
    )
}

/// Whether git sees `path` as untracked and not ignored.
fn is_untracked(dir: &Path, path: &str) -> Result<bool, GitError> {
    let out = gitx::run(
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
    Ok(!out.trim().is_empty())
}

/// The patch that adds every line of an untracked file, the way git prints a
/// new file. A binary file gets git's notice, and an empty file gets headers
/// only.
fn untracked_diff(dir: &Path, path: &str) -> Result<String, GitError> {
    let bytes = fs::read(dir.join(path)).map_err(|e| GitError {
        args: gitx::owned_args(&["diff", "--", path]),
        stderr: gitx::os_error_text(&e),
    })?;
    let header = format!("diff --git a/{path} b/{path}\nnew file mode 100644\n");
    if bytes.is_empty() {
        return Ok(header);
    }
    if bytes[..bytes.len().min(8000)].contains(&0) {
        return Ok(format!(
            "{header}Binary files /dev/null and b/{path} differ\n"
        ));
    }
    let text = String::from_utf8_lossy(&bytes);
    let body = text.strip_suffix('\n').unwrap_or(&text);
    let mut lines = String::new();
    for line in body.split('\n') {
        lines.push('+');
        lines.push_str(line);
        lines.push('\n');
    }
    let count = body.split('\n').count();
    let no_eol = if text.ends_with('\n') {
        ""
    } else {
        "\\ No newline at end of file\n"
    };
    Ok(format!(
        "{header}--- /dev/null\n+++ b/{path}\n@@ -0,0 +1,{count} @@\n{lines}{no_eol}"
    ))
}

/// The patch from HEAD to the index for `path`, with git's prefixes. Works before the first commit.
pub fn staged_diff(dir: &Path, path: &str) -> Result<String, GitError> {
    gitx::run(
        dir,
        &[],
        &[
            "--literal-pathspecs",
            "diff",
            "--cached",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            "--",
            path,
        ],
    )
}

/// Stages the worktree state of each path, including deletions and untracked files.
pub fn stage(dir: &Path, paths: &[&str]) -> Result<(), GitError> {
    if paths.is_empty() {
        return Ok(());
    }
    let mut args = vec!["--literal-pathspecs", "add", "-A", "--"];
    args.extend_from_slice(paths);
    gitx::run(dir, &[], &args).map(|_| ())
}

/// Moves each path's staged state back to HEAD, or removes it from the index when
/// there is no commit yet. The worktree is not touched.
pub fn unstage(dir: &Path, paths: &[&str]) -> Result<(), GitError> {
    if paths.is_empty() {
        return Ok(());
    }
    let mut args = if has_head(dir) {
        vec!["--literal-pathspecs", "restore", "--staged", "--"]
    } else {
        vec!["--literal-pathspecs", "rm", "--cached", "-r", "-q", "--"]
    };
    args.extend_from_slice(paths);
    gitx::run(dir, &[], &args).map(|_| ())
}

/// Whether HEAD names a commit.
fn has_head(dir: &Path) -> bool {
    gitx::run(dir, &[], &["rev-parse", "--verify", "--quiet", "HEAD"]).is_ok()
}

/// Applies `patch` to the index, or removes it when `reverse` is set. A patch that
/// does not apply leaves the index unchanged.
pub fn apply_to_index(dir: &Path, patch: &str, reverse: bool) -> Result<(), GitError> {
    if patch.trim().is_empty() {
        return Err(GitError {
            args: gitx::owned_args(&["apply", "--cached"]),
            stderr: "nothing to apply".to_owned(),
        });
    }
    let mut check = vec!["apply", "--cached", "--check"];
    let mut apply = vec!["apply", "--cached"];
    if reverse {
        check.push("--reverse");
        apply.push("--reverse");
    }
    check.push("-");
    apply.push("-");
    gitx::run(dir, patch.as_bytes(), &check)?;
    gitx::run(dir, patch.as_bytes(), &apply).map(|_| ())
}

/// Writes the index as a tree object and returns its id.
pub fn write_tree(dir: &Path) -> Result<String, GitError> {
    let out = gitx::run(dir, &[], &["write-tree"])?;
    Ok(out.trim().to_owned())
}

/// Replaces the index with `tree`. The worktree is not touched.
pub fn read_tree(dir: &Path, tree: &str) -> Result<(), GitError> {
    if tree.is_empty() || tree.starts_with('-') {
        return Err(GitError {
            args: gitx::owned_args(&["read-tree", tree]),
            stderr: format!("invalid tree '{tree}'"),
        });
    }
    gitx::run(dir, &[], &["read-tree", tree]).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testrepo::{commit_all, git, repo, write};
    use std::fs;
    use std::process::Command;
    use tempfile::TempDir;

    fn file(path: &str, staged: Option<u8>, unstaged: Option<u8>) -> FileStatus {
        FileStatus {
            path: path.to_owned(),
            orig_path: None,
            staged,
            unstaged,
        }
    }

    #[test]
    fn status_of_clean_repo_is_empty() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "init");
        assert_eq!(status(r.path()).unwrap(), vec![]);
    }

    #[test]
    fn status_reports_modified_unstaged_file() {
        let r = repo();
        write(r.path(), "f.txt", "one\n");
        commit_all(r.path(), "init");
        write(r.path(), "f.txt", "two\n");
        assert_eq!(
            status(r.path()).unwrap(),
            vec![file("f.txt", None, Some(b'M'))]
        );
    }

    #[test]
    fn status_reports_staged_new_file() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "init");
        write(r.path(), "new.txt", "n\n");
        git(r.path(), &["add", "new.txt"]);
        assert_eq!(
            status(r.path()).unwrap(),
            vec![file("new.txt", Some(b'A'), None)]
        );
    }

    #[test]
    fn status_reports_staged_then_modified_again() {
        let r = repo();
        write(r.path(), "f.txt", "one\n");
        commit_all(r.path(), "init");
        write(r.path(), "f.txt", "two\n");
        git(r.path(), &["add", "f.txt"]);
        write(r.path(), "f.txt", "three\n");
        assert_eq!(
            status(r.path()).unwrap(),
            vec![file("f.txt", Some(b'M'), Some(b'M'))]
        );
    }

    #[test]
    fn status_reports_deleted_in_worktree() {
        let r = repo();
        write(r.path(), "f.txt", "one\n");
        commit_all(r.path(), "init");
        fs::remove_file(r.path().join("f.txt")).unwrap();
        assert_eq!(
            status(r.path()).unwrap(),
            vec![file("f.txt", None, Some(b'D'))]
        );
    }

    #[test]
    fn status_lists_untracked_file_in_subdirectory() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "init");
        write(r.path(), "dir/x.txt", "x\n");
        assert_eq!(
            status(r.path()).unwrap(),
            vec![file("dir/x.txt", None, Some(b'?'))]
        );
    }

    #[test]
    fn status_keeps_spaces_in_paths_and_sorts_by_path() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "init");
        write(r.path(), "my notes.txt", "m\n");
        write(r.path(), "dir with space/x.txt", "x\n");
        git(r.path(), &["add", "dir with space/x.txt"]);
        assert_eq!(
            status(r.path()).unwrap(),
            vec![
                file("dir with space/x.txt", Some(b'A'), None),
                file("my notes.txt", None, Some(b'?')),
            ]
        );
    }

    #[test]
    fn status_reports_staged_rename_with_original_path() {
        let r = repo();
        write(r.path(), "old.txt", "same content\nevery line\n");
        commit_all(r.path(), "init");
        git(r.path(), &["mv", "old.txt", "new.txt"]);
        assert_eq!(
            status(r.path()).unwrap(),
            vec![FileStatus {
                path: "new.txt".to_owned(),
                orig_path: Some("old.txt".to_owned()),
                staged: Some(b'R'),
                unstaged: None,
            }]
        );
    }

    #[test]
    fn status_reports_unmerged_path_on_both_sides() {
        let r = repo();
        write(r.path(), "f.txt", "base\n");
        commit_all(r.path(), "init");
        git(r.path(), &["switch", "-q", "-c", "feature"]);
        write(r.path(), "f.txt", "feature\n");
        commit_all(r.path(), "feature");
        git(r.path(), &["switch", "-q", "main"]);
        write(r.path(), "f.txt", "main\n");
        commit_all(r.path(), "main");
        let merge = Command::new("git")
            .arg("-C")
            .arg(r.path())
            .args(["merge", "-q", "feature"])
            .output()
            .unwrap();
        assert!(!merge.status.success());
        assert_eq!(
            status(r.path()).unwrap(),
            vec![file("f.txt", Some(b'U'), Some(b'U'))]
        );
    }

    #[test]
    fn status_of_unborn_repo_with_one_staged_file() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        git(r.path(), &["add", "a.txt"]);
        assert_eq!(
            status(r.path()).unwrap(),
            vec![file("a.txt", Some(b'A'), None)]
        );
    }

    #[test]
    fn unstaged_diff_of_modified_file_has_headers_and_hunk() {
        let r = repo();
        write(r.path(), "f.txt", "1\n2\n3\n");
        commit_all(r.path(), "init");
        write(r.path(), "f.txt", "1\nTWO\n3\n");
        let text = unstaged_diff(r.path(), "f.txt").unwrap();
        assert!(text.starts_with("diff --git a/f.txt b/f.txt\n"), "{text:?}");
        assert!(
            text.ends_with("@@ -1,3 +1,3 @@\n 1\n-2\n+TWO\n 3\n"),
            "{text:?}"
        );
    }

    #[test]
    fn unstaged_diff_of_untracked_file_with_final_newline() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "init");
        write(r.path(), "new.txt", "x\ny\n");
        assert_eq!(
            unstaged_diff(r.path(), "new.txt").unwrap(),
            "diff --git a/new.txt b/new.txt\nnew file mode 100644\n--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1,2 @@\n+x\n+y\n"
        );
    }

    #[test]
    fn unstaged_diff_of_untracked_file_without_final_newline() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "init");
        write(r.path(), "no_eol.txt", "p\nq");
        assert_eq!(
            unstaged_diff(r.path(), "no_eol.txt").unwrap(),
            "diff --git a/no_eol.txt b/no_eol.txt\nnew file mode 100644\n--- /dev/null\n+++ b/no_eol.txt\n@@ -0,0 +1,2 @@\n+p\n+q\n\\ No newline at end of file\n"
        );
    }

    #[test]
    fn unstaged_diff_of_empty_and_binary_untracked_files() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "init");
        write(r.path(), "empty.txt", "");
        write(r.path(), "bin.dat", "a\0b");
        assert_eq!(
            unstaged_diff(r.path(), "empty.txt").unwrap(),
            "diff --git a/empty.txt b/empty.txt\nnew file mode 100644\n"
        );
        assert_eq!(
            unstaged_diff(r.path(), "bin.dat").unwrap(),
            "diff --git a/bin.dat b/bin.dat\nnew file mode 100644\nBinary files /dev/null and b/bin.dat differ\n"
        );
    }

    #[test]
    fn unstaged_diff_ignores_user_prefix_and_color_config() {
        let r = repo();
        write(r.path(), "f.txt", "1\n2\n3\n");
        commit_all(r.path(), "init");
        git(r.path(), &["config", "diff.noprefix", "true"]);
        git(r.path(), &["config", "diff.mnemonicPrefix", "true"]);
        git(r.path(), &["config", "color.diff", "always"]);
        write(r.path(), "f.txt", "1\nTWO\n3\n");
        let text = unstaged_diff(r.path(), "f.txt").unwrap();
        assert!(text.starts_with("diff --git a/f.txt b/f.txt\n"), "{text:?}");
        assert!(!text.contains('\x1b'), "{text:?}");
    }

    #[test]
    fn staged_diff_works_on_unborn_branch() {
        let r = repo();
        write(r.path(), "a.txt", "one\n");
        git(r.path(), &["add", "a.txt"]);
        let text = staged_diff(r.path(), "a.txt").unwrap();
        assert!(
            text.starts_with("diff --git a/a.txt b/a.txt\nnew file mode 100644\n"),
            "{text:?}"
        );
        assert!(text.ends_with("+one\n"), "{text:?}");
    }

    #[test]
    fn stage_then_unstage_with_head() {
        let r = repo();
        write(r.path(), "f.txt", "one\n");
        commit_all(r.path(), "init");
        write(r.path(), "f.txt", "two\n");
        stage(r.path(), &["f.txt"]).unwrap();
        assert_eq!(
            status(r.path()).unwrap(),
            vec![file("f.txt", Some(b'M'), None)]
        );
        unstage(r.path(), &["f.txt"]).unwrap();
        assert_eq!(
            status(r.path()).unwrap(),
            vec![file("f.txt", None, Some(b'M'))]
        );
    }

    #[test]
    fn stage_new_and_deleted_files() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "init");
        fs::remove_file(r.path().join("a.txt")).unwrap();
        write(r.path(), "new.txt", "n\n");
        stage(r.path(), &["a.txt", "new.txt"]).unwrap();
        assert_eq!(
            status(r.path()).unwrap(),
            vec![
                file("a.txt", Some(b'D'), None),
                file("new.txt", Some(b'A'), None),
            ]
        );
    }

    #[test]
    fn unstage_new_file_with_head_leaves_it_untracked() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "init");
        write(r.path(), "new.txt", "n\n");
        stage(r.path(), &["new.txt"]).unwrap();
        unstage(r.path(), &["new.txt"]).unwrap();
        assert_eq!(
            status(r.path()).unwrap(),
            vec![file("new.txt", None, Some(b'?'))]
        );
    }

    #[test]
    fn unstage_without_head_removes_from_index_and_keeps_file() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        stage(r.path(), &["a.txt"]).unwrap();
        unstage(r.path(), &["a.txt"]).unwrap();
        assert_eq!(
            status(r.path()).unwrap(),
            vec![file("a.txt", None, Some(b'?'))]
        );
        assert!(r.path().join("a.txt").exists());
    }

    #[test]
    fn stage_and_unstage_of_no_paths_do_not_run_git() {
        let outside = TempDir::new().unwrap();
        assert!(stage(outside.path(), &[]).is_ok());
        assert!(unstage(outside.path(), &[]).is_ok());
    }

    #[test]
    fn apply_unstaged_diff_stages_the_file_and_reverse_unstages_it() {
        let r = repo();
        write(r.path(), "f.txt", "1\n2\n3\n");
        commit_all(r.path(), "init");
        write(r.path(), "f.txt", "1\nTWO\n3\n");

        let patch = unstaged_diff(r.path(), "f.txt").unwrap();
        apply_to_index(r.path(), &patch, false).unwrap();
        assert_eq!(
            status(r.path()).unwrap(),
            vec![file("f.txt", Some(b'M'), None)]
        );

        let patch = staged_diff(r.path(), "f.txt").unwrap();
        apply_to_index(r.path(), &patch, true).unwrap();
        assert_eq!(
            status(r.path()).unwrap(),
            vec![file("f.txt", None, Some(b'M'))]
        );
    }

    #[test]
    fn apply_untracked_diff_stages_new_file_with_its_bytes() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "init");
        write(r.path(), "plain.txt", "x\ny\n");
        write(r.path(), "no eol.txt", "p\nq");
        write(r.path(), "empty.txt", "");
        for path in ["plain.txt", "no eol.txt", "empty.txt"] {
            let patch = unstaged_diff(r.path(), path).unwrap();
            apply_to_index(r.path(), &patch, false).unwrap_or_else(|e| panic!("{path}: {e}"));
            assert_eq!(
                status(r.path())
                    .unwrap()
                    .into_iter()
                    .find(|f| f.path == path)
                    .map(|f| f.staged),
                Some(Some(b'A')),
                "{path}"
            );
            let on_disk = fs::read(r.path().join(path)).unwrap();
            let in_index = git(r.path(), &["show", &format!(":{path}")]);
            assert_eq!(in_index.as_bytes(), on_disk.as_slice(), "{path}");
        }
    }

    #[test]
    fn apply_rejects_garbage_patch_and_leaves_index_unchanged() {
        let r = repo();
        write(r.path(), "f.txt", "one\n");
        commit_all(r.path(), "init");
        let before = write_tree(r.path()).unwrap();
        assert!(apply_to_index(r.path(), "not a patch\n", false).is_err());
        assert_eq!(write_tree(r.path()).unwrap(), before);
    }

    #[test]
    fn apply_rejects_patch_that_no_longer_applies() {
        let r = repo();
        write(r.path(), "f.txt", "1\n2\n3\n");
        commit_all(r.path(), "init");
        write(r.path(), "f.txt", "1\nTWO\n3\n");
        let patch = unstaged_diff(r.path(), "f.txt").unwrap();
        // Staging the change first makes the patch's preimage stale.
        stage(r.path(), &["f.txt"]).unwrap();
        let before = write_tree(r.path()).unwrap();
        assert!(apply_to_index(r.path(), &patch, false).is_err());
        assert_eq!(write_tree(r.path()).unwrap(), before);
        assert_eq!(
            status(r.path()).unwrap(),
            vec![file("f.txt", Some(b'M'), None)]
        );
    }

    #[test]
    fn apply_empty_patch_is_an_error() {
        let r = repo();
        let err = apply_to_index(r.path(), "", false).unwrap_err();
        assert_eq!(err.stderr, "nothing to apply");
    }

    #[test]
    fn write_tree_then_read_tree_restores_the_index() {
        let r = repo();
        write(r.path(), "a.txt", "one\n");
        commit_all(r.path(), "init");
        let tree = write_tree(r.path()).unwrap();
        assert_eq!(tree.len(), 40);

        write(r.path(), "a.txt", "two\n");
        stage(r.path(), &["a.txt"]).unwrap();
        assert_eq!(
            status(r.path()).unwrap(),
            vec![file("a.txt", Some(b'M'), None)]
        );

        read_tree(r.path(), &tree).unwrap();
        assert_eq!(
            status(r.path()).unwrap(),
            vec![file("a.txt", None, Some(b'M'))]
        );
    }

    #[test]
    fn read_tree_rejects_option_like_tree_without_running_git() {
        let r = repo();
        let err = read_tree(r.path(), "--help").unwrap_err();
        assert_eq!(err.args, vec!["read-tree", "--help"]);
        assert!(read_tree(r.path(), "").is_err());
    }
}
