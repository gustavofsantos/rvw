//! The prompts and pickers of the palette's git actions: a name for a new
//! branch or tag, a message for a stash, the stash, branch or tag to act on.
//! One overlay draws both shapes; `git_actions` runs what they ask for.

use gpui::prelude::*;
use gpui::{
    div, px, ClickEvent, Context, Div, Entity, KeyDownEvent, ScrollHandle, SharedString, Stateful,
};
use gpui::{Subscription, Window};
use rvw_git::stash::StashEntry;
use rvw_view::branches::{self, Hit};
use rvw_view::palette::Action;

use crate::shell::Shell;
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme;
use crate::tools;

/// An open prompt or picker.
pub struct GitAsk {
    /// The action the overlay serves; Enter runs it.
    pub(crate) action: Action,
    pub(crate) input: Entity<TextInput>,
    /// A picker's candidates as listed: stash labels, branch or tag names.
    /// Empty for a prompt.
    pub(crate) items: Vec<String>,
    /// The stash entries behind the stash items, at the same indices. Empty
    /// for the other pickers.
    pub(crate) stashes: Vec<StashEntry>,
    /// The items the field's query matches, best first.
    pub(crate) hits: Vec<Hit>,
    /// The row Enter runs: an index into `hits`.
    pub(crate) highlighted: usize,
    /// Why Enter did not run, one line.
    pub(crate) error: Option<String>,
    /// Keeps a picker's highlighted row in view when the arrows move it.
    scroll: ScrollHandle,
    _events: Subscription,
    _typing: Subscription,
}

impl GitAsk {
    /// The index into `items` of the highlighted row, if a row matches.
    pub(crate) fn chosen(&self) -> Option<usize> {
        self.hits.get(self.highlighted).map(|hit| hit.index)
    }
}

/// Whether the action picks an item from a list rather than taking a name.
pub(crate) fn is_picker(action: Action) -> bool {
    matches!(
        action,
        Action::ApplyStash
            | Action::DropStash
            | Action::DeleteBranch
            | Action::DeleteTag
            | Action::PushTag
    )
}

/// A stash as the picker lists it: its index and git's subject.
pub(crate) fn stash_item(entry: &StashEntry) -> String {
    format!("stash@{{{}}}: {}", entry.index, entry.message)
}

/// The field's placeholder for the action's prompt or picker.
fn placeholder(action: Action, branch: &str) -> String {
    match action {
        Action::NewBranch => "Name of the new branch (created here and checked out)".to_owned(),
        Action::RenameBranch => format!("New name for {branch}"),
        Action::NewTag => "Name of the new tag (on this commit)".to_owned(),
        Action::StashChanges => "Message for the stash (optional)".to_owned(),
        Action::ApplyStash => "Apply which stash?".to_owned(),
        Action::DropStash => "Drop which stash?".to_owned(),
        Action::DeleteBranch => "Delete which branch?".to_owned(),
        Action::DeleteTag => "Delete which tag?".to_owned(),
        Action::PushTag => "Push which tag?".to_owned(),
        _ => String::new(),
    }
}

impl Shell {
    /// Opens a prompt or picker with focus in its field, closing any other
    /// picker. `items` and `stashes` are empty for a prompt. A rename starts
    /// with the current branch's name typed in.
    pub(crate) fn open_git_ask(
        &mut self,
        action: Action,
        items: Vec<String>,
        stashes: Vec<StashEntry>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let branch = self.git.facts.branch.clone().unwrap_or_default();
        let initial = if action == Action::RenameBranch {
            branch.clone()
        } else {
            String::new()
        };
        self.tools.close_all();
        self.finder = None;
        self.git.confirm = None;
        let placeholder = placeholder(action, &branch);
        let input = cx.new(|cx| TextInput::new(&placeholder, cx));
        input.update(cx, |input, cx| input.set_text(&initial, cx));
        let events = cx.subscribe_in(
            &input,
            window,
            |this: &mut Self, _, event: &TextInputEvent, _, cx| match event {
                TextInputEvent::Submit => this.submit_git_ask(cx),
                TextInputEvent::Cancel => this.close_git_ask(cx),
            },
        );
        let typing = cx.observe(&input, |this: &mut Self, _, cx: &mut Context<Self>| {
            this.rank_git_ask(cx);
        });
        input.read(cx).focus(window);
        self.git.ask = Some(GitAsk {
            action,
            input,
            items,
            stashes,
            hits: Vec::new(),
            highlighted: 0,
            error: None,
            scroll: ScrollHandle::new(),
            _events: events,
            _typing: typing,
        });
        self.rank_git_ask(cx);
    }

    /// Opens the prompt of `action`: a name or a message.
    pub(crate) fn open_git_prompt(
        &mut self,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_git_ask(action, Vec::new(), Vec::new(), window, cx);
    }

    /// Closes the prompt or picker; closing a closed one does nothing.
    pub(crate) fn close_git_ask(&mut self, cx: &mut Context<Self>) {
        self.git.ask = None;
        cx.notify();
    }

    /// Re-ranks the items against the field's text and resets the highlight.
    fn rank_git_ask(&mut self, cx: &mut Context<Self>) {
        let Some(ask) = self.git.ask.as_mut() else {
            return;
        };
        let query = ask.input.read(cx).text().to_owned();
        ask.hits = branches::rank(&query, &ask.items);
        ask.highlighted = 0;
        ask.error = None;
        ask.scroll.scroll_to_item(0);
        cx.notify();
    }

    /// Moves the highlight of the open picker one row.
    fn move_git_ask(&mut self, down: bool, cx: &mut Context<Self>) {
        if let Some(ask) = self.git.ask.as_mut() {
            let last = ask.hits.len().saturating_sub(1);
            ask.highlighted = if down {
                (ask.highlighted + 1).min(last)
            } else {
                ask.highlighted.saturating_sub(1)
            };
            ask.scroll.scroll_to_item(ask.highlighted);
        }
        cx.notify();
    }

    /// A click on row `row` of the open picker runs that row.
    pub(crate) fn choose_git_row(&mut self, row: usize, cx: &mut Context<Self>) {
        if let Some(ask) = self.git.ask.as_mut() {
            ask.highlighted = row;
        }
        self.submit_git_ask(cx);
    }

    /// The arrows, Enter and Escape while a prompt or picker is open. True
    /// when the key was taken.
    ///
    /// Every key is taken while the overlay is open. The field gets the
    /// letters itself; the shell's own keys (J, K, S, C) must not act on the
    /// review behind the overlay.
    pub(crate) fn git_ask_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let Some(ask) = self.git.ask.as_ref() else {
            return false;
        };
        let picker = is_picker(ask.action);
        match event.keystroke.key.as_str() {
            "down" if picker => self.move_git_ask(true, cx),
            "up" if picker => self.move_git_ask(false, cx),
            "escape" => self.close_git_ask(cx),
            _ => {}
        }
        true
    }
}

/// The open prompt or picker over the window.
pub fn overlay(ask: &GitAsk, cx: &Context<Shell>) -> Div {
    let mut body = div().flex().flex_col().flex_1().min_h(px(0.));
    if let Some(error) = ask.error.as_deref() {
        body = body.child(
            div()
                .px(px(20.))
                .py(px(8.))
                .text_color(theme::rejected())
                .child(error.to_owned()),
        );
    }
    if is_picker(ask.action) {
        body = body.child(picker_list(ask, cx));
    }
    tools::overlay(tools::card(
        div().child(ask.input.clone()),
        body,
        &["\u{21b5} run", "esc close"],
    ))
}

/// The picker's rows, or the line that says why there are none.
fn picker_list(ask: &GitAsk, cx: &Context<Shell>) -> Div {
    if ask.items.is_empty() {
        return message("Nothing to choose from.");
    }
    if ask.hits.is_empty() {
        return message("Nothing matches.");
    }
    // The rows are the scrolled element's own children: `scroll_to_item`
    // finds the highlighted one by its index among them.
    div().flex().flex_col().flex_1().min_h(px(0.)).child(
        div()
            .id("git-picker-results")
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.))
            .py(px(6.))
            .overflow_y_scroll()
            .track_scroll(&ask.scroll)
            .children(
                ask.hits
                    .iter()
                    .enumerate()
                    .map(|(row, hit)| item_row(row, hit, row == ask.highlighted, cx)),
            ),
    )
}

fn message(text: &str) -> Div {
    div()
        .px(px(24.))
        .py(px(16.))
        .text_color(theme::muted())
        .child(text.to_owned())
}

fn item_row(row: usize, hit: &Hit, highlighted: bool, cx: &Context<Shell>) -> Stateful<Div> {
    let name = div()
        .flex_1()
        .min_w(px(0.))
        .truncate()
        .font_family(theme::code_font())
        .text_size(px(theme::CODE_FONT_SIZE))
        .text_color(theme::text())
        .child(tools::matched_text(&hit.name, &hit.matches));
    div()
        .id(SharedString::from(format!("git-ask-{row}")))
        .flex()
        .items_center()
        .h(px(40.))
        .mx(px(8.))
        .px(px(12.))
        .rounded(theme::ROW_RADIUS)
        .when(highlighted, |d| d.bg(theme::hairline()))
        .cursor_pointer()
        .on_click(cx.listener(
            move |this: &mut Shell, _: &ClickEvent, _: &mut Window, cx| {
                this.choose_git_row(row, cx);
            },
        ))
        .child(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stash_is_listed_by_its_index_and_subject() {
        let entry = StashEntry {
            index: 2,
            commit: "abc".to_owned(),
            message: "On main: try the parser".to_owned(),
        };
        assert_eq!(stash_item(&entry), "stash@{2}: On main: try the parser");
    }

    #[test]
    fn only_the_item_actions_are_pickers() {
        assert!(is_picker(Action::ApplyStash));
        assert!(is_picker(Action::PushTag));
        assert!(!is_picker(Action::NewBranch));
        assert!(!is_picker(Action::StashChanges));
    }
}
