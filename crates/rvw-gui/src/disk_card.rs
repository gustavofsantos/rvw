//! The card for an open file whose buffer and disk copy disagree. A dirty
//! buffer whose file changed on disk, or whose file is gone, asks the reviewer
//! what to do; a clean buffer reloads silently. The rules are pure and
//! unit-tested here; the card itself is drawn from `Shell` state.

use gpui::prelude::*;
use gpui::{App, ClickEvent, Context, Div, Stateful, Window, div, px};
use rvw_view::filebuf::DiskState;
use rvw_view::filesave::SaveError;

use crate::shell::Shell;
use crate::theme;

/// What the card asks, and what it knows about the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prompt {
    /// The file changed on disk; `disk_blob` is its bytes' blob id.
    Changed { disk_blob: String },
    /// The file is no longer on disk. Only Close is offered.
    Gone,
}

/// The card, for the file at `path`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskCard {
    pub path: String,
    pub prompt: Prompt,
}

/// What a disk state asks the window to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Need {
    /// Nothing: the buffer matches the disk, or the reviewer has seen the card.
    Nothing,
    /// The buffer is clean and the disk changed: drop the buffer, show the disk.
    Reload,
    /// Show the card.
    Ask(Prompt),
}

/// How a save that did not write is shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveFailure {
    /// The card explains it.
    Ask(Prompt),
    /// One line in the toast.
    Notice(String),
}

/// The state of the file on disk against the buffer. `loaded` is the blob id
/// the buffer is saved against (its own, or the disk's after Keep mine), and
/// `disk` is the blob id of the bytes on disk, `None` when the file is gone.
pub fn disk_state(dirty: bool, loaded: &str, disk: Option<&str>) -> DiskState {
    match disk {
        None => DiskState::Gone,
        Some(blob) if blob == loaded => DiskState::Same,
        Some(_) if dirty => DiskState::ChangedDirty,
        Some(_) => DiskState::ChangedClean,
    }
}

/// What the window does for a disk state. `gone_acked` is whether the
/// reviewer has closed the card for a file that is gone.
pub fn need(state: DiskState, disk: Option<&str>, gone_acked: bool) -> Need {
    match (state, disk) {
        (DiskState::Same, _) => Need::Nothing,
        (DiskState::ChangedClean, _) => Need::Reload,
        (DiskState::Gone, _) if gone_acked => Need::Nothing,
        (DiskState::Gone, _) | (DiskState::ChangedDirty, None) => Need::Ask(Prompt::Gone),
        (DiskState::ChangedDirty, Some(blob)) => Need::Ask(Prompt::Changed {
            disk_blob: blob.to_owned(),
        }),
    }
}

/// How a failed save is shown: a moved or missing file gets the card, any
/// other failure is one line in the toast.
pub fn save_failure(error: &SaveError) -> SaveFailure {
    match error {
        SaveError::Moved { disk_blob } => SaveFailure::Ask(Prompt::Changed {
            disk_blob: disk_blob.clone(),
        }),
        SaveError::Gone => SaveFailure::Ask(Prompt::Gone),
        SaveError::Io(text) => SaveFailure::Notice(format!("Not saved: {text}")),
    }
}

/// The blob Keep mine saves over: the disk's, when the card offers it.
pub fn keep_blob(prompt: &Prompt) -> Option<&str> {
    match prompt {
        Prompt::Changed { disk_blob } => Some(disk_blob),
        Prompt::Gone => None,
    }
}

/// The card's first line.
pub fn title(prompt: &Prompt) -> &'static str {
    match prompt {
        Prompt::Changed { .. } => "This file changed on disk while you were editing it.",
        Prompt::Gone => "The file no longer exists.",
    }
}

impl Shell {
    /// Whether the card is up for the open file. A card for another file
    /// waits until that file is open again, and takes no keys meanwhile.
    pub(crate) fn disk_card_open(&self) -> bool {
        match (self.disk_card.as_ref(), self.selected.as_ref()) {
            (Some(card), Some(selected)) => card.path == selected.path,
            _ => false,
        }
    }

    /// Keep mine: the buffer stays, and its save is made against the disk's
    /// current bytes, so the next ⌘S writes over them.
    pub(crate) fn keep_mine(&mut self, cx: &mut Context<Self>) {
        let Some(card) = self.disk_card.take() else {
            return;
        };
        if let (Some(blob), Some(edit)) = (keep_blob(&card.prompt), self.edits.get_mut(&card.path))
        {
            edit.base = Some(blob.to_owned());
        }
        cx.notify();
    }

    /// Take theirs: the buffer is dropped and the file is read from disk.
    pub(crate) fn take_theirs(&mut self, cx: &mut Context<Self>) {
        let Some(card) = self.disk_card.take() else {
            return;
        };
        self.edits.remove(&card.path);
        self.highlights = crate::highlights::Highlighter::default();
        let mode = self
            .selected
            .as_ref()
            .filter(|selected| selected.path == card.path)
            .map(|selected| selected.mode);
        if let Some(mode) = mode {
            self.request_file(&card.path, mode, false, cx);
        }
        cx.notify();
    }

    /// Close on a card for a missing file. The card stays shut until the
    /// file comes back, so the window does not raise it on every poll.
    pub(crate) fn close_disk_card(&mut self, cx: &mut Context<Self>) {
        let Some(card) = self.disk_card.take() else {
            return;
        };
        if let Some(edit) = self.edits.get_mut(&card.path) {
            edit.gone_acked = true;
        }
        cx.notify();
    }

    /// The card over the window, when it is up for the open file.
    pub(crate) fn disk_overlay(&self, cx: &Context<Self>) -> Option<Div> {
        if !self.disk_card_open() {
            return None;
        }
        let card = self.disk_card.as_ref()?;
        let footer = match &card.prompt {
            Prompt::Changed { .. } => div()
                .flex()
                .justify_end()
                .gap(px(8.))
                .child(button(
                    "disk-keep",
                    "Keep mine",
                    cx.listener(|this: &mut Shell, _: &ClickEvent, _: &mut Window, cx| {
                        this.keep_mine(cx);
                    }),
                ))
                .child(button(
                    "disk-theirs",
                    "Take theirs",
                    cx.listener(|this: &mut Shell, _: &ClickEvent, _: &mut Window, cx| {
                        this.take_theirs(cx);
                    }),
                )),
            Prompt::Gone => div().flex().justify_end().child(button(
                "disk-close",
                "Close",
                cx.listener(|this: &mut Shell, _: &ClickEvent, _: &mut Window, cx| {
                    this.close_disk_card(cx);
                }),
            )),
        };
        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .w(px(520.))
            .p(px(24.))
            .rounded(theme::WINDOW_RADIUS)
            .bg(theme::paper())
            .border_1()
            .border_color(theme::hairline())
            .child(div().text_color(theme::ink()).child(title(&card.prompt)));
        if let Prompt::Changed { .. } = card.prompt {
            body = body.child(div().text_color(theme::muted()).child(
                "Your unsaved edits are still here. Keep mine writes them over the file \
                     on your next save. Take theirs drops them and shows the file as it is \
                     on disk.",
            ));
        }
        Some(crate::tools::overlay(body.child(footer)))
    }
}

fn button(
    id: &'static str,
    label: &'static str,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    div()
        .id(id)
        .px(px(14.))
        .h(px(30.))
        .flex()
        .items_center()
        .rounded(theme::CONTROL_RADIUS)
        .cursor_pointer()
        .border_1()
        .border_color(theme::hairline())
        .text_color(theme::ink())
        .child(label)
        .on_click(on_click)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disk_state_compares_the_saved_blob_with_the_disk() {
        assert_eq!(disk_state(true, "b1", Some("b1")), DiskState::Same);
        assert_eq!(disk_state(false, "b1", Some("b2")), DiskState::ChangedClean);
        assert_eq!(disk_state(true, "b1", Some("b2")), DiskState::ChangedDirty);
        assert_eq!(disk_state(true, "b1", None), DiskState::Gone);
        assert_eq!(disk_state(false, "b1", None), DiskState::Gone);
    }

    #[test]
    fn a_clean_buffer_reloads_and_a_dirty_one_asks() {
        assert_eq!(need(DiskState::Same, Some("b1"), false), Need::Nothing);
        assert_eq!(
            need(DiskState::ChangedClean, Some("b2"), false),
            Need::Reload
        );
        assert_eq!(
            need(DiskState::ChangedDirty, Some("b2"), false),
            Need::Ask(Prompt::Changed {
                disk_blob: "b2".to_owned()
            })
        );
    }

    #[test]
    fn a_missing_file_asks_once_and_then_stays_shut() {
        assert_eq!(need(DiskState::Gone, None, false), Need::Ask(Prompt::Gone));
        assert_eq!(need(DiskState::Gone, None, true), Need::Nothing);
    }

    #[test]
    fn a_refused_save_becomes_the_card_or_one_line() {
        assert_eq!(
            save_failure(&SaveError::Moved {
                disk_blob: "b9".to_owned()
            }),
            SaveFailure::Ask(Prompt::Changed {
                disk_blob: "b9".to_owned()
            })
        );
        assert_eq!(
            save_failure(&SaveError::Gone),
            SaveFailure::Ask(Prompt::Gone)
        );
        assert_eq!(
            save_failure(&SaveError::Io("disk full".to_owned())),
            SaveFailure::Notice("Not saved: disk full".to_owned())
        );
    }

    #[test]
    fn keep_mine_saves_against_the_disk_copy_and_a_gone_file_offers_none() {
        let changed = Prompt::Changed {
            disk_blob: "b2".to_owned(),
        };
        assert_eq!(keep_blob(&changed), Some("b2"));
        assert_eq!(keep_blob(&Prompt::Gone), None);
        assert_eq!(title(&Prompt::Gone), "The file no longer exists.");
    }
}
