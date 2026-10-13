//! The Files tab: the whole repository as a tree of folders and files. A
//! folder opens and closes; a file is selected and opens whole (`file_view`).
//! The listing and the marks come from `rvw_view`; this file lays them out.

use std::ops::Range;
use std::path::Path;

use gpui::prelude::*;
use gpui::{
    div, px, uniform_list, AnyElement, ClickEvent, Context, Div, FontWeight, Pixels,
    ScrollStrategy, SharedString, UniformListScrollHandle, Window,
};
use rvw_view::files::Marks;
use rvw_view::keys::Screen;
use rvw_view::nav;
use rvw_view::tree::{self, build_tree, NodeId, Row, Tree};

use crate::review_screen;
use crate::shell::Shell;
use crate::theme;

/// The repository's files, and the tree they make with the folders' open state.
pub struct FileList {
    /// Every file, sorted; what the finder searches.
    pub paths: Vec<String>,
    pub tree: Tree,
    /// The tree's visible rows, kept so a frame does not walk the tree; set
    /// again whenever a folder opens or closes.
    pub rows: Vec<Row>,
}

/// Lists the files of the repository at `workspace`.
///
/// # Errors
///
/// Fails with git's message when the listing cannot be read.
pub fn load_listing(workspace: &str) -> Result<FileList, String> {
    let paths = tree::list_files(Path::new(workspace)).map_err(|e| e.to_string())?;
    let tree = build_tree(&paths);
    let rows = tree.rows();
    Ok(FileList { paths, tree, rows })
}

/// The left panel on the Files tab: the tree of the whole repository, or why
/// it cannot be listed.
pub fn left(
    listing: Option<&Result<FileList, String>>,
    scroll: &UniformListScrollHandle,
    cx: &Context<Shell>,
) -> Div {
    let (summary, body): (String, AnyElement) = match listing {
        Some(Ok(list)) => {
            let count = list.paths.len();
            let summary = format!(
                "{count} {} in the repository",
                crate::queue_screen::noun(count, "file", "files")
            );
            // Only the rows in view are built, from the shell's current state.
            let rows = uniform_list(
                "file-tree",
                list.rows.len(),
                cx.processor(
                    |this: &mut Shell,
                     range: Range<usize>,
                     _: &mut Window,
                     cx: &mut Context<Shell>| {
                        let Some(Ok(list)) = this.listing.as_ref() else {
                            return Vec::new();
                        };
                        let marks = Marks::new(&this.snapshot.changes, &this.snapshot.comments);
                        let selected = this.selected.as_ref().map(|s| s.path.as_str());
                        list.rows
                            .get(range)
                            .unwrap_or_default()
                            .iter()
                            .map(|row| tree_row(&list.tree, *row, &marks, selected, cx))
                            .collect()
                    },
                ),
            )
            .track_scroll(scroll.clone())
            .flex_1()
            .min_h(px(0.));
            (summary, rows.into_any_element())
        }
        Some(Err(message)) => (
            "Whole repository".to_owned(),
            review_screen::notice(message).into_any_element(),
        ),
        None => (
            "Whole repository".to_owned(),
            review_screen::notice("Listing the repository...").into_any_element(),
        ),
    };
    review_screen::panel(Screen::Files, &summary, body, cx)
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
                    .font_family(theme::code_font())
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
                .font_family(theme::code_font())
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
            list.rows = list.tree.rows();
        }
        cx.notify();
    }

    /// Lists the repository the first time a view needs it.
    pub fn ensure_listing(&mut self) {
        if self.listing.is_none() {
            self.listing = Some(load_listing(&self.snapshot.workspace));
        }
    }

    /// J and K, and the arrow keys: selects the next or previous file of the
    /// list on screen, as a click on it would, and brings its row into view.
    /// Does nothing on a screen without a file list.
    pub fn step_file(&mut self, delta: i32, cx: &mut Context<Self>) {
        let rows = self.file_rows();
        let paths: Vec<&str> = rows.iter().map(|(_, path)| path.as_str()).collect();
        let current = self
            .selected
            .as_ref()
            .map(|selected| selected.path.as_str());
        let Some(index) = nav::file_step(&paths, current, delta) else {
            return;
        };
        let (row, path) = &rows[index];
        self.select_file(path, cx);
        let scroll = if self.screen == Screen::Files {
            &self.files_scroll
        } else {
            &self.changes_scroll
        };
        scroll.scroll_to_item(*row, ScrollStrategy::Center);
    }

    /// Whether a keyboard step has any file to reach.
    pub(crate) fn has_file_rows(&self) -> bool {
        !self.file_rows().is_empty()
    }

    /// The files a keyboard step can reach, in display order, each with its
    /// row's index in the rendered list.
    fn file_rows(&self) -> Vec<(usize, String)> {
        match (self.screen, self.listing.as_ref()) {
            (Screen::Diff, _) => self
                .snapshot
                .changes
                .iter()
                .enumerate()
                .map(|(index, file)| (index, file.path.clone()))
                .collect(),
            (Screen::Files, Some(Ok(list))) => list
                .rows
                .iter()
                .enumerate()
                .filter_map(|(index, row)| {
                    let node = list.tree.node(row.node);
                    (!node.dir).then(|| (index, node.path.clone()))
                })
                .collect(),
            _ => Vec::new(),
        }
    }
}
