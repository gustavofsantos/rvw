//! rvw-gui: the native review window. This slice shows the queue screen.

mod data;
mod frame;
mod key_strip;
mod queue_screen;
mod shell;
mod theme;

use std::path::PathBuf;

use gpui::{
    px, size, App, AppContext, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions,
};

use crate::data::Snapshot;
use crate::shell::Shell;

#[derive(Default)]
struct Args {
    workspace: Option<PathBuf>,
    db: Option<PathBuf>,
    lane: String,
}

fn main() {
    if let Err(message) = run() {
        eprintln!("rvw-gui: {message}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = parse_args(std::env::args().skip(1))?;

    let start = match args.workspace {
        Some(dir) => dir,
        None => std::env::current_dir().map_err(|e| e.to_string())?,
    };
    let workspace = rvw_git::workspace::resolve(&start)?
        .to_string_lossy()
        .into_owned();

    let db = match args.db {
        Some(path) => std::path::absolute(path).map_err(|e| e.to_string())?,
        None => rvw_store::default_path().map_err(|e| e.message)?,
    };
    let store = rvw_store::Store::open(&db).map_err(|e| e.message)?;
    let service = rvw_core::Service::new(Box::new(store));

    let snapshot = data::load(&service, &workspace, &args.lane)?;
    show_window(snapshot);
    Ok(())
}

fn parse_args(mut args: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut parsed = Args::default();
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--workspace" => parsed.workspace = Some(PathBuf::from(flag_value(&mut args, &flag)?)),
            "--db" => parsed.db = Some(PathBuf::from(flag_value(&mut args, &flag)?)),
            "--lane" => parsed.lane = flag_value(&mut args, &flag)?,
            // accepted now; the author is not used by any screen yet
            "--author" => {
                flag_value(&mut args, &flag)?;
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(parsed)
}

fn flag_value(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    args.next().ok_or_else(|| format!("{flag} needs a value"))
}

/// Starts the application and opens the one window. Errors here happen after
/// the database is open, so they are reported from inside the app callback.
fn show_window(snapshot: Snapshot) {
    Application::new().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1440.), px(900.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("rvw".into()),
                ..TitlebarOptions::default()
            }),
            ..WindowOptions::default()
        };
        if let Err(error) = cx.open_window(options, |_window, cx| cx.new(|_| Shell::new(snapshot)))
        {
            eprintln!("rvw-gui: {error}");
            std::process::exit(1);
        }
        cx.activate(true);
    });
}
