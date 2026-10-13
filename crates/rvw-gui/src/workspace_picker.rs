//! The workspace picker (⌘O, or a click on the workspace in the top bar): a
//! card with one field, the workspaces rvw holds a queue for, and "Open a
//! folder…" for any other repository. The order is
//! `rvw_view::workspaces`, the ranking `rvw_view::branches`. Choosing one
//! reads it off the UI thread and then rebuilds the window for it, as a
//! launch in that directory would.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::prelude::*;
use gpui::{
    actions, div, px, AnyElement, App, ClickEvent, Context, Div, Entity, FontWeight, KeyBinding,
    PathPromptOptions, ScrollHandle, SharedString, Stateful, Subscription, Window,
};
use rvw_core::operations::WorkspacesInput;
use rvw_view::branches::{self, Hit};
use rvw_view::workspaces::{self, Item};

use crate::data::{self, Identity};
use crate::seen::Seen;
use crate::shell::Shell;
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme;
use crate::tools;

actions!(rvw_workspace, [OpenWorkspaces]);

/// Binds ⌘O (Ctrl-O off macOS) to open the picker. Call once at startup.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("secondary-o", OpenWorkspaces, None)]);
}

/// An open picker. The workspaces are read once, when it opens.
pub struct WorkspacePicker {
    input: Entity<TextInput>,
    items: Vec<Item>,
    hits: Vec<Hit>,
    /// The row Enter takes: an index into `hits`, or `hits.len()` for
    /// "Open a folder…", which is always the last row.
    highlighted: usize,
    /// Why the last choice did not open, one line.
    error: Option<String>,
    /// The workspace being read, as labelled, while it is read.
    opening: Option<String>,
    /// Keeps the highlighted row in view when the arrows move it.
    scroll: ScrollHandle,
    _events: Subscription,
    _typing: Subscription,
}

/// The home directory, which labels write `~`.
fn home() -> Option<String> {
    std::env::home_dir().map(|home| home.to_string_lossy().into_owned())
}

impl Shell {
    /// Opens the picker with focus in its field, closing any other picker.
    pub fn open_workspaces(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tools.workspaces.is_some() {
            return;
        }
        self.tools.close_all();
        self.finder = None;
        self.git.ask = None;
        // The queue keeps workspaces whose directory is gone; they are left
        // out. Checking for `.git` needs no git process.
        let found: Vec<_> = self
            .service
            .workspaces(WorkspacesInput { all: true })
            .map(|out| out.workspaces)
            .unwrap_or_default()
            .into_iter()
            .filter(|row| Path::new(&row.workspace).join(".git").exists())
            .collect();
        let items = workspaces::items(&found, &self.identity.workspace, home().as_deref());
        let input = cx.new(|cx| TextInput::new("Open a workspace", cx));
        let events = cx.subscribe_in(
            &input,
            window,
            |this: &mut Self, _, event: &TextInputEvent, window, cx| match event {
                TextInputEvent::Submit => this.open_highlighted_workspace(window, cx),
                TextInputEvent::Cancel => this.close_workspaces(cx),
            },
        );
        let typing = cx.observe(&input, |this: &mut Self, _, cx: &mut Context<Self>| {
            this.rank_workspaces(cx);
        });
        input.read(cx).focus(window);
        self.tools.workspaces = Some(WorkspacePicker {
            input,
            items,
            hits: Vec::new(),
            highlighted: 0,
            error: None,
            opening: None,
            scroll: ScrollHandle::new(),
            _events: events,
            _typing: typing,
        });
        self.rank_workspaces(cx);
    }

    pub fn close_workspaces(&mut self, cx: &mut Context<Self>) {
        self.tools.workspaces = None;
        cx.notify();
    }

    fn rank_workspaces(&mut self, cx: &mut Context<Self>) {
        let Some(picker) = self.tools.workspaces.as_mut() else {
            return;
        };
        let query = picker.input.read(cx).text().to_owned();
        let labels: Vec<String> = picker.items.iter().map(|item| item.label.clone()).collect();
        picker.hits = branches::rank(&query, &labels);
        picker.highlighted = 0;
        picker.error = None;
        picker.scroll.scroll_to_item(0);
        cx.notify();
    }

    /// Down or up arrow: moves the highlight, through "Open a folder…" last.
    pub(crate) fn move_workspace(&mut self, down: bool, cx: &mut Context<Self>) {
        if let Some(picker) = self.tools.workspaces.as_mut() {
            let last = picker.hits.len();
            picker.highlighted = if down {
                (picker.highlighted + 1).min(last)
            } else {
                picker.highlighted.saturating_sub(1)
            };
            picker.scroll.scroll_to_item(picker.highlighted);
        }
        cx.notify();
    }

    fn open_highlighted_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(row) = self.tools.workspaces.as_ref().map(|p| p.highlighted) {
            self.open_workspace_row(row, window, cx);
        }
    }

    /// Opens the workspace of row `row`, or the folder dialog for the last
    /// row. The open workspace only closes the picker.
    pub fn open_workspace_row(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(picker) = self.tools.workspaces.as_ref() else {
            return;
        };
        if picker.opening.is_some() {
            return;
        }
        let Some(hit) = picker.hits.get(row) else {
            choose_folder(window, cx);
            return;
        };
        let Some(item) = picker.items.get(hit.index) else {
            return;
        };
        if item.current {
            self.close_workspaces(cx);
            return;
        }
        let path = PathBuf::from(&item.path);
        self.switch_workspace(&path, window, cx);
    }

    /// Reads the repository `dir` is in off the UI thread, then rebuilds the
    /// window for it with the same author and lane. A directory outside git,
    /// or a queue that cannot be read, stays an error in the picker.
    fn switch_workspace(&mut self, dir: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let workspace = match rvw_git::workspace::resolve(dir) {
            Ok(root) => root.to_string_lossy().into_owned(),
            Err(message) => return self.workspace_error(message, cx),
        };
        if workspace == self.identity.workspace {
            self.close_workspaces(cx);
            return;
        }
        if let Some(picker) = self.tools.workspaces.as_mut() {
            picker.opening = Some(workspaces::label(&workspace, home().as_deref()));
            picker.error = None;
        }
        cx.notify();
        let service = Arc::clone(&self.service);
        let db = self.db_path.clone();
        let seen = Seen::load(&db);
        let acknowledged = seen.ids_for(&workspace);
        let identity = Identity {
            workspace: workspace.clone(),
            author: self.identity.author.clone(),
            lane: self.identity.lane.clone(),
        };
        cx.spawn_in(window, async move |this, cx| {
            let reader = Arc::clone(&service);
            let lane = identity.lane.clone();
            let loaded = cx
                .background_executor()
                .spawn(async move { data::load(&reader, &workspace, &lane, acknowledged, None) })
                .await;
            this.update_in(cx, move |this, window, cx| match loaded {
                Ok(snapshot) => {
                    // The new shell replaces this one once this update ends.
                    window.defer(cx, move |window, cx| {
                        window.replace_root(cx, move |window, cx| {
                            Shell::new(snapshot, service, identity, seen, db, window, cx)
                        });
                    });
                }
                Err(message) => this.workspace_error(message, cx),
            })
            .ok();
        })
        .detach();
    }

    fn workspace_error(&mut self, message: String, cx: &mut Context<Self>) {
        if let Some(picker) = self.tools.workspaces.as_mut() {
            picker.opening = None;
            picker.error = Some(message);
        }
        cx.notify();
    }
}

/// Asks the system for a folder, then opens the repository it is in.
fn choose_folder(window: &mut Window, cx: &mut Context<Shell>) {
    let chosen = cx.prompt_for_paths(PathPromptOptions {
        files: false,
        directories: true,
        multiple: false,
        prompt: Some("Open".into()),
    });
    cx.spawn_in(window, async move |this, cx| {
        let Ok(Ok(Some(paths))) = chosen.await else {
            return;
        };
        let Some(dir) = paths.into_iter().next() else {
            return;
        };
        this.update_in(cx, |this, window, cx| {
            this.switch_workspace(&dir, window, cx);
        })
        .ok();
    })
    .detach();
}

pub fn overlay(picker: &WorkspacePicker, cx: &Context<Shell>) -> Div {
    let mut notices = div().flex().flex_col().flex_none();
    if let Some(label) = picker.opening.as_deref() {
        notices = notices.child(notice(format!("Opening {label}…"), theme::muted()));
    }
    if let Some(error) = picker.error.as_deref() {
        notices = notices.child(notice(error.to_owned(), theme::rejected()));
    }
    if picker.hits.is_empty() {
        notices = notices.child(notice("No workspace matches.".to_owned(), theme::muted()));
    }
    let mut rows: Vec<AnyElement> = Vec::new();
    for (row, hit) in picker.hits.iter().enumerate() {
        if let Some(item) = picker.items.get(hit.index) {
            let highlighted = row == picker.highlighted;
            rows.push(workspace_row(row, hit, item, highlighted, cx).into_any_element());
        }
    }
    let folder_row = picker.hits.len();
    rows.push(folder_row_view(folder_row, picker.highlighted == folder_row, cx).into_any_element());
    // The rows are the scrolled element's own children: `scroll_to_item`
    // finds the highlighted one by its index among them.
    let list = div()
        .id("workspace-results")
        .flex()
        .flex_col()
        .flex_1()
        .min_h(px(0.))
        .py(px(6.))
        .overflow_y_scroll()
        .track_scroll(&picker.scroll)
        .children(rows);
    tools::overlay(tools::card(
        div().child(picker.input.clone()),
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.))
            .child(notices)
            .child(list),
        &["\u{2191}\u{2193} move", "\u{21b5} open", "esc close"],
    ))
}

fn notice(text: String, color: gpui::Rgba) -> Div {
    div().px(px(20.)).py(px(8.)).text_color(color).child(text)
}

fn workspace_row(
    row: usize,
    hit: &Hit,
    item: &Item,
    highlighted: bool,
    cx: &Context<Shell>,
) -> Stateful<Div> {
    let name = tools::matched_text(&hit.name, &hit.matches)
        .flex_none()
        .font_family(theme::code_font())
        .text_size(px(theme::CODE_FONT_SIZE))
        .text_color(theme::text());
    let mut label = div()
        .flex()
        .items_center()
        .gap(px(8.))
        .flex_1()
        .min_w(px(0.))
        .child(name);
    if item.current {
        label = label.child(
            div()
                .flex_none()
                .text_size(px(theme::KEY_FONT_SIZE))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme::done())
                .child("open"),
        );
    }
    let mut counts = div()
        .flex()
        .flex_none()
        .gap(px(12.))
        .text_size(px(theme::KEY_FONT_SIZE));
    if item.pending > 0 {
        counts = counts.child(
            div()
                .text_color(theme::pending())
                .child(format!("○ {} pending", item.pending)),
        );
    }
    if item.pulled > 0 {
        counts = counts.child(
            div()
                .text_color(theme::agent())
                .child(format!("◐ {} with the agent", item.pulled)),
        );
    }
    row_frame(SharedString::from(format!("workspace-{row}")), highlighted)
        .on_click(cx.listener(
            move |this: &mut Shell,
                  _: &ClickEvent,
                  window: &mut Window,
                  cx: &mut Context<Shell>| {
                this.open_workspace_row(row, window, cx);
            },
        ))
        .child(label)
        .child(counts)
}

fn folder_row_view(row: usize, highlighted: bool, cx: &Context<Shell>) -> Stateful<Div> {
    row_frame(SharedString::from("workspace-folder"), highlighted)
        .on_click(cx.listener(
            move |this: &mut Shell,
                  _: &ClickEvent,
                  window: &mut Window,
                  cx: &mut Context<Shell>| {
                this.open_workspace_row(row, window, cx);
            },
        ))
        .text_color(theme::muted())
        .child("Open a folder…")
}

fn row_frame(id: SharedString, highlighted: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .gap(px(12.))
        .h(px(40.))
        .mx(px(8.))
        .px(px(12.))
        .rounded(theme::ROW_RADIUS)
        .when(highlighted, |d| d.bg(theme::hairline()))
        .cursor_pointer()
}
