//! The projects a lane host shows: the project files read from the config
//! directory, plus an implicit project for a repository no file lists. Each
//! project's lanes are its file's `[[lane]]`s, found or still to create as
//! worktrees under the data directory, then the worktrees the file does not
//! name.

use std::path::{Path, PathBuf};

use rvw_git::worktree::{self, Worktree};
use rvw_project::Project;

/// One sidebar row of a project, before facts.
#[derive(Debug, Clone)]
pub struct LaneEntry {
    pub name: String,
    pub branch: Option<String>,
    /// The worktree's directory; `None` when it is still to create.
    pub path: Option<PathBuf>,
    /// Where the worktree goes (`path` once created).
    pub target: PathBuf,
    /// The repository to create it in.
    pub repo: PathBuf,
    pub main: bool,
    /// Names of the project's `[terminals]` to start on first open.
    pub terminals: Vec<String>,
}

pub struct HostProject {
    pub project: Project,
    /// The rvw data directory (beside the database): lanes live under it.
    data_dir: PathBuf,
    /// Each repository's worktrees, by repository name.
    trees: Vec<(String, PathBuf, Vec<Worktree>)>,
}

impl HostProject {
    fn new(project: Project, data_dir: &Path) -> Self {
        let mut host = HostProject {
            project,
            data_dir: data_dir.to_path_buf(),
            trees: Vec::new(),
        };
        host.refresh();
        host
    }

    /// Lists every repository's worktrees again.
    pub fn refresh(&mut self) {
        self.trees = self
            .project
            .repos
            .iter()
            .map(|repo| {
                let list = worktree::list(&repo.path).unwrap_or_default();
                (repo.name.clone(), repo.path.clone(), list)
            })
            .collect();
    }

    /// Whether `workspace` is a worktree of one of this project's repositories.
    pub fn holds(&self, workspace: &Path) -> bool {
        self.trees
            .iter()
            .any(|(_, _, list)| list.iter().any(|tree| same_path(&tree.path, workspace)))
    }

    /// The rows: file lanes in file order, then the other worktrees.
    pub fn entries(&self) -> Vec<LaneEntry> {
        let mut rows: Vec<LaneEntry> = Vec::new();
        let mut taken: Vec<PathBuf> = Vec::new();
        for lane in &self.project.lanes {
            let Some((_, repo, list)) = self.trees.iter().find(|(name, _, _)| *name == lane.repo)
            else {
                continue;
            };
            let target = rvw_git::lanes::lane_path(
                &self.data_dir,
                &self.project.name,
                &lane.repo,
                &lane.name,
            );
            let found = list.iter().find(|tree| same_path(&tree.path, &target));
            if let Some(tree) = found {
                taken.push(tree.path.clone());
            }
            rows.push(LaneEntry {
                name: lane.name.clone(),
                branch: found.and_then(|tree| tree.branch.clone()),
                path: found.map(|tree| tree.path.clone()),
                target,
                repo: repo.clone(),
                main: false,
                terminals: lane.terminals.clone(),
            });
        }
        for (_, repo, list) in &self.trees {
            for tree in list {
                if taken.iter().any(|path| same_path(path, &tree.path)) || tree.prunable {
                    continue;
                }
                rows.push(LaneEntry {
                    name: lane_name(tree),
                    branch: tree.branch.clone(),
                    path: Some(tree.path.clone()),
                    target: tree.path.clone(),
                    repo: repo.clone(),
                    main: tree.main,
                    terminals: Vec::new(),
                });
            }
        }
        rows
    }

    /// The command line of the project's terminal `name`.
    pub fn terminal_command(&self, name: &str) -> Option<&str> {
        self.project.terminals.get(name).map(String::as_str)
    }
}

/// What a worktree is called in the sidebar: its branch, or its directory's
/// name when detached.
pub fn lane_name(tree: &Worktree) -> String {
    tree.branch.clone().unwrap_or_else(|| {
        tree.path.file_name().map_or_else(
            || "detached".to_owned(),
            |n| n.to_string_lossy().into_owned(),
        )
    })
}

fn same_path(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// The projects to show for `workspace`: every valid project file in
/// `config_dir`, and an implicit project of the workspace's repository when
/// no file lists it. The notices are the refusals of the files that did not
/// open, one line each.
pub fn load(
    config_dir: &Path,
    data_dir: &Path,
    home: &Path,
    workspace: &Path,
) -> (Vec<HostProject>, Vec<String>) {
    let mut projects = Vec::new();
    let mut notices = Vec::new();
    let mut files: Vec<PathBuf> = std::fs::read_dir(config_dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
        .collect();
    files.sort();
    for file in files {
        let text = match std::fs::read_to_string(&file) {
            Ok(text) => text,
            Err(error) => {
                notices.push(format!("{}: {error}", file.display()));
                continue;
            }
        };
        match rvw_project::parse(&text, Some(&file), home) {
            Ok(project) => projects.push(HostProject::new(project, data_dir)),
            Err(error) => notices.push(error.to_string()),
        }
    }
    if !projects.iter().any(|project| project.holds(workspace)) {
        let main = worktree::list(workspace)
            .ok()
            .and_then(|list| list.into_iter().find(|tree| tree.main))
            .map_or_else(|| workspace.to_path_buf(), |tree| tree.path);
        let name = main.file_name().map_or_else(
            || "repository".to_owned(),
            |n| n.to_string_lossy().into_owned(),
        );
        projects.push(HostProject::new(
            rvw_project::implicit(&name, &main),
            data_dir,
        ));
    }
    (projects, notices)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::process::Command;

    use super::load;

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .current_dir(dir)
            .status()
            .unwrap();
        assert!(status.success(), "{args:?}");
    }

    #[test]
    fn a_file_lane_without_a_worktree_is_listed_to_create_and_a_bad_file_is_a_notice() {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("api");
        fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["commit", "-q", "--allow-empty", "-m", "first"]);
        let config = root.path().join("projects");
        fs::create_dir(&config).unwrap();
        fs::write(
            config.join("pay.toml"),
            format!(
                "name = \"pay\"\n[repos.api]\npath = \"{}\"\n[terminals]\nrepl = \"echo hi\"\n[[lane]]\nname = \"likes\"\nrepo = \"api\"\nterminals = [\"repl\"]\n",
                repo.display()
            ),
        )
        .unwrap();
        fs::write(config.join("bad.toml"), "nope = 1\n").unwrap();
        let data = root.path().join("data");

        let (projects, notices) = load(&config, &data, root.path(), &repo);
        assert_eq!(projects.len(), 1, "the file lists the workspace's repo");
        assert_eq!(notices.len(), 1);
        assert!(notices[0].contains("bad.toml"), "{notices:?}");
        let rows = projects[0].entries();
        let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
        assert_eq!(names, ["likes", "main"]);
        assert!(rows[0].path.is_none());
        assert_eq!(rows[0].terminals, ["repl"]);
        assert!(rows[1].main);
        assert_eq!(projects[0].terminal_command("repl"), Some("echo hi"));
    }

    #[test]
    fn a_repository_no_file_lists_gets_an_implicit_project() {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("solo");
        fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["commit", "-q", "--allow-empty", "-m", "first"]);
        let (projects, notices) = load(
            &root.path().join("none"),
            &root.path().join("data"),
            root.path(),
            &repo,
        );
        assert!(notices.is_empty());
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].project.name, "solo");
        assert_eq!(projects[0].entries().len(), 1);
    }
}
