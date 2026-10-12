//! Fills a tool call's workspace, lane and author from the server's defaults,
//! and resolves the workspace to its canonical git root. Port of `scope` in
//! internal/mcpserver/server.go.

use std::path::Path;

use crate::Options;

/// Scopes one call. `workspace` is replaced by the canonical git root; a lane
/// or author that is blank after trimming takes the server default. Lane and
/// author are passed only by the tools that take them.
///
/// Errors are the message a tool error result carries.
pub fn scope(
    opts: &Options,
    workspace: &mut String,
    lane: Option<&mut String>,
    author: Option<&mut String>,
) -> Result<(), String> {
    let mut dir = workspace.trim().to_owned();
    if dir.is_empty() {
        dir.clone_from(&opts.workspace);
    }
    if dir.is_empty() {
        return Err(
            "workspace is required: pass the absolute path of your project directory".to_owned(),
        );
    }
    if !Path::new(&dir).is_absolute() {
        return Err(format!("workspace '{dir}' is not an absolute path"));
    }
    if !rvw_git::workspace::is_dir(Path::new(&dir)) {
        return Err(format!("workspace '{dir}' is not a directory"));
    }
    let resolved =
        rvw_git::workspace::resolve(Path::new(&dir)).map_err(|e| format!("workspace {e}"))?;
    *workspace = resolved.to_string_lossy().into_owned();
    if let Some(lane) = lane {
        default_field(lane, &opts.lane);
    }
    if let Some(author) = author {
        default_field(author, &opts.author);
    }
    Ok(())
}

/// Trims `field` and, when nothing is left, fills it from `default`.
fn default_field(field: &mut String, default: &str) {
    let trimmed = field.trim().to_owned();
    *field = if trimmed.is_empty() {
        default.to_owned()
    } else {
        trimmed
    };
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::process::Command;

    use super::*;

    fn opts(workspace: &str, lane: &str, author: &str) -> Options {
        Options {
            workspace: workspace.to_owned(),
            lane: lane.to_owned(),
            author: author.to_owned(),
            version: "test".to_owned(),
        }
    }

    /// A git repository under a fresh temp directory; the directory is returned
    /// so it lives as long as the test.
    fn repo() -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("proj");
        fs::create_dir(&root).expect("mkdir");
        let status = Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["init", "-q"])
            .status()
            .expect("git init");
        assert!(status.success());
        let canonical = fs::canonicalize(&root).expect("canonicalize");
        (dir, canonical.to_string_lossy().into_owned())
    }

    fn scope_ws(opts: &Options, ws: &str) -> Result<String, String> {
        let mut workspace = ws.to_owned();
        scope(opts, &mut workspace, None, None).map(|()| workspace)
    }

    #[test]
    fn blank_workspace_without_a_default_is_required() {
        let err = scope_ws(&opts("", "", ""), "   ").unwrap_err();
        assert_eq!(
            err,
            "workspace is required: pass the absolute path of your project directory"
        );
    }

    #[test]
    fn relative_workspace_is_refused() {
        let err = scope_ws(&opts("", "", ""), "proj").unwrap_err();
        assert_eq!(err, "workspace 'proj' is not an absolute path");
    }

    #[test]
    fn missing_directory_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("nope");
        let text = missing.to_string_lossy().into_owned();
        let err = scope_ws(&opts("", "", ""), &text).unwrap_err();
        assert_eq!(err, format!("workspace '{text}' is not a directory"));
    }

    #[test]
    fn directory_outside_git_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let canonical = fs::canonicalize(dir.path()).expect("canonicalize");
        let text = canonical.to_string_lossy().into_owned();
        let err = scope_ws(&opts("", "", ""), &text).unwrap_err();
        assert_eq!(
            err,
            format!("workspace {text} is not inside a git repository")
        );
    }

    #[test]
    fn a_subdirectory_resolves_to_the_git_root() {
        let (_guard, root) = repo();
        let sub = Path::new(&root).join("src");
        fs::create_dir(&sub).expect("mkdir");
        let resolved = scope_ws(&opts("", "", ""), &sub.to_string_lossy()).expect("scope");
        assert_eq!(resolved, root);
    }

    #[test]
    fn an_empty_workspace_takes_the_server_default() {
        let (_guard, root) = repo();
        let resolved = scope_ws(&opts(&root, "", ""), "  ").expect("scope");
        assert_eq!(resolved, root);
    }

    #[test]
    fn blank_lane_and_author_take_the_defaults_and_present_ones_are_trimmed() {
        let (_guard, root) = repo();
        let o = opts(&root, "auth", "claude");

        let (mut lane, mut author) = (" \t".to_owned(), String::new());
        scope(&o, &mut root.clone(), Some(&mut lane), Some(&mut author)).expect("scope");
        assert_eq!((lane.as_str(), author.as_str()), ("auth", "claude"));

        let (mut lane, mut author) = (" api ".to_owned(), " bob ".to_owned());
        scope(&o, &mut root.clone(), Some(&mut lane), Some(&mut author)).expect("scope");
        assert_eq!((lane.as_str(), author.as_str()), ("api", "bob"));
    }

    #[test]
    fn a_tool_without_lane_or_author_leaves_them_alone() {
        let (_guard, root) = repo();
        let mut workspace = root.clone();
        let o = opts(&root, "auth", "claude");
        scope(&o, &mut workspace, None, None).expect("scope");
        assert_eq!(workspace, root);
    }
}
