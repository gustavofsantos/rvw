//! The narrow history edit (B14): a card over the window listing the
//! unpushed commits, where each can be kept, reworded, squashed into the one
//! below or dropped; then the before and after, and Apply.

use gpui::prelude::*;
use gpui::{
    AnyElement, Context, Div, Entity, FontWeight, KeyDownEvent, ScrollHandle, Subscription, Window,
    div, px,
};
use rvw_git::history::{self, LogEntry};
use rvw_git::rewrite::{self, Action, End, Planned};
use rvw_view::keys::Screen;

use crate::git_tools::last_line;
use crate::icon::{self, Icon};
use crate::queue_screen::noun;
use crate::shell::Shell;
use crate::text_area::{TextArea, TextAreaEvent};
use crate::theme;
use crate::tools;

/// How many unpushed commits the card lists.
const LIMIT: usize = 50;

const HINT_CHOOSE: &str =
    "J K move · R reword · S squash · D drop · U keep · ↵ preview · esc close";
const HINT_EDITOR: &str = "⌘↵ save message · esc cancel";
const HINT_CONFIRM: &str = "↵ apply · esc back";

/// The open card.
pub struct RewriteCard {
    branch: String,
    commits: Vec<LogEntry>,
    end: End,
    /// One per commit, newest first.
    choices: Vec<Choice>,
    highlighted: usize,
    /// The message being written for row `.0`, the field that holds it, and
    /// the subscription that hears its Submit and Cancel.
    editor: Option<(usize, Entity<TextArea>, Subscription)>,
    /// The open editor writes a squash of its row, not a reword.
    squash: bool,
    stage: Stage,
    error: Option<String>,
    scroll: ScrollHandle,
}

/// What the card does with one commit.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Choice {
    Keep,
    Reword(String),
    /// Folds the commit into the one below, which takes this message.
    Squash(String),
    Drop,
}

/// The card's two stages: choosing, and the confirmation of the planned history.
enum Stage {
    Choose,
    Confirm(Planned),
}

impl RewriteCard {
    /// A message field of the card has the keyboard, so the shell must not
    /// take the focus back.
    pub fn editing(&self) -> bool {
        self.editor.is_some()
    }
}

impl Shell {
    /// Opens the card on the current branch's unpushed commits, every row kept.
    /// When git cannot list them, or there is none to edit, the reason goes to
    /// the status line and no card opens.
    pub fn open_rewrite(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let editable = match rewrite::editable(self.repo_path(), LIMIT) {
            Ok(editable) => editable,
            Err(e) => {
                self.git.network_result = Some(last_line(&e.stderr));
                cx.notify();
                return;
            }
        };
        if editable.commits.is_empty() {
            self.git.network_result = Some(format!(
                "No unpushed commits to edit on {}",
                editable.branch
            ));
            cx.notify();
            return;
        }
        self.tools.close_all();
        self.finder = None;
        self.git.ask = None;
        self.git.confirm = None;
        self.rewrite = Some(RewriteCard {
            choices: vec![Choice::Keep; editable.commits.len()],
            branch: editable.branch,
            commits: editable.commits,
            end: editable.end,
            highlighted: 0,
            editor: None,
            squash: false,
            stage: Stage::Choose,
            error: None,
            scroll: ScrollHandle::new(),
        });
        self.focus_shell(window);
        cx.notify();
    }

    /// The keys while the card is open; true when the key was taken, which is
    /// every key, since the card is modal. While a message field is open the
    /// shell takes no key: the field gets the typed text, and its Esc and ⌘↵
    /// arrive as its own events.
    pub(crate) fn rewrite_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(card) = self.rewrite.as_ref() else {
            return false;
        };
        if card.editing() {
            return true;
        }
        if event.keystroke.modifiers.number_of_modifiers() != 0 {
            return true;
        }
        let key = event.keystroke.key.as_str();
        if matches!(card.stage, Stage::Confirm(_)) {
            self.confirm_key(key, cx);
        } else {
            self.choose_key(key, window, cx);
        }
        true
    }

    /// The keys of the choosing stage, with no field open.
    fn choose_key(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(card) = self.rewrite.as_mut() {
            card.error = None;
        }
        match key {
            "j" | "down" => self.move_row(true, cx),
            "k" | "up" => self.move_row(false, cx),
            "r" => self.start_reword(window, cx),
            "s" => self.start_squash(window, cx),
            "d" => self.toggle_drop(cx),
            "u" => self.keep_row(cx),
            "enter" => self.preview_rewrite(cx),
            "escape" => {
                self.rewrite = None;
                cx.notify();
            }
            _ => {}
        }
    }

    /// The keys of the confirmation: Enter applies, Esc goes back to the choices.
    fn confirm_key(&mut self, key: &str, cx: &mut Context<Self>) {
        match key {
            "enter" => self.apply_rewrite(cx),
            "escape" => {
                if let Some(card) = self.rewrite.as_mut() {
                    card.stage = Stage::Choose;
                    card.error = None;
                }
                cx.notify();
            }
            _ => {}
        }
    }

    /// Moves the highlight one row, stopping at either end, and scrolls to it.
    fn move_row(&mut self, down: bool, cx: &mut Context<Self>) {
        let Some(card) = self.rewrite.as_mut() else {
            return;
        };
        let last = card.commits.len().saturating_sub(1);
        card.highlighted = if down {
            (card.highlighted + 1).min(last)
        } else {
            card.highlighted.saturating_sub(1)
        };
        card.scroll.scroll_to_item(card.highlighted);
        cx.notify();
    }

    /// `r`: opens the message field for the highlighted commit, prefilled with
    /// its full message.
    fn start_reword(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((index, id)) = self.highlighted_commit() else {
            return;
        };
        match history::show(self.repo_path(), &id) {
            Ok(detail) => self.open_editor(index, &detail.message, false, window, cx),
            Err(e) => self.card_error(last_line(&e.stderr), cx),
        }
    }

    /// `s`: opens the message field for a squash of the highlighted commit into
    /// the one below. It starts from the lower commit's message, a blank line,
    /// then this commit's message.
    fn start_squash(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((index, id)) = self.highlighted_commit() else {
            return;
        };
        let Some(below) = squash_target(index, self.commit_count()) else {
            self.card_error(
                "The oldest commit has nothing below it to squash into".to_owned(),
                cx,
            );
            return;
        };
        // A message already chosen for the lower commit is the one it will
        // have, so the squash starts from that.
        let pending = self
            .rewrite
            .as_ref()
            .and_then(|card| match card.choices.get(below) {
                Some(Choice::Reword(text) | Choice::Squash(text)) => Some(text.clone()),
                _ => None,
            });
        let Some(below) = self.commit_id(below) else {
            return;
        };
        let newer = history::show(self.repo_path(), &id);
        let older = match pending {
            Some(text) => Ok(text),
            None => history::show(self.repo_path(), &below).map(|detail| detail.message),
        };
        match (older, newer) {
            (Ok(older), Ok(newer)) => {
                let text = squash_prefill(&older, &newer.message);
                self.open_editor(index, &text, true, window, cx);
            }
            (Err(e), _) | (_, Err(e)) => self.card_error(last_line(&e.stderr), cx),
        }
    }

    /// Opens the message field for row `index`, filled with `text`, and gives
    /// it the keyboard. `squash` says what Submit writes.
    fn open_editor(
        &mut self,
        index: usize,
        text: &str,
        squash: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let area = cx.new(|cx| TextArea::new("Commit message", cx).with_lines(3, 10));
        area.update(cx, |area, cx| area.set_text(text, cx));
        area.update(cx, |area, cx| area.focus(window, cx));
        let events = cx.subscribe(
            &area,
            |this: &mut Self, area: Entity<TextArea>, event: &TextAreaEvent, cx| {
                this.editor_event(&area, *event, cx);
            },
        );
        if let Some(card) = self.rewrite.as_mut() {
            card.editor = Some((index, area, events));
            card.squash = squash;
            card.error = None;
        }
        cx.notify();
    }

    /// Submit saves the field's text for its row; Cancel drops the field.
    fn editor_event(
        &mut self,
        area: &Entity<TextArea>,
        event: TextAreaEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            TextAreaEvent::Submit => self.save_editor(area, cx),
            TextAreaEvent::Cancel => {
                if let Some(card) = self.rewrite.as_mut() {
                    card.editor = None;
                }
                cx.notify();
            }
        }
    }

    /// Saves the field's trimmed text as the row's reword or squash and closes
    /// the field. Empty text is refused and the field stays open.
    fn save_editor(&mut self, area: &Entity<TextArea>, cx: &mut Context<Self>) {
        let text = area.read(cx).text().trim().to_owned();
        let Some(card) = self.rewrite.as_mut() else {
            return;
        };
        let Some((index, open, _)) = card.editor.as_ref() else {
            return;
        };
        if open.entity_id() != area.entity_id() {
            return;
        }
        let index = *index;
        if text.is_empty() {
            card.error = Some("A commit needs a message".to_owned());
        } else {
            card.choices[index] = if card.squash {
                Choice::Squash(text)
            } else {
                Choice::Reword(text)
            };
            card.editor = None;
            card.error = None;
        }
        cx.notify();
    }

    /// `d`: drops the highlighted commit, or keeps it again when it is dropped.
    fn toggle_drop(&mut self, cx: &mut Context<Self>) {
        let Some(card) = self.rewrite.as_mut() else {
            return;
        };
        let index = card.highlighted;
        if card.commits[index].parents.is_empty() {
            card.error = Some("The first commit cannot be dropped".to_owned());
        } else if card.choices[index] == Choice::Drop {
            card.choices[index] = Choice::Keep;
        } else {
            card.choices[index] = Choice::Drop;
        }
        cx.notify();
    }

    /// `u`: keeps the highlighted commit as it is.
    fn keep_row(&mut self, cx: &mut Context<Self>) {
        if let Some(card) = self.rewrite.as_mut() {
            card.choices[card.highlighted] = Choice::Keep;
        }
        cx.notify();
    }

    /// Enter: plans the chosen edits and shows the confirmation. Nothing is
    /// changed yet; a plan that fails stays on the choices with its reason.
    fn preview_rewrite(&mut self, cx: &mut Context<Self>) {
        let Some(card) = self.rewrite.as_ref() else {
            return;
        };
        let edits = edits_of(&card.commits, &card.choices);
        let result = if edits.is_empty() {
            Err("Choose something to change first".to_owned())
        } else {
            rewrite::plan(self.repo_path(), &edits).map_err(|e| last_line(&e.stderr))
        };
        if let Some(card) = self.rewrite.as_mut() {
            match result {
                Ok(planned) => {
                    card.stage = Stage::Confirm(planned);
                    card.error = None;
                }
                Err(message) => card.error = Some(message),
            }
        }
        cx.notify();
    }

    /// Enter in the confirmation: saves a checkpoint, then moves the branch to
    /// the planned history. A failed checkpoint or apply keeps the card open.
    fn apply_rewrite(&mut self, cx: &mut Context<Self>) {
        let Some(card) = self.rewrite.as_ref() else {
            return;
        };
        let Stage::Confirm(planned) = &card.stage else {
            return;
        };
        let planned = planned.clone();
        if let Err(message) = self.checkpoint_before("Before editing history", &[], cx) {
            self.card_error(message, cx);
            return;
        }
        match rewrite::apply(self.repo_path(), &planned) {
            Ok(()) => self.finish_rewrite(&planned, cx),
            Err(e) => self.card_error(last_line(&e.stderr), cx),
        }
    }

    /// Closes the card after an applied edit, re-reads what it changed and says
    /// what it did on the History and Changes screens.
    fn finish_rewrite(&mut self, planned: &Planned, cx: &mut Context<Self>) {
        self.rewrite = None;
        self.refresh_facts();
        self.reload_changes();
        let _ = self.refresh();
        if self.screen == Screen::History {
            self.reload_history(cx);
        }
        let sentence = outcome_sentence(&planned.branch, planned.before.len(), planned.after.len());
        self.history.notice = Some(sentence.clone());
        self.changes.notice = Some(sentence);
        cx.notify();
    }

    /// Shows `message` in the card's error line.
    fn card_error(&mut self, message: String, cx: &mut Context<Self>) {
        if let Some(card) = self.rewrite.as_mut() {
            card.error = Some(message);
        }
        cx.notify();
    }

    fn commit_count(&self) -> usize {
        self.rewrite.as_ref().map_or(0, |card| card.commits.len())
    }

    fn commit_id(&self, index: usize) -> Option<String> {
        self.rewrite
            .as_ref()?
            .commits
            .get(index)
            .map(|commit| commit.id.clone())
    }

    /// The highlighted row and its commit id.
    fn highlighted_commit(&self) -> Option<(usize, String)> {
        let index = self.rewrite.as_ref()?.highlighted;
        Some((index, self.commit_id(index)?))
    }
}

/// The card over the window, while one is open.
pub(crate) fn rewrite_overlay_of(card: &RewriteCard) -> AnyElement {
    let body = match &card.stage {
        Stage::Choose => choose_body(card),
        Stage::Confirm(planned) => confirm_body(planned),
    };
    let hint = if card.editing() {
        HINT_EDITOR
    } else if matches!(card.stage, Stage::Confirm(_)) {
        HINT_CONFIRM
    } else {
        HINT_CHOOSE
    };
    let frame = div()
        .flex()
        .flex_col()
        .w(px(860.))
        .max_h_full()
        .overflow_hidden()
        .rounded(theme::WINDOW_RADIUS)
        .bg(theme::paper())
        .border_1()
        .border_color(theme::hairline())
        .child(title_bar(&card.branch))
        .child(
            div()
                .flex_none()
                .px(px(24.))
                .py(px(10.))
                .text_size(px(12.))
                .text_color(theme::muted())
                .child(end_sentence(card.end)),
        )
        .child(body)
        .when_some(card.error.clone(), |d, message| {
            d.child(
                div()
                    .flex_none()
                    .px(px(24.))
                    .py(px(8.))
                    .text_color(theme::rejected())
                    .child(message),
            )
        })
        .child(hint_row(hint));
    tools::overlay(frame).into_any_element()
}

impl Shell {
    /// The card over the window, when open.
    pub(crate) fn rewrite_overlay(&self, _cx: &Context<Self>) -> Option<AnyElement> {
        self.rewrite.as_ref().map(rewrite_overlay_of)
    }
}

fn title_bar(branch: &str) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .h(px(56.))
        .px(px(24.))
        .border_b_1()
        .border_color(theme::hairline())
        .child(
            div()
                .text_size(px(22.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme::ink())
                .child(format!("Edit unpushed commits on {branch}")),
        )
}

/// The key hints on the bottom line, like the other cards' footers.
fn hint_row(hint: &str) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(16.))
        .h(px(36.))
        .px(px(20.))
        .border_t_1()
        .border_color(theme::hairline())
        .font_family(theme::code_font())
        .text_size(px(theme::KEY_FONT_SIZE))
        .text_color(theme::faint())
        .child(theme::chord(hint))
}

/// The rows of the choosing stage. Each row is one child of the scrolled
/// list, so the highlighted index scrolls to the right row.
fn choose_body(card: &RewriteCard) -> AnyElement {
    div()
        .id("rewrite-rows")
        .flex()
        .flex_col()
        .flex_1()
        .min_h(px(0.))
        .py(px(8.))
        .overflow_y_scroll()
        .track_scroll(&card.scroll)
        .children(
            card.commits
                .iter()
                .zip(card.choices.iter())
                .enumerate()
                .map(|(index, (commit, choice))| {
                    let editor = card
                        .editor
                        .as_ref()
                        .filter(|(at, _, _)| *at == index)
                        .map(|(_, area, _)| area);
                    commit_item(commit, choice, index == card.highlighted, editor)
                }),
        )
        .into_any_element()
}

/// One commit: its row, and the message field under it while it is open.
fn commit_item(
    commit: &LogEntry,
    choice: &Choice,
    highlighted: bool,
    editor: Option<&Entity<TextArea>>,
) -> Div {
    let item = div()
        .flex()
        .flex_col()
        .child(commit_row(commit, choice, highlighted));
    match editor {
        Some(area) => item.child(editor_box(area)),
        None => item,
    }
}

/// A commit's row: the choice word, the short id and the subject. A reword or
/// squash shows its new first line under the subject; a squash also says where
/// it goes.
fn commit_row(commit: &LogEntry, choice: &Choice, highlighted: bool) -> Div {
    let drop = *choice == Choice::Drop;
    let subject = div()
        .min_w(px(0.))
        .truncate()
        .text_color(if drop { theme::muted() } else { theme::text() })
        .when(drop, gpui::Styled::line_through)
        .child(commit.subject.clone());
    let mut details = div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w(px(0.))
        .child(subject);
    if let Some(text) = choice_text(choice) {
        details = details.child(
            div()
                .min_w(px(0.))
                .truncate()
                .text_size(px(12.))
                .text_color(theme::muted())
                .child(first_line(text).to_owned()),
        );
    }
    if matches!(choice, Choice::Squash(_)) {
        details = details.child(
            icon::labelled(
                Icon::Into,
                icon::SMALL,
                theme::faint(),
                "into the commit below",
            )
            .text_size(px(12.)),
        );
    }
    let word = div().w(px(64.)).flex_none().child(choice_word(choice));
    let word = match choice {
        Choice::Keep => word.text_color(theme::faint()),
        Choice::Drop => word.text_color(theme::rejected()),
        Choice::Reword(_) | Choice::Squash(_) => word
            .text_color(theme::ink())
            .font_weight(FontWeight::MEDIUM),
    };
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .min_h(px(32.))
        .px(px(24.))
        .py(px(4.))
        .when(highlighted, |d| d.bg(theme::selected_line()))
        .child(word)
        .child(
            div()
                .flex_none()
                .font_family(theme::code_font())
                .text_size(px(theme::CODE_FONT_SIZE))
                .text_color(theme::faint())
                .child(short(&commit.id).to_owned()),
        )
        .child(details)
}

/// The message field, boxed like the commit message on the Changes screen.
fn editor_box(area: &Entity<TextArea>) -> Div {
    div()
        .mx(px(24.))
        .mb(px(8.))
        .px(px(14.))
        .py(px(8.))
        .rounded(theme::CONTROL_RADIUS)
        .bg(theme::paper())
        .border_1()
        .border_color(theme::hairline_control())
        .child(area.clone())
}

/// The before and after lists of the confirmation, and what applying changes.
fn confirm_body(planned: &Planned) -> AnyElement {
    div()
        .id("rewrite-confirm")
        .flex()
        .flex_col()
        .flex_1()
        .min_h(px(0.))
        .gap(px(14.))
        .p(px(24.))
        .overflow_y_scroll()
        .child(
            div()
                .flex()
                .gap(px(24.))
                .child(list_column("Before", &planned.before))
                .child(list_column("After", &planned.after)),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_color(theme::muted())
                .child(files_sentence(planned.changes_files)),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_color(theme::faint())
                .child("A checkpoint is saved first, so this can be restored."),
        )
        .into_any_element()
}

/// One side of the confirmation: a heading, then short id and subject per commit.
fn list_column(heading: &str, commits: &[LogEntry]) -> Div {
    div()
        .flex_1()
        .min_w(px(0.))
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(div().text_color(theme::muted()).child(heading.to_owned()))
        .children(commits.iter().map(|commit| {
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .min_w(px(0.))
                .child(
                    div()
                        .flex_none()
                        .font_family(theme::code_font())
                        .text_size(px(theme::CODE_FONT_SIZE))
                        .text_color(theme::faint())
                        .child(short(&commit.id).to_owned()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .truncate()
                        .text_color(theme::text())
                        .child(commit.subject.clone()),
                )
        }))
}

/// The edits the choices make, in row order. A kept commit is no edit.
fn edits_of(commits: &[LogEntry], choices: &[Choice]) -> Vec<(String, Action)> {
    commits
        .iter()
        .zip(choices)
        .filter_map(|(commit, choice)| {
            let action = match choice {
                Choice::Keep => return None,
                Choice::Reword(text) => Action::Reword(text.clone()),
                Choice::Squash(text) => Action::Squash(text.clone()),
                Choice::Drop => Action::Drop,
            };
            Some((commit.id.clone(), action))
        })
        .collect()
}

/// The row a squash of row `index` folds into: the one below it, which is
/// older. None on the last row, which has nothing below.
fn squash_target(index: usize, count: usize) -> Option<usize> {
    let below = index + 1;
    (below < count).then_some(below)
}

/// The message a squash starts from: the older commit's message, a blank line,
/// then the squashed commit's message.
fn squash_prefill(older: &str, newer: &str) -> String {
    format!("{older}\n\n{newer}")
}

/// Why the list of commits ends where it does.
fn end_sentence(end: End) -> String {
    match end {
        End::Pushed => "Older commits are on a remote and stay as they are.".to_owned(),
        End::Merge => "The list stops below a merge.".to_owned(),
        End::Root => "This is the whole history of the branch.".to_owned(),
        End::Limit => format!("Only the newest {LIMIT} are listed."),
    }
}

/// What applying the edit does to the files.
fn files_sentence(changes_files: bool) -> &'static str {
    if changes_files {
        "Files change: dropping a commit changes the working tree. Files with \
         local changes are never overwritten."
    } else {
        "Your files and staged changes are not touched."
    }
}

/// The line that says what an applied edit did, shown on the History and
/// Changes screens.
fn outcome_sentence(branch: &str, before: usize, after: usize) -> String {
    format!(
        "Edited history on {branch}: {before} {} became {after}",
        noun(before, "commit", "commits")
    )
}

/// The word that names a row's choice.
fn choice_word(choice: &Choice) -> &'static str {
    match choice {
        Choice::Keep => "keep",
        Choice::Reword(_) => "reword",
        Choice::Squash(_) => "squash",
        Choice::Drop => "drop",
    }
}

/// The message a reword or squash writes, if the choice has one.
fn choice_text(choice: &Choice) -> Option<&str> {
    match choice {
        Choice::Reword(text) | Choice::Squash(text) => Some(text.as_str()),
        Choice::Keep | Choice::Drop => None,
    }
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or("")
}

/// The short id a row shows.
fn short(id: &str) -> &str {
    id.get(..7).unwrap_or(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, subject: &str) -> LogEntry {
        LogEntry {
            id: id.to_owned(),
            parents: vec!["parent".to_owned()],
            author: String::new(),
            email: String::new(),
            time: 0,
            refs: Vec::new(),
            subject: subject.to_owned(),
        }
    }

    #[test]
    fn edits_name_only_the_commits_that_are_not_kept_with_their_action() {
        let commits = [entry("aaa", "c3"), entry("bbb", "c2"), entry("ccc", "c1")];
        let choices = [
            Choice::Keep,
            Choice::Reword("c2 better".to_owned()),
            Choice::Drop,
        ];
        assert_eq!(
            edits_of(&commits, &choices),
            [
                ("bbb".to_owned(), Action::Reword("c2 better".to_owned())),
                ("ccc".to_owned(), Action::Drop),
            ]
        );
        assert!(edits_of(&commits, &[Choice::Keep, Choice::Keep, Choice::Keep]).is_empty());
    }

    #[test]
    fn a_squash_edit_carries_its_message() {
        let commits = [entry("aaa", "c2"), entry("bbb", "c1")];
        let choices = [Choice::Squash("c1 and c2".to_owned()), Choice::Keep];
        assert_eq!(
            edits_of(&commits, &choices),
            [("aaa".to_owned(), Action::Squash("c1 and c2".to_owned()))]
        );
    }

    #[test]
    fn the_squash_prefill_is_the_older_message_a_blank_line_then_this_one() {
        assert_eq!(
            squash_prefill("older subject\n\nolder body", "newer subject"),
            "older subject\n\nolder body\n\nnewer subject"
        );
    }

    #[test]
    fn a_squash_has_a_target_below_it_except_on_the_last_row() {
        assert_eq!(squash_target(0, 3), Some(1));
        assert_eq!(squash_target(1, 3), Some(2));
        assert_eq!(squash_target(2, 3), None);
        assert_eq!(squash_target(0, 1), None);
    }

    #[test]
    fn the_outcome_sentence_counts_before_and_after() {
        assert_eq!(
            outcome_sentence("main", 3, 1),
            "Edited history on main: 3 commits became 1"
        );
        assert_eq!(
            outcome_sentence("feat/x", 1, 1),
            "Edited history on feat/x: 1 commit became 1"
        );
    }

    #[test]
    fn the_files_sentence_says_whether_the_files_move() {
        assert!(files_sentence(true).starts_with("Files change:"));
        assert_eq!(
            files_sentence(false),
            "Your files and staged changes are not touched."
        );
    }

    #[test]
    fn the_end_sentence_names_why_the_list_stops() {
        assert_eq!(
            end_sentence(End::Pushed),
            "Older commits are on a remote and stay as they are."
        );
        assert_eq!(end_sentence(End::Merge), "The list stops below a merge.");
        assert_eq!(
            end_sentence(End::Root),
            "This is the whole history of the branch."
        );
        assert_eq!(end_sentence(End::Limit), "Only the newest 50 are listed.");
    }

    #[test]
    fn each_choice_has_its_word_and_its_text() {
        assert_eq!(choice_word(&Choice::Keep), "keep");
        assert_eq!(choice_word(&Choice::Reword(String::new())), "reword");
        assert_eq!(choice_word(&Choice::Squash(String::new())), "squash");
        assert_eq!(choice_word(&Choice::Drop), "drop");
        assert_eq!(choice_text(&Choice::Keep), None);
        assert_eq!(choice_text(&Choice::Drop), None);
        assert_eq!(
            choice_text(&Choice::Reword("first\nsecond".to_owned())).map(first_line),
            Some("first")
        );
    }

    #[test]
    fn a_row_shows_the_short_id() {
        assert_eq!(short("0123456789abcdef"), "0123456");
        assert_eq!(short("abc"), "abc");
    }
}
