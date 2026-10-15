//! The window's root: a sidebar of lanes beside the open lane's `Shell`.
//! Each lane is a worktree, so a workspace of its own, and each keeps its
//! own `Shell` (queue, buffers, terminals) alive while another is shown. A
//! lane's `Shell` is made the first time the lane is opened.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::prelude::*;
use gpui::{App, Context, Entity, FontWeight, KeyBinding, Window, actions, div, px};
use rvw_core::Service;
use rvw_view::lanes::{self, Dot, LaneInput, ProjectInput, Sidebar};
use rvw_view::queue;

use crate::data::{Identity, Snapshot};
use crate::shell::Shell;
use crate::theme;

actions!(rvw_lanes, [PreviousLane, NextLane]);

/// The sidebar's width.
const SIDEBAR_WIDTH: f32 = 208.;

/// ⌥⌘↑ and ⌥⌘↓ move to the previous and the next lane (Ctrl-Alt off macOS,
/// outside the terminal).
pub fn bind_keys(cx: &mut App) {
    let context = crate::terminal_dock::rvw_chord_context();
    cx.bind_keys([
        KeyBinding::new("secondary-alt-up", PreviousLane, context),
        KeyBinding::new("secondary-alt-down", NextLane, context),
    ]);
}

/// The opened lanes (workspace, shell), shared with every lane's `Shell`.
pub(crate) type SharedLanes = std::rc::Rc<std::cell::RefCell<Vec<(String, Entity<Shell>)>>>;

/// What a worktree is called in the sidebar: its branch, or its directory's
/// name when detached.
fn lane_name(tree: &rvw_git::worktree::Worktree) -> String {
    tree.branch.clone().unwrap_or_else(|| {
        tree.path.file_name().map_or_else(
            || "detached".to_owned(),
            |n| n.to_string_lossy().into_owned(),
        )
    })
}

struct OpenLane {
    workspace: String,
    shell: Entity<Shell>,
}

pub struct LaneHost {
    /// Lanes opened so far, in the order they were first opened.
    open: Vec<OpenLane>,
    active: usize,
    /// The repository's worktrees, read when the host starts and whenever a
    /// lane is opened. Each is a lane.
    worktrees: Vec<rvw_git::worktree::Worktree>,
    project: rvw_project::Project,
    shared: SharedLanes,
}

impl LaneHost {
    pub fn new(first: Entity<Shell>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let workspace = first.read(cx).snapshot.workspace.clone();
        let name = std::path::Path::new(&workspace)
            .file_name()
            .map_or_else(|| workspace.clone(), |n| n.to_string_lossy().into_owned());
        let project = rvw_project::implicit(&name, std::path::Path::new(&workspace));
        let worktrees =
            rvw_git::worktree::list(std::path::Path::new(&workspace)).unwrap_or_default();
        let this = cx.entity().downgrade();
        let shared = SharedLanes::default();
        shared.borrow_mut().push((workspace.clone(), first.clone()));
        first.update(cx, |shell, _| {
            shell.host = Some(this.clone());
            shell.inset = SIDEBAR_WIDTH;
            shell.lanes = shared.clone();
        });
        // Closing the window asks about every lane's terminals and edits.
        window.on_window_should_close(cx, move |window, cx| {
            this.update(cx, |host, cx| host.close_allowed(window, cx))
                .unwrap_or(true)
        });
        LaneHost {
            open: vec![OpenLane {
                workspace,
                shell: first,
            }],
            active: 0,
            worktrees,
            project,
            shared,
        }
    }

    pub(crate) fn has(&self, workspace: &str) -> bool {
        self.open.iter().any(|lane| lane.workspace == workspace)
    }

    fn active_shell(&self) -> &Entity<Shell> {
        &self.open[self.active].shell
    }

    /// Shows the opened lane `workspace` and gives it the keys.
    pub(crate) fn show(&mut self, workspace: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self
            .open
            .iter()
            .position(|lane| lane.workspace == workspace)
        {
            self.active = index;
            self.active_shell()
                .update(cx, |shell, _| shell.focus_shell(window));
            cx.notify();
        }
    }

    /// Opens a lane from its loaded snapshot and shows it.
    pub(crate) fn add_lane(
        &mut self,
        snapshot: Snapshot,
        service: Arc<Service>,
        identity: Identity,
        db: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let workspace = identity.workspace.clone();
        if self.has(&workspace) {
            self.show(&workspace, window, cx);
            return;
        }
        let this = cx.entity().downgrade();
        let shell = cx.new(|cx| Shell::new(snapshot, service, identity, db, window, cx));
        shell.update(cx, |shell, _| {
            shell.host = Some(this);
            shell.inset = SIDEBAR_WIDTH;
            shell.lanes = self.shared.clone();
        });
        self.shared
            .borrow_mut()
            .push((workspace.clone(), shell.clone()));
        self.open.push(OpenLane {
            workspace: workspace.clone(),
            shell,
        });
        self.refresh_worktrees();
        self.show(&workspace, window, cx);
    }

    /// Drops the lane at `workspace` after its worktree was removed. The
    /// shown lane is kept when it is another one.
    pub(crate) fn forget(&mut self, workspace: &str, window: &mut Window, cx: &mut Context<Self>) {
        let shown = self.open[self.active].workspace.clone();
        self.open.retain(|lane| lane.workspace != workspace);
        self.shared
            .borrow_mut()
            .retain(|(open, _)| open != workspace);
        if self.open.is_empty() {
            return;
        }
        self.active = self
            .open
            .iter()
            .position(|lane| lane.workspace == shown)
            .unwrap_or(0);
        self.refresh_worktrees();
        self.active_shell()
            .update(cx, |shell, _| shell.focus_shell(window));
        cx.notify();
    }

    /// Opens the new-lane prompt or the archive picker on the shown lane.
    fn ask(
        &mut self,
        action: rvw_view::palette::Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.active_shell()
            .clone()
            .update(cx, |shell, cx| shell.open_worktree_ask(action, window, cx));
    }

    fn refresh_worktrees(&mut self) {
        if let Some(first) = self.open.first()
            && let Ok(list) = rvw_git::worktree::list(std::path::Path::new(&first.workspace))
        {
            self.worktrees = list;
        }
    }

    pub(crate) fn request_quit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.active_shell()
            .clone()
            .update(cx, |shell, cx| shell.request_quit(window, cx));
    }

    fn close_allowed(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.active_shell()
            .clone()
            .update(cx, |shell, cx| shell.close_allowed(window, cx))
    }

    /// The sidebar model: one project (the implicit one), every worktree a
    /// lane. A lane's dot is waiting when its opened shell has a decision
    /// waiting on the reviewer.
    fn sidebar(&self, cx: &App) -> Sidebar {
        let inputs: Vec<LaneInput> = self
            .worktrees
            .iter()
            .map(|tree| {
                let path = tree.path.to_string_lossy().into_owned();
                let waiting = self
                    .open
                    .iter()
                    .find(|lane| lane.workspace == path)
                    .is_some_and(|lane| {
                        queue::waiting_on_you(&lane.shell.read(cx).snapshot.comments) > 0
                    });
                LaneInput {
                    name: lane_name(tree),
                    branch: tree.branch.clone(),
                    path: Some(tree.path.clone()),
                    main: tree.main,
                    waiting,
                    running: false,
                }
            })
            .collect();
        lanes::build(&[ProjectInput {
            project: &self.project,
            lanes: inputs,
            open: true,
        }])
    }

    fn step(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let sidebar = self.sidebar(cx);
        let current_workspace = &self.open[self.active].workspace;
        let current_name = self
            .worktrees
            .iter()
            .find(|tree| tree.path.to_string_lossy() == current_workspace.as_str())
            .map(lane_name);
        let current = current_name
            .as_deref()
            .map(|name| (self.project.name.as_str(), name));
        let Some((_, lane)) = lanes::step(&sidebar, current, delta) else {
            return;
        };
        let Some(path) = self
            .worktrees
            .iter()
            .find(|tree| lane_name(tree) == lane)
            .map(|tree| tree.path.clone())
        else {
            return;
        };
        self.open_path(&path, window, cx);
    }

    /// Shows the lane at `path`, opening it first when it was not.
    fn open_path(&mut self, path: &std::path::Path, window: &mut Window, cx: &mut Context<Self>) {
        let workspace = path.to_string_lossy().into_owned();
        if self.has(&workspace) {
            self.show(&workspace, window, cx);
            return;
        }
        // The loading and the error display are the shell's workspace
        // switch; it hands the loaded lane back to `add_lane`.
        let path = path.to_path_buf();
        self.active_shell()
            .clone()
            .update(cx, |shell, cx| shell.switch_workspace(&path, window, cx));
    }

    /// The sidebar's New and Archive buttons.
    fn sidebar_buttons(cx: &mut Context<Self>) -> gpui::AnyElement {
        let button = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .px(px(8.))
                .h(px(24.))
                .flex()
                .items_center()
                .rounded(theme::CONTROL_RADIUS)
                .text_size(px(12.))
                .text_color(theme::muted())
                .cursor_pointer()
                .hover(|s| s.bg(theme::hairline()))
                .child(label)
        };
        div()
            .flex()
            .gap(px(4.))
            .px(px(8.))
            .pt(px(10.))
            .child(
                button("new-lane", "+ New lane").on_click(cx.listener(|this, _, window, cx| {
                    this.ask(rvw_view::palette::Action::NewWorktree, window, cx);
                })),
            )
            .child(button("archive-lane", "Archive…").on_click(cx.listener(
                |this, _, window, cx| {
                    this.ask(rvw_view::palette::Action::RemoveWorktree, window, cx);
                },
            )))
            .into_any_element()
    }

    fn sidebar_view(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let sidebar = self.sidebar(cx);
        let active_path = self.open[self.active].workspace.clone();
        let mut rows = Vec::new();
        rows.push(Self::sidebar_buttons(cx));
        for group in &sidebar.projects {
            rows.push(
                div()
                    .px(px(14.))
                    .pt(px(14.))
                    .pb(px(6.))
                    .text_size(px(12.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::muted())
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(group.name.clone())
                    .into_any_element(),
            );
            for (index, row) in group.lanes.iter().enumerate() {
                let tree = self
                    .worktrees
                    .iter()
                    .find(|tree| lane_name(tree) == row.name);
                let path = tree.map(|tree| tree.path.clone());
                let selected = path
                    .as_ref()
                    .is_some_and(|p| p.to_string_lossy() == active_path.as_str());
                let dot = match row.dot {
                    Dot::Waiting => Some(theme::needs_you()),
                    Dot::Running => Some(theme::agent()),
                    Dot::Idle => None,
                };
                let label = if row.main {
                    format!("{}  main", row.name)
                } else {
                    row.name.clone()
                };
                rows.push(
                    div()
                        .id(("lane", index))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .mx(px(8.))
                        .pl(px(
                            8. + 12. * f32::from(u16::try_from(row.depth).unwrap_or(0))
                        ))
                        .pr(px(8.))
                        .h(px(30.))
                        .rounded(theme::CONTROL_RADIUS)
                        .text_size(px(13.))
                        .text_color(theme::text())
                        .cursor_pointer()
                        .when(selected, |d| d.bg(theme::selected_panel()))
                        .hover(|s| s.bg(theme::hairline()))
                        .child(
                            div()
                                .size(px(7.))
                                .flex_none()
                                .rounded_full()
                                .when_some(dot, gpui::Styled::bg),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child(label),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            if let Some(path) = path.clone() {
                                this.open_path(&path, window, cx);
                            }
                        }))
                        .into_any_element(),
                );
            }
        }
        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(SIDEBAR_WIDTH))
            .h_full()
            .bg(theme::panel())
            .border_r_1()
            .border_color(theme::hairline())
            .children(rows)
    }
}

impl Render for LaneHost {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let shell = self.active_shell().clone();
        div()
            .size_full()
            .flex()
            .font_family(theme::ui_font())
            .text_size(px(theme::BASE_FONT_SIZE))
            .text_color(theme::text())
            .bg(theme::paper())
            .on_action(cx.listener(|this, _: &PreviousLane, window, cx| this.step(-1, window, cx)))
            .on_action(cx.listener(|this, _: &NextLane, window, cx| this.step(1, window, cx)))
            .child(self.sidebar_view(cx))
            .child(div().flex_1().min_w(px(0.)).h_full().child(shell))
    }
}
