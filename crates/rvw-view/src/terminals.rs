//! terminals: the terminals of one workspace, as the dock lists them. The
//! tabs, the active one, whether the dock is shown, and the words the quit
//! guard uses. Pure: the caller owns the processes and reports their ends.
//! A tab's title is the command, and for a finished process its exit status;
//! nothing is inferred from output.

/// A terminal's stable identity. The caller assigns it; the view never holds
/// the process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TerminalId(pub u64);

/// How a terminal's process stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Running,
    /// The process ended. `None` when it left no exit code (a signal).
    Exited(Option<i32>),
}

/// What a tab in the dock holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A process on a PTY, typed into by the reviewer.
    Process,
    /// The log of the git commands rvw runs: read-only, no process behind it.
    Git,
}

/// One terminal in the dock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Terminal {
    pub id: TerminalId,
    pub kind: Kind,
    /// The command line the terminal was started with.
    pub command: String,
    pub status: Status,
    /// What the program asked the user's attention with, until the tab is
    /// looked at. Only the latest is kept.
    pub attention: Option<String>,
    /// The program in the foreground of a running terminal, when it is not
    /// the one the terminal started with (a shell that runs `claude`).
    pub program: Option<String>,
}

impl Terminal {
    /// The tab's text: the command, then the exit status once finished.
    #[must_use]
    pub fn title(&self) -> String {
        if self.kind == Kind::Git {
            return self.command.clone();
        }
        match self.status {
            // Only the user's shell hands its terminal to other programs.
            Status::Running if self.command == SHELL_LABEL => {
                self.program.clone().unwrap_or_else(|| self.command.clone())
            }
            Status::Running => self.command.clone(),
            Status::Exited(Some(code)) => format!("{} (exit {code})", self.command),
            Status::Exited(None) => format!("{} (ended by signal)", self.command),
        }
    }

    #[must_use]
    pub fn running(&self) -> bool {
        self.kind == Kind::Process && self.status == Status::Running
    }
}

/// The tab label of the git log.
pub const GIT_LABEL: &str = "git";
/// The tab label of the user's own shell.
pub const SHELL_LABEL: &str = "shell";
/// The longest command line kept, in bytes.
pub const MAX_COMMAND_LEN: usize = 1024;

/// What a new terminal runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launch {
    /// The user's own shell, from the account database.
    Shell,
    /// A command line, run as `/bin/sh -c`.
    Command(String),
    /// The git log tab: not a process.
    Git,
}

impl Launch {
    /// The tab's command text.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Shell => SHELL_LABEL.to_owned(),
            Self::Command(line) => line.clone(),
            Self::Git => GIT_LABEL.to_owned(),
        }
    }

    /// The program and arguments to spawn; `None` is the user's shell.
    #[must_use]
    pub fn argv(&self) -> Option<Vec<String>> {
        match self {
            Self::Shell | Self::Git => None,
            Self::Command(line) => Some(vec!["/bin/sh".to_owned(), "-c".to_owned(), line.clone()]),
        }
    }
}

/// One row of the new-terminal palette.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchEntry {
    pub launch: Launch,
    /// The row's text.
    pub title: String,
    /// What the row says about itself: where it came from.
    pub note: &'static str,
}

/// The rows for what is typed. With nothing typed: the shell. With text:
/// the text itself as a command first, so Enter always runs what was typed,
/// then the shell when its name contains the text (case ignored).
#[must_use]
pub fn launch_entries(query: &str) -> Vec<LaunchEntry> {
    let query = query.trim();
    let shell = LaunchEntry {
        launch: Launch::Shell,
        title: "Shell".to_owned(),
        note: "your shell",
    };
    let git = LaunchEntry {
        launch: Launch::Git,
        title: "Git log".to_owned(),
        note: "every git command rvw runs, read-only",
    };
    if query.is_empty() {
        return vec![shell, git];
    }
    let mut out = Vec::new();
    if query.len() <= MAX_COMMAND_LEN {
        out.push(LaunchEntry {
            launch: Launch::Command(query.to_owned()),
            title: query.to_owned(),
            note: "run this command",
        });
    }
    if shell.title.to_lowercase().contains(&query.to_lowercase()) {
        out.push(shell);
    }
    if git.title.to_lowercase().contains(&query.to_lowercase()) {
        out.push(git);
    }
    out
}

/// The most terminals the quit guard names before it counts the rest.
const NAMED_LIMIT: usize = 3;

/// The terminals of a workspace, the active one, and the dock's visibility.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Terminals {
    list: Vec<Terminal>,
    active: Option<TerminalId>,
    shown: bool,
}

impl Terminals {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The terminals in tab order.
    #[must_use]
    pub fn list(&self) -> &[Terminal] {
        &self.list
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    #[must_use]
    pub fn get(&self, id: TerminalId) -> Option<&Terminal> {
        self.list.iter().find(|t| t.id == id)
    }

    #[must_use]
    pub fn active(&self) -> Option<TerminalId> {
        self.active
    }

    /// Position of the active terminal in `list`.
    #[must_use]
    pub fn active_index(&self) -> Option<usize> {
        let active = self.active?;
        self.list.iter().position(|t| t.id == active)
    }

    /// Adds a running terminal at the end, makes it active and shows the dock.
    /// Returns false, changing nothing, when `id` is already listed.
    pub fn add(&mut self, id: TerminalId, command: &str) -> bool {
        if self.get(id).is_some() {
            return false;
        }
        self.list.push(Terminal {
            id,
            kind: Kind::Process,
            command: command.to_owned(),
            status: Status::Running,
            attention: None,
            program: None,
        });
        self.active = Some(id);
        self.shown = true;
        true
    }

    /// The git log tab, if one is open.
    #[must_use]
    pub fn git(&self) -> Option<TerminalId> {
        self.list.iter().find(|t| t.kind == Kind::Git).map(|t| t.id)
    }

    /// Adds the git log tab, active, and shows the dock. There is one: when it
    /// is open already that one becomes active. Returns whether a tab was
    /// added, false when `id` is taken and no git tab exists.
    pub fn add_git(&mut self, id: TerminalId) -> bool {
        if let Some(open) = self.git() {
            self.active = Some(open);
            self.shown = true;
            return false;
        }
        if self.get(id).is_some() {
            return false;
        }
        self.list.push(Terminal {
            id,
            kind: Kind::Git,
            command: GIT_LABEL.to_owned(),
            status: Status::Running,
            attention: None,
            program: None,
        });
        self.active = Some(id);
        self.shown = true;
        true
    }

    /// Removes a terminal. The active one passes to the tab that takes its
    /// place, or the one before when it was last. Closing the last terminal
    /// hides the dock. Returns false when `id` is not listed.
    pub fn close(&mut self, id: TerminalId) -> bool {
        let Some(at) = self.list.iter().position(|t| t.id == id) else {
            return false;
        };
        self.list.remove(at);
        if self.active == Some(id) {
            self.active = self.list.get(at).or_else(|| self.list.last()).map(|t| t.id);
        }
        if self.list.is_empty() {
            self.shown = false;
        }
        true
    }

    /// Closes the active terminal and returns its id.
    pub fn close_active(&mut self) -> Option<TerminalId> {
        let id = self.active?;
        self.close(id);
        Some(id)
    }

    /// Records that a terminal's process ended. Returns false when `id` is
    /// not listed. The tab stays, showing the status, until it is closed.
    pub fn exited(&mut self, id: TerminalId, code: Option<i32>) -> bool {
        match self.list.iter_mut().find(|t| t.id == id) {
            Some(t) => {
                t.status = Status::Exited(code);
                true
            }
            None => false,
        }
    }

    /// Records that the program in `id` asked for attention with `text`.
    /// Returns false when `id` is not listed.
    pub fn notify(&mut self, id: TerminalId, text: &str) -> bool {
        match self.list.iter_mut().find(|t| t.id == id) {
            Some(t) => {
                t.attention = Some(text.to_owned());
                true
            }
            None => false,
        }
    }

    /// Records the foreground program of `id`, `None` when the terminal's own
    /// command is back. Returns true when the tab's text may have changed.
    pub fn set_program(&mut self, id: TerminalId, program: Option<String>) -> bool {
        match self.list.iter_mut().find(|t| t.id == id) {
            Some(t) if t.program != program => {
                t.program = program;
                true
            }
            _ => false,
        }
    }

    /// Whether the active tab is on screen: the dock is shown and a terminal
    /// is active. What the user is looking at needs no badge.
    #[must_use]
    pub fn watching(&self, id: TerminalId) -> bool {
        self.dock_shown() && self.active == Some(id)
    }

    /// Forgets the attention request of `id`: true when there was one.
    pub fn clear_attention(&mut self, id: TerminalId) -> bool {
        self.list
            .iter_mut()
            .find(|t| t.id == id)
            .is_some_and(|t| t.attention.take().is_some())
    }

    /// Terminals still asking for attention.
    #[must_use]
    pub fn attention(&self) -> Vec<&Terminal> {
        self.list.iter().filter(|t| t.attention.is_some()).collect()
    }

    /// Makes `id` active. Returns false when it is not listed.
    pub fn switch_to(&mut self, id: TerminalId) -> bool {
        if self.get(id).is_none() {
            return false;
        }
        self.active = Some(id);
        true
    }

    /// The next tab, wrapping at the end.
    pub fn next(&mut self) {
        self.step(1);
    }

    /// The previous tab, wrapping at the start.
    pub fn prev(&mut self) {
        self.step(self.list.len().saturating_sub(1));
    }

    fn step(&mut self, by: usize) {
        if let Some(at) = self.active_index() {
            let to = (at + by) % self.list.len();
            self.active = Some(self.list[to].id);
        }
    }

    /// The tab a `⌘1`..`⌘9` chord names; `number` counts from 1. Returns
    /// false when there is no such tab.
    pub fn switch_to_number(&mut self, number: usize) -> bool {
        match number.checked_sub(1).and_then(|i| self.list.get(i)) {
            Some(t) => {
                self.active = Some(t.id);
                true
            }
            None => false,
        }
    }

    /// Whether the dock is on screen. Only a window with terminals shows it.
    #[must_use]
    pub fn dock_shown(&self) -> bool {
        self.shown && !self.list.is_empty()
    }

    /// Shows or hides the dock without touching the terminals: hiding is not
    /// closing. With nothing to show it stays hidden and returns false.
    pub fn toggle_dock(&mut self) -> bool {
        if self.list.is_empty() {
            return false;
        }
        self.shown = !self.shown;
        true
    }

    pub fn hide_dock(&mut self) {
        self.shown = false;
    }

    pub fn show_dock(&mut self) {
        self.shown = !self.list.is_empty();
    }

    /// Terminals whose process is still alive.
    #[must_use]
    pub fn running(&self) -> Vec<&Terminal> {
        self.list.iter().filter(|t| t.running()).collect()
    }

    /// What the window asks before quitting, naming the running terminals;
    /// `None` when none runs and the window may close.
    #[must_use]
    pub fn quit_guard(&self) -> Option<String> {
        let running = self.running();
        if running.is_empty() {
            return None;
        }
        let names: Vec<&str> = running
            .iter()
            .take(NAMED_LIMIT)
            .map(|t| t.command.as_str())
            .collect();
        let more = running.len() - names.len();
        let mut list = names.join(", ");
        if more > 0 {
            list = format!("{list} and {more} more");
        }
        let (count, pronoun) = match running.len() {
            1 => ("1 terminal is".to_owned(), "it"),
            n => (format!("{n} terminals are"), "them"),
        };
        Some(format!(
            "{count} still running: {list}. Quitting ends {pronoun}."
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: TerminalId = TerminalId(1);
    const B: TerminalId = TerminalId(2);
    const C: TerminalId = TerminalId(3);

    fn three() -> Terminals {
        let mut t = Terminals::new();
        t.add(A, "claude");
        t.add(B, "cargo test");
        t.add(C, "zsh");
        t
    }

    #[test]
    fn a_shell_tab_names_the_program_in_front() {
        let mut t = three();
        let shell = TerminalId(4);
        t.add(shell, SHELL_LABEL);
        assert!(t.set_program(shell, Some("claude".into())));
        assert!(!t.set_program(shell, Some("claude".into())));
        assert_eq!(t.get(shell).unwrap().title(), "claude");
        // A tab that was started with a command keeps it.
        assert!(t.set_program(B, Some("rustc".into())));
        assert_eq!(t.get(B).unwrap().title(), "cargo test");
        assert!(t.set_program(shell, None));
        assert_eq!(t.get(shell).unwrap().title(), "shell");
        assert!(!t.set_program(TerminalId(9), None));
    }

    #[test]
    fn attention_is_kept_until_the_tab_is_watched() {
        let mut t = three();
        assert!(t.notify(A, "needs input"));
        assert!(!t.notify(TerminalId(9), "x"));
        assert_eq!(t.attention().len(), 1);
        assert!(!t.watching(A));
        t.switch_to(A);
        assert!(t.watching(A));
        t.hide_dock();
        assert!(!t.watching(A));
        t.show_dock();
        assert!(t.clear_attention(A));
        assert!(!t.clear_attention(A));
        assert!(t.attention().is_empty());
    }

    #[test]
    fn add_activates_the_new_terminal_and_shows_the_dock() {
        let mut t = Terminals::new();
        assert!(!t.dock_shown());
        assert_eq!(t.active(), None);
        assert!(t.add(A, "claude"));
        assert!(t.add(B, "zsh"));
        assert_eq!(t.active(), Some(B));
        assert_eq!(t.active_index(), Some(1));
        assert!(t.dock_shown());
        assert_eq!(t.list().len(), 2);
    }

    #[test]
    fn add_refuses_a_duplicate_id() {
        let mut t = three();
        t.switch_to(A);
        assert!(!t.add(A, "other"));
        assert_eq!(t.list().len(), 3);
        assert_eq!(t.active(), Some(A));
        assert_eq!(t.get(A).unwrap().command, "claude");
    }

    #[test]
    fn close_of_the_active_tab_passes_to_the_one_that_takes_its_place() {
        let mut t = three();
        t.switch_to(B);
        assert!(t.close(B));
        assert_eq!(t.active(), Some(C));
        assert!(t.close(C));
        assert_eq!(t.active(), Some(A));
    }

    #[test]
    fn close_of_another_tab_keeps_the_active_one() {
        let mut t = three();
        assert!(t.close(A));
        assert_eq!(t.active(), Some(C));
        assert_eq!(t.list().len(), 2);
    }

    #[test]
    fn close_of_an_unknown_id_changes_nothing() {
        let mut t = three();
        assert!(!t.close(TerminalId(9)));
        assert_eq!(t.list().len(), 3);
        assert_eq!(t.active(), Some(C));
    }

    #[test]
    fn closing_the_last_terminal_empties_and_hides_the_dock() {
        let mut t = Terminals::new();
        t.add(A, "zsh");
        assert_eq!(t.close_active(), Some(A));
        assert!(t.is_empty());
        assert_eq!(t.active(), None);
        assert!(!t.dock_shown());
        assert_eq!(t.close_active(), None);
        t.add(B, "zsh");
        assert!(t.dock_shown());
    }

    #[test]
    fn next_and_prev_wrap() {
        let mut t = three();
        t.next();
        assert_eq!(t.active(), Some(A));
        t.prev();
        assert_eq!(t.active(), Some(C));
        t.prev();
        assert_eq!(t.active(), Some(B));
    }

    #[test]
    fn next_and_prev_on_nothing_or_one_do_not_move() {
        let mut t = Terminals::new();
        t.next();
        t.prev();
        assert_eq!(t.active(), None);
        t.add(A, "zsh");
        t.next();
        t.prev();
        assert_eq!(t.active(), Some(A));
    }

    #[test]
    fn switch_by_number_counts_from_one_and_refuses_out_of_range() {
        let mut t = three();
        assert!(t.switch_to_number(1));
        assert_eq!(t.active(), Some(A));
        assert!(t.switch_to_number(2));
        assert_eq!(t.active(), Some(B));
        assert!(!t.switch_to_number(0));
        assert!(!t.switch_to_number(4));
        assert!(!t.switch_to_number(9));
        assert_eq!(t.active(), Some(B));
    }

    #[test]
    fn switch_to_an_id() {
        let mut t = three();
        assert!(t.switch_to(A));
        assert_eq!(t.active(), Some(A));
        assert!(!t.switch_to(TerminalId(9)));
        assert_eq!(t.active(), Some(A));
    }

    #[test]
    fn a_running_title_is_the_command() {
        let t = three();
        assert_eq!(t.get(B).unwrap().title(), "cargo test");
    }

    #[test]
    fn a_finished_title_carries_the_exit_status() {
        let mut t = three();
        assert!(t.exited(B, Some(101)));
        assert_eq!(t.get(B).unwrap().title(), "cargo test (exit 101)");
        assert!(t.exited(A, Some(0)));
        assert_eq!(t.get(A).unwrap().title(), "claude (exit 0)");
        assert!(t.exited(C, None));
        assert_eq!(t.get(C).unwrap().title(), "zsh (ended by signal)");
        assert!(!t.exited(TerminalId(9), Some(0)));
    }

    #[test]
    fn a_finished_terminal_stays_until_closed() {
        let mut t = three();
        t.exited(C, Some(0));
        assert_eq!(t.list().len(), 3);
        assert_eq!(t.active(), Some(C));
        assert!(!t.get(C).unwrap().running());
    }

    #[test]
    fn hiding_the_dock_is_not_closing() {
        let mut t = three();
        t.hide_dock();
        assert!(!t.dock_shown());
        assert_eq!(t.list().len(), 3);
        assert_eq!(t.running().len(), 3);
        assert_eq!(t.active(), Some(C));
        assert!(t.toggle_dock());
        assert!(t.dock_shown());
        t.toggle_dock();
        t.show_dock();
        assert!(t.dock_shown());
    }

    #[test]
    fn a_hidden_dock_shows_again_when_a_terminal_is_added() {
        let mut t = three();
        t.hide_dock();
        t.add(TerminalId(4), "htop");
        assert!(t.dock_shown());
    }

    #[test]
    fn an_empty_dock_cannot_be_shown() {
        let mut t = Terminals::new();
        assert!(!t.toggle_dock());
        t.show_dock();
        assert!(!t.dock_shown());
    }

    #[test]
    fn quit_guard_is_silent_without_running_terminals() {
        let mut t = Terminals::new();
        assert_eq!(t.quit_guard(), None);
        t.add(A, "zsh");
        t.exited(A, Some(0));
        assert_eq!(t.quit_guard(), None);
    }

    #[test]
    fn quit_guard_names_one_running_terminal() {
        let mut t = three();
        t.exited(A, Some(0));
        t.exited(C, Some(1));
        assert_eq!(
            t.quit_guard().as_deref(),
            Some("1 terminal is still running: cargo test. Quitting ends it.")
        );
    }

    #[test]
    fn quit_guard_names_several_and_counts_the_rest() {
        let mut t = three();
        assert_eq!(
            t.quit_guard().as_deref(),
            Some("3 terminals are still running: claude, cargo test, zsh. Quitting ends them.")
        );
        t.add(TerminalId(4), "htop");
        t.add(TerminalId(5), "vim");
        assert_eq!(
            t.quit_guard().as_deref(),
            Some(
                "5 terminals are still running: claude, cargo test, zsh and 2 more. \
                 Quitting ends them."
            )
        );
    }

    #[test]
    fn quit_guard_counts_hidden_terminals() {
        let mut t = three();
        t.hide_dock();
        assert!(t.quit_guard().is_some());
    }

    #[test]
    fn entries_with_no_query_list_the_shell_and_the_git_log() {
        let rows = launch_entries("  ");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].launch, Launch::Shell);
        assert_eq!(rows[1].launch, Launch::Git);
    }

    #[test]
    fn the_git_log_is_one_read_only_tab_that_never_blocks_quitting() {
        let mut t = Terminals::new();
        assert!(t.add_git(TerminalId(1)));
        assert!(t.add(TerminalId(2), "sh"));
        assert!(!t.add_git(TerminalId(3)));
        assert_eq!(t.active(), Some(TerminalId(1)));
        assert_eq!(t.git(), Some(TerminalId(1)));
        assert_eq!(t.get(TerminalId(1)).unwrap().title(), "git");
        t.close(TerminalId(2));
        assert!(t.quit_guard().is_none());
    }

    #[test]
    fn a_typed_command_comes_first() {
        let rows = launch_entries("htop");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].launch, Launch::Command("htop".to_owned()));
        let rows = launch_entries("SHE");
        assert_eq!(rows[0].launch, Launch::Command("SHE".to_owned()));
        assert_eq!(rows[1].launch, Launch::Shell);
    }

    #[test]
    fn launch_argv_runs_a_command_through_sh() {
        assert_eq!(Launch::Shell.argv(), None);
        assert_eq!(Launch::Shell.label(), "shell");
        let argv = Launch::Command("a | b".to_owned()).argv().unwrap();
        assert_eq!(argv, ["/bin/sh", "-c", "a | b"]);
    }
}
