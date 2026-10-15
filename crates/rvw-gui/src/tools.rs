//! The state of the window's pickers and the view mode: the compare menu, the
//! branch picker, the action palette and the unified or split diff. They live
//! together so the shell holds one field for them, and so that opening one
//! closes the others. Each picker has its own file; this one has the shared
//! state, the keys that move through them, and the pieces of their cards.

use gpui::prelude::*;
use gpui::{AnyElement, Context, Div, KeyDownEvent, div, px};
use rvw_view::changes::Compare;
use rvw_view::compare_menu;
use rvw_view::keys::Screen;
use rvw_view::viewmode::ViewMode;

use crate::branch_picker::BranchPicker;
use crate::compare_picker::CompareMenu;
use crate::palette_overlay::Palette;
use crate::shell::Shell;
use crate::theme;
use crate::workspace_picker::WorkspacePicker;

#[derive(Default)]
pub struct Tools {
    /// The comparison the reviewer chose; `None` is the automatic one.
    pub compare: Option<Compare>,
    pub compare_menu: Option<CompareMenu>,
    pub branches: Option<BranchPicker>,
    pub palette: Option<Palette>,
    pub workspaces: Option<WorkspacePicker>,
    /// The diff view shows two columns instead of one.
    pub split: bool,
}

impl Tools {
    /// A picker with a text field is open, so the field holds the focus.
    pub fn field_open(&self) -> bool {
        self.branches.is_some() || self.palette.is_some() || self.workspaces.is_some()
    }

    pub fn close_all(&mut self) {
        self.compare_menu = None;
        self.branches = None;
        self.palette = None;
        self.workspaces = None;
    }
}

impl Shell {
    /// Esc closes the compare menu; the arrows move the highlight of the open
    /// branch picker or palette. True when the key was taken.
    pub(crate) fn tools_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let key = event.keystroke.key.as_str();
        if self.tools.compare_menu.is_some() {
            if key == "escape" {
                self.close_compare_menu(cx);
            }
            return true;
        }
        let delta = match key {
            "down" => true,
            "up" => false,
            _ => return false,
        };
        if self.tools.branches.is_some() {
            self.move_branch(delta, cx);
            true
        } else if self.tools.palette.is_some() {
            self.move_palette(delta, cx);
            true
        } else if self.tools.workspaces.is_some() {
            self.move_workspace(delta, cx);
            true
        } else {
            false
        }
    }

    /// Chooses the unified or the split diff.
    pub fn set_split(&mut self, split: bool, cx: &mut Context<Self>) {
        if self.tools.split != split {
            self.tools.split = split;
            cx.notify();
        }
    }

    /// Whether the diff view can be split: the review screen with the diff of
    /// a file of the change open, not the whole file or a file outside it.
    pub(crate) fn split_available(&self) -> bool {
        self.screen == Screen::Diff
            && self.selected.as_ref().is_some_and(|selected| {
                selected.mode == ViewMode::Diff && !selected.outside && selected.diff.is_ok()
            })
    }

    /// Whether the open file is drawn in two columns.
    pub(crate) fn split_active(&self) -> bool {
        self.tools.split && self.split_available()
    }

    /// Re-reads the snapshot and the files listing after the comparison or
    /// the branch changed. The open file stays when it is still in the list;
    /// a file that left the list is closed, and so is the composer.
    pub(crate) fn reload_for_new_base(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        let was_change = self.selected.as_ref().is_some_and(|s| !s.outside);
        self.refresh()?;
        let gone = self.selected.as_ref().is_some_and(|selected| {
            was_change
                && !self
                    .snapshot
                    .changes
                    .iter()
                    .any(|file| file.path == selected.path)
        });
        if gone {
            self.selected = None;
        }
        self.composer = None;
        self.removal = None;
        self.listing = None;
        if self.screen == Screen::Files {
            self.ensure_listing();
        }
        cx.notify();
        Ok(())
    }

    /// The comparisons of the compare menu, resolved now.
    pub(crate) fn compare_items(&self) -> Vec<compare_menu::Item> {
        compare_menu::resolve_all(
            std::path::Path::new(&self.snapshot.workspace),
            self.snapshot.compare,
        )
    }

    /// The overlays the pickers draw over the window.
    pub(crate) fn tool_overlays(&self, cx: &Context<Self>) -> Vec<AnyElement> {
        let mut out = Vec::new();
        if self.tools.compare_menu.is_some() {
            out.push(crate::compare_picker::scrim(cx).into_any_element());
        }
        if let Some(picker) = self.tools.branches.as_ref() {
            out.push(crate::branch_picker::overlay(picker, cx).into_any_element());
        }
        if let Some(palette) = self.tools.palette.as_ref() {
            out.push(crate::palette_overlay::overlay(palette, cx).into_any_element());
        }
        if let Some(picker) = self.tools.workspaces.as_ref() {
            out.push(crate::workspace_picker::overlay(picker, cx).into_any_element());
        }
        if let Some(ask) = self.git.ask.as_ref() {
            out.push(crate::git_overlay::overlay(ask, cx).into_any_element());
        }
        if let Some(card) = crate::checkpoint_ui::overlay(self, cx) {
            out.push(card.into_any_element());
        }
        if let Some(open) = self.launcher.as_ref() {
            out.push(crate::terminal_launcher::overlay(open, cx).into_any_element());
        }
        if let Some(message) = self.quit_ask.as_ref() {
            out.push(crate::terminal_launcher::quit_overlay(message, cx).into_any_element());
        }
        out
    }
}

/// A dimmed scrim with `card` near the top, over the whole window.
pub fn overlay(card: Div) -> Div {
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_start()
        .justify_center()
        .pt(px(104.))
        .pb(px(24.))
        .bg(theme::scrim())
        .occlude()
        .child(card)
}

/// The card frame of a picker: a field row on top, `body`, then a footer of
/// key hints.
pub fn card(input: Div, body: impl IntoElement, hints: &[&'static str]) -> Div {
    div()
        .flex()
        .flex_col()
        .w(px(860.))
        .max_h_full()
        .overflow_hidden()
        .rounded(theme::WINDOW_RADIUS)
        .bg(theme::paper())
        .border_1()
        .border_color(theme::hairline())
        .child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .h(px(52.))
                .px(px(20.))
                .border_b_1()
                .border_color(theme::hairline())
                .child(div().flex_1().min_w(px(0.)).child(input)),
        )
        .child(body)
        .child(
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
                .children(hints.iter().map(|hint| div().child(*hint))),
        )
}

/// A name drawn with its matched characters in bold.
pub fn matched_text(text: &str, matches: &[usize]) -> Div {
    div()
        .flex()
        .children(
            rvw_view::finder::runs(text, matches)
                .into_iter()
                .map(|(run, matched)| {
                    div()
                        .when(matched, |d| {
                            d.font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(theme::ink())
                        })
                        .child(run)
                }),
        )
}
