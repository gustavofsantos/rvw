//! Syntax highlighting as data: which byte ranges of a line are a keyword, a
//! string, a comment and so on. No colors here; the GUI maps each [`Class`] to
//! its own palette. Tokenizing follows the terminal viewer's highlighter
//! (syntect with two-face's extra syntaxes), which this crate does not depend on.

use std::sync::OnceLock;

use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxReference, SyntaxSet};
use two_face::syntax::extra_newlines;

use crate::diff::{FileDiff, Line, LineKind};

/// The largest text highlighted; bigger input is all [`Class::Plain`].
const MAX_HIGHLIGHT: usize = 1 << 20;
/// How much of the text is searched for a NUL byte.
const BINARY_PROBE: usize = 8 << 10;
/// The most lines of one diff or file that a front end highlights: about a
/// second of tokenizing, spent off the UI thread.
pub const MAX_LINES: usize = 4_000;

/// Whether a diff or file of `line_count` lines is highlighted at all.
#[must_use]
pub fn worth_highlighting(line_count: usize) -> bool {
    line_count <= MAX_LINES
}

/// What a run of text is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Class {
    Plain,
    Keyword,
    String,
    Comment,
    Number,
    Type,
    Function,
    Constant,
    Operator,
    Punctuation,
}

/// A run of one line with one class. Offsets are bytes into the line text,
/// on char boundaries. The spans of a line are sorted, do not overlap, cover
/// the whole line, and are never empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub class: Class,
}

/// Highlights the lines of `file_name`'s source as one stream: the parser state
/// carries from one line to the next. Returns one list of spans per line.
#[must_use]
pub fn lines(file_name: &str, lines: &[&str]) -> Vec<Vec<Span>> {
    let ss = syntax_set();
    let total: usize = lines.iter().map(|l| l.len() + 1).sum();
    let syntax = if total > MAX_HIGHLIGHT || is_binary(lines.iter().copied()) {
        None
    } else {
        find_syntax(ss, file_name, lines.first().copied().unwrap_or(""))
    };
    tokenize(ss, syntax, lines)
}

/// Highlights a diff. Added and context lines are tokenized as the new file's
/// stream, removed and context lines as the old file's stream; each hunk starts
/// a new stream. The result is indexed `[hunk][line]`, and a context line takes
/// its spans from the new stream.
#[must_use]
pub fn diff(file_name: &str, diff: &FileDiff) -> Vec<Vec<Vec<Span>>> {
    let ss = syntax_set();
    let all = || {
        diff.hunks
            .iter()
            .flat_map(|h| h.lines.iter().map(|l| l.text.as_str()))
    };
    let total: usize = all().map(|l| l.len() + 1).sum();
    let first = all().next().unwrap_or("");
    let syntax = if total > MAX_HIGHLIGHT || is_binary(all()) {
        None
    } else {
        find_syntax(ss, file_name, first)
    };
    diff.hunks
        .iter()
        .map(|hunk| hunk_spans(ss, syntax, &hunk.lines))
        .collect()
}

/// Highlights a diff whose lines are all one file's text (the unchanged file
/// the Files tab shows): a single stream, split back into the hunks' shapes.
/// The result is indexed `[hunk][line]`, like [`diff`].
#[must_use]
pub fn unchanged(file_name: &str, file: &FileDiff) -> Vec<Vec<Vec<Span>>> {
    let texts: Vec<&str> = file
        .hunks
        .iter()
        .flat_map(|h| h.lines.iter().map(|l| l.text.as_str()))
        .collect();
    let mut spans = lines(file_name, &texts).into_iter();
    file.hunks
        .iter()
        .map(|hunk| spans.by_ref().take(hunk.lines.len()).collect())
        .collect()
}

/// The spans of one hunk's lines, from its new and old streams.
fn hunk_spans(ss: &SyntaxSet, syntax: Option<&SyntaxReference>, lines: &[Line]) -> Vec<Vec<Span>> {
    let new_text: Vec<&str> = lines
        .iter()
        .filter(|l| l.kind != LineKind::Removed)
        .map(|l| l.text.as_str())
        .collect();
    let old_text: Vec<&str> = lines
        .iter()
        .filter(|l| l.kind != LineKind::Added)
        .map(|l| l.text.as_str())
        .collect();
    let mut new_spans = tokenize(ss, syntax, &new_text).into_iter();
    let mut old_spans = tokenize(ss, syntax, &old_text).into_iter();
    lines
        .iter()
        .map(|line| match line.kind {
            LineKind::Added => new_spans.next().unwrap_or_default(),
            LineKind::Removed => old_spans.next().unwrap_or_default(),
            LineKind::Context => {
                let spans = new_spans.next().unwrap_or_default();
                old_spans.next();
                spans
            }
        })
        .collect()
}

/// Tokenizes a stream of lines with one parser state. Without a syntax, every
/// non-empty line is one plain span.
fn tokenize(ss: &SyntaxSet, syntax: Option<&SyntaxReference>, lines: &[&str]) -> Vec<Vec<Span>> {
    let Some(syntax) = syntax else {
        return lines.iter().map(|&line| plain_line(line)).collect();
    };
    let mut state = ParseState::new(syntax);
    // Scopes carry across lines: a block comment spans several.
    let mut stack = ScopeStack::new();
    lines
        .iter()
        .map(|line| line_spans(ss, &mut state, &mut stack, line))
        .collect()
}

/// The spans of one line, given the parser state from the lines before it.
fn line_spans(
    ss: &SyntaxSet,
    state: &mut ParseState,
    stack: &mut ScopeStack,
    line: &str,
) -> Vec<Span> {
    // Newline-aware syntaxes expect each line to end in a newline.
    let with_newline = format!("{line}\n");
    let Ok(ops) = state.parse_line(&with_newline, ss) else {
        return plain_line(line);
    };
    let mut out = Vec::new();
    let mut pos = 0;
    for (idx, op) in ops {
        let end = char_floor(line, idx).max(pos);
        push_span(&mut out, pos, end, class_of(stack.as_slice()));
        pos = end;
        if stack.apply(&op).is_err() {
            break;
        }
    }
    push_span(&mut out, pos, line.len(), class_of(stack.as_slice()));
    out
}

/// The one plain span of a line, or none for an empty line.
fn plain_line(line: &str) -> Vec<Span> {
    let mut out = Vec::new();
    push_span(&mut out, 0, line.len(), Class::Plain);
    out
}

/// Appends `start..end` as `class`, merging into the last span when they touch
/// and the class is the same. An empty range adds nothing.
fn push_span(out: &mut Vec<Span>, start: usize, end: usize, class: Class) {
    if start >= end {
        return;
    }
    if let Some(last) = out.last_mut()
        && last.class == class
        && last.end == start
    {
        last.end = end;
        return;
    }
    out.push(Span { start, end, class });
}

/// The largest char boundary at or before `idx` (and at most the line length).
fn char_floor(line: &str, idx: usize) -> usize {
    let mut i = idx.min(line.len());
    // Index 0 is always a boundary, so this stops.
    while !line.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// True when a NUL byte appears in the first 8 KiB of the text.
fn is_binary<'a>(lines: impl Iterator<Item = &'a str>) -> bool {
    let mut seen = 0;
    for line in lines {
        if seen >= BINARY_PROBE {
            break;
        }
        let take = (BINARY_PROBE - seen).min(line.len());
        if line.as_bytes().get(..take).is_some_and(|b| b.contains(&0)) {
            return true;
        }
        seen += line.len() + 1;
    }
    false
}

fn syntax_set() -> &'static SyntaxSet {
    static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
    SYNTAXES.get_or_init(extra_newlines)
}

/// Extensions with no syntax of their own, mapped to the nearest one.
const EXTENSION_ALIASES: &[(&str, &str)] = &[("jsx", "js"), ("h", "c")];

/// The syntax for a file: by its name or extension, then by its first line.
fn find_syntax<'a>(ss: &'a SyntaxSet, name: &str, first_line: &str) -> Option<&'a SyntaxReference> {
    let base = name.rsplit('/').next().unwrap_or(name);
    let by_ext = |ext: &str| {
        EXTENSION_ALIASES
            .iter()
            .find(|(from, _)| from.eq_ignore_ascii_case(ext))
            .and_then(|(_, to)| ss.find_syntax_by_extension(to))
            .or_else(|| ss.find_syntax_by_extension(ext))
    };
    by_ext(base)
        .or_else(|| base.rsplit_once('.').and_then(|(_, ext)| by_ext(ext)))
        .or_else(|| ss.find_syntax_by_first_line(first_line))
}

/// Scope prefixes and the class they give, most specific first. Within one
/// scope the first matching prefix wins.
const RULES: &[(&str, Class)] = &[
    ("comment", Class::Comment),
    ("punctuation.definition.comment", Class::Comment),
    ("string", Class::String),
    ("punctuation.definition.string", Class::String),
    ("constant.character.escape", Class::String),
    ("constant.numeric", Class::Number),
    ("constant", Class::Constant),
    ("keyword.operator", Class::Operator),
    ("keyword", Class::Keyword),
    ("storage.type", Class::Keyword),
    ("storage", Class::Keyword),
    ("entity.name.function", Class::Function),
    ("support.function", Class::Function),
    ("meta.function-call", Class::Function),
    ("variable.function", Class::Function),
    ("entity.name.type", Class::Type),
    ("entity.name.class", Class::Type),
    ("support.type", Class::Type),
    ("support.class", Class::Type),
    ("punctuation", Class::Punctuation),
];

/// The class of a token, from its scope stack: the innermost scope with a rule.
fn class_of(scopes: &[Scope]) -> Class {
    static COMPILED: OnceLock<Vec<(Scope, Class)>> = OnceLock::new();
    let rules = COMPILED.get_or_init(|| {
        RULES
            .iter()
            .filter_map(|(prefix, class)| Scope::new(prefix).ok().map(|s| (s, *class)))
            .collect()
    });
    for scope in scopes.iter().rev() {
        for (prefix, class) in rules {
            if prefix.is_prefix_of(*scope) {
                return *class;
            }
        }
    }
    Class::Plain
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::parse;
    use std::fmt::Write;
    use std::time::Instant;

    /// The class of the byte at `at` in `line`'s spans.
    fn class_at(spans: &[Span], at: usize) -> Option<Class> {
        spans
            .iter()
            .find(|s| s.start <= at && at < s.end)
            .map(|s| s.class)
    }

    /// The text of the first span with `class`.
    fn text_of_class<'a>(line: &'a str, spans: &[Span], class: Class) -> Vec<&'a str> {
        spans
            .iter()
            .filter(|s| s.class == class)
            .map(|s| &line[s.start..s.end])
            .collect()
    }

    /// Asserts the spans of `line` cover it exactly, in order, on boundaries.
    fn assert_covers(line: &str, spans: &[Span]) {
        let mut pos = 0;
        for s in spans {
            assert_eq!(s.start, pos, "gap or overlap in {line:?}: {spans:?}");
            assert!(s.start < s.end, "empty span in {line:?}");
            assert!(line.is_char_boundary(s.start) && line.is_char_boundary(s.end));
            pos = s.end;
        }
        assert_eq!(pos, line.len(), "spans stop short in {line:?}");
    }

    fn check_all(name: &str, text: &str) {
        let lines: Vec<&str> = text.lines().collect();
        let spans = lines_of(name, &lines);
        assert_eq!(spans.len(), lines.len());
        for (line, s) in lines.iter().zip(&spans) {
            assert_covers(line, s);
        }
    }

    fn lines_of(name: &str, lines: &[&str]) -> Vec<Vec<Span>> {
        super::lines(name, lines)
    }

    #[test]
    fn rust_line_classes() {
        let line = "fn main() { let x = 42; // hi";
        let spans = &lines_of("main.rs", &[line])[0];
        assert_covers(line, spans);
        assert_eq!(
            class_at(spans, line.find("fn").unwrap_or(0)),
            Some(Class::Keyword)
        );
        assert_eq!(
            class_at(spans, line.find("let").unwrap_or(0)),
            Some(Class::Keyword)
        );
        assert_eq!(text_of_class(line, spans, Class::Number), ["42"]);
        assert_eq!(text_of_class(line, spans, Class::Comment), ["// hi"]);
        assert_eq!(text_of_class(line, spans, Class::Function), ["main"]);
    }

    #[test]
    fn rust_string_and_its_quotes_are_string() {
        let line = "    println!(\"a\");";
        let spans = &lines_of("main.rs", &[line])[0];
        assert_covers(line, spans);
        assert_eq!(text_of_class(line, spans, Class::String), ["\"a\""]);
    }

    #[test]
    fn storage_type_is_keyword_and_types_are_type() {
        // Pinned: `storage.type` (Rust `let`, `fn`) is Keyword; a type name is Type.
        let line = "let v: Vec<u8> = Vec::new();";
        let spans = &lines_of("main.rs", &[line])[0];
        assert_covers(line, spans);
        assert_eq!(class_at(spans, 0), Some(Class::Keyword));
        assert_eq!(
            class_at(spans, line.find("Vec").unwrap_or(0)),
            Some(Class::Type)
        );
    }

    #[test]
    fn samples_cover_every_line_exactly() {
        check_all(
            "lib.rs",
            "use std::fmt;\n/* block\n   more */ fn f<'a>(x: &'a str) -> u32 { 1_000 + 0x1F }\n\"é ü\" // ✓\n",
        );
        check_all(
            "app.py",
            "def f(x):\n    \"\"\"doc\"\"\"\n    return x + 1  # add\n\nprint(f'{x!r}')\n",
        );
        check_all(
            "app.js",
            "const a = `t ${x}`;\n/* multi\nline */ function g() { return a ?? 2.5e3; }\n",
        );
        check_all(
            "README.md",
            "# Title\n\n- item `code` **bold**\n```\nblock\n```\n",
        );
        check_all("data.json", "{\n  \"k\": [1, true, null, \"é\"]\n}\n");
        check_all("Cargo.toml", "[package]\nname = \"x\" # n\nversion = 1.2\n");
    }

    #[test]
    fn block_comment_keeps_comment_on_its_second_line() {
        let lines = ["/* start", "still comment */ let x = 1;"];
        let spans = lines_of("lib.rs", &lines);
        assert_covers(lines[1], &spans[1]);
        assert_eq!(class_at(&spans[1], 0), Some(Class::Comment));
        assert_eq!(
            text_of_class(lines[1], &spans[1], Class::Comment),
            ["still comment */"]
        );
        assert_eq!(
            class_at(&spans[1], lines[1].find("let").unwrap_or(0)),
            Some(Class::Keyword)
        );
    }

    #[test]
    fn unknown_extension_is_all_plain() {
        let line = "fn main() { 42 }";
        let spans = &lines_of("data.unknownext", &[line])[0];
        assert_eq!(
            spans,
            &[Span {
                start: 0,
                end: line.len(),
                class: Class::Plain
            }]
        );
    }

    #[test]
    fn empty_line_has_no_spans() {
        let spans = lines_of("lib.rs", &["", "fn a() {}", ""]);
        assert!(spans[0].is_empty());
        assert!(spans[2].is_empty());
        assert!(!spans[1].is_empty());
    }

    #[test]
    fn non_ascii_offsets_fall_on_char_boundaries() {
        let lines = ["let s = \"héllo wörld 🦀\"; // ünï ✓", "let é = 1;"];
        let spans = lines_of("lib.rs", &lines);
        for (line, s) in lines.iter().zip(&spans) {
            assert_covers(line, s);
            for span in s {
                assert!(line.is_char_boundary(span.start));
                assert!(line.is_char_boundary(span.end));
            }
        }
    }

    #[test]
    fn binary_text_is_all_plain() {
        let lines = ["fn a() {}\0", "let x = 1;"];
        let spans = lines_of("lib.rs", &lines);
        for s in spans.iter().flatten() {
            assert_eq!(s.class, Class::Plain);
        }
        assert_covers(lines[1], &spans[1]);
    }

    #[test]
    fn diff_has_one_entry_per_line_and_removed_lines_use_the_old_stream() {
        // The old file opens a block comment on line 1 (removed); the context
        // line after it is a comment in the old stream and in the new one.
        let text = "\
@@ -1,3 +1,3 @@
-/* old open
+/* new open
 still closed */
 let y = 2;
-let z = 3;
+let z = 4;
";
        let file = parse(text);
        let spans = diff("lib.rs", &file);
        assert_eq!(spans.len(), file.hunks.len());
        let hunk = &file.hunks[0];
        assert_eq!(spans[0].len(), hunk.lines.len());
        for (line, s) in hunk.lines.iter().zip(&spans[0]) {
            assert_covers(&line.text, s);
        }
        // Removed "/* old open" is a comment in the old stream.
        assert_eq!(class_at(&spans[0][0], 0), Some(Class::Comment));
        // The context line closes the old comment, so it is comment text in both streams.
        assert_eq!(class_at(&spans[0][1], 0), Some(Class::Comment));
        // The added line opens a comment, and nothing before it is open.
        assert_eq!(class_at(&spans[0][2], 0), Some(Class::Comment));
        // Context "let y" takes the new stream, where the comment is closed.
        assert_eq!(class_at(&spans[0][3], 0), Some(Class::Keyword));
    }

    #[test]
    fn removed_line_is_tokenized_in_the_old_stream() {
        // Old: "/* open" then "still */ let old = 1;" (the comment closes, then code).
        // New: "let new = 2;" alone. The removed line is comment only in the old stream.
        let file = parse("@@ -1,2 +1,1 @@\n-/* open\n-still */ let old = 1;\n+let new = 2;\n");
        let spans = diff("lib.rs", &file);
        let removed = &spans[0][1];
        assert_eq!(class_at(removed, 0), Some(Class::Comment));
        assert_eq!(
            text_of_class("still */ let old = 1;", removed, Class::Keyword),
            ["let"]
        );
        assert_eq!(class_at(&spans[0][2], 0), Some(Class::Keyword));
    }

    #[test]
    fn diff_resets_state_at_each_hunk() {
        let text = "\
@@ -1 +1 @@
-/* open
+/* open
@@ -10 +10 @@
 let a = 1;
";
        let file = parse(text);
        let spans = diff("lib.rs", &file);
        assert_eq!(spans.len(), 2);
        assert_eq!(class_at(&spans[1][0], 0), Some(Class::Keyword));
    }

    #[test]
    fn the_cap_admits_four_thousand_lines_and_no_more() {
        assert!(worth_highlighting(0));
        assert!(worth_highlighting(MAX_LINES));
        assert!(!worth_highlighting(MAX_LINES + 1));
    }

    #[test]
    fn an_unchanged_file_is_one_stream_split_by_hunk() {
        let file = crate::diff::from_text("/* open\nstill */\nlet a = 1;");
        let spans = unchanged("lib.rs", &file);
        let total: usize = file.hunks.iter().map(|h| h.lines.len()).sum();
        assert_eq!(spans.len(), file.hunks.len());
        assert_eq!(spans.iter().map(Vec::len).sum::<usize>(), total);
        // The block comment carries over the line break.
        assert_eq!(class_at(&spans[0][1], 0), Some(Class::Comment));
        assert_eq!(class_at(&spans[0][2], 0), Some(Class::Keyword));
    }

    #[test]
    fn diff_of_an_unknown_file_is_plain() {
        let file = parse("@@ -1,2 +1,2 @@\n-a\n+b\n c\n");
        let spans = diff("notes.unknownext", &file);
        for s in spans.iter().flatten().flatten() {
            assert_eq!(s.class, Class::Plain);
        }
        assert_eq!(spans[0].len(), 3);
    }

    #[test]
    fn highlighting_five_thousand_lines_is_measured() {
        let mut text = String::new();
        for i in 0..5000 {
            let _ = writeln!(
                text,
                "fn f{i}(x: u32) -> u32 {{ let y = x + {i}; /* c */ y }}"
            );
        }
        let lines: Vec<&str> = text.lines().collect();
        // The first call builds the syntax set; time the warm calls only.
        let _ = lines_of("big.rs", &lines[..1]);
        let start = Instant::now();
        let spans = lines_of("big.rs", &lines);
        let elapsed = start.elapsed();
        assert_eq!(spans.len(), 5000);
        println!("highlighted {} lines in {elapsed:?}", lines.len());
    }
}
