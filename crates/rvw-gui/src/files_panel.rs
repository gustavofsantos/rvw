//! The Files tab: the whole repository as a tree of folders and files. A
//! folder opens and closes; a file is selected, and a file outside the change
//! opens as plain content. The listing and the marks come from `rvw_view`;
//! this file lays them out and reads the one file the centre shows.

use std::fs;
use std::path::Path;

use gpui::prelude::*;
use gpui::{
    div, px, AnyElement, ClickEvent, Context, Div, FontWeight, Pixels, SharedString, Window,
};
use rvw_view::context::{self, Shown};
use rvw_view::files::Marks;
use rvw_view::keys::Screen;
use rvw_view::selection::Selection;
use rvw_view::tree::{self, build_tree, NodeId, Row, Tree};

use crate::data::Snapshot;
use crate::review_screen::{self, Selected};
use crate::shell::Shell;
use crate::theme;

/// The repository's files, and the tree they make with the folders' open state.
pub struct FileList {
    /// Every file, sorted; what the finder searches.
    pub paths: Vec<String>,
    pub tree: Tree,
}

/// Lists the files of the repository at `workspace`.
///
/// # Errors
///
/// Fails with git's message when the listing cannot be read.
pub fn load_listing(workspace: &str) -> Result<FileList, String> {
    let paths = tree::list_files(Path::new(workspace)).map_err(|e| e.to_string())?;
    let tree = build_tree(&paths);
    Ok(FileList { paths, tree })
}

/// The file at `path` as it is on disk, for a file the change does not touch.
/// An unreadable, binary or empty file has an error to show instead of lines.
pub fn load_outside(workspace: &str, path: &str) -> Selected {
    let (diff, cut) = match fs::read(Path::new(workspace).join(path)) {
        Ok(bytes) => match context::shown(&bytes) {
            Shown::Text { diff, cut } => (Ok(diff), cut),
            Shown::Binary => (Err("Binary file, no preview shown.".to_owned()), false),
            Shown::Empty => (Err("Empty file, nothing to show.".to_owned()), false),
        },
        Err(error) => (Err(format!("Cannot read {path}: {error}")), false),
    };
    Selected {
        path: path.to_owned(),
        diff,
        selection: Selection::default(),
        outside: true,
        cut,
    }
}

/// The left panel on the Files tab: the tree of the whole repository, or why
/// it cannot be listed.
pub fn left(
    snapshot: &Snapshot,
    listing: Option<&Result<FileList, String>>,
    selected: Option<&str>,
    cx: &Context<Shell>,
) -> Div {
    let (summary, rows): (String, Vec<AnyElement>) = match listing {
        Some(Ok(list)) => {
            let marks = Marks::new(&snapshot.changes, &snapshot.comments);
            let count = list.paths.len();
            let summary = format!(
                "{count} {} in the repository",
                crate::queue_screen::noun(count, "file", "files")
            );
            let rows = list
                .tree
                .rows()
                .into_iter()
                .map(|row| tree_row(&list.tree, row, &marks, selected, cx))
                .collect();
            (summary, rows)
        }
        Some(Err(message)) => (
            "Whole repository".to_owned(),
            vec![review_screen::notice(message).into_any_element()],
        ),
        None => (
            "Whole repository".to_owned(),
            vec![review_screen::notice("Listing the repository...").into_any_element()],
        ),
    };
    review_screen::panel(
        Screen::Files,
        &summary,
        review_screen::list("file-tree", rows),
        cx,
    )
}

/// One visible row: a folder with its chevron, or a file with its change dot
/// and open comment count.
fn tree_row(
    tree: &Tree,
    row: Row,
    marks: &Marks,
    selected: Option<&str>,
    cx: &Context<Shell>,
) -> AnyElement {
    let node = tree.node(row.node);
    let id = SharedString::from(format!("tree-{}", node.path));
    if node.dir {
        let on_click = cx.listener(
            move |this: &mut Shell, _: &ClickEvent, _: &mut Window, cx: &mut Context<Shell>| {
                this.toggle_dir(row.node, cx);
            },
        );
        return div()
            .id(id)
            .flex()
            .items_center()
            .gap(px(6.))
            .h(px(28.))
            .mx(px(8.))
            .pl(indent(row.depth, 0.))
            .pr(px(12.))
            .rounded(theme::ROW_RADIUS)
            .text_color(theme::text())
            .on_click(on_click)
            .child(
                div()
                    .w(px(10.))
                    .flex_none()
                    .text_size(px(10.))
                    .text_color(theme::faint())
                    .child(if node.expanded { "▾" } else { "▸" }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .truncate()
                    .font_family(theme::CODE_FONT)
                    .text_size(px(12.))
                    .child(node.name.clone()),
            )
            .into_any_element();
    }

    let mark = marks.get(&node.path);
    let is_selected = selected == Some(node.path.as_str());
    let path = node.path.clone();
    let on_click = cx.listener(
        move |this: &mut Shell, _: &ClickEvent, _: &mut Window, cx: &mut Context<Shell>| {
            this.select_file(&path, cx);
        },
    );
    let dot = div()
        .size(px(6.))
        .flex_none()
        .rounded_full()
        .when(mark.changed, |d| d.bg(theme::pending()));
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(8.))
        .h(px(28.))
        .mx(px(8.))
        .pl(indent(row.depth, 16.))
        .pr(px(12.))
        .rounded(theme::ROW_RADIUS)
        .text_color(theme::text())
        .when(is_selected, |d| {
            d.bg(theme::selected_panel())
                .text_color(theme::ink())
                .font_weight(FontWeight::MEDIUM)
        })
        .on_click(on_click)
        .child(dot)
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .font_family(theme::CODE_FONT)
                .text_size(px(12.))
                .child(node.name.clone()),
        )
        .when(mark.open_comments > 0, |d| {
            d.child(
                div()
                    .flex_none()
                    .text_color(theme::pending())
                    .child(format!("○ {}", mark.open_comments)),
            )
        })
        .into_any_element()
}

/// The left indent of a row: 14px per folder above it, then `extra` for a file
/// so its name lines up past the folder chevrons.
fn indent(depth: usize, extra: f32) -> Pixels {
    let levels = f32::from(u16::try_from(depth).unwrap_or(u16::MAX));
    px(12. + 14. * levels + extra)
}

impl Shell {
    /// Opens or closes a folder of the tree.
    pub fn toggle_dir(&mut self, id: NodeId, cx: &mut Context<Self>) {
        if let Some(Ok(list)) = self.listing.as_mut() {
            let open = list.tree.node(id).expanded;
            list.tree.set_expanded(id, !open);
        }
        cx.notify();
    }

    /// Lists the repository the first time a view needs it.
    pub fn ensure_listing(&mut self) {
        if self.listing.is_none() {
            self.listing = Some(load_listing(&self.snapshot.workspace));
        }
    }
}
