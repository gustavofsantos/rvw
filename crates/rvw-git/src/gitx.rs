//! Port of internal/gitx/gitx.go. Signatures are fixed; bodies are TODO.

use std::fmt;
use std::path::{Path, PathBuf};

/// A git command that failed. `stderr` is git's trimmed message (or the OS
/// error text when git could not be started).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitError {
    pub args: Vec<String>,
    pub stderr: String,
}

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "git {}: {}", self.args.join(" "), self.stderr)
    }
}

impl std::error::Error for GitError {}

/// One hunk header of a zero-context diff: the old file's lines
/// `old_start..old_start+old_lines-1` became the new file's `new_start..`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hunk {
    pub old_start: i64,
    pub old_lines: i64,
    pub new_start: i64,
    pub new_lines: i64,
}

/// A file that differs between a commit and the worktree. `status` is git's
/// letter (A, M, D, T, U) or `b'?'` for an untracked file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub path: String,
    pub status: u8,
}

/// The root of the worktree holding `dir`, if `dir` is inside one.
pub fn toplevel(_dir: &Path) -> Option<PathBuf> {
    todo!()
}

/// Whether `rel` is in the index of the repository at `dir`. A git failure
/// other than "not tracked" is an error.
pub fn tracked(_dir: &Path, _rel: &str) -> Result<bool, GitError> {
    todo!()
}

/// Writes `content` to the object store and returns its blob id.
pub fn store_content(_dir: &Path, _content: &str) -> Result<String, GitError> {
    todo!()
}

/// Writes the file at `rel` (relative to `dir`) to the object store.
pub fn store_file(_dir: &Path, _rel: &str) -> Result<String, GitError> {
    todo!()
}

/// Reads a blob back, if the repository at `dir` still has it.
pub fn blob(_dir: &Path, _id: &str) -> Option<String> {
    todo!()
}

/// Files git sees in the worktree: tracked plus untracked-not-ignored,
/// relative and slash-separated.
pub fn files(_dir: &Path) -> Result<Vec<String>, GitError> {
    todo!()
}

/// Diffs `path` against commit `base`. The bool is `true` (with no hunks) for
/// an untracked, non-ignored file.
pub fn changes(_dir: &Path, _path: &str, _base: &str) -> Result<(Vec<Hunk>, bool), GitError> {
    todo!()
}

/// What differs between `base` and the worktree, then untracked files last.
pub fn changed_files(_dir: &Path, _base: &str) -> Result<Vec<Change>, GitError> {
    todo!()
}

/// Lines added and deleted between `base` and the worktree (`--numstat`).
pub fn line_changes(_dir: &Path, _base: &str) -> Result<(i64, i64), GitError> {
    todo!()
}

/// Resolves `rev` to a commit id.
pub fn commit(_dir: &Path, _rev: &str) -> Option<String> {
    todo!()
}

/// The best common ancestor of commits `a` and `b`.
pub fn merge_base(_dir: &Path, _a: &str, _b: &str) -> Result<String, GitError> {
    todo!()
}

/// The checked-out branch, `"detached <short id>"`, or `""` when unknown.
pub fn branch(_dir: &Path) -> String {
    todo!()
}

/// The repository's main line, if any.
pub fn default_branch(_dir: &Path) -> Option<String> {
    todo!()
}
