//! Projects: the TOML files that group repositories, lanes, terminals and
//! commands for the GUI. A project is not a workspace; each lane is one.
//! See `docs/gui-m7-plan.md`. No git and no database here: reading text,
//! writing text, and naming the directory the files live in.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

/// One repository of a project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    /// Its key in `[repos.<name>]`.
    pub name: String,
    /// Absolute, with `~` expanded.
    pub path: PathBuf,
}

/// The project's `[commands]`; each is run in a new terminal tab of a lane.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Commands {
    pub run: Option<String>,
    pub push: Option<String>,
    pub pr: Option<String>,
}

/// One `[[lane]]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lane {
    pub name: String,
    /// A key of `repos`.
    pub repo: String,
    /// The lane this one stacks on. Read in M7, acted on in M8.
    pub after: Option<String>,
    /// Names of entries of `Project::terminals`, started when the lane opens.
    pub terminals: Vec<String>,
}

/// A project as the file says it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Project {
    pub name: String,
    /// The file it was read from; `None` for an implicit project.
    pub file: Option<PathBuf>,
    pub repos: Vec<Repo>,
    /// `[terminals]`: name to command, in the file's order of names sorted.
    pub terminals: BTreeMap<String, String>,
    pub commands: Commands,
    /// `[hooks]`: read and kept in M7, nothing runs them until M8.
    pub hooks: BTreeMap<String, String>,
    /// In the file's order.
    pub lanes: Vec<Lane>,
}

/// A refusal: one line naming the file and the key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub file: Option<PathBuf>,
    /// The offending key, e.g. `lane[1].repo`; empty when the file as a whole.
    pub key: String,
    pub message: String,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(file) = &self.file {
            write!(f, "{}: ", file.display())?;
        }
        if !self.key.is_empty() {
            write!(f, "{}: ", self.key)?;
        }
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

impl Error {
    fn unimplemented(what: &str) -> Self {
        Error {
            file: None,
            key: String::new(),
            message: format!("{what} is not built yet"),
        }
    }
}

/// The directory holding project files: `config` when given (`--config`),
/// else `$XDG_CONFIG_HOME/rvw/projects`, else `<home>/.config/rvw/projects`.
/// `xdg_config_home` and `home` are passed in; this crate reads no
/// environment.
pub fn projects_dir(config: Option<&Path>, xdg_config_home: Option<&Path>, home: &Path) -> PathBuf {
    let _ = (config, xdg_config_home, home);
    PathBuf::new()
}

/// Parses `text` (read from `file`). `~` in a repo path expands to `home`.
/// Strict: an unknown key, a lane whose `repo` is not listed, two lanes of one
/// name, a repo path that is not an existing directory, or an `after` that
/// names no lane or closes a cycle is an [`Error`].
pub fn parse(text: &str, file: Option<&Path>, home: &Path) -> Result<Project, Error> {
    let _ = (text, file, home);
    Err(Error::unimplemented("reading a project"))
}

/// The implicit project of one repository: no file, no lanes written.
pub fn implicit(name: &str, repo: &Path) -> Project {
    Project {
        name: name.to_owned(),
        repos: vec![Repo {
            name: name.to_owned(),
            path: repo.to_path_buf(),
        }],
        ..Project::default()
    }
}

/// `text` with `lane` appended as a `[[lane]]`, the rest untouched (comments
/// and order kept).
pub fn add_lane(text: &str, lane: &Lane) -> Result<String, Error> {
    let _ = (text, lane);
    Err(Error::unimplemented("adding a lane"))
}

/// `text` without the `[[lane]]` named `name`, the rest untouched.
pub fn remove_lane(text: &str, name: &str) -> Result<String, Error> {
    let _ = (text, name);
    Err(Error::unimplemented("removing a lane"))
}
