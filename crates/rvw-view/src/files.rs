//! What the file tree and the finder show about each file of the repository:
//! whether the change touches it, and how many comments are open on it. The
//! comments can sit on files outside the change, so the counts come from
//! every comment, not from the changed files alone.

use std::collections::{HashMap, HashSet};

use rvw_core::types::{Comment, Status};

use crate::changes::ChangedFile;

/// What is known about one file of the repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Mark {
    /// The file is in the change.
    pub changed: bool,
    /// Comments on the file that are pending or pulled.
    pub open_comments: usize,
}

/// The marks of every file, looked up by path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Marks {
    changed: HashSet<String>,
    open: HashMap<String, usize>,
}

impl Marks {
    /// Joins the changed files with the open comments of each file.
    #[must_use]
    pub fn new(changes: &[ChangedFile], comments: &[Comment]) -> Self {
        let changed_paths = changes.iter().map(|file| file.path.clone()).collect();
        let mut open = HashMap::new();
        for comment in comments {
            if matches!(comment.status, Status::Pending | Status::Pulled) {
                *open.entry(comment.file.clone()).or_insert(0) += 1;
            }
        }
        Self {
            changed: changed_paths,
            open,
        }
    }

    /// The mark of `path`; a file with no change and no comment has the default.
    #[must_use]
    pub fn get(&self, path: &str) -> Mark {
        Mark {
            changed: self.changed.contains(path),
            open_comments: self.open.get(path).copied().unwrap_or(0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn changed(path: &str) -> ChangedFile {
        ChangedFile {
            path: path.to_owned(),
            status: 'M',
            open_comments: 0,
        }
    }

    fn comment(file: &str, status: Status) -> Comment {
        Comment {
            points_at: None,
            reopens: None,
            verdict: None,
            id: "r1".into(),
            status,
            workspace: "/ws".into(),
            lane: None,
            author: None,
            file: file.into(),
            path: format!("/ws/{file}"),
            start_line: 1,
            end_line: 1,
            filetype: "rust".into(),
            code: String::new(),
            comment: "note".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            pulled_at: None,
            resolved_at: None,
            resolved_by: None,
            resolution_note: None,
            file_version: String::new(),
            resolved_file_version: String::new(),
            review_id: String::new(),
            edited_at: String::new(),
        }
    }

    #[test]
    fn a_changed_file_is_marked_and_a_context_file_is_not() {
        let marks = Marks::new(&[changed("src/a.rs")], &[]);
        assert!(marks.get("src/a.rs").changed);
        assert!(!marks.get("src/b.rs").changed);
    }

    #[test]
    fn open_comments_count_on_files_inside_and_outside_the_change() {
        let comments = [
            comment("src/a.rs", Status::Pending),
            comment("src/a.rs", Status::Pulled),
            comment("docs/x.md", Status::Pending),
        ];
        let marks = Marks::new(&[changed("src/a.rs")], &comments);
        assert_eq!(marks.get("src/a.rs").open_comments, 2);
        assert_eq!(marks.get("docs/x.md").open_comments, 1);
        assert!(!marks.get("docs/x.md").changed);
    }

    #[test]
    fn done_and_rejected_comments_are_not_open() {
        let comments = [
            comment("a.rs", Status::Done),
            comment("a.rs", Status::Rejected),
        ];
        assert_eq!(Marks::new(&[], &comments).get("a.rs").open_comments, 0);
    }

    #[test]
    fn an_unknown_path_has_the_default_mark() {
        assert_eq!(Marks::new(&[], &[]).get("nope"), Mark::default());
    }
}
