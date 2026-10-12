//! Port of internal/workspace/workspace.go.

use std::io;
use std::path::{Path, PathBuf};

/// The canonical workspace for `dir`: the root of the worktree holding it,
/// with symlinks resolved. A dir outside git is an error whose message is
/// `<dir> is not inside a git repository`.
pub fn resolve(_dir: &Path) -> Result<PathBuf, String> {
    todo!()
}

/// Whether `path` names an existing directory.
pub fn is_dir(_path: &Path) -> bool {
    todo!()
}

/// Makes `path` absolute and resolves symlinks, keeping any trailing part that
/// does not exist yet as written. Any other failure is an error.
pub fn canonical(_path: &Path) -> io::Result<PathBuf> {
    todo!()
}

/// `path` relative to workspace `ws`, if `path` lies inside it (filepath.Rel
/// plus filepath.IsLocal).
pub fn in_workspace(_ws: &Path, _path: &Path) -> Option<String> {
    todo!()
}
