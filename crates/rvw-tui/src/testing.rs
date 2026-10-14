//! Test fixtures shared by the model, update and view tests: a git repository
//! with every file committed on `main`, and a store beside it. Port of the
//! fixtures in `internal/tui/tui_test.go` and `golden_test.go`.

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use regex_lite_dates::mask_dates;
use rvw_core::operations::AddInput;
use rvw_core::types::Comment;
use rvw_core::Service;
use tempfile::TempDir;

use crate::model::{Effect, Event, Model};
use crate::Options;

pub const PARSE_PY: &str = "import parse


def handle(req):
    if req.kind == \"x\":
        body = req.body
        return parse(body)
    return default(req)


def default(req):
\treturn None
";

pub struct Fixture {
    pub dir: TempDir,
    pub svc: Arc<Service>,
    /// The canonical workspace path.
    pub ws: String,
}

/// A repository with `src/api/parse.py`, `src/util.py`,
/// `test/api/api_parser_test.go`, `docs/api/papers.md` and `README.md`
/// committed on `main`.
pub fn setup() -> Fixture {
    let dir = TempDir::new().expect("tempdir");
    let root = std::fs::canonicalize(dir.path()).expect("canonical tempdir");
    let ws = root.join("proj");
    std::fs::create_dir_all(&ws).expect("workspace dir");
    let f_ws = ws.to_string_lossy().into_owned();
    let store = rvw_store::Store::open(&root.join("rvw.db")).expect("store");
    let svc = Arc::new(Service::new(Box::new(store)));
    let f = Fixture { dir, svc, ws: f_ws };
    f.write("src/api/parse.py", PARSE_PY);
    f.write("src/util.py", "def util():\n    pass\n");
    f.write("test/api/api_parser_test.go", "package api\n");
    f.write("docs/api/papers.md", "# Papers\n");
    f.write("README.md", "# Demo\n\nhello\n");
    f.git(&["init", "-q"]);
    f.git(&["add", "."]);
    f.git(&["commit", "-qm", "init"]);
    f.git(&["branch", "-M", "main"]);
    f
}

impl Fixture {
    pub fn write(&self, rel: &str, content: &str) {
        let path = Path::new(&self.ws).join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).expect("mkdir");
        std::fs::write(path, content).expect("write file");
    }

    pub fn git(&self, args: &[&str]) {
        let out = Command::new("git")
            .args(["-C", &self.ws])
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .expect("run git");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    pub fn add(&self, file: &str, start: i64, end: i64, text: &str, author: &str) -> Comment {
        self.svc
            .add(AddInput {
                points_at: None,
                workspace: self.ws.clone(),
                file: file.to_owned(),
                start_line: start,
                end_line: end,
                comment: text.to_owned(),
                author: author.to_owned(),
                ..AddInput::default()
            })
            .expect("add comment")
    }

    /// The model the Go tests use: author `tester`, editor `true`, 100x30.
    pub fn model(&self) -> Model {
        let mut m = Model::new(Options {
            service: Arc::clone(&self.svc),
            workspace: self.ws.clone(),
            lane: String::new(),
            author: "tester".to_owned(),
            editor: "true".to_owned(),
            style: String::new(),
            leader: String::new(),
        })
        .expect("model");
        m.update(Event::Resize {
            width: 100,
            height: 30,
        });
        m
    }
}

/// The key name for a test key: `leader` is the space key.
fn key_name(k: &str) -> &str {
    if k == "leader" {
        "space"
    } else {
        k
    }
}

/// Presses named keys (`leader`, `enter`, `esc`, `tab`, `down`, ...), and types
/// any other string one character at a time. Returns every effect produced.
pub fn keys(m: &mut Model, ks: &[&str]) -> Vec<Effect> {
    let named = [
        "leader",
        "enter",
        "esc",
        "tab",
        "down",
        "up",
        "backspace",
        "space",
    ];
    let mut effects = Vec::new();
    for k in ks {
        if named.contains(k) || k.starts_with("ctrl+") || *k == "pgdown" || *k == "pgup" {
            effects.extend(m.update(Event::Key(key_name(k).to_owned())));
        } else {
            for c in k.chars() {
                effects.extend(m.update(Event::Key(c.to_string())));
            }
        }
    }
    effects
}

/// The screen as plain text (colors stripped), one row per line joined by
/// `\n`, with dates masked as in the Go goldens.
pub fn screen_text(m: &Model) -> String {
    let text = m
        .screen()
        .iter()
        .map(|row| row.iter().map(|p| p.text.as_str()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");
    mask_dates(&text)
}

/// A tiny `\d{4}-\d{2}-\d{2}` masker, so no regex dependency is needed.
mod regex_lite_dates {
    pub fn mask_dates(s: &str) -> String {
        let b: Vec<char> = s.chars().collect();
        let mut out = String::new();
        let mut i = 0;
        while i < b.len() {
            let is_date = i + 10 <= b.len()
                && b[i..i + 4].iter().all(char::is_ascii_digit)
                && b[i + 4] == '-'
                && b[i + 5..i + 7].iter().all(char::is_ascii_digit)
                && b[i + 7] == '-'
                && b[i + 8..i + 10].iter().all(char::is_ascii_digit);
            if is_date {
                out.push_str("YYYY-MM-DD");
                i += 10;
            } else {
                out.push(b[i]);
                i += 1;
            }
        }
        out
    }
}
