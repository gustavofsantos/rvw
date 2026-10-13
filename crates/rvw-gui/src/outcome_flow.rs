//! The outcome screen's behaviour: opening a comment, marking a decided one
//! seen, going back, and stepping through the review's comments. Opening reads
//! the comment off the UI thread; the rules are in `rvw_view` and `seen`.

use std::sync::Arc;

use gpui::Context;
use rvw_core::operations::GetInput;
use rvw_core::types::Status;
use rvw_view::keys::Screen;
use rvw_view::nav;
use rvw_view::outcome::{self, Moment, Story};

use crate::outcome_screen::Outcome;
use crate::shell::Shell;

impl Shell {
    /// Opens the outcome of comment `id`. The comment and its evidence (the
    /// code before and after, read from git) are read off the UI thread; the
    /// screen switches when they arrive, unless another comment was opened
    /// meanwhile. A done or rejected comment is marked seen, so the waiting
    /// counter drops. A failure to mark it is shown beside the story, which
    /// still opens.
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
    /// asked for last.
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
        let (story, timeline, error) = match read {
            Ok((story, timeline)) => {
                let error = self.mark_seen(&story);
                (Ok(story), timeline, error)
            }
            Err(error) => (Err(error.message), Vec::new(), None),
        };
        self.outcome = Some(Outcome {
            id: id.to_owned(),
            back,
            story,
            timeline,
            error,
        });
        self.screen = Screen::Outcome;
        self.removal = None;
        cx.notify();
    }

    /// Records a decided comment as seen and counts it so. Returns the
    /// message to show when that fails.
    fn mark_seen(&mut self, story: &Story) -> Option<String> {
        let decided = matches!(story.status, Status::Done | Status::Rejected);
        if !decided || self.seen.contains(&self.identity.workspace, &story.id) {
            return None;
        }
        let workspace = self.identity.workspace.clone();
        if let Err(message) = self.seen.mark(&workspace, &story.id) {
            return Some(message);
        }
        self.acknowledge(&story.id);
        None
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
}
