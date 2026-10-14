//! Fixed-width review lines and review sheets. Port of
//! internal/render/display.go.

use std::collections::HashMap;

use rvw_core::operations::{Evidence, ReviewSheet};
use rvw_core::types::{Comment, Status, Verdict, VerdictKind};

use crate::render::{attribution, link_lines};

/// The display column limit; no rendered line exceeds it.
const WIDTH: usize = 80;

/// Lines being assembled, one string per display line.
#[derive(Default)]
struct Lines(Vec<String>);

impl Lines {
    /// Appends `text` wrapped to the display width, continuation lines indented
    /// under `prefix`.
    fn line(&mut self, text: &str, prefix: &str) {
        let prefix_width = prefix.chars().count();
        let limit = WIDTH.saturating_sub(prefix_width).max(1);
        let chunks = wrap(&text.replace('\t', "        "), limit);
        let indent = " ".repeat(prefix_width);
        for (i, chunk) in chunks.iter().enumerate() {
            let lead = if i == 0 { prefix } else { &indent };
            self.0.push(format!("{lead}{chunk}"));
        }
    }

    fn raw(&mut self, lines: &[&str]) {
        self.0.extend(lines.iter().map(|s| (*s).to_owned()));
    }

    fn rule(&mut self) {
        self.0.push("─".repeat(WIDTH));
    }

    fn source(&mut self, c: &Comment) {
        self.raw(&["SOURCE"]);
        let digits = c.end_line.to_string().len();
        for (i, code) in c.code.split('\n').enumerate() {
            // A comment's code is a handful of lines, so the offset never overflows.
            #[allow(clippy::cast_possible_wrap)]
            let n = c.start_line + i as i64;
            let marker = if n <= c.end_line { ">" } else { " " };
            self.line(code, &format!("{marker} {n:>digits$} | "));
        }
    }

    fn note(&mut self, text: &str) {
        self.raw(&["", "NOTE"]);
        for l in text.split('\n') {
            self.line(l, "  ");
        }
    }

    /// The links a comment carries, under its heading.
    fn links(&mut self, c: &Comment) {
        for l in link_lines(c) {
            self.line(&l, "");
        }
    }

    /// The resolution of a done or rejected comment: who and when, then the note.
    fn resolution(&mut self, c: &Comment) {
        self.raw(&["", "RESOLUTION"]);
        if c.resolved_by.is_some() || c.resolved_at.is_some() {
            let mut details = c.status.as_str().to_owned();
            if let Some(by) = &c.resolved_by {
                details.push_str(" by @");
                details.push_str(by);
            }
            if let Some(at) = &c.resolved_at {
                details.push_str(" at ");
                details.push_str(at);
            }
            self.line(&details, "  ");
        }
        if let Some(note) = &c.resolution_note {
            for l in note.split('\n') {
                self.line(l, "  ");
            }
        }
    }

    /// A reviewer's verdict: its line, its note quoted as the resolution note
    /// is, and the follow-up a reopen created.
    fn verdict(&mut self, v: &Verdict) {
        self.raw(&[""]);
        self.line(
            &format!(
                "Verdict: {} by {} ({}) at {}.",
                v.kind.as_str(),
                v.by,
                v.actor.as_str(),
                v.at
            ),
            "",
        );
        if let Some(note) = &v.note {
            for l in note.split('\n') {
                self.line(l, "  ");
            }
        }
        if let (VerdictKind::Reopened, Some(id)) = (v.kind, &v.followup) {
            self.line(&format!("Reopened as {id}."), "");
        }
    }

    /// The lines joined, trailing blanks trimmed, with one final newline.
    fn finish(&self) -> String {
        format!(
            "{}\n",
            self.0.join("\n").trim_end_matches([' ', '\t', '\n'])
        )
    }
}

/// One comment as a fixed-width review line: its source (or, once done, the
/// diff it produced), and its note or resolution.
#[must_use]
pub fn display(ev: &Evidence) -> String {
    let c = &ev.comment;
    let mut s = Lines::default();
    s.line(
        &format!(
            "REVIEW LINE {}  [{}]  {}",
            c.id,
            c.status.as_str().to_uppercase(),
            c.location()
        ),
        "",
    );
    s.links(c);
    let who = attribution(c.author.as_deref(), c.lane.as_deref());
    if !who.is_empty() {
        s.line(&format!("REVIEWER {who}"), "");
    }
    s.rule();

    if c.status == Status::Done {
        s.raw(&["DIFF"]);
        if !ev.diff_available {
            s.line("snapshot evidence unavailable", "  ");
        } else if ev.diff.is_empty() {
            s.line("no changes recorded", "  ");
        } else {
            for d in &ev.diff {
                s.line(d, "  ");
            }
        }
    } else {
        s.source(c);
    }

    if c.status.resolved() {
        s.resolution(c);
    } else {
        s.note(&c.comment);
    }
    if let Some(v) = &c.verdict {
        s.verdict(v);
    }
    s.finish()
}

/// A submitted review: decision, summary, a ledger of its comments in order
/// (missing ones named), then each comment's evidence.
#[must_use]
pub fn sheet(rs: &ReviewSheet) -> String {
    let r = &rs.review;
    let by_id: HashMap<&str, &Comment> = rs.comments.iter().map(|c| (c.id.as_str(), c)).collect();
    let mut s = Lines::default();
    s.line(
        &format!(
            "REVIEW SHEET {}  [{}]",
            r.id,
            rs.state.as_str().to_uppercase()
        ),
        "",
    );
    let who = attribution(r.author.as_deref(), r.lane.as_deref());
    if !who.is_empty() {
        s.line(&format!("REVIEWER {who}"), "");
    }
    s.line(
        &format!("DECISION  {}", r.decision.replace('-', " ").to_uppercase()),
        "",
    );
    s.rule();
    s.raw(&["SUMMARY"]);
    for l in r.summary.split('\n') {
        s.line(l, "  ");
    }

    s.raw(&["", "LEDGER"]);
    if r.comment_ids.is_empty() {
        s.line("(no linked comments)", "  ");
    }
    for (i, id) in r.comment_ids.iter().enumerate() {
        match by_id.get(id.as_str()) {
            None => s.line(&format!("MISSING {id}"), "  "),
            Some(c) => s.line(
                &format!(
                    "{}. {}  {}  [{}]",
                    i + 1,
                    c.id,
                    c.location(),
                    c.status.as_str().to_uppercase()
                ),
                "  ",
            ),
        }
    }

    s.rule();
    for (i, id) in r.comment_ids.iter().enumerate() {
        let Some(c) = by_id.get(id.as_str()) else {
            continue;
        };
        if i > 0 {
            s.raw(&[""]);
        }
        s.line(
            &format!("EVIDENCE {}  {}  {}", i + 1, c.id, c.location()),
            "",
        );
        s.links(c);
        s.source(c);
        s.note(&c.comment);
        if let Some(v) = &c.verdict {
            s.verdict(v);
        }
    }
    s.finish()
}

/// Fills `text` into lines of at most `limit` chars, breaking at whitespace and
/// splitting words longer than a line. Whitespace is kept as written.
fn wrap(text: &str, limit: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut cur: Vec<char> = Vec::new();
    for tok in tokens(text) {
        let mut rest = tok.as_slice();
        while !rest.is_empty() {
            if cur.len() + rest.len() <= limit {
                cur.extend_from_slice(rest);
                break;
            }
            if rest.len() <= limit && !cur.is_empty() {
                lines.push(cur.drain(..).collect());
                continue;
            }
            let room = limit - cur.len();
            cur.extend_from_slice(&rest[..room]);
            rest = &rest[room..];
            lines.push(cur.drain(..).collect());
        }
    }
    if !cur.is_empty() || lines.is_empty() {
        lines.push(cur.drain(..).collect());
    }
    lines
}

/// Splits `text` into alternating runs of whitespace and non-whitespace.
fn tokens(text: &str) -> Vec<Vec<char>> {
    let mut out = Vec::new();
    let mut cur: Vec<char> = Vec::new();
    let mut space = false;
    for r in text.chars() {
        let is_space = r.is_whitespace();
        if !cur.is_empty() && is_space != space {
            out.push(std::mem::take(&mut cur));
        }
        space = is_space;
        cur.push(r);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use rvw_core::operations::SheetState;
    use rvw_core::types::{Location, Status, VerdictKind};

    use super::*;
    use crate::fixtures::{comment, review, verdict};

    fn located(file: &str, start: i64, end: i64) -> Location {
        Location {
            file: file.into(),
            start_line: start,
            end_line: end,
        }
    }

    /// A comment ann raised, handed over, and resolved with a note.
    fn decided(status: Status) -> Comment {
        let mut c = comment("r7", status, "a.go", 3, 3, "x", "n");
        c.author = Some("ann".into());
        c.pulled_at = Some("2026-01-02T00:00:00Z".into());
        c.resolved_by = Some("ann".into());
        c.resolved_at = Some("2026-01-02T01:00:00Z".into());
        c.resolution_note = Some("fixed it".into());
        c
    }

    fn evidence(c: Comment, diff: &[&str], diff_available: bool) -> Evidence {
        Evidence {
            comment: c,
            diff: diff.iter().map(|s| (*s).to_owned()).collect(),
            diff_available,
        }
    }

    fn rule() -> String {
        "─".repeat(80)
    }

    /// The lines joined with newlines, plus the final newline.
    fn joined(lines: &[&str]) -> String {
        let mut s = lines.join("\n");
        s.push('\n');
        s
    }

    #[test]
    fn wrap_breaks_at_whitespace_and_keeps_it() {
        assert_eq!(wrap("hello world foo", 8), vec!["hello ", "world ", "foo"]);
    }

    #[test]
    fn wrap_splits_a_word_longer_than_a_line() {
        let word = "a".repeat(100);
        let got = wrap(&word, 80);
        assert_eq!(got, vec!["a".repeat(80), "a".repeat(20)]);
    }

    #[test]
    fn wrap_keeps_a_line_of_exactly_the_limit_whole() {
        let line = "b".repeat(80);
        assert_eq!(wrap(&line, 80), vec![line.clone()]);
        let over = "b".repeat(81);
        assert_eq!(wrap(&over, 80), vec!["b".repeat(80), "b".to_owned()]);
    }

    #[test]
    fn wrap_of_empty_text_is_one_empty_line() {
        assert_eq!(wrap("", 10), vec![""]);
    }

    #[test]
    fn wrap_moves_a_word_that_fits_to_a_new_line_rather_than_splitting_it() {
        assert_eq!(wrap("abcdef ghi", 8), vec!["abcdef ", "ghi"]);
    }

    #[test]
    fn tokens_alternate_whitespace_and_words() {
        let got: Vec<String> = tokens("  a b\r\n")
            .into_iter()
            .map(|t| t.into_iter().collect())
            .collect();
        assert_eq!(got, vec!["  ", "a", " ", "b", "\r\n"]);
    }

    #[test]
    fn line_prefixes_the_first_chunk_and_indents_the_rest_to_the_width() {
        let mut s = Lines::default();
        s.line(&"x".repeat(100), "  ");
        assert_eq!(s.0.len(), 2);
        assert_eq!(s.0[0], format!("  {}", "x".repeat(78)));
        assert_eq!(s.0[0].chars().count(), 80);
        assert_eq!(s.0[1], format!("  {}", "x".repeat(22)));
    }

    #[test]
    fn line_replaces_tabs_with_eight_spaces() {
        let mut s = Lines::default();
        s.line("a\tb", "");
        assert_eq!(s.0, vec!["a        b"]);
    }

    #[test]
    fn line_keeps_carriage_returns_as_written() {
        let mut s = Lines::default();
        s.line("ok\r", "  ");
        assert_eq!(s.0, vec!["  ok\r"]);
    }

    #[test]
    fn source_marks_the_commented_lines_and_right_aligns_numbers() {
        let mut c = comment("r1", Status::Pending, "a.go", 9, 10, "a\nb\nc", "n");
        c.end_line = 10;
        let mut s = Lines::default();
        s.source(&c);
        assert_eq!(
            s.0,
            vec!["SOURCE", ">  9 | a", "> 10 | b", "  11 | c"]
                .into_iter()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn pending_comment_shows_source_and_note() {
        let mut c = comment(
            "r7",
            Status::Pending,
            "a.go",
            3,
            4,
            "fn a() {\n    b();\n}",
            "check this\nand this",
        );
        c.author = Some("ann".into());
        let got = display(&evidence(c, &[], false));
        let r = rule();
        assert_eq!(
            got,
            joined(&[
                "REVIEW LINE r7  [PENDING]  a.go:3-4",
                "REVIEWER @ann",
                &r,
                "SOURCE",
                "> 3 | fn a() {",
                "> 4 |     b();",
                "  5 | }",
                "",
                "NOTE",
                "  check this",
                "  and this",
            ])
        );
    }

    #[test]
    fn done_comment_shows_its_diff_and_resolution() {
        let mut c = comment("r7", Status::Done, "a.go", 3, 3, "x", "n");
        c.author = Some("ann".into());
        c.lane = Some("api".into());
        c.resolved_by = Some("ann".into());
        c.resolved_at = Some("2026-01-02T00:00:00Z".into());
        c.resolution_note = Some("fixed it".into());
        let got = display(&evidence(c, &["-old", "+new"], true));
        let r = rule();
        assert_eq!(
            got,
            joined(&[
                "REVIEW LINE r7  [DONE]  a.go:3",
                "REVIEWER @ann #api",
                &r,
                "DIFF",
                "  -old",
                "  +new",
                "",
                "RESOLUTION",
                "  done by @ann at 2026-01-02T00:00:00Z",
                "  fixed it",
            ])
        );
    }

    #[test]
    fn done_comment_without_snapshots_or_changes_says_so() {
        let c = comment("r7", Status::Done, "a.go", 3, 3, "x", "n");
        let unavailable = display(&evidence(c.clone(), &[], false));
        assert!(unavailable.contains("DIFF\n  snapshot evidence unavailable\n"));
        let empty = display(&evidence(c, &[], true));
        assert!(empty.contains("DIFF\n  no changes recorded\n"));
    }

    #[test]
    fn rejected_comment_without_who_or_when_shows_only_its_note() {
        let mut c = comment("r8", Status::Rejected, "b.go", 5, 5, "y", "n");
        c.resolution_note = Some("not now".into());
        let got = display(&evidence(c, &[], false));
        assert!(got.contains("SOURCE\n> 5 | y\n"));
        assert!(!got.contains("NOTE"));
        assert!(got.ends_with("\nRESOLUTION\n  not now\n"));
    }

    #[test]
    fn rejected_comment_names_who_and_when_before_the_note() {
        let mut c = comment("r8", Status::Rejected, "b.go", 5, 5, "y", "n");
        c.resolved_by = Some("bob".into());
        c.resolved_at = Some("now".into());
        let got = display(&evidence(c, &[], false));
        assert!(got.ends_with("\nRESOLUTION\n  rejected by @bob at now\n"));
    }

    #[test]
    fn long_source_and_notes_wrap_inside_the_width() {
        let long = "w ".repeat(60);
        let c = comment("r1", Status::Pending, "a.go", 1, 1, "x", long.trim_end());
        for line in display(&evidence(c, &[], false)).lines() {
            assert!(line.chars().count() <= 80, "line too wide: {line:?}");
        }
    }

    #[test]
    fn note_lines_keep_crlf_and_empty_lines() {
        let c = comment("r1", Status::Pending, "a.go", 1, 1, "x", "one\r\n\ntwo");
        let got = display(&evidence(c, &[], false));
        assert!(got.ends_with("NOTE\n  one\r\n  \n  two\n"));
    }

    #[test]
    fn sheet_ledger_names_missing_comments_and_skips_their_evidence() {
        let mut r = review(
            "rv2",
            "request-changes",
            "Fix two\nthings",
            &["r1", "r9", "r2"],
        );
        r.author = Some("bob".into());
        r.lane = Some("api".into());
        let c1 = comment("r1", Status::Pending, "a.go", 1, 1, "x", "n1");
        let c2 = comment("r2", Status::Done, "b.go", 2, 2, "y", "n2");
        let rs = ReviewSheet {
            review: r,
            state: SheetState::Pending,
            comments: vec![c1, c2],
        };
        let got = sheet(&rs);
        let rl = rule();
        assert_eq!(
            got,
            joined(&[
                "REVIEW SHEET rv2  [PENDING]",
                "REVIEWER @bob #api",
                "DECISION  REQUEST CHANGES",
                &rl,
                "SUMMARY",
                "  Fix two",
                "  things",
                "",
                "LEDGER",
                "  1. r1  a.go:1  [PENDING]",
                "  MISSING r9",
                "  3. r2  b.go:2  [DONE]",
                &rl,
                "EVIDENCE 1  r1  a.go:1",
                "SOURCE",
                "> 1 | x",
                "",
                "NOTE",
                "  n1",
                "",
                "EVIDENCE 3  r2  b.go:2",
                "SOURCE",
                "> 2 | y",
                "",
                "NOTE",
                "  n2",
            ])
        );
    }

    #[test]
    fn sheet_with_no_linked_comments_says_so() {
        let r = review("rv3", "approve", "", &[]);
        let rs = ReviewSheet {
            review: r,
            state: SheetState::Complete,
            comments: vec![],
        };
        let got = sheet(&rs);
        let rl = rule();
        assert_eq!(
            got,
            joined(&[
                "REVIEW SHEET rv3  [COMPLETE]",
                "DECISION  APPROVE",
                &rl,
                "SUMMARY",
                "  ",
                "",
                "LEDGER",
                "  (no linked comments)",
                &rl,
            ])
        );
    }

    #[test]
    fn sheet_decision_comment_is_uppercased_without_hyphens() {
        let r = review("rv4", "comment", "s", &[]);
        let rs = ReviewSheet {
            review: r,
            state: SheetState::Pulled,
            comments: vec![],
        };
        assert!(sheet(&rs).contains("DECISION  COMMENT\n"));
    }

    #[test]
    fn points_at_and_reopens_follow_the_heading_when_set() {
        let mut c = comment("r7", Status::Pending, "a.go", 3, 3, "x", "n");
        c.points_at = Some(located("b.go", 5, 6));
        c.reopens = Some("r3".into());
        let got = display(&evidence(c, &[], false));
        let r = rule();
        assert_eq!(
            got,
            joined(&[
                "REVIEW LINE r7  [PENDING]  a.go:3",
                "Points at `b.go:5-6`.",
                "Reopens r3.",
                &r,
                "SOURCE",
                "> 3 | x",
                "",
                "NOTE",
                "  n",
            ])
        );
    }

    #[test]
    fn accepted_verdict_without_a_note_is_one_line_after_the_resolution() {
        let mut c = decided(Status::Done);
        c.verdict = Some(verdict(VerdictKind::Accepted, None, None));
        let got = display(&evidence(c, &[], false));
        let r = rule();
        assert_eq!(
            got,
            joined(&[
                "REVIEW LINE r7  [DONE]  a.go:3",
                "REVIEWER @ann",
                &r,
                "DIFF",
                "  snapshot evidence unavailable",
                "",
                "RESOLUTION",
                "  done by @ann at 2026-01-02T01:00:00Z",
                "  fixed it",
                "",
                "Verdict: accepted by alice (human) at 2026-01-03T00:00:00Z.",
            ])
        );
    }

    #[test]
    fn accepted_verdict_with_a_note_quotes_it_like_the_resolution_note() {
        let mut c = decided(Status::Done);
        c.verdict = Some(verdict(VerdictKind::Accepted, Some("agreed\nthanks"), None));
        let got = display(&evidence(c, &[], false));
        assert!(got.ends_with(
            "\nVerdict: accepted by alice (human) at 2026-01-03T00:00:00Z.\n  agreed\n  thanks\n"
        ));
        assert!(!got.contains("Reopened as"));
    }

    #[test]
    fn reopened_verdict_names_its_reason_and_the_follow_up() {
        let mut c = decided(Status::Rejected);
        c.resolution_note = Some("not now".into());
        c.verdict = Some(verdict(
            VerdictKind::Reopened,
            Some("still broken"),
            Some("r9"),
        ));
        let got = display(&evidence(c, &[], false));
        let r = rule();
        assert_eq!(
            got,
            joined(&[
                "REVIEW LINE r7  [REJECTED]  a.go:3",
                "REVIEWER @ann",
                &r,
                "SOURCE",
                "> 3 | x",
                "",
                "RESOLUTION",
                "  rejected by @ann at 2026-01-02T01:00:00Z",
                "  not now",
                "",
                "Verdict: reopened by alice (human) at 2026-01-03T00:00:00Z.",
                "  still broken",
                "Reopened as r9.",
            ])
        );
    }

    #[test]
    fn sheet_evidence_shows_links_and_the_verdict_after_the_note() {
        let mut r = review("rv2", "request-changes", "Fix", &["r1"]);
        r.author = None;
        let mut c = comment("r1", Status::Done, "a.go", 1, 1, "x", "n1");
        c.points_at = Some(located("b.go", 9, 9));
        c.verdict = Some(verdict(VerdictKind::Accepted, Some("ok"), None));
        let rs = ReviewSheet {
            review: r,
            state: SheetState::Complete,
            comments: vec![c],
        };
        let got = sheet(&rs);
        assert!(got.contains(
            "EVIDENCE 1  r1  a.go:1\nPoints at `b.go:9`.\nSOURCE\n> 1 | x\n\nNOTE\n  n1\n\n\
             Verdict: accepted by alice (human) at 2026-01-03T00:00:00Z.\n  ok\n"
        ));
    }

    #[test]
    fn sheet_ledger_rows_stay_one_line_each() {
        let mut r = review("rv2", "approve", "", &["r1"]);
        r.author = None;
        let mut c = comment("r1", Status::Done, "a.go", 1, 1, "x", "n1");
        c.points_at = Some(located("b.go", 9, 9));
        c.verdict = Some(verdict(VerdictKind::Accepted, None, None));
        let rs = ReviewSheet {
            review: r,
            state: SheetState::Complete,
            comments: vec![c],
        };
        assert!(sheet(&rs).contains("  1. r1  a.go:1  [DONE]\n"));
    }
}
