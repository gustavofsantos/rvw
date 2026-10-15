//! The command tree and one handler per command.

use std::fmt::Write as _;
use std::io::{IsTerminal, Read, Write};

use clap::{Arg, ArgAction, ArgMatches, Command, value_parser};
use rvw_core::operations::{
    AddInput, EditInput, GateInput, GetInput, LedgerInput, OverrideInput, PullInput, QueryInput,
    RemoveInput, ResolveInput, SHEET_FILTERS, SessionsInput, SheetsInput, SubmitInput,
    VerdictInput, WorkspacesInput,
};
use rvw_core::service::MAX_SOURCE;
use rvw_core::types::{Actor, Comment, GATE_ACTIONS, Handoff, LineRange, Location, VerdictKind};
use rvw_core::types::{DECISIONS, OUTCOME_DONE, OUTCOME_REJECTED, STATUS_FILTERS, is_review_id};
use rvw_render as render;

use crate::PROG;
use crate::app::{
    App, CliResult, Out, author_of, core_err, expand_home, file_path, lane_of, one_of,
};
use crate::help;
use crate::mcp::{self, ServerFlags};

fn version() -> &'static str {
    option_env!("RVW_VERSION").unwrap_or("(devel)")
}

/// A value-taking option. Like pflag, it takes the next word as its value even
/// when that word starts with `-`.
fn flag(name: &'static str, help: &'static str) -> Arg {
    Arg::new(name)
        .long(name)
        .help(help)
        .value_name(name.to_uppercase())
        .allow_hyphen_values(true)
}

/// The one positional ID. It takes any number of words so that a surplus one
/// is reported the way cobra reports it, by `one_id`.
fn id_arg() -> Arg {
    Arg::new("id").value_name("ID").required(true).num_args(0..)
}

fn bool_flag(name: &'static str, help: &'static str) -> Arg {
    Arg::new(name)
        .long(name)
        .help(help)
        .action(ArgAction::SetTrue)
}

fn format_flag(default: &'static str, allowed: &[&str]) -> Arg {
    flag(
        "format",
        Box::leak(
            format!("output shape: {} (default: {default})", allowed.join(", ")).into_boxed_str(),
        ),
    )
    .default_value(default)
}

fn lane_flags(cmd: Command) -> Command {
    cmd.arg(flag(
        "lane",
        "only this lane's comments (default: every lane)",
    ))
    .arg(bool_flag("all-lanes", "every lane, overriding --lane"))
}

fn status_flag(cmd: Command, help: &'static str) -> Command {
    cmd.arg(flag("status", help).default_value("pending"))
}

fn sub(name: &'static str, short: &'static str, long: &'static str) -> Command {
    Command::new(name)
        .about(short)
        .long_about(long)
        .disable_version_flag(true)
        .disable_help_flag(true)
}

#[allow(clippy::too_many_lines)] // one declarative table of every command
pub fn root() -> Command {
    let add = sub(
        "add",
        "enqueue a review comment on a file range",
        help::ADD_LONG,
    )
    .override_usage("rvw add --file PATH --lines N|N-M [--comment TEXT]")
    .after_help(help::ADD_EXAMPLE)
    .arg(flag("file", "file the comment is about").required(true))
    .arg(flag("lines", "1-indexed, inclusive line range: N or N-M").required(true))
    .arg(flag("comment", "the comment text (may be multi-line)"))
    .arg(flag(
        "comment-file",
        "read the comment from a file, or - for stdin",
    ))
    .arg(flag(
        "code-file",
        "snapshot the code from here instead of disk, or - for stdin (whole file)",
    ))
    .arg(flag(
        "filetype",
        "fence language (default: guessed from the extension)",
    ))
    .arg(flag("lane", "branch/lane this comment belongs to"))
    .arg(flag("author", "who raised it (default: $USER)"))
    .arg(flag(
        "points-at",
        "another file range this note refers to: FILE:N or FILE:N-M",
    ))
    .arg(format_flag("text", &["text", "json", "ids"]));

    let submit = sub(
        "submit",
        "submit a review over pending review comments",
        help::SUBMIT_LONG,
    )
    .override_usage("rvw submit --decision DECISION --summary TEXT [--id ID]... | --no-comments")
    .arg(
        flag(
            "id",
            "comment id (repeatable; default: this author and exact lane)",
        )
        .action(ArgAction::Append)
        .conflicts_with("no-comments"),
    )
    .arg(bool_flag(
        "no-comments",
        "submit a summary-only review without selecting pending comments",
    ))
    .arg(flag("summary", "review assessment").required(true))
    .arg(
        flag(
            "decision",
            "review decision: comment, approve, request-changes",
        )
        .required(true),
    )
    .arg(flag("lane", "review lane"))
    .arg(flag("author", "reviewer (default: $USER)"))
    .arg(format_flag("text", &["text", "json", "ids"]));

    let list = lane_flags(status_flag(
        sub(
            "list",
            "show queued review comments or submitted reviews without dequeuing them",
            help::LIST_LONG,
        )
        .after_help(help::LIST_EXAMPLE)
        .arg(flag("file", "only comments on this file"))
        .arg(bool_flag(
            "reviews",
            "list submitted review sheets instead of comments",
        ))
        .arg(format_flag(
            "text",
            &["text", "json", "ids", "count", "markdown"],
        )),
        "which comments to show: pending, pulled, done, rejected, open (pending + pulled), all",
    ));

    let count = lane_flags(status_flag(
        sub(
            "count",
            "print the pending handoff count (statusline-friendly)",
            help::COUNT_LONG,
        )
        .arg(flag("file", "only count comments on this file")),
        "which comments to count (default: pending)",
    ));

    let pull = lane_flags(
        sub(
            "pull",
            "dequeue review handoffs for an agent to act on",
            help::PULL_LONG,
        )
        .after_help(help::PULL_EXAMPLE)
        .arg(flag("file", "only handoffs touching this file"))
        .arg(flag("id", "pull only this id (repeatable)").action(ArgAction::Append))
        .arg(
            flag("limit", "pull at most N handoffs, oldest first").value_parser(value_parser!(i64)),
        )
        .arg(bool_flag(
            "peek",
            "print without dequeuing (leaves the queue intact)",
        ))
        .arg(format_flag(
            "markdown",
            &["markdown", "json", "text", "ids"],
        )),
    );

    let show = sub(
        "show",
        "show one review comment (or review sheet) with its source evidence",
        help::SHOW_LONG,
    )
    .override_usage("rvw show ID")
    .after_help(help::SHOW_EXAMPLE)
    .arg(id_arg())
    .arg(format_flag("text", &["text", "json", "markdown"]));

    let edit = sub(
        "edit",
        "replace the text of a queued review comment",
        help::EDIT_LONG,
    )
    .override_usage("rvw edit ID [--comment TEXT]")
    .after_help(help::EDIT_EXAMPLE)
    .arg(id_arg())
    .arg(flag("comment", "new text (default: read stdin)"))
    .arg(format_flag("text", &["text", "json"]));

    let remove = sub(
        "remove",
        "delete a queued review comment before it is handed over",
        help::REMOVE_LONG,
    )
    .override_usage("rvw remove ID")
    .after_help(help::REMOVE_EXAMPLE)
    .arg(id_arg())
    .arg(format_flag("text", &["text", "json"]));

    let resolve = sub(
        "resolve",
        "record that a comment was acted on",
        help::RESOLVE_LONG,
    )
    .override_usage("rvw resolve ID")
    .after_help(help::RESOLVE_EXAMPLE)
    .arg(id_arg())
    .arg(flag("note", "what you did"))
    .arg(flag("author", "who decided (default: $USER)"));
    let reject = sub(
        "reject",
        "record that a comment will not be actioned, and why",
        help::REJECT_LONG,
    )
    .override_usage("rvw reject ID --note TEXT")
    .after_help(help::REJECT_EXAMPLE)
    .arg(id_arg())
    .arg(flag("note", "why not (required)"))
    .arg(flag("author", "who decided (default: $USER)"));

    let accept = sub(
        "accept",
        "agree with a decision made after a handoff",
        help::ACCEPT_LONG,
    )
    .override_usage("rvw accept ID [--note TEXT]")
    .after_help(help::ACCEPT_EXAMPLE)
    .arg(id_arg())
    .arg(flag("note", "a remark on the verdict (optional)"))
    .arg(flag(
        "author",
        "who judges, not who decided (default: $USER)",
    ))
    .arg(format_flag("text", &["text", "json"]));

    let reopen = sub(
        "reopen",
        "send a decided comment back to the agent as a follow-up",
        help::REOPEN_LONG,
    )
    .override_usage("rvw reopen ID --note TEXT [--lines N-M]")
    .after_help(help::REOPEN_EXAMPLE)
    .arg(id_arg())
    .arg(flag("note", "what is still wrong (required)"))
    .arg(flag(
        "lines",
        "the follow-up's range: N or N-M (default: the original's)",
    ))
    .arg(flag(
        "author",
        "who judges, not who decided (default: $USER)",
    ))
    .arg(format_flag("text", &["text", "json", "ids"]));

    let gate_override = sub(
        "override",
        "record going ahead past an unclear gate, and why",
        help::GATE_OVERRIDE_LONG,
    )
    .override_usage("rvw gate override --action ACTION --reason TEXT [--lane LANE]")
    .after_help(help::GATE_OVERRIDE_EXAMPLE)
    .arg(flag("action", "commit or push (required)").required(true))
    .arg(flag("reason", "why the action goes ahead (required)").required(true))
    .arg(flag(
        "lane",
        "only this lane's comments (default: every lane)",
    ))
    .arg(flag("author", "who overrides (default: $USER)"))
    .arg(format_flag("text", &["text", "json"]));

    let gate = lane_flags(
        sub(
            "gate",
            "report what is open or unjudged before a commit or push",
            help::GATE_LONG,
        )
        .after_help(help::GATE_EXAMPLE)
        .arg(format_flag("text", &["text", "json"]))
        .subcommand(gate_override),
    );

    let workspaces = sub(
        "workspaces",
        "list workspaces holding review handoffs",
        help::WORKSPACES_LONG,
    )
    .arg(bool_flag("all", "include workspaces with nothing pending"))
    .arg(format_flag("text", &["text", "json"]));

    let sessions_cmd = sub(
        "sessions",
        "list the sessions agents reported, newest first",
        help::SESSIONS_LONG,
    )
    .after_help(help::SESSIONS_EXAMPLE)
    .arg(format_flag("text", &["text", "json", "markdown"]));

    let ledger_cmd = sub(
        "ledger",
        "write the audit ledger of a range of commits",
        help::LEDGER_LONG,
    )
    .after_help(help::LEDGER_EXAMPLE)
    .arg(flag(
        "range",
        "git revision range, e.g. main..HEAD (default: the default branch to HEAD)",
    ))
    .arg(flag(
        "since",
        "only commits dated at or after this, RFC 3339 or YYYY-MM-DD",
    ))
    .arg(flag(
        "until",
        "only commits dated at or before this, RFC 3339 or YYYY-MM-DD",
    ))
    .arg(flag(
        "lane",
        "only this lane's comments and overrides (default: every lane)",
    ))
    .arg(format_flag("markdown", &["markdown", "json", "html"]))
    .arg(flag("output", "write the report to FILE instead of stdout"))
    .arg(bool_flag(
        "force",
        "overwrite the --output FILE if it exists",
    ));

    let path = sub("path", "print where the queue lives", help::PATH_LONG).arg(bool_flag(
        "workspace-only",
        "print the resolved workspace directory",
    ));

    let server_flags =
        |c: Command| {
            c.arg(flag(
            "lane",
            "pin calls that name no lane to this lane (default: every lane)",
        ))
        .arg(flag("author", "author of calls that name none (default: $USER)"))
        .arg(flag(
            "http",
            "serve streamable HTTP on this loopback address (e.g. 127.0.0.1:7777) instead of stdio",
        ))
        };
    let mcp_serve = server_flags(
        sub(
            "serve",
            "run the MCP server on stdio, or on a local HTTP address",
            help::MCP_SERVE_LONG,
        )
        .override_usage("rvw mcp serve [--http ADDR]")
        .after_help(help::MCP_SERVE_EXAMPLE),
    );
    let mcp_config = server_flags(
        sub(
            "config",
            "print how to add the MCP server to Claude Code",
            help::MCP_CONFIG_LONG,
        )
        .after_help(help::MCP_CONFIG_EXAMPLE)
        .arg(flag("scope", "claude mcp add scope: local, project, user").default_value("user"))
        .arg(flag("name", "server name in Claude Code").default_value("rvw"))
        .arg(format_flag("text", &["text", "json"])),
    );
    let mcp = sub(
        "mcp",
        "serve the queue as MCP tools, and print how to connect Claude Code",
        help::MCP_LONG,
    )
    .subcommand(mcp_serve)
    .subcommand(mcp_config);

    let tui = sub(
        "tui",
        "browse the workspace and leave review comments in a terminal UI",
        help::TUI_LONG,
    )
    .after_help("  rvw tui\n  rvw tui --lane refactor-auth --editor \"code --wait\"\n  rvw tui --leader ,\n  rvw --workspace ~/src/project tui")
    .arg(flag("lane", "lane the comments and reviews you write land in"))
    .arg(flag("author", "who is reviewing (default: $USER)"))
    .arg(flag(
        "editor",
        "command to write comments in (default: $VISUAL, else $EDITOR, else vi)",
    ))
    .arg(
        flag(
            "leader",
            "key that starts leader mappings: \"space\" or one character, such as \",\"",
        )
        .default_value("space"),
    );

    let help_cmd = sub(
        "help",
        "Help about any command",
        "Help about any command: `rvw help COMMAND` prints that command's help.",
    )
    .arg(Arg::new("topic").value_name("COMMAND").num_args(0..));

    Command::new(PROG)
        .about(help::ABOUT)
        .override_usage("rvw COMMAND")
        .after_help(help::EXAMPLES)
        .disable_help_subcommand(true)
        .disable_version_flag(true)
        .disable_help_flag(true)
        .arg(
            flag(
                "workspace",
                "queue to act on: the git toplevel of this directory (default: $PWD)",
            )
            .global(true),
        )
        .arg(
            flag(
                "db",
                "database file (default: $XDG_DATA_HOME/rvw/rvw.db, else ~/.local/share/rvw/rvw.db)",
            )
            .global(true),
        )
        .arg(
            Arg::new("version")
                .long("version")
                .short('v')
                .action(ArgAction::SetTrue)
                .help("version for rvw"),
        )
        .subcommands([
            add, submit, list, pull, show, edit, remove, resolve, reject, accept, reopen, gate,
            count, workspaces, path, sessions_cmd, ledger_cmd, mcp, tui, help_cmd,
        ])
}

pub fn run(app: &App, m: &ArgMatches, out: &mut Out) -> CliResult<()> {
    if m.get_flag("version") {
        writeln!(out, "{PROG} version {}", version()).map_err(|e| e.to_string())?;
        return Ok(());
    }
    match m.subcommand() {
        Some(("add", s)) => add(app, s, out),
        Some(("submit", s)) => submit(app, s, out),
        Some(("list", s)) => list(app, s, out),
        Some(("count", s)) => count(app, s, out),
        Some(("pull", s)) => pull(app, s, out),
        Some(("show", s)) => show(app, s, out),
        Some(("edit", s)) => edit(app, s, out),
        Some(("remove", s)) => remove(app, s, out),
        Some(("resolve", s)) => decide(app, s, OUTCOME_DONE, out),
        Some(("reject", s)) => decide(app, s, OUTCOME_REJECTED, out),
        Some(("accept", s)) => accept(app, s, out),
        Some(("reopen", s)) => reopen(app, s, out),
        Some(("gate", s)) => gate_cmd(app, s, out),
        Some(("workspaces", s)) => workspaces(app, s, out),
        Some(("sessions", s)) => sessions(app, s, out),
        Some(("ledger", s)) => ledger(app, s, out),
        Some(("path", s)) => path(app, s, out),
        Some(("mcp", s)) => mcp_cmd(app, s, out),
        Some(("tui", s)) => tui(app, s),
        Some(("help", s)) => {
            let topic: Vec<String> = many(s, "topic");
            w(write!(out, "{}", help::text(&topic)))
        }
        _ => w(write!(out, "{}", help::text(&[]))),
    }
}

// ── small helpers ────────────────────────────────────────────────────────────

fn s(m: &ArgMatches, name: &str) -> String {
    m.get_one::<String>(name).cloned().unwrap_or_default()
}

fn many(m: &ArgMatches, name: &str) -> Vec<String> {
    m.get_many::<String>(name)
        .map(|v| v.cloned().collect())
        .unwrap_or_default()
}

fn w<T>(r: std::io::Result<T>) -> CliResult<T> {
    r.map_err(|e| e.to_string())
}

fn json<T: serde::Serialize>(v: &T) -> CliResult<()> {
    w(render::json(&mut std::io::stdout(), v))
}

fn pinned(m: &ArgMatches) -> String {
    if m.get_flag("all-lanes") {
        String::new()
    } else {
        lane_of(&s(m, "lane"))
    }
}

fn check_status(value: &str) -> CliResult<()> {
    one_of("--status", value, &STATUS_FILTERS)
}

fn query(app: &App, m: &ArgMatches) -> CliResult<QueryInput> {
    let status = s(m, "status");
    check_status(&status)?;
    let workspace = app.workspace()?;
    Ok(QueryInput {
        workspace,
        status,
        file: file_path(&s(m, "file"))?,
        lane: pinned(m),
    })
}

fn empty_notice(ws: &str, lane: &str) {
    let scope = if lane.is_empty() {
        String::new()
    } else {
        format!(" in lane {lane}")
    };
    App::notice(&format!("no review comments for {ws}{scope}"));
}

fn location_line(c: &Comment) -> String {
    format!(
        "{}  {}  {}",
        c.id,
        c.location(),
        render::first_line(&c.comment)
    )
}

// ── add / submit ─────────────────────────────────────────────────────────────

fn comment_text(
    given: bool,
    comment: &str,
    comment_file: &str,
    code_file: &str,
) -> CliResult<String> {
    let mut text = comment.to_owned();
    if given {
    } else if comment_file == "-" {
        if code_file == "-" {
            return Err("--comment-file - conflicts with --code-file - (one stdin)".to_owned());
        }
        text = App::read_stdin_if_piped()?.unwrap_or_default();
    } else if !comment_file.is_empty() {
        let data = std::fs::read(expand_home(comment_file))
            .map_err(|_| format!("--comment-file '{comment_file}' does not exist"))?;
        text = String::from_utf8_lossy(&data).into_owned();
    } else if code_file != "-" {
        text = App::read_stdin_if_piped()?.unwrap_or_default();
    }
    if text.trim().is_empty() {
        return Err(
            "a review comment needs text: pass --comment TEXT, --comment-file PATH, or pipe it in"
                .to_owned(),
        );
    }
    Ok(text)
}

fn code_source(code_file: &str) -> CliResult<Option<String>> {
    match code_file {
        "" => Ok(None),
        "-" => Ok(Some(App::read_stdin_if_piped()?.unwrap_or_default())),
        path => {
            let f = std::fs::File::open(expand_home(path)).map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    format!("--code-file '{code_file}' does not exist")
                } else {
                    rvw_git::os_error_text(&e)
                }
            })?;
            // One byte past the limit is enough for the service to refuse it.
            let mut data = Vec::new();
            w(f.take(MAX_SOURCE as u64 + 1).read_to_end(&mut data))?;
            Ok(Some(String::from_utf8_lossy(&data).into_owned()))
        }
    }
}

/// `FILE:N` or `FILE:N-M`. Split on the last colon, so the file may hold one;
/// the range is digits and at most one dash, and a single N is one line.
fn points_at_of(spec: &str) -> CliResult<Location> {
    let bad = || format!("--points-at '{spec}' is not FILE:N or FILE:N-M");
    let (file, lines) = spec.rsplit_once(':').ok_or_else(bad)?;
    let (first, last) = lines.split_once('-').unwrap_or((lines, lines));
    let number = |t: &str| {
        if !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()) {
            t.parse::<i64>().ok()
        } else {
            None
        }
    };
    match (file.is_empty(), number(first), number(last)) {
        (false, Some(start_line), Some(end_line)) => Ok(Location {
            file: file_path(file)?,
            start_line,
            end_line,
        }),
        _ => Err(bad()),
    }
}

fn add(app: &App, m: &ArgMatches, out: &mut Out) -> CliResult<()> {
    let format = s(m, "format");
    one_of("--format", &format, &["text", "json", "ids"])?;
    let ws = app.workspace()?;
    let range = LineRange::parse(&s(m, "lines")).map_err(core_err)?;
    let text = comment_text(
        m.contains_id("comment"),
        &s(m, "comment"),
        &s(m, "comment-file"),
        &s(m, "code-file"),
    )?;
    let source = code_source(&s(m, "code-file"))?;
    let file = file_path(&s(m, "file"))?;
    let points_at = if m.contains_id("points-at") {
        Some(points_at_of(&s(m, "points-at"))?)
    } else {
        None
    };
    let svc = app.svc()?;
    let c = svc
        .add(AddInput {
            points_at,
            workspace: ws,
            file,
            start_line: range.start_line,
            end_line: range.end_line,
            comment: text,
            source,
            filetype: s(m, "filetype"),
            lane: lane_of(&s(m, "lane")),
            author: author_of(&s(m, "author")),
        })
        .map_err(core_err)?;
    match format.as_str() {
        "json" => json(&c),
        "ids" => w(writeln!(out, "{}", c.id)),
        _ => w(writeln!(out, "{}", location_line(&c))),
    }
}

fn submit(app: &App, m: &ArgMatches, out: &mut Out) -> CliResult<()> {
    let format = s(m, "format");
    one_of("--format", &format, &["text", "json", "ids"])?;
    let decision = s(m, "decision");
    one_of("--decision", &decision, &DECISIONS)?;
    let ws = app.workspace()?;
    let svc = app.svc()?;
    let r = svc
        .submit(SubmitInput {
            workspace: ws,
            decision,
            summary: s(m, "summary"),
            comment_ids: many(m, "id"),
            no_comments: m.get_flag("no-comments"),
            lane: lane_of(&s(m, "lane")),
            author: author_of(&s(m, "author")),
        })
        .map_err(core_err)?;
    match format.as_str() {
        "json" => json(&r),
        "ids" => w(writeln!(out, "{}", r.id)),
        _ => w(writeln!(
            out,
            "{}  {}  {} review comment(s)",
            r.id,
            r.decision,
            r.comment_ids.len()
        )),
    }
}

// ── reads ────────────────────────────────────────────────────────────────────

fn list(app: &App, m: &ArgMatches, out: &mut Out) -> CliResult<()> {
    let format = s(m, "format");
    one_of(
        "--format",
        &format,
        &["text", "json", "ids", "count", "markdown"],
    )?;
    if m.get_flag("reviews") {
        return list_sheets(app, m, &format, out);
    }
    let input = query(app, m)?;
    let lane = input.lane.clone();
    let svc = app.svc()?;
    let result = svc.list(input).map_err(core_err)?;
    match format.as_str() {
        "json" => json(&render::Envelope {
            workspace: result.workspace.clone(),
            count: result.comments.len(),
            reviews: result.comments.clone(),
            submitted_reviews: Vec::new(),
        }),
        "ids" => w(render::ids(out, &[], &result.comments)),
        "count" => w(writeln!(out, "{}", result.comments.len())),
        "markdown" => w(write!(
            out,
            "{}",
            render::markdown(&result.workspace, &[], &result.comments, false)
        )),
        _ => {
            if !w(render::text(out, &[], &result.comments))? {
                empty_notice(&result.workspace, &lane);
            }
            Ok(())
        }
    }
}

fn list_sheets(app: &App, m: &ArgMatches, format: &str, out: &mut Out) -> CliResult<()> {
    let status = s(m, "status");
    one_of("--status", &status, &SHEET_FILTERS)?;
    let ws = app.workspace()?;
    let file = file_path(&s(m, "file"))?;
    let lane = pinned(m);
    let svc = app.svc()?;
    let result = svc
        .sheets(SheetsInput {
            workspace: ws,
            status,
            file,
            lane: lane.clone(),
        })
        .map_err(core_err)?;
    match format {
        "json" => json(&result),
        "ids" => {
            for rs in &result.sheets {
                w(writeln!(out, "{}", rs.review.id))?;
            }
            Ok(())
        }
        "count" => w(writeln!(out, "{}", result.count)),
        "markdown" => {
            let handoffs: Vec<Handoff> = result
                .sheets
                .iter()
                .map(|rs| Handoff {
                    review: rs.review.clone(),
                    comments: rs.comments.clone(),
                })
                .collect();
            w(write!(
                out,
                "{}",
                render::markdown(&result.workspace, &handoffs, &[], false)
            ))
        }
        _ => {
            for rs in &result.sheets {
                w(writeln!(out, "{}", render::sheet_line(rs)))?;
            }
            if result.count == 0 {
                let scope = if lane.is_empty() {
                    String::new()
                } else {
                    format!(" in lane {lane}")
                };
                App::notice(&format!(
                    "no submitted reviews for {}{scope}",
                    result.workspace
                ));
            }
            Ok(())
        }
    }
}

fn count(app: &App, m: &ArgMatches, out: &mut Out) -> CliResult<()> {
    let input = query(app, m)?;
    let svc = app.svc()?;
    let result = svc.count(input).map_err(core_err)?;
    w(writeln!(out, "{}", result.count))
}

fn pull(app: &App, m: &ArgMatches, out: &mut Out) -> CliResult<()> {
    let format = s(m, "format");
    one_of("--format", &format, &["markdown", "json", "text", "ids"])?;
    let limit = m.get_one::<i64>("limit").copied();
    if limit.is_some_and(|l| l < 1) {
        return Err("--limit must be >= 1".to_owned());
    }
    let ws = app.workspace()?;
    let file = file_path(&s(m, "file"))?;
    let lane = pinned(m);
    let svc = app.svc()?;
    let result = svc
        .pull(PullInput {
            workspace: ws,
            lane: lane.clone(),
            file,
            ids: many(m, "id"),
            limit: limit.unwrap_or(0),
            peek: m.get_flag("peek"),
        })
        .map_err(core_err)?;
    match format.as_str() {
        "json" => json(&render::Envelope {
            workspace: result.workspace.clone(),
            count: result.count(),
            reviews: result.comments.clone(),
            submitted_reviews: result.reviews.clone(),
        })?,
        "ids" => w(render::ids(out, &result.reviews, &result.comments))?,
        "text" => {
            if !w(render::text(out, &result.reviews, &result.comments))? {
                empty_notice(&result.workspace, &lane);
            }
        }
        _ => w(write!(
            out,
            "{}",
            render::markdown(
                &result.workspace,
                &result.reviews,
                &result.comments,
                result.drained
            )
        ))?,
    }
    if let Some(e) = &result.elsewhere {
        App::notice(&format!(
            "{} pending in other lanes: {}",
            e.count,
            e.lanes.join(", ")
        ));
    }
    Ok(())
}

/// The single ID argument; cobra's `ExactArgs(1)` wording for any other count.
fn one_id(m: &ArgMatches) -> CliResult<String> {
    let mut ids = many(m, "id");
    if ids.len() != 1 {
        return Err(format!("accepts 1 arg(s), received {}", ids.len()));
    }
    Ok(ids.remove(0))
}

fn get_input(app: &App, id: String) -> CliResult<GetInput> {
    Ok(GetInput {
        workspace: app.workspace()?,
        id,
    })
}

fn show(app: &App, m: &ArgMatches, out: &mut Out) -> CliResult<()> {
    let id = one_id(m)?;
    let format = s(m, "format");
    one_of("--format", &format, &["text", "json", "markdown"])?;
    let input = get_input(app, id)?;
    let svc = app.svc()?;
    if is_review_id(&input.id) {
        let ws = input.workspace.clone();
        let sheet = svc.sheet(input).map_err(core_err)?;
        return match format.as_str() {
            "json" => json(&sheet),
            "markdown" => {
                let handoff = Handoff {
                    review: sheet.review.clone(),
                    comments: sheet.comments.clone(),
                };
                w(write!(
                    out,
                    "{}",
                    render::markdown(&ws, &[handoff], &[], false)
                ))
            }
            _ => w(write!(out, "{}", render::sheet(&sheet))),
        };
    }
    let ws = input.workspace.clone();
    let ev = svc.evidence(input).map_err(core_err)?;
    match format.as_str() {
        "json" => json(&ev),
        "markdown" => w(write!(
            out,
            "{}",
            render::markdown(&ws, &[], std::slice::from_ref(&ev.comment), false)
        )),
        _ => w(write!(out, "{}", render::display(&ev))),
    }
}

fn sessions(app: &App, m: &ArgMatches, out: &mut Out) -> CliResult<()> {
    let format = s(m, "format");
    one_of("--format", &format, &["text", "json", "markdown"])?;
    let ws = app.workspace()?;
    let svc = app.svc()?;
    let result = svc
        .sessions(SessionsInput {
            workspace: ws.clone(),
        })
        .map_err(core_err)?;
    match format.as_str() {
        "json" => json(&result),
        "markdown" => w(write!(
            out,
            "{}",
            render::sessions_markdown(&ws, &result.sessions)
        )),
        _ => {
            if !w(render::sessions_text(out, &result.sessions))? {
                App::notice(&format!("no reported sessions for {ws}"));
            }
            Ok(())
        }
    }
}

fn ledger(app: &App, m: &ArgMatches, out: &mut Out) -> CliResult<()> {
    let format = s(m, "format");
    one_of("--format", &format, &["markdown", "json", "html"])?;
    let output = expand_home(&s(m, "output"));
    let force = m.get_flag("force");
    if force && output.is_empty() {
        return Err("--force needs --output".to_owned());
    }
    // Refused before the ledger is read: a report that cannot be written is
    // not worth the walk through history.
    if !output.is_empty() {
        check_output(&output, force)?;
    }
    let ws = app.workspace()?;
    let svc = app.svc()?;
    let ledger = svc
        .ledger(LedgerInput {
            workspace: ws,
            range: s(m, "range"),
            since: s(m, "since"),
            until: s(m, "until"),
            lane: lane_of(&s(m, "lane")),
        })
        .map_err(core_err)?;
    let text = match format.as_str() {
        "json" => render::ledger_json(&ledger),
        "html" => render::ledger_html(&ledger),
        _ => render::ledger_markdown(&ledger),
    };
    if output.is_empty() {
        return w(write!(out, "{text}"));
    }
    write_output(&output, &text)?;
    App::notice(&format!("wrote {output}"));
    Ok(())
}

/// Refuses an `--output` that would overwrite a file without `--force`, or
/// that sits in a directory which does not exist. Nothing is created.
fn check_output(path: &str, force: bool) -> CliResult<()> {
    let target = std::path::Path::new(path);
    if target.exists() && !force {
        return Err(format!(
            "--output '{path}' exists; pass --force to overwrite it"
        ));
    }
    let dir = parent_dir(target);
    if !dir.is_dir() {
        return Err(format!(
            "--output '{path}': directory '{}' does not exist",
            dir.display()
        ));
    }
    Ok(())
}

/// Writes `text` to `path` through a temporary file in the same directory,
/// renamed over the target, so a reader never sees half a report.
fn write_output(path: &str, text: &str) -> CliResult<()> {
    let target = std::path::Path::new(path);
    let name = target
        .file_name()
        .ok_or_else(|| format!("--output '{path}' names no file"))?;
    let tmp = parent_dir(target).join(format!(
        ".{}.{}.tmp",
        name.to_string_lossy(),
        std::process::id()
    ));
    replace_file(&tmp, target, text).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("--output '{path}': {e}")
    })
}

fn replace_file(
    tmp: &std::path::Path,
    target: &std::path::Path,
    text: &str,
) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(tmp)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    std::fs::rename(tmp, target)
}

/// The directory a path sits in; a bare file name sits in the current one.
fn parent_dir(path: &std::path::Path) -> &std::path::Path {
    match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => std::path::Path::new("."),
    }
}

fn workspaces(app: &App, m: &ArgMatches, out: &mut Out) -> CliResult<()> {
    let format = s(m, "format");
    one_of("--format", &format, &["text", "json"])?;
    let svc = app.svc()?;
    let result = svc
        .workspaces(WorkspacesInput {
            all: m.get_flag("all"),
        })
        .map_err(core_err)?;
    let rows = result.workspaces;
    if format == "json" {
        return json(&rows);
    }
    if rows.is_empty() {
        App::notice("no workspace has pending review comments");
        return Ok(());
    }
    let width = rows
        .iter()
        .map(|r| r.pending.to_string().len())
        .max()
        .unwrap_or(0);
    for r in &rows {
        w(writeln!(
            out,
            "{:>width$} pending  {}",
            r.pending, r.workspace
        ))?;
    }
    Ok(())
}

fn path(app: &App, m: &ArgMatches, out: &mut Out) -> CliResult<()> {
    if m.get_flag("workspace-only") {
        return w(writeln!(out, "{}", app.workspace()?));
    }
    let svc = app.svc()?;
    w(writeln!(out, "{}", svc.location()))
}

// ── decisions and edits ──────────────────────────────────────────────────────

fn decide(app: &App, m: &ArgMatches, outcome: &str, out: &mut Out) -> CliResult<()> {
    let id = one_id(m)?;
    let note = s(m, "note");
    if outcome == OUTCOME_REJECTED && note.trim().is_empty() {
        return Err("rejecting a comment needs a reason: pass --note TEXT".to_owned());
    }
    let input = get_input(app, id)?;
    let svc = app.svc()?;
    let c = svc
        .resolve(ResolveInput {
            workspace: input.workspace,
            id: input.id,
            outcome: outcome.to_owned(),
            note: note.clone(),
            author: author_of(&s(m, "author")),
        })
        .map_err(core_err)?;
    let mut line = format!("{}  {}  {}", c.id, c.location(), c.status);
    if let Some(by) = &c.resolved_by {
        let _ = write!(line, " by @{by}");
    }
    if let Some(n) = c.resolution_note.as_deref().filter(|n| !n.is_empty()) {
        let _ = write!(line, " — {}", render::first_line(n));
    }
    w(writeln!(out, "{line}"))
}

fn edit(app: &App, m: &ArgMatches, out: &mut Out) -> CliResult<()> {
    let id = one_id(m)?;
    let format = s(m, "format");
    one_of("--format", &format, &["text", "json"])?;
    let comment = if m.contains_id("comment") {
        s(m, "comment")
    } else {
        match App::read_stdin_if_piped()? {
            Some(text) => text,
            None => return Err("pass the new text with --comment TEXT, or pipe it in".to_owned()),
        }
    };
    let input = get_input(app, id)?;
    let svc = app.svc()?;
    let c = svc
        .edit(EditInput {
            workspace: input.workspace,
            id: input.id,
            comment,
        })
        .map_err(core_err)?;
    if format == "json" {
        return json(&c);
    }
    w(writeln!(out, "{}", location_line(&c)))
}

fn remove(app: &App, m: &ArgMatches, out: &mut Out) -> CliResult<()> {
    let id = one_id(m)?;
    let format = s(m, "format");
    one_of("--format", &format, &["text", "json"])?;
    let input = get_input(app, id)?;
    let svc = app.svc()?;
    let c = svc
        .remove(RemoveInput {
            workspace: input.workspace,
            id: input.id,
        })
        .map_err(core_err)?;
    if format == "json" {
        return json(&c);
    }
    w(writeln!(out, "removed {}", location_line(&c)))
}

// ── verdicts and the gate ────────────────────────────────────────────────────

/// Who gave the verdict on `c`, for the one-line report.
fn verdict_by(c: &Comment) -> &str {
    c.verdict.as_ref().map_or("", |v| v.by.as_str())
}

fn accept(app: &App, m: &ArgMatches, out: &mut Out) -> CliResult<()> {
    let id = one_id(m)?;
    let format = s(m, "format");
    one_of("--format", &format, &["text", "json"])?;
    let input = get_input(app, id)?;
    let svc = app.svc()?;
    let result = svc
        .judge(
            VerdictInput {
                workspace: input.workspace,
                id: input.id,
                verdict: VerdictKind::Accepted.as_str().to_owned(),
                note: s(m, "note"),
                start_line: 0,
                end_line: 0,
                author: author_of(&s(m, "author")),
            },
            Actor::Human,
        )
        .map_err(core_err)?;
    if format == "json" {
        return json(&result);
    }
    w(writeln!(
        out,
        "{} accepted by {}",
        result.comment.id,
        verdict_by(&result.comment)
    ))
}

fn reopen(app: &App, m: &ArgMatches, out: &mut Out) -> CliResult<()> {
    let id = one_id(m)?;
    let format = s(m, "format");
    one_of("--format", &format, &["text", "json", "ids"])?;
    let note = s(m, "note");
    if note.trim().is_empty() {
        return Err("reopening a comment needs a note: pass --note TEXT".to_owned());
    }
    // Without --lines the follow-up keeps the original's range: (0, 0).
    let (start_line, end_line) = if m.contains_id("lines") {
        let range = LineRange::parse(&s(m, "lines")).map_err(core_err)?;
        (range.start_line, range.end_line)
    } else {
        (0, 0)
    };
    let input = get_input(app, id)?;
    let svc = app.svc()?;
    let result = svc
        .judge(
            VerdictInput {
                workspace: input.workspace,
                id: input.id,
                verdict: VerdictKind::Reopened.as_str().to_owned(),
                note,
                start_line,
                end_line,
                author: author_of(&s(m, "author")),
            },
            Actor::Human,
        )
        .map_err(core_err)?;
    if format == "json" {
        return json(&result);
    }
    let Some(followup) = result.followup.as_ref() else {
        return Err("reopening made no follow-up".to_owned());
    };
    match format.as_str() {
        "ids" => w(writeln!(out, "{}", followup.id)),
        _ => w(writeln!(
            out,
            "{} reopened as {}",
            result.comment.id, followup.id
        )),
    }
}

/// `rvw gate`, or `rvw gate override` when that subcommand is named.
fn gate_cmd(app: &App, m: &ArgMatches, out: &mut Out) -> CliResult<()> {
    match m.subcommand() {
        Some(("override", sm)) => gate_override(app, sm, out),
        _ => gate(app, m, out),
    }
}

fn gate(app: &App, m: &ArgMatches, out: &mut Out) -> CliResult<()> {
    let format = s(m, "format");
    one_of("--format", &format, &["text", "json"])?;
    let ws = app.workspace()?;
    let svc = app.svc()?;
    let result = svc
        .gate(GateInput {
            workspace: ws,
            lane: pinned(m),
        })
        .map_err(core_err)?;
    if format == "json" {
        return json(&result);
    }
    if result.clear {
        return w(writeln!(out, "clear"));
    }
    if !result.open.is_empty() {
        w(writeln!(out, "open: {}", result.open.join(" ")))?;
    }
    if !result.unjudged.is_empty() {
        w(writeln!(out, "unjudged: {}", result.unjudged.join(" ")))?;
    }
    Ok(())
}

fn gate_override(app: &App, m: &ArgMatches, out: &mut Out) -> CliResult<()> {
    let action = s(m, "action");
    one_of("--action", &action, &GATE_ACTIONS)?;
    let format = s(m, "format");
    one_of("--format", &format, &["text", "json"])?;
    let ws = app.workspace()?;
    let svc = app.svc()?;
    let record = svc
        .override_gate(
            OverrideInput {
                workspace: ws,
                action,
                reason: s(m, "reason"),
                lane: lane_of(&s(m, "lane")),
                author: author_of(&s(m, "author")),
            },
            Actor::Human,
        )
        .map_err(core_err)?;
    if format == "json" {
        return json(&record);
    }
    w(writeln!(
        out,
        "override recorded: {}, {} open, {} unjudged",
        record.action,
        record.open.len(),
        record.unjudged.len()
    ))
}

// ── mcp / tui ────────────────────────────────────────────────────────────────

fn server_flags(m: &ArgMatches) -> ServerFlags {
    ServerFlags {
        lane: s(m, "lane"),
        author: s(m, "author"),
        http: s(m, "http"),
    }
}

fn mcp_cmd(app: &App, m: &ArgMatches, out: &mut Out) -> CliResult<()> {
    match m.subcommand() {
        Some(("serve", sm)) => {
            let flags = server_flags(sm);
            if !flags.http.is_empty() {
                mcp::check_loopback(&flags.http)?;
            }
            let ws = app.workspace()?;
            let svc = std::sync::Arc::clone(app.svc()?);
            let opts = rvw_mcp::Options {
                workspace: ws,
                lane: lane_of(&flags.lane),
                author: author_of(&flags.author),
                version: version().to_owned(),
            };
            if flags.http.is_empty() {
                rvw_mcp::serve_stdio(svc, opts)
            } else {
                rvw_mcp::serve_http(svc, opts, &flags.http, &|addr| {
                    App::notice(&format!("serving MCP at http://{addr}{}", mcp::MCP_PATH));
                })
            }
        }
        Some(("config", sm)) => {
            let flags = server_flags(sm);
            mcp::config(
                app,
                &flags,
                &s(sm, "scope"),
                &s(sm, "name"),
                &s(sm, "format"),
                out,
            )
        }
        _ => w(write!(out, "{}", help::text(&["mcp".to_owned()]))),
    }
}

/// The leader key as the UI names keys: `space`, or the one printable
/// character given.
fn leader_of(flag: &str) -> CliResult<String> {
    if flag == "space" || flag == " " {
        return Ok("space".to_owned());
    }
    let mut chars = flag.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if !c.is_control() && !c.is_whitespace() => Ok(flag.to_owned()),
        _ => Err(format!(
            "--leader must be space or one character, got {flag:?}"
        )),
    }
}

fn tui(app: &App, m: &ArgMatches) -> CliResult<()> {
    leader_of(&s(m, "leader"))?;
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err("tui needs an interactive terminal on stdin and stdout".to_owned());
    }
    let workspace = app.workspace()?;
    let service = std::sync::Arc::clone(app.svc()?);
    let editor = std::env::var("VISUAL")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| {
            std::env::var("EDITOR")
                .ok()
                .filter(|v| !v.trim().is_empty())
        });
    let flag_editor = s(m, "editor");
    rvw_tui::run(rvw_tui::Options {
        service,
        workspace,
        lane: lane_of(&s(m, "lane")),
        author: author_of(&s(m, "author")),
        editor: if flag_editor.trim().is_empty() {
            editor.unwrap_or_else(|| "vi".to_owned())
        } else {
            flag_editor.trim().to_owned()
        },
        style: String::new(),
        leader: leader_of(&s(m, "leader"))?,
    })
}
