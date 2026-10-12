//! rvw's command line: turns flags, environment and stdin into service inputs,
//! and service outputs into text on stdout. Notices go to stderr; every
//! failure is one `rvw: ...` line and exit status 1.

mod app;
mod commands;
mod help;
mod mcp;

use std::io::Write;

use rvw_render::Terminal;

pub(crate) const PROG: &str = "rvw";

/// Runs the command line and returns the process exit status.
pub fn run(args: &[String]) -> i32 {
    // The first SIGINT/SIGTERM cancels a git call in flight, as the Go
    // process's cancelled context did.
    // A second signal ends the process, as in Go, even if the first is ignored.
    let flag = rvw_git::cancel_flag();
    for sig in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
        let _ = signal_hook::flag::register_conditional_shutdown(sig, 128 + sig, flag.clone());
        let _ = signal_hook::flag::register(sig, flag.clone());
    }
    // Text output carries comments, code and names that agents wrote: it goes
    // through a Terminal writer so none of it can drive the terminal. JSON is
    // escaped by its encoder and the TUI draws its own escapes.
    let mut out = Terminal::new(std::io::stdout());
    let mut err = Terminal::new(std::io::stderr());
    let code = match app::dispatch(args, &mut out) {
        Ok(()) => 0,
        Err(message) => {
            let _ = writeln!(err, "{PROG}: {message}");
            1
        }
    };
    let _ = out.finish();
    let _ = err.finish();
    let _ = std::io::stdout().flush();
    code
}
