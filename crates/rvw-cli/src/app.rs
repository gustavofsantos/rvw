//! Process-level context shared by every command: the flag-resolved workspace
//! and database, the lazily opened service, and the error mapping.

use std::cell::OnceCell;
use std::io::{IsTerminal, Read, Write};
use std::path::PathBuf;
use std::sync::Arc;

use clap::error::{ContextKind, ErrorKind};
use rvw_core::{Error as CoreError, Service};
use rvw_render::Terminal;
use rvw_store::Store;

use crate::{commands, PROG};

/// Every failure is the text after `rvw: `.
pub type CliResult<T> = Result<T, String>;

pub struct App {
    workspace_flag: String,
    db_flag: String,
    service: OnceCell<Arc<Service>>,
}

pub type Out = Terminal<std::io::Stdout>;

impl App {
    pub fn svc(&self) -> CliResult<&Arc<Service>> {
        if let Some(s) = self.service.get() {
            return Ok(s);
        }
        let path = self.db_path()?;
        let store = Store::open(&path).map_err(|e| e.message)?;
        let _ = self.service.set(Arc::new(Service::new(Box::new(store))));
        Ok(self.service.get().expect("service was just set"))
    }

    /// `--db`, else the XDG default.
    fn db_path(&self) -> CliResult<PathBuf> {
        if self.db_flag.is_empty() {
            return rvw_store::default_path().map_err(|e| e.message);
        }
        std::path::absolute(expand_home(&self.db_flag)).map_err(|e| e.to_string())
    }

    pub fn db_flag(&self) -> &str {
        &self.db_flag
    }

    pub fn workspace_flag(&self) -> &str {
        &self.workspace_flag
    }

    /// Which queue to act on: `--workspace`, else the current directory; then
    /// its git worktree root, which it must have.
    pub fn workspace(&self) -> CliResult<String> {
        let dir = if self.workspace_flag.is_empty() {
            std::env::current_dir().map_err(|e| e.to_string())?
        } else {
            let dir = PathBuf::from(expand_home(&self.workspace_flag));
            if !rvw_git::workspace::is_dir(&dir) {
                return Err(format!(
                    "--workspace '{}' is not a directory",
                    dir.display()
                ));
            }
            dir
        };
        rvw_git::workspace::resolve(&dir).map(|p| p.to_string_lossy().into_owned())
    }

    /// A notice on stderr: `rvw: <text>`.
    pub fn notice(text: &str) {
        let mut err = Terminal::new(std::io::stderr());
        let _ = writeln!(err, "{PROG}: {text}");
        let _ = err.finish();
    }

    /// Reads stdin unless it is an interactive terminal.
    pub fn read_stdin_if_piped() -> CliResult<Option<String>> {
        let stdin = std::io::stdin();
        if stdin.is_terminal() {
            return Ok(None);
        }
        let mut data = Vec::new();
        stdin
            .lock()
            .read_to_end(&mut data)
            .map_err(|e| e.to_string())?;
        Ok(Some(String::from_utf8_lossy(&data).into_owned()))
    }
}

pub fn core_err(e: CoreError) -> String {
    e.message
}

pub fn expand_home(path: &str) -> String {
    if path == "~" || path.starts_with("~/") {
        if let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()) {
            return format!("{}{}", home.to_string_lossy(), &path[1..]);
        }
    }
    path.to_owned()
}

/// A `--file` argument made absolute against the current directory.
pub fn file_path(file: &str) -> CliResult<String> {
    if file.is_empty() {
        return Ok(String::new());
    }
    std::path::absolute(expand_home(file))
        .map(|p| p.to_string_lossy().into_owned())
        .map_err(|e| e.to_string())
}

/// Who speaks: the flag, else the login user. Agents pass `--author`.
pub fn author_of(flag: &str) -> String {
    let user = std::env::var("USER").unwrap_or_default();
    let pick = if flag.is_empty() { user.as_str() } else { flag };
    pick.trim().to_owned()
}

pub fn lane_of(flag: &str) -> String {
    flag.trim().to_owned()
}

pub fn one_of(flag: &str, value: &str, allowed: &[&str]) -> CliResult<()> {
    if allowed.contains(&value) {
        Ok(())
    } else {
        Err(format!(
            "{flag} '{value}' is not one of {}",
            allowed.join(", ")
        ))
    }
}

/// Parses `args`, runs the command, and returns what failed.
pub fn dispatch(args: &[String], out: &mut Out) -> CliResult<()> {
    let root = commands::root();
    let matches = match root
        .try_get_matches_from(std::iter::once(PROG.to_owned()).chain(args.iter().cloned()))
    {
        Ok(m) => m,
        Err(e) => return Err(map_clap_error(&e, out)),
    };
    let app = App {
        workspace_flag: matches
            .get_one::<String>("workspace")
            .cloned()
            .unwrap_or_default(),
        db_flag: matches.get_one::<String>("db").cloned().unwrap_or_default(),
        service: OnceCell::new(),
    };
    commands::run(&app, &matches, out)
}

/// Maps clap's own errors to cobra's one-line wording. Help and version are
/// printed and are not failures; they surface here as an empty error handled
/// by the caller via `Ok`.
fn map_clap_error(e: &clap::Error, out: &mut Out) -> String {
    match e.kind() {
        ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => {
            let _ = write!(out, "{}", e.render());
            let _ = out.finish();
            let _ = std::io::stdout().flush();
            std::process::exit(0);
        }
        ErrorKind::InvalidSubcommand => {
            let name = e
                .get(ContextKind::InvalidSubcommand)
                .map(ToString::to_string)
                .unwrap_or_default();
            format!("unknown command \"{name}\" for \"{PROG}\"")
        }
        ErrorKind::UnknownArgument => {
            let arg = e
                .get(ContextKind::InvalidArg)
                .map(ToString::to_string)
                .unwrap_or_default();
            format!("unknown flag: {}", arg.split_whitespace().next().unwrap_or(""))
        }
        ErrorKind::MissingRequiredArgument => {
            let raw = e
                .get(ContextKind::InvalidArg)
                .map(ToString::to_string)
                .unwrap_or_default();
            if raw.starts_with('<') {
                return "accepts 1 arg(s), received 0".to_owned();
            }
            let names: Vec<String> = raw
                .split("--")
                .filter_map(|p| p.split_whitespace().next())
                .map(|n| format!("\"{n}\""))
                .collect();
            format!("required flag(s) {} not set", names.join(", "))
        }
        ErrorKind::ArgumentConflict => {
            "if any flags in the group [id no-comments] are set none of the others can be; [id no-comments] were all set"
                .to_owned()
        }
        _ => {
            let text = e.to_string();
            let first = text.lines().next().unwrap_or("");
            first.strip_prefix("error: ").unwrap_or(first).to_owned()
        }
    }
}
