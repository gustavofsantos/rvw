//! The commit gate's card. Before a commit, an amend or a push, the comments
//! still open or awaiting a verdict are read off the UI thread. When there are
//! none, the action runs. When there are some, a card names them and asks for a
//! reason: Confirm records the override with that reason and then runs the
//! action, and a failed record runs nothing. The wording is in
//! `rvw_view::commit_screen`; this file lays it out and runs the calls.

use std::sync::Arc;

use gpui::prelude::*;
use gpui::{
    div, px, ClickEvent, Context, Div, Entity, FontWeight, KeyDownEvent, Subscription, Window,
};
use rvw_core::operations::{GateInput, GateOutput, OverrideInput};
use rvw_core::types::Actor;
use rvw_view::commit_screen::{self, Gate, GateAction};

use crate::shell::Shell;
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme;
use crate::tools;

/// The placeholder of the reason field.
const REASON_PLACEHOLDER: &str = "Why go ahead anyway?";

/// What waits for the gate, and what runs once it is clear.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gated {
    Commit,
    Amend,
    Push,
    /// Publishing sends the branch's commits, so the gate reads it as a push.
    Publish,
}

impl Gated {
    /// The wording the card uses for this action.
    fn wording(self) -> GateAction {
        match self {
            Gated::Commit => GateAction::Commit,
            Gated::Amend => GateAction::Amend,
            Gated::Push | Gated::Publish => GateAction::Push,
        }
    }
}

/// The open card: the action waiting, and where its check stands.
pub struct GateCard {
    /// Numbers the card, so a read or an override that lands late is told
    /// apart from the card open now.
    id: u64,
    action: Gated,
    stage: Stage,
}

enum Stage {
    /// The gate is being read. No card shows yet.
    Checking,
    /// The gate is not clear: the comments it names and the reason field.
    Open(Waiting),
    /// The gate could not be read. Nothing runs.
    Failed(String),
}

struct Waiting {
    open: Vec<String>,
    unjudged: Vec<String>,
    lane: String,
    input: Entity<TextInput>,
    /// The override is being recorded. The card waits and cannot close.
    recording: bool,
    /// Why the card did not go ahead: a failed record, or no author.
    error: Option<String>,
    /// The field takes the keyboard on the next frame. Opening has no window
    /// to focus with, so the frame does it; see `focus_gate`.
    focus_pending: bool,
    _events: Subscription,
    _typing: Subscription,
}

impl GateCard {
    /// Whether the reason field holds the keyboard.
    pub fn editing(&self) -> bool {
        matches!(self.stage, Stage::Open(_))
    }

    fn recording(&self) -> bool {
        matches!(&self.stage, Stage::Open(open) if open.recording)
    }
}

impl Shell {
    /// Runs `action` once the gate lets it through. The gate is read off the UI
    /// thread: when it is clear the action runs at once, otherwise the card
    /// opens. A request while a check or a card is open does nothing.
    pub(crate) fn gated(&mut self, action: Gated, cx: &mut Context<Self>) {
        if self.gate.is_some() {
            return;
        }
        self.gate_requests = self.gate_requests.wrapping_add(1);
        let id = self.gate_requests;
        self.gate = Some(GateCard {
            id,
            action,
            stage: Stage::Checking,
        });
        let input = GateInput {
            workspace: self.identity.workspace.clone(),
            lane: self.identity.lane.clone(),
        };
        let service = Arc::clone(&self.service);
        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_executor()
                .spawn(async move { service.gate(input).map_err(|e| e.message) })
                .await;
            this.update(cx, |this, cx| this.gate_read_done(id, outcome, cx))
                .ok();
        })
        .detach();
        cx.notify();
    }

    /// Acts on what the gate read found: runs the action when it is clear, opens
    /// the card when it is not, and shows the failure when the read failed.
    fn gate_read_done(
        &mut self,
        id: u64,
        outcome: Result<GateOutput, String>,
        cx: &mut Context<Self>,
    ) {
        let Some(card) = self.gate.as_mut().filter(|card| card.id == id) else {
            return;
        };
        match outcome {
            Ok(read) if read.clear => {
                let action = card.action;
                self.gate = None;
                self.run_gated(action, cx);
            }
            Ok(read) => {
                let reason = self.gate_reason.clone();
                let input = cx.new(|cx| TextInput::new(REASON_PLACEHOLDER, cx));
                input.update(cx, |field, cx| field.set_text(&reason, cx));
                let events =
                    cx.subscribe(&input, |this: &mut Self, _, event: &TextInputEvent, cx| {
                        match event {
                            TextInputEvent::Submit => this.confirm_gate(cx),
                            TextInputEvent::Cancel => this.close_gate(cx),
                        }
                    });
                // The reason last typed stays for the session: the next card
                // opens with it.
                let typing = cx.observe(
                    &input,
                    |this: &mut Self, field: Entity<TextInput>, cx: &mut Context<Self>| {
                        field.read(cx).text().clone_into(&mut this.gate_reason);
                        cx.notify();
                    },
                );
                card.stage = Stage::Open(Waiting {
                    open: read.open,
                    unjudged: read.unjudged,
                    lane: read.lane.unwrap_or_default(),
                    input,
                    recording: false,
                    error: None,
                    focus_pending: true,
                    _events: events,
                    _typing: typing,
                });
                self.tools.close_all();
                self.finder = None;
                self.git.ask = None;
            }
            Err(message) => card.stage = Stage::Failed(message),
        }
        cx.notify();
    }

    /// Confirm: records the override with the typed reason, then runs the action.
    /// Nothing runs when the record fails; the card stays open with the reason.
    pub(crate) fn confirm_gate(&mut self, cx: &mut Context<Self>) {
        let Some(card) = self.gate.as_mut() else {
            return;
        };
        let (id, action) = (card.id, card.action);
        let Stage::Open(open) = &mut card.stage else {
            return;
        };
        if open.recording {
            return;
        }
        let author = self.identity.author.trim().to_owned();
        if let Some(note) = commit_screen::gate_author_note(&author) {
            open.error = Some(note.to_owned());
            cx.notify();
            return;
        }
        let reason = open.input.read(cx).text().trim().to_owned();
        if !commit_screen::gate_confirm_ready(&reason, &author) {
            return;
        }
        open.recording = true;
        open.error = None;
        let input = OverrideInput {
            workspace: self.identity.workspace.clone(),
            action: action.wording().core_action().to_owned(),
            reason,
            lane: self.identity.lane.clone(),
            author,
        };
        let service = Arc::clone(&self.service);
        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_executor()
                .spawn(async move {
                    service
                        .override_gate(input, Actor::Human)
                        .map(|_| ())
                        .map_err(|e| e.message)
                })
                .await;
            this.update(cx, |this, cx| this.gate_override_done(id, outcome, cx))
                .ok();
        })
        .detach();
        cx.notify();
    }

    /// Acts on the override's record: runs the action once it is recorded, and
    /// keeps the card open with the reason when it is not.
    fn gate_override_done(&mut self, id: u64, outcome: Result<(), String>, cx: &mut Context<Self>) {
        let Some(card) = self.gate.as_mut().filter(|card| card.id == id) else {
            return;
        };
        match outcome {
            Ok(()) => {
                let action = card.action;
                self.gate = None;
                self.run_gated(action, cx);
            }
            Err(message) => {
                if let Stage::Open(open) = &mut card.stage {
                    open.recording = false;
                    open.error = Some(message);
                }
            }
        }
        cx.notify();
    }

    /// Runs the action the gate let through.
    fn run_gated(&mut self, action: Gated, cx: &mut Context<Self>) {
        match action {
            Gated::Commit | Gated::Amend => self.commit_gated(cx),
            Gated::Push => self.push_now(cx),
            Gated::Publish => self.publish_now(cx),
        }
    }

    /// Closes the card. Refused while an override is being recorded: the record
    /// may land, so the card waits for it.
    pub(crate) fn close_gate(&mut self, cx: &mut Context<Self>) {
        if self.gate.as_ref().is_some_and(GateCard::recording) {
            return;
        }
        self.gate = None;
        cx.notify();
    }

    /// The card is modal: while it is open every key is its own. Escape closes it.
    /// True when a card is open.
    pub(crate) fn gate_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        if self.gate.is_none() {
            return false;
        }
        if event.keystroke.key == "escape" {
            self.close_gate(cx);
        }
        true
    }

    /// Gives the reason field the keyboard once its card has opened. The frame
    /// calls this, since it is the one place with the window.
    pub(crate) fn focus_gate(&mut self, window: &mut Window, cx: &gpui::App) {
        let Some(card) = self.gate.as_mut() else {
            return;
        };
        if let Stage::Open(open) = &mut card.stage {
            if std::mem::take(&mut open.focus_pending) {
                open.input.read(cx).focus(window);
            }
        }
    }

    /// The card over the window, when one is open and its check has read.
    pub(crate) fn gate_overlay(&self, cx: &Context<Self>) -> Option<Div> {
        let card = self.gate.as_ref()?;
        let wording = card.action.wording();
        let author = self.identity.author.trim();
        let (body, ready, hint) = match &card.stage {
            Stage::Checking => return None,
            Stage::Open(open) => {
                let reason = open.input.read(cx).text().trim();
                let ready = !open.recording && commit_screen::gate_confirm_ready(reason, author);
                (
                    open_body(wording, open, author),
                    ready,
                    "The reason and the comments named here are recorded with the override.",
                )
            }
            Stage::Failed(message) => (failed_body(message), false, ""),
        };
        let card = div()
            .flex()
            .flex_col()
            .w(px(560.))
            .max_h_full()
            .overflow_hidden()
            .rounded(theme::WINDOW_RADIUS)
            .bg(theme::paper())
            .border_1()
            .border_color(theme::hairline())
            .child(title_bar(&commit_screen::gate_title(wording)))
            .child(body)
            .child(footer(hint, wording, ready, cx));
        Some(tools::overlay(card))
    }
}

fn title_bar(title: &str) -> Div {
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
                .child(title.to_owned()),
        )
}

/// The notice, the comments it names, the author note, the reason field and
/// any error of the last try.
fn open_body(wording: GateAction, open: &Waiting, author: &str) -> Div {
    let gate = Gate::from_ids(&open.open, &open.unjudged, &open.lane);
    let notice = commit_screen::gate_notice(&gate, wording).unwrap_or_default();
    let mut body = div()
        .flex()
        .flex_col()
        .gap(px(12.))
        .p(px(24.))
        .child(div().text_color(theme::needs_you()).child(notice));
    for line in commit_screen::gate_id_lines(&open.open, &open.unjudged) {
        body = body.child(
            div()
                .font_family(theme::code_font())
                .text_size(px(theme::CODE_FONT_SIZE))
                .text_color(theme::text())
                .child(line),
        );
    }
    if let Some(note) = commit_screen::gate_author_note(author) {
        body = body.child(div().text_color(theme::muted()).child(note));
    }
    body = body.child(
        div()
            .px(px(14.))
            .py(px(8.))
            .rounded(theme::CONTROL_RADIUS)
            .bg(theme::paper())
            .border_1()
            .border_color(theme::hairline_control())
            .child(open.input.clone()),
    );
    if let Some(error) = open.error.clone() {
        body = body.child(div().text_color(theme::rejected()).child(error));
    }
    body
}

/// The line that says the gate could not be read, and that nothing ran.
fn failed_body(message: &str) -> Div {
    div()
        .p(px(24.))
        .text_color(theme::rejected())
        .child(format!(
            "Could not read the commit gate: {message}. Nothing ran."
        ))
}

/// The hint on the left, then Cancel and the confirm button. Confirm is dim,
/// with no click, until a reason is typed and an author is set.
fn footer(hint: &str, wording: GateAction, ready: bool, cx: &Context<Shell>) -> Div {
    let cancel = div()
        .id("gate-cancel")
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
            |this: &mut Shell, _: &ClickEvent, _: &mut Window, cx: &mut Context<Shell>| {
                this.close_gate(cx);
            },
        ))
        .child("Cancel")
        .child(key_cap("esc"));
    let go = div()
        .id("gate-go")
        .flex()
        .items_center()
        .gap(px(8.))
        .h(px(36.))
        .px(px(20.))
        .rounded(theme::CONTROL_RADIUS)
        .font_family(theme::ui_font())
        .child(commit_screen::gate_confirm_label(wording))
        .child(key_cap("\u{21b5}"));
    let go = if ready {
        go.bg(theme::ink())
            .text_color(theme::paper())
            .font_weight(FontWeight::MEDIUM)
            .cursor_pointer()
            .on_click(cx.listener(
                |this: &mut Shell, _: &ClickEvent, _: &mut Window, cx: &mut Context<Shell>| {
                    this.confirm_gate(cx);
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
                .child(hint.to_owned()),
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
