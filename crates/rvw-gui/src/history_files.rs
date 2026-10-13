//! The History screen's file tools: blame of a file, a file as it was in the
//! selected commit, and the selected commit compared with a branch or tag.

use gpui::{Context, Window};
use rvw_view::palette::Action;

use crate::shell::Shell;

impl Shell {
    /// Shows the blame of `path` (the file on disk) on the History screen.
    pub fn blame_file(&mut self, _path: &str, _cx: &mut Context<Self>) {}

    /// Opens the picker of a History action: the files of the selected
    /// commit, or the branches and tags to compare it with.
    pub(crate) fn open_history_ask(
        &mut self,
        _action: Action,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }

    /// Runs the History picker's choice `label`. The line to show, or why it
    /// failed (shown in the picker).
    pub(crate) fn submit_history_ask(
        &mut self,
        _action: Action,
        _label: &str,
        _cx: &mut Context<Self>,
    ) -> Result<String, String> {
        Err("Not done yet".to_owned())
    }
}
