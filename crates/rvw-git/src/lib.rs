//! The handful of git plumbing commands rvw needs, plus path canonicalisation.
//! Port of internal/gitx and internal/workspace. Every call is
//! `git -C <dir> <subcommand> ...` with git taken from `PATH`.

// Query helpers return values that are plainly used; `# Errors` is given by
// each error type's doc rather than repeated per function.

pub mod blame;
pub mod checkpoint;
pub mod commits;
pub mod gitx;
pub mod history;
pub mod index;
pub mod merge;
pub mod patch_id;
pub mod refs;
pub mod remote;
pub mod rewrite;
pub mod search;
pub mod shell_env;
pub mod stash;
pub mod workspace;
pub mod worktree;

#[cfg(test)]
pub(crate) mod testrepo;

pub use gitx::{
    Change, GitError, GitLog, Hunk, cancel_flag, os_error_text, set_logger, set_shell_env,
};
