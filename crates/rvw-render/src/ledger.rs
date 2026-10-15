//! The audit ledger as markdown, HTML and JSON. Text from comments is made
//! safe for a terminal first; HTML also escapes every dynamic string.

use rvw_core::operations::{LedgerCommit, LedgerOutput, LinkKind, LinkedComment, UnlinkedComment};
use rvw_core::types::{Comment, GateOverride, Verdict};

use crate::render::json;
use crate::terminal::safe;

/// The ledger as markdown: a summary, each commit with its linked comments,
/// then the unlinked comments and the gate overrides.
#[must_use]
pub fn ledger_markdown(l: &LedgerOutput) -> String {
    let mut out: Vec<String> = vec![
        "# Audit ledger".to_owned(),
        String::new(),
        format!("**Workspace:** {}", one_line(&l.workspace)),
        format!("**Range:** {}", one_line(&l.range)),
        String::new(),
        summary(l),
        String::new(),
    ];
    if l.commits.is_empty() {
        out.extend(["No commits in range.".to_owned(), String::new()]);
    }
    for c in &l.commits {
        commit_markdown(&mut out, l, c);
    }
    out.extend(["## Unlinked comments".to_owned(), String::new()]);
    if l.unlinked.is_empty() {
        out.extend(["None.".to_owned(), String::new()]);
    }
    for u in &l.unlinked {
        out.push(unlinked_markdown(u));
    }
    if !l.unlinked.is_empty() {
        out.push(String::new());
    }
    out.extend(["## Gate overrides".to_owned(), String::new()]);
    if l.overrides.is_empty() {
        out.extend(["None.".to_owned(), String::new()]);
    }
    for o in &l.overrides {
        override_markdown(&mut out, o);
    }
    format!("{}\n", out.join("\n").trim_end())
}

/// The ledger as one self-contained HTML page with a print stylesheet.
#[must_use]
pub fn ledger_html(l: &LedgerOutput) -> String {
    let mut h: Vec<String> = vec![
        "<!doctype html>".to_owned(),
        "<html lang=\"en\">".to_owned(),
        "<head>".to_owned(),
        "<meta charset=\"utf-8\">".to_owned(),
        "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">".to_owned(),
        format!("<title>Audit ledger — {}</title>", esc(&l.workspace)),
        format!("<style>{STYLE}</style>"),
        "</head>".to_owned(),
        "<body>".to_owned(),
        "<main>".to_owned(),
        "<h1>Audit ledger</h1>".to_owned(),
        format!(
            "<p><strong>Workspace:</strong> {}<br>\n<strong>Range:</strong> {}</p>",
            esc(&l.workspace),
            esc(&l.range)
        ),
        format!("<p class=\"summary\">{}</p>", esc(&summary(l))),
    ];
    if l.commits.is_empty() {
        h.push("<p>No commits in range.</p>".to_owned());
    }
    for c in &l.commits {
        commit_html(&mut h, l, c);
    }
    h.push("<section class=\"unlinked\">".to_owned());
    h.push("<h2>Unlinked comments</h2>".to_owned());
    if l.unlinked.is_empty() {
        h.push("<p>None.</p>".to_owned());
    } else {
        h.push("<ul>".to_owned());
        for u in &l.unlinked {
            let c = &u.comment;
            h.push(format!(
                "<li><code>{}</code> <code>{}</code> ({}): {}</li>",
                esc(&c.id),
                esc(&c.location()),
                c.status.as_str(),
                esc(&u.reason)
            ));
        }
        h.push("</ul>".to_owned());
    }
    h.push("</section>".to_owned());
    h.push("<section class=\"overrides\">".to_owned());
    h.push("<h2>Gate overrides</h2>".to_owned());
    if l.overrides.is_empty() {
        h.push("<p>None.</p>".to_owned());
    }
    for o in &l.overrides {
        override_html(&mut h, o);
    }
    h.push("</section>".to_owned());
    h.extend(["</main>", "</body>", "</html>"].map(str::to_owned));
    format!("{}\n", h.join("\n"))
}

/// The ledger as pretty JSON, followed by a newline.
///
/// # Panics
///
/// Never for a ledger: every field is a string, a number, a boolean, an
/// option or a sequence of those, which always serialize.
#[must_use]
pub fn ledger_json(l: &LedgerOutput) -> String {
    let mut buf = Vec::new();
    // Writing to a Vec cannot fail, and every field of the ledger serializes.
    json(&mut buf, l).expect("a ledger serializes to JSON");
    String::from_utf8(buf).expect("JSON output is UTF-8")
}

const STYLE: &str = "\
body { font-family: system-ui, sans-serif; line-height: 1.5; max-width: 56rem; margin: 2rem auto; padding: 0 1rem; }
pre { border: 1px solid; overflow-x: auto; padding: 0.5rem; }
blockquote { border-left: 3px solid; margin-left: 0; padding-left: 1rem; white-space: pre-wrap; }
@media print {
  body { max-width: none; margin: 0; }
  a { text-decoration: none; }
  .commit, .comment, .override { page-break-inside: avoid; }
}";

fn summary(l: &LedgerOutput) -> String {
    let linked: usize = l.commits.iter().map(|c| c.comments.len()).sum();
    format!(
        "{} commit(s), {linked} linked comment(s), {} unlinked, {} override(s).",
        l.commits.len(),
        l.unlinked.len(),
        l.overrides.len()
    )
}

/// `text` made safe for a terminal, with its lines joined: for a markdown
/// bullet or heading that must stay on one line.
fn one_line(text: &str) -> String {
    safe(text).lines().collect::<Vec<_>>().join(" ")
}

/// `text` in a markdown code span, longer backtick fences when it has backticks.
fn code_span(text: &str) -> String {
    let text = one_line(text);
    if text.contains('`') {
        format!("`` {text} ``")
    } else {
        format!("`{text}`")
    }
}

/// The lines of a fenced code block; the fence is longer than any backtick
/// run in `body`, so the body cannot close it.
fn fenced(body: &str, lang: &str) -> Vec<String> {
    let safe_body = safe(body);
    let longest = safe_body
        .split(|ch| ch != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(longest.max(2) + 1);
    let mut lines = vec![format!("{fence}{}", one_line(lang))];
    lines.extend(safe_body.split('\n').map(str::to_owned));
    lines.push(fence);
    lines
}

/// `text` as a markdown block quote, one `>` line per line of text.
fn quote(text: &str) -> Vec<String> {
    safe(text)
        .split('\n')
        .map(|line| {
            if line.is_empty() {
                ">".to_owned()
            } else {
                format!("> {line}")
            }
        })
        .collect()
}

/// `r1`, the first seven characters of a commit id.
fn short(id: &str) -> String {
    one_line(&id.chars().take(7).collect::<String>())
}

/// `claude-code haiku (session s-1)`, or the honest alternative for a commit
/// that was not reported by an agent.
fn provenance(l: &LedgerOutput, c: &LedgerCommit) -> String {
    match &c.session_id {
        Some(id) => match l.sessions.iter().find(|s| &s.session_id == id) {
            Some(s) => format!(
                "reported by {} {} (session {})",
                one_line(&s.tool),
                one_line(&s.model),
                one_line(id)
            ),
            None => format!(
                "reported session {} (not in the ledger's sessions)",
                one_line(id)
            ),
        },
        None if c.marked => "session marker, not reported".to_owned(),
        None => "human".to_owned(),
    }
}

fn commit_markdown(out: &mut Vec<String>, l: &LedgerOutput, c: &LedgerCommit) {
    out.push(format!("## {} {}", short(&c.id), one_line(&c.subject)));
    out.push(String::new());
    out.push(format!("- **Author:** {}", one_line(&c.author)));
    out.push(format!("- **Time:** {}", one_line(&c.time)));
    out.push(format!("- **Provenance:** {}", provenance(l, c)));
    out.push(String::new());
    if c.comments.is_empty() {
        out.extend(["No linked comments.".to_owned(), String::new()]);
    }
    for lc in &c.comments {
        linked_markdown(out, lc);
    }
}

fn linked_markdown(out: &mut Vec<String>, lc: &LinkedComment) {
    let c = &lc.comment;
    out.push(format!(
        "### {} {}",
        one_line(&c.id),
        code_span(&c.location())
    ));
    out.push(String::new());
    out.push(format!("- **Link:** {}", link_label(lc.link)));
    out.push(format!("- **Decision:** {}", decision(c)));
    if let Some(v) = &c.verdict {
        out.push(format!("- **Verdict:** {}", verdict_text(v)));
    }
    out.push(String::new());
    if !c.comment.is_empty() {
        out.extend(quote(&c.comment));
        out.push(String::new());
    }
    out.push("**Reviewed code:**".to_owned());
    out.push(String::new());
    out.extend(fenced(&c.code, &c.filetype));
    out.push(String::new());
    out.push("**Resolution diff:**".to_owned());
    out.push(String::new());
    if lc.diff.is_empty() {
        out.extend(["_No resolution diff recorded._".to_owned(), String::new()]);
    } else {
        out.extend(fenced(&lc.diff.join("\n"), "diff"));
        out.push(String::new());
    }
}

fn unlinked_markdown(u: &UnlinkedComment) -> String {
    let c = &u.comment;
    format!(
        "- {} {} ({}): {}",
        one_line(&c.id),
        code_span(&c.location()),
        c.status.as_str(),
        one_line(&u.reason)
    )
}

fn override_markdown(out: &mut Vec<String>, o: &GateOverride) {
    out.push(format!(
        "### {} by {} ({}) at {}",
        one_line(&o.action),
        one_line(&o.by),
        o.actor.as_str(),
        one_line(&o.at)
    ));
    out.push(String::new());
    if let Some(lane) = &o.lane {
        out.push(format!("- **Lane:** {}", one_line(lane)));
    }
    out.push(format!("- **Reason:** {}", one_line(&o.reason)));
    out.push(format!("- **Open:** {}", ids_text(&o.open)));
    out.push(format!("- **Unjudged:** {}", ids_text(&o.unjudged)));
    out.push(String::new());
}

fn commit_html(h: &mut Vec<String>, l: &LedgerOutput, c: &LedgerCommit) {
    h.push("<section class=\"commit\">".to_owned());
    h.push(format!(
        "<h2><code>{}</code> {}</h2>",
        esc(&short(&c.id)),
        esc(&c.subject)
    ));
    h.push("<ul>".to_owned());
    h.push(format!(
        "<li><strong>Author:</strong> {}</li>",
        esc(&c.author)
    ));
    h.push(format!("<li><strong>Time:</strong> {}</li>", esc(&c.time)));
    h.push(format!(
        "<li><strong>Provenance:</strong> {}</li>",
        esc(&provenance(l, c))
    ));
    h.push("</ul>".to_owned());
    if c.comments.is_empty() {
        h.push("<p>No linked comments.</p>".to_owned());
    }
    for lc in &c.comments {
        linked_html(h, lc);
    }
    h.push("</section>".to_owned());
}

fn linked_html(h: &mut Vec<String>, lc: &LinkedComment) {
    let c = &lc.comment;
    h.push("<article class=\"comment\">".to_owned());
    h.push(format!(
        "<h3>{} <code>{}</code></h3>",
        esc(&c.id),
        esc(&c.location())
    ));
    h.push("<ul>".to_owned());
    h.push(format!(
        "<li><strong>Link:</strong> {}</li>",
        link_label(lc.link)
    ));
    h.push(format!(
        "<li><strong>Decision:</strong> {}</li>",
        esc(&decision(c))
    ));
    if let Some(v) = &c.verdict {
        h.push(format!(
            "<li><strong>Verdict:</strong> {}</li>",
            esc(&verdict_text(v))
        ));
    }
    h.push("</ul>".to_owned());
    if !c.comment.is_empty() {
        h.push(format!("<blockquote>{}</blockquote>", esc(&c.comment)));
    }
    h.push("<p><strong>Reviewed code:</strong></p>".to_owned());
    h.push(format!("<pre><code>{}</code></pre>", esc(&c.code)));
    h.push("<p><strong>Resolution diff:</strong></p>".to_owned());
    if lc.diff.is_empty() {
        h.push("<p><em>No resolution diff recorded.</em></p>".to_owned());
    } else {
        h.push(format!(
            "<pre class=\"diff\"><code>{}</code></pre>",
            esc(&lc.diff.join("\n"))
        ));
    }
    h.push("</article>".to_owned());
}

fn override_html(h: &mut Vec<String>, o: &GateOverride) {
    h.push("<article class=\"override\">".to_owned());
    h.push(format!(
        "<h3>{} by {} ({}) at {}</h3>",
        esc(&o.action),
        esc(&o.by),
        o.actor.as_str(),
        esc(&o.at)
    ));
    h.push("<ul>".to_owned());
    if let Some(lane) = &o.lane {
        h.push(format!("<li><strong>Lane:</strong> {}</li>", esc(lane)));
    }
    h.push(format!(
        "<li><strong>Reason:</strong> {}</li>",
        esc(&o.reason)
    ));
    h.push(format!(
        "<li><strong>Open:</strong> {}</li>",
        esc(&ids_text(&o.open))
    ));
    h.push(format!(
        "<li><strong>Unjudged:</strong> {}</li>",
        esc(&ids_text(&o.unjudged))
    ));
    h.push("</ul>".to_owned());
    h.push("</article>".to_owned());
}

fn link_label(link: LinkKind) -> &'static str {
    match link {
        LinkKind::Exact => "exact",
        LinkKind::SameChange => "same-change",
    }
}

/// `done, resolved by gustavo: the note`.
fn decision(c: &Comment) -> String {
    let mut s = c.status.as_str().to_owned();
    if let Some(by) = &c.resolved_by {
        s = format!("{s}, resolved by {}", one_line(by));
    }
    if let Some(note) = &c.resolution_note {
        s = format!("{s}: {}", one_line(note));
    }
    s
}

/// `accepted by alice (human): the note`.
fn verdict_text(v: &Verdict) -> String {
    let mut s = format!(
        "{} by {} ({})",
        v.kind.as_str(),
        one_line(&v.by),
        v.actor.as_str()
    );
    if let Some(note) = &v.note {
        s = format!("{s}: {}", one_line(note));
    }
    s
}

fn ids_text(ids: &[String]) -> String {
    if ids.is_empty() {
        "none".to_owned()
    } else {
        ids.iter()
            .map(|id| one_line(id))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Escapes the five characters that matter in HTML text and attributes, after
/// making the text safe for a terminal.
fn esc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in safe(text).chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use rvw_core::operations::{
        LedgerCommit, LedgerOutput, LinkKind, LinkedComment, UnlinkedComment,
    };
    use rvw_core::types::{Actor, GateOverride, Session, Status, VerdictKind};

    use super::*;
    use crate::fixtures::{comment, verdict};

    const AGENT: &str = "aaaaaaa1234567890abcdef1234567890abcdef";
    const HUMAN: &str = "bbbbbbb1234567890abcdef1234567890abcdef";

    fn fixture() -> LedgerOutput {
        let mut r1 = comment(
            "r1",
            Status::Done,
            "parser.go",
            4,
            5,
            "if s == \"\" {\n    return nil\n}",
            "Empty input must not panic.",
        );
        r1.filetype = "go".into();
        r1.resolved_by = Some("gustavo".into());
        r1.resolution_note = Some("Return nil for empty input".into());
        r1.verdict = Some(verdict(VerdictKind::Accepted, Some("Looks right"), None));
        let linked = LinkedComment {
            comment: r1,
            link: LinkKind::Exact,
            diff: vec![
                "@@ -3,1 +3,4 @@".into(),
                " func parse(s string) {".into(),
                "+    if s == \"\" {".into(),
                "+        return nil".into(),
                "+    }".into(),
            ],
        };
        let mut r2 = comment("r2", Status::Done, "README.md", 1, 1, "# Title", "Reword");
        r2.filetype = "markdown".into();
        r2.resolved_by = Some("alice".into());
        LedgerOutput {
            workspace: "/ws".into(),
            range: "main..HEAD".into(),
            commits: vec![
                LedgerCommit {
                    id: AGENT.into(),
                    subject: "Fix the parser".into(),
                    author: "gustavo".into(),
                    time: "2026-01-02T10:00:00Z".into(),
                    session_id: Some("s-1".into()),
                    marked: false,
                    comments: vec![linked],
                },
                LedgerCommit {
                    id: HUMAN.into(),
                    subject: "Tidy the docs".into(),
                    author: "alice".into(),
                    time: "2026-01-03T10:00:00Z".into(),
                    session_id: None,
                    marked: false,
                    comments: vec![],
                },
            ],
            unlinked: vec![UnlinkedComment {
                comment: r2,
                reason: "no commit in range touches the file".into(),
            }],
            overrides: vec![GateOverride {
                workspace: "/ws".into(),
                lane: None,
                action: "commit".into(),
                reason: "hotfix".into(),
                by: "alice".into(),
                actor: Actor::Human,
                at: "2026-01-03T12:00:00Z".into(),
                open: vec!["r7".into()],
                unjudged: vec!["r1".into()],
            }],
            sessions: vec![Session {
                session_id: "s-1".into(),
                tool: "claude-code".into(),
                model: "haiku".into(),
                summary: Some("Fix the parser".into()),
                commits: vec![AGENT.into()],
                reported_at: "2026-01-02T10:05:00Z".into(),
            }],
        }
    }

    fn empty() -> LedgerOutput {
        LedgerOutput {
            workspace: "/ws".into(),
            range: "main..HEAD".into(),
            commits: vec![],
            unlinked: vec![],
            overrides: vec![],
            sessions: vec![],
        }
    }

    #[test]
    fn markdown_matches_the_golden_ledger() {
        let want = "\
# Audit ledger

**Workspace:** /ws
**Range:** main..HEAD

2 commit(s), 1 linked comment(s), 1 unlinked, 1 override(s).

## aaaaaaa Fix the parser

- **Author:** gustavo
- **Time:** 2026-01-02T10:00:00Z
- **Provenance:** reported by claude-code haiku (session s-1)

### r1 `parser.go:4-5`

- **Link:** exact
- **Decision:** done, resolved by gustavo: Return nil for empty input
- **Verdict:** accepted by alice (human): Looks right

> Empty input must not panic.

**Reviewed code:**

```go
if s == \"\" {
    return nil
}
```

**Resolution diff:**

```diff
@@ -3,1 +3,4 @@
 func parse(s string) {
+    if s == \"\" {
+        return nil
+    }
```

## bbbbbbb Tidy the docs

- **Author:** alice
- **Time:** 2026-01-03T10:00:00Z
- **Provenance:** human

No linked comments.

## Unlinked comments

- r2 `README.md:1` (done): no commit in range touches the file

## Gate overrides

### commit by alice (human) at 2026-01-03T12:00:00Z

- **Reason:** hotfix
- **Open:** r7
- **Unjudged:** r1
";
        assert_eq!(ledger_markdown(&fixture()), want);
    }

    #[test]
    fn markdown_of_an_empty_ledger_says_so() {
        let want = "\
# Audit ledger

**Workspace:** /ws
**Range:** main..HEAD

0 commit(s), 0 linked comment(s), 0 unlinked, 0 override(s).

No commits in range.

## Unlinked comments

None.

## Gate overrides

None.
";
        assert_eq!(ledger_markdown(&empty()), want);
    }

    #[test]
    fn provenance_marks_a_session_marker_that_was_never_reported() {
        let mut l = fixture();
        l.commits[1].marked = true;
        assert!(ledger_markdown(&l).contains("- **Provenance:** session marker, not reported\n"));
    }

    #[test]
    fn control_characters_in_comment_text_are_shown_not_acted_on() {
        let mut l = fixture();
        l.commits[0].comments[0].comment.comment = "bell\u{7}here".into();
        let out = ledger_markdown(&l);
        assert!(out.contains("> bell\u{fffd}here\n"));
        assert!(!out.contains('\u{7}'));
    }

    #[test]
    fn html_escapes_comment_text_and_is_one_self_contained_page() {
        let mut l = fixture();
        l.commits[0].comments[0].comment.comment = "<script>alert(1)</script> & more".into();
        l.commits[0].comments[0].comment.code = "a < b && c".into();
        l.overrides[0].reason = "it's \"fine\"".into();
        let out = ledger_html(&l);
        assert!(out.starts_with("<!doctype html>\n"));
        assert!(out.contains("<meta charset=\"utf-8\">"));
        assert!(out.contains("<meta name=\"viewport\""));
        assert!(out.contains("@media print"));
        assert!(out.contains("page-break-inside: avoid"));
        assert!(out.contains("&lt;script&gt;alert(1)&lt;/script&gt; &amp; more"));
        assert!(out.contains("a &lt; b &amp;&amp; c"));
        assert!(out.contains("it&#39;s &quot;fine&quot;"));
        assert!(!out.contains("<script"));
        assert!(!out.contains("src="));
        assert!(!out.contains("href="));
    }

    #[test]
    fn html_of_an_empty_ledger_has_no_commits() {
        let out = ledger_html(&empty());
        assert!(out.contains("<p>No commits in range.</p>"));
        assert!(out.ends_with("</html>\n"));
    }

    #[test]
    fn json_is_the_ledger_object_with_a_trailing_newline() {
        let out = ledger_json(&fixture());
        assert!(out.ends_with("}\n"));
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["range"], "main..HEAD");
        assert_eq!(v["commits"][0]["comments"][0]["link"], "exact");
        assert_eq!(v["commits"][1]["session_id"], serde_json::Value::Null);
    }
}
