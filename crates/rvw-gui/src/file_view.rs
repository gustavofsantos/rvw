//! Which of its two views a file opens in: the diff of the change, or the
//! whole file as it is on disk with the changed lines tinted like added ones.
//! The rules are `rvw_view::viewmode`; this file reads the file, remembers the
//! reviewer's choice for the session, and draws the "Changes | File" control.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::sync::Arc;

use gpui::prelude::*;
use gpui::{div, px, ClickEvent, Context, Div, FontWeight, Stateful, Window};
use rvw_view::context::{self, Shown};
use rvw_view::selection::Selection;
use rvw_view::viewmode::{self, ViewMode};

use crate::data::Snapshot;
use crate::review_screen::Selected;
use crate::shell::Shell;
use crate::theme;

/// The largest file read for the whole view.
const MAX_BYTES: u64 = 1 << 20;

/// The bytes of the file at `path`, or what to show instead of them.
fn read(dir: &Path, path: &str) -> Result<Vec<u8>, String> {
    let full = dir.join(path);
    let size = fs::metadata(&full)
        .map_err(|error| format!("Cannot read {path}: {error}"))?
        .len();
    if size > MAX_BYTES {
        return Err("File is larger than 1 MiB, no preview shown.".to_owned());
    }
    fs::read(&full).map_err(|error| format!("Cannot read {path}: {error}"))
}

/// The file on disk as one list of lines. A file the change touches has its
/// added and modified lines marked; any other is plain. An unreadable, large,
/// binary or empty file has a message to show instead of lines.
fn whole(workspace: &str, base_id: &str, path: &str, changed: bool) -> Selected {
    let dir = Path::new(workspace);
    let (diff, cut) = match read(dir, path) {
        Ok(bytes) => {
            let marks = if changed {
                viewmode::marked_lines(dir, path, base_id, &bytes)
            } else {
                BTreeSet::new()
            };
            match context::shown_marked(&bytes, &marks) {
                Shown::Text { diff, cut } => (Ok(Arc::new(diff)), cut),
                Shown::Binary => (Err("Binary file, no preview shown.".to_owned()), false),
                Shown::Empty => (Err("Empty file, nothing to show.".to_owned()), false),
            }
        }
        Err(message) => (Err(message), false),
    };
    Selected {
        path: path.to_owned(),
        diff,
        selection: Selection::default(),
        outside: !changed,
        cut,
        mode: ViewMode::File,
        changed,
    }
}

/// Reads `path` for the centre in `wanted` mode from plain inputs, so it can
/// run off the UI thread. `workspace` is the repository, `base_id` the commit
/// the changes are taken against, and `changed` says the change touches the
/// file. A file the change does not touch is always its content; a deleted or
/// unreadable changed file falls back to its diff.
pub(crate) fn read_selected(
    workspace: &str,
    base_id: &str,
    changed: bool,
    path: &str,
    wanted: ViewMode,
) -> Selected {
    if viewmode::effective(wanted, changed) == ViewMode::Diff {
        return Selected::read(workspace, base_id, path);
    }
    let file = whole(workspace, base_id, path, changed);
    if changed && file.diff.is_err() {
        Selected::read(workspace, base_id, path)
    } else {
        file
    }
}

/// `Shell::load_selected` against `snapshot`, on the UI thread; see
/// `read_selected` for the read itself.
pub(crate) fn load_selected(snapshot: &Snapshot, path: &str, wanted: ViewMode) -> Selected {
    let changed = snapshot.changes.iter().any(|file| file.path == path);
    read_selected(
        &snapshot.workspace,
        &snapshot.base_id,
        changed,
        path,
        wanted,
    )
}

impl Shell {
    /// The mode `path` opens in: the reviewer's last choice for it, else the
    /// default of the screen the click came from.
    pub(crate) fn mode_for(&self, path: &str) -> ViewMode {
        self.file_modes
            .get(path)
            .copied()
            .unwrap_or_else(|| viewmode::default_for(self.screen))
    }

    /// Reads `path` for the centre in `wanted` mode. A file the change does
    /// not touch is always its content; a deleted or unreadable changed file
    /// falls back to its diff.
    pub(crate) fn load_selected(&self, path: &str, wanted: ViewMode) -> Selected {
        load_selected(&self.snapshot, path, wanted)
    }

    /// Shows the file on screen in `mode` and remembers the choice. Does
    /// nothing for a file the change does not touch, which has the one view.
    /// The file on screen is the one asked for last, while its read runs.
    pub fn set_file_mode(&mut self, mode: ViewMode, cx: &mut Context<Self>) {
        let Some((path, shown)) = self.shown_file() else {
            return;
        };
        let changed = self.snapshot.changes.iter().any(|file| file.path == path);
        if shown == mode || !changed {
            return;
        }
        self.request_file(&path, mode, true, cx);
    }

    /// The palette's switch between the whole file and its changes.
    pub fn toggle_file_mode(&mut self, cx: &mut Context<Self>) {
        if let Some((_, mode)) = self.shown_file() {
            self.set_file_mode(mode.other(), cx);
        }
    }
}

/// The "Changes | File" control; `mode` is the segment lit.
pub fn mode_toggle(mode: ViewMode, cx: &Context<Shell>) -> Div {
    div()
        .flex()
        .flex_none()
        .p(px(2.))
        .rounded(px(7.))
        .bg(theme::hairline())
        .font_family(theme::ui_font())
        .child(segment("Changes", "mode-diff", ViewMode::Diff, mode, cx))
        .child(segment("File", "mode-file", ViewMode::File, mode, cx))
}

fn segment(
    label: &'static str,
    id: &'static str,
    to: ViewMode,
    current: ViewMode,
    cx: &Context<Shell>,
) -> Stateful<Div> {
    let active = to == current;
    div()
        .id(id)
        .px(px(10.))
        .py(px(3.))
        .rounded(px(5.))
        .cursor_pointer()
        .when(active, |d| {
            d.bg(theme::paper())
                .text_color(theme::ink())
                .font_weight(FontWeight::MEDIUM)
        })
        .when(!active, |d| {
            d.text_color(theme::muted())
                .hover(|style| style.text_color(theme::ink()))
        })
        .on_click(cx.listener(
            move |this: &mut Shell, _: &ClickEvent, _: &mut Window, cx: &mut Context<Shell>| {
                this.set_file_mode(to, cx);
            },
        ))
        .child(label)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file the change does not touch is read as its content, with no git.
    #[test]
    fn unchanged_file_reads_as_its_content() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("a.txt"), "one\ntwo\n").expect("write");
        let workspace = dir.path().to_str().expect("utf-8 path");
        let selected = read_selected(workspace, "", false, "a.txt", ViewMode::Diff);
        assert_eq!(selected.mode, ViewMode::File);
        assert!(selected.outside);
        assert!(!selected.changed);
        assert!(selected.diff.is_ok());
    }

    /// A file that cannot be read has its reason as the diff's error, not a panic.
    #[test]
    fn missing_file_reads_as_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let workspace = dir.path().to_str().expect("utf-8 path");
        let selected = read_selected(workspace, "", false, "gone.txt", ViewMode::File);
        assert_eq!(selected.mode, ViewMode::File);
        assert!(selected
            .diff
            .is_err_and(|message| message.starts_with("Cannot read gone.txt")));
    }
}
