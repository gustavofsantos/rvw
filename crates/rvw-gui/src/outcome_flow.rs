//! The outcome screen's behaviour: opening a comment, marking a decided one
//! seen, going back, and stepping through the review's comments. Each step is
//! one service call or one reload; the rules are in `rvw_view` and `seen`.

use gpui::Context;
use rvw_core::operations::GetInput;
use rvw_core::types::Status;
use rvw_view::keys::Screen;
use rvw_view::nav;
use rvw_view::outcome::{self, Story};

use crate::outcome_screen::Outcome;
use crate::shell::Shell;

impl Shell {
    /// Opens the outcome of comment `id`. A done or rejected comment is marked
    /// seen first, and the snapshot reloads so the waiting counter drops. A
    /// failure to mark it is shown beside the story, which still opens.
    pub fn open_outcome(&mut self, id: &str, cx: &mut Context<Self>) {
        let back = match self.outcome.as_ref() {
            Some(open) if self.screen == Screen::Outcome => open.back,
            _ => self.screen,
        };
        let input = GetInput {
            workspace: self.identity.workspace.clone(),
            id: id.to_owned(),
        };
        let (story, timeline, error) = match self.service.evidence(input) {
            Ok(evidence) => {
                let story = outcome::story(&evidence);
                let timeline = outcome::timeline(&evidence.comment);
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
        cx.notify();
    }

    /// Records a decided comment as seen and reloads. Returns the message to
    /// show when that fails.
    fn mark_seen(&mut self, story: &Story) -> Option<String> {
        let decided = matches!(story.status, Status::Done | Status::Rejected);
        if !decided || self.seen.contains(&self.identity.workspace, &story.id) {
            return None;
        }
        let workspace = self.identity.workspace.clone();
        if let Err(message) = self.seen.mark(&workspace, &story.id) {
            return Some(message);
        }
        self.refresh()
            .err()
            .map(|message| format!("Saved, but the view did not reload: {message}"))
    }

    /// Returns from the outcome screen to the screen it was opened from.
    pub fn close_outcome(&mut self, cx: &mut Context<Self>) {
        if let Some(open) = self.outcome.take() {
            self.screen = open.back;
        }
        cx.notify();
    }

    /// Moves the outcome screen to the next or previous comment of its review.
    pub(crate) fn step_outcome(&mut self, delta: i32, cx: &mut Context<Self>) {
        let Some(current) = self.outcome.as_ref().map(|open| open.id.clone()) else {
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
