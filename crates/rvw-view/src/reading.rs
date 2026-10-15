//! Read marks: a file is read for a lane until its blob changes (M7 slice 6).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The files the user has read, per lane, each with the blob id it was read at.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReadMarks {
    /// (lane key, file path) -> blob id the user read
    marks: BTreeMap<(String, String), String>,
}

impl ReadMarks {
    /// Marks `path` as read in `lane` at `blob`, replacing an earlier mark.
    pub fn mark(&mut self, lane: &str, path: &str, blob: &str) {
        self.marks
            .insert((lane.to_owned(), path.to_owned()), blob.to_owned());
    }

    /// Forgets the mark of `path` in `lane`.
    pub fn unmark(&mut self, lane: &str, path: &str) {
        self.marks.remove(&(lane.to_owned(), path.to_owned()));
    }

    /// Whether `path` in `lane` was read at the blob it has now. A changed
    /// blob is unread again.
    pub fn is_read(&self, lane: &str, path: &str, current_blob: &str) -> bool {
        self.marks
            .get(&(lane.to_owned(), path.to_owned()))
            .is_some_and(|blob| blob == current_blob)
    }

    /// How many of `files` (path, current blob) are unread in `lane`.
    pub fn unread_count(&self, lane: &str, files: &[(String, String)]) -> usize {
        files
            .iter()
            .filter(|(path, blob)| !self.is_read(lane, path, blob))
            .count()
    }
}

/// The GUI state of one project: read marks, the last centre tab of each lane
/// and the collapsed groups. Losing it loses only these.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GuiState {
    /// read marks of every lane
    pub read: ReadMarks,
    /// lane key -> the centre tab the lane returns to
    pub last_tab: BTreeMap<String, String>,
    /// project names whose group is collapsed
    pub collapsed: BTreeSet<String>,
}

impl GuiState {
    /// Reads the state file text. Never fails: unreadable, empty or corrupt
    /// text gives the default state, and each unknown or malformed line is
    /// skipped.
    pub fn parse(text: &str) -> GuiState {
        let mut state = GuiState::default();
        for line in text.lines() {
            let fields: Option<Vec<String>> = line.split('\t').map(unescape).collect();
            let Some(fields) = fields else {
                continue;
            };
            match fields.iter().map(String::as_str).collect::<Vec<_>>()[..] {
                ["read", lane, path, blob] => state.read.mark(lane, path, blob),
                ["tab", lane, tab] => {
                    state.last_tab.insert(lane.to_owned(), tab.to_owned());
                }
                ["collapsed", project] => {
                    state.collapsed.insert(project.to_owned());
                }
                _ => {}
            }
        }
        state
    }

    /// The state file text. Sorted, so equal states render equal strings.
    pub fn render(&self) -> String {
        let mut out = String::new();
        for ((lane, path), blob) in &self.read.marks {
            push_line(&mut out, &["read", lane, path, blob]);
        }
        for (lane, tab) in &self.last_tab {
            push_line(&mut out, &["tab", lane, tab]);
        }
        for project in &self.collapsed {
            push_line(&mut out, &["collapsed", project]);
        }
        out
    }
}

/// The state file of `project`: `data_dir/gui/<project>.state`. The project
/// name is made file-safe: path separators become `_`, and a leading `.` too,
/// so no name can leave the `gui` directory.
pub fn state_path(data_dir: &Path, project: &str) -> PathBuf {
    let mut name: String = project
        .chars()
        .map(|c| if c == '/' || c == '\\' { '_' } else { c })
        .collect();
    if name.is_empty() {
        name.push('_');
    } else if name.starts_with('.') {
        name.replace_range(..1, "_");
    }
    data_dir.join("gui").join(format!("{name}.state"))
}

/// Writes one tab-separated line with each field escaped.
fn push_line(out: &mut String, fields: &[&str]) {
    let escaped: Vec<String> = fields.iter().map(|f| escape(f)).collect();
    out.push_str(&escaped.join("\t"));
    out.push('\n');
}

/// Escapes backslash, tab, newline and carriage return so a field holds no
/// separator.
fn escape(field: &str) -> String {
    let mut out = String::with_capacity(field.len());
    for c in field.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out
}

/// Reverses [`escape`]; `None` for a malformed escape.
fn unescape(field: &str) -> Option<String> {
    let mut out = String::with_capacity(field.len());
    let mut chars = field.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next()? {
            '\\' => out.push('\\'),
            't' => out.push('\t'),
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            _ => return None,
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(p, b)| ((*p).to_owned(), (*b).to_owned()))
            .collect()
    }

    #[test]
    fn changed_blob_is_unread() {
        let mut marks = ReadMarks::default();
        marks.mark("lane-a", "src/lib.rs", "b1");
        assert!(marks.is_read("lane-a", "src/lib.rs", "b1"));
        assert!(!marks.is_read("lane-a", "src/lib.rs", "b2"));
        assert!(!marks.is_read("lane-b", "src/lib.rs", "b1"));
        marks.unmark("lane-a", "src/lib.rs");
        assert!(!marks.is_read("lane-a", "src/lib.rs", "b1"));
    }

    #[test]
    fn unread_count_counts_changed_and_new_files() {
        let mut marks = ReadMarks::default();
        marks.mark("lane-a", "a.rs", "1");
        marks.mark("lane-a", "b.rs", "2");
        marks.mark("lane-b", "c.rs", "3");
        let now = files(&[
            ("a.rs", "1"),
            ("b.rs", "changed"),
            ("c.rs", "3"),
            ("d.rs", "4"),
        ]);
        // a.rs is read at its blob; b.rs changed, c.rs is only read in lane-b, d.rs is new
        assert_eq!(marks.unread_count("lane-a", &now), 3);
        assert_eq!(marks.unread_count("lane-b", &now), 3);
        assert_eq!(marks.unread_count("lane-a", &[]), 0);
    }

    #[test]
    fn round_trip_keeps_every_field() {
        let mut state = GuiState::default();
        state
            .read
            .mark("lane\ta", "dir/with\\slash\nname.rs", "abc123");
        state.read.mark("lane-b", "x.rs", "def");
        state
            .last_tab
            .insert("lane-a".to_owned(), "terminal 2".to_owned());
        state.collapsed.insert("project one".to_owned());
        let text = state.render();
        assert_eq!(GuiState::parse(&text), state);
    }

    #[test]
    fn corrupt_text_gives_default() {
        let default = GuiState::default();
        assert_eq!(GuiState::parse(""), default);
        assert_eq!(GuiState::parse("\u{0}\u{1}\u{fffd} garbage"), default);
        assert_eq!(GuiState::parse("read\tlane\tpath"), default);
        assert_eq!(GuiState::parse("read\tl\tp\tb\textra"), default);
        assert_eq!(GuiState::parse("read\tbad\\q\tp\tb"), default);
        assert_eq!(GuiState::parse("read\tl\tp\ttrailing\\"), default);
    }

    #[test]
    fn corrupt_lines_do_not_hide_good_ones() {
        let state = GuiState::parse("junk\nread\tlane\ta.rs\tb1\nread\tbroken\n");
        assert!(state.read.is_read("lane", "a.rs", "b1"));
        assert_eq!(
            state.read.unread_count("lane", &files(&[("broken", "x")])),
            1
        );
    }

    #[test]
    fn render_is_deterministic() {
        let mut first = GuiState::default();
        first.read.mark("b", "z.rs", "2");
        first.read.mark("a", "y.rs", "1");
        first.last_tab.insert("b".to_owned(), "t".to_owned());
        first.last_tab.insert("a".to_owned(), "s".to_owned());
        first.collapsed.insert("q".to_owned());
        first.collapsed.insert("p".to_owned());

        let mut second = GuiState::default();
        second.collapsed.insert("p".to_owned());
        second.last_tab.insert("a".to_owned(), "s".to_owned());
        second.read.mark("a", "y.rs", "1");
        second.collapsed.insert("q".to_owned());
        second.read.mark("b", "z.rs", "2");
        second.last_tab.insert("b".to_owned(), "t".to_owned());

        assert_eq!(
            first.render(),
            "read\ta\ty.rs\t1\nread\tb\tz.rs\t2\ntab\ta\ts\ntab\tb\tt\ncollapsed\tp\ncollapsed\tq\n"
        );
        assert_eq!(first.render(), second.render());
    }

    #[test]
    fn state_path_is_file_safe() {
        let dir = Path::new("/data");
        assert_eq!(state_path(dir, "rvw"), Path::new("/data/gui/rvw.state"));
        assert_eq!(state_path(dir, "a/b"), Path::new("/data/gui/a_b.state"));
        assert_eq!(state_path(dir, "a\\b"), Path::new("/data/gui/a_b.state"));
        assert_eq!(
            state_path(dir, "../escape"),
            Path::new("/data/gui/_._escape.state")
        );
        assert_eq!(
            state_path(dir, ".hidden"),
            Path::new("/data/gui/_hidden.state")
        );
        assert_eq!(state_path(dir, ""), Path::new("/data/gui/_.state"));
        for project in ["..", ".", "x/../..", "/abs", ".."] {
            let path = state_path(dir, project);
            assert_eq!(path.parent(), Some(Path::new("/data/gui")), "{project}");
        }
    }
}
