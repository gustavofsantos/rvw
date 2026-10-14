//! The terminal dock: one terminal in the workspace window. `rvw-term` owns
//! the PTY and the emulation; this view paints its snapshot, forwards keys
//! and pastes, and keeps the PTY the size of the dock. No terminal logic.
//!
//! Output is picked up by a short timer that runs only while the dock is
//! shown and the process is alive: it drains the terminal's events and, when
//! something changed, takes one snapshot and repaints, so there is at most
//! one repaint per tick. rvw-term's channel cannot wake a task, which is why
//! it is a timer. A hidden dock keeps the process and its output.

use std::path::PathBuf;
use std::time::Duration;

use gpui::prelude::*;
use gpui::{
    actions, div, fill, font, outline, point, px, relative, rgb, size, App, BorderStyle, Bounds,
    Context, ElementId, ElementInputHandler, Entity, EntityInputHandler, FocusHandle, Focusable,
    GlobalElementId, KeyBinding, KeyDownEvent, LayoutId, MouseButton, MouseDownEvent, PaintQuad,
    Pixels, Point, Rgba, ShapedLine, StrikethroughStyle, Style, Subscription, Task, TextRun,
    UTF16Selection, UnderlineStyle, Window,
};
use rvw_term::{Color, CursorKind, Event, Snapshot, SpawnOptions, Terminal};
use rvw_view::terminals::{TerminalId, Terminals};

use crate::terminal_grid::{self, Layout, Run, Width};
use crate::terminal_input::{translate, Translated};
use crate::theme;

/// The key context of a focused terminal. Chords of rvw that a shell also
/// uses are bound outside it (`rvw_chord_context`).
pub const KEY_CONTEXT: &str = "Terminal";

const DOCK_HEIGHT: Pixels = px(300.);
const HEADER_HEIGHT: Pixels = px(32.);
const PADDING: Pixels = px(12.);
const FONT_SIZE: f32 = 13.;
const LINE_HEIGHT_RATIO: f32 = 1.4;
/// How often a shown, live terminal is looked at: one frame.
const FRAME: Duration = Duration::from_millis(16);
/// What the tab says. The shell is the user's own, from the account
/// database; its path is not read here.
const LABEL: &str = "shell";

actions!(rvw_terminal, [ToggleTerminal, TerminalCopy, TerminalPaste]);

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

/// The dock's chords, and on Linux the Ctrl-Shift forms of rvw's chords that
/// act while a terminal has the keyboard.
pub fn bind_keys(cx: &mut App) {
    #[cfg(target_os = "macos")]
    cx.bind_keys([
        KeyBinding::new("secondary-j", ToggleTerminal, None),
        KeyBinding::new("secondary-c", TerminalCopy, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-v", TerminalPaste, Some(KEY_CONTEXT)),
    ]);
    #[cfg(not(target_os = "macos"))]
    cx.bind_keys([
        KeyBinding::new("ctrl-shift-j", ToggleTerminal, None),
        KeyBinding::new("ctrl-shift-c", TerminalCopy, Some(KEY_CONTEXT)),
        KeyBinding::new("ctrl-shift-v", TerminalPaste, Some(KEY_CONTEXT)),
        KeyBinding::new("ctrl-shift-p", crate::finder::OpenFinder, Some(KEY_CONTEXT)),
        KeyBinding::new(
            "ctrl-shift-k",
            crate::palette_overlay::OpenPalette,
            Some(KEY_CONTEXT),
        ),
        KeyBinding::new(
            "ctrl-shift-o",
            crate::workspace_picker::OpenWorkspaces,
            Some(KEY_CONTEXT),
        ),
        KeyBinding::new(
            "ctrl-shift-z",
            crate::changes_state::RestoreLatest,
            Some(KEY_CONTEXT),
        ),
        KeyBinding::new("ctrl-shift-q", crate::Quit, Some(KEY_CONTEXT)),
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
fn first_grid(window: &Window) -> (u16, u16) {
    let (cell_width, cell_height) = cell_size(window);
    let width = window.viewport_size().width - PADDING * 2.;
    let height = DOCK_HEIGHT - HEADER_HEIGHT;
    terminal_grid::grid_size(
        f32::from(width),
        f32::from(height),
        f32::from(cell_width),
        f32::from(cell_height),
    )
    .unwrap_or((80, 24))
}

/// One terminal, its tab model and its view state.
pub struct TerminalView {
    focus: FocusHandle,
    /// The tab: its title says the command and, once it ended, how.
    model: Terminals,
    term: Option<Terminal>,
    snapshot: Option<Snapshot>,
    /// Why no terminal started.
    error: Option<String>,
    /// A line the header shows after the title: what a paste could not do.
    notice: Option<String>,
    workspace: PathBuf,
    /// The grid the PTY has, from the last paint.
    grid: (u16, u16),
    /// Where the grid was painted and the size of a cell, for the input
    /// method's candidate window.
    painted: Option<(Bounds<Pixels>, Pixels, Pixels)>,
    /// Text an input method has composed and not committed.
    marked: Option<String>,
    poll: Option<Task<()>>,
    /// Kills the process when the application quits.
    _quit: Subscription,
}

impl TerminalView {
    pub fn new(workspace: PathBuf, cx: &mut Context<Self>) -> Self {
        let quit = cx.on_app_quit(|this, _| {
            // Dropping the terminal closes it: SIGHUP to its group, then
            // SIGKILL after the grace period, and the child is reaped.
            this.poll = None;
            this.term = None;
            async {}
        });
        Self {
            focus: cx.focus_handle(),
            model: Terminals::new(),
            term: None,
            snapshot: None,
            error: None,
            notice: None,
            workspace,
            grid: (80, 24),
            painted: None,
            marked: None,
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

    /// Shows the dock, starting the terminal the first time, or hides it. A
    /// hidden terminal keeps running; hiding is not closing.
    pub fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.error.take().is_some() {
            cx.notify();
            return;
        }
        if self.term.is_none() {
            self.start(window, cx);
        } else if self.model.dock_shown() {
            self.model.hide_dock();
            self.poll = None;
        } else {
            self.model.show_dock();
            self.pump(cx);
            self.start_poll(cx);
        }
        if self.shown() {
            window.focus(&self.focus);
        }
        cx.notify();
    }

    fn start(&mut self, window: &Window, cx: &mut Context<Self>) {
        let (cols, rows) = first_grid(window);
        let options = SpawnOptions {
            command: None,
            working_directory: Some(self.workspace.clone()),
            cols,
            rows,
            ..SpawnOptions::default()
        };
        match Terminal::spawn(options) {
            Ok(term) => {
                self.model.add(TerminalId(term.id().0), LABEL);
                self.snapshot = Some(term.snapshot());
                self.term = Some(term);
                self.grid = (cols, rows);
                self.start_poll(cx);
            }
            Err(error) => self.error = Some(format!("The terminal did not start: {error}")),
        }
    }

    /// Replaces a finished terminal with a new shell in the same place.
    fn restart(&mut self, window: &Window, cx: &mut Context<Self>) {
        if let Some(mut old) = self.term.take() {
            old.kill();
            self.model.close(TerminalId(old.id().0));
        }
        self.poll = None;
        self.snapshot = None;
        self.marked = None;
        self.start(window, cx);
    }

    fn start_poll(&mut self, cx: &mut Context<Self>) {
        self.poll = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(FRAME).await;
            match this.update(cx, Self::pump) {
                Ok(true) => {}
                _ => break,
            }
        }));
    }

    /// Takes what the terminal reported and repaints once if the screen
    /// changed. Returns whether to keep looking: not once the process ended
    /// and its last output is shown.
    fn pump(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(term) = self.term.as_ref() else {
            return false;
        };
        let events: Vec<Event> = term.events().try_iter().collect();
        if events.is_empty() {
            return !self.exited();
        }
        let mut ended = false;
        for event in events {
            if let Event::Exited(exit) = event {
                self.model.exited(TerminalId(term.id().0), exit.code);
                ended = true;
            }
        }
        self.snapshot = Some(term.snapshot());
        cx.notify();
        !ended
    }

    fn exited(&self) -> bool {
        self.term
            .as_ref()
            .is_some_and(|term| term.exit_status().is_some())
    }

    fn write(&mut self, bytes: &[u8]) {
        if let Some(term) = self.term.as_ref() {
            // A full queue or a closed terminal drops the input; there is no
            // one to tell, and the screen shows what arrived.
            let _ = term.write(bytes);
        }
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        if self.exited() {
            // A finished terminal takes no input: Enter starts a new shell.
            if keystroke.key == "enter" && keystroke.modifiers.number_of_modifiers() == 0 {
                self.restart(window, cx);
                cx.stop_propagation();
                cx.notify();
            }
            return;
        }
        let app_cursor = self.snapshot.as_ref().is_some_and(|s| s.mode.app_cursor);
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
        if self.exited() {
            return;
        }
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        let bracketed = self
            .snapshot
            .as_ref()
            .is_some_and(|shot| shot.mode.bracketed_paste);
        let bytes = rvw_term::encode_paste(&text, bracketed);
        if bytes.len() > rvw_term::MAX_WRITE {
            self.notice = Some("Paste is too large".to_owned());
            cx.notify();
            return;
        }
        self.notice = None;
        self.write(&bytes);
    }

    /// Copy waits for the selection model. Until then the chord is taken, so
    /// it does not reach the program as Ctrl-C does.
    fn copy(_: &mut Self, _: &TerminalCopy, _: &mut Window, _: &mut Context<Self>) {}

    /// Called by the grid's prepaint with where it sits: keeps the PTY the
    /// size of the area.
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
        if (cols, rows) == self.grid {
            return;
        }
        if let Some(term) = self.term.as_ref() {
            if term.resize(cols, rows).is_ok() {
                self.grid = (cols, rows);
            }
        }
    }

    fn header(&self) -> gpui::Div {
        let title = self
            .model
            .active()
            .and_then(|id| self.model.get(id))
            .map_or_else(|| LABEL.to_owned(), rvw_view::terminals::Terminal::title);
        let program = self
            .snapshot
            .as_ref()
            .and_then(|shot| shot.title.clone())
            .filter(|title| !title.is_empty());
        let ended = self.exited();
        let mut header = div()
            .flex()
            .items_center()
            .gap(px(12.))
            .h(HEADER_HEIGHT)
            .px(PADDING)
            .border_b_1()
            .border_color(theme::hairline())
            .text_size(px(12.))
            .child(
                div()
                    .font_family(theme::code_font())
                    .text_color(if ended { theme::faint() } else { theme::ink() })
                    .child(title),
            );
        if let Some(program) = program {
            header = header.child(div().text_color(theme::muted()).truncate().child(program));
        }
        if ended {
            header = header.child(
                div()
                    .text_color(theme::muted())
                    .child("Enter starts a new shell"),
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match self.error.as_ref() {
            Some(error) => div()
                .p(PADDING)
                .text_color(theme::needs_you())
                .child(error.clone()),
            None => div()
                .flex_1()
                .min_h(px(0.))
                .px(PADDING)
                .child(GridElement { view: cx.entity() }),
        };
        div()
            .flex()
            .flex_col()
            .w_full()
            .h(DOCK_HEIGHT)
            .flex_shrink_0()
            .bg(theme::paper())
            .border_t_1()
            .border_color(theme::hairline())
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key_down))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::copy))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this: &mut Self, _: &MouseDownEvent, window, _| {
                    window.focus(&this.focus);
                }),
            )
            .child(self.header())
            .child(body)
    }
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
        let cursor = self.snapshot.as_ref()?.cursor;
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
        let mut prepaint = Prepaint {
            fills: Vec::new(),
            lines: Vec::new(),
            cursor: None,
            marked: None,
            line_height: cell.1,
        };
        let Some(snapshot) = view.snapshot.as_ref() else {
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
        for (line, origin) in prepaint.lines.drain(..) {
            // A failed paint leaves this frame without that text; the next
            // frame draws it again.
            let _ = line.paint(origin, prepaint.line_height, window, cx);
        }
        if let Some(cursor) = prepaint.cursor.take() {
            window.paint_quad(cursor);
        }
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
    fn ansi_and_cube_colours_resolve() {
        assert_eq!(resolve(Color::Rgb(1, 2, 3)), rgb(0x0001_0203));
        assert_eq!(resolve(Color::Indexed(196)), rgb(0x00ff_0000));
        assert_eq!(resolve(Color::Indexed(1)), theme::ansi(1));
        assert_eq!(resolve(Color::Foreground), theme::ink());
        assert_eq!(resolve(Color::Background), theme::paper());
    }
}
