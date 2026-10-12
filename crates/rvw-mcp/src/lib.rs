//! The review service as MCP tools, over stdio or loopback streamable HTTP.
//! Port of internal/mcpserver. Slice 3 fills this in.

use std::sync::Arc;

use rvw_core::Service;

/// The server's defaults, set by `rvw mcp serve` flags.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Used when a tool call leaves `workspace` empty.
    pub workspace: String,
    /// Pins every call that does not name a lane itself.
    pub lane: String,
    /// Speaks for every call that does not name an author itself.
    pub author: String,
    pub version: String,
}

/// Serves over stdin/stdout until the client disconnects or a signal cancels.
pub fn serve_stdio(_svc: Arc<Service>, _opts: Options) -> Result<(), String> {
    Err("mcp serve is not available in this build yet".to_owned())
}

/// Serves streamable HTTP at `/mcp` on `addr` (already checked as loopback).
/// `on_listening` receives the bound `host:port` once the socket is open.
/// Returns when `rvw_git::cancel_flag()` is set.
pub fn serve_http(
    _svc: Arc<Service>,
    _opts: Options,
    _addr: &str,
    _on_listening: &dyn Fn(&str),
) -> Result<(), String> {
    Err("mcp serve is not available in this build yet".to_owned())
}
