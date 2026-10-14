//! The action palette (⌘K): a card with one field and the actions it can
//! run. Which actions exist, their order and why one is off are
//! `rvw_view::palette`; this file draws them and runs the highlighted one by
//! calling the shell method the action names.

use gpui::prelude::*;
use gpui::{
    actions, div, px, App, ClickEvent, Context, Div, Entity, KeyBinding, ScrollHandle,
    SharedString, Stateful, Subscription, Window,
};
use rvw_git::merge::Operation;
use rvw_view::changes::Compare;
use rvw_view::keys::Screen;
use rvw_view::palette::{self, Action, Entry, State};
use rvw_view::panels::Side;

use crate::shell::Shell;
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme;
use crate::tools;

actions!(rvw_palette, [OpenPalette]);

/// Binds ⌘K (Ctrl-K off macOS) to open the palette. Call once at startup.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([KeyBinding::new(
        "secondary-k",
        OpenPalette,
        crate::terminal_dock::rvw_chord_context(),
    )]);
}

/// An open palette: its field, the entries for the field's text and which
/// one is highlighted (always a runnable one when there is one).
pub struct Palette {
    input: Entity<TextInput>,
    entries: Vec<Entry>,
    grouped: bool,
    highlighted: usize,
    scroll: ScrollHandle,
    _events: Subscription,
    _typing: Subscription,
}

impl Shell {
    /// Opens the palette, or closes it when it is open. Does nothing while
    /// the composer or the submit dialog is open.
    pub fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tools.palette.is_some() {
            self.close_palette(cx);
            return;
        }
        if self.composer.is_some() || self.submit.is_some() {
            return;
        }
        self.tools.close_all();
        self.finder = None;
        let input = cx.new(|cx| TextInput::new("Type a command", cx));
        let events = cx.subscribe_in(
            &input,
            window,
            |this: &mut Self, _, event: &TextInputEvent, window, cx| match event {
                TextInputEvent::Submit => this.run_highlighted(window, cx),
                TextInputEvent::Cancel => this.close_palette(cx),
            },
        );
        let typing = cx.observe(&input, |this: &mut Self, _, cx: &mut Context<Self>| {
            this.refresh_palette(cx);
        });
        input.read(cx).focus(window);
        self.tools.palette = Some(Palette {
            input,
            entries: Vec::new(),
            grouped: true,
            highlighted: 0,
            scroll: ScrollHandle::new(),
            _events: events,
            _typing: typing,
        });
        self.refresh_palette(cx);
    }

    pub fn close_palette(&mut self, cx: &mut Context<Self>) {
        self.tools.palette = None;
        cx.notify();
    }

    /// The path of the file the screen shows: the review's or the Changes
    /// screen's. `None` on every other screen, or when no file is open.
    fn open_file_path(&self) -> Option<String> {
        match self.screen {
            Screen::Diff | Screen::Files => self.selected.as_ref().map(|s| s.path.clone()),
            Screen::Changes => self.changes.open.as_ref().map(|open| open.path.clone()),
            _ => None,
        }
    }

    fn palette_state(&self) -> State {
        State {
            screen: self.screen,
            has_open_file: self.open_file_path().is_some(),
            has_selected_commit: self.history.history.selected_row().is_some(),
            operation_in_progress: matches!(
                self.git.facts.operation,
                Some(Operation::Merge | Operation::CherryPick)
            ),
            conflicted: self.git.facts.conflicted.len(),
            draft_count: self.snapshot.queue.counters.draft,
            has_selection: self
                .selected
                .as_ref()
                .is_some_and(|selected| !selected.selection.is_empty()),
            has_files: self.has_file_rows(),
            file_has_changes: self
                .selected
                .as_ref()
                .is_some_and(|selected| selected.changed),
            has_staged: !self.changes.lists.staged.is_empty(),
            on_branch: self.git.facts.branch.is_some(),
            has_upstream: self.git.facts.upstream.is_some(),
            has_remote: self.git.facts.remote.is_some(),
            has_stashes: !self.git.facts.stashes.is_empty(),
            has_tags: !self.git.facts.tags.is_empty(),
            has_checkpoints: !self.git.facts.checkpoints.is_empty(),
            network_busy: self.git.busy.is_some(),
        }
    }

    fn refresh_palette(&mut self, cx: &mut Context<Self>) {
        let state = self.palette_state();
        let Some(open) = self.tools.palette.as_mut() else {
            return;
        };
        let query = open.input.read(cx).text().to_owned();
        let found = palette::entries(&query, &state);
        open.grouped = query.is_empty();
        open.entries = if open.grouped {
            palette::grouped(found)
        } else {
            found
        };
        open.highlighted = palette::first_available(&open.entries);
        open.scroll.scroll_to_top_of_item(0);
        cx.notify();
    }

    pub(crate) fn move_palette(&mut self, down: bool, cx: &mut Context<Self>) {
        if let Some(open) = self.tools.palette.as_mut() {
            open.highlighted = palette::step(&open.entries, open.highlighted, down);
            let child = child_index(&open.entries, open.highlighted, open.grouped);
            open.scroll.scroll_to_item(child);
        }
        cx.notify();
    }

    fn run_highlighted(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.tools.palette.as_ref().map(|open| open.highlighted) {
            self.run_entry(index, window, cx);
        }
    }

    /// Runs the action of row `index`, unless it is unavailable. The palette
    /// closes first, so an action that opens a field takes the focus.
    pub fn run_entry(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let action = self
            .tools
            .palette
            .as_ref()
            .and_then(|open| open.entries.get(index))
            .filter(|entry| entry.unavailable.is_none())
            .map(|entry| entry.action);
        if let Some(action) = action {
            self.tools.palette = None;
            self.run_action(action, window, cx);
        }
    }

    /// Calls the shell method each action names.
    fn run_action(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            Action::GoToQueue => self.show(Screen::Queue, cx),
            Action::GoToReview => self.show(Screen::Diff, cx),
            Action::GoToFiles => self.show(Screen::Files, cx),
            Action::FindFile => self.open_finder(window, cx),
            Action::NextFile => self.step_file(1, cx),
            Action::PreviousFile => self.step_file(-1, cx),
            Action::Comment => self.open_composer(window, cx),
            Action::SubmitReview => self.open_submit(window, cx),
            Action::CompareUncommitted => self.set_compare(Compare::Head, cx),
            Action::CompareDefaultBranch => self.set_compare(Compare::Default, cx),
            Action::ComparePreviousCommit => self.set_compare(Compare::Prev, cx),
            Action::SwitchBranch => self.open_branches(window, cx),
            Action::SwitchWorkspace => self.open_workspaces(window, cx),
            Action::ToggleSplit => {
                let split = !self.tools.split;
                self.set_split(split, cx);
            }
            Action::ToggleFullFile => self.toggle_file_mode(cx),
            // A failed re-read keeps the snapshot on screen as it was.
            Action::ReloadQueue => {
                self.reload_for_new_base(cx).ok();
            }
            Action::GoToChanges => self.show(Screen::Changes, cx),
            Action::StageChosen => self.move_chosen(cx),
            Action::Commit => self.commit_changes(cx),
            Action::SaveCheckpoint => self.save_checkpoint(cx),
            Action::RestoreCheckpoint => self.open_checkpoints(window, cx),
            Action::GoToHistory => self.show(Screen::History, cx),
            Action::SearchCommits => {
                self.show(Screen::History, cx);
                self.focus_search(window, cx);
            }
            Action::FileHistory => {
                if let Some(path) = self.open_file_path() {
                    self.show_file_history(&path, cx);
                }
            }
            Action::CherryPick => self.cherry_pick_selected(cx),
            Action::FinishOperation => self.finish_operation(cx),
            Action::AbortOperation => self.abort_operation(cx),
            Action::OpenMergeTool => self.open_merge_tool(cx),
            Action::BlameFile => {
                if let Some(path) = self.open_file_path() {
                    self.blame_file(&path, cx);
                }
            }
            Action::EditHistory => self.open_rewrite(window, cx),
            Action::OpenTerminal => self.open_terminal(cx),
            Action::ToggleTerminalDock => self.toggle_terminal(window, cx),
            Action::NewTerminal => self.open_launcher(window, cx),
            Action::ToggleLeftPanel => self.toggle_panel(Side::Left, cx),
            Action::ToggleRightPanel => self.toggle_panel(Side::Right, cx),
            Action::OpenDiffTool => {
                if let Some(path) = self.open_file_path() {
                    self.open_diff_tool(&path, cx);
                }
            }
            Action::BrowseCommitFiles
            | Action::CompareWith
            | Action::NewWorktree
            | Action::RemoveWorktree
            | Action::StashChanges
            | Action::ApplyStash
            | Action::DropStash
            | Action::NewBranch
            | Action::RenameBranch
            | Action::DeleteBranch
            | Action::MergeBranch
            | Action::NewTag
            | Action::PushTag
            | Action::DeleteTag
            | Action::Fetch
            | Action::Pull
            | Action::Push
            | Action::Publish => self.run_git_action(action, window, cx),
        }
        cx.notify();
    }
}

/// The position of entry `index` among the results' children, which include
/// one header per group when the list is grouped.
fn child_index(entries: &[Entry], index: usize, grouped: bool) -> usize {
    if !grouped {
        return index;
    }
    let mut headers = 0;
    let mut previous = None;
    for entry in entries.iter().take(index + 1) {
        if previous != Some(entry.group) {
            headers += 1;
            previous = Some(entry.group);
        }
    }
    index + headers
}

pub fn overlay(open: &Palette, cx: &Context<Shell>) -> Div {
    let mut rows: Vec<gpui::AnyElement> = Vec::new();
    if open.entries.is_empty() {
        rows.push(
            div()
                .px(px(24.))
                .py(px(16.))
                .text_color(theme::muted())
                .child("No action matches.")
                .into_any_element(),
        );
    }
    let mut previous = None;
    for (index, entry) in open.entries.iter().enumerate() {
        if open.grouped && previous != Some(entry.group) {
            previous = Some(entry.group);
            rows.push(group_header(entry.group).into_any_element());
        }
        rows.push(entry_row(index, entry, index == open.highlighted, cx).into_any_element());
    }
    // The rows are the scrolled element's own children: `scroll_to_item`
    // finds the highlighted one by its index among them.
    let list = div()
        .id("palette-results")
        .flex()
        .flex_col()
        .py(px(6.))
        .max_h(px(560.))
        .overflow_y_scroll()
        .track_scroll(&open.scroll)
        .children(rows);
    tools::overlay(tools::card(
        div().child(open.input.clone()),
        list,
        &["\u{2191}\u{2193} move", "\u{21b5} run", "esc close"],
    ))
}

fn group_header(name: &'static str) -> Div {
    div()
        .h(px(24.))
        .px(px(20.))
        .pt(px(6.))
        .text_size(px(theme::KEY_FONT_SIZE))
        .text_color(theme::faint())
        .child(name)
}

fn entry_row(index: usize, entry: &Entry, highlighted: bool, cx: &Context<Shell>) -> Stateful<Div> {
    let runnable = entry.unavailable.is_none();
    let title = tools::matched_text(entry.title, &entry.matches);
    let mut row = div()
        .id(SharedString::from(format!("action-{index}")))
        .flex()
        .items_center()
        .gap(px(12.))
        .h(px(32.))
        .mx(px(8.))
        .px(px(12.))
        .rounded(theme::ROW_RADIUS)
        .text_color(if runnable {
            theme::text()
        } else {
            theme::faint()
        })
        .when(highlighted && runnable, |d| d.bg(theme::hairline()))
        .child(div().flex_1().min_w(px(0.)).truncate().child(title));
    let right = match (entry.unavailable, entry.chord) {
        (Some(reason), _) => Some((reason.to_owned(), false)),
        (None, Some(chord)) => Some((theme::chord(chord), true)),
        (None, None) => None,
    };
    if let Some((text, mono)) = right {
        row = row.child(
            div()
                .flex_none()
                .text_size(px(theme::KEY_FONT_SIZE))
                .text_color(theme::faint())
                .when(mono, |d| d.font_family(theme::code_font()))
                .child(text),
        );
    }
    if runnable {
        row = row.cursor_pointer().on_click(cx.listener(
            move |this: &mut Shell,
                  _: &ClickEvent,
                  window: &mut Window,
                  cx: &mut Context<Shell>| {
                this.run_entry(index, window, cx);
            },
        ));
    }
    row
}
