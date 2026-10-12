//! rvw-gui: the native review window: the queue and the review screens.

mod composer;
mod data;
mod diff_view;
mod frame;
mod key_strip;
mod queue_screen;
mod review_screen;
mod shell;
mod text_input;
mod theme;

use std::path::PathBuf;

use gpui::{
    px, size, App, AppContext, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions,
};

use crate::data::{Identity, Snapshot};
use crate::shell::Shell;

#[derive(Default)]
struct Args {
    workspace: Option<PathBuf>,
    db: Option<PathBuf>,
    lane: String,
    author: String,
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
    let identity = Identity {
        workspace,
        author: args.author,
        lane: args.lane,
    };
    show_window(snapshot, service, identity);
    Ok(())
}

fn parse_args(mut args: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut parsed = Args::default();
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--workspace" => parsed.workspace = Some(PathBuf::from(flag_value(&mut args, &flag)?)),
            "--db" => parsed.db = Some(PathBuf::from(flag_value(&mut args, &flag)?)),
            "--lane" => parsed.lane = flag_value(&mut args, &flag)?,
            "--author" => parsed.author = flag_value(&mut args, &flag)?,
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
fn show_window(snapshot: Snapshot, service: rvw_core::Service, identity: Identity) {
    Application::new().run(move |cx: &mut App| {
        text_input::bind_keys(cx);
        let bounds = Bounds::centered(None, size(px(1440.), px(900.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("rvw".into()),
                ..TitlebarOptions::default()
            }),
            ..WindowOptions::default()
        };
        let opened = cx.open_window(options, move |_window, cx| {
            cx.new(|cx| Shell::new(snapshot, service, identity, cx))
        });
        if let Err(error) = opened {
            eprintln!("rvw-gui: {error}");
            std::process::exit(1);
        }
        cx.activate(true);
    });
}
