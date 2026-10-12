//! Syntax highlighting for the viewer. Port of internal/tui/highlight.go, with
//! syntect (and two-face's extra syntaxes) in place of chroma.
//!
//! A file is tokenized once. With no theme named, tokens take the terminal's
//! own ANSI colors by scope; with a theme name, they take the theme's colors.

use std::sync::OnceLock;

use ratatui::style::{Color, Modifier, Style};
use rvw_render::terminal::is_control;
use syntect::highlighting::{FontStyle, Highlighter, Theme};
use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxReference, SyntaxSet};
use two_face::theme::{EmbeddedLazyThemeSet, EmbeddedThemeName};
use unicode_width::UnicodeWidthChar;

use crate::style;

/// Columns between tab stops.
const TAB_WIDTH: usize = 4;
/// The largest file highlighted; bigger ones are plain text.
const MAX_HIGHLIGHT: usize = 1 << 20;
/// How much of a file is searched for a NUL byte.
const BINARY_PROBE: usize = 8 << 10;
/// The replacement shown for a control character.
const REPLACEMENT: char = '\u{FFFD}';

/// A run of text on one line with one style.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub text: String,
    pub style: Style,
}

/// Reports a NUL byte in the first 8 KB, the way git decides.
#[must_use]
pub fn is_binary(data: &[u8]) -> bool {
    data[..data.len().min(BINARY_PROBE)].contains(&0)
}

/// The lines of `text` as the service counts them: a trailing newline does not
/// start another line, and a carriage return before a newline is dropped.
#[must_use]
pub fn source_lines(text: &str) -> Vec<String> {
    let mut lines: Vec<&str> = text.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    lines
        .into_iter()
        .map(|l| l.strip_suffix('\r').unwrap_or(l).to_owned())
        .collect()
}

/// Tokenizes `text` (named `name`, for its syntax) and returns one list of
/// segments per source line. An empty `style_name` uses the terminal's ANSI
/// colors; a theme name uses that theme; an unknown name shows no colors.
#[must_use]
pub fn highlight(name: &str, text: &str, style_name: &str) -> Vec<Vec<Segment>> {
    let ss = syntax_set();
    let syntax = if text.len() <= MAX_HIGHLIGHT {
        find_syntax(ss, name, text)
    } else {
        None
    }
    .unwrap_or_else(|| ss.find_syntax_plain_text());
    let colors = Colors::for_name(style_name);
    let mut lexer = Lexer {
        state: ParseState::new(syntax),
        // Scopes carry across lines: a block comment spans several.
        stack: ScopeStack::new(),
    };
    source_lines(text)
        .iter()
        .map(|line| lexer.line(ss, line, &colors))
        .collect()
}

/// The parser state carried from one line to the next.
struct Lexer {
    state: ParseState,
    stack: ScopeStack,
}

impl Lexer {
    /// Highlights one line.
    fn line(&mut self, ss: &SyntaxSet, line: &str, colors: &Colors) -> Vec<Segment> {
        // Newline-aware syntaxes expect each line to end in a newline.
        let with_newline = format!("{line}\n");
        let Ok(ops) = self.state.parse_line(&with_newline, ss) else {
            return line_segments(&[(line.to_owned(), Style::default())]);
        };
        let mut tokens = Vec::new();
        let mut pos = 0;
        for (idx, op) in ops {
            let end = idx.min(line.len());
            if let Some(token) = line.get(pos..end).filter(|t| !t.is_empty()) {
                tokens.push((token.to_owned(), token_style(colors, &self.stack)));
                pos = end;
            }
            if self.stack.apply(&op).is_err() {
                break;
            }
        }
        if let Some(rest) = line.get(pos..).filter(|t| !t.is_empty()) {
            tokens.push((rest.to_owned(), token_style(colors, &self.stack)));
        }
        line_segments(&tokens)
    }
}

/// Expands tabs to the next tab stop from column `col` and replaces control
/// characters with U+FFFD, returning the text and the column after it.
#[must_use]
pub fn display_text(s: &str, col: usize) -> (String, usize) {
    let mut out = String::with_capacity(s.len());
    let mut col = col;
    for c in s.chars() {
        if c == '\t' {
            let n = TAB_WIDTH - col % TAB_WIDTH;
            out.extend(std::iter::repeat_n(' ', n));
            col += n;
            continue;
        }
        let c = if c == '\n' || is_control(c) {
            REPLACEMENT
        } else {
            c
        };
        out.push(c);
        col += c.width().unwrap_or(0);
    }
    (out, col)
}

fn syntax_set() -> &'static SyntaxSet {
    static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
    SYNTAXES.get_or_init(two_face::syntax::extra_newlines)
}

fn theme_set() -> &'static EmbeddedLazyThemeSet {
    static THEMES: OnceLock<EmbeddedLazyThemeSet> = OnceLock::new();
    THEMES.get_or_init(two_face::theme::extra)
}

/// Extensions with no syntax of their own, mapped to the nearest one.
const EXTENSION_ALIASES: &[(&str, &str)] = &[("jsx", "js"), ("h", "c")];

/// The syntax for a file: by its name or extension, then by its first line.
fn find_syntax<'a>(ss: &'a SyntaxSet, name: &str, text: &str) -> Option<&'a SyntaxReference> {
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
        .or_else(|| ss.find_syntax_by_first_line(text.lines().next().unwrap_or("")))
}

/// How tokens are colored.
enum Colors {
    /// The terminal's ANSI palette, by scope.
    Ansi,
    /// A syntect theme's colors.
    Theme(&'static Theme),
    /// An unknown theme name: no colors.
    Mono,
}

impl Colors {
    fn for_name(name: &str) -> Colors {
        if name.is_empty() {
            return Colors::Ansi;
        }
        let set = theme_set();
        EmbeddedLazyThemeSet::theme_names()
            .iter()
            .copied()
            .find(|t: &EmbeddedThemeName| t.as_name() == name)
            .map_or(Colors::Mono, |t| Colors::Theme(set.get(t)))
    }
}

/// Expands the tokens of one line into segments, carrying the tab column.
fn line_segments(tokens: &[(String, Style)]) -> Vec<Segment> {
    let mut col = 0;
    let mut out = Vec::new();
    for (text, style) in tokens {
        let (text, next) = display_text(text, col);
        col = next;
        if !text.is_empty() {
            out.push(Segment {
                text,
                style: *style,
            });
        }
    }
    out
}

fn token_style(colors: &Colors, stack: &ScopeStack) -> Style {
    match colors {
        Colors::Ansi => ansi_style(stack.as_slice()),
        Colors::Theme(theme) => {
            let st = Highlighter::new(theme).style_for_stack(stack.as_slice());
            theme_style(st)
        }
        Colors::Mono => Style::default(),
    }
}

fn theme_style(st: syntect::highlighting::Style) -> Style {
    let c = st.foreground;
    // A zero alpha is how syntect marks an ANSI palette index in the red channel.
    let color = if c.a == 0 {
        Color::Indexed(c.r)
    } else {
        Color::Rgb(c.r, c.g, c.b)
    };
    let mut out = Style::new().fg(color);
    if st.font_style.contains(FontStyle::BOLD) {
        out = out.add_modifier(Modifier::BOLD);
    }
    if st.font_style.contains(FontStyle::ITALIC) {
        out = out.add_modifier(Modifier::ITALIC);
    }
    if st.font_style.contains(FontStyle::UNDERLINE) {
        out = out.add_modifier(Modifier::UNDERLINED);
    }
    out
}

/// One ANSI rule: a scope prefix and the style it gives, or `None` to leave
/// the text plain (operators, for one, stay plain).
type Rule = (&'static str, Option<fn() -> Style>);

/// A compiled rule: the scope to match and its style, as in [`Rule`].
type ScopedRule = (Scope, Option<fn() -> Style>);

/// The ANSI rules, most specific first. The innermost scope with a matching
/// rule wins, the way chroma's token categories fall back to their parents.
const ANSI_RULES: &[Rule] = &[
    ("comment.block.preprocessor", Some(preproc)),
    ("comment", Some(comment)),
    ("markup.deleted", Some(red)),
    ("markup.inserted", Some(green)),
    ("markup.heading", Some(heading)),
    ("markup.bold", Some(bold)),
    ("markup.italic", Some(italic)),
    ("invalid", Some(red)),
    ("string.regexp", Some(cyan)),
    ("constant.character.escape", Some(cyan)),
    ("string", Some(green)),
    ("constant.numeric", Some(yellow)),
    ("constant", Some(yellow)),
    ("keyword.operator", None),
    ("storage.type", Some(yellow)),
    ("storage", Some(magenta)),
    ("keyword", Some(magenta)),
    ("entity.name.function", Some(blue)),
    ("support.function", Some(blue)),
    ("variable.function", Some(blue)),
    ("entity.name.class", Some(yellow)),
    ("entity.name.type", Some(yellow)),
    ("support.class", Some(yellow)),
    ("support.type", Some(yellow)),
    ("entity.name.tag", Some(blue)),
    ("entity.other.attribute-name", Some(yellow)),
    ("entity.name.label", Some(cyan)),
    ("entity.name.decorator", Some(cyan)),
    ("meta.annotation", Some(cyan)),
    ("support", Some(cyan)),
    ("variable.language", Some(cyan)),
];

fn comment() -> Style {
    Style::new()
        .fg(Color::Indexed(8))
        .add_modifier(Modifier::ITALIC)
}
fn preproc() -> Style {
    Style::new().fg(Color::Indexed(6))
}
fn red() -> Style {
    Style::new().fg(Color::Indexed(1))
}
fn green() -> Style {
    Style::new().fg(Color::Indexed(2))
}
fn yellow() -> Style {
    Style::new().fg(Color::Indexed(3))
}
fn blue() -> Style {
    Style::new().fg(Color::Indexed(4))
}
fn magenta() -> Style {
    Style::new().fg(Color::Indexed(5))
}
fn cyan() -> Style {
    Style::new().fg(Color::Indexed(6))
}
fn heading() -> Style {
    Style::new()
        .fg(Color::Indexed(4))
        .add_modifier(Modifier::BOLD)
}
fn bold() -> Style {
    Style::new().add_modifier(Modifier::BOLD)
}
fn italic() -> Style {
    Style::new().add_modifier(Modifier::ITALIC)
}

/// The ANSI style of a token, from its scope stack, innermost scope first.
fn ansi_style(scopes: &[Scope]) -> Style {
    static RULES: OnceLock<Vec<ScopedRule>> = OnceLock::new();
    let rules = RULES.get_or_init(|| {
        ANSI_RULES
            .iter()
            .filter_map(|(prefix, rule)| Scope::new(prefix).ok().map(|s| (s, *rule)))
            .collect()
    });
    for scope in scopes.iter().rev() {
        for (prefix, rule) in rules {
            if prefix.is_prefix_of(*scope) {
                return rule.map_or_else(style::plain, |f| f());
            }
        }
    }
    style::plain()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(line: &[Segment]) -> String {
        line.iter().map(|s| s.text.as_str()).collect()
    }

    #[test]
    fn binary_is_a_nul_in_the_first_8k() {
        assert!(is_binary(b"abc\0def"));
        assert!(!is_binary("plain text\n".as_bytes()));
        let mut late = vec![b'a'; BINARY_PROBE + 10];
        late[BINARY_PROBE + 5] = 0;
        assert!(!is_binary(&late));
        assert!(is_binary(&[0u8]));
        assert!(!is_binary(&[]));
    }

    #[test]
    fn source_lines_ignore_a_trailing_newline_and_crlf() {
        assert_eq!(source_lines("a\nb\n"), vec!["a", "b"]);
        assert_eq!(source_lines("a\r\nb"), vec!["a", "b"]);
        assert_eq!(source_lines("a\n\n"), vec!["a", ""]);
        assert!(source_lines("").is_empty());
    }

    #[test]
    fn display_text_expands_tabs_to_the_next_stop() {
        assert_eq!(display_text("a\tb", 0), ("a   b".to_owned(), 5));
        assert_eq!(display_text("\t", 2), ("  ".to_owned(), 4));
        assert_eq!(display_text("ab\t", 3), ("ab   ".to_owned(), 8));
        assert_eq!(display_text("x\tz", 1), ("x  z".to_owned(), 5));
    }

    #[test]
    fn display_text_shows_control_characters_as_replacements() {
        assert_eq!(display_text("a\u{7}b", 0), ("a\u{FFFD}b".to_owned(), 3));
        assert_eq!(display_text("a\u{9b}", 0).0, "a\u{FFFD}");
    }

    #[test]
    fn highlight_has_one_line_per_source_line() {
        let text = "fn main() {\n\tlet x = 1;\n\n}\n";
        let lines = highlight("main.rs", text, "");
        assert_eq!(lines.len(), source_lines(text).len());
        assert!(lines[2].is_empty());
        assert_eq!(text_of(&lines[1]), "    let x = 1;");
    }

    #[test]
    fn highlight_expands_tabs_and_keeps_the_text() {
        let lines = highlight("notes.txt", "a\tb\n", "");
        assert_eq!(text_of(&lines[0]), "a   b");
    }

    #[test]
    fn highlight_of_an_unknown_file_is_one_plain_run_per_line() {
        let lines = highlight("data.unknownext", "one\ntwo", "");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0][0].style, Style::default());
    }

    #[test]
    fn rust_and_python_get_more_than_one_style() {
        let rust = highlight("lib.rs", "fn main() { let s = \"hi\"; // note\n}\n", "");
        let styles: std::collections::HashSet<_> = rust[0].iter().map(|s| s.style).collect();
        assert!(styles.len() > 1, "rust styles: {styles:?}");

        let py = highlight("app.py", "def f(x):\n    return x + 1  # add\n", "");
        let styles: std::collections::HashSet<_> = py[1].iter().map(|s| s.style).collect();
        assert!(styles.len() > 1, "python styles: {styles:?}");
    }

    #[test]
    fn comments_are_dim_and_italic_in_ansi_mode() {
        let lines = highlight("x.py", "# hello\n", "");
        let seg = &lines[0][0];
        assert_eq!(seg.style.fg, Some(Color::Indexed(8)));
        assert!(seg.style.add_modifier.contains(Modifier::ITALIC));
    }

    #[test]
    fn a_named_theme_colors_with_rgb_and_an_unknown_one_is_plain() {
        let themed = highlight("lib.rs", "fn main() {}\n", "base16-ocean.dark");
        assert!(themed[0]
            .iter()
            .any(|s| matches!(s.style.fg, Some(Color::Rgb(..)))));
        let mono = highlight("lib.rs", "fn main() {}\n", "no-such-theme");
        assert!(mono[0].iter().all(|s| s.style == Style::default()));
    }

    #[test]
    fn invalid_and_binary_input_does_not_panic() {
        let lossy = String::from_utf8_lossy(&[0xff, b'a', 0, b'\n', 0xc3]);
        let lines = highlight("blob.rs", &lossy, "");
        assert_eq!(lines.len(), source_lines(&lossy).len());
        let _ = highlight("", "\u{0}\u{1b}[31m\n", "");
    }

    #[test]
    fn ts_and_tsx_resolve_to_a_real_syntax() {
        let ss = syntax_set();
        for name in [
            "a.ts", "a.tsx", "a.jsx", "a.toml", "a.bash", "a.ex", "a.zsh",
        ] {
            let syntax = find_syntax(ss, name, "");
            assert!(syntax.is_some(), "no syntax for {name}");
        }
    }
}
