//! The handful of git plumbing commands rvw needs, plus path canonicalisation.
//! Port of internal/gitx and internal/workspace. Every call is
//! `git -C <dir> <subcommand> ...` with git taken from `PATH`.

// Query helpers return values that are plainly used; `# Errors` is given by
// each error type's doc rather than repeated per function.

pub mod gitx;
pub mod workspace;

pub use gitx::{cancel_flag, os_error_text, Change, GitError, Hunk};
