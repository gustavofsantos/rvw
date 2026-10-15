//! "Why this line" above the composer: the summary of the reported session
//! that last changed the lines a new comment is written on. The blame and the
//! commit markers are read off the UI thread; the choice is `rvw_view`'s.
//! A failed blame, an untracked file or no reported owner shows nothing.

use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::AtomicBool;

use gpui::prelude::*;
use gpui::{Context, Div, div, px};
use rvw_core::types::Session;
use rvw_git::blame::{self, BlameLine};
use rvw_git::history;
use rvw_view::why_line::{WhyLine, text, why_line};

use crate::data;
use crate::shell::Shell;
use crate::theme;

impl Shell {
    /// Reads the "why this line" of a new comment's range in the background,
    /// into the open composer. Does nothing for an edit or without a range.
    pub(crate) fn load_why_line(&mut self, cx: &mut Context<Self>) {
        let Some(composer) = self.composer.as_ref() else {
            return;
        };
        if composer.editing.is_some() {
            return;
        }
        let Some(selected) = self.selected.as_ref() else {
            return;
        };
        let Some((lo, hi)) = selected
            .diff
            .as_ref()
            .ok()
            .and_then(|diff| selected.selection.lines(diff))
        else {
            return;
        };
        let input = composer.input.entity_id();
        let sessions = data::sessions(&self.service, &self.identity.workspace).unwrap_or_default();
        let dir = self.repo_path().to_path_buf();
        let path = selected.path.clone();
        cx.spawn(async move |this, cx| {
            let read = cx
                .background_executor()
                .spawn(async move { read(&dir, &path, (lo, hi), &sessions) })
                .await;
            this.update(cx, |this, cx| {
                // The composer may have closed, or another one replaced it.
                if let Some(composer) = this
                    .composer
                    .as_mut()
                    .filter(|composer| composer.input.entity_id() == input)
                {
                    composer.why = read;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }
}

/// The reported session behind the lines `lo..=hi` of `path` on disk, or None.
/// Blame sees uncommitted lines too; they have no commit and so no owner.
fn read(dir: &Path, path: &str, (lo, hi): (u32, u32), sessions: &[Session]) -> Option<WhyLine> {
    let stop = AtomicBool::new(false);
    let blamed = blame::blame(dir, None, path, &stop).ok()?;
    let commits = commits_in_range(&blamed, (lo, hi));
    let markers: Vec<Vec<String>> = commits
        .iter()
        .map(|id| {
            // A marker only matters when no report lists the commit; a failed
            // read leaves the commit without markers, which is never a guess.
            history::show(dir, id)
                .map(|detail| detail.entry.session_markers)
                .unwrap_or_default()
        })
        .collect();
    why_line(&commits, &markers, sessions)
}

/// The distinct commits of the lines `lo..=hi`, oldest first by author time,
/// then by the first of their lines in the range. Uncommitted lines are left out.
fn commits_in_range(blamed: &[BlameLine], (lo, hi): (u32, u32)) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut found: Vec<(i64, usize, String)> = Vec::new();
    for line in blamed
        .iter()
        .filter(|line| (lo as usize..=hi as usize).contains(&line.line))
    {
        let uncommitted = line.id.bytes().all(|byte| byte == b'0');
        if !uncommitted && seen.insert(line.id.clone()) {
            found.push((line.time, line.line, line.id.clone()));
        }
    }
    found.sort();
    found.into_iter().map(|(_, _, id)| id).collect()
}

/// The muted line drawn above the composer text: `tool · model — "summary"`.
#[must_use]
pub fn row(line: &WhyLine) -> Div {
    div()
        .mx(px(24.))
        .px(px(14.))
        .pb(px(4.))
        .truncate()
        .text_size(px(theme::KEY_FONT_SIZE + 0.5))
        .text_color(theme::faint())
        .child(text(line))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(number: usize, id: &str, time: i64) -> BlameLine {
        BlameLine {
            line: number,
            id: id.to_owned(),
            author: String::new(),
            time,
            summary: String::new(),
            text: String::new(),
        }
    }

    #[test]
    fn commits_in_range_are_distinct_oldest_first_without_uncommitted_lines() {
        let zeros = "0".repeat(40);
        let blamed = [
            line(1, "newer", 300),
            line(2, "older", 100),
            line(3, &zeros, 0),
            line(4, "newer", 300),
            line(5, "outside", 50),
        ];
        assert_eq!(commits_in_range(&blamed, (1, 4)), ["older", "newer"]);
        assert_eq!(commits_in_range(&blamed, (3, 3)), Vec::<String>::new());
    }

    #[test]
    fn equal_times_keep_the_earlier_line_first() {
        let blamed = [line(2, "second", 100), line(1, "first", 100)];
        assert_eq!(commits_in_range(&blamed, (1, 2)), ["first", "second"]);
    }
}
