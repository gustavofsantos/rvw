//! rvw-gui: the native review window: the queue and the review screens.

mod assets;
mod branch_picker;
mod compare_picker;
mod composer;
mod data;
mod diff_list;
mod diff_view;
mod draft_edit;
mod draft_panel;
mod file_view;
mod files_panel;
mod finder;
mod frame;
mod highlights;
mod key_strip;
mod outcome_flow;
mod outcome_screen;
mod outcome_story;
mod palette_overlay;
mod queue_screen;
mod review_screen;
mod seen;
mod shell;
mod split_view;
mod submit_dialog;
mod text_area;
mod text_area_element;
mod text_area_math;
mod text_input;
mod theme;
mod tools;
mod verdict;
mod window_chrome;

use std::path::PathBuf;

use gpui::{
    px, size, App, AppContext, Application, Bounds, TitlebarOptions, WindowBounds,
    WindowDecorations, WindowOptions,
};

use crate::data::{Identity, Snapshot};
use crate::seen::Seen;
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

    let db = match args.db {
        Some(path) => std::path::absolute(path).map_err(|e| e.to_string())?,
        None => rvw_store::default_path().map_err(|e| e.message)?,
    };
    let store = rvw_store::Store::open(&db).map_err(|e| e.message)?;
    let service = rvw_core::Service::new(Box::new(store));

    let workspace = launch_workspace(args.workspace, &service)?;

    let seen = Seen::load(&db);
    let snapshot = data::load(
        &service,
        &workspace,
        &args.lane,
        seen.ids_for(&workspace),
        None,
    )?;
    let identity = Identity {
        workspace,
        author: args.author,
        lane: args.lane,
    };
    show_window(snapshot, service, identity, seen);
    Ok(())
}

/// The workspace to open. `--workspace` is exact. Otherwise the working
/// directory is it, as from a shell; a desktop launcher starts in $HOME, which
/// is none, so that falls back to the queue with the most work.
fn launch_workspace(flag: Option<PathBuf>, service: &rvw_core::Service) -> Result<String, String> {
    if let Some(dir) = flag {
        return resolve_workspace(&dir);
    }
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    resolve_workspace(&cwd).or_else(|not_found| data::busiest_workspace(service).ok_or(not_found))
}

fn resolve_workspace(dir: &std::path::Path) -> Result<String, String> {
    Ok(rvw_git::workspace::resolve(dir)?
        .to_string_lossy()
        .into_owned())
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
fn show_window(snapshot: Snapshot, service: rvw_core::Service, identity: Identity, seen: Seen) {
    Application::new()
        .with_assets(assets::Assets)
        .run(move |cx: &mut App| {
            let installed = cx.text_system().all_font_names();
            theme::choose_fonts(&installed);
            theme::follow(cx.window_appearance());
            text_input::bind_keys(cx);
            text_area::bind_keys(cx);
            submit_dialog::bind_keys(cx);
            finder::bind_keys(cx);
            palette_overlay::bind_keys(cx);
            let bounds = Bounds::centered(None, size(px(1440.), px(900.)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("rvw".into()),
                    ..TitlebarOptions::default()
                }),
                // Where the compositor draws no frame, the app draws its own
                // controls (window_chrome.rs); macOS keeps its native bar.
                window_decorations: Some(WindowDecorations::Client),
                window_min_size: Some(size(px(900.), px(600.))),
                // Matches `StartupWMClass` and the icon name in
                // packaging/linux/rvw.desktop, so the shell finds the icon.
                app_id: Some("rvw".into()),
                ..WindowOptions::default()
            };
            let opened = cx.open_window(options, move |window, cx| {
                theme::follow(window.appearance());
                cx.new(|cx| Shell::new(snapshot, service, identity, seen, window, cx))
            });
            if let Err(error) = opened {
                eprintln!("rvw-gui: {error}");
                std::process::exit(1);
            }
            cx.activate(true);
        });
}
