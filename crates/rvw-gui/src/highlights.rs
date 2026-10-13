//! Syntax colour for the open file, computed off the UI thread.
//!
//! Tokenizing a long file takes about a second, so the window never waits for
//! it: the file opens as plain text, a background task tokenizes a copy of its
//! diff, and the spans land in one shared cell the visible rows read. Filling
//! the cell needs a repaint and nothing else: row heights do not change, so
//! the list keeps its scroll position and its measured rows.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{Context, HighlightStyle, Hsla};
use rvw_view::diff::FileDiff;
use rvw_view::highlight::{self, Class, Span};

use crate::shell::Shell;
use crate::theme;

/// The spans of a file, indexed `[hunk][line]`.
pub type Spans = Vec<Vec<Vec<Span>>>;

/// The cell the diff rows read; empty until the background task finishes.
pub type Shared = Rc<RefCell<Option<Rc<Spans>>>>;

/// What the shell remembers about the file being highlighted.
#[derive(Default)]
pub struct Highlighter {
    pub shared: Shared,
    /// The shell's `diff_generation` this state was last compared at.
    generation: Option<u64>,
    path: String,
    diff: Option<Rc<FileDiff>>,
    /// Bumped for each task started; a result with another token is stale.
    token: u64,
}

/// The colour runs of one line: its coloured spans as byte ranges. Empty when
/// the line has nothing but ink, which keeps it a plain string element.
pub fn runs(spans: &[Span]) -> Vec<(std::ops::Range<usize>, HighlightStyle)> {
    spans
        .iter()
        .filter_map(|span| {
            colour(span.class).map(|color| {
                (
                    span.start..span.end,
                    HighlightStyle {
                        color: Some(color),
                        ..HighlightStyle::default()
                    },
                )
            })
        })
        .collect()
}

fn colour(class: Class) -> Option<Hsla> {
    theme::syntax(class).map(Hsla::from)
}

impl Shell {
    /// Starts tokenizing when the open file's diff is new. Called by every
    /// render; the usual cost is one integer comparison.
    pub(crate) fn sync_highlights(&mut self, cx: &mut Context<Self>) {
        let state = &mut self.highlights;
        if state.generation == Some(self.diff_generation) {
            return;
        }
        state.generation = Some(self.diff_generation);
        let Some(selected) = self.selected.as_ref() else {
            reset(state, "", None);
            return;
        };
        let Ok(diff) = &selected.diff else {
            reset(state, "", None);
            return;
        };
        // A reload after a comment re-reads the same file: keep its colours.
        let same =
            state.path == selected.path && state.diff.as_ref().is_some_and(|old| **old == **diff);
        if same {
            return;
        }
        reset(state, &selected.path, Some(Rc::clone(diff)));
        let lines: usize = diff.hunks.iter().map(|hunk| hunk.lines.len()).sum();
        if !highlight::worth_highlighting(lines) {
            return;
        }
        state.token += 1;
        let token = state.token;
        let name = selected.path.clone();
        let copy = (**diff).clone();
        let outside = selected.outside;
        cx.spawn(async move |this, cx| {
            let spans = cx
                .background_executor()
                .spawn(async move {
                    if outside {
                        highlight::unchanged(&name, &copy)
                    } else {
                        highlight::diff(&name, &copy)
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                if this.highlights.token == token {
                    *this.highlights.shared.borrow_mut() = Some(Rc::new(spans));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }
}

/// Forgets the previous file's spans and invalidates its running task.
fn reset(state: &mut Highlighter, path: &str, diff: Option<Rc<FileDiff>>) {
    state.token += 1;
    path.clone_into(&mut state.path);
    state.diff = diff;
    *state.shared.borrow_mut() = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_and_quiet_classes_make_no_runs() {
        let spans = [
            Span {
                start: 0,
                end: 3,
                class: Class::Plain,
            },
            Span {
                start: 3,
                end: 5,
                class: Class::Punctuation,
            },
        ];
        assert!(runs(&spans).is_empty());
    }

    #[test]
    fn a_keyword_is_one_coloured_run() {
        let spans = [
            Span {
                start: 0,
                end: 2,
                class: Class::Keyword,
            },
            Span {
                start: 2,
                end: 6,
                class: Class::Plain,
            },
        ];
        let found = runs(&spans);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, 0..2);
        assert!(found[0].1.color.is_some());
    }
}
