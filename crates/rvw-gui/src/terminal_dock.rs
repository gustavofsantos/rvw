//! The terminal dock: the workspace's terminals in the window. `rvw-term` owns
//! the PTY and the emulation; this view paints its snapshot, forwards keys
//! and pastes, and keeps the PTY the size of the dock. No terminal logic.
//!
//! Output is picked up by a short timer that runs while a process is alive:
//! it drains the terminals' events and, when the shown one changed, takes one
//! snapshot and repaints, so there is at most one repaint per tick. A hidden
//! dock is looked at slowly, only to learn of ends, and keeps every process
//! and its output. rvw-term's channel cannot wake a task, which is why it is
//! a timer.

use std::path::PathBuf;
use std::time::Duration;

use gpui::prelude::*;
use gpui::{
    actions, div, fill, font, outline, point, px, relative, rgb, size, App, BorderStyle, Bounds,
    ClipboardItem, Context, CursorStyle, DispatchPhase, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, FocusHandle, Focusable, GlobalElementId, Hitbox, HitboxBehavior,
    KeyBinding, KeyDownEvent, LayoutId, ModifiersChangedEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, Rgba, ScrollWheelEvent, ShapedLine,
    StrikethroughStyle, Style, Subscription, Task, TextRun, UTF16Selection, UnderlineStyle, Window,
};
use rvw_term::{Color, CursorKind, Event, Scroll, Snapshot, SpawnOptions, Terminal, UrlMatch};
use rvw_view::resize;
use rvw_view::terminals::{Launch, TerminalId, Terminals};

use crate::icon::{self, Icon};
use crate::resize_handle::{self, Edge};
use crate::terminal_grid::{self, Layout, Run, Width};
use crate::terminal_input::{translate, Translated};
use crate::terminal_mouse::PointerState;
use crate::terminal_pointer::{self, SearchBar};
use crate::theme;

/// The key context of a focused terminal. Chords of rvw that a shell also
/// uses are bound outside it (`rvw_chord_context`).
pub const KEY_CONTEXT: &str = "Terminal";

/// The key context of the window's root. gpui evaluates a `!Terminal`
/// predicate as false against an empty context stack, so without a context
/// here the chords bound in `!Terminal` would never fire outside a terminal.
pub const APP_CONTEXT: &str = "Rvw";

const HEADER_HEIGHT: Pixels = px(32.);
const PADDING: Pixels = px(12.);
const FONT_SIZE: f32 = 13.;
const LINE_HEIGHT_RATIO: f32 = 1.4;
/// How often a shown, live terminal is looked at: one frame.
const FRAME: Duration = Duration::from_millis(16);
/// How often a hidden dock's terminals are looked at, to learn of their end.
const HIDDEN_FRAME: Duration = Duration::from_millis(250);
const TAB_MAX_WIDTH: Pixels = px(240.);

actions!(
    rvw_terminal,
    [
        ToggleTerminal,
        TerminalCopy,
        TerminalFind,
        TerminalPaste,
        NewTerminal,
        CloseTerminal,
        NextTerminal,
        PreviousTerminal,
        Terminal1,
        Terminal2,
        Terminal3,
        Terminal4,
        Terminal5,
        Terminal6,
        Terminal7,
        Terminal8,
        Terminal9
    ]
);

/// The context rvw's own chords (find a file, actions, workspaces, restore,
/// quit) are bound in. On macOS they use the command key, which no program
/// receives, so they act everywhere. Elsewhere they use Ctrl, which a shell
/// needs, so they act outside the terminal only, and the terminal has their
/// Ctrl-Shift forms (`bind_keys`).
pub fn rvw_chord_context() -> Option<&'static str> {
    if cfg!(target_os = "macos") {
        None
    } else {
        Some("!Terminal")
    }
}

/// The modifier prefix of the tab chords: the command key on macOS, and
/// Ctrl-Shift elsewhere so the shell keeps plain Ctrl keys.
const CHORD: &str = if cfg!(target_os = "macos") {
    "secondary-"
} else {
    "ctrl-shift-"
};

/// The dock's chords, and on Linux the Ctrl-Shift forms of rvw's chords that
/// act while a terminal has the keyboard.
pub fn bind_keys(cx: &mut App) {
    let ctx = Some(KEY_CONTEXT);
    let chord = |key: &str| format!("{CHORD}{key}");
    cx.bind_keys([
        KeyBinding::new(&chord("j"), ToggleTerminal, None),
        KeyBinding::new(&chord("t"), NewTerminal, None),
        KeyBinding::new(&chord("c"), TerminalCopy, ctx),
        KeyBinding::new(&chord("v"), TerminalPaste, ctx),
        KeyBinding::new(&chord("f"), TerminalFind, ctx),
        KeyBinding::new(&chord("w"), CloseTerminal, ctx),
        KeyBinding::new(&chord("1"), Terminal1, ctx),
        KeyBinding::new(&chord("2"), Terminal2, ctx),
        KeyBinding::new(&chord("3"), Terminal3, ctx),
        KeyBinding::new(&chord("4"), Terminal4, ctx),
        KeyBinding::new(&chord("5"), Terminal5, ctx),
        KeyBinding::new(&chord("6"), Terminal6, ctx),
        KeyBinding::new(&chord("7"), Terminal7, ctx),
        KeyBinding::new(&chord("8"), Terminal8, ctx),
        KeyBinding::new(&chord("9"), Terminal9, ctx),
    ]);
    #[cfg(target_os = "macos")]
    cx.bind_keys([
        KeyBinding::new("secondary-shift-]", NextTerminal, ctx),
        KeyBinding::new("secondary-shift-[", PreviousTerminal, ctx),
    ]);
    #[cfg(not(target_os = "macos"))]
    cx.bind_keys([
        KeyBinding::new("ctrl-shift-pagedown", NextTerminal, ctx),
        KeyBinding::new("ctrl-shift-pageup", PreviousTerminal, ctx),
        KeyBinding::new("ctrl-shift-p", crate::finder::OpenFinder, ctx),
        KeyBinding::new("ctrl-shift-k", crate::palette_overlay::OpenPalette, ctx),
        KeyBinding::new("ctrl-shift-o", crate::workspace_picker::OpenWorkspaces, ctx),
        KeyBinding::new("ctrl-shift-z", crate::changes_state::RestoreLatest, ctx),
        KeyBinding::new("ctrl-shift-q", crate::Quit, ctx),
    ]);
}

/// The width and line height of one cell of the terminal font.
fn cell_size(window: &Window) -> (Pixels, Pixels) {
    let text = window.text_system();
    let id = text.resolve_font(&font(theme::code_font()));
    let width = text
        .em_advance(id, px(FONT_SIZE))
        .unwrap_or(px(FONT_SIZE * 0.6));
    (width, px((FONT_SIZE * LINE_HEIGHT_RATIO).round()))
}

/// The grid a dock the width of the window fits, before the first paint
/// measures the real one.
fn first_grid(window: &Window, dock_height: f32) -> (u16, u16) {
    let (cell_width, cell_height) = cell_size(window);
    let width = window.viewport_size().width - PADDING * 2.;
    let height = px(dock_height) - HEADER_HEIGHT;
    terminal_grid::grid_size(
        f32::from(width),
        f32::from(height),
        f32::from(cell_width),
        f32::from(cell_height),
    )
    .unwrap_or((80, 24))
}

/// One running terminal and what the view keeps of it.
pub(crate) struct Live {
    id: TerminalId,
    pub(crate) term: Terminal,
    launch: Launch,
    pub(crate) snapshot: Snapshot,
    /// The screen changed since `snapshot` was taken.
    dirty: bool,
    /// The grid the PTY has.
    grid: (u16, u16),
}

/// The terminals of the workspace: their tabs and the active one painted.
pub struct TerminalView {
    focus: FocusHandle,
    /// The tabs: titles say the command and, once ended, how.
    model: Terminals,
    live: Vec<Live>,
    /// Why no terminal started.
    error: Option<String>,
    /// A line the header shows after the tabs: what a paste could not do.
    pub(crate) notice: Option<String>,
    workspace: PathBuf,
    /// Where the grid was painted and the size of a cell, for the input
    /// method's candidate window.
    pub(crate) painted: Option<(Bounds<Pixels>, Pixels, Pixels)>,
    /// Text an input method has composed and not committed.
    marked: Option<String>,
    /// The pointer between events: drags, wheel remainders.
    pub(crate) pointer: PointerState,
    /// The link under the pointer while the open-link key is held.
    pub(crate) hover_url: Option<UrlMatch>,
    /// The find bar over the grid.
    search: SearchBar,
    /// The dock's height the reviewer dragged it to; see `dock_height`.
    height: f32,
    next_id: u64,
    poll: Option<Task<()>>,
    /// Kills every process when the application quits.
    _quit: Subscription,
}

impl TerminalView {
    /// The dock's height in a window: the dragged one, kept within what the
    /// window allows now.
    fn dock_height(&self, window: &Window) -> f32 {
        resize::DOCK.clamp(self.height, f32::from(window.viewport_size().height))
    }

    pub fn new(workspace: PathBuf, cx: &mut Context<Self>) -> Self {
        let quit = cx.on_app_quit(|this, _| {
            // Dropping a terminal closes it: SIGHUP to its group, then
            // SIGKILL after the grace period, and the child is reaped. They
            // end together, so the wait is one grace period, not many.
            this.poll = None;
            let ends: Vec<_> = this
                .live
                .drain(..)
                .map(|live| std::thread::spawn(move || drop(live)))
                .collect();
            for end in ends {
                let _ = end.join();
            }
            async {}
        });
        Self {
            focus: cx.focus_handle(),
            model: Terminals::new(),
            live: Vec::new(),
            error: None,
            notice: None,
            workspace,
            painted: None,
            marked: None,
            pointer: PointerState::default(),
            hover_url: None,
            search: SearchBar::default(),
            height: resize::DOCK_DEFAULT,
            next_id: 1,
            poll: None,
            _quit: quit,
        }
    }

    /// Whether the dock is on screen.
    pub fn shown(&self) -> bool {
        self.model.dock_shown() || self.error.is_some()
    }

    /// Whether the terminal has the keyboard.
    pub fn has_focus(&self, window: &Window, cx: &App) -> bool {
        self.focus.contains_focused(window, cx)
    }

    pub(crate) fn active_live(&self) -> Option<&Live> {
        let id = self.model.active()?;
        self.live.iter().find(|live| live.id == id)
    }

    fn active_live_mut(&mut self) -> Option<&mut Live> {
        let id = self.model.active()?;
        self.live.iter_mut().find(|live| live.id == id)
    }

    /// The screen of the active terminal.
    pub(crate) fn snapshot(&self) -> Option<&Snapshot> {
        self.active_live().map(|live| &live.snapshot)
    }

    /// Shows the dock, starting a shell the first time, or hides it. Hidden
    /// terminals keep running; hiding is not closing.
    pub fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.error.take().is_some() {
            cx.notify();
            return;
        }
        if self.model.is_empty() {
            self.launch(&Launch::Shell, window, cx);
            return;
        }
        if self.model.dock_shown() {
            self.model.hide_dock();
        } else {
            self.model.show_dock();
            self.refresh_active();
            window.focus(&self.focus);
        }
        cx.notify();
    }

    /// Starts a terminal in the workspace root as a new, active tab and
    /// shows the dock.
    pub fn launch(&mut self, launch: &Launch, window: &mut Window, cx: &mut Context<Self>) {
        let height = self.dock_height(window);
        let (cols, rows) = first_grid(window, height);
        let options = SpawnOptions {
            command: launch.argv(),
            working_directory: Some(self.workspace.clone()),
            cols,
            rows,
            ..SpawnOptions::default()
        };
        match Terminal::spawn(options) {
            Ok(term) => {
                let id = TerminalId(self.next_id);
                self.next_id += 1;
                self.error = None;
                self.model.add(id, &launch.label());
                self.live.push(Live {
                    id,
                    snapshot: term.snapshot(),
                    term,
                    launch: launch.clone(),
                    dirty: false,
                    grid: (cols, rows),
                });
                self.marked = None;
                window.focus(&self.focus);
                self.start_poll(cx);
            }
            Err(error) => self.error = Some(format!("The terminal did not start: {error}")),
        }
        cx.notify();
    }

    /// Closes a tab. A running process is ended, off the UI thread; a
    /// finished one is just removed.
    fn close_tab(&mut self, id: TerminalId, cx: &mut Context<Self>) {
        if self.search.is_open() {
            self.close_search(cx);
        }
        self.model.close(id);
        if let Some(at) = self.live.iter().position(|live| live.id == id) {
            let gone = self.live.remove(at);
            // Closing waits for the process to end; not on this thread.
            std::thread::spawn(move || drop(gone));
        }
        self.marked = None;
        self.refresh_active();
        cx.notify();
    }

    fn switch_to(&mut self, id: TerminalId, window: &mut Window, cx: &mut Context<Self>) {
        if self.model.switch_to(id) {
            self.after_switch(window, cx);
        }
    }

    fn after_switch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        if self.search.is_open() {
            self.close_search(cx);
        }
        self.notice = None;
        self.refresh_active();
        window.focus(&self.focus);
        cx.notify();
    }

    fn switch_number(&mut self, number: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.model.switch_to_number(number) {
            self.after_switch(window, cx);
        }
    }

    /// Takes a fresh snapshot of the active terminal.
    fn refresh_active(&mut self) {
        if let Some(live) = self.active_live_mut() {
            live.snapshot = live.term.snapshot();
            live.dirty = false;
        }
    }

    /// Runs a finished terminal's command again, as a new tab at the end.
    fn restart(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(live) = self.active_live() else {
            return;
        };
        let (id, launch) = (live.id, live.launch.clone());
        self.close_tab(id, cx);
        self.launch(&launch, window, cx);
    }

    fn start_poll(&mut self, cx: &mut Context<Self>) {
        self.poll = Some(cx.spawn(async move |this, cx| loop {
            let wait = match this.read_with(cx, |view, _| view.model.dock_shown()) {
                Ok(true) => FRAME,
                Ok(false) => HIDDEN_FRAME,
                Err(_) => break,
            };
            cx.background_executor().timer(wait).await;
            match this.update(cx, Self::pump) {
                Ok(true) => {}
                _ => break,
            }
        }));
    }

    /// Takes what the terminals reported, records the ends, and repaints once
    /// if the shown screen or a tab changed. A hidden dock takes no
    /// snapshots. Returns whether to keep looking: while a process runs.
    fn pump(&mut self, cx: &mut Context<Self>) -> bool {
        let mut changed = false;
        for live in &mut self.live {
            for event in live.term.events().try_iter() {
                if event == Event::Changed {
                    live.dirty = true;
                }
            }
            if let Some(exit) = live.term.exit_status() {
                if self
                    .model
                    .get(live.id)
                    .is_some_and(rvw_view::terminals::Terminal::running)
                {
                    self.model.exited(live.id, exit.code);
                    live.dirty = true;
                    changed = true;
                }
            }
        }
        if self.model.dock_shown() {
            if let Some(live) = self.active_live_mut() {
                if live.dirty {
                    live.snapshot = live.term.snapshot();
                    live.dirty = false;
                    changed = true;
                }
            }
        }
        if changed {
            // Where the link was is no longer known to be where it is.
            self.hover_url = None;
            cx.notify();
        }
        !self.model.running().is_empty()
    }

    /// What the window asks before quitting, naming the running terminals;
    /// `None` when none runs. Looks at the processes now, not at the last
    /// event.
    pub fn quit_guard(&mut self, cx: &mut Context<Self>) -> Option<String> {
        self.pump(cx);
        self.model.quit_guard()
    }

    pub(crate) fn exited(&self) -> bool {
        self.active_live()
            .is_some_and(|live| live.term.exit_status().is_some())
    }

    fn write(&mut self, bytes: &[u8]) {
        if let Some(live) = self.active_live() {
            // A full queue or a closed terminal drops the input; there is no
            // one to tell, and the screen shows what arrived.
            let _ = live.term.write(bytes);
        }
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        if self.search.is_open() {
            self.search_key(event, cx);
            return;
        }
        if self.exited() {
            // A finished terminal takes no input: Enter runs it again.
            if keystroke.key == "enter" && keystroke.modifiers.number_of_modifiers() == 0 {
                self.restart(window, cx);
                cx.stop_propagation();
                cx.notify();
            }
            return;
        }
        let app_cursor = self.snapshot().is_some_and(|s| s.mode.app_cursor);
        if self.scroll_page(keystroke, cx) {
            return;
        }
        match translate(keystroke, app_cursor, cfg!(target_os = "macos")) {
            Translated::Send(bytes) => {
                self.write(&bytes);
                cx.stop_propagation();
            }
            // The platform hands the text to the input handler next.
            Translated::Text | Translated::Pass => {}
        }
    }

    fn paste(&mut self, _: &TerminalPaste, _: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        self.paste_text(&text, cx);
    }

    /// Pastes `text` as the program wants it, or into the find bar while
    /// that is open.
    pub(crate) fn paste_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.search.is_open() {
            if self.search.insert(text) {
                self.search_changed(cx);
            }
            return;
        }
        if self.exited() {
            return;
        }
        let bracketed = self
            .snapshot()
            .is_some_and(|shot| shot.mode.bracketed_paste);
        let bytes = rvw_term::encode_paste(text, bracketed);
        if bytes.len() > rvw_term::MAX_WRITE {
            self.notice = Some("Paste is too large".to_owned());
            cx.notify();
            return;
        }
        self.notice = None;
        self.write(&bytes);
    }

    /// Puts the selection on the clipboard. With none selected the chord is
    /// still taken, so it does not reach the program as Ctrl-C does.
    fn copy(&mut self, _: &TerminalCopy, _: &mut Window, cx: &mut Context<Self>) {
        let text = self
            .active_live()
            .and_then(|live| live.term.selected_text());
        if let Some(text) = text {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    /// Shift with `PageUp` or `PageDown` scrolls the scrollback by a screen.
    /// A full-screen program has no scrollback, so it gets the key.
    fn scroll_page(&mut self, keystroke: &gpui::Keystroke, cx: &mut Context<Self>) -> bool {
        let held = keystroke.modifiers;
        let plain_shift = held.shift && !held.control && !held.alt && !held.platform;
        let page = match keystroke.key.as_str() {
            "pageup" => Scroll::PageUp,
            "pagedown" => Scroll::PageDown,
            _ => return false,
        };
        if !plain_shift || self.snapshot().is_some_and(|s| s.mode.alt_screen) {
            return false;
        }
        if let Some(live) = self.active_live() {
            live.term.scroll(page);
        }
        cx.stop_propagation();
        cx.notify();
        true
    }

    /// Opens the find bar over the grid.
    fn find(&mut self, _: &TerminalFind, _: &mut Window, cx: &mut Context<Self>) {
        self.search.open();
        cx.notify();
    }

    /// While the bar is open it has the keys: Enter and Shift-Enter step,
    /// Escape closes, Backspace edits. Typed text arrives through the input
    /// handler; nothing reaches the program.
    fn search_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        match keystroke.key.as_str() {
            "escape" => self.close_search(cx),
            "enter" => {
                if let Some(live) = self.active_live() {
                    if keystroke.modifiers.shift {
                        live.term.search_prev();
                    } else {
                        live.term.search_next();
                    }
                }
                self.refresh_active();
                cx.notify();
            }
            "backspace" => {
                if self.search.backspace() {
                    self.search_changed(cx);
                }
            }
            _ => {
                // Chords that would be bytes for the program are not.
                if matches!(
                    translate(keystroke, false, cfg!(target_os = "macos")),
                    Translated::Send(_)
                ) {
                    cx.stop_propagation();
                }
                return;
            }
        }
        cx.stop_propagation();
    }

    /// The query changed: search again and show the first match.
    fn search_changed(&mut self, cx: &mut Context<Self>) {
        let query = self.search.query().to_owned();
        if let Some(live) = self.active_live() {
            if query.is_empty() {
                live.term.clear_search();
            } else if live.term.set_search(&query) {
                live.term.search_next();
            }
        }
        self.refresh_active();
        cx.notify();
    }

    /// Closes the bar and clears the search on every terminal.
    fn close_search(&mut self, cx: &mut Context<Self>) {
        self.search.close();
        for live in &self.live {
            live.term.clear_search();
        }
        self.refresh_active();
        cx.notify();
    }

    fn close_active(&mut self, _: &CloseTerminal, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.model.active() {
            self.close_tab(id, cx);
        }
    }

    fn next(&mut self, _: &NextTerminal, window: &mut Window, cx: &mut Context<Self>) {
        self.model.next();
        self.after_switch(window, cx);
    }

    fn previous(&mut self, _: &PreviousTerminal, window: &mut Window, cx: &mut Context<Self>) {
        self.model.prev();
        self.after_switch(window, cx);
    }

    /// Called by the grid's prepaint with where it sits: keeps the active PTY
    /// the size of the area.
    fn fit(&mut self, bounds: Bounds<Pixels>, cell: (Pixels, Pixels)) {
        self.painted = Some((bounds, cell.0, cell.1));
        let Some((cols, rows)) = terminal_grid::grid_size(
            f32::from(bounds.size.width),
            f32::from(bounds.size.height),
            f32::from(cell.0),
            f32::from(cell.1),
        ) else {
            return;
        };
        if let Some(live) = self.active_live_mut() {
            if (cols, rows) != live.grid && live.term.resize(cols, rows).is_ok() {
                live.grid = (cols, rows);
            }
        }
    }

    /// What floats over the grid: the find bar and the scrolled note.
    fn overlays(&self) -> Vec<gpui::AnyElement> {
        let mut out = Vec::new();
        let chip = || {
            div()
                .absolute()
                .right(px(24.))
                .px(px(8.))
                .py(px(4.))
                .rounded(theme::CONTROL_RADIUS)
                .border_1()
                .border_color(theme::hairline())
                .bg(theme::paper())
                .text_size(px(12.))
                .font_family(theme::code_font())
        };
        if self.search.is_open() {
            let status = self.snapshot().and_then(|shot| shot.search.as_ref());
            let query = self.search.query();
            let shown = if query.is_empty() {
                div().text_color(theme::faint()).child("Find")
            } else {
                div().text_color(theme::ink()).child(query.to_owned())
            };
            out.push(
                chip()
                    .top(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(shown)
                    .child(div().text_color(theme::ink()).child("\u{258f}"))
                    .child(
                        div()
                            .text_color(theme::muted())
                            .child(self.search.label(status)),
                    )
                    .into_any_element(),
            );
        }
        let offset = self.snapshot().map_or(0, |shot| shot.display_offset);
        if offset > 0 {
            out.push(
                chip()
                    .bottom(px(8.))
                    .text_color(theme::muted())
                    .child(terminal_pointer::scrolled_label(offset))
                    .into_any_element(),
            );
        }
        out
    }

    /// The tab row, then what the active terminal says about itself.
    fn header(&self, cx: &mut Context<Self>) -> gpui::Div {
        let active = self.model.active();
        let mut tabs = div().flex().items_center().gap(px(4.)).min_w(px(0.));
        for (index, tab) in self.model.list().iter().enumerate() {
            let id = tab.id;
            let selected = active == Some(id);
            let finished = !tab.running();
            let close = div()
                .id(("close-tab", index))
                .px(px(4.))
                .flex()
                .items_center()
                .rounded(theme::CONTROL_RADIUS)
                .text_color(theme::muted())
                .hover(|style| style.bg(theme::hairline()))
                .cursor_pointer()
                .child(icon::view(Icon::Close, icon::SMALL, theme::muted()))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this: &mut Self, _, _, cx| {
                    this.close_tab(id, cx);
                    cx.stop_propagation();
                }));
            tabs = tabs.child(
                div()
                    .id(("tab", index))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .h(px(24.))
                    .px(px(8.))
                    .max_w(TAB_MAX_WIDTH)
                    .rounded(theme::CONTROL_RADIUS)
                    .cursor_pointer()
                    .font_family(theme::code_font())
                    .text_color(if selected && !finished {
                        theme::ink()
                    } else if finished {
                        theme::faint()
                    } else {
                        theme::muted()
                    })
                    .when(selected, |d| d.bg(theme::hairline()))
                    .child(div().truncate().child(tab.title()))
                    .child(close)
                    .on_click(cx.listener(move |this: &mut Self, _, window, cx| {
                        this.switch_to(id, window, cx);
                    })),
            );
        }
        let plus = div()
            .id("new-terminal")
            .px(px(8.))
            .h(px(24.))
            .flex()
            .items_center()
            .rounded(theme::CONTROL_RADIUS)
            .text_color(theme::muted())
            .hover(|style| style.bg(theme::hairline()))
            .cursor_pointer()
            .child(icon::view(Icon::Add, icon::SIZE, theme::muted()))
            .on_click(|_, window, cx| window.dispatch_action(Box::new(NewTerminal), cx));
        let program = self
            .snapshot()
            .and_then(|shot| shot.title.clone())
            .filter(|title| !title.is_empty());
        let mut header = div()
            .flex()
            .items_center()
            .gap(px(12.))
            .h(HEADER_HEIGHT)
            .px(PADDING)
            .border_b_1()
            .border_color(theme::hairline())
            .text_size(px(12.))
            .child(tabs)
            .child(plus);
        if let Some(program) = program {
            header = header.child(div().text_color(theme::muted()).truncate().child(program));
        }
        if self.exited() {
            header = header.child(
                div()
                    .text_color(theme::muted())
                    .child("Enter runs it again"),
            );
        }
        if let Some(notice) = self.notice.as_ref() {
            header = header.child(div().text_color(theme::needs_you()).child(notice.clone()));
        }
        header
    }
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let height = self.dock_height(window);
        let overlays = self.overlays();
        let body = match self.error.as_ref() {
            Some(error) => div()
                .p(PADDING)
                .text_color(theme::needs_you())
                .child(error.clone()),
            None => div()
                .relative()
                .flex_1()
                .min_h(px(0.))
                .px(PADDING)
                .child(GridElement { view: cx.entity() })
                .children(overlays),
        };
        let mut dock = div()
            .flex()
            .flex_col()
            .w_full()
            .relative()
            .h(px(height))
            .flex_shrink_0()
            .bg(theme::paper())
            .border_t_1()
            .border_color(theme::hairline())
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key_down))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::find))
            .on_modifiers_changed(cx.listener(
                |this: &mut Self, event: &ModifiersChangedEvent, _, cx| {
                    if !event.modifiers.secondary() && this.hover_url.take().is_some() {
                        cx.notify();
                    }
                },
            ))
            .on_action(cx.listener(Self::close_active))
            .on_action(cx.listener(Self::next))
            .on_action(cx.listener(Self::previous))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this: &mut Self, _: &MouseDownEvent, window, _| {
                    window.focus(&this.focus);
                }),
            );
        dock = numbered_tabs(dock, cx);
        let handle = resize_handle::handle(
            "dock-resize",
            Edge::Top,
            height,
            resize::DOCK,
            |this: &mut Self, height, _| this.height = height,
            |this: &mut Self, _| this.height = resize::DOCK_DEFAULT,
            cx,
        );
        dock.child(self.header(cx)).child(body).child(handle)
    }
}

/// Wires `Terminal1`..`Terminal9` to the tab of that number.
fn numbered_tabs(dock: gpui::Div, cx: &mut Context<TerminalView>) -> gpui::Div {
    macro_rules! number {
        ($dock:ident, $($action:ident => $n:expr),+) => {
            $($dock = $dock.on_action(cx.listener(
                |this: &mut TerminalView, _: &$action, window, cx| {
                    this.switch_number($n, window, cx);
                },
            ));)+
        };
    }
    let mut dock = dock;
    number!(
        dock,
        Terminal1 => 1, Terminal2 => 2, Terminal3 => 3, Terminal4 => 4, Terminal5 => 5,
        Terminal6 => 6, Terminal7 => 7, Terminal8 => 8, Terminal9 => 9
    );
    dock
}

/// The input method's side: composed text is shown at the cursor and, once
/// committed, written to the program as typed.
impl EntityInputHandler for TerminalView {
    fn text_for_range(
        &mut self,
        _: std::ops::Range<usize>,
        _: &mut Option<std::ops::Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        None
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<std::ops::Range<usize>> {
        let marked = self.marked.as_ref()?;
        Some(0..marked.encode_utf16().count())
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if self.marked.take().is_some() {
            cx.notify();
        }
    }

    fn replace_text_in_range(
        &mut self,
        _: Option<std::ops::Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.search.is_open() {
            if self.search.insert(text) {
                self.search_changed(cx);
            }
            return;
        }
        if self.marked.take().is_some() {
            cx.notify();
        }
        if !self.exited() && !text.is_empty() {
            self.write(text.as_bytes());
        }
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<std::ops::Range<usize>>,
        new_text: &str,
        _: Option<std::ops::Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.search.is_open() {
            return;
        }
        self.marked = (!new_text.is_empty()).then(|| new_text.to_owned());
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _: std::ops::Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let (area, width, height) = self.painted?;
        let cursor = self.snapshot()?.cursor;
        let origin = point(
            area.left() + width * terminal_grid::cells(cursor.col),
            area.top() + height * terminal_grid::cells(cursor.row),
        );
        Some(Bounds::new(origin, size(width, height)))
    }

    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

/// What a terminal colour looks like in the theme.
fn resolve(color: Color) -> Rgba {
    match color {
        Color::Foreground => theme::ink(),
        Color::Background => theme::paper(),
        Color::Indexed(index) => match terminal_grid::palette_rgb(index) {
            Some((r, g, b)) => rgb((u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)),
            None => theme::ansi(index),
        },
        Color::Rgb(r, g, b) => rgb((u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)),
    }
}

/// The element that paints the grid. It sizes itself to the room it is
/// given; its prepaint tells the view, which resizes the PTY.
struct GridElement {
    view: Entity<TerminalView>,
}

struct Prepaint {
    fills: Vec<PaintQuad>,
    lines: Vec<(ShapedLine, Point<Pixels>)>,
    cursor: Option<PaintQuad>,
    marked: Option<(PaintQuad, ShapedLine, Point<Pixels>)>,
    /// Selection and search matches: under the text.
    marks: Vec<PaintQuad>,
    /// The link under the pointer.
    underlines: Vec<PaintQuad>,
    hitbox: Hitbox,
    pointer_style: CursorStyle,
    line_height: Pixels,
}

impl IntoElement for GridElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl GridElement {
    fn shape(run: &Run, cell: (Pixels, Pixels), window: &Window) -> ShapedLine {
        let style = run.style;
        let mut color = resolve(style.fg);
        if style.dim {
            color.a *= 0.6;
        }
        let mark = Some(color.into());
        let mut face = font(theme::code_font());
        if style.bold {
            face = face.bold();
        }
        if style.italic {
            face = face.italic();
        }
        let text_run = TextRun {
            len: run.text.len(),
            font: face,
            color: color.into(),
            background_color: None,
            underline: style.underline.then(|| UnderlineStyle {
                color: mark,
                thickness: px(1.),
                wavy: false,
            }),
            strikethrough: style.strikeout.then_some(StrikethroughStyle {
                color: mark,
                thickness: px(1.),
            }),
        };
        let force = match run.width {
            Width::Cells => Some(cell.0),
            Width::Wide => Some(cell.0 * 2.),
            Width::Natural => None,
        };
        window
            .text_system()
            .shape_line(run.text.clone().into(), px(FONT_SIZE), &[text_run], force)
    }

    /// Quads for the selection, the search matches (both under the text) and
    /// the link under the pointer (over it).
    fn decorations(
        snapshot: &Snapshot,
        link: Option<&UrlMatch>,
        origin: Point<Pixels>,
        cell: (Pixels, Pixels),
    ) -> (Vec<PaintQuad>, Vec<PaintQuad>) {
        let span = |row: usize, start: usize, end: usize| {
            Bounds::new(
                point(
                    origin.x + cell.0 * terminal_grid::cells(start),
                    origin.y + cell.1 * terminal_grid::cells(row),
                ),
                size(cell.0 * terminal_grid::cells(end + 1 - start), cell.1),
            )
        };
        let fits = |row: usize, start: usize, end: usize| {
            row < snapshot.rows && start <= end && end < snapshot.cols
        };
        let (mut marks, mut underlines) = (Vec::new(), Vec::new());
        for found in &snapshot.matches {
            if fits(found.row, found.start, found.end) {
                let colour = if found.current {
                    theme::terminal_match_current()
                } else {
                    theme::terminal_match()
                };
                marks.push(fill(span(found.row, found.start, found.end), colour));
            }
        }
        for selected in &snapshot.selection {
            if fits(selected.row, selected.start, selected.end) {
                marks.push(fill(
                    span(selected.row, selected.start, selected.end),
                    theme::text_selection(),
                ));
            }
        }
        if let Some(url) = link {
            for line in terminal_pointer::url_spans(url, snapshot.cols) {
                if fits(line.row, line.start, line.end) {
                    let mut under = span(line.row, line.start, line.end);
                    under.origin.y += cell.1 - px(1.);
                    under.size.height = px(1.);
                    underlines.push(fill(under, theme::ink()));
                }
            }
        }
        (marks, underlines)
    }

    fn cursor_quad(
        snapshot: &Snapshot,
        origin: Point<Pixels>,
        cell: (Pixels, Pixels),
        focused: bool,
    ) -> Option<PaintQuad> {
        let cursor = snapshot.cursor;
        if !cursor.visible || cursor.row >= snapshot.rows || cursor.col >= snapshot.cols {
            return None;
        }
        let at = point(
            origin.x + cell.0 * terminal_grid::cells(cursor.col),
            origin.y + cell.1 * terminal_grid::cells(cursor.row),
        );
        let colour = theme::ink();
        if !focused || cursor.kind == CursorKind::HollowBlock {
            return Some(outline(
                Bounds::new(at, size(cell.0, cell.1)),
                colour,
                BorderStyle::Solid,
            ));
        }
        match cursor.kind {
            // Drawn as an inverse cell by `terminal_grid::layout`.
            CursorKind::Block | CursorKind::HollowBlock => None,
            CursorKind::Underline => Some(fill(
                Bounds::new(point(at.x, at.y + cell.1 - px(2.)), size(cell.0, px(2.))),
                colour,
            )),
            CursorKind::Beam => Some(fill(Bounds::new(at, size(px(2.), cell.1)), colour)),
        }
    }
}

impl GridElement {
    /// The pointer's listeners for this frame. Moves and releases are taken
    /// anywhere, so a drag that leaves the grid keeps going.
    fn listen(&self, hitbox: &Hitbox, window: &mut Window) {
        let (view, hit) = (self.view.clone(), hitbox.clone());
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble {
                let hovered = hit.is_hovered(window);
                view.update(cx, |this, cx| {
                    // A click on the grid takes the keyboard, whatever else
                    // it does: the pointer handler stops the dock's own.
                    if hovered {
                        window.focus(&this.focus);
                    }
                    this.mouse_down(event, hovered, cx);
                });
            }
        });
        let (view, hit) = (self.view.clone(), hitbox.clone());
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble {
                let hovered = hit.is_hovered(window);
                view.update(cx, |this, cx| this.mouse_move(event, hovered, cx));
            }
        });
        let view = self.view.clone();
        window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
            if phase == DispatchPhase::Bubble {
                view.update(cx, |this, cx| this.mouse_up(event, cx));
            }
        });
        let (view, hit) = (self.view.clone(), hitbox.clone());
        window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble {
                let hovered = hit.should_handle_scroll(window);
                view.update(cx, |this, cx| this.wheel(event, hovered, cx));
            }
        });
    }
}

impl Element for GridElement {
    type RequestLayoutState = ();
    type PrepaintState = Prepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let cell = cell_size(window);
        self.view.update(cx, |view, _| view.fit(bounds, cell));
        let view = self.view.read(cx);
        let focused = view.has_focus(window, cx);
        let pointer_style = if view.hover_url.is_some() {
            CursorStyle::PointingHand
        } else if view.snapshot().is_some_and(|s| s.mode.mouse_reporting) {
            CursorStyle::Arrow
        } else {
            CursorStyle::IBeam
        };
        let mut prepaint = Prepaint {
            fills: Vec::new(),
            lines: Vec::new(),
            cursor: None,
            marked: None,
            marks: Vec::new(),
            underlines: Vec::new(),
            hitbox: window.insert_hitbox(bounds, HitboxBehavior::Normal),
            pointer_style,
            line_height: cell.1,
        };
        let Some(snapshot) = view.snapshot() else {
            return prepaint;
        };
        let layout: Layout = terminal_grid::layout(snapshot, focused);
        let at = |row: usize, col: usize| {
            point(
                bounds.left() + cell.0 * terminal_grid::cells(col),
                bounds.top() + cell.1 * terminal_grid::cells(row),
            )
        };
        for span in &layout.fills {
            prepaint.fills.push(fill(
                Bounds::new(
                    at(span.row, span.col),
                    size(cell.0 * terminal_grid::cells(span.cols), cell.1),
                ),
                resolve(span.color),
            ));
        }
        let (marks, underlines) =
            Self::decorations(snapshot, view.hover_url.as_ref(), bounds.origin, cell);
        prepaint.marks = marks;
        prepaint.underlines = underlines;
        for run in &layout.runs {
            prepaint
                .lines
                .push((Self::shape(run, cell, window), at(run.row, run.col)));
        }
        prepaint.cursor = Self::cursor_quad(snapshot, bounds.origin, cell, focused);
        if let Some(text) = view.marked.as_ref() {
            // Composed text sits at the cursor, underlined, until committed.
            let place = at(snapshot.cursor.row, snapshot.cursor.col);
            let run = Run {
                row: snapshot.cursor.row,
                col: snapshot.cursor.col,
                cols: text.chars().count(),
                text: text.clone(),
                style: terminal_grid::Style {
                    fg: Color::Foreground,
                    bg: Color::Background,
                    bold: false,
                    italic: false,
                    underline: true,
                    strikeout: false,
                    dim: false,
                },
                width: Width::Natural,
            };
            let line = Self::shape(&run, cell, window);
            let backing = fill(
                Bounds::new(place, size(line.width.max(cell.0), cell.1)),
                theme::paper(),
            );
            prepaint.marked = Some((backing, line, place));
        }
        prepaint
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.view.read(cx).focus.clone();
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.view.clone()),
            cx,
        );
        for quad in prepaint.fills.drain(..) {
            window.paint_quad(quad);
        }
        for quad in prepaint.marks.drain(..) {
            window.paint_quad(quad);
        }
        for (line, origin) in prepaint.lines.drain(..) {
            // A failed paint leaves this frame without that text; the next
            // frame draws it again.
            let _ = line.paint(origin, prepaint.line_height, window, cx);
        }
        for quad in prepaint.underlines.drain(..) {
            window.paint_quad(quad);
        }
        if let Some(cursor) = prepaint.cursor.take() {
            window.paint_quad(cursor);
        }
        window.set_cursor_style(prepaint.pointer_style, &prepaint.hitbox);
        self.listen(&prepaint.hitbox, window);
        if let Some((backing, line, origin)) = prepaint.marked.take() {
            window.paint_quad(backing);
            let _ = line.paint(origin, prepaint.line_height, window, cx);
        }
    }
}

/// The dock element to place under the review body.
pub fn dock(view: &Entity<TerminalView>) -> gpui::AnyElement {
    view.clone().into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    actions!(rvw_terminal_test, [Probe]);

    #[test]
    fn the_chord_contexts_parse() {
        // KeyBinding::new panics on a context it cannot parse.
        let _ = KeyBinding::new("ctrl-p", Probe, rvw_chord_context());
        let _ = KeyBinding::new("ctrl-shift-p", Probe, Some(KEY_CONTEXT));
    }

    #[test]
    fn the_chords_fire_outside_a_terminal_and_not_inside_one() {
        let predicate = gpui::KeyBindingContextPredicate::parse("!Terminal").unwrap();
        let app = gpui::KeyContext::parse(APP_CONTEXT).unwrap();
        let terminal = gpui::KeyContext::parse(KEY_CONTEXT).unwrap();
        assert!(predicate.depth_of(std::slice::from_ref(&app)).is_some());
        assert!(predicate.depth_of(&[app, terminal]).is_none());
    }

    #[test]
    fn ansi_and_cube_colours_resolve() {
        assert_eq!(resolve(Color::Rgb(1, 2, 3)), rgb(0x0001_0203));
        assert_eq!(resolve(Color::Indexed(196)), rgb(0x00ff_0000));
        assert_eq!(resolve(Color::Indexed(1)), theme::ansi(1));
        assert_eq!(resolve(Color::Foreground), theme::ink());
        assert_eq!(resolve(Color::Background), theme::paper());
    }
}
