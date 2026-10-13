//! The find-a-file picker: the repository's files ranked against the query,
//! each with the characters to highlight. The ranking is the fuzzy matcher's,
//! with changed files first on ties.

use std::cmp::Ordering;

use crate::files::{Mark, Marks};
use crate::fuzzy::{compare_candidates, fuzzy_match, Candidate};

/// The most rows the picker shows.
pub const MAX_RESULTS: usize = 12;

/// One row of the picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    /// The path relative to the workspace.
    pub path: String,
    /// The directories above the file, without a trailing slash; empty at the top.
    pub dir: String,
    /// The file name, the last path segment.
    pub name: String,
    /// Char indices into `name` of the characters the query matched.
    pub name_matches: Vec<usize>,
    pub mark: Mark,
}

/// A file that matched, before it is cut to a [`Hit`].
struct Entry {
    score: i64,
    changed: bool,
    index: usize,
    positions: Vec<usize>,
    candidate: Candidate,
}

/// The best [`MAX_RESULTS`] files for `query`, best first. An empty query
/// gives none: the picker lists files once the reviewer types.
#[must_use]
pub fn hits(query: &str, files: &[String], marks: &Marks) -> Vec<Hit> {
    if query.trim().is_empty() {
        return Vec::new();
    }
    let mut entries: Vec<Entry> = files
        .iter()
        .enumerate()
        .filter_map(|(index, path)| {
            let found = fuzzy_match(query, path)?;
            let mark = marks.get(path);
            Some(Entry {
                score: found.score,
                changed: mark.changed,
                index,
                positions: found.positions,
                candidate: Candidate {
                    label: path.clone(),
                    comments: mark.open_comments,
                },
            })
        })
        .collect();
    entries.sort_by(order);
    entries.truncate(MAX_RESULTS);
    entries
        .into_iter()
        .map(|entry| hit(&files[entry.index], marks, &entry.positions))
        .collect()
}

/// Best first: higher score, then changed files, then the fuzzy matcher's
/// tie-break (open comments, shorter path), then input order.
fn order(a: &Entry, b: &Entry) -> Ordering {
    b.score
        .cmp(&a.score)
        .then_with(|| b.changed.cmp(&a.changed))
        .then_with(|| compare_candidates(&a.candidate, &b.candidate))
        .then_with(|| a.index.cmp(&b.index))
}

/// Splits a path into its directory and name, and moves the matched positions
/// from path chars to name chars.
fn hit(path: &str, marks: &Marks, positions: &[usize]) -> Hit {
    let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
    let name_start = path.chars().count() - name.chars().count();
    let name_matches = positions
        .iter()
        .filter(|&&at| at >= name_start)
        .map(|at| at - name_start)
        .collect();
    Hit {
        path: path.to_owned(),
        dir: dir.to_owned(),
        name: name.to_owned(),
        name_matches,
        mark: marks.get(path),
    }
}

/// Splits `text` into runs of plain and matched chars, in order, for drawing
/// the matched chars in bold. `matches` are ascending char indices.
#[must_use]
pub fn runs(text: &str, matches: &[usize]) -> Vec<(String, bool)> {
    let mut out: Vec<(String, bool)> = Vec::new();
    for (at, c) in text.chars().enumerate() {
        let inside = matches.contains(&at);
        match out.last_mut() {
            Some((run, flag)) if *flag == inside => run.push(c),
            _ => out.push((c.to_string(), inside)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::changes::ChangedFile;

    fn files(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|p| (*p).to_owned()).collect()
    }

    fn changed(path: &str) -> ChangedFile {
        ChangedFile {
            path: path.to_owned(),
            status: 'M',
            open_comments: 0,
        }
    }

    #[test]
    fn a_hit_splits_the_name_from_its_directory_and_maps_the_matches() {
        let all = files(&["crates/rvw-core/src/service.rs"]);
        let found = hits("serv", &all, &Marks::default());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].dir, "crates/rvw-core/src");
        assert_eq!(found[0].name, "service.rs");
        assert_eq!(found[0].name_matches, [0, 1, 2, 3]);
    }

    #[test]
    fn matches_inside_the_directory_leave_no_name_highlight() {
        let all = files(&["crates/lib.rs"]);
        let found = hits("crate", &all, &Marks::default());
        assert!(found[0].name_matches.is_empty());
    }

    #[test]
    fn a_top_level_file_has_an_empty_directory() {
        let all = files(&["Cargo.toml"]);
        let found = hits("cargo", &all, &Marks::default());
        assert_eq!(found[0].dir, "");
        assert_eq!(found[0].name, "Cargo.toml");
        assert_eq!(found[0].name_matches, [0, 1, 2, 3, 4]);
    }

    #[test]
    fn a_better_match_ranks_first() {
        let all = files(&["docs/service-notes.md", "crates/service.rs"]);
        let found = hits("service", &all, &Marks::default());
        assert_eq!(found[0].path, "crates/service.rs");
    }

    #[test]
    fn on_a_tie_the_changed_file_comes_first() {
        let all = files(&["a/zz.rs", "b/zz.rs"]);
        let marks = Marks::new(&[changed("b/zz.rs")], &[]);
        let found = hits("zz", &all, &marks);
        assert_eq!(found[0].path, "b/zz.rs");
        assert!(found[0].mark.changed);
    }

    #[test]
    fn an_empty_query_lists_no_file() {
        let all = files(&["a.rs", "b.rs", "c.rs"]);
        let marks = Marks::new(&[changed("c.rs")], &[]);
        assert!(hits("", &all, &marks).is_empty());
        assert!(hits("  ", &all, &marks).is_empty());
    }

    #[test]
    fn a_query_every_file_matches_lists_the_changed_files_first() {
        let all = files(&["a.rs", "b.rs", "c.rs"]);
        let marks = Marks::new(&[changed("c.rs")], &[]);
        let found = hits(".rs", &all, &marks);
        let order: Vec<&str> = found.iter().map(|h| h.path.as_str()).collect();
        assert_eq!(order, ["c.rs", "a.rs", "b.rs"]);
    }

    #[test]
    fn at_most_twelve_hits_are_returned() {
        let names: Vec<String> = (0..30).map(|n| format!("f{n}.rs")).collect();
        assert_eq!(hits("f", &names, &Marks::default()).len(), MAX_RESULTS);
    }

    #[test]
    fn a_query_with_no_match_gives_no_hits() {
        let all = files(&["a.rs"]);
        assert!(hits("zzz", &all, &Marks::default()).is_empty());
    }

    #[test]
    fn runs_group_matched_and_plain_chars() {
        assert_eq!(
            runs("service.rs", &[0, 1, 2]),
            [("ser".to_owned(), true), ("vice.rs".to_owned(), false)]
        );
        assert_eq!(runs("abc", &[]), [("abc".to_owned(), false)]);
        assert!(runs("", &[]).is_empty());
    }
}
