//! The viewer's detail pages and review rows: pure builders from the service's
//! types to styled lines. Port of the page code in internal/tui/reviews.go.

use rvw_core::operations::{Evidence, ReviewSheet, SheetState};
use rvw_core::types::{Comment, Status};
use unicode_width::UnicodeWidthStr;

use crate::style::{self, Piece, expand_tabs, piece};

/// The narrowest a wrapped paragraph is laid out, whatever the width.
const MIN_WRAP: usize = 10;

/// A detail page under construction: headed sections of wrapped text or raw
/// lines, laid out for `w` columns. Every line starts with one space of margin.
#[derive(Debug, Clone, Default)]
pub struct Page {
    w: usize,
    pub lines: Vec<Vec<Piece>>,
}

impl Page {
    /// An empty page for `w` columns.
    #[must_use]
    pub fn new(w: usize) -> Page {
        Page {
            w,
            lines: Vec::new(),
        }
    }

    /// A line of `pieces`, after the margin.
    pub fn line(&mut self, pieces: Vec<Piece>) {
        let mut row = Vec::with_capacity(pieces.len() + 1);
        row.push(piece(" ", style::plain()));
        row.extend(pieces);
        self.lines.push(row);
    }

    /// An empty line.
    pub fn blank(&mut self) {
        self.lines.push(Vec::new());
    }

    /// A section heading, after a blank line unless the page is empty.
    pub fn heading(&mut self, title: &str) {
        if !self.lines.is_empty() {
            self.blank();
        }
        self.line(vec![piece(title, style::title())]);
    }

    /// Prose wrapped under the current margin, each line led by `lead`.
    pub fn text(&mut self, s: &str, lead: &str, st: ratatui::style::Style) {
        let limit = MIN_WRAP.max(self.w.saturating_sub(2 + lead.width()));
        for l in wrap(s.trim(), limit) {
            self.line(vec![
                piece(lead, style::plain()),
                piece(expand_tabs(&l), st),
            ]);
        }
    }
}

/// Wraps `text` to `limit` columns, breaking at spaces and splitting a word
/// wider than the limit. Existing newlines are kept.
#[must_use]
pub fn wrap(text: &str, limit: usize) -> Vec<String> {
    let limit = limit.max(1);
    let mut out = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        let mut width = 0;
        for word in para.split_whitespace() {
            let word_width = word.width();
            if width > 0 && width + 1 + word_width <= limit {
                line.push(' ');
                line.push_str(word);
                width += 1 + word_width;
                continue;
            }
            if width > 0 {
                out.push(std::mem::take(&mut line));
                width = 0;
            }
            for c in word.chars() {
                let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
                if width + cw > limit && width > 0 {
                    out.push(std::mem::take(&mut line));
                    width = 0;
                }
                line.push(c);
                width += cw;
            }
        }
        out.push(line);
    }
    out
}

/// `"@name"`, or `"someone"` for an anonymous author.
#[must_use]
pub fn who(author: Option<&str>) -> String {
    match author {
        Some(a) if !a.is_empty() => format!("@{a}"),
        _ => "someone".to_owned(),
    }
}

/// The date of a stored timestamp: everything before its `T`.
#[must_use]
pub fn day(ts: &str) -> String {
    ts.split_once('T').map_or(ts, |(d, _)| d).to_owned()
}

/// A comment's file name and lines, without the directories.
#[must_use]
pub fn short_location(c: &Comment) -> String {
    let base = c.file.rsplit('/').next().unwrap_or(&c.file);
    location(base, c.start_line, c.end_line)
}

/// `file:line`, or `file:start-end` for a range.
#[must_use]
pub fn location(file: &str, start: i64, end: i64) -> String {
    if start == end {
        format!("{file}:{start}")
    } else {
        format!("{file}:{start}-{end}")
    }
}

/// A comment's status at a glance: done, rejected, pulled, or open.
#[must_use]
pub fn status_mark(s: Status) -> Piece {
    match s {
        Status::Done => piece("✓", style::done()),
        Status::Rejected => piece("✗", style::rejected()),
        Status::Pulled => piece("◐", style::pulled()),
        Status::Pending => piece("○", style::dim()),
    }
}

/// A review's progress at a glance.
#[must_use]
pub fn state_mark(s: SheetState) -> Piece {
    match s {
        SheetState::Complete => piece("✓", style::done()),
        SheetState::Pulled => piece("◐", style::pulled()),
        SheetState::Pending => piece("○", style::dim()),
    }
}

/// The first line of `s`, trimmed and ready to draw.
fn first_line(s: &str) -> String {
    let s = s.trim();
    expand_tabs(s.split('\n').next().unwrap_or(""))
}

/// The page for one comment: what was asked, the code as reviewed, and how it
/// was addressed.
#[must_use]
pub fn evidence_page(ev: &Evidence, w: usize) -> Vec<Vec<Piece>> {
    let c = &ev.comment;
    let mut p = Page::new(w);
    let mut head = vec![
        piece(c.id.as_str(), style::id()),
        piece(
            format!(" · {} · {}", c.location(), who(c.author.as_deref())),
            style::dim(),
        ),
    ];
    if !c.review_id.is_empty() {
        head.push(piece(format!(" · in {}", c.review_id), style::dim()));
    }
    p.line(head);

    let mark = status_mark(c.status);
    match c.status {
        Status::Done | Status::Rejected => p.line(vec![
            mark.clone(),
            piece(
                format!(
                    " {} by {} on {}",
                    c.status,
                    who(c.resolved_by.as_deref()),
                    day(c.resolved_at.as_deref().unwrap_or(""))
                ),
                mark.style,
            ),
        ]),
        Status::Pulled => p.line(vec![
            mark.clone(),
            piece(
                format!(
                    " pulled on {}, not decided yet",
                    day(c.pulled_at.as_deref().unwrap_or(""))
                ),
                mark.style,
            ),
        ]),
        Status::Pending => p.line(vec![
            mark.clone(),
            piece(format!(" pending since {}", day(&c.created_at)), mark.style),
        ]),
    }
    p.line(vec![piece(
        format!(
            "o opens {} at line {} · esc goes back",
            c.file, c.start_line
        ),
        style::dim(),
    )]);

    p.heading("Comment");
    p.text(&c.comment, "  ", style::plain());

    p.heading("Code as reviewed");
    let code = crate::highlight::source_lines(&c.code);
    let last = c.start_line + i64::try_from(code.len()).unwrap_or(i64::MAX) - 1;
    let num_w = last.to_string().len();
    for (i, l) in code.iter().enumerate() {
        let n = c.start_line + i64::try_from(i).unwrap_or(i64::MAX);
        p.line(vec![
            piece(format!("  {n:>num_w$}  "), style::line_nr()),
            piece(expand_tabs(l), style::plain()),
        ]);
    }

    if c.status.resolved() {
        p.heading("Resolution");
        match c.resolution_note.as_deref().filter(|n| !n.is_empty()) {
            Some(note) => p.text(note, "  ", style::plain()),
            None => p.line(vec![piece("  no note", style::dim())]),
        }
    }
    if c.status == Status::Done {
        p.heading("Changes while resolving");
        if !ev.diff_available {
            p.line(vec![piece(
                "  no snapshot of the file to compare",
                style::dim(),
            )]);
        } else if ev.diff.is_empty() {
            p.line(vec![piece("  nothing changed in this file", style::dim())]);
        }
        for l in &ev.diff {
            p.line(vec![piece("  ", style::plain()), diff_piece(l)]);
        }
    }
    p.lines
}

/// One line of a unified diff, colored by its kind.
#[must_use]
pub fn diff_piece(l: &str) -> Piece {
    let l = expand_tabs(l);
    let st = if l.starts_with("+++") || l.starts_with("---") {
        style::dim()
    } else if l.starts_with("@@") {
        style::sign_style(style::Sign::Changed)
    } else if l.starts_with('+') {
        style::sign_style(style::Sign::Added)
    } else if l.starts_with('-') {
        style::sign_style(style::Sign::Deleted)
    } else {
        style::plain()
    };
    piece(l, st)
}

/// The page for one submitted review: its summary and each linked comment.
#[must_use]
pub fn sheet_page(sheet: &ReviewSheet, width: usize) -> Vec<Vec<Piece>> {
    let review = &sheet.review;
    let mut page = Page::new(width);
    page.line(vec![
        piece(review.id.as_str(), style::id()),
        piece(
            format!(
                " · {} · {} · submitted on {}",
                review.decision,
                who(review.author.as_deref()),
                day(&review.created_at)
            ),
            style::dim(),
        ),
    ]);
    let mark = state_mark(sheet.state);
    page.line(vec![
        mark.clone(),
        piece(format!(" {}", sheet.state.as_str()), mark.style),
    ]);
    page.line(vec![piece(
        "open a comment in the sidebar for its code and changes · esc goes back",
        style::dim(),
    )]);

    page.heading("Summary");
    page.text(&review.summary, "  ", style::plain());

    page.heading("Comments");
    if review.comment_ids.is_empty() {
        page.line(vec![piece(
            "  summary only: no comments linked",
            style::dim(),
        )]);
    }
    for id in &review.comment_ids {
        let Some(c) = sheet.comments.iter().find(|c| &c.id == id) else {
            page.line(vec![
                piece("  ", style::plain()),
                piece(id.as_str(), style::id()),
                piece(" no longer exists", style::dim()),
            ]);
            continue;
        };
        page.line(vec![
            piece("  ", style::plain()),
            status_mark(c.status),
            piece(" ", style::plain()),
            piece(c.id.as_str(), style::id()),
            piece(format!(" {}", c.location()), style::dim()),
        ]);
        page.text(&first_line(&c.comment), "      ", style::plain());
        if let Some(note) = c.resolution_note.as_deref().filter(|n| !n.is_empty()) {
            let by = who(c.resolved_by.as_deref());
            page.text(&format!("{by}: {note}"), "      ", style::dim());
        }
    }
    page.lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use rvw_core::types::{Review, ReviewStatus};

    fn text_of(page: &[Vec<Piece>]) -> String {
        page.iter()
            .map(|row| row.iter().map(|p| p.text.as_str()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn wrap_breaks_at_spaces_and_splits_long_words() {
        assert_eq!(wrap("aa bb cc", 5), vec!["aa bb", "cc"]);
        assert_eq!(wrap("abcdefg", 3), vec!["abc", "def", "g"]);
        assert_eq!(wrap("", 10), vec![""]);
        assert_eq!(wrap("a\nb", 10), vec!["a", "b"]);
    }

    #[test]
    fn who_and_day_read_stored_values() {
        assert_eq!(who(Some("claude")), "@claude");
        assert_eq!(who(None), "someone");
        assert_eq!(who(Some("")), "someone");
        assert_eq!(day("2026-10-12T09:00:00Z"), "2026-10-12");
        assert_eq!(day(""), "");
    }

    #[test]
    fn short_location_drops_directories() {
        assert_eq!(location("parse.py", 5, 8), "parse.py:5-8");
        assert_eq!(location("util.py", 2, 2), "util.py:2");
    }

    #[test]
    fn marks_match_each_status() {
        assert_eq!(status_mark(Status::Done).text, "✓");
        assert_eq!(status_mark(Status::Rejected).text, "✗");
        assert_eq!(status_mark(Status::Pulled).text, "◐");
        assert_eq!(status_mark(Status::Pending).text, "○");
        assert_eq!(state_mark(SheetState::Complete).text, "✓");
        assert_eq!(state_mark(SheetState::Pulled).text, "◐");
        assert_eq!(state_mark(SheetState::Pending).text, "○");
    }

    #[test]
    fn diff_lines_take_their_kind_style() {
        assert_eq!(
            diff_piece("+x").style,
            style::sign_style(style::Sign::Added)
        );
        assert_eq!(
            diff_piece("-x").style,
            style::sign_style(style::Sign::Deleted)
        );
        assert_eq!(
            diff_piece("@@ -1 +1 @@").style,
            style::sign_style(style::Sign::Changed)
        );
        assert_eq!(diff_piece("+++ b/x").style, style::dim());
        assert_eq!(diff_piece(" ctx").style, style::plain());
    }

    #[test]
    fn page_headings_separate_sections_with_a_blank_line() {
        let mut p = Page::new(40);
        p.heading("One");
        p.line(vec![piece("body", style::plain())]);
        p.heading("Two");
        assert_eq!(p.lines.len(), 4);
        assert!(p.lines[2].is_empty());
        assert_eq!(p.lines[0][0].text, " ");
    }

    #[test]
    fn page_text_is_led_and_wrapped_to_the_width() {
        let mut p = Page::new(12);
        p.text("aaa bbb ccc", "  ", style::plain());
        let rows: Vec<String> = p
            .lines
            .iter()
            .map(|r| r.iter().map(|x| x.text.as_str()).collect())
            .collect();
        assert_eq!(rows, vec!["   aaa bbb", "   ccc"]);
    }

    fn comment(id: &str, status: Status) -> Comment {
        Comment {
            points_at: None,
            reopens: None,
            verdict: None,
            id: id.to_owned(),
            status,
            workspace: "/ws".to_owned(),
            lane: None,
            author: Some("reviewer".to_owned()),
            file: "src/util.py".to_owned(),
            path: "src/util.py".to_owned(),
            start_line: 2,
            end_line: 2,
            filetype: "python".to_owned(),
            code: "x = 1\n".to_owned(),
            comment: "typo".to_owned(),
            created_at: "2026-01-01T00:00:00Z".to_owned(),
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

    fn review(id: &str, comment_ids: &[&str]) -> Review {
        Review {
            id: id.to_owned(),
            status: ReviewStatus::Pending,
            workspace: "/ws".to_owned(),
            lane: None,
            author: Some("reviewer".to_owned()),
            decision: "request-changes".to_owned(),
            summary: "Fix both before merging.".to_owned(),
            comment_ids: comment_ids.iter().map(|s| (*s).to_owned()).collect(),
            created_at: "2026-01-05T10:00:00Z".to_owned(),
            pulled_at: None,
        }
    }

    #[test]
    fn a_review_sheet_lists_each_comment_with_its_mark() {
        let mut done = comment("r1", Status::Done);
        done.file = "src/api/parse.py".to_owned();
        done.start_line = 5;
        done.end_line = 8;
        done.resolution_note = Some("extracted parse_x()".to_owned());
        done.resolved_by = Some("claude".to_owned());
        let sheet = ReviewSheet {
            review: review("rv1", &["r1", "r9"]),
            state: SheetState::Pulled,
            comments: vec![done],
        };
        let text = text_of(&sheet_page(&sheet, 70));
        for want in [
            "rv1 · request-changes · @reviewer · submitted on 2026-01-05",
            "pulled",
            "Fix both before merging.",
            "✓ r1 src/api/parse.py:5-8",
            "@claude: extracted parse_x()",
            "r9 no longer exists",
        ] {
            assert!(text.contains(want), "no {want:?} in:\n{text}");
        }
    }

    #[test]
    fn a_summary_only_review_says_so() {
        let sheet = ReviewSheet {
            review: review("rv2", &[]),
            state: SheetState::Complete,
            comments: Vec::new(),
        };
        let text = text_of(&sheet_page(&sheet, 70));
        assert!(text.contains("summary only: no comments linked"));
    }

    #[test]
    fn a_pending_comment_says_when_it_was_asked() {
        let mut c = comment("r4", Status::Pending);
        c.created_at = "2026-01-02T03:04:05Z".to_owned();
        c.comment = "rename it".to_owned();
        c.code = "let x = 1;\n".to_owned();
        c.start_line = 1;
        c.end_line = 1;
        let ev = Evidence {
            comment: c,
            diff: Vec::new(),
            diff_available: false,
        };
        let text = text_of(&evidence_page(&ev, 60));
        assert!(text.contains("pending since 2026-01-02"));
        assert!(text.contains("rename it"));
        assert!(!text.contains("Resolution"));
    }

    #[test]
    fn a_done_comment_shows_its_resolution_and_changes() {
        let mut c = comment("r1", Status::Done);
        c.file = "src/api/parse.py".to_owned();
        c.start_line = 5;
        c.end_line = 8;
        c.code = "if x:\n    body\n".to_owned();
        c.resolved_by = Some("claude".to_owned());
        c.resolved_at = Some("2026-02-03T00:00:00Z".to_owned());
        c.resolution_note = Some("extracted parse_x()".to_owned());
        let ev = Evidence {
            comment: c,
            diff: vec!["-    body".to_owned(), "+    parse_x()".to_owned()],
            diff_available: true,
        };
        let text = text_of(&evidence_page(&ev, 60));
        for want in [
            "r1 · src/api/parse.py:5-8 · @reviewer",
            "done by @claude on 2026-02-03",
            "   5  if x:",
            "extracted parse_x()",
            "Changes while resolving",
            "-    body",
            "+    parse_x()",
            "o opens src/api/parse.py at line 5",
        ] {
            assert!(text.contains(want), "no {want:?} in:\n{text}");
        }
    }

    #[test]
    fn a_rejected_comment_has_no_changes_section() {
        let mut c = comment("r2", Status::Rejected);
        c.resolved_by = Some("claude".to_owned());
        c.resolved_at = Some("2026-02-03T00:00:00Z".to_owned());
        c.resolution_note = Some("not a typo: pass is intended".to_owned());
        let ev = Evidence {
            comment: c,
            diff: Vec::new(),
            diff_available: true,
        };
        let text = text_of(&evidence_page(&ev, 60));
        assert!(text.contains("rejected by @claude"));
        assert!(text.contains("not a typo: pass is intended"));
        assert!(!text.contains("Changes while resolving"));
    }

    #[test]
    fn a_done_comment_without_a_snapshot_says_so() {
        let c = comment("r3", Status::Done);
        let ev = Evidence {
            comment: c,
            diff: Vec::new(),
            diff_available: false,
        };
        let text = text_of(&evidence_page(&ev, 60));
        assert!(text.contains("no snapshot of the file to compare"));
        assert!(text.contains("no note"));
    }
}
