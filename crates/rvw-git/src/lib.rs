//! The handful of git plumbing commands rvw needs, plus path canonicalisation.
//! Port of internal/gitx and internal/workspace. Every call is
//! `git -C <dir> <subcommand> ...` with git taken from `PATH`.

pub mod gitx;
pub mod workspace;

pub use gitx::{Change, GitError, Hunk};
