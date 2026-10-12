//! A unified diff as display lines: hunks, context, additions and removals.

/// What a line of a hunk body is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Added,
    Removed,
}

/// One line of a hunk body, with its numbers in the old and new file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub kind: LineKind,
    /// Line number in the old file; `None` for an added line.
    pub old_no: Option<u32>,
    /// Line number in the new file; `None` for a removed line.
    pub new_no: Option<u32>,
    /// The line without its leading ' ', '+' or '-'.
    pub text: String,
}

/// A hunk: its whole `@@ -a,b +c,d @@` header line and its body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub header: String,
    pub lines: Vec<Line>,
}

/// A parsed unified diff of one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    pub hunks: Vec<Hunk>,
    /// The text said "Binary files ... differ"; there are no hunks then.
    pub binary: bool,
    /// Number of added lines.
    pub added: usize,
    /// Number of removed lines.
    pub removed: usize,
}

/// Parses the text of a unified diff of one file. Header lines (`diff`,
/// `index`, `---`, `+++`, mode lines) are skipped. A hunk body is read by its
/// header counts, so a body line whose text looks like a header stays text.
pub fn parse(diff: &str) -> FileDiff {
    let mut file = FileDiff {
        hunks: Vec::new(),
        binary: false,
        added: 0,
        removed: 0,
    };
    // Lines of the current hunk still expected in the old and new file.
    let (mut old_left, mut new_left) = (0u32, 0u32);
    let (mut old_no, mut new_no) = (0u32, 0u32);
    // Split on '\n' only, so a '\r' stays part of the text.
    let diff = diff.strip_suffix('\n').unwrap_or(diff);
    for raw in diff.split('\n') {
        if old_left > 0 || new_left > 0 {
            if raw.starts_with('\\') {
                continue;
            }
            if let Some((kind, text)) = body_line(raw) {
                let (old, new) = match kind {
                    LineKind::Context => {
                        old_left = old_left.saturating_sub(1);
                        new_left = new_left.saturating_sub(1);
                        (Some(old_no), Some(new_no))
                    }
                    LineKind::Removed => {
                        old_left = old_left.saturating_sub(1);
                        file.removed += 1;
                        (Some(old_no), None)
                    }
                    LineKind::Added => {
                        new_left = new_left.saturating_sub(1);
                        file.added += 1;
                        (None, Some(new_no))
                    }
                };
                if kind != LineKind::Added {
                    old_no += 1;
                }
                if kind != LineKind::Removed {
                    new_no += 1;
                }
                // A body line only follows a header, so a hunk is open.
                if let Some(hunk) = file.hunks.last_mut() {
                    hunk.lines.push(Line {
                        kind,
                        old_no: old,
                        new_no: new,
                        text: text.to_owned(),
                    });
                }
                continue;
            }
            // Not a body line: the hunk was cut short, so read this as a header.
            old_left = 0;
            new_left = 0;
        }
        if let Some(counts) = raw.strip_prefix("@@ -").and_then(hunk_counts) {
            (old_left, new_left) = (counts.old_lines, counts.new_lines);
            (old_no, new_no) = (counts.old_start, counts.new_start);
            file.hunks.push(Hunk {
                header: raw.to_owned(),
                lines: Vec::new(),
            });
        } else if raw.starts_with("Binary files ") {
            file.binary = true;
        }
    }
    file
}

/// Splits one hunk body line into its kind and text; `None` when the line
/// does not start with ' ', '+' or '-' (an empty line is an empty context).
fn body_line(raw: &str) -> Option<(LineKind, &str)> {
    match raw.as_bytes().first() {
        None => Some((LineKind::Context, "")),
        Some(b' ') => Some((LineKind::Context, &raw[1..])),
        Some(b'+') => Some((LineKind::Added, &raw[1..])),
        Some(b'-') => Some((LineKind::Removed, &raw[1..])),
        Some(_) => None,
    }
}

/// The start and count of each side of a hunk header, read from the text
/// after "@@ -": "a,b +c,d @@ ...". A count left out is 1.
struct HunkCounts {
    old_start: u32,
    old_lines: u32,
    new_start: u32,
    new_lines: u32,
}

fn hunk_counts(rest: &str) -> Option<HunkCounts> {
    let (old, rest) = rest.split_once(" +")?;
    let (new, _) = rest.split_once(" @@")?;
    let (old_start, old_lines) = span(old)?;
    let (new_start, new_lines) = span(new)?;
    Some(HunkCounts {
        old_start,
        old_lines,
        new_start,
        new_lines,
    })
}

/// Parses "start" or "start,count".
fn span(s: &str) -> Option<(u32, u32)> {
    match s.split_once(',') {
        Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
        None => Some((s.parse().ok()?, 1)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(kind: LineKind, old_no: Option<u32>, new_no: Option<u32>, text: &str) -> Line {
        Line {
            kind,
            old_no,
            new_no,
            text: text.to_owned(),
        }
    }

    #[test]
    fn two_hunks_have_exact_line_numbers() {
        let diff = "\
diff --git a/f.txt b/f.txt
index 1111111..2222222 100644
--- a/f.txt
+++ b/f.txt
@@ -2,3 +2,3 @@ fn one()
 two
-three
+THREE
 four
@@ -10,2 +10,3 @@
 ten
+eleven a
 twelve
";
        let parsed = parse(diff);
        assert_eq!(parsed.added, 2);
        assert_eq!(parsed.removed, 1);
        assert!(!parsed.binary);
        assert_eq!(parsed.hunks.len(), 2);

        let first = &parsed.hunks[0];
        assert_eq!(first.header, "@@ -2,3 +2,3 @@ fn one()");
        assert_eq!(
            first.lines,
            vec![
                line(LineKind::Context, Some(2), Some(2), "two"),
                line(LineKind::Removed, Some(3), None, "three"),
                line(LineKind::Added, None, Some(3), "THREE"),
                line(LineKind::Context, Some(4), Some(4), "four"),
            ]
        );

        let second = &parsed.hunks[1];
        assert_eq!(second.header, "@@ -10,2 +10,3 @@");
        assert_eq!(
            second.lines,
            vec![
                line(LineKind::Context, Some(10), Some(10), "ten"),
                line(LineKind::Added, None, Some(11), "eleven a"),
                line(LineKind::Context, Some(11), Some(12), "twelve"),
            ]
        );
    }

    #[test]
    fn added_only_file() {
        let diff = "\
--- /dev/null
+++ b/new.txt
@@ -0,0 +1,3 @@
+a
+b
+c
";
        let parsed = parse(diff);
        assert_eq!(parsed.added, 3);
        assert_eq!(parsed.removed, 0);
        assert_eq!(parsed.hunks.len(), 1);
        assert_eq!(parsed.hunks[0].header, "@@ -0,0 +1,3 @@");
        assert_eq!(
            parsed.hunks[0].lines,
            vec![
                line(LineKind::Added, None, Some(1), "a"),
                line(LineKind::Added, None, Some(2), "b"),
                line(LineKind::Added, None, Some(3), "c"),
            ]
        );
    }

    #[test]
    fn removed_only_file() {
        let diff = "\
--- a/gone.txt
+++ /dev/null
@@ -1,2 +0,0 @@
-x
-y
";
        let parsed = parse(diff);
        assert_eq!(parsed.added, 0);
        assert_eq!(parsed.removed, 2);
        assert_eq!(
            parsed.hunks[0].lines,
            vec![
                line(LineKind::Removed, Some(1), None, "x"),
                line(LineKind::Removed, Some(2), None, "y"),
            ]
        );
    }

    #[test]
    fn no_newline_marker_is_skipped() {
        let diff = "\
@@ -1 +1 @@
-old
\\ No newline at end of file
+new
\\ No newline at end of file
";
        let parsed = parse(diff);
        assert_eq!(parsed.added, 1);
        assert_eq!(parsed.removed, 1);
        assert_eq!(
            parsed.hunks[0].lines,
            vec![
                line(LineKind::Removed, Some(1), None, "old"),
                line(LineKind::Added, None, Some(1), "new"),
            ]
        );
    }

    #[test]
    fn binary_sets_flag_and_has_no_hunks() {
        let diff = "\
diff --git a/img.png b/img.png
index 1111111..2222222 100644
Binary files a/img.png and b/img.png differ
";
        let parsed = parse(diff);
        assert!(parsed.binary);
        assert!(parsed.hunks.is_empty());
        assert_eq!(parsed.added, 0);
        assert_eq!(parsed.removed, 0);
    }

    #[test]
    fn empty_input_gives_empty_file_diff() {
        assert_eq!(
            parse(""),
            FileDiff {
                hunks: Vec::new(),
                binary: false,
                added: 0,
                removed: 0,
            }
        );
    }

    #[test]
    fn body_line_that_looks_like_a_header_stays_text() {
        // The context line " @@ -1 +1 @@" and the removed line "-@@ -5 +5 @@"
        // belong to the body: the counts say the hunk is not over yet.
        let diff = "\
@@ -1,3 +1,3 @@
 @@ -1 +1 @@
-@@ -5 +5 @@
+plain
 end
@@ -9 +9 @@
-nine
+NINE
";
        let parsed = parse(diff);
        assert_eq!(parsed.hunks.len(), 2);
        assert_eq!(
            parsed.hunks[0].lines,
            vec![
                line(LineKind::Context, Some(1), Some(1), "@@ -1 +1 @@"),
                line(LineKind::Removed, Some(2), None, "@@ -5 +5 @@"),
                line(LineKind::Added, None, Some(2), "plain"),
                line(LineKind::Context, Some(3), Some(3), "end"),
            ]
        );
        assert_eq!(parsed.hunks[1].header, "@@ -9 +9 @@");
        assert_eq!(parsed.added, 2);
        assert_eq!(parsed.removed, 2);
    }
}
