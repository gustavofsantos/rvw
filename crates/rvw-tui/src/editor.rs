//! Port of internal/tui/editor.go: the temp file a comment is written in, the
//! scissors-line template around it, and the editor command that opens it.

use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

/// Separates what the user writes from the context below it, as in
/// `git commit --cleanup=scissors`.
pub const SCISSORS: &str = "# ------------------------ >8 ------------------------";

/// Caps the code quoted into the temp file.
pub const MAX_CONTEXT_LINES: usize = 40;

/// The text the editor opens: `body`, then the scissors line and `#`-prefixed
/// `context` lines below it. A body without a trailing newline gets one, and
/// a blank line separates the body from the scissors line.
#[must_use]
pub fn template(body: &str, context: &[String]) -> String {
    let mut out = String::from(body);
    if !body.ends_with('\n') {
        out.push('\n');
    }
    out.push('\n');
    out.push_str(SCISSORS);
    out.push('\n');
    out.push_str("# Do not modify or remove the line above.\n");
    out.push_str("# Everything below it is ignored. Save an empty note to cancel.\n");
    for line in context {
        out.push_str(format!("# {line}").trim_end_matches(' '));
        out.push('\n');
    }
    out
}

/// What the user wrote, read back from the edited template: everything above
/// the scissors line, or, when that line was removed, every line not starting
/// with `#`. Surrounding whitespace is trimmed.
#[must_use]
pub fn strip_template(text: &str) -> String {
    let normalized = text.replace("\r\n", "\n");
    let lines: Vec<&str> = normalized.split('\n').collect();
    let kept: Vec<&str> = match lines.iter().position(|&l| l == SCISSORS) {
        Some(cut) => lines[..cut].to_vec(),
        None => lines.into_iter().filter(|l| !l.starts_with('#')).collect(),
    };
    kept.join("\n").trim().to_owned()
}

/// The context lines quoting a range of code: a blank line, then each line
/// numbered from `start` with a right-aligned number, up to
/// [`MAX_CONTEXT_LINES`], then a note of how many lines were left out.
#[must_use]
pub fn quote_code(lines: &[String], start: i64) -> Vec<String> {
    let count = i64::try_from(lines.len()).unwrap_or(i64::MAX);
    let width = (start + count - 1).to_string().len();
    let mut out = vec![String::new()];
    for (i, line) in lines.iter().enumerate() {
        if i == MAX_CONTEXT_LINES {
            out.push(format!("  {:width$}  … {} more lines", "", lines.len() - i));
            break;
        }
        let number = start + i64::try_from(i).unwrap_or(i64::MAX);
        out.push(format!("  {number:>width$}  {line}"));
    }
    out
}

/// Writes `content` to a new temp file named `rvw-*.md` in the system temp
/// directory, readable only by its owner on Unix, and returns its path. The
/// caller removes the file when the editor is done.
///
/// # Errors
///
/// Returns the I/O error when the file cannot be created or written; a file
/// that could not be written is removed again.
pub fn write_temp(content: &str) -> io::Result<PathBuf> {
    let dir = env::temp_dir();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let pid = std::process::id();
    for attempt in 0..100u32 {
        let path = dir.join(format!("rvw-{pid}-{stamp}-{attempt}.md"));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(mut file) => {
                if let Err(e) = file.write_all(content.as_bytes()) {
                    drop(file);
                    let _ = fs::remove_file(&path);
                    return Err(e);
                }
                return Ok(path);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "no free temp file name",
    ))
}

/// The command that opens `path` in `editor`. The editor is a command line
/// run through `sh`, so an editor given with arguments (`code --wait`) works,
/// as it does for git. The command is `sh -c '<editor> "$@"' <editor> <path>`.
#[must_use]
pub fn editor_command(editor: &str, path: &Path) -> Command {
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(format!("{editor} \"$@\""))
        .arg(editor)
        .arg(path);
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn strip_template_reads_back_what_the_user_wrote() {
        let tmpl = template("", &strings(&["New comment on a.py:1", "", "  1  x = 1"]));
        assert_eq!(strip_template(&tmpl), "", "an untouched template is empty");
        assert_eq!(
            strip_template(&format!("# Title\n\nbody\n{tmpl}")),
            "# Title\n\nbody",
            "text above the scissors is kept, headings too"
        );
        assert_eq!(
            strip_template("note\n# context\nmore\n"),
            "note\nmore",
            "without scissors, # lines go"
        );
        assert!(
            template("old text", &[]).starts_with(&format!("old text\n\n{SCISSORS}")),
            "an edit starts with the old text"
        );
    }

    #[test]
    fn strip_template_handles_crlf_and_surrounding_space() {
        assert_eq!(strip_template("  a\r\nb\r\n\r\n"), "a\nb");
        assert_eq!(strip_template("\n\n  \n"), "");
    }

    #[test]
    fn strip_template_cuts_at_the_first_scissors_line_only() {
        let text = format!("one\n{SCISSORS}\ntwo\n{SCISSORS}\nthree");
        assert_eq!(strip_template(&text), "one");
    }

    #[test]
    fn scissors_line_must_match_exactly() {
        // An indented copy is not the scissors line, so it is not a cut, and
        // since it does not start with '#' it stays with the text.
        assert_eq!(
            strip_template(&format!(" {SCISSORS}\n# note\nkept")),
            format!("{SCISSORS}\nkept")
        );
    }

    #[test]
    fn template_adds_a_newline_only_when_the_body_lacks_one() {
        let with = template("body\n", &[]);
        let without = template("body", &[]);
        assert_eq!(with, format!("body\n\n{SCISSORS}\n# Do not modify or remove the line above.\n# Everything below it is ignored. Save an empty note to cancel.\n"));
        assert_eq!(with, without);
    }

    #[test]
    fn template_trims_trailing_spaces_on_context_lines() {
        let out = template("", &strings(&["ctx   ", "  x"]));
        assert!(out.contains("\n# ctx\n"), "{out:?}");
        assert!(out.contains("\n#   x\n"), "{out:?}");
    }

    #[test]
    fn quote_code_numbers_lines_right_aligned() {
        let got = quote_code(&strings(&["a", "b"]), 9);
        assert_eq!(got, ["", "   9  a", "  10  b"]);
    }

    #[test]
    fn quote_code_caps_the_lines_and_says_how_many_are_left() {
        let lines: Vec<String> = (1..=45).map(|i| format!("l{i}")).collect();
        let got = quote_code(&lines, 1);
        assert_eq!(got.len(), 1 + MAX_CONTEXT_LINES + 1);
        assert_eq!(got[1], "   1  l1");
        assert_eq!(
            got.last().unwrap(),
            "      … 5 more lines",
            "the note is padded to the number width"
        );
    }

    #[test]
    fn quote_code_of_no_lines_is_just_the_blank_line() {
        assert_eq!(quote_code(&[], 3), [""]);
    }

    #[test]
    fn quote_code_keeps_unicode_and_trailing_text_as_given() {
        let got = quote_code(&strings(&["π = 3.14  ", "日本"]), 1);
        assert_eq!(got[1], "  1  π = 3.14  ");
        assert_eq!(got[2], "  2  日本");
    }

    #[test]
    fn write_temp_round_trips_and_is_removed_by_the_caller() {
        let path = write_temp("hello\nworld\n").unwrap();
        assert!(path.starts_with(env::temp_dir()));
        assert_eq!(path.extension(), Some(OsStr::new("md")));
        assert!(path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("rvw-"));
        assert_eq!(fs::read_to_string(&path).unwrap(), "hello\nworld\n");
        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn write_temp_gives_a_fresh_path_each_time() {
        let a = write_temp("a").unwrap();
        let b = write_temp("b").unwrap();
        assert_ne!(a, b);
        fs::remove_file(a).unwrap();
        fs::remove_file(b).unwrap();
    }

    #[test]
    fn editor_command_runs_the_editor_line_through_sh() {
        let cmd = editor_command("code --wait", Path::new("/tmp/x.md"));
        assert_eq!(cmd.get_program(), OsStr::new("sh"));
        let args: Vec<&OsStr> = cmd.get_args().collect();
        assert_eq!(
            args,
            [
                OsStr::new("-c"),
                OsStr::new("code --wait \"$@\""),
                OsStr::new("code --wait"),
                OsStr::new("/tmp/x.md"),
            ]
        );
    }

    #[test]
    fn editor_command_passes_the_path_as_an_argument() {
        let dir = env::temp_dir();
        let path = dir.join(format!("rvw-editor-test-{}.md", std::process::id()));
        fs::write(&path, "seed").unwrap();
        // `sh -c 'printf ... "$@"'` receives the path as $1, and so does `true`.
        let status = editor_command("true", &path).status().unwrap();
        assert!(status.success());
        let status = editor_command("test -f", &path).status().unwrap();
        assert!(
            status.success(),
            "the path reaches the editor as an argument"
        );
        fs::remove_file(&path).unwrap();
    }
}
