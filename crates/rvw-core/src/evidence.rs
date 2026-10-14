//! Port of internal/review/evidence.go: the code snapshot's filetype and the
//! unified diff between a reviewed and a resolved file version.

use std::cmp::{max, min};
use std::collections::HashMap;
use std::path::Path;

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
/// yield nothing. The grouping is go-difflib's `SequenceMatcher`, ported below.
pub(crate) fn unified_diff(a: &[String], b: &[String], from: &str, to: &str) -> Vec<String> {
    let groups = SequenceMatcher::new(a, b).grouped_opcodes(CONTEXT);
    if groups.is_empty() {
        return Vec::new();
    }
    let mut out = vec![format!("--- {from}"), format!("+++ {to}")];
    for group in &groups {
        let first = group[0];
        let last = group[group.len() - 1];
        out.push(format!(
            "@@ -{} +{} @@",
            format_range(first.i1, last.i2),
            format_range(first.j1, last.j2)
        ));
        for op in group {
            match op.tag {
                Tag::Equal => push_lines(&mut out, " ", &a[op.i1..op.i2]),
                Tag::Delete => push_lines(&mut out, "-", &a[op.i1..op.i2]),
                Tag::Insert => push_lines(&mut out, "+", &b[op.j1..op.j2]),
                Tag::Replace => {
                    push_lines(&mut out, "-", &a[op.i1..op.i2]);
                    push_lines(&mut out, "+", &b[op.j1..op.j2]);
                }
            }
        }
    }
    out
}

/// The opcode tags of difflib, which are its single letters `r d i e`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tag {
    Replace,
    Delete,
    Insert,
    Equal,
}

/// One opcode: `a[i1..i2]` and `b[j1..j2]` as a change or an equal run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct OpCode {
    tag: Tag,
    i1: usize,
    i2: usize,
    j1: usize,
    j2: usize,
}

/// A matching block: `a[a..a + size] == b[b..b + size]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Match {
    a: usize,
    b: usize,
    size: usize,
}

/// A pending step of the matching-blocks recursion, kept on an explicit stack.
#[derive(Clone, Copy)]
enum Work {
    Span {
        alo: usize,
        ahi: usize,
        blo: usize,
        bhi: usize,
    },
    Emit(Match),
}

/// Port of go-difflib's `SequenceMatcher` as rvw calls it: `autoJunk` on and
/// no `IsJunk`, so no element is junk and the junk-only extension loops of
/// `findLongestMatch` never run. The popular-element purge is kept.
struct SequenceMatcher<'a> {
    a: &'a [String],
    b: &'a [String],
    /// Each line of `b` to its ascending indices, without popular lines.
    b2j: HashMap<&'a str, Vec<usize>>,
}

impl<'a> SequenceMatcher<'a> {
    /// `NewMatcher` followed by `SetSeqs`, including `chainB`.
    fn new(a: &'a [String], b: &'a [String]) -> Self {
        let mut b2j: HashMap<&str, Vec<usize>> = HashMap::new();
        for (i, line) in b.iter().enumerate() {
            b2j.entry(line.as_str()).or_default().push(i);
        }
        // Purge popular lines: on inputs of 200 or more, a line that appears
        // more than n/100 + 1 times is dropped from the index.
        let ntest = b.len() / 100 + 1;
        if b.len() >= 200 {
            b2j.retain(|_, indices| indices.len() <= ntest);
        }
        Self { a, b, b2j }
    }

    /// `findLongestMatch`: the longest block in `a[alo..ahi]` and
    /// `b[blo..bhi]`, earliest in `a` and then in `b` on ties, extended over
    /// the non-junk equal lines on each side.
    fn find_longest_match(&self, alo: usize, ahi: usize, blo: usize, bhi: usize) -> Match {
        let mut best = Match {
            a: alo,
            b: blo,
            size: 0,
        };
        // j2len[j] = length of the longest match ending at a[i - 1], b[j].
        let mut j2len: HashMap<usize, usize> = HashMap::new();
        for i in alo..ahi {
            let mut newj2len = HashMap::new();
            if let Some(indices) = self.b2j.get(self.a[i].as_str()) {
                for &j in indices {
                    if j < blo {
                        continue;
                    }
                    if j >= bhi {
                        break;
                    }
                    let previous = j.checked_sub(1).and_then(|p| j2len.get(&p)).copied();
                    let k = previous.unwrap_or(0) + 1;
                    newj2len.insert(j, k);
                    if k > best.size {
                        best = Match {
                            a: i + 1 - k,
                            b: j + 1 - k,
                            size: k,
                        };
                    }
                }
            }
            j2len = newj2len;
        }

        // Extend the best block over equal lines on each side. Popular lines
        // are not indexed, so the block may grow over them here.
        while best.a > alo && best.b > blo && self.a[best.a - 1] == self.b[best.b - 1] {
            best = Match {
                a: best.a - 1,
                b: best.b - 1,
                size: best.size + 1,
            };
        }
        while best.a + best.size < ahi
            && best.b + best.size < bhi
            && self.a[best.a + best.size] == self.b[best.b + best.size]
        {
            best.size += 1;
        }
        best
    }

    /// `GetMatchingBlocks`: the matching blocks in order, adjacent ones merged,
    /// ending with the sentinel `(len(a), len(b), 0)`.
    fn matching_blocks(&self) -> Vec<Match> {
        let mut matched = Vec::new();
        let mut work = vec![Work::Span {
            alo: 0,
            ahi: self.a.len(),
            blo: 0,
            bhi: self.b.len(),
        }];
        while let Some(item) = work.pop() {
            match item {
                Work::Emit(block) => matched.push(block),
                Work::Span { alo, ahi, blo, bhi } => {
                    let found = self.find_longest_match(alo, ahi, blo, bhi);
                    if found.size == 0 {
                        continue;
                    }
                    let (i, j, k) = (found.a, found.b, found.size);
                    // Pushed in reverse so the left span is handled first.
                    if i + k < ahi && j + k < bhi {
                        work.push(Work::Span {
                            alo: i + k,
                            ahi,
                            blo: j + k,
                            bhi,
                        });
                    }
                    work.push(Work::Emit(found));
                    if alo < i && blo < j {
                        work.push(Work::Span {
                            alo,
                            ahi: i,
                            blo,
                            bhi: j,
                        });
                    }
                }
            }
        }

        let mut merged = Vec::new();
        let (mut i1, mut j1, mut k1) = (0, 0, 0);
        for block in matched {
            if i1 + k1 == block.a && j1 + k1 == block.b {
                k1 += block.size;
            } else {
                if k1 > 0 {
                    merged.push(Match {
                        a: i1,
                        b: j1,
                        size: k1,
                    });
                }
                (i1, j1, k1) = (block.a, block.b, block.size);
            }
        }
        if k1 > 0 {
            merged.push(Match {
                a: i1,
                b: j1,
                size: k1,
            });
        }
        merged.push(Match {
            a: self.a.len(),
            b: self.b.len(),
            size: 0,
        });
        merged
    }

    /// `GetOpCodes`: how to turn `a` into `b`, as runs of changes and equal
    /// lines.
    fn opcodes(&self) -> Vec<OpCode> {
        let (mut i, mut j) = (0, 0);
        let mut ops = Vec::new();
        for block in self.matching_blocks() {
            let (ai, bj, size) = (block.a, block.b, block.size);
            let tag = if i < ai && j < bj {
                Some(Tag::Replace)
            } else if i < ai {
                Some(Tag::Delete)
            } else if j < bj {
                Some(Tag::Insert)
            } else {
                None
            };
            if let Some(tag) = tag {
                ops.push(OpCode {
                    tag,
                    i1: i,
                    i2: ai,
                    j1: j,
                    j2: bj,
                });
            }
            (i, j) = (ai + size, bj + size);
            if size > 0 {
                ops.push(OpCode {
                    tag: Tag::Equal,
                    i1: ai,
                    i2: i,
                    j1: bj,
                    j2: j,
                });
            }
        }
        ops
    }

    /// `GetGroupedOpCodes(n)`: the change clusters, each with up to `n` lines
    /// of context. Empty when nothing changed.
    fn grouped_opcodes(&self, n: usize) -> Vec<Vec<OpCode>> {
        let mut codes = self.opcodes();
        if codes.is_empty() {
            codes.push(OpCode {
                tag: Tag::Equal,
                i1: 0,
                i2: 1,
                j1: 0,
                j2: 1,
            });
        }
        // Trim the context of a leading or trailing equal run to n lines.
        let first = codes[0];
        if first.tag == Tag::Equal {
            codes[0] = OpCode {
                i1: max(first.i1, first.i2.saturating_sub(n)),
                j1: max(first.j1, first.j2.saturating_sub(n)),
                ..first
            };
        }
        let last_index = codes.len() - 1;
        let last = codes[last_index];
        if last.tag == Tag::Equal {
            codes[last_index] = OpCode {
                i2: min(last.i2, last.i1 + n),
                j2: min(last.j2, last.j1 + n),
                ..last
            };
        }

        let nn = n + n;
        let mut groups = Vec::new();
        let mut group = Vec::new();
        for code in codes {
            let (mut i1, mut j1) = (code.i1, code.j1);
            // A long equal run ends the current group and starts the next.
            if code.tag == Tag::Equal && code.i2 - code.i1 > nn {
                group.push(OpCode {
                    i2: min(code.i2, code.i1 + n),
                    j2: min(code.j2, code.j1 + n),
                    ..code
                });
                groups.push(std::mem::take(&mut group));
                i1 = max(code.i1, code.i2.saturating_sub(n));
                j1 = max(code.j1, code.j2.saturating_sub(n));
            }
            group.push(OpCode { i1, j1, ..code });
        }
        // A lone equal run at the very end is only context; it is not a hunk.
        if group.len() > 1 || group.first().is_some_and(|op| op.tag != Tag::Equal) {
            groups.push(group);
        }
        groups
    }
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

    /// One case generated by go-difflib v1.0.0 (see `testdata/`).
    #[derive(serde::Deserialize)]
    struct GoCase {
        a: Vec<String>,
        b: Vec<String>,
        file: String,
        expected: String,
    }

    #[test]
    fn unified_diff_matches_go_difflib_byte_for_byte() {
        let cases: Vec<GoCase> =
            serde_json::from_str(include_str!("../testdata/difflib_cases.json"))
                .expect("difflib_cases.json parses");
        assert_eq!(cases.len(), 338, "the case file was regenerated");
        for (index, case) in cases.iter().enumerate() {
            // The Go side got each line with its "\n" terminator.
            let a: Vec<String> = case.a.iter().map(|l| format!("{l}\n")).collect();
            let b: Vec<String> = case.b.iter().map(|l| format!("{l}\n")).collect();
            let lines = unified_diff(
                &a,
                &b,
                &format!("a/{}", case.file),
                &format!("b/{}", case.file),
            );
            let got = lines.iter().fold(String::new(), |mut text, line| {
                text.push_str(line);
                text.push('\n');
                text
            });
            assert_eq!(
                got, case.expected,
                "case {index}: a={:?} b={:?}",
                case.a, case.b
            );
        }
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
            points_at: None,
            reopens: None,
            verdict: None,
            file_version: String::new(),
            ..blank_comment()
        };
        assert_eq!(snapshot_diff(&c), (Vec::new(), false));
    }

    pub(crate) fn blank_comment() -> Comment {
        Comment {
            points_at: None,
            reopens: None,
            verdict: None,
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
