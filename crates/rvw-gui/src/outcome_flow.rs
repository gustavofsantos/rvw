//! The outcome screen's behaviour: opening a comment, going back, stepping
//! through the review's comments, and giving a verdict on a decision. Opening
//! reads the comment off the UI thread, and so does a verdict; the rules are
//! in `rvw_view` and in the service.

use std::sync::Arc;

use gpui::{AppContext, Context, Window};
use rvw_core::operations::{GetInput, VerdictInput};
use rvw_core::types::{Actor, VerdictKind};
use rvw_core::Reviewer;
use rvw_view::keys::Screen;
use rvw_view::nav;
use rvw_view::outcome::{self, Moment, Story};

use crate::outcome_screen::{self, Outcome, ReopenNote};
use crate::shell::Shell;
use crate::text_area::{TextArea, TextAreaEvent};

/// Why a verdict is refused when the window has no author to give it.
pub const NEEDS_AUTHOR: &str = "set --author to judge a comment";

impl Shell {
    /// Opens the outcome of comment `id`. The comment and its evidence (the
    /// code before and after, read from git) are read off the UI thread; the
    /// screen switches when they arrive, unless another comment was opened
    /// meanwhile. Opening a decided comment does not close it: only a verdict
    /// does.
    pub fn open_outcome(&mut self, id: &str, cx: &mut Context<Self>) {
        self.outcome_pending = Some(id.to_owned());
        let service = Arc::clone(&self.service);
        let input = GetInput {
            workspace: self.identity.workspace.clone(),
            id: id.to_owned(),
        };
        let id = id.to_owned();
        cx.spawn(async move |this, cx| {
            let read = cx
                .background_executor()
                .spawn(async move {
                    service.evidence(input).map(|evidence| {
                        let timeline = outcome::timeline(&evidence.comment);
                        (outcome::story(&evidence), timeline)
                    })
                })
                .await;
            this.update(cx, |this, cx| this.show_outcome(&id, read, cx))
                .ok();
        })
        .detach();
    }

    /// Shows what `open_outcome` read for `id`, when it is still the comment
    /// asked for last. The error of a verdict on the same comment stays, so a
    /// refused verdict or failed reload is still shown after the read.
    fn show_outcome(
        &mut self,
        id: &str,
        read: Result<(Story, Vec<Moment>), rvw_core::Error>,
        cx: &mut Context<Self>,
    ) {
        if self.outcome_pending.as_deref() != Some(id) {
            return;
        }
        self.outcome_pending = None;
        let back = match self.outcome.as_ref() {
            Some(open) if self.screen == Screen::Outcome => open.back,
            _ => self.screen,
        };
        let kept = self
            .outcome
            .as_ref()
            .filter(|open| open.id == id)
            .and_then(|open| open.error.clone());
        let (story, timeline) = match read {
            Ok((story, timeline)) => (Ok(story), timeline),
            Err(error) => (Err(error.message), Vec::new()),
        };
        self.outcome = Some(Outcome {
            id: id.to_owned(),
            back,
            story,
            timeline,
            error: kept,
            reopen: None,
            judging: false,
        });
        self.screen = Screen::Outcome;
        self.removal = None;
        cx.notify();
    }

    /// Returns from the outcome screen to the screen it was opened from.
    pub fn close_outcome(&mut self, cx: &mut Context<Self>) {
        self.outcome_pending = None;
        if let Some(open) = self.outcome.take() {
            self.screen = open.back;
        }
        self.removal = None;
        cx.notify();
    }

    /// Moves the outcome screen to the next or previous comment of its review.
    pub(crate) fn step_outcome(&mut self, delta: i32, cx: &mut Context<Self>) {
        // Keys pressed faster than comments load step from the one asked for.
        let asked = self.outcome_pending.clone();
        let Some(current) = asked.or_else(|| self.outcome.as_ref().map(|open| open.id.clone()))
        else {
            return;
        };
        let ids: Vec<String> = outcome::group_of(&self.snapshot.queue, &current)
            .map(|group| group.rows.iter().map(|row| row.id.clone()).collect())
            .unwrap_or_default();
        let index = ids.iter().position(|id| *id == current);
        if let Some(next) = nav::step(ids.len(), index, delta).and_then(|i| ids.get(i)) {
            let next = next.clone();
            self.open_outcome(&next, cx);
        }
    }

    /// Whether the outcome screen shows a comment that awaits a verdict.
    pub(crate) fn can_judge(&self) -> bool {
        self.screen == Screen::Outcome && self.outcome.as_ref().is_some_and(Outcome::awaits_verdict)
    }

    /// Whether the reopen note field is open on the outcome screen, so keys go
    /// to it. The outcome stays in place after another screen is shown, so the
    /// screen is checked too.
    pub(crate) fn reopening(&self) -> bool {
        self.screen == Screen::Outcome
            && self
                .outcome
                .as_ref()
                .is_some_and(|open| open.reopen.is_some())
    }

    /// Opens the note field a reopen needs, with focus in it. Does nothing
    /// unless the open comment awaits a verdict.
    pub fn open_reopen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_judge() || self.reopening() {
            return;
        }
        let input = cx.new(|cx| TextArea::new("What is still wrong?", cx).with_lines(3, 8));
        let events = cx.subscribe(
            &input,
            |this: &mut Self, _, event: &TextAreaEvent, cx: &mut Context<Self>| match event {
                TextAreaEvent::Submit => this.judge_outcome(VerdictKind::Reopened, cx),
                TextAreaEvent::Cancel => this.close_reopen(cx),
            },
        );
        // Typing changes whether Reopen is enabled, so the shell redraws.
        let typing = cx.observe(&input, |_: &mut Self, _, cx: &mut Context<Self>| {
            cx.notify();
        });
        input.update(cx, |area, cx| area.focus(window, cx));
        if let Some(open) = self.outcome.as_mut() {
            open.reopen = Some(ReopenNote::new(input, events, typing));
        }
        cx.notify();
    }

    /// Closes the reopen note field, dropping what was typed.
    pub fn close_reopen(&mut self, cx: &mut Context<Self>) {
        if let Some(open) = self.outcome.as_mut() {
            open.reopen = None;
        }
        cx.notify();
    }

    /// Accepts the decision on the open comment, or reopens it with the note
    /// in the field. The call runs off the UI thread, as the human at this
    /// window. The human who owns the queue may judge a decision they made
    /// themselves (a legacy comment an agent recorded under their name); the
    /// CLI and MCP keep the second-reviewer rule. On success the queue reads again and the outcome is read again
    /// to show the verdict; on failure the message shows in the right panel.
    pub fn judge_outcome(&mut self, kind: VerdictKind, cx: &mut Context<Self>) {
        if !self.can_judge() {
            return;
        }
        let (id, note) = {
            let Some(open) = self.outcome.as_ref() else {
                return;
            };
            if open.judging {
                return;
            }
            let note = match kind {
                VerdictKind::Accepted => String::new(),
                VerdictKind::Reopened => {
                    let Some(reopen) = open.reopen.as_ref() else {
                        return;
                    };
                    reopen.input.read(cx).text().trim().to_owned()
                }
            };
            if kind == VerdictKind::Reopened && !outcome_screen::reopen_ready(&note) {
                return;
            }
            (open.id.clone(), note)
        };
        if self.identity.author.trim().is_empty() {
            self.judge_failed(&id, NEEDS_AUTHOR.to_owned(), cx);
            return;
        }
        let input = VerdictInput {
            workspace: self.identity.workspace.clone(),
            id: id.clone(),
            verdict: kind.as_str().to_owned(),
            note,
            author: self.identity.author.clone(),
            ..VerdictInput::default()
        };
        if let Some(open) = self.outcome.as_mut() {
            open.judging = true;
            open.error = None;
        }
        cx.notify();
        let service = Arc::clone(&self.service);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    service
                        .judge_as(input, Actor::Human, Reviewer::Anyone)
                        .map(|given| given.comment.id)
                        .map_err(|e| e.message)
                })
                .await;
            this.update(cx, |this, cx| this.finish_judge(&id, result, cx))
                .ok();
        })
        .detach();
    }

    /// Shows what a verdict call returned for comment `id`. A saved verdict
    /// reads the queue again, so its counters drop, and the outcome again, so
    /// it shows the verdict. A refused one shows its message.
    fn finish_judge(&mut self, id: &str, result: Result<String, String>, cx: &mut Context<Self>) {
        match result {
            Ok(_) => {
                if let Err(message) = self.refresh() {
                    self.judge_failed(
                        id,
                        format!("Saved, but the queue did not reload: {message}"),
                        cx,
                    );
                }
                if self.outcome.as_ref().is_some_and(|open| open.id == id) {
                    self.open_outcome(id, cx);
                }
            }
            Err(message) => self.judge_failed(id, message, cx),
        }
    }

    /// Stops the verdict wait and shows `message` in the right panel, when the
    /// comment is still open.
    fn judge_failed(&mut self, id: &str, message: String, cx: &mut Context<Self>) {
        if let Some(open) = self.outcome.as_mut().filter(|open| open.id == id) {
            open.judging = false;
            open.error = Some(message);
        }
        cx.notify();
    }
}
