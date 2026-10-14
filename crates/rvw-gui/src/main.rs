//! rvw-gui: the native review window: the queue and the review screens.

mod assets;
mod branch_picker;
mod changes_screen;
mod changes_state;
mod checkpoint_ui;
mod code_scroll;
#[cfg(test)]
mod commit_loop_tests;
mod compare_picker;
mod composer;
mod data;
mod diff_list;
mod diff_view;
mod draft_edit;
mod draft_panel;
mod external_tools;
mod file_view;
mod files_panel;
mod finder;
mod frame;
mod git_actions;
mod git_overlay;
mod git_tools;
mod highlights;
mod history_actions;
mod history_files;
mod history_screen;
mod history_state;
mod icon;
mod last_workspace;
mod live;
mod outcome_flow;
mod outcome_screen;
mod outcome_story;
mod palette_overlay;
mod queue_screen;
mod resize_handle;
mod review_screen;
mod rewrite_card;
mod shell;
mod split_view;
mod submit_dialog;
mod terminal_dock;
mod terminal_grid;
mod terminal_input;
mod terminal_launcher;
mod terminal_mouse;
mod terminal_pointer;
mod text_area;
mod text_area_element;
mod text_area_math;
mod text_input;
mod theme;
mod tools;
mod verdict;
mod window_chrome;
mod workspace_picker;
mod worktree_ui;

use std::path::{Path, PathBuf};

use gpui::{
    actions, px, size, App, AppContext, Application, Bounds, KeyBinding, Menu, MenuItem,
    TitlebarOptions, WindowBounds, WindowDecorations, WindowOptions,
};

use crate::data::{Identity, Snapshot};
use crate::shell::Shell;

actions!(rvw_app, [Quit]);

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

    let workspace = launch_workspace(args.workspace.as_deref(), &db, &service)?;
    last_workspace::save(&db, &workspace).ok();

    let snapshot = data::load(&service, &workspace, &args.lane, None)?;
    let identity = Identity {
        workspace,
        author: args.author,
        lane: args.lane,
    };
    show_window(snapshot, service, identity, db);
    Ok(())
}

/// The workspace to open. `--workspace` is exact, and an error when it is not
/// a repository. Otherwise `last_workspace::choose` decides: the working
/// directory when it is a repository, then the workspace the window showed
/// last, then the queue with the most work. A desktop launcher starts in
/// $HOME, which is no repository, so the last two apply there.
fn launch_workspace(
    flag: Option<&Path>,
    db: &Path,
    service: &rvw_core::Service,
) -> Result<String, String> {
    if let Some(dir) = flag {
        return resolve_workspace(dir);
    }
    let cwd_repo = std::env::current_dir()
        .map_err(|e| e.to_string())
        .and_then(|cwd| resolve_workspace(&cwd));
    let not_found = cwd_repo.clone().err().unwrap_or_default();
    last_workspace::choose(
        cwd_repo.ok(),
        last_workspace::load(db),
        is_workspace_root,
        || data::busiest_workspace(service),
    )
    .ok_or(not_found)
}

/// Whether `path` is the root of a repository, as recorded: resolving it
/// gives back the same path. A path inside another repository is not.
fn is_workspace_root(path: &str) -> bool {
    resolve_workspace(Path::new(path)).as_deref() == Ok(path)
}

fn resolve_workspace(dir: &Path) -> Result<String, String> {
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
fn show_window(snapshot: Snapshot, service: rvw_core::Service, identity: Identity, db: PathBuf) {
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
            changes_state::bind_keys(cx);
            terminal_dock::bind_keys(cx);
            frame::bind_keys(cx);
            workspace_picker::bind_keys(cx);
            quit_with_the_window(cx);
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
                let service = std::sync::Arc::new(service);
                cx.new(|cx| Shell::new(snapshot, service, identity, db, window, cx))
            });
            if let Err(error) = opened {
                eprintln!("rvw-gui: {error}");
                std::process::exit(1);
            }
            cx.activate(true);
        });
}

/// The app is its one window: closing it quits, as the Close control does
/// where rvw draws its own. macOS would otherwise keep the process running
/// with no window, and a click on the Dock icon would reopen nothing. ⌘Q
/// and the app menu's Quit quit too.
fn quit_with_the_window(cx: &mut App) {
    cx.on_window_closed(|cx| {
        if cx.windows().is_empty() {
            cx.quit();
        }
    })
    .detach();
    // Reached when no focused view took the action (the app menu): the
    // window asks first, as it does for its own chord.
    cx.on_action(|_: &Quit, cx| {
        let shell = cx.windows().into_iter().find_map(|w| w.downcast::<Shell>());
        match shell {
            Some(window) => {
                let asked = window.update(cx, Shell::request_quit);
                if asked.is_err() {
                    cx.quit();
                }
            }
            None => cx.quit(),
        }
    });
    cx.bind_keys([KeyBinding::new(
        "secondary-q",
        Quit,
        terminal_dock::rvw_chord_context(),
    )]);
    cx.set_menus(vec![Menu {
        name: "rvw".into(),
        items: vec![MenuItem::action("Quit rvw", Quit)],
    }]);
}
