//! Hand-offs to the user's own tools (B12): a terminal in the repository, and
//! git's configured diff tool on the open file. Each starts detached; rvw
//! does not wait for it, and the live refresh shows what it changes.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use gpui::Context;
use rvw_view::commit_screen::Side;
use rvw_view::keys::Screen;

use crate::shell::Shell;

/// Terminals tried in order on Linux; the first on `PATH` opens.
const TERMINALS: [&str; 9] = [
    "x-terminal-emulator",
    "gnome-terminal",
    "kgx",
    "konsole",
    "kitty",
    "alacritty",
    "wezterm",
    "foot",
    "xterm",
];

impl Shell {
    /// Opens a terminal whose working directory is the repository.
    pub fn open_terminal(&mut self, cx: &mut Context<Self>) {
        let dir = self.repo_path().to_path_buf();
        let line = match terminal_command(&dir) {
            Some(mut command) => match detached(&mut command) {
                Ok(()) => "Opened a terminal".to_owned(),
                Err(e) => format!("Could not open a terminal: {e}"),
            },
            None => format!("No terminal found; tried {}", TERMINALS.join(", ")),
        };
        self.git.network_result = Some(line);
        cx.notify();
    }

    /// Opens `path` in git's diff tool: against the review's base on the
    /// review, staged or unstaged as listed on the Changes screen.
    pub fn open_diff_tool(&mut self, path: &str, cx: &mut Context<Self>) {
        let mut command = Command::new("git");
        command
            .arg("-C")
            .arg(self.repo_path())
            .args(["difftool", "--no-prompt"]);
        match self.screen {
            Screen::Diff | Screen::Files if !self.snapshot.base_id.is_empty() => {
                command.arg(&self.snapshot.base_id);
            }
            Screen::Changes
                if self
                    .changes
                    .open
                    .as_ref()
                    .is_some_and(|open| open.side == Side::Staged) =>
            {
                command.arg("--cached");
            }
            _ => {}
        }
        command.args(["--", path]);
        if let Err(e) = detached(&mut command) {
            self.git.network_result = Some(format!("Could not start git difftool: {e}"));
            cx.notify();
        }
    }
}

/// The command that opens a terminal in `dir`, if one is installed.
fn terminal_command(dir: &Path) -> Option<Command> {
    if cfg!(target_os = "macos") {
        let mut command = Command::new("open");
        command.args(["-a", "Terminal"]).arg(dir);
        return Some(command);
    }
    let found = TERMINALS.iter().find_map(|name| on_path(name))?;
    let mut command = Command::new(&found);
    if found.ends_with("wezterm") {
        command.arg("start").arg("--cwd").arg(dir);
    }
    command.current_dir(dir);
    Some(command)
}

/// The full path of program `name` in one of `PATH`'s directories.
fn on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// Starts `command` with no pipes to this process, so it outlives a busy UI.
fn detached(command: &mut Command) -> std::io::Result<()> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
}
