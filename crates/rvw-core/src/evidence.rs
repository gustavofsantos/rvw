//! Port of internal/review/evidence.go: the code snapshot's filetype and the
//! unified diff between a reviewed and a resolved file version.

use std::path::Path;

use similar::{DiffTag, TextDiff};

use crate::types::Comment;

/// Lines of unchanged context around each change, as difflib is called with.
const CONTEXT: usize = 3;

/// The fence language of each known extension (lowercase, with the dot).
const FILETYPES: &[(&str, &str)] = &[
    (".bash", "bash"),
    (".c", "c"),
    (".clj", "clojure"),
    (".cljc", "clojure"),
    (".cljs", "clojure"),
    (".cpp", "cpp"),
    (".css", "css"),
    (".edn", "clojure"),
    (".ex", "elixir"),
    (".exs", "elixir"),
    (".go", "go"),
    (".h", "c"),
    (".hs", "haskell"),
    (".html", "html"),
    (".java", "java"),
    (".js", "javascript"),
    (".json", "json"),
    (".jsx", "javascript"),
    (".lua", "lua"),
    (".md", "markdown"),
    (".py", "python"),
    (".rb", "ruby"),
    (".rs", "rust"),
    (".scm", "scheme"),
    (".sh", "bash"),
    (".sql", "sql"),
    (".toml", "toml"),
    (".ts", "typescript"),
    (".tsx", "typescript"),
    (".vim", "vim"),
    (".yaml", "yaml"),
    (".yml", "yaml"),
    (".zsh", "bash"),
];

/// The fence language for `path`, guessed from its extension. Like Go's
/// `filepath.Ext`, a dot anywhere in the final path element starts the
/// extension (so `.bashrc` has the extension `.bashrc`). Empty when unknown.
pub(crate) fn filetype_of(path: &str) -> String {
    let base = path.rsplit('/').next().unwrap_or(path);
    let ext = match base.rfind('.') {
        Some(i) => base[i..].to_lowercase(),
        None => return String::new(),
    };
    FILETYPES
        .iter()
        .find(|(e, _)| *e == ext)
        .map(|(_, t)| (*t).to_string())
        .unwrap_or_default()
}

/// The unified diff between the file as reviewed and as resolved, read back
/// from the git object store. The bool is false when either version is gone
/// or was never kept; the lines are then empty.
pub(crate) fn snapshot_diff(c: &Comment) -> (Vec<String>, bool) {
    if c.file_version.is_empty() || c.resolved_file_version.is_empty() {
        return (Vec::new(), false);
    }
    let ws = Path::new(&c.workspace);
    let before = rvw_git::gitx::blob(ws, &c.file_version);
    let after = rvw_git::gitx::blob(ws, &c.resolved_file_version);
    let (Some(before), Some(after)) = (before, after) else {
        return (Vec::new(), false);
    };
    let lines = unified_diff(
        &with_newlines(&before),
        &with_newlines(&after),
        &format!("a/{}", c.file),
        &format!("b/{}", c.file),
    );
    (lines, true)
}

/// Splits text into lines that each end in `\n`, the shape difflib expects; a
/// missing final newline is not reported as a change.
pub(crate) fn with_newlines(text: &str) -> Vec<String> {
    crate::service::split_source(&text.replace("\r\n", "\n"))
        .into_iter()
        .map(|line| format!("{line}\n"))
        .collect()
}

/// Renders the grouped changes the way go-difflib's `GetUnifiedDiffString`
/// does, one output line per entry, without line terminators. Identical inputs
/// yield nothing. The grouping comes from `similar`; the text is formatted here.
pub(crate) fn unified_diff(a: &[String], b: &[String], from: &str, to: &str) -> Vec<String> {
    let old: String = a.concat();
    let new: String = b.concat();
    let groups: Vec<_> = TextDiff::from_lines(&old, &new)
        .grouped_ops(CONTEXT)
        .into_iter()
        .filter(|g| !(g.len() == 1 && g[0].as_tag_tuple().0 == DiffTag::Equal))
        .collect();
    if groups.is_empty() {
        return Vec::new();
    }
    let mut out = vec![format!("--- {from}"), format!("+++ {to}")];
    for group in &groups {
        let (_, first_old, first_new) = group[0].as_tag_tuple();
        let (_, last_old, last_new) = group[group.len() - 1].as_tag_tuple();
        out.push(format!(
            "@@ -{} +{} @@",
            format_range(first_old.start, last_old.end),
            format_range(first_new.start, last_new.end)
        ));
        for op in group {
            let (tag, old_range, new_range) = op.as_tag_tuple();
            match tag {
                DiffTag::Equal => push_lines(&mut out, " ", &a[old_range]),
                DiffTag::Delete => push_lines(&mut out, "-", &a[old_range]),
                DiffTag::Insert => push_lines(&mut out, "+", &b[new_range]),
                DiffTag::Replace => {
                    push_lines(&mut out, "-", &a[old_range]);
                    push_lines(&mut out, "+", &b[new_range]);
                }
            }
        }
    }
    out
}

fn push_lines(out: &mut Vec<String>, prefix: &str, lines: &[String]) {
    for line in lines {
        out.push(format!(
            "{prefix}{}",
            line.strip_suffix('\n').unwrap_or(line)
        ));
    }
}

/// difflib's `formatRangeUnified`: `start,length`, with a lone line as `start`
/// and an empty range as the line before it.
fn format_range(start: usize, stop: usize) -> String {
    let mut beginning = start + 1;
    let length = stop - start;
    if length == 1 {
        return beginning.to_string();
    }
    if length == 0 {
        beginning -= 1;
    }
    format!("{beginning},{length}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        with_newlines(text)
    }

    #[test]
    fn filetype_follows_the_extension_map() {
        assert_eq!(filetype_of("/w/app.PY"), "python");
        assert_eq!(filetype_of("/w/main.tsx"), "typescript");
        assert_eq!(filetype_of("/w/.zshrc"), "");
        assert_eq!(filetype_of("/w/.zsh"), "bash");
        assert_eq!(filetype_of("/w/Makefile"), "");
        assert_eq!(filetype_of("/w.d/Makefile"), "");
    }

    #[test]
    fn a_trailing_newline_is_not_a_line() {
        assert_eq!(with_newlines("a\nb\n"), vec!["a\n", "b\n"]);
        assert_eq!(with_newlines("a\nb"), vec!["a\n", "b\n"]);
        assert!(with_newlines("").is_empty());
        assert_eq!(with_newlines("a\r\nb\r\n"), vec!["a\n", "b\n"]);
    }

    #[test]
    fn identical_text_has_no_diff() {
        assert!(unified_diff(&lines("x\ny\n"), &lines("x\ny\n"), "a/f", "b/f").is_empty());
    }

    #[test]
    fn a_change_is_one_hunk_with_context() {
        let a = lines("one\ntwo\nthree\nfour\n");
        let b = lines("one\nchanged-two\nthree\nfour\n");
        let diff = unified_diff(&a, &b, "a/app.py", "b/app.py");
        assert_eq!(
            diff,
            vec![
                "--- a/app.py",
                "+++ b/app.py",
                "@@ -1,4 +1,4 @@",
                " one",
                "-two",
                "+changed-two",
                " three",
                " four",
            ]
        );
    }

    #[test]
    fn range_format_matches_difflib() {
        assert_eq!(format_range(0, 1), "1");
        assert_eq!(format_range(0, 0), "0,0");
        assert_eq!(format_range(2, 2), "2,0");
        assert_eq!(format_range(0, 4), "1,4");
    }

    #[test]
    fn a_missing_snapshot_is_not_available() {
        let c = Comment {
            file_version: String::new(),
            ..blank_comment()
        };
        assert_eq!(snapshot_diff(&c), (Vec::new(), false));
    }

    pub(crate) fn blank_comment() -> Comment {
        Comment {
            id: "r1".into(),
            status: crate::types::Status::Done,
            workspace: "/nonexistent".into(),
            lane: None,
            author: None,
            file: "app.py".into(),
            path: "/nonexistent/app.py".into(),
            start_line: 1,
            end_line: 1,
            filetype: "python".into(),
            code: String::new(),
            comment: String::new(),
            created_at: String::new(),
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
}
