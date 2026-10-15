//! The audit export: the ledger of the current branch written to a file the
//! reviewer names, in the format its extension gives. The ledger is
//! `Service::ledger` with the default range and every lane, rendered by
//! `rvw_render`; this file checks the path and writes the result. The export
//! card (`export_card.rs`) asks for the path and shows what comes back.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use rvw_core::Service;
use rvw_core::operations::LedgerInput;
use rvw_render as render;

/// What a second Enter is asked for when the file is there already.
pub const EXISTS: &str = "File exists. Enter again to overwrite.";

/// The formats an audit file can take, by extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Markdown,
    Html,
    Json,
}

impl Format {
    /// The format of a path's extension, case-insensitively. Any other
    /// extension, or none, is refused with one line.
    pub fn from_path(path: &Path) -> Result<Self, String> {
        let extension = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase);
        match extension.as_deref() {
            Some("md") => Ok(Self::Markdown),
            Some("html") => Ok(Self::Html),
            Some("json") => Ok(Self::Json),
            _ => Err("Use a .md, .html or .json extension".to_owned()),
        }
    }

    /// The format's name, as the card shows it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Markdown => "markdown",
            Self::Html => "html",
            Self::Json => "json",
        }
    }
}

/// The file the card proposes for a branch: `audit-<branch>.md`, with every
/// character a file name cannot keep (a slash, a space) turned into `-`.
pub fn default_file_name(branch: &str) -> String {
    let clean: String = branch
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect();
    if clean.is_empty() {
        "audit.md".to_owned()
    } else {
        format!("audit-{clean}.md")
    }
}

/// The path a typed name names: a relative one is under the workspace.
pub fn target(workspace: &str, typed: &str) -> PathBuf {
    Path::new(workspace).join(typed)
}

/// Writes the ledger of the current branch to `typed`, relative to the
/// workspace. Refuses, before the ledger is read, a path with no name, an
/// extension it cannot render, a folder that does not exist and an existing
/// file unless `overwrite` is set. The file appears whole or not at all.
pub fn export(
    service: &Service,
    workspace: &str,
    typed: &str,
    overwrite: bool,
) -> Result<PathBuf, String> {
    if typed.trim().is_empty() {
        return Err("Type a path for the audit file".to_owned());
    }
    let target = target(workspace, typed.trim());
    let format = Format::from_path(&target)?;
    let parent = match target.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    if !parent.is_dir() {
        return Err(format!("The folder {} does not exist", parent.display()));
    }
    if target.is_dir() {
        return Err(format!("{} is a folder", target.display()));
    }
    if target.exists() && !overwrite {
        return Err(EXISTS.to_owned());
    }
    let ledger = service
        .ledger(LedgerInput {
            workspace: workspace.to_owned(),
            range: String::new(),
            since: String::new(),
            until: String::new(),
            lane: String::new(),
        })
        .map_err(|e| first_line(&e.message))?;
    let text = match format {
        Format::Markdown => render::ledger_markdown(&ledger),
        Format::Html => render::ledger_html(&ledger),
        Format::Json => render::ledger_json(&ledger),
    };
    write_atomic(&target, &text)
        .map_err(|e| format!("Could not write {}: {e}", target.display()))?;
    Ok(target)
}

/// Writes `text` through a temporary file in the same folder, renamed over
/// the target, so a reader never sees half a report. Mirrors the CLI's
/// `--output`.
fn write_atomic(target: &Path, text: &str) -> std::io::Result<()> {
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let dir = match target.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        fs::rename(&tmp, target)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// The first non-empty line of a message, trimmed.
fn first_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("The ledger could not be read.")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use rvw_core::Service;

    use super::*;

    /// A repository with `main` and a `feat` branch one commit ahead of it,
    /// and a service over a store beside it. Returns the workspace path too.
    fn repo_on_feature() -> (tempfile::TempDir, Service, String) {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("proj");
        fs::create_dir(&root).expect("mkdir");
        git(&root, &["init", "-q", "-b", "main"]);
        fs::write(root.join("app.py"), "one\ntwo\nthree\n").expect("write");
        git(&root, &["add", "."]);
        git(&root, &["commit", "-q", "-m", "first"]);
        git(&root, &["checkout", "-q", "-b", "feat"]);
        fs::write(root.join("app.py"), "one\nTWO\nthree\n").expect("write");
        git(&root, &["add", "."]);
        git(&root, &["commit", "-q", "-m", "shout the two"]);
        let workspace = fs::canonicalize(&root)
            .expect("canonicalize")
            .to_string_lossy()
            .into_owned();
        let store = rvw_store::Store::open(&dir.path().join("q.db")).expect("open store");
        (dir, Service::new(Box::new(store)), workspace)
    }

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=rvw", "-c", "user.email=rvw@example.test"])
            .args(args)
            .status()
            .expect("git");
        assert!(status.success(), "git {args:?}");
    }

    fn names_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .expect("read dir")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        names
    }

    #[test]
    fn the_extension_picks_the_format_whatever_its_case() {
        assert_eq!(Format::from_path(Path::new("a.md")), Ok(Format::Markdown));
        assert_eq!(Format::from_path(Path::new("a.HTML")), Ok(Format::Html));
        assert_eq!(Format::from_path(Path::new("a.Json")), Ok(Format::Json));
        for refused in ["a.txt", "audit", "a.md.bak"] {
            assert_eq!(
                Format::from_path(Path::new(refused)),
                Err("Use a .md, .html or .json extension".to_owned()),
                "{refused}"
            );
        }
    }

    #[test]
    fn the_proposed_name_keeps_only_name_characters() {
        assert_eq!(default_file_name("feat"), "audit-feat.md");
        assert_eq!(default_file_name("feature/x y"), "audit-feature-x-y.md");
        assert_eq!(default_file_name("detached abc1"), "audit-detached-abc1.md");
        assert_eq!(default_file_name(""), "audit.md");
    }

    #[test]
    fn writes_markdown_under_the_workspace_and_leaves_no_temp_file() {
        let (_dir, service, workspace) = repo_on_feature();
        let written = export(&service, &workspace, "audit-feat.md", false).expect("export");
        assert_eq!(written, Path::new(&workspace).join("audit-feat.md"));
        let text = fs::read_to_string(&written).expect("read");
        assert!(text.starts_with("# Audit ledger"), "{text}");
        assert!(text.contains("shout the two"), "{text}");
        assert_eq!(
            names_in(Path::new(&workspace)).len(),
            3,
            "app.py, .git, audit"
        );
        assert!(names_in(Path::new(&workspace)).contains(&"audit-feat.md".to_owned()));
        assert!(
            !names_in(Path::new(&workspace))
                .iter()
                .any(|name| Path::new(name).extension().is_some_and(|e| e == "tmp"))
        );
    }

    #[test]
    fn writes_html_and_json_by_extension() {
        let (_dir, service, workspace) = repo_on_feature();
        let html = export(&service, &workspace, "audit.html", false).expect("html");
        assert!(
            fs::read_to_string(html)
                .expect("read")
                .contains("<!doctype html>")
        );
        let json = export(&service, &workspace, "audit.json", false).expect("json");
        let text = fs::read_to_string(json).expect("read");
        assert!(text.contains("\"commits\""), "{text}");
    }

    #[test]
    fn refuses_an_existing_file_until_overwrite_is_set() {
        let (_dir, service, workspace) = repo_on_feature();
        let path = Path::new(&workspace).join("audit.md");
        fs::write(&path, "old report").expect("write");
        assert_eq!(
            export(&service, &workspace, "audit.md", false),
            Err(EXISTS.to_owned())
        );
        assert_eq!(fs::read_to_string(&path).expect("read"), "old report");
        export(&service, &workspace, "audit.md", true).expect("overwrite");
        assert_ne!(fs::read_to_string(&path).expect("read"), "old report");
    }

    #[test]
    fn refuses_a_missing_folder_an_unknown_extension_and_a_blank_path() {
        let (_dir, service, workspace) = repo_on_feature();
        let missing = export(&service, &workspace, "nope/audit.md", false).expect_err("folder");
        assert!(missing.starts_with("The folder "), "{missing}");
        assert!(missing.ends_with("does not exist"), "{missing}");
        assert_eq!(
            export(&service, &workspace, "audit.txt", false),
            Err("Use a .md, .html or .json extension".to_owned())
        );
        assert_eq!(
            export(&service, &workspace, "  ", false),
            Err("Type a path for the audit file".to_owned())
        );
        assert_eq!(names_in(Path::new(&workspace)).len(), 2, "nothing written");
    }

    #[test]
    fn shows_the_default_branch_refusal_verbatim() {
        let (_dir, service, workspace) = repo_on_feature();
        git(Path::new(&workspace), &["checkout", "-q", "main"]);
        assert_eq!(
            export(&service, &workspace, "audit.md", false),
            Err("the ledger of the default branch needs --range or --since".to_owned())
        );
        assert!(!Path::new(&workspace).join("audit.md").exists());
    }

    #[test]
    fn a_relative_path_is_under_the_workspace_and_an_absolute_one_is_kept() {
        let (dir, service, workspace) = repo_on_feature();
        let elsewhere: PathBuf = dir.path().join("out.md");
        let written = export(
            &service,
            &workspace,
            elsewhere.to_str().expect("utf-8 path"),
            false,
        )
        .expect("absolute");
        assert_eq!(written, elsewhere);
        assert!(written.exists());
    }
}
