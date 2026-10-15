//! The review service as MCP tools, over stdio or loopback streamable HTTP.
//! Port of internal/mcpserver: one tool per `Service` operation.

mod scope;
mod server;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use rmcp::ServiceExt;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rvw_core::Service;
use server::RvwServer;
use tokio_util::sync::CancellationToken;

/// How often a serving loop checks the cancel flag.
const POLL: Duration = Duration::from_millis(100);
/// How long a streamable HTTP server may take to drain after cancellation.
const DRAIN: Duration = Duration::from_secs(2);

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
pub fn serve_stdio(svc: Arc<Service>, opts: Options) -> Result<(), String> {
    let runtime = runtime()?;
    let result = runtime.block_on(async {
        let session = async {
            let running = match RvwServer::new(svc, opts)
                .serve(rmcp::transport::stdio())
                .await
            {
                Ok(running) => running,
                // stdin closed before the client said hello: nothing to serve.
                Err(rmcp::service::ServerInitializeError::ConnectionClosed(_)) => return Ok(()),
                Err(e) => return Err(e.to_string()),
            };
            running
                .waiting()
                .await
                .map(|_| ())
                .map_err(|e| e.to_string())
        };
        // A blocked stdin read cannot be interrupted, and the handshake may
        // still be waiting on it, so a set flag ends the session regardless.
        let flag = rvw_git::cancel_flag();
        tokio::select! {
            result = session => result,
            () = wait_for(&flag) => Ok(()),
        }
    });
    runtime.shutdown_timeout(Duration::from_millis(100));
    result
}

/// Serves streamable HTTP at `/mcp` on `addr` (already checked as loopback).
/// `on_listening` receives the bound `host:port` once the socket is open.
/// Returns when `rvw_git::cancel_flag()` is set.
pub fn serve_http(
    svc: Arc<Service>,
    opts: Options,
    addr: &str,
    on_listening: &dyn Fn(&str),
) -> Result<(), String> {
    let runtime = runtime()?;
    let result = runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| format!("cannot listen on {addr}: {e}"))?;
        let bound = listener.local_addr().map_err(|e| e.to_string())?;
        on_listening(&bound.to_string());

        let token = CancellationToken::new();
        let config = StreamableHttpServerConfig::default()
            .with_cancellation_token(token.clone())
            .with_json_response(true);
        let service = StreamableHttpService::new(
            move || Ok(RvwServer::new(Arc::clone(&svc), opts.clone())),
            Arc::new(LocalSessionManager::default()),
            config,
        );
        let app = axum::Router::new().nest_service("/mcp", service);

        let flag = rvw_git::cancel_flag();
        let shutdown = async move {
            wait_for(&flag).await;
            token.cancel();
        };
        let serving = axum::serve(listener, app).with_graceful_shutdown(shutdown);
        // Open connections may outlive the shutdown signal; drain for a bounded time.
        match tokio::time::timeout(DRAIN, serving).await {
            Ok(Ok(())) | Err(_) => Ok(()),
            Ok(Err(e)) => Err(e.to_string()),
        }
    });
    runtime.shutdown_timeout(Duration::from_millis(100));
    result
}

fn runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|e| e.to_string())
}

/// Returns once `flag` is set, checking it every [`POLL`].
async fn wait_for(flag: &AtomicBool) {
    while !flag.load(Ordering::Relaxed) {
        tokio::time::sleep(POLL).await;
    }
}
