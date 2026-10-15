//! Reading a project file. Strict: every refusal is one [`Error`] naming the
//! file and the key, and the project does not open.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use toml::Value;

use crate::{Commands, Error, Lane, Project, Repo};

/// The file as written. Sections are read as tables and checked one by one, so
/// an error can name the entry it is in (`repos.api.path`, `lane[1].repo`).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    name: String,
    #[serde(default)]
    repos: toml::Table,
    #[serde(default)]
    terminals: toml::Table,
    #[serde(default)]
    commands: toml::Table,
    #[serde(default)]
    hooks: toml::Table,
    #[serde(default)]
    lane: Vec<toml::Table>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRepo {
    path: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCommands {
    run: Option<String>,
    push: Option<String>,
    pr: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLane {
    name: String,
    repo: String,
    after: Option<String>,
    #[serde(default)]
    terminals: Vec<String>,
}

/// The directory holding project files: `config` as given, else
/// `xdg_config_home/rvw/projects` when that is absolute (XDG ignores a relative
/// one), else `home/.config/rvw/projects`.
pub(crate) fn projects_dir(
    config: Option<&Path>,
    xdg_config_home: Option<&Path>,
    home: &Path,
) -> PathBuf {
    if let Some(dir) = config {
        return dir.to_path_buf();
    }
    let base = match xdg_config_home.filter(|dir| dir.is_absolute()) {
        Some(dir) => dir.to_path_buf(),
        None => home.join(".config"),
    };
    base.join("rvw").join("projects")
}

/// Parses `text` (read from `file`). See [`crate::parse`].
pub(crate) fn parse(text: &str, file: Option<&Path>, home: &Path) -> Result<Project, Error> {
    let raw: File = toml::from_str(text).map_err(|e| from_de(file, "", &e))?;

    let mut repos = Vec::new();
    for (name, value) in raw.repos {
        let prefix = format!("repos.{name}");
        let repo: RawRepo = value.try_into().map_err(|e| from_de(file, &prefix, &e))?;
        let path = expand(&repo.path, home).ok_or_else(|| {
            fail(
                file,
                format!("{prefix}.path"),
                "must be an absolute path or start with ~/",
            )
        })?;
        if !path.is_dir() {
            return Err(fail(
                file,
                format!("{prefix}.path"),
                format!("{} is not an existing directory", path.display()),
            ));
        }
        repos.push(Repo { name, path });
    }

    let mut terminals = BTreeMap::new();
    for (name, value) in raw.terminals {
        let key = format!("terminals.{name}");
        terminals.insert(name, string_value(file, &key, value)?);
    }

    let mut hooks = BTreeMap::new();
    for (name, value) in raw.hooks {
        let key = format!("hooks.{name}");
        hooks.insert(name, string_value(file, &key, value)?);
    }

    let commands: RawCommands = Value::Table(raw.commands)
        .try_into()
        .map_err(|e| from_de(file, "commands", &e))?;

    let mut lanes: Vec<Lane> = Vec::new();
    for (index, table) in raw.lane.into_iter().enumerate() {
        let prefix = format!("lane[{index}]");
        let lane: RawLane = Value::Table(table)
            .try_into()
            .map_err(|e| from_de(file, &prefix, &e))?;
        if !repos.iter().any(|repo| repo.name == lane.repo) {
            return Err(fail(
                file,
                format!("{prefix}.repo"),
                format!("{} is not a key of [repos]", lane.repo),
            ));
        }
        if let Some(first) = lanes.iter().position(|other| other.name == lane.name) {
            return Err(fail(
                file,
                format!("{prefix}.name"),
                format!("{} is already the name of lane[{first}]", lane.name),
            ));
        }
        if let Some(missing) = lane
            .terminals
            .iter()
            .find(|name| !terminals.contains_key(*name))
        {
            return Err(fail(
                file,
                format!("{prefix}.terminals"),
                format!("{missing} is not a key of [terminals]"),
            ));
        }
        lanes.push(Lane {
            name: lane.name,
            repo: lane.repo,
            after: lane.after,
            terminals: lane.terminals,
        });
    }

    check_after(file, &lanes)?;

    Ok(Project {
        name: raw.name,
        file: file.map(Path::to_path_buf),
        repos,
        terminals,
        commands: Commands {
            run: commands.run,
            push: commands.push,
            pr: commands.pr,
        },
        hooks,
        lanes,
    })
}

/// Every `after` names a lane, and no lane reaches itself through `after`.
fn check_after(file: Option<&Path>, lanes: &[Lane]) -> Result<(), Error> {
    let index_of = |name: &str| lanes.iter().position(|lane| lane.name == name);
    for (index, lane) in lanes.iter().enumerate() {
        if let Some(after) = lane
            .after
            .as_ref()
            .filter(|after| index_of(after).is_none())
        {
            return Err(fail(
                file,
                format!("lane[{index}].after"),
                format!("{after} is not the name of a lane of this project"),
            ));
        }
    }
    // Each lane has at most one `after`, so a cycle is a chain that returns to
    // its start. The first lane in file order on a cycle is the one reported.
    for start in 0..lanes.len() {
        let mut chain = vec![lanes[start].name.as_str()];
        let mut current = start;
        for _ in 0..lanes.len() {
            let Some(next) = lanes[current].after.as_deref().and_then(index_of) else {
                break;
            };
            chain.push(lanes[next].name.as_str());
            if next == start {
                return Err(fail(
                    file,
                    format!("lane[{start}].after"),
                    format!("the stack closes a cycle: {}", chain.join(" -> ")),
                ));
            }
            current = next;
        }
    }
    Ok(())
}

/// `~` and `~/...` expand to `home`; any other path must be absolute.
fn expand(raw: &str, home: &Path) -> Option<PathBuf> {
    if raw == "~" {
        return Some(home.to_path_buf());
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        return Some(home.join(rest));
    }
    let path = PathBuf::from(raw);
    path.is_absolute().then_some(path)
}

fn string_value(file: Option<&Path>, key: &str, value: Value) -> Result<String, Error> {
    match value {
        Value::String(text) => Ok(text),
        _ => Err(fail(file, key.to_owned(), "must be a string")),
    }
}

/// Turns a serde error into an [`Error`]. An unknown or missing field is
/// reported under its own key; any other error under `prefix`.
fn from_de(file: Option<&Path>, prefix: &str, error: &toml::de::Error) -> Error {
    let message = error.message();
    let field = ["unknown field `", "missing field `"]
        .iter()
        .find_map(|marker| message.strip_prefix(marker))
        .and_then(|rest| rest.split('`').next());
    let key = match field {
        Some(field) if prefix.is_empty() => field.to_owned(),
        Some(field) => format!("{prefix}.{field}"),
        None => prefix.to_owned(),
    };
    fail(file, key, message)
}

fn fail(file: Option<&Path>, key: String, message: impl Into<String>) -> Error {
    Error {
        file: file.map(Path::to_path_buf),
        key,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::{expand, parse, projects_dir};
    use crate::{Error, Project};

    /// A home directory holding `work/<name>` for each repository name.
    fn home_with(repos: &[&str]) -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        for repo in repos {
            fs::create_dir_all(home.path().join("work").join(repo)).unwrap();
        }
        home
    }

    fn read(text: &str, home: &Path) -> Result<Project, Error> {
        parse(text, Some(Path::new("/projects/payments.toml")), home)
    }

    fn refusal(text: &str, home: &Path) -> Error {
        read(text, home).expect_err("the project should be refused")
    }

    const PLAN: &str = r#"
name = "payments-platform"

[repos.proto]
path = "~/work/proto"
[repos.api]
path = "~/work/api"

[terminals]
claude = "claude"
repl   = "cargo watch -x test"

[commands]
run  = "make run"
push = "git push -u origin HEAD"
pr   = "gh pr create --fill"

[hooks]
stop = "notify-send done"

[[lane]]
name  = "like-schema"
repo  = "api"

[[lane]]
name  = "likes-endpoint"
repo  = "api"
after = "like-schema"
terminals = ["claude", "repl"]

[[lane]]
name = "proto-notes"
repo = "proto"
"#;

    #[test]
    fn reads_the_plan_example() {
        let home = home_with(&["proto", "api"]);
        let project = read(PLAN, home.path()).unwrap();

        assert_eq!(project.name, "payments-platform");
        assert_eq!(
            project.file.as_deref(),
            Some(Path::new("/projects/payments.toml"))
        );
        let repos: Vec<(&str, &Path)> = project
            .repos
            .iter()
            .map(|repo| (repo.name.as_str(), repo.path.as_path()))
            .collect();
        assert_eq!(
            repos,
            [
                ("proto", home.path().join("work/proto").as_path()),
                ("api", home.path().join("work/api").as_path()),
            ]
        );
        assert_eq!(project.terminals["claude"], "claude");
        assert_eq!(project.terminals["repl"], "cargo watch -x test");
        assert_eq!(project.commands.run.as_deref(), Some("make run"));
        assert_eq!(
            project.commands.push.as_deref(),
            Some("git push -u origin HEAD")
        );
        assert_eq!(project.commands.pr.as_deref(), Some("gh pr create --fill"));
        assert_eq!(project.hooks["stop"], "notify-send done");

        let lanes: Vec<&str> = project
            .lanes
            .iter()
            .map(|lane| lane.name.as_str())
            .collect();
        assert_eq!(lanes, ["like-schema", "likes-endpoint", "proto-notes"]);
        let stacked = &project.lanes[1];
        assert_eq!(stacked.repo, "api");
        assert_eq!(stacked.after.as_deref(), Some("like-schema"));
        assert_eq!(stacked.terminals, ["claude", "repl"]);
        assert_eq!(project.lanes[0].after, None);
        assert!(project.lanes[0].terminals.is_empty());
    }

    #[test]
    fn minimal_project_has_no_lanes_or_sections() {
        let home = home_with(&["api"]);
        let project = read(
            "name = \"tiny\"\n[repos.api]\npath = \"~/work/api\"\n",
            home.path(),
        )
        .unwrap();
        assert!(project.lanes.is_empty());
        assert!(project.terminals.is_empty());
        assert!(project.hooks.is_empty());
        assert_eq!(project.commands, crate::Commands::default());
    }

    #[test]
    fn expands_tilde_alone_and_tilde_slash() {
        let home = Path::new("/home/someone");
        assert_eq!(expand("~", home).unwrap(), home);
        assert_eq!(expand("~/code/api", home).unwrap(), home.join("code/api"));
        assert_eq!(expand("/srv/api", home).unwrap(), Path::new("/srv/api"));
        assert_eq!(expand("~user/api", home), None);
        assert_eq!(expand("code/api", home), None);
    }

    #[test]
    fn unknown_top_level_key_is_named() {
        let home = home_with(&["api"]);
        let error = refusal("name = \"x\"\ncolour = \"red\"\n", home.path());
        assert_eq!(error.key, "colour");
        assert!(error.message.starts_with("unknown field `colour`"));
    }

    #[test]
    fn unknown_key_in_repo_lane_and_commands_is_named() {
        let home = home_with(&["api"]);
        let repo = refusal(
            "name = \"x\"\n[repos.api]\npath = \"~/work/api\"\nbranch = \"main\"\n",
            home.path(),
        );
        assert_eq!(repo.key, "repos.api.branch");

        let lane = refusal(
            "name = \"x\"\n[repos.api]\npath = \"~/work/api\"\n[[lane]]\nname = \"a\"\nrepo = \"api\"\nbogus = 1\n",
            home.path(),
        );
        assert_eq!(lane.key, "lane[0].bogus");

        let commands = refusal("name = \"x\"\n[commands]\nbuild = \"make\"\n", home.path());
        assert_eq!(commands.key, "commands.build");
    }

    #[test]
    fn missing_repo_path_is_named() {
        let home = home_with(&[]);
        let error = refusal("name = \"x\"\n[repos.api]\n", home.path());
        assert_eq!(error.key, "repos.api.path");
        assert!(error.message.starts_with("missing field `path`"));
    }

    #[test]
    fn repo_path_must_be_an_existing_directory() {
        let home = home_with(&["api"]);
        fs::write(home.path().join("work/a-file"), "").unwrap();

        let missing = refusal(
            "name = \"x\"\n[repos.api]\npath = \"~/work/missing\"\n",
            home.path(),
        );
        assert_eq!(missing.key, "repos.api.path");
        assert!(missing.message.contains("is not an existing directory"));

        let file = refusal(
            "name = \"x\"\n[repos.api]\npath = \"~/work/a-file\"\n",
            home.path(),
        );
        assert_eq!(file.key, "repos.api.path");
    }

    #[test]
    fn repo_path_must_be_absolute_or_home_relative() {
        let home = home_with(&["api"]);
        let error = refusal(
            "name = \"x\"\n[repos.api]\npath = \"work/api\"\n",
            home.path(),
        );
        assert_eq!(error.key, "repos.api.path");
    }

    #[test]
    fn lane_repo_must_be_a_key_of_repos() {
        let home = home_with(&["api"]);
        let error = refusal(
            "name = \"x\"\n[repos.api]\npath = \"~/work/api\"\n[[lane]]\nname = \"a\"\nrepo = \"web\"\n",
            home.path(),
        );
        assert_eq!(error.key, "lane[0].repo");
        assert!(error.message.contains("web"));
    }

    #[test]
    fn lane_without_repo_is_named() {
        let home = home_with(&["api"]);
        let error = refusal(
            "name = \"x\"\n[repos.api]\npath = \"~/work/api\"\n[[lane]]\nname = \"a\"\n",
            home.path(),
        );
        assert_eq!(error.key, "lane[0].repo");
    }

    #[test]
    fn duplicate_lane_names_are_refused() {
        let home = home_with(&["api"]);
        let error = refusal(
            "name = \"x\"\n[repos.api]\npath = \"~/work/api\"\n\
             [[lane]]\nname = \"a\"\nrepo = \"api\"\n\
             [[lane]]\nname = \"b\"\nrepo = \"api\"\n\
             [[lane]]\nname = \"a\"\nrepo = \"api\"\n",
            home.path(),
        );
        assert_eq!(error.key, "lane[2].name");
        assert_eq!(error.message, "a is already the name of lane[0]");
    }

    #[test]
    fn after_must_name_a_lane() {
        let home = home_with(&["api"]);
        let error = refusal(
            "name = \"x\"\n[repos.api]\npath = \"~/work/api\"\n\
             [[lane]]\nname = \"a\"\nrepo = \"api\"\nafter = \"nope\"\n",
            home.path(),
        );
        assert_eq!(error.key, "lane[0].after");
        assert!(error.message.contains("nope"));
    }

    #[test]
    fn after_must_not_form_a_cycle() {
        let home = home_with(&["api"]);
        let two = refusal(
            "name = \"x\"\n[repos.api]\npath = \"~/work/api\"\n\
             [[lane]]\nname = \"a\"\nrepo = \"api\"\nafter = \"b\"\n\
             [[lane]]\nname = \"b\"\nrepo = \"api\"\nafter = \"a\"\n",
            home.path(),
        );
        assert_eq!(two.key, "lane[0].after");
        assert_eq!(two.message, "the stack closes a cycle: a -> b -> a");

        let itself = refusal(
            "name = \"x\"\n[repos.api]\npath = \"~/work/api\"\n\
             [[lane]]\nname = \"a\"\nrepo = \"api\"\nafter = \"a\"\n",
            home.path(),
        );
        assert_eq!(itself.key, "lane[0].after");
    }

    #[test]
    fn a_chain_into_a_cycle_names_the_cycle_lane() {
        let home = home_with(&["api"]);
        // lane[0] leads into the cycle b <-> c but is not on it.
        let error = refusal(
            "name = \"x\"\n[repos.api]\npath = \"~/work/api\"\n\
             [[lane]]\nname = \"a\"\nrepo = \"api\"\nafter = \"b\"\n\
             [[lane]]\nname = \"b\"\nrepo = \"api\"\nafter = \"c\"\n\
             [[lane]]\nname = \"c\"\nrepo = \"api\"\nafter = \"b\"\n",
            home.path(),
        );
        assert_eq!(error.key, "lane[1].after");
    }

    #[test]
    fn a_lane_terminal_must_be_a_key_of_terminals() {
        let home = home_with(&["api"]);
        let error = refusal(
            "name = \"x\"\n[terminals]\nclaude = \"claude\"\n[repos.api]\npath = \"~/work/api\"\n\
             [[lane]]\nname = \"a\"\nrepo = \"api\"\nterminals = [\"claude\", \"repl\"]\n",
            home.path(),
        );
        assert_eq!(error.key, "lane[0].terminals");
        assert!(error.message.contains("repl"));
    }

    #[test]
    fn terminals_and_hooks_must_be_strings() {
        let home = home_with(&["api"]);
        let terminal = refusal("name = \"x\"\n[terminals]\nclaude = 1\n", home.path());
        assert_eq!(terminal.key, "terminals.claude");

        let hook = refusal("name = \"x\"\n[hooks]\nstop = [\"a\"]\n", home.path());
        assert_eq!(hook.key, "hooks.stop");
    }

    #[test]
    fn refusal_names_the_file_and_key_on_one_line() {
        let home = home_with(&["api"]);
        let error = refusal(
            "name = \"x\"\n[repos.api]\npath = \"~/work/api\"\n[[lane]]\nname = \"a\"\nrepo = \"web\"\n",
            home.path(),
        );
        assert_eq!(
            error.to_string(),
            "/projects/payments.toml: lane[0].repo: web is not a key of [repos]"
        );
    }

    #[test]
    fn projects_dir_prefers_config_then_xdg_then_home() {
        let home = Path::new("/home/me");
        assert_eq!(
            projects_dir(Some(Path::new("/elsewhere")), Some(Path::new("/xdg")), home),
            Path::new("/elsewhere")
        );
        assert_eq!(
            projects_dir(None, Some(Path::new("/xdg")), home),
            Path::new("/xdg/rvw/projects")
        );
        assert_eq!(
            projects_dir(None, Some(Path::new("relative/xdg")), home),
            Path::new("/home/me/.config/rvw/projects")
        );
        assert_eq!(
            projects_dir(None, None, home),
            Path::new("/home/me/.config/rvw/projects")
        );
    }
}
