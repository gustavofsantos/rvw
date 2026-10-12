//! rvw-gui: the native review window. This first slice shows the shell with
//! the queue counters.

mod data;
mod shell;
mod theme;

use std::path::PathBuf;

use gpui::{
    px, size, App, AppContext, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions,
};

use crate::data::ShellData;
use crate::shell::Shell;

#[derive(Default)]
struct Args {
    workspace: Option<PathBuf>,
    db: Option<PathBuf>,
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

    let data = data::load(&service, &workspace)?;
    show_window(data);
    Ok(())
}

fn parse_args(mut args: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut parsed = Args::default();
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--workspace" => parsed.workspace = Some(flag_value(&mut args, &flag)?),
            "--db" => parsed.db = Some(flag_value(&mut args, &flag)?),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(parsed)
}

fn flag_value(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<PathBuf, String> {
    args.next()
        .map(PathBuf::from)
        .ok_or_else(|| format!("{flag} needs a value"))
}

/// Starts the application and opens the one window. Errors here happen after
/// the database is open, so they are reported from inside the app callback.
fn show_window(data: ShellData) {
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
        if let Err(error) = cx.open_window(options, |_window, cx| cx.new(|_| Shell::new(data))) {
            eprintln!("rvw-gui: {error}");
            std::process::exit(1);
        }
        cx.activate(true);
    });
}
