//! Help texts, kept word for word from the Go command line, and the renderer
//! that lays them out the way cobra does.

use std::fmt::Write as _;

use clap::builder::ValueParser;
use clap::{value_parser, Arg, ArgAction, Command};

use crate::PROG;

pub const ABOUT: &str =
    "rvw — a per-workspace queue of code review feedback, written in an editor or
by a reviewing agent and pulled by a coding agent.

You annotate code where you read it (an editor plugin, `rvw tui`, or `rvw add`
straight from a shell); every comment lands in a durable queue keyed by the
workspace it was written in. Whatever agent you are running later drains that
queue with `rvw pull` — pulled comments leave the queue, so the same note is
never worked twice.

Comments stay standalone by default. `rvw submit` can group zero or more
pending comments under one review with a decision and summary. Pulling any
linked comment hands over the complete review as one unit.

A comment carries who raised it and which lane it belongs to. It ends in a
recorded resolution: `resolve` (done) or `reject` (with a reason).

Everything is set with flags:
  --workspace PATH  the queue to act on: the git toplevel of this directory
                    (default: the current directory); worktrees are their
                    own workspace, and a directory outside git has none
  --lane NAME       the lane a comment lands in and a read is pinned to
                    (default: every lane). Scoping is strict — a pull pinned
                    to a lane never swallows another lane's comments, and
                    says on stderr when comments are waiting elsewhere.
  --author WHO      who is speaking (default: $USER); agents pass their name
  --db PATH         the database (default: $XDG_DATA_HOME/rvw/rvw.db, else
                    ~/.local/share/rvw/rvw.db)";

pub const EXAMPLES: &str =
    "  rvw add --file src/api.py --lines 40-58 --comment \"extract this branch\"
  rvw add --file src/api.py --lines 12 --comment \"typo\" --lane refactor-auth
  rvw submit --id r1 --id r2 --decision request-changes --summary \"Fix both\"
  rvw submit --no-comments --decision comment --summary \"No findings\"
  rvw list                              # what is queued here, oldest first
  rvw list --file src/api.py --format json
  rvw list --reviews                    # submitted review sheets and their state
  rvw pull                              # dequeue everything, as markdown
  rvw pull --limit 1 --format json      # dequeue one, machine-readable
  rvw pull --peek                       # look without dequeuing
  rvw resolve r3 --note \"renamed, test added\"
  rvw reject r4 --note \"intentional: the caller validates\"
  rvw show r3                           # one comment, its decision and its diff
  rvw show rv1                          # a whole review sheet
  rvw count                             # pending handoff count, for a statusline
  rvw workspaces                        # every workspace holding comments
  rvw mcp config                        # connect Claude Code to the MCP server
  rvw tui                               # read code and comment in a terminal UI";

pub const ADD_LONG: &str = "Enqueue one review comment on a line range of one file.

The code shown to the agent is snapshotted at add time: by default it is read
from the file on disk, or from --code-file when the editor holds unsaved
changes (pass the WHOLE buffer; --lines slices it). The comment comes from
--comment, --comment-file, or stdin when it is piped.";

pub const ADD_EXAMPLE: &str =
    "  rvw add --file src/api.py --lines 40-58 --comment \"extract this branch\"
  rvw add --file src/api.py --lines 12 --comment \"typo\" --format json
  nvim-buffer | rvw add --file src/api.py --lines 40-58 --code-file - --comment \"reads oddly\"
  echo \"long note\" | rvw add --file a.py --lines 3-9";

pub const SUBMIT_LONG: &str =
    "Submit one review with a decision and summary over explicitly selected
pending review comments. Without --id it takes this author's pending, unlinked
comments in this exact lane; --no-comments submits a summary-only review.";

pub const LIST_LONG: &str =
    "Show this workspace's review comments, oldest first. Nothing is dequeued.

With --reviews it shows submitted review sheets instead, each with its state:
pending, pulled, or complete once every linked comment is decided. --status
then takes pending, pulled, complete, open (not yet complete) or all.";

pub const LIST_EXAMPLE: &str = "  rvw list
  rvw list --file src/api.py --format json   # what an editor draws signs from
  rvw list --status pulled                   # what has already been handed over
  rvw list --reviews                         # submitted reviews still pending
  rvw list --reviews --status all            # every submitted review";

pub const COUNT_LONG: &str =
    "Print how many review handoffs are pending here: a submitted review counts
once, with its linked comments. For any other --status it counts comments.";

pub const PULL_LONG: &str = "Dequeue this workspace's submitted reviews and standalone comments.

A submitted review and its linked comments move as one handoff. Pulled
handoffs do not appear again. Use --peek to read without draining.

Lane scoping is strict: a pull pinned to a lane takes only that lane's
comments — never an unlaned one, never another lane's — and reports on
stderr when comments are waiting elsewhere, so nothing starves unseen.";

pub const PULL_EXAMPLE: &str = "  rvw pull                     # everything pending, as markdown
  rvw pull --format json       # same, machine-readable
  rvw pull --limit 1           # oldest handoff only
  rvw pull --id r3 --id r7     # named comments only
  rvw pull --lane auth         # only that branch's comments
  rvw pull --peek              # read without dequeuing";

pub const SHOW_LONG: &str =
    "Show one review comment as a fixed-width review line: its source, or the
diff it produced once done, and its note or resolution. A submitted review id
shows the whole review sheet. Nothing is dequeued.";

pub const SHOW_EXAMPLE: &str = "  rvw show r3
  rvw show rv1
  rvw show r3 --format json";

pub const WORKSPACES_LONG: &str =
    "List every workspace in the store, with its pending handoff count.";

pub const PATH_LONG: &str =
    "Print the database backing every queue, or with --workspace-only the resolved workspace.";

pub const RESOLVE_LONG: &str =
    "Record that a review comment was acted on, and by whom. For a tracked file
that still exists, resolve also saves its current contents as a Git blob.
`pull` says a comment was handed over; `resolve` says what became of it,
so only a pulled comment can be resolved.";

pub const RESOLVE_EXAMPLE: &str = "  rvw resolve r3
  rvw resolve r3 --note \"renamed to first(), added a test\"";

pub const REJECT_LONG: &str =
    "Record that a review comment will not be actioned. The reason is required —
a silent decline is exactly what this queue exists to prevent.

It is also how a reviewer withdraws a pending comment: it leaves the queue but
stays on record. Nothing in rvw is deleted.";

pub const REJECT_EXAMPLE: &str =
    "  rvw reject r3 --note \"intentional: the caller already validates\"
  rvw reject r5 --note \"withdrawn: I misread the diff\"";

pub const EDIT_LONG: &str =
    "Replace the text of one pending review comment. The file, range, and code
snapshot are unchanged — reject and re-add to move a comment. Once a comment is
pulled its text is what the agent was asked, so it can no longer be edited.";

pub const EDIT_EXAMPLE: &str = "  rvw edit r3 --comment \"split this function\"
  pbpaste | rvw edit r3";

pub const MCP_LONG: &str =
    "Serve the queue over the Model Context Protocol, for agents that prefer tool
calls to shell commands. Every tool is one rvw operation: add, submit, list,
list_reviews, count, pull, show_comment, show_review, resolve, edit and
workspaces. Each call names its workspace, so one server serves every project.

`rvw mcp config` prints how to register the server with Claude Code.";

pub const MCP_SERVE_LONG: &str =
    "Run the MCP server. By default it speaks over stdin and stdout, the way
Claude Code starts a local server. With --http it listens on that address and
serves streamable HTTP at /mcp, for clients that connect to a running
server. The server has no authentication, so the address must be loopback
(127.0.0.1, ::1 or localhost).

A call that leaves workspace empty uses --workspace, else the git worktree the
server started in. Outside git the server refuses to start. --lane and
--author fill calls that leave them empty.";

pub const MCP_SERVE_EXAMPLE: &str = "  rvw mcp serve
  rvw mcp serve --author claude
  rvw mcp serve --http 127.0.0.1:7777";

pub const MCP_CONFIG_LONG: &str =
    "Print the `claude mcp add` command, and the equivalent .mcp.json entry, that
register rvw as an MCP server in Claude Code. Nothing is changed; run the
command or paste the entry yourself.

By default Claude Code starts the server on stdio. With --http it connects to
a server you keep running with `rvw mcp serve --http ADDR`. --db, --workspace,
--lane and --author are carried into the server command.";

pub const MCP_CONFIG_EXAMPLE: &str = "  rvw mcp config
  rvw mcp config --author claude --scope project
  rvw mcp config --http 127.0.0.1:7777
  rvw mcp config --format json          # only the .mcp.json entry";

pub const TUI_LONG: &str = r"Open a full-screen review UI on the workspace: a file tree, the current file
with syntax highlighting, and a rail in the gutter marking every line under an
open comment. Select lines, comment on them, edit a comment, submit a review.
The workspace is a git worktree; the tree leaves out the files git ignores.
t cycles the tree through every file, the changed files only, and the
reviews with their comments; opening a review or comment shows what was asked
and how it was addressed, with the diff made while resolving it, and o opens
the file at the comment. b chooses what the changed files, and the
gutter's change marks, compare with: uncommitted changes (against HEAD, the
default), the default branch (against its merge-base with main or master),
or the previous commit (against HEAD~1). Untracked files count as changes.
The bottom bar shows the comments on the cursor line, else the branch and
the uncommitted changes as +added -deleted line counts.

Comments are written in your editor: --editor, else $VISUAL, else $EDITOR,
else vi. Lines from the scissors line down are context and are dropped; an
empty note cancels. --lane and --author stamp what you add and submit; the
gutter shows every lane's open comments.

Nothing is watched: the file and its comments are re-read when you open a
file, press r, or add, edit or submit; the branch and the changes when you
press r, t or b; the reviews when you press r or t, or add, edit or submit.

Keys (? shows them in the UI); <leader> is space unless --leader says:
  <leader>p go to file · <leader>l open comments · Tab switch pane · s submit · q quit
  t files, changes or reviews · b compare with: uncommitted, default branch, previous commit
  tree    j/k move · l/↵ open or expand · h collapse or go to parent · gg/G
  viewer  j/k · C-d/C-u half page · gg/G · NG or :N go to line
          ]c/[c next/previous comment · ]h/[h next/previous change, around
          V select lines · c comment · e edit
          on a review or comment page: j/k scroll · o open the file · esc back
          r reload";

// ── cobra-style help ─────────────────────────────────────────────────────────

/// Walks a command line the way cobra does before parsing it: the command path
/// it names, and whether it asks for help (`-h` or `--help`, before any `--`).
/// A flag that takes a value consumes the next token even when that token
/// starts with `-`, as pflag does.
pub fn walk(root: &Command, args: &[String]) -> (Vec<String>, bool) {
    let mut cur = root;
    let mut path = Vec::new();
    let mut help = false;
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        i += 1;
        if a == "--" {
            break;
        }
        if a == "--help" || a == "-h" {
            help = true;
        } else if let Some(name) = a.strip_prefix("--") {
            if !name.contains('=') && takes_value(root, cur, name) {
                i += 1;
            }
        } else if a.starts_with('-') && a.len() > 1 {
            // A short cluster. None of rvw's short flags takes a value.
        } else if let Some(sub) = cur.find_subcommand(a) {
            cur = sub;
            path.push(a.to_owned());
        }
    }
    (path, help)
}

fn takes_value(root: &Command, cur: &Command, long: &str) -> bool {
    cur.get_arguments()
        .chain(root.get_arguments().filter(|a| a.is_global_set()))
        .find(|a| a.get_long() == Some(long))
        .is_some_and(|a| a.get_action().takes_values())
}

/// The help text of the command at `path` (empty: the root command). A name
/// that is not a command ends the walk, as cobra's help topics do.
pub fn text(path: &[String]) -> String {
    let root = crate::commands::root();
    let mut cmd = &root;
    let mut found = vec![PROG.to_owned()];
    for name in path {
        match cmd.find_subcommand(name) {
            Some(sub) => {
                cmd = sub;
                found.push(name.clone());
            }
            None => break,
        }
    }
    render(&root, cmd, &found.join(" "))
}

/// A flag row: its sort key (the long name), the flag column, and its help.
type FlagRow = (String, String, String);

/// Cobra's layout: description, Usage, Examples, Available Commands, Flags,
/// Global Flags, and the pointer to `--help` for a command with subcommands.
fn render(root: &Command, cmd: &Command, path: &str) -> String {
    let is_root = path == PROG;
    let mut blocks = Vec::new();
    let long = cmd
        .get_long_about()
        .or_else(|| cmd.get_about())
        .map(ToString::to_string)
        .unwrap_or_default();
    if !long.trim().is_empty() {
        blocks.push(long.trim_end().to_owned());
    }

    let subs: Vec<&Command> = cmd.get_subcommands().filter(|c| !c.is_hide_set()).collect();
    let base = cmd
        .get_overridden_usage()
        .map_or_else(|| path.to_owned(), ToString::to_string);
    let mut usage = format!("Usage:\n  {base} [flags]");
    if !subs.is_empty() {
        let _ = write!(usage, "\n  {path} [command]");
    }
    blocks.push(usage);

    if let Some(examples) = cmd.get_after_help() {
        blocks.push(format!("Examples:\n{}", examples.to_string().trim_end()));
    }
    if !subs.is_empty() {
        blocks.push(commands_block(&subs));
    }

    let name = path.rsplit(' ').next().unwrap_or(PROG);
    let mut rows: Vec<FlagRow> = cmd
        .get_arguments()
        .filter(|a| a.get_long().is_some() || a.get_short().is_some())
        .filter(|a| is_root || !a.is_global_set())
        .map(flag_row)
        .collect();
    rows.push(help_row(name));
    blocks.push(flags_block("Flags", rows));

    if !is_root {
        let globals: Vec<FlagRow> = root
            .get_arguments()
            .filter(|a| a.is_global_set())
            .map(flag_row)
            .collect();
        if !globals.is_empty() {
            blocks.push(flags_block("Global Flags", globals));
        }
    }

    if !subs.is_empty() {
        blocks.push(format!(
            "Use \"{path} [command] --help\" for more information about a command."
        ));
    }
    let mut out = blocks.join("\n\n");
    out.push('\n');
    out
}

fn commands_block(subs: &[&Command]) -> String {
    let mut rows: Vec<(String, String)> = subs
        .iter()
        .map(|c| {
            (
                c.get_name().to_owned(),
                c.get_about().map_or_else(String::new, ToString::to_string),
            )
        })
        .collect();
    rows.sort();
    let pad = rows.iter().map(|r| r.0.len()).max().unwrap_or(0).max(11);
    let mut out = "Available Commands:".to_owned();
    for (name, about) in rows {
        let _ = write!(out, "\n  {name:<pad$} {about}");
    }
    out
}

fn flag_row(a: &Arg) -> FlagRow {
    let long = a.get_long().unwrap_or_default();
    let takes = a.get_action().takes_values();
    let mut column = match a.get_short() {
        Some(c) => format!("  -{c}, "),
        None => "      ".to_owned(),
    };
    let _ = write!(column, "--{long}");
    let mut help = a.get_help().map(ToString::to_string).unwrap_or_default();
    if takes {
        let kind = if matches!(a.get_action(), ArgAction::Append) {
            "stringArray"
        } else if a.get_value_parser().type_id() == ValueParser::from(value_parser!(i64)).type_id()
        {
            "int"
        } else {
            "string"
        };
        let _ = write!(column, " {kind}");
        let default = a
            .get_default_values()
            .first()
            .map(|d| d.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !default.is_empty() {
            let _ = write!(help, " (default {default:?})");
        }
    }
    (long.to_owned(), column, help)
}

fn help_row(name: &str) -> FlagRow {
    (
        "help".to_owned(),
        "  -h, --help".to_owned(),
        format!("help for {name}"),
    )
}

/// One flags section, aligned like pflag: help text starts three columns past
/// the longest flag column.
fn flags_block(title: &str, mut rows: Vec<FlagRow>) -> String {
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    let width = rows.iter().map(|r| r.1.len()).max().unwrap_or(0);
    let mut out = format!("{title}:");
    for (_, column, help) in rows {
        out.push('\n');
        out.push_str(&column);
        if !help.is_empty() {
            out.push_str(&" ".repeat(width - column.len() + 3));
            out.push_str(&help);
        }
    }
    out
}
