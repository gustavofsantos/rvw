//! Worktrees from the window: a new one for a branch, which the window then
//! shows, and removing one. Switching is the workspace picker's job.

use gpui::{Context, Window};
use rvw_view::palette::Action;

use crate::shell::Shell;

impl Shell {
    /// Opens the prompt for a new worktree's branch, or the picker of the
    /// worktrees that can be removed.
    pub(crate) fn open_worktree_ask(
        &mut self,
        _action: Action,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }

    /// Runs the worktree prompt (`typed`) or picker (`label`). Reports in
    /// the overlay on failure and closes it on success.
    pub(crate) fn submit_worktree_ask(
        &mut self,
        _action: Action,
        _typed: &str,
        _label: &str,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }
}
