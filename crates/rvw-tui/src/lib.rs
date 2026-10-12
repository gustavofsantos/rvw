//! rvw's terminal review UI: browse the workspace, read highlighted source,
//! and comment on line ranges. An adapter over `rvw_core::Service`: it holds no
//! domain logic and every action is one service call. Port of internal/tui.

pub mod editor;
pub mod highlight;
pub mod model;
pub mod pages;
pub mod rail;
mod shell;
pub mod style;
#[cfg(test)]
pub(crate) mod testing;
pub mod update;
pub mod view;

/// Moved to `rvw-view`; re-exported so existing paths keep working.
pub use rvw_view::{changes, fuzzy, tree};

use std::sync::Arc;

use rvw_core::Service;

/// A session's configuration. `workspace` is the canonical path an adapter
/// resolved; `lane` and `author` stamp what the session writes.
pub struct Options {
    pub service: Arc<Service>,
    pub workspace: String,
    pub lane: String,
    pub author: String,
    /// The command line comments are written in, run through `sh`.
    pub editor: String,
    /// A syntect theme name; empty uses the terminal's own ANSI colors.
    pub style: String,
    /// The key that starts leader mappings: `space`, or one character.
    pub leader: String,
}

/// Shows the UI on the terminal until the user quits.
pub fn run(opts: Options) -> Result<(), String> {
    shell::run(opts)
}
