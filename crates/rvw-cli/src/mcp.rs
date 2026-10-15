//! `rvw mcp config` and the loopback rules shared with `rvw mcp serve`.

use std::net::IpAddr;

use serde::Serialize;

use crate::PROG;
use crate::app::{App, CliResult, Out, lane_of, one_of};

pub const MCP_PATH: &str = "/mcp";

/// The defaults a server applies to calls that leave them empty.
pub struct ServerFlags {
    pub lane: String,
    pub author: String,
    pub http: String,
}

/// One server entry of Claude Code's `.mcp.json`.
#[derive(Serialize)]
struct McpServer {
    #[serde(rename = "type")]
    kind: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    command: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    args: Vec<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    url: String,
}

/// Refuses an `--http` address other programs on the network could reach:
/// the server has no authentication, so it must stay on this machine.
pub fn check_loopback(addr: &str) -> CliResult<()> {
    let bad_form = || format!("--http '{addr}' is not HOST:PORT");
    let host = if let Some(rest) = addr.strip_prefix('[') {
        let (host, tail) = rest.split_once(']').ok_or_else(bad_form)?;
        if !tail.starts_with(':') {
            return Err(bad_form());
        }
        host
    } else {
        let (host, _port) = addr.rsplit_once(':').ok_or_else(bad_form)?;
        if host.contains(':') {
            return Err(bad_form());
        }
        host
    };
    if host == "localhost" {
        return Ok(());
    }
    if host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback()) {
        return Ok(());
    }
    Err(format!("--http must be a loopback address, got '{addr}'"))
}

fn mcp_url(addr: &str) -> String {
    format!("http://{addr}{MCP_PATH}")
}

/// How to start this binary: `rvw` when PATH finds this very file, else its
/// absolute path.
fn executable() -> String {
    let Ok(me) = std::env::current_exe().and_then(std::fs::canonicalize) else {
        return PROG.to_owned();
    };
    let found = std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|d| d.join(PROG))
            .find(|p| p.is_file())
    });
    if let Some(found) = found.and_then(|p| std::fs::canonicalize(p).ok())
        && found == me
    {
        return PROG.to_owned();
    }
    me.to_string_lossy().into_owned()
}

/// Quotes each word that a POSIX shell would split or expand.
fn shell_join(words: &[String]) -> String {
    let safe = |c: char| c.is_ascii_alphanumeric() || "-_./:=@,+".contains(c);
    words
        .iter()
        .map(|w| {
            if !w.is_empty() && w.chars().all(safe) {
                w.clone()
            } else {
                format!("'{}'", w.replace('\'', r"'\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The `rvw mcp serve` command line that config describes.
fn serve_args(app: &App, flags: &ServerFlags, with_http: bool) -> Vec<String> {
    let mut args = vec![executable(), "mcp".to_owned(), "serve".to_owned()];
    if !app.db_flag().is_empty()
        && let Ok(db) = std::path::absolute(crate::app::expand_home(app.db_flag()))
    {
        args.push("--db".to_owned());
        args.push(db.to_string_lossy().into_owned());
    }
    if !app.workspace_flag().is_empty()
        && let Ok(ws) = app.workspace()
    {
        args.push("--workspace".to_owned());
        args.push(ws);
    }
    let lane = lane_of(&flags.lane);
    if !lane.is_empty() {
        args.push("--lane".to_owned());
        args.push(lane);
    }
    let author = flags.author.trim();
    if !author.is_empty() {
        args.push("--author".to_owned());
        args.push(author.to_owned());
    }
    if with_http {
        args.push("--http".to_owned());
        args.push(flags.http.clone());
    }
    args
}

fn entry(
    app: &App,
    flags: &ServerFlags,
    scope: &str,
    name: &str,
) -> CliResult<(McpServer, String)> {
    if name.trim().is_empty() {
        return Err("--name must not be empty".to_owned());
    }
    if !flags.http.is_empty() {
        check_loopback(&flags.http)?;
        let url = mcp_url(&flags.http);
        let add = shell_join(
            &[
                "claude",
                "mcp",
                "add",
                "--transport",
                "http",
                "--scope",
                scope,
                name,
                &url,
            ]
            .map(str::to_owned),
        );
        return Ok((
            McpServer {
                kind: "http".to_owned(),
                command: String::new(),
                args: Vec::new(),
                url,
            },
            add,
        ));
    }
    let args = serve_args(app, flags, false);
    let mut words: Vec<String> = ["claude", "mcp", "add", "--scope", scope, name, "--"]
        .map(str::to_owned)
        .to_vec();
    words.extend(args.iter().cloned());
    Ok((
        McpServer {
            kind: "stdio".to_owned(),
            command: args[0].clone(),
            args: args[1..].to_vec(),
            url: String::new(),
        },
        shell_join(&words),
    ))
}

pub fn config(
    app: &App,
    flags: &ServerFlags,
    scope: &str,
    name: &str,
    format: &str,
    out: &mut Out,
) -> CliResult<()> {
    use std::io::Write;
    one_of("--format", format, &["text", "json"])?;
    one_of("--scope", scope, &["local", "project", "user"])?;
    if !app.workspace_flag().is_empty() {
        app.workspace()?;
    }
    let (server, add) = entry(app, flags, scope, name)?;
    let doc = serde_json::json!({ "mcpServers": { name: server } });
    if format == "json" {
        return rvw_render::json(&mut std::io::stdout(), &doc).map_err(|e| e.to_string());
    }
    let mut b: Vec<u8> = Vec::new();
    if flags.http.is_empty() {
        write!(
            b,
            "# Register rvw with Claude Code; it starts the server on stdio:\n{add}\n\n"
        )
    } else {
        write!(
            b,
            "# 1. Keep the server running:\n{}\n\n# 2. Register it with Claude Code:\n{add}\n\n",
            shell_join(&serve_args(app, flags, true))
        )
    }
    .map_err(|e| e.to_string())?;
    writeln!(b, "# Or add this to .mcp.json at the project root:").map_err(|e| e.to_string())?;
    rvw_render::json(&mut b, &doc).map_err(|e| e.to_string())?;
    write!(
        b,
        "\n# Check it with `claude mcp list`, or /mcp inside Claude Code.\n"
    )
    .map_err(|e| e.to_string())?;
    out.write_all(&b).map_err(|e| e.to_string())
}
