//! Checkpoints on screen: the list on the Changes screen, the picker behind
//! "Restore a checkpoint…", and the confirmation that shows what a restore
//! changes before it runs. The git side is `rvw_git::checkpoint`.

use std::time::{SystemTime, UNIX_EPOCH};

use gpui::prelude::*;
use gpui::{
    div, px, ClickEvent, Context, Div, FontWeight, KeyDownEvent, SharedString, Stateful, Window,
};
use rvw_git::checkpoint::{self, Checkpoint, Preview};

use crate::git_tools::{last_line, RepoFacts};
use crate::queue_screen::noun;
use crate::shell::Shell;
use crate::theme;
use crate::tools;

/// How many checkpoints the Changes screen lists.
const LIST_ROWS: usize = 8;
/// How many changed files the confirmation lists before "and N more".
const FILES_SHOWN: usize = 12;

/// An open restore confirmation. Its left column lists every checkpoint,
/// newest first; `checkpoint` is the row `highlighted` points at.
pub struct RestoreConfirm {
    pub checkpoint: Checkpoint,
    /// What the restore would change, or why that could not be read.
    pub preview: Result<Preview, String>,
    /// Why the last restore failed.
    pub error: Option<String>,
    /// The row of `checkpoint` in `facts.checkpoints`.
    pub highlighted: usize,
}

impl Shell {
    /// "Save a checkpoint" in the palette.
    pub fn save_checkpoint(&mut self, cx: &mut Context<Self>) {
        if let Err(message) = self.checkpoint_before("Saved by you", &[], cx) {
            self.changes.error = Some(message);
        }
        cx.notify();
    }

    /// "Restore a checkpoint…" in the palette: the list to pick one from. Does
    /// nothing when there is no checkpoint, which the palette shows as dimmed.
    pub fn open_checkpoints(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(newest) = self.git.facts.checkpoints.first().cloned() else {
            return;
        };
        self.show_restore(0, newest, cx);
        self.focus_shell(window);
    }

    /// Opens the confirmation for `checkpoint`, with its row highlighted. A
    /// checkpoint that is no longer listed opens the newest instead.
    pub fn open_restore(&mut self, checkpoint: Checkpoint, cx: &mut Context<Self>) {
        let (index, shown) = match index_of(&self.git.facts.checkpoints, &checkpoint.id) {
            Some(index) => (index, checkpoint),
            None => match self.git.facts.checkpoints.first() {
                Some(newest) => (0, newest.clone()),
                None => return,
            },
        };
        self.show_restore(index, shown, cx);
    }

    /// ⌘Z: the confirmation for the newest checkpoint.
    pub fn restore_latest(&mut self, cx: &mut Context<Self>) {
        if self.restore_blocked() {
            return;
        }
        self.refresh_facts();
        if let Some(newest) = self.git.facts.checkpoints.first().cloned() {
            self.show_restore(0, newest, cx);
        } else {
            self.changes.notice = Some("No checkpoint yet".to_owned());
            cx.notify();
        }
    }

    /// Escape, the arrows and Enter while the confirmation is open. The
    /// confirmation is modal, so every key is taken; true when one is open.
    pub(crate) fn restore_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        if self.git.confirm.is_none() {
            return false;
        }
        match event.keystroke.key.as_str() {
            "escape" => self.close_restore(cx),
            "up" => self.move_restore(false, cx),
            "down" => self.move_restore(true, cx),
            "enter" => self.run_restore(cx),
            _ => {}
        }
        true
    }

    /// Whether a field or another dialog holds the keyboard, or the
    /// confirmation is already open, so ⌘Z must not open one.
    fn restore_blocked(&self) -> bool {
        self.composer.is_some()
            || self.submit.is_some()
            || self.finder.is_some()
            || self.tools.field_open()
            || self.git.ask.is_some()
            || self.git.confirm.is_some()
            || self.rewrite.is_some()
            || self.gate.is_some()
    }

    /// Opens the confirmation on the row `index`, closing the other pickers.
    fn show_restore(&mut self, index: usize, checkpoint: Checkpoint, cx: &mut Context<Self>) {
        let preview = self.preview_of(&checkpoint);
        self.tools.close_all();
        self.finder = None;
        self.git.ask = None;
        self.git.confirm = Some(RestoreConfirm {
            checkpoint,
            preview,
            error: None,
            highlighted: index,
        });
        cx.notify();
    }

    /// What restoring `checkpoint` would change, or git's reason it cannot be read.
    fn preview_of(&self, checkpoint: &Checkpoint) -> Result<Preview, String> {
        checkpoint::preview(self.repo_path(), checkpoint).map_err(|e| last_line(&e.stderr))
    }

    /// Moves the highlight one row, stopping at either end.
    fn move_restore(&mut self, down: bool, cx: &mut Context<Self>) {
        let Some(confirm) = self.git.confirm.as_ref() else {
            return;
        };
        let last = self.git.facts.checkpoints.len().saturating_sub(1);
        let index = if down {
            (confirm.highlighted + 1).min(last)
        } else {
            confirm.highlighted.saturating_sub(1)
        };
        self.highlight_restore(index, cx);
    }

    /// Highlights row `index` and reads what restoring it would change.
    fn highlight_restore(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(checkpoint) = self.git.facts.checkpoints.get(index).cloned() else {
            return;
        };
        let preview = self.preview_of(&checkpoint);
        if let Some(confirm) = self.git.confirm.as_mut() {
            confirm.highlighted = index;
            confirm.checkpoint = checkpoint;
            confirm.preview = preview;
            confirm.error = None;
        }
        cx.notify();
    }

    fn close_restore(&mut self, cx: &mut Context<Self>) {
        self.git.confirm = None;
        cx.notify();
    }

    /// Restores the checkpoint the confirmation shows. Refused while the
    /// preview cannot be read, as the button is disabled then. A failed restore
    /// keeps the confirmation open with the reason.
    fn run_restore(&mut self, cx: &mut Context<Self>) {
        let Some(confirm) = self.git.confirm.as_ref() else {
            return;
        };
        if confirm.preview.is_err() {
            return;
        }
        let checkpoint = confirm.checkpoint.clone();
        match checkpoint::restore(self.repo_path(), &checkpoint) {
            Ok(_) => {
                self.git.confirm = None;
                self.changes.notice = Some(format!(
                    "Restored “{}”. The state before it is saved as a checkpoint.",
                    checkpoint.label
                ));
                self.changes.error = None;
                self.refresh_facts();
                self.reload_changes();
                let _ = self.refresh();
            }
            Err(e) => {
                if let Some(confirm) = self.git.confirm.as_mut() {
                    confirm.error = Some(last_line(&e.stderr));
                }
            }
        }
        cx.notify();
    }
}

/// The confirmation card over the window, while one is open.
pub fn overlay(shell: &Shell, cx: &Context<Shell>) -> Option<Div> {
    let confirm = shell.git.confirm.as_ref()?;
    let card = div()
        .flex()
        .flex_col()
        .w(px(860.))
        .max_h_full()
        .overflow_hidden()
        .rounded(theme::WINDOW_RADIUS)
        .bg(theme::paper())
        .border_1()
        .border_color(theme::hairline())
        .child(title_bar())
        .child(
            div()
                .flex()
                .flex_1()
                .min_h(px(0.))
                .child(history_column(
                    &shell.git.facts.checkpoints,
                    confirm.highlighted,
                    now_secs(),
                    cx,
                ))
                .child(detail_column(confirm)),
        )
        .child(footer(confirm, cx));
    Some(tools::overlay(card))
}

/// The checkpoint list of the Changes screen's right panel.
pub fn list(facts: &RepoFacts, cx: &Context<Shell>) -> Div {
    let now = now_secs();
    let has = !facts.checkpoints.is_empty();
    let restore = div()
        .id("checkpoint-restore-latest")
        .flex()
        .flex_none()
        .items_center()
        .gap(px(8.))
        .child("Restore\u{2026}")
        .child(key_cap("\u{2318}Z"));
    let restore = if has {
        restore
            .text_color(theme::ink())
            .font_weight(FontWeight::MEDIUM)
            .cursor_pointer()
            .on_click(cx.listener(
                |this: &mut Shell, _: &ClickEvent, window: &mut Window, cx: &mut Context<Shell>| {
                    this.restore_latest(cx);
                    this.focus_shell(window);
                },
            ))
    } else {
        restore.text_color(theme::faint())
    };
    let heading = div()
        .flex()
        .items_center()
        .child(
            div()
                .flex_1()
                .text_color(theme::muted())
                .child("Checkpoints"),
        )
        .child(restore);
    let body = if has {
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .children(facts.checkpoints.iter().take(LIST_ROWS).enumerate().map(
                |(index, checkpoint)| {
                    let checkpoint = checkpoint.clone();
                    row(format!("checkpoint-row-{index}"), &checkpoint, false, now).on_click(
                        cx.listener(
                            move |this: &mut Shell,
                                  _: &ClickEvent,
                                  window: &mut Window,
                                  cx: &mut Context<Shell>| {
                                this.open_restore(checkpoint.clone(), cx);
                                this.focus_shell(window);
                            },
                        ),
                    )
                },
            ))
            .into_any_element()
    } else {
        div()
            .text_color(theme::muted())
            .child("No checkpoint yet.")
            .into_any_element()
    };
    div()
        .flex()
        .flex_col()
        .gap(px(12.))
        .child(heading)
        .child(body)
        .child(div().text_size(px(12.)).text_color(theme::faint()).child(
            "rvw saves a checkpoint before every change it makes. Restoring one is \
                     shown first and can be reversed.",
        ))
}

fn title_bar() -> Div {
    div()
        .flex()
        .items_center()
        .h(px(56.))
        .px(px(24.))
        .border_b_1()
        .border_color(theme::hairline())
        .child(
            div()
                .flex_1()
                .text_size(px(16.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme::ink())
                .child("Restore a checkpoint"),
        )
}

/// The checkpoints, newest first; a click highlights its row and reads its preview.
fn history_column(
    checkpoints: &[Checkpoint],
    highlighted: usize,
    now: i64,
    cx: &Context<Shell>,
) -> Stateful<Div> {
    let rows = checkpoints.iter().enumerate().map(|(index, checkpoint)| {
        row(
            format!("restore-row-{index}"),
            checkpoint,
            index == highlighted,
            now,
        )
        .on_click(cx.listener(
            move |this: &mut Shell,
                  _: &ClickEvent,
                  window: &mut Window,
                  cx: &mut Context<Shell>| {
                this.highlight_restore(index, cx);
                this.focus_shell(window);
            },
        ))
    });
    div()
        .id("restore-history")
        .flex()
        .flex_col()
        .w(px(300.))
        .flex_none()
        .py(px(8.))
        .border_r_1()
        .border_color(theme::hairline())
        .overflow_y_scroll()
        .children(rows)
}

/// One checkpoint's row: its label, and how long ago it was taken.
fn row(id: String, checkpoint: &Checkpoint, selected: bool, now: i64) -> Stateful<Div> {
    div()
        .id(SharedString::from(id))
        .flex()
        .items_center()
        .gap(px(10.))
        .h(px(34.))
        .mx(px(8.))
        .px(px(12.))
        .rounded(theme::ROW_RADIUS)
        .cursor_pointer()
        .when(selected, |d| d.bg(theme::selected_panel()))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .text_color(if selected {
                    theme::ink()
                } else {
                    theme::text()
                })
                .child(checkpoint.label.clone()),
        )
        .child(
            div()
                .flex_none()
                .text_size(px(theme::KEY_FONT_SIZE))
                .text_color(theme::faint())
                .child(ago(now, checkpoint.at)),
        )
}

/// The highlighted checkpoint's preview: what it restores, and the restore's
/// own error when it failed.
fn detail_column(confirm: &RestoreConfirm) -> Stateful<Div> {
    let body = match &confirm.preview {
        Ok(preview) => preview_body(preview),
        Err(message) => div().text_color(theme::rejected()).child(message.clone()),
    };
    div()
        .id("restore-detail")
        .flex()
        .flex_col()
        .flex_1()
        .min_w(px(0.))
        .gap(px(14.))
        .p(px(24.))
        .overflow_y_scroll()
        .child(body)
        .when_some(confirm.error.clone(), |d, message| {
            d.child(div().text_color(theme::rejected()).child(message))
        })
}

/// The lines describing what restoring the preview's checkpoint changes.
fn preview_body(preview: &Preview) -> Div {
    let then = preview.branch_then.as_deref().unwrap_or("detached");
    let mut body = div().flex().flex_col().gap(px(14.)).child(
        div()
            .text_color(theme::ink())
            .child(format!("Branch: {then}")),
    );
    if preview.branch_now != preview.branch_then {
        let now = match &preview.branch_now {
            Some(now) => format!("You are on {now} now."),
            None => "You are detached now.".to_owned(),
        };
        body = body.child(div().text_color(theme::muted()).child(now));
    }
    if preview.commits_left_out > 0 {
        let branch = preview.branch_then.as_deref().unwrap_or("the branch");
        body = body.child(
            div()
                .text_color(theme::needs_you())
                .child(commits_sentence(preview.commits_left_out, branch)),
        );
    }
    body = body.child(files_block(&preview.files));
    if preview.index_differs {
        body = body.child(
            div()
                .text_color(theme::muted())
                .child("The staged changes differ too."),
        );
    }
    if !preview.recreated.is_empty() {
        body = body.child(
            div()
                .text_color(theme::muted())
                .child(format!("Brought back: {}", preview.recreated.join(", "))),
        );
    }
    // Orange: an ignored file is not in any other checkpoint, so the user
    // must see that this one replaces it.
    if !preview.overwritten_ignored.is_empty() {
        body = body.child(div().text_color(theme::needs_you()).child(format!(
            "Ignored files it replaces (kept in the checkpoint saved first): {}",
            preview.overwritten_ignored.join(", ")
        )));
    }
    body
}

/// "Files that change" and the first files of the restore, then how many more.
fn files_block(files: &[(char, String)]) -> Div {
    let block = div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(div().text_color(theme::muted()).child("Files that change"));
    if files.is_empty() {
        return block.child(div().text_color(theme::muted()).child("No file changes."));
    }
    let (shown, hidden) = cut_files(files);
    let block = block.children(shown.iter().map(|(letter, path)| file_row(*letter, path)));
    if hidden == 0 {
        block
    } else {
        block.child(
            div()
                .text_color(theme::faint())
                .child(format!("and {hidden} more")),
        )
    }
}

/// One changed file: git's status letter, then its path, both in the code font.
fn file_row(letter: char, path: &str) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(10.))
        .font_family(theme::code_font())
        .text_size(px(theme::CODE_FONT_SIZE))
        .child(
            div()
                .w(px(12.))
                .flex_none()
                .text_color(theme::muted())
                .child(letter.to_string()),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .text_color(theme::text())
                .child(path.to_owned()),
        )
}

/// The key hint line on the left, then Cancel and the primary Restore. Restore
/// is dim, with no click, while the preview cannot be read.
fn footer(confirm: &RestoreConfirm, cx: &Context<Shell>) -> Div {
    let cancel = div()
        .id("restore-cancel")
        .flex()
        .items_center()
        .gap(px(8.))
        .h(px(36.))
        .px(px(16.))
        .rounded(theme::CONTROL_RADIUS)
        .border_1()
        .border_color(theme::hairline_control())
        .font_family(theme::ui_font())
        .text_color(theme::ink())
        .cursor_pointer()
        .on_click(cx.listener(
            |this: &mut Shell, _: &ClickEvent, window: &mut Window, cx: &mut Context<Shell>| {
                this.close_restore(cx);
                this.focus_shell(window);
            },
        ))
        .child("Cancel")
        .child(key_cap("esc"));
    let go = div()
        .id("restore-go")
        .flex()
        .items_center()
        .gap(px(8.))
        .h(px(36.))
        .px(px(20.))
        .rounded(theme::CONTROL_RADIUS)
        .font_family(theme::ui_font())
        .child("Restore")
        .child(key_cap("\u{21b5}"));
    let go = if confirm.preview.is_ok() {
        go.bg(theme::ink())
            .text_color(theme::paper())
            .font_weight(FontWeight::MEDIUM)
            .cursor_pointer()
            .on_click(cx.listener(
                |this: &mut Shell, _: &ClickEvent, window: &mut Window, cx: &mut Context<Shell>| {
                    this.run_restore(cx);
                    this.focus_shell(window);
                },
            ))
    } else {
        go.bg(theme::selected_panel()).text_color(theme::faint())
    };
    div()
        .flex()
        .items_center()
        .gap(px(16.))
        .h(px(68.))
        .px(px(24.))
        .border_t_1()
        .border_color(theme::hairline())
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .text_size(px(12.))
                .text_color(theme::muted())
                .child(
                    "Restoring saves the current state as a checkpoint first, so you can go back.",
                ),
        )
        .child(cancel)
        .child(go)
}

fn key_cap(glyph: &str) -> Div {
    div()
        .font_family(theme::code_font())
        .text_size(px(theme::KEY_FONT_SIZE))
        .text_color(theme::faint())
        .child(theme::chord(glyph))
}

/// The row of checkpoint `id` in `checkpoints`, if it is there.
fn index_of(checkpoints: &[Checkpoint], id: &str) -> Option<usize> {
    checkpoints
        .iter()
        .position(|checkpoint| checkpoint.id == id)
}

/// The first files to list and how many are left out.
fn cut_files(files: &[(char, String)]) -> (&[(char, String)], usize) {
    let shown = files.len().min(FILES_SHOWN);
    (&files[..shown], files.len().saturating_sub(shown))
}

/// "1 commit leaves main (kept in the checkpoint saved before the restore)."
fn commits_sentence(count: usize, branch: &str) -> String {
    let leave = if count == 1 { "leaves" } else { "leave" };
    format!(
        "{count} {} {leave} {branch} (kept in the checkpoint saved before the restore).",
        noun(count, "commit", "commits")
    )
}

/// How long ago a checkpoint was taken, in the coarse steps the list shows:
/// "now", "41m", "23h", "3d".
fn ago(now_secs: i64, at: i64) -> String {
    let secs = now_secs.saturating_sub(at).max(0);
    match secs {
        0..=59 => "now".to_owned(),
        60..=3_599 => format!("{}m", secs / 60),
        3_600..=86_399 => format!("{}h", secs / 3_600),
        _ => format!("{}d", secs / 86_400),
    }
}

/// Seconds since the Unix epoch, now.
fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checkpoint(id: &str, at: i64) -> Checkpoint {
        Checkpoint {
            id: id.to_owned(),
            label: format!("label {id}"),
            at,
            branch: Some("main".to_owned()),
            head: None,
            index_tree: "index".to_owned(),
            worktree_tree: "tree".to_owned(),
            kept: Vec::new(),
        }
    }

    fn files(count: usize) -> Vec<(char, String)> {
        (0..count)
            .map(|i| ('M', format!("src/file{i}.rs")))
            .collect()
    }

    #[test]
    fn ago_reads_the_time_in_coarse_steps() {
        let now = 1_000_000_000;
        assert_eq!(ago(now, now), "now");
        assert_eq!(ago(now, now - 59), "now");
        assert_eq!(ago(now, now - 60), "1m");
        assert_eq!(ago(now, now - 41 * 60), "41m");
        assert_eq!(ago(now, now - 3_600), "1h");
        assert_eq!(ago(now, now - 23 * 3_600), "23h");
        assert_eq!(ago(now, now - 25 * 3_600), "1d");
        assert_eq!(ago(now, now - 3 * 86_400), "3d");
    }

    #[test]
    fn a_time_in_the_future_reads_as_now() {
        assert_eq!(ago(1_000, 1_010), "now");
    }

    #[test]
    fn the_file_list_shows_twelve_then_counts_the_rest() {
        let none = files(0);
        assert_eq!(cut_files(&none), (&none[..], 0));

        let twelve = files(12);
        assert_eq!(cut_files(&twelve), (&twelve[..], 0));

        let thirteen = files(13);
        let (shown, hidden) = cut_files(&thirteen);
        assert_eq!(shown.len(), 12);
        assert_eq!(hidden, 1);

        let thirty = files(30);
        let (shown, hidden) = cut_files(&thirty);
        assert_eq!(shown.len(), 12);
        assert_eq!(hidden, 18);
    }

    #[test]
    fn the_commit_sentence_agrees_with_its_count() {
        assert_eq!(
            commits_sentence(1, "main"),
            "1 commit leaves main (kept in the checkpoint saved before the restore)."
        );
        assert_eq!(
            commits_sentence(2, "main"),
            "2 commits leave main (kept in the checkpoint saved before the restore)."
        );
    }

    #[test]
    fn a_checkpoint_is_found_by_its_id() {
        let list = vec![
            checkpoint("c3", 30),
            checkpoint("c2", 20),
            checkpoint("c1", 10),
        ];
        assert_eq!(index_of(&list, "c2"), Some(1));
        assert_eq!(index_of(&list, "gone"), None);
        assert_eq!(index_of(&[], "c1"), None);
    }
}
