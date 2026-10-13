//! The History screen's file tools: blame of a file, a file as it was in the
//! selected commit, and the selected commit compared with a branch or tag.

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use gpui::{Context, ScrollStrategy, Window};
use rvw_git::blame::{self, BlameLine};
use rvw_git::history;
use rvw_view::diff::{parse, FileDiff, Hunk, Line, LineKind};
use rvw_view::history::short;
use rvw_view::keys::Screen;
use rvw_view::palette::Action;

use crate::git_tools::last_line;
use crate::history_state::{open_file, BlameView, CompareView, OpenCommitFile, Source, CONTEXT};
use crate::shell::Shell;

/// A tab's width in columns, as `code_scroll` counts it.
const TAB_COLUMNS: usize = 4;

impl Shell {
    /// Shows the blame of `path` (the file on disk) on the History screen, with
    /// the file's own history in the list. The blame is read off the UI thread.
    pub fn blame_file(&mut self, path: &str, cx: &mut Context<Self>) {
        self.show(Screen::History, cx);
        self.show_history_source(
            Source::File {
                path: path.to_owned(),
            },
            cx,
        );
        self.history.code_scroll_x = gpui::Pixels::ZERO;
        let request = self.history.blame_request;
        let stop = Arc::clone(&self.history.blame_stop);
        let dir = self.repo_path().to_path_buf();
        let path = path.to_owned();
        cx.spawn(async move |this, cx| {
            let read = cx
                .background_executor()
                .spawn(async move { read_blame(&dir, &path, &stop) })
                .await;
            this.update(cx, |this, cx| {
                if this.history.blame_request == request {
                    this.history.blame = Some(read);
                    this.history
                        .blame_scroll
                        .scroll_to_item(0, ScrollStrategy::Top);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Leaves the blame and shows the commit list again.
    pub(crate) fn close_blame(&mut self, cx: &mut Context<Self>) {
        self.history.cancel_blame();
        cx.notify();
    }

    /// Drops the comparison from the right panel.
    pub(crate) fn close_compare(&mut self, cx: &mut Context<Self>) {
        self.history.cancel_compare();
        cx.notify();
    }

    /// Selects the commit `id` of the blame in the list. A line not committed
    /// yet selects nothing; a commit the list has not read yet says so.
    pub(crate) fn select_blame_commit(&mut self, id: &str, cx: &mut Context<Self>) {
        if is_uncommitted(id) {
            return;
        }
        if self.history.history.select_id(id) {
            if let Some(index) = self.history.history.selected() {
                self.select_commit(index, cx);
            }
        } else {
            self.history.notice = Some(
                "That commit is further down the file's history; scroll to load it.".to_owned(),
            );
            cx.notify();
        }
    }

    /// Opens the picker of a History action: the files of the selected
    /// commit, or the branches and tags to compare it with.
    pub(crate) fn open_history_ask(
        &mut self,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.selected_commit_id() else {
            self.history.notice = Some("Select a commit first.".to_owned());
            cx.notify();
            return;
        };
        match action {
            Action::BrowseCommitFiles => match blame::tree(self.repo_path(), &id) {
                Ok(paths) => self.open_git_ask(action, paths, Vec::new(), window, cx),
                Err(e) => {
                    self.history.notice = Some(last_line(&e.stderr));
                    cx.notify();
                }
            },
            Action::CompareWith => {
                let facts = &self.git.facts;
                let items: Vec<String> =
                    facts.branches.iter().chain(&facts.tags).cloned().collect();
                self.open_git_ask(action, items, Vec::new(), window, cx);
            }
            _ => {}
        }
    }

    /// Runs the History picker's choice `label`. The line to show, or why it
    /// failed (shown in the picker).
    pub(crate) fn submit_history_ask(
        &mut self,
        action: Action,
        label: &str,
        cx: &mut Context<Self>,
    ) -> Result<String, String> {
        let Some(id) = self.selected_commit_id() else {
            return Err("Select a commit first.".to_owned());
        };
        let short_id = short(&id).to_owned();
        let line = match action {
            Action::BrowseCommitFiles => {
                let dir = self.repo_path().to_path_buf();
                let path = label.to_owned();
                self.spawn_open(move || read_file_at(&dir, &id, &path), cx);
                format!("Opening {label} as it was in {short_id}\u{2026}")
            }
            Action::CompareWith => {
                self.start_compare(label, id, cx);
                format!("Comparing {label} with {short_id}\u{2026}")
            }
            _ => return Err("Not a History action.".to_owned()),
        };
        self.history.notice = Some(line.clone());
        cx.notify();
        Ok(line)
    }

    /// Compares the commit `to` with `from` in the background; the files that
    /// differ are shown on the right when the read lands.
    fn start_compare(&mut self, from: &str, to: String, cx: &mut Context<Self>) {
        self.history.cancel_compare();
        let request = self.history.compare_request;
        let dir = self.repo_path().to_path_buf();
        let from = from.to_owned();
        cx.spawn(async move |this, cx| {
            let read = cx
                .background_executor()
                .spawn(async move {
                    history::compare(&dir, &from, &to)
                        .map(|files| CompareView { from, to, files })
                        .map_err(|e| last_line(&e.stderr))
                })
                .await;
            this.update(cx, |this, cx| {
                if this.history.compare_request == request {
                    this.history.compare = Some(read);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Opens `path` of the comparison in the centre: its diff between the
    /// two revisions.
    pub(crate) fn open_compare_file(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(Ok(view)) = self.history.compare.as_ref() else {
            return;
        };
        let (from, to) = (view.from.clone(), view.to.clone());
        let dir = self.repo_path().to_path_buf();
        let path = path.to_owned();
        self.spawn_open(move || read_compare_file(&dir, &from, &to, &path), cx);
    }
}

/// The blame of `path` on disk, with the columns of its longest line.
fn read_blame(dir: &Path, path: &str, stop: &AtomicBool) -> Result<BlameView, String> {
    let lines = blame::blame(dir, None, path, stop).map_err(|e| last_line(&e.stderr))?;
    Ok(BlameView {
        path: path.to_owned(),
        longest_line: longest_text(&lines),
        lines,
    })
}

/// The whole of `path` as it was in commit `id`. A file the commit does not
/// have is an error that names it.
fn read_file_at(dir: &Path, id: &str, path: &str) -> Result<OpenCommitFile, String> {
    match blame::file_at(dir, id, path).map_err(|e| last_line(&e.stderr))? {
        Some(content) => Ok(open_file(id, path, whole_file(&content, id))),
        None => Err(format!("{path} is not in {}", short(id))),
    }
}

/// The diff of `path` between the two revisions, parsed.
fn read_compare_file(
    dir: &Path,
    from: &str,
    to: &str,
    path: &str,
) -> Result<OpenCommitFile, String> {
    let raw =
        history::compare_diff(dir, from, to, path, CONTEXT).map_err(|e| last_line(&e.stderr))?;
    Ok(open_file(to, path, parse(&raw)))
}

/// `content` as one hunk of context lines, numbered from 1, under the header
/// "Whole file at <short id>". Nothing is counted as added or removed. Lines
/// split on '\n' only, as `diff::parse` splits, so a '\r' stays in the text.
pub(crate) fn whole_file(content: &str, commit: &str) -> FileDiff {
    let lines = if content.is_empty() {
        Vec::new()
    } else {
        let body = content.strip_suffix('\n').unwrap_or(content);
        body.split('\n')
            .enumerate()
            .map(|(index, text)| {
                let no = u32::try_from(index + 1).unwrap_or(u32::MAX);
                Line {
                    kind: LineKind::Context,
                    old_no: Some(no),
                    new_no: Some(no),
                    text: text.to_owned(),
                }
            })
            .collect()
    };
    FileDiff {
        hunks: vec![Hunk {
            header: format!("Whole file at {}", short(commit)),
            lines,
        }],
        binary: false,
        added: 0,
        removed: 0,
    }
}

/// Whether line `index` starts a run of consecutive lines from one commit.
/// The id, author and age of a run show only on its first line. False past
/// the end.
pub(crate) fn starts_run(lines: &[BlameLine], index: usize) -> bool {
    let Some(line) = lines.get(index) else {
        return false;
    };
    index == 0 || lines[index - 1].id != line.id
}

/// Whether `id` is all zeros, the id git gives a line not committed yet. An
/// empty id is not one.
pub(crate) fn is_uncommitted(id: &str) -> bool {
    !id.is_empty() && id.bytes().all(|b| b == b'0')
}

/// The columns of the longest line of `lines`, a tab taking four.
pub(crate) fn longest_text(lines: &[BlameLine]) -> usize {
    lines
        .iter()
        .map(|line| {
            line.text
                .chars()
                .map(|c| if c == '\t' { TAB_COLUMNS } else { 1 })
                .sum::<usize>()
        })
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blame_line(line: usize, id: &str, text: &str) -> BlameLine {
        BlameLine {
            line,
            id: id.to_owned(),
            author: String::new(),
            time: 0,
            summary: String::new(),
            text: text.to_owned(),
        }
    }

    fn texts(diff: &FileDiff) -> Vec<&str> {
        diff.hunks[0]
            .lines
            .iter()
            .map(|l| l.text.as_str())
            .collect()
    }

    #[test]
    fn whole_file_numbers_each_line_as_context() {
        let diff = whole_file("first\nsecond\n", "0123456789abcdef");
        assert_eq!(diff.hunks.len(), 1);
        assert_eq!(diff.hunks[0].header, "Whole file at 0123456");
        assert_eq!(texts(&diff), ["first", "second"]);
        for (index, line) in diff.hunks[0].lines.iter().enumerate() {
            let no = Some(u32::try_from(index + 1).unwrap());
            assert_eq!(line.kind, LineKind::Context);
            assert_eq!((line.old_no, line.new_no), (no, no));
        }
        assert_eq!((diff.added, diff.removed), (0, 0));
    }

    #[test]
    fn whole_file_keeps_a_carriage_return_and_a_missing_final_newline() {
        let diff = whole_file("a\r\nb", "abcdef0");
        assert_eq!(texts(&diff), ["a\r", "b"]);
    }

    #[test]
    fn whole_file_of_an_empty_file_has_no_lines() {
        assert!(whole_file("", "abcdef0").hunks[0].lines.is_empty());
        assert_eq!(texts(&whole_file("\n", "abcdef0")), [""]);
    }

    #[test]
    fn a_run_starts_where_the_commit_changes() {
        let lines = [
            blame_line(1, "aaa", ""),
            blame_line(2, "aaa", ""),
            blame_line(3, "bbb", ""),
            blame_line(4, "aaa", ""),
        ];
        let starts: Vec<bool> = (0..lines.len()).map(|i| starts_run(&lines, i)).collect();
        assert_eq!(starts, [true, false, true, true]);
        assert!(!starts_run(&lines, 4));
    }

    #[test]
    fn an_all_zero_id_is_uncommitted() {
        assert!(is_uncommitted(&"0".repeat(40)));
        assert!(!is_uncommitted(""));
        assert!(!is_uncommitted("0a0"));
        assert!(!is_uncommitted("0123456"));
    }

    #[test]
    fn the_longest_line_counts_a_tab_as_four_columns() {
        let lines = [blame_line(1, "a", "ab"), blame_line(2, "a", "\tx")];
        assert_eq!(longest_text(&lines), 5);
        assert_eq!(longest_text(&[]), 0);
    }
}
