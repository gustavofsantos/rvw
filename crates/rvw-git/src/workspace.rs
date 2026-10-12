//! Port of internal/workspace/workspace.go.

use std::io;
use std::path::{Component, Path, PathBuf};

use crate::gitx;

/// The canonical workspace for `dir`: the root of the worktree holding it,
/// with symlinks resolved. A dir outside git is an error whose message is
/// `<dir> is not inside a git repository`.
pub fn resolve(dir: &Path) -> Result<PathBuf, String> {
    let base = canonical(dir).map_err(|e| e.to_string())?;
    let top = gitx::toplevel(&base)
        .ok_or_else(|| format!("{} is not inside a git repository", base.display()))?;
    canonical(&top).map_err(|e| e.to_string())
}

/// Whether `path` names an existing directory.
pub fn is_dir(path: &Path) -> bool {
    path.is_dir()
}

/// Makes `path` absolute and resolves symlinks, keeping any trailing part that
/// does not exist yet as written. Any other failure is an error.
pub fn canonical(path: &Path) -> io::Result<PathBuf> {
    let abs = clean(&if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    });
    let mut missing: Vec<std::ffi::OsString> = Vec::new();
    let mut cur = abs.clone();
    loop {
        match std::fs::canonicalize(&cur) {
            Ok(mut resolved) => {
                for m in missing.iter().rev() {
                    resolved.push(m);
                }
                return Ok(resolved);
            }
            Err(e)
                if e.kind() == io::ErrorKind::NotFound
                    || e.kind() == io::ErrorKind::NotADirectory =>
            {
                let (Some(parent), Some(name)) = (cur.parent(), cur.file_name()) else {
                    return Ok(abs);
                };
                missing.push(name.to_os_string());
                cur = parent.to_path_buf();
            }
            Err(e) => {
                return Err(io::Error::new(
                    e.kind(),
                    format!("cannot resolve {}: {e}", abs.display()),
                ));
            }
        }
    }
}

/// `path` relative to workspace `ws`, if `path` lies inside it (filepath.Rel
/// plus filepath.IsLocal). A path equal to `ws` is `"."`; the result uses
/// `/` separators.
pub fn in_workspace(ws: &Path, path: &Path) -> Option<String> {
    if ws.is_absolute() != path.is_absolute() {
        return None;
    }
    let ws = clean(ws);
    let path = clean(path);
    let rel = path.strip_prefix(&ws).ok()?;
    let mut parts = Vec::new();
    for c in rel.components() {
        match c {
            Component::Normal(name) => parts.push(name.to_string_lossy().into_owned()),
            Component::CurDir => {}
            _ => return None, // `..`, a root or a prefix: not local
        }
    }
    if parts.is_empty() {
        return Some(".".to_owned());
    }
    Some(parts.join("/"))
}

/// Lexical cleanup as filepath.Clean: drops `.`, folds `..` into the
/// preceding element, and never climbs above a root. An empty result is `.`.
fn clean(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::Prefix(_) | Component::RootDir => out.push(c.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => match out.components().next_back() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                Some(Component::RootDir | Component::Prefix(_)) => {}
                _ => out.push(".."),
            },
            Component::Normal(name) => out.push(name),
        }
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;
    use tempfile::TempDir;

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}");
    }

    #[test]
    fn resolve_finds_worktree_root_from_subdirectory() {
        let r = TempDir::new().unwrap();
        git(r.path(), &["init", "-q", "-b", "main"]);
        let sub = r.path().join("a").join("b");
        fs::create_dir_all(&sub).unwrap();

        let want = fs::canonicalize(r.path()).unwrap();
        assert_eq!(resolve(&sub).unwrap(), want);
    }

    #[test]
    fn resolve_outside_git_is_an_error() {
        let outside = TempDir::new().unwrap();
        let err = resolve(outside.path()).unwrap_err();
        let want = fs::canonicalize(outside.path()).unwrap();
        assert_eq!(
            err,
            format!("{} is not inside a git repository", want.display())
        );
    }

    #[test]
    fn is_dir_distinguishes_dirs_files_and_missing() {
        let t = TempDir::new().unwrap();
        let f = t.path().join("f");
        fs::write(&f, "x").unwrap();
        assert!(is_dir(t.path()));
        assert!(!is_dir(&f));
        assert!(!is_dir(&t.path().join("missing")));
    }

    #[test]
    fn canonical_resolves_symlinks_and_keeps_missing_tail() {
        let t = TempDir::new().unwrap();
        let real = t.path().join("real");
        fs::create_dir(&real).unwrap();
        let link = t.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let real = fs::canonicalize(&real).unwrap();
        assert_eq!(canonical(&link).unwrap(), real);
        assert_eq!(
            canonical(&link.join("missing").join("tail.txt")).unwrap(),
            real.join("missing").join("tail.txt")
        );
    }

    #[test]
    fn canonical_cleans_dot_dot_lexically_before_resolving() {
        let t = TempDir::new().unwrap();
        let real = fs::canonicalize(t.path()).unwrap();
        let spelled = t.path().join("nope").join("..").join("x");
        assert_eq!(canonical(&spelled).unwrap(), real.join("x"));
    }

    #[test]
    fn in_workspace_relative_slash_paths() {
        let ws = Path::new("/a/b");
        assert_eq!(
            in_workspace(ws, Path::new("/a/b/c/d.txt")).as_deref(),
            Some("c/d.txt")
        );
        assert_eq!(in_workspace(ws, Path::new("/a/b")).as_deref(), Some("."));
        assert_eq!(
            in_workspace(ws, Path::new("/a/b/../b/c")).as_deref(),
            Some("c")
        );
    }

    #[test]
    fn in_workspace_rejects_outside_and_sibling_prefixes() {
        let ws = Path::new("/a/b");
        assert_eq!(in_workspace(ws, Path::new("/a/bc")), None);
        assert_eq!(in_workspace(ws, Path::new("/a/x")), None);
        assert_eq!(in_workspace(ws, Path::new("/a/b/../x")), None);
        assert_eq!(in_workspace(ws, Path::new("c/d")), None);
    }
}
