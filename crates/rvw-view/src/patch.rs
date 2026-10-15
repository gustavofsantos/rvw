//! Builds the patch that stages or unstages part of one file's diff.
//!
//! `git apply --cached` (stage) and `git apply --cached --reverse` (unstage)
//! read the patch made here. The patch keeps the file headers of the diff and
//! only the changed lines the user chose. An unchosen change becomes context
//! or is dropped, so the patch applies to the index without it. Hunk headers
//! are counted again afterwards. The module is pure: diff text in, patch text
//! out. Its integration tests run git to check that the patches apply.

use std::collections::BTreeSet;
use std::fmt::{self, Write as _};

/// Which way a patch moves changes between the worktree and the index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// From the unstaged diff, applied with `git apply --cached`.
    Stage,
    /// From the staged diff, applied with `git apply --cached --reverse`.
    Unstage,
}

/// Why no patch could be built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchError {
    /// The chosen lines contain no added or removed line.
    NothingChosen,
    /// The file is new, deleted, renamed, copied, binary or changes mode: it moves
    /// only as a whole file.
    WholeFileOnly,
}

impl fmt::Display for PatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NothingChosen => "no changed line is selected",
            Self::WholeFileOnly => "this file can only be staged as a whole",
        })
    }
}

/// A patch with only the chosen lines of `diff`.
///
/// `chosen` holds (hunk, line) pairs. Hunks count from 0 in the order of their
/// `@@` headers, and lines count from 0 within a hunk's body, leaving out
/// "\ No newline at end of file" lines. The pair `(h, l)` is therefore
/// `file.hunks[h].lines[l]` in `crate::diff::parse`.
///
/// # Errors
///
/// `WholeFileOnly` when the diff is of a file that moves only as a whole, and
/// `NothingChosen` when no hunk keeps a chosen added or removed line.
pub fn lines_patch(
    diff: &str,
    chosen: &BTreeSet<(usize, usize)>,
    direction: Direction,
) -> Result<String, PatchError> {
    let (head, hunks) = split(diff);
    if head.iter().any(|line| is_whole_file(line)) {
        return Err(PatchError::WholeFileOnly);
    }
    let mut kept = String::new();
    // Added lines minus removed lines in the kept hunks before this one.
    let mut shift = 0;
    for (index, hunk) in hunks.iter().enumerate() {
        let Some(body) = rewrite(hunk, index, chosen, direction) else {
            continue;
        };
        let (old_start, new_start) = starts(&hunk.header, &body, shift, direction);
        // Writing to a String cannot fail.
        let _ = writeln!(
            kept,
            "@@ -{old_start},{} +{new_start},{} @@{}",
            body.old_lines, body.new_lines, hunk.header.tail
        );
        kept.push_str(&body.text);
        shift += body.new_lines - body.old_lines;
    }
    if kept.is_empty() {
        return Err(PatchError::NothingChosen);
    }
    let mut patch = String::new();
    for line in head {
        patch.push_str(line);
        patch.push('\n');
    }
    patch.push_str(&kept);
    Ok(patch)
}

/// A patch with every line of hunk `hunk`.
///
/// # Errors
///
/// The same as `lines_patch`. A `hunk` that does not exist gives `NothingChosen`.
pub fn hunk_patch(diff: &str, hunk: usize, direction: Direction) -> Result<String, PatchError> {
    let (_, hunks) = split(diff);
    let chosen: BTreeSet<(usize, usize)> = hunks
        .get(hunk)
        .map(|hunk_text| {
            (0..hunk_text.lines.len())
                .map(|line| (hunk, line))
                .collect()
        })
        .unwrap_or_default();
    lines_patch(diff, &chosen, direction)
}

/// Header lines that show a change which moves only as a whole file.
const WHOLE_FILE_LINES: [&str; 7] = [
    "new file mode",
    "deleted file mode",
    "rename from",
    "copy from",
    "old mode",
    "Binary files",
    "GIT binary patch",
];

/// Whether a header line marks a whole-file change.
fn is_whole_file(line: &str) -> bool {
    WHOLE_FILE_LINES
        .iter()
        .any(|prefix| line.starts_with(*prefix))
}

/// What a line of a hunk body is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Context,
    Added,
    Removed,
}

/// One line of a hunk body with the marker line that follows it, if any.
struct BodyLine<'a> {
    kind: Kind,
    /// The whole line, with its leading ' ', '+' or '-'.
    raw: &'a str,
    /// A "\ No newline at end of file" line that follows this line.
    marker: Option<&'a str>,
}

/// The numbers and the text after `@@` of a hunk header.
struct HunkHeader<'a> {
    old_start: i64,
    old_count: i64,
    new_start: i64,
    new_count: i64,
    /// The text after the closing `@@`, such as " fn main", kept as it was.
    tail: &'a str,
}

/// One hunk: its header and its body lines.
struct Hunk<'a> {
    header: HunkHeader<'a>,
    lines: Vec<BodyLine<'a>>,
}

/// The rewritten body of a kept hunk, with the line counts its header needs.
struct Body {
    text: String,
    /// Context and removed lines: the old side.
    old_lines: i64,
    /// Context and added lines: the new side.
    new_lines: i64,
}

/// Splits a one-file diff into its header lines and its hunks. A hunk body
/// is read by its header counts, as `crate::diff::parse` reads it, so a line
/// that looks like a header stays body text while the counts say so.
fn split(diff: &str) -> (Vec<&str>, Vec<Hunk<'_>>) {
    // Split on '\n' only, so a '\r' stays part of the line.
    let diff = diff.strip_suffix('\n').unwrap_or(diff);
    let mut head = Vec::new();
    let mut hunks: Vec<Hunk<'_>> = Vec::new();
    // Lines of the current hunk still expected on the old and new side.
    let (mut old_left, mut new_left) = (0_i64, 0_i64);
    // Whether the previous line was a body line, so a marker belongs to it.
    let mut after_body = false;
    for raw in diff.split('\n') {
        if raw.starts_with('\\') {
            if after_body
                && let Some(line) = hunks.last_mut().and_then(|hunk| hunk.lines.last_mut())
            {
                line.marker = Some(raw);
            }
            continue;
        }
        if old_left > 0 || new_left > 0 {
            if let Some(kind) = body_kind(raw) {
                match kind {
                    Kind::Context => {
                        old_left = old_left.saturating_sub(1);
                        new_left = new_left.saturating_sub(1);
                    }
                    Kind::Removed => old_left = old_left.saturating_sub(1),
                    Kind::Added => new_left = new_left.saturating_sub(1),
                }
                if let Some(hunk) = hunks.last_mut() {
                    hunk.lines.push(BodyLine {
                        kind,
                        raw,
                        marker: None,
                    });
                }
                after_body = true;
                continue;
            }
            // The hunk was cut short, so this line is read as a header.
            old_left = 0;
            new_left = 0;
        }
        after_body = false;
        if let Some(header) = parse_header(raw) {
            old_left = header.old_count;
            new_left = header.new_count;
            hunks.push(Hunk {
                header,
                lines: Vec::new(),
            });
        } else if hunks.is_empty() {
            head.push(raw);
        }
    }
    (head, hunks)
}

/// The kind of a body line, or `None` when it does not start with ' ', '+'
/// or '-'. An empty line is an empty context line.
fn body_kind(raw: &str) -> Option<Kind> {
    match raw.as_bytes().first() {
        None | Some(b' ') => Some(Kind::Context),
        Some(b'+') => Some(Kind::Added),
        Some(b'-') => Some(Kind::Removed),
        Some(_) => None,
    }
}

/// Reads a `@@ -a,b +c,d @@tail` header line. A count left out is 1.
fn parse_header(line: &str) -> Option<HunkHeader<'_>> {
    let rest = line.strip_prefix("@@ -")?;
    let (old, rest) = rest.split_once(" +")?;
    let (new, tail) = rest.split_once(" @@")?;
    let (old_start, old_count) = span(old)?;
    let (new_start, new_count) = span(new)?;
    Some(HunkHeader {
        old_start,
        old_count,
        new_start,
        new_count,
        tail,
    })
}

/// Parses "start" or "start,count".
fn span(text: &str) -> Option<(i64, i64)> {
    match text.split_once(',') {
        Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
        None => Some((text.parse().ok()?, 1)),
    }
}

/// Rewrites the body of hunk `index` for the chosen lines. Returns `None`
/// when no chosen line is an added or removed line, so the hunk is left out.
fn rewrite(
    hunk: &Hunk<'_>,
    index: usize,
    chosen: &BTreeSet<(usize, usize)>,
    direction: Direction,
) -> Option<Body> {
    let picked = |line: usize| chosen.contains(&(index, line));
    let any_change = hunk
        .lines
        .iter()
        .enumerate()
        .any(|(line, body)| body.kind != Kind::Context && picked(line));
    if !any_change {
        return None;
    }
    let mut body = Body {
        text: String::new(),
        old_lines: 0,
        new_lines: 0,
    };
    for (number, line) in hunk.lines.iter().enumerate() {
        // An unchosen removal that ends the old side stays a removal when a
        // chosen addition follows it. The new side then needs the line with its
        // newline, so the addition is written back here unless the diff has
        // already kept it.
        let ends_old_side = direction == Direction::Stage
            && line.kind == Kind::Removed
            && line.marker.is_some()
            && !picked(number)
            && ((number + 1)..hunk.lines.len())
                .any(|later| hunk.lines[later].kind == Kind::Added && picked(later));
        if ends_old_side {
            push_line(&mut body, Kind::Removed, line.raw, line.marker);
            // git lists the newline version of the line right after it, with the same text.
            let newline_kept = hunk.lines.get(number + 1).is_some_and(|next| {
                next.kind == Kind::Added && picked(number + 1) && next.raw[1..] == line.raw[1..]
            });
            if !newline_kept {
                let added = format!("+{}", &line.raw[1..]);
                push_line(&mut body, Kind::Added, &added, None);
            }
            continue;
        }
        let kind = match (line.kind, picked(number), direction) {
            (Kind::Context, _, _) | (Kind::Added | Kind::Removed, true, _) => line.kind,
            (Kind::Removed, false, Direction::Stage) | (Kind::Added, false, Direction::Unstage) => {
                Kind::Context
            }
            (Kind::Added, false, Direction::Stage) | (Kind::Removed, false, Direction::Unstage) => {
                continue;
            }
        };
        if kind == Kind::Context && line.kind != Kind::Context {
            // A changed line that became context keeps its text after a space.
            let context = format!(" {}", &line.raw[1..]);
            push_line(&mut body, kind, &context, line.marker);
        } else {
            push_line(&mut body, kind, line.raw, line.marker);
        }
    }
    Some(body)
}

/// Appends one line of a body, its marker line after it, and counts it on
/// each side it is on.
fn push_line(body: &mut Body, kind: Kind, text: &str, marker: Option<&str>) {
    body.text.push_str(text);
    body.text.push('\n');
    if let Some(marker) = marker {
        body.text.push_str(marker);
        body.text.push('\n');
    }
    if kind != Kind::Added {
        body.old_lines += 1;
    }
    if kind != Kind::Removed {
        body.new_lines += 1;
    }
}

/// The start of each side of a kept hunk, written the way git writes it: the
/// line before the hunk, plus one when the side has lines. `shift` is the
/// added lines minus the removed lines of the kept hunks before this one.
fn starts(header: &HunkHeader<'_>, body: &Body, shift: i64, direction: Direction) -> (i64, i64) {
    // The lines before the hunk on each side, from the input header.
    let before_old = header.old_start - i64::from(header.old_count > 0);
    let before_new = header.new_start - i64::from(header.new_count > 0);
    match direction {
        // The index is the old side, so its positions stay and the new side moves.
        Direction::Stage => {
            let new_before = before_old + shift;
            (
                before_old + i64::from(body.old_lines > 0),
                new_before + i64::from(body.new_lines > 0),
            )
        }
        // The index is the new side, so its positions stay and the old side moves.
        Direction::Unstage => {
            let old_before = before_new - shift;
            (
                old_before + i64::from(body.old_lines > 0),
                before_new + i64::from(body.new_lines > 0),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::Path;
    use std::process::{Command, Stdio};

    /// The header lines of the one-file diffs below.
    const FILE_HEADER: &str =
        "diff --git a/f.txt b/f.txt\nindex 3b18e51..b1e6722 100644\n--- a/f.txt\n+++ b/f.txt\n";

    /// Turns pairs into the set of chosen lines.
    fn picked(pairs: &[(usize, usize)]) -> BTreeSet<(usize, usize)> {
        pairs.iter().copied().collect()
    }

    /// Every line of every hunk of `diff`.
    fn every_line(diff: &str) -> BTreeSet<(usize, usize)> {
        let mut all = BTreeSet::new();
        for (hunk, lines) in super::split(diff).1.iter().enumerate() {
            for line in 0..lines.lines.len() {
                all.insert((hunk, line));
            }
        }
        all
    }

    /// One hunk: `-b` and `+B` replace one line between two context lines.
    fn replace_hunk() -> String {
        [
            FILE_HEADER,
            "@@ -1,4 +1,4 @@ fn main\n",
            " a\n",
            "-b\n",
            "+B\n",
            " c\n",
            " d\n",
        ]
        .concat()
    }

    /// Two hunks: an added line near the top, then a replacement at line 10.
    fn two_hunks() -> String {
        [
            FILE_HEADER,
            "@@ -1,3 +1,4 @@\n",
            " a\n",
            "+NEW\n",
            " b\n",
            " c\n",
            "@@ -10,3 +11,3 @@ tail\n",
            " x\n",
            "-y\n",
            "+Y\n",
            " z\n",
        ]
        .concat()
    }

    #[test]
    fn stage_plus_only_keeps_the_removed_line_as_context() {
        let expected = [
            FILE_HEADER,
            "@@ -1,4 +1,5 @@ fn main\n",
            " a\n",
            " b\n",
            "+B\n",
            " c\n",
            " d\n",
        ]
        .concat();
        assert_eq!(
            lines_patch(&replace_hunk(), &picked(&[(0, 2)]), Direction::Stage),
            Ok(expected)
        );
    }

    #[test]
    fn stage_minus_only_drops_the_unchosen_addition() {
        let expected = [
            FILE_HEADER,
            "@@ -1,4 +1,3 @@ fn main\n",
            " a\n",
            "-b\n",
            " c\n",
            " d\n",
        ]
        .concat();
        assert_eq!(
            lines_patch(&replace_hunk(), &picked(&[(0, 1)]), Direction::Stage),
            Ok(expected)
        );
    }

    #[test]
    fn unstage_plus_only_drops_the_unchosen_removal() {
        let expected = [
            FILE_HEADER,
            "@@ -1,3 +1,4 @@ fn main\n",
            " a\n",
            "+B\n",
            " c\n",
            " d\n",
        ]
        .concat();
        assert_eq!(
            lines_patch(&replace_hunk(), &picked(&[(0, 2)]), Direction::Unstage),
            Ok(expected)
        );
    }

    #[test]
    fn unstage_minus_only_turns_the_unchosen_addition_into_context() {
        let expected = [
            FILE_HEADER,
            "@@ -1,5 +1,4 @@ fn main\n",
            " a\n",
            "-b\n",
            " B\n",
            " c\n",
            " d\n",
        ]
        .concat();
        assert_eq!(
            lines_patch(&replace_hunk(), &picked(&[(0, 1)]), Direction::Unstage),
            Ok(expected)
        );
    }

    #[test]
    fn choosing_every_line_gives_the_hunk_back() {
        assert_eq!(
            hunk_patch(&replace_hunk(), 0, Direction::Stage),
            Ok(replace_hunk())
        );
        assert_eq!(
            hunk_patch(&replace_hunk(), 0, Direction::Unstage),
            Ok(replace_hunk())
        );
    }

    #[test]
    fn a_hunk_without_a_chosen_change_is_left_out() {
        let expected = [
            FILE_HEADER,
            "@@ -10,3 +10,2 @@ tail\n",
            " x\n",
            "-y\n",
            " z\n",
        ]
        .concat();
        assert_eq!(
            lines_patch(&two_hunks(), &picked(&[(1, 1)]), Direction::Stage),
            Ok(expected)
        );
    }

    #[test]
    fn a_later_hunk_moves_only_by_the_kept_hunks_before_it() {
        // Keeping the first hunk adds one line above the second one.
        assert_eq!(
            lines_patch(
                &two_hunks(),
                &picked(&[(0, 1), (1, 1), (1, 2)]),
                Direction::Stage
            ),
            Ok(two_hunks())
        );
        // Without the first hunk the second one starts at line 10 again.
        let alone = [
            FILE_HEADER,
            "@@ -10,3 +10,3 @@ tail\n",
            " x\n",
            "-y\n",
            "+Y\n",
            " z\n",
        ]
        .concat();
        assert_eq!(
            lines_patch(&two_hunks(), &picked(&[(1, 1), (1, 2)]), Direction::Stage),
            Ok(alone)
        );
        // Keeping the first hunk moves the second one to line 11.
        let shifted = [
            FILE_HEADER,
            "@@ -1,3 +1,4 @@\n",
            " a\n",
            "+NEW\n",
            " b\n",
            " c\n",
            "@@ -10,3 +11,2 @@ tail\n",
            " x\n",
            "-y\n",
            " z\n",
        ]
        .concat();
        assert_eq!(
            lines_patch(&two_hunks(), &picked(&[(0, 1), (1, 1)]), Direction::Stage),
            Ok(shifted)
        );
    }

    #[test]
    fn unstage_moves_a_later_hunk_back_by_the_kept_hunks_before_it() {
        assert_eq!(
            lines_patch(
                &two_hunks(),
                &picked(&[(0, 1), (1, 1), (1, 2)]),
                Direction::Unstage
            ),
            Ok(two_hunks())
        );
        let alone = [
            FILE_HEADER,
            "@@ -11,3 +11,3 @@ tail\n",
            " x\n",
            "-y\n",
            "+Y\n",
            " z\n",
        ]
        .concat();
        assert_eq!(
            lines_patch(&two_hunks(), &picked(&[(1, 1), (1, 2)]), Direction::Unstage),
            Ok(alone)
        );
    }

    #[test]
    fn a_pure_addition_keeps_its_start_before_the_insertion() {
        let diff = [FILE_HEADER, "@@ -2,0 +3,2 @@\n", "+x\n", "+y\n"].concat();
        assert_eq!(hunk_patch(&diff, 0, Direction::Stage), Ok(diff.clone()));
        let only_y = [FILE_HEADER, "@@ -2,0 +3,1 @@\n", "+y\n"].concat();
        assert_eq!(
            lines_patch(&diff, &picked(&[(0, 1)]), Direction::Stage),
            Ok(only_y)
        );
    }

    #[test]
    fn a_whole_deletion_and_a_deletion_in_the_middle_keep_git_zero_starts() {
        let whole = [FILE_HEADER, "@@ -1,2 +0,0 @@\n", "-a\n", "-b\n"].concat();
        assert_eq!(hunk_patch(&whole, 0, Direction::Stage), Ok(whole.clone()));
        assert_eq!(hunk_patch(&whole, 0, Direction::Unstage), Ok(whole.clone()));
        let middle = [FILE_HEADER, "@@ -2,2 +1,0 @@\n", "-2\n", "-3\n"].concat();
        assert_eq!(hunk_patch(&middle, 0, Direction::Stage), Ok(middle.clone()));
        assert_eq!(
            hunk_patch(&middle, 0, Direction::Unstage),
            Ok(middle.clone())
        );
    }

    #[test]
    fn a_context_only_choice_is_nothing_chosen() {
        assert_eq!(
            lines_patch(&replace_hunk(), &picked(&[(0, 0)]), Direction::Stage),
            Err(PatchError::NothingChosen)
        );
        assert_eq!(
            lines_patch(&replace_hunk(), &picked(&[]), Direction::Unstage),
            Err(PatchError::NothingChosen)
        );
        let context = [FILE_HEADER, "@@ -1,2 +1,2 @@\n", " a\n", " b\n"].concat();
        assert_eq!(
            hunk_patch(&context, 0, Direction::Stage),
            Err(PatchError::NothingChosen)
        );
        assert_eq!(
            hunk_patch(&replace_hunk(), 7, Direction::Stage),
            Err(PatchError::NothingChosen)
        );
    }

    #[test]
    fn a_whole_file_change_is_refused() {
        let kinds = [
            "new file mode 100644",
            "deleted file mode 100644",
            "rename from old.txt",
            "copy from old.txt",
            "old mode 100644",
            "Binary files a/f.txt and b/f.txt differ",
            "GIT binary patch",
        ];
        for kind in kinds {
            let diff = format!("diff --git a/f.txt b/f.txt\n{kind}\n@@ -1 +1 @@\n-a\n+b\n");
            assert_eq!(
                hunk_patch(&diff, 0, Direction::Stage),
                Err(PatchError::WholeFileOnly),
                "{kind}"
            );
            assert_eq!(
                lines_patch(&diff, &picked(&[(0, 0), (0, 1)]), Direction::Unstage),
                Err(PatchError::WholeFileOnly),
                "{kind}"
            );
        }
    }

    #[test]
    fn error_messages_read_as_one_line() {
        assert_eq!(
            PatchError::NothingChosen.to_string(),
            "no changed line is selected"
        );
        assert_eq!(
            PatchError::WholeFileOnly.to_string(),
            "this file can only be staged as a whole"
        );
    }

    #[test]
    fn crlf_lines_keep_their_carriage_return() {
        let diff = [
            FILE_HEADER,
            "@@ -1,2 +1,2 @@\n",
            " x\r\n",
            "-y\r\n",
            "+Y\r\n",
        ]
        .concat();
        let expected = [FILE_HEADER, "@@ -1,2 +1,1 @@\n", " x\r\n", "-y\r\n"].concat();
        assert_eq!(
            lines_patch(&diff, &picked(&[(0, 1)]), Direction::Stage),
            Ok(expected)
        );
    }

    #[test]
    fn no_newline_marker_follows_the_line_it_belongs_to() {
        let diff = [
            FILE_HEADER,
            "@@ -1,2 +1,2 @@\n",
            " a\n",
            "-b\n",
            "\\ No newline at end of file\n",
            "+B\n",
            "\\ No newline at end of file\n",
        ]
        .concat();
        // Staging the removal keeps its marker and drops the addition's.
        let stage = [
            FILE_HEADER,
            "@@ -1,2 +1,1 @@\n",
            " a\n",
            "-b\n",
            "\\ No newline at end of file\n",
        ]
        .concat();
        assert_eq!(
            lines_patch(&diff, &picked(&[(0, 1)]), Direction::Stage),
            Ok(stage)
        );
        // Unstaging the addition drops the removal and its marker.
        let unstage = [
            FILE_HEADER,
            "@@ -1,1 +1,2 @@\n",
            " a\n",
            "+B\n",
            "\\ No newline at end of file\n",
        ]
        .concat();
        assert_eq!(
            lines_patch(&diff, &picked(&[(0, 2)]), Direction::Unstage),
            Ok(unstage)
        );
    }

    #[test]
    fn a_marked_removal_before_a_chosen_addition_gets_its_newline_back() {
        // `a\nb` became `a\nb\nc`: git lists `-b` (no newline), `+b`, then `+c`.
        let diff = [
            FILE_HEADER,
            "@@ -1,2 +1,3 @@\n",
            " a\n",
            "-b\n",
            "\\ No newline at end of file\n",
            "+b\n",
            "+c\n",
            "\\ No newline at end of file\n",
        ]
        .concat();
        // Choosing `+c` alone keeps `-b` as a removal and writes `+b` back.
        assert_eq!(
            lines_patch(&diff, &picked(&[(0, 3)]), Direction::Stage),
            Ok(diff.clone())
        );
        // Choosing `+b` alone keeps the removal and drops `+c` with its marker.
        let only_b = [
            FILE_HEADER,
            "@@ -1,2 +1,2 @@\n",
            " a\n",
            "-b\n",
            "\\ No newline at end of file\n",
            "+b\n",
        ]
        .concat();
        assert_eq!(
            lines_patch(&diff, &picked(&[(0, 2)]), Direction::Stage),
            Ok(only_b)
        );
    }

    /// A temporary repository whose one commit holds the file `f`.
    struct Repo {
        dir: tempfile::TempDir,
    }

    impl Repo {
        /// A repository whose only commit has `committed` as file `f`.
        fn with_file(committed: &str) -> Self {
            let repo = Self {
                dir: tempfile::tempdir().expect("create temp dir"),
            };
            repo.git(&["init", "-q", "-b", "main"]);
            repo.git(&["config", "user.email", "test@example.com"]);
            repo.git(&["config", "user.name", "Test"]);
            repo.git(&["config", "commit.gpgsign", "false"]);
            repo.git(&["config", "core.autocrlf", "false"]);
            repo.write(committed);
            repo.git(&["add", "f"]);
            repo.git(&["commit", "-q", "-m", "init"]);
            repo
        }

        fn path(&self) -> &Path {
            self.dir.path()
        }

        /// Replaces the worktree file `f`.
        fn write(&self, content: &str) {
            std::fs::write(self.path().join("f"), content).expect("write f");
        }

        /// Runs git in the repository and returns its standard output.
        fn git(&self, args: &[&str]) -> Vec<u8> {
            let output = Command::new("git")
                .args(args)
                .current_dir(self.path())
                .output()
                .expect("run git");
            assert!(
                output.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            output.stdout
        }

        /// The diff of `f` with fixed prefixes; `extra` adds flags such as `--cached`.
        fn diff(&self, extra: &[&str]) -> String {
            let mut args = vec![
                "diff",
                "--no-color",
                "--no-ext-diff",
                "--src-prefix=a/",
                "--dst-prefix=b/",
            ];
            args.extend_from_slice(extra);
            args.extend_from_slice(&["--", "f"]);
            String::from_utf8(self.git(&args)).expect("diff is UTF-8")
        }

        /// Stages the whole worktree file `f`.
        fn add(&self) {
            self.git(&["add", "f"]);
        }

        /// The content of `f` in the index, byte for byte.
        fn index(&self) -> Vec<u8> {
            self.git(&["show", ":f"])
        }

        /// Whether the index matches HEAD.
        fn index_is_head(&self) -> bool {
            Command::new("git")
                .args(["diff", "--cached", "--quiet"])
                .current_dir(self.path())
                .status()
                .expect("run git")
                .success()
        }

        /// Applies `patch` to the index with `git apply --cached`, reversed when `reverse`.
        fn apply(&self, patch: &str, reverse: bool) {
            self.apply_with(patch, reverse, &[]);
        }

        /// `apply` with extra flags, such as `--unidiff-zero` for a hunk without context.
        fn apply_with(&self, patch: &str, reverse: bool, flags: &[&str]) {
            let mut args = vec!["apply", "--cached"];
            args.extend_from_slice(flags);
            if reverse {
                args.push("--reverse");
            }
            args.push("-");
            let mut child = Command::new("git")
                .args(&args)
                .current_dir(self.path())
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn git apply");
            child
                .stdin
                .take()
                .expect("piped stdin")
                .write_all(patch.as_bytes())
                .expect("write patch");
            let output = child.wait_with_output().expect("wait for git apply");
            assert!(
                output.status.success(),
                "git apply failed: {}\n{patch}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    /// Thirty lines, `line 1` to `line 30`, with the given line numbers replaced.
    fn numbered(replaced: &[(usize, &str)]) -> String {
        (1..=30)
            .flat_map(|number| {
                let line = match replaced.iter().find(|(at, _)| *at == number) {
                    Some((_, text)) => (*text).to_owned(),
                    None => format!("line {number}"),
                };
                [line, "\n".to_owned()]
            })
            .collect()
    }

    /// `base` with the line `text` inserted after line `after`.
    fn insert_after(base: &str, after: usize, text: &str) -> String {
        base.lines()
            .enumerate()
            .flat_map(|(index, line)| {
                let mut out = vec![format!("{line}\n")];
                if index + 1 == after {
                    out.push(format!("{text}\n"));
                }
                out
            })
            .collect()
    }

    /// The lines of a small file, each ending in a newline.
    fn text(lines: &[&str]) -> String {
        lines.iter().flat_map(|line| [*line, "\n"]).collect()
    }

    #[test]
    fn stage_the_first_of_two_hunks() {
        let repo = Repo::with_file(&numbered(&[]));
        repo.write(&numbered(&[(2, "two"), (20, "twenty")]));
        let diff = repo.diff(&[]);
        assert_eq!(super::split(&diff).1.len(), 2);
        let patch = hunk_patch(&diff, 0, Direction::Stage).expect("patch");
        repo.apply(&patch, false);
        assert_eq!(repo.index(), numbered(&[(2, "two")]).as_bytes());
    }

    #[test]
    fn stage_the_second_of_two_hunks_alone() {
        let repo = Repo::with_file(&numbered(&[]));
        repo.write(&numbered(&[(2, "two"), (20, "twenty")]));
        let patch = hunk_patch(&repo.diff(&[]), 1, Direction::Stage).expect("patch");
        repo.apply(&patch, false);
        assert_eq!(repo.index(), numbered(&[(20, "twenty")]).as_bytes());
    }

    #[test]
    fn stage_every_line_of_a_later_hunk_after_an_insertion() {
        let worktree = insert_after(&numbered(&[(20, "twenty")]), 2, "inserted");
        let repo = Repo::with_file(&numbered(&[]));
        repo.write(&worktree);
        let diff = repo.diff(&[]);
        let patch = lines_patch(&diff, &every_line(&diff), Direction::Stage).expect("patch");
        repo.apply(&patch, false);
        assert_eq!(repo.index(), worktree.as_bytes());
    }

    #[test]
    fn stage_only_the_later_hunk_after_an_insertion() {
        let repo = Repo::with_file(&numbered(&[]));
        repo.write(&insert_after(&numbered(&[(20, "twenty")]), 2, "inserted"));
        let diff = repo.diff(&[]);
        let patch = hunk_patch(&diff, 1, Direction::Stage).expect("patch");
        repo.apply(&patch, false);
        assert_eq!(repo.index(), numbered(&[(20, "twenty")]).as_bytes());
    }

    #[test]
    fn unstage_the_first_of_two_staged_hunks() {
        let repo = Repo::with_file(&numbered(&[]));
        repo.write(&numbered(&[(2, "two"), (20, "twenty")]));
        repo.add();
        let diff = repo.diff(&["--cached"]);
        assert_eq!(super::split(&diff).1.len(), 2);
        let patch = hunk_patch(&diff, 0, Direction::Unstage).expect("patch");
        repo.apply(&patch, true);
        assert_eq!(repo.index(), numbered(&[(20, "twenty")]).as_bytes());
    }

    #[test]
    fn unstage_one_of_three_added_lines() {
        let head = text(&[
            "one", "two", "three", "four", "five", "six", "seven", "eight",
        ]);
        let worktree = text(&[
            "one", "two", "three", "four", "five", "x1", "x2", "x3", "six", "seven", "eight",
        ]);
        let repo = Repo::with_file(&head);
        repo.write(&worktree);
        repo.add();
        let diff = repo.diff(&["--cached"]);
        // Hunk lines: three, four, five, +x1, +x2, +x3, six, seven, eight.
        let patch = lines_patch(&diff, &picked(&[(0, 4)]), Direction::Unstage).expect("patch");
        repo.apply(&patch, true);
        let expected = text(&[
            "one", "two", "three", "four", "five", "x1", "x3", "six", "seven", "eight",
        ]);
        assert_eq!(repo.index(), expected.as_bytes());
    }

    #[test]
    fn stage_then_unstage_the_same_hunk_restores_head() {
        let repo = Repo::with_file(&numbered(&[]));
        repo.write(&numbered(&[(2, "two"), (20, "twenty")]));
        let patch = hunk_patch(&repo.diff(&[]), 0, Direction::Stage).expect("patch");
        repo.apply(&patch, false);
        let staged = repo.diff(&["--cached"]);
        let back = hunk_patch(&staged, 0, Direction::Unstage).expect("patch");
        repo.apply(&back, true);
        assert!(repo.index_is_head());
        assert_eq!(repo.index(), numbered(&[]).as_bytes());
    }

    #[test]
    fn stage_one_plus_line_then_unstage_it_restores_head() {
        let head = text(&["a", "b", "c", "d"]);
        let repo = Repo::with_file(&head);
        repo.write(&text(&["a", "B", "c", "d"]));
        let patch =
            lines_patch(&repo.diff(&[]), &picked(&[(0, 2)]), Direction::Stage).expect("patch");
        repo.apply(&patch, false);
        assert_eq!(repo.index(), text(&["a", "b", "B", "c", "d"]).as_bytes());
        let staged = repo.diff(&["--cached"]);
        let back = lines_patch(&staged, &picked(&[(0, 2)]), Direction::Unstage).expect("patch");
        repo.apply(&back, true);
        assert!(repo.index_is_head());
    }

    #[test]
    fn stage_a_last_line_without_a_final_newline() {
        let repo = Repo::with_file("a\nb");
        repo.write("a\nB");
        let diff = repo.diff(&[]);
        let all = lines_patch(&diff, &every_line(&diff), Direction::Stage).expect("patch");
        repo.apply(&all, false);
        assert_eq!(repo.index(), b"a\nB");
    }

    #[test]
    fn stage_every_line_of_a_file_gaining_a_final_line_without_newline() {
        let repo = Repo::with_file("a\nb");
        repo.write("a\nb\nc");
        let diff = repo.diff(&[]);
        let all = lines_patch(&diff, &every_line(&diff), Direction::Stage).expect("patch");
        repo.apply(&all, false);
        assert_eq!(repo.index(), b"a\nb\nc");
    }

    #[test]
    fn stage_a_removal_of_a_last_line_without_a_final_newline() {
        let repo = Repo::with_file("a\nb");
        repo.write("a\nB");
        let diff = repo.diff(&[]);
        let patch = lines_patch(&diff, &picked(&[(0, 1)]), Direction::Stage).expect("patch");
        repo.apply(&patch, false);
        assert_eq!(repo.index(), b"a\n");
    }

    #[test]
    fn stage_only_the_added_last_line_of_a_file_without_final_newline() {
        let repo = Repo::with_file("a\nb");
        repo.write("a\nb\nc");
        let diff = repo.diff(&[]);
        let patch = lines_patch(&diff, &picked(&[(0, 3)]), Direction::Stage).expect("patch");
        repo.apply(&patch, false);
        assert_eq!(repo.index(), b"a\nb\nc");
        // Unstaging everything that is staged gives back HEAD.
        let staged = repo.diff(&["--cached"]);
        let back = hunk_patch(&staged, 0, Direction::Unstage).expect("patch");
        repo.apply(&back, true);
        assert!(repo.index_is_head());
    }

    #[test]
    fn stage_only_the_added_middle_line_of_a_file_without_final_newline() {
        let repo = Repo::with_file("a\nb");
        repo.write("a\nb\nc");
        let diff = repo.diff(&[]);
        let patch = lines_patch(&diff, &picked(&[(0, 2)]), Direction::Stage).expect("patch");
        repo.apply(&patch, false);
        assert_eq!(repo.index(), b"a\nb\n");
    }

    #[test]
    fn stage_only_the_replaced_last_line_of_a_file_without_final_newline() {
        let repo = Repo::with_file("a\nb");
        repo.write("a\nB");
        let diff = repo.diff(&[]);
        let patch = lines_patch(&diff, &picked(&[(0, 2)]), Direction::Stage).expect("patch");
        repo.apply(&patch, false);
        assert_eq!(repo.index(), b"a\nb\nB");
    }

    #[test]
    fn stage_keeps_crlf_line_endings() {
        let repo = Repo::with_file("x\r\ny\r\nz\r\n");
        repo.write("x\r\nY\r\nz\r\n");
        let diff = repo.diff(&[]);
        let plus_only = lines_patch(&diff, &picked(&[(0, 2)]), Direction::Stage).expect("patch");
        repo.apply(&plus_only, false);
        assert_eq!(repo.index(), b"x\r\ny\r\nY\r\nz\r\n");

        let repo = Repo::with_file("x\r\ny\r\nz\r\n");
        repo.write("x\r\nY\r\nz\r\n");
        let all = hunk_patch(&repo.diff(&[]), 0, Direction::Stage).expect("patch");
        repo.apply(&all, false);
        assert_eq!(repo.index(), b"x\r\nY\r\nz\r\n");
    }

    #[test]
    fn stage_a_hunk_at_the_top_of_the_file() {
        let repo = Repo::with_file(&text(&["a", "b", "c", "d"]));
        repo.write(&text(&["A", "b", "c", "d"]));
        let patch = hunk_patch(&repo.diff(&[]), 0, Direction::Stage).expect("patch");
        repo.apply(&patch, false);
        assert_eq!(repo.index(), text(&["A", "b", "c", "d"]).as_bytes());
    }

    #[test]
    fn stage_a_pure_addition_at_the_top_with_no_context() {
        let repo = Repo::with_file(&text(&["1", "2", "3", "4", "5"]));
        let worktree = text(&["x", "1", "2", "3", "4", "5"]);
        repo.write(&worktree);
        let diff = repo.diff(&["-U0"]);
        assert!(diff.contains("@@ -0,0 +1 @@"), "{diff}");
        let patch = hunk_patch(&diff, 0, Direction::Stage).expect("patch");
        repo.apply_with(&patch, false, &["--unidiff-zero"]);
        assert_eq!(repo.index(), worktree.as_bytes());
    }

    #[test]
    fn stage_and_unstage_a_deletion_with_no_context() {
        let repo = Repo::with_file(&text(&["1", "2", "3", "4", "5"]));
        repo.write(&text(&["1", "4", "5"]));
        let diff = repo.diff(&["-U0"]);
        assert!(diff.contains("@@ -2,2 +1,0 @@"), "{diff}");
        let patch = hunk_patch(&diff, 0, Direction::Stage).expect("patch");
        repo.apply_with(&patch, false, &["--unidiff-zero"]);
        assert_eq!(repo.index(), text(&["1", "4", "5"]).as_bytes());
        let staged = repo.diff(&["--cached", "-U0"]);
        let back = hunk_patch(&staged, 0, Direction::Unstage).expect("patch");
        repo.apply_with(&back, true, &["--unidiff-zero"]);
        assert!(repo.index_is_head());
    }
}
