//! Text, markdown and JSON renderings of service outputs. Port of
//! internal/render/render.go.

use std::io::{self, Write};

use rvw_core::operations::ReviewSheet;
use rvw_core::types::{Comment, Handoff, Review};
use serde::Serialize;

/// Writes `v` as JSON indented by two spaces, followed by a newline. Characters
/// such as `<`, `>` and `&` are written as they are, while U+2028 and U+2029
/// are escaped, as Go always does.
///
/// # Errors
///
/// Returns an error when `v` cannot be serialized or `w` cannot be written.
pub fn json<T: Serialize>(w: &mut impl Write, v: &T) -> io::Result<()> {
    let text = serde_json::to_string_pretty(v).map_err(io::Error::other)?;
    let escaped = text
        .replace('\u{2028}', "\x5cu2028")
        .replace('\u{2029}', "\x5cu2029");
    w.write_all(escaped.as_bytes())?;
    w.write_all(b"\n")
}

/// The JSON shape of `list` and `pull`: comments under "reviews" and submitted
/// reviews under `submitted_reviews`, kept for existing consumers.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Envelope {
    pub workspace: String,
    pub count: usize,
    pub reviews: Vec<Comment>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub submitted_reviews: Vec<Handoff>,
}

/// The first line of `text`, trimmed of surrounding blank lines.
#[must_use]
pub fn first_line(text: &str) -> &str {
    let trimmed = text.trim();
    trimmed.split_once('\n').map_or(trimmed, |(line, _)| line)
}

/// `@author #lane`, omitting whichever is absent or empty.
#[must_use]
pub fn attribution(author: Option<&str>, lane: Option<&str>) -> String {
    let mut bits = Vec::new();
    if let Some(a) = author.filter(|a| !a.is_empty()) {
        bits.push(format!("@{a}"));
    }
    if let Some(l) = lane.filter(|l| !l.is_empty()) {
        bits.push(format!("#{l}"));
    }
    bits.join(" ")
}

/// The one-line summary of a comment: `r3  file:2-4  @who  note`.
#[must_use]
pub fn comment_line(c: &Comment, id_width: usize) -> String {
    let mut line = format!("{:<id_width$}  {}  ", c.id, c.location());
    let who = attribution(c.author.as_deref(), c.lane.as_deref());
    if !who.is_empty() {
        line.push_str(&who);
        line.push_str("  ");
    }
    line.push_str(first_line(&c.comment));
    line
}

/// The one-line summary of a submitted review.
#[must_use]
pub fn review_line(r: &Review) -> String {
    format!(
        "{}  {}  {} review comment(s)  {}",
        r.id,
        r.decision,
        r.comment_ids.len(),
        first_line(&r.summary)
    )
}

/// The one-line summary of a review sheet:
/// `rv1  [pending]  approve  2 review comment(s)  @who  summary`.
#[must_use]
pub fn sheet_line(rs: &ReviewSheet) -> String {
    let r = &rs.review;
    let mut line = format!(
        "{}  [{}]  {}  {} review comment(s)  ",
        r.id,
        rs.state.as_str(),
        r.decision,
        r.comment_ids.len()
    );
    let who = attribution(r.author.as_deref(), r.lane.as_deref());
    if !who.is_empty() {
        line.push_str(&who);
        line.push_str("  ");
    }
    line.push_str(first_line(&r.summary));
    line
}

/// Writes handoffs one per line, reviews first, and reports whether it wrote
/// anything.
///
/// # Errors
///
/// Returns an error when `w` cannot be written.
pub fn text(w: &mut impl Write, reviews: &[Handoff], comments: &[Comment]) -> io::Result<bool> {
    for r in reviews {
        writeln!(w, "{}", review_line(&r.review))?;
    }
    let width = comments.iter().map(|c| c.id.len()).max().unwrap_or(0);
    for c in comments {
        writeln!(w, "{}", comment_line(c, width))?;
        for line in link_lines(c) {
            writeln!(w, "{line}")?;
        }
    }
    Ok(!reviews.is_empty() || !comments.is_empty())
}

/// `file:N` or `file:N-M`, the form `Comment::location` gives a range.
pub(crate) fn span(file: &str, start: i64, end: i64) -> String {
    if start == end {
        format!("{file}:{start}")
    } else {
        format!("{file}:{start}-{end}")
    }
}

/// The lines a comment adds under its heading when it has them: where it
/// points at, then the decision it reopens. Empty for a plain comment.
pub(crate) fn link_lines(c: &Comment) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(p) = &c.points_at {
        lines.push(format!(
            "Points at `{}`.",
            span(&p.file, p.start_line, p.end_line)
        ));
    }
    if let Some(id) = &c.reopens {
        lines.push(format!("Reopens {id}."));
    }
    lines
}

/// Writes one id per line, reviews first.
///
/// # Errors
///
/// Returns an error when `w` cannot be written.
pub fn ids(w: &mut impl Write, reviews: &[Handoff], comments: &[Comment]) -> io::Result<()> {
    for r in reviews {
        writeln!(w, "{}", r.review.id)?;
    }
    for c in comments {
        writeln!(w, "{}", c.id)?;
    }
    Ok(())
}

/// The handoff an agent reads: each comment's location, the code as it stood,
/// and the note as a quote. `drained` says the comments have left the queue.
pub fn markdown(ws: &str, reviews: &[Handoff], comments: &[Comment], drained: bool) -> String {
    if reviews.is_empty() && comments.is_empty() {
        return format!("No pending review comments for {ws}.\n");
    }
    let mut out: Vec<String> = Vec::new();
    for h in reviews {
        let r = &h.review;
        out.push(format!("# Review {} — {ws}", r.id));
        out.push(String::new());
        out.push(format!("**Decision:** {}", decision_label(&r.decision)));
        out.push(String::new());
        out.push(r.summary.clone());
        out.push(String::new());
        out.extend(["## Review comments", ""].map(str::to_owned));
        for (i, c) in h.comments.iter().enumerate() {
            append_comment_markdown(&mut out, "###", i + 1, c);
        }
    }
    if !comments.is_empty() {
        out.push(format!("# Review comments — {ws}"));
        out.push(String::new());
        out.extend(
            [
                "Address the following review comments, in the order listed. Each names a file",
                "and line range, the code as it stood when the comment was written, and the",
                "reviewer's note as a quote.",
                "",
            ]
            .map(str::to_owned),
        );
    }
    if drained {
        out.extend(
            [
                "These comments have been dequeued — they will not appear again. Record what",
                "became of each one with `rvw resolve <id> --note ...`, or",
                "`rvw reject <id> --note ...` when you are not making the change.",
                "",
            ]
            .map(str::to_owned),
        );
    }
    for (i, c) in comments.iter().enumerate() {
        append_comment_markdown(&mut out, "##", i + 1, c);
    }
    format!("{}\n", out.join("\n").trim_end_matches([' ', '\t', '\n']))
}

fn append_comment_markdown(out: &mut Vec<String>, heading: &str, n: usize, c: &Comment) {
    let mut head = format!("{heading} {n}. `{}` ({}", c.location(), c.id);
    let who = attribution(c.author.as_deref(), c.lane.as_deref());
    if !who.is_empty() {
        head.push_str(" · ");
        head.push_str(&who);
    }
    out.push(format!("{head})"));
    out.extend(link_lines(c));
    out.push(String::new());
    out.push(format!("```{}", c.filetype));
    out.extend(c.code.split('\n').map(str::to_owned));
    out.push("```".to_owned());
    out.push(String::new());
    for line in c.comment.split('\n') {
        if line.is_empty() {
            out.push(">".to_owned());
        } else {
            out.push(format!("> {line}"));
        }
    }
    out.push(String::new());
}

/// "Request changes" for "request-changes".
fn decision_label(d: &str) -> String {
    let s = d.replace('-', " ");
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().chain(chars).collect(),
    }
}

#[cfg(test)]
mod tests {
    use rvw_core::types::{Location, Status, VerdictKind};

    use super::*;
    use crate::fixtures::{comment, review, verdict};

    fn handoff(r: Review, comments: Vec<Comment>) -> Handoff {
        Handoff {
            review: r,
            comments,
        }
    }

    fn located(file: &str, start: i64, end: i64) -> Location {
        Location {
            file: file.into(),
            start_line: start,
            end_line: end,
        }
    }

    #[test]
    fn text_lists_points_at_and_reopens_under_the_row_when_set() {
        let mut c = comment("r1", Status::Pending, "a.go", 1, 1, "x", "hi");
        c.points_at = Some(located("b.go", 5, 5));
        c.reopens = Some("r3".into());
        let mut out = Vec::new();
        text(&mut out, &[], &[c]).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "r1  a.go:1  hi\n\
             Points at `b.go:5`.\n\
             Reopens r3.\n"
        );
    }

    #[test]
    fn text_points_at_a_range_in_the_dashed_form() {
        let mut c = comment("r1", Status::Pending, "a.go", 1, 1, "x", "hi");
        c.points_at = Some(located("b.go", 2, 4));
        let mut out = Vec::new();
        text(&mut out, &[], &[c]).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "r1  a.go:1  hi\nPoints at `b.go:2-4`.\n"
        );
    }

    #[test]
    fn text_list_rows_never_show_a_verdict() {
        let mut c = comment("r1", Status::Done, "a.go", 1, 1, "x", "hi");
        c.verdict = Some(verdict(VerdictKind::Reopened, Some("again"), Some("r9")));
        let mut out = Vec::new();
        text(&mut out, &[], &[c]).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "r1  a.go:1  hi\n");
    }

    #[test]
    fn markdown_puts_points_at_and_reopens_under_the_heading() {
        let mut c = comment("r1", Status::Pending, "a.go", 2, 3, "x := 1", "first");
        c.filetype = "go".into();
        c.points_at = Some(located("b.go", 5, 7));
        c.reopens = Some("r3".into());
        let got = markdown("/ws", &[], &[c], false);
        assert_eq!(
            got,
            "# Review comments — /ws\n\
             \n\
             Address the following review comments, in the order listed. Each names a file\n\
             and line range, the code as it stood when the comment was written, and the\n\
             reviewer's note as a quote.\n\
             \n\
             ## 1. `a.go:2-3` (r1)\n\
             Points at `b.go:5-7`.\n\
             Reopens r3.\n\
             \n\
             ```go\n\
             x := 1\n\
             ```\n\
             \n\
             > first\n"
        );
    }

    #[test]
    fn markdown_of_a_review_comment_shows_its_links_under_its_heading() {
        let mut c = comment("r2", Status::Pulled, "c.go", 1, 1, "a", "in review");
        c.reopens = Some("r8".into());
        let h = handoff(review("rv3", "comment", "S", &["r2"]), vec![c]);
        let got = markdown("/ws", &[h], &[], false);
        assert!(got.contains("### 1. `c.go:1` (r2)\nReopens r8.\n\n```\na\n```\n"));
    }

    #[test]
    fn unset_links_add_no_lines_to_any_rendering() {
        let c = comment("r1", Status::Done, "a.go", 2, 3, "x := 1", "first");
        let rs = ReviewSheet {
            review: review("rv1", "approve", "ok", &["r1"]),
            state: rvw_core::operations::SheetState::Complete,
            comments: vec![c.clone()],
        };
        let ev = rvw_core::operations::Evidence {
            comment: c.clone(),
            diff: Vec::new(),
            diff_available: false,
        };
        let renderings = [
            text_of(&c),
            markdown("/ws", &[], std::slice::from_ref(&c), false),
            crate::display::display(&ev),
            crate::display::sheet(&rs),
        ];
        for got in renderings {
            assert!(!got.contains("Points at"), "{got}");
            assert!(!got.contains("Reopens"), "{got}");
            assert!(!got.contains("Verdict"), "{got}");
        }
    }

    fn text_of(c: &Comment) -> String {
        let mut out = Vec::new();
        text(&mut out, &[], std::slice::from_ref(c)).unwrap();
        String::from_utf8(out).unwrap()
    }

    fn sheet_of(state: rvw_core::operations::SheetState, r: Review) -> ReviewSheet {
        ReviewSheet {
            review: r,
            state,
            comments: Vec::new(),
        }
    }

    #[test]
    fn json_indents_by_two_and_ends_with_a_newline() {
        let mut out = Vec::new();
        json(&mut out, &serde_json::json!({"a": [1, {"b": null}]})).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "{\n  \"a\": [\n    1,\n    {\n      \"b\": null\n    }\n  ]\n}\n"
        );
    }

    #[test]
    fn json_does_not_escape_html_but_escapes_line_separators() {
        let mut out = Vec::new();
        json(
            &mut out,
            &serde_json::json!({"s": "<a & b>\u{2028}x\u{2029}"}),
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "{\n  \"s\": \"<a & b>\\u2028x\\u2029\"\n}\n"
        );
    }

    #[test]
    fn envelope_omits_empty_submitted_reviews() {
        let env = Envelope {
            workspace: "/ws".into(),
            count: 0,
            reviews: Vec::new(),
            submitted_reviews: Vec::new(),
        };
        let mut out = Vec::new();
        json(&mut out, &env).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "{\n  \"workspace\": \"/ws\",\n  \"count\": 0,\n  \"reviews\": []\n}\n"
        );
    }

    #[test]
    fn envelope_names_comments_reviews_and_keeps_submitted_order() {
        let c = comment("r1", Status::Pending, "a.go", 1, 1, "x", "n");
        let env = Envelope {
            workspace: "/ws".into(),
            count: 1,
            reviews: vec![c],
            submitted_reviews: vec![handoff(review("rv1", "approve", "ok", &[]), vec![])],
        };
        let mut out = Vec::new();
        json(&mut out, &env).unwrap();
        let text = String::from_utf8(out).unwrap();
        let ws = text.find("\"workspace\"").unwrap();
        let count = text.find("\"count\"").unwrap();
        let reviews = text.find("\"reviews\"").unwrap();
        let submitted = text.find("\"submitted_reviews\"").unwrap();
        assert!(ws < count && count < reviews && reviews < submitted);
        assert!(text.contains("\"id\": \"rv1\""));
    }

    #[test]
    fn first_line_trims_blank_lines_and_stops_at_the_first_newline() {
        assert_eq!(first_line("\n\n  hello \nworld\n"), "hello ");
        assert_eq!(first_line("\n\n  hello\nworld\n"), "hello");
        assert_eq!(first_line("one"), "one");
        assert_eq!(first_line(""), "");
        assert_eq!(first_line("   \n  "), "");
    }

    #[test]
    fn attribution_covers_each_combination() {
        assert_eq!(attribution(None, None), "");
        assert_eq!(attribution(Some(""), Some("")), "");
        assert_eq!(attribution(Some("ann"), None), "@ann");
        assert_eq!(attribution(None, Some("api")), "#api");
        assert_eq!(attribution(Some("ann"), Some("api")), "@ann #api");
        assert_eq!(attribution(Some(""), Some("api")), "#api");
    }

    #[test]
    fn comment_line_pads_the_id_and_shows_only_the_first_line() {
        let mut c = comment("r1", Status::Pending, "a.go", 2, 4, "x", "first\nsecond");
        assert_eq!(comment_line(&c, 3), "r1   a.go:2-4  first");
        c.author = Some("ann".into());
        c.lane = Some("api".into());
        assert_eq!(comment_line(&c, 4), "r1    a.go:2-4  @ann #api  first");
    }

    #[test]
    fn review_and_sheet_lines_name_the_decision_and_count() {
        let mut r = review("rv1", "request-changes", "Fix\nmore", &["r1", "r2"]);
        assert_eq!(
            review_line(&r),
            "rv1  request-changes  2 review comment(s)  Fix"
        );
        r.author = Some("bob".into());
        r.lane = Some("api".into());
        let rs = sheet_of(rvw_core::operations::SheetState::Pending, r);
        assert_eq!(
            sheet_line(&rs),
            "rv1  [pending]  request-changes  2 review comment(s)  @bob #api  Fix"
        );
    }

    #[test]
    fn text_writes_reviews_then_comments_padded_to_the_widest_id() {
        let mut r = review("rv1", "approve", "Looks good\nsecond", &["r1"]);
        r.author = None;
        let reviews = vec![handoff(r, vec![])];
        let mut c1 = comment("r1", Status::Pending, "a.go", 1, 1, "x", "hi\nthere");
        c1.author = Some("ann".into());
        c1.lane = Some("api".into());
        let c2 = comment("r10", Status::Pending, "b.go", 2, 3, "y", "bad");
        let mut out = Vec::new();
        let wrote = text(&mut out, &reviews, &[c1, c2]).unwrap();
        assert!(wrote);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "rv1  approve  1 review comment(s)  Looks good\n\
             r1   a.go:1  @ann #api  hi\n\
             r10  b.go:2-3  bad\n"
        );
    }

    #[test]
    fn text_reports_nothing_written_for_empty_input() {
        let mut out = Vec::new();
        assert!(!text(&mut out, &[], &[]).unwrap());
        assert_eq!(out, Vec::<u8>::new());
    }

    #[test]
    fn ids_lists_reviews_first() {
        let c = comment("r1", Status::Pending, "a.go", 1, 1, "x", "n");
        let reviews = vec![handoff(review("rv2", "comment", "", &[]), vec![])];
        let mut out = Vec::new();
        ids(&mut out, &reviews, &[c]).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "rv2\nr1\n");
    }

    #[test]
    fn markdown_with_nothing_names_the_workspace() {
        assert_eq!(
            markdown("/ws", &[], &[], false),
            "No pending review comments for /ws.\n"
        );
    }

    #[test]
    fn markdown_quotes_standalone_comments_with_code_and_attribution() {
        let mut c = comment(
            "r1",
            Status::Pending,
            "a.go",
            2,
            3,
            "x := 1\ny := 2",
            "first\n\nsecond",
        );
        c.filetype = "go".into();
        c.author = Some("ann".into());
        c.lane = Some("api".into());
        let got = markdown("/ws", &[], &[c], false);
        assert_eq!(
            got,
            "# Review comments — /ws\n\
             \n\
             Address the following review comments, in the order listed. Each names a file\n\
             and line range, the code as it stood when the comment was written, and the\n\
             reviewer's note as a quote.\n\
             \n\
             ## 1. `a.go:2-3` (r1 · @ann #api)\n\
             \n\
             ```go\n\
             x := 1\n\
             y := 2\n\
             ```\n\
             \n\
             > first\n\
             >\n\
             > second\n"
        );
    }

    #[test]
    fn markdown_review_has_a_decision_label_and_the_drained_notice() {
        let c = comment("r1", Status::Pulled, "b.go", 5, 5, "return nil", "fix this");
        let h = handoff(
            review("rv1", "request-changes", "Needs work", &["r1"]),
            vec![c],
        );
        let got = markdown("/ws", &[h], &[], true);
        assert_eq!(
            got,
            "# Review rv1 — /ws\n\
             \n\
             **Decision:** Request changes\n\
             \n\
             Needs work\n\
             \n\
             ## Review comments\n\
             \n\
             ### 1. `b.go:5` (r1)\n\
             \n\
             ```\n\
             return nil\n\
             ```\n\
             \n\
             > fix this\n\
             \n\
             These comments have been dequeued — they will not appear again. Record what\n\
             became of each one with `rvw resolve <id> --note ...`, or\n\
             `rvw reject <id> --note ...` when you are not making the change.\n"
        );
    }

    #[test]
    fn markdown_numbers_review_comments_and_standalone_comments_separately() {
        let rc = comment("r2", Status::Pulled, "c.go", 1, 1, "a", "in review");
        let h = handoff(review("rv3", "comment", "S", &["r2"]), vec![rc]);
        let standalone = comment("r9", Status::Pending, "d.go", 7, 7, "b", "loose");
        let got = markdown("/ws", &[h], &[standalone], false);
        assert!(got.contains("### 1. `c.go:1` (r2)\n"));
        assert!(got.contains("## 1. `d.go:7` (r9)\n"));
        assert!(got.contains("**Decision:** Comment\n"));
        assert!(got.ends_with("> loose\n"));
    }

    #[test]
    fn decision_label_capitalises_and_spaces_out_hyphens() {
        assert_eq!(decision_label(""), "");
        assert_eq!(decision_label("approve"), "Approve");
        assert_eq!(decision_label("request-changes"), "Request changes");
    }
}
