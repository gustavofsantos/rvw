//! The terminal dock's pointer: selection, wheel, mouse reports to programs
//! and links. It translates gpui events and calls `rvw-term`; the decisions
//! that are pure (cells, click counts, wheel lines, link checks) live in
//! `terminal_pointer`.
//!
//! A program that turned mouse reporting on gets the plain mouse; Shift
//! keeps the mouse local, so text can still be selected. Reports use
//! `write_report`, which leaves the viewport and the selection alone.

use std::process::{Command, Stdio};

use gpui::{
    App, ClipboardItem, Context, Modifiers as Held, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, Point, ScrollDelta, ScrollWheelEvent,
};
use rvw_term::{MouseEvent, MouseEventKind, Scroll, UrlMatch, Wheel};

use crate::terminal_dock::TerminalView;
use crate::terminal_pointer::{self as pointer, Anchor, Hit};

/// What a held button is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Drag {
    /// Selecting text here.
    Select,
    /// The program has the button; releases go to it.
    Report(rvw_term::MouseButton),
}

/// Pointer state between events.
#[derive(Debug, Default)]
pub struct PointerState {
    pub drag: Option<Drag>,
    /// A press that has not moved yet: a click, or the start of a drag.
    pub anchor: Option<Anchor>,
    pub wheel: pointer::WheelAccumulator,
    /// The last cell reported as motion, so one cell is reported once.
    pub last_reported: Option<(usize, usize)>,
}

fn mods(held: Held) -> rvw_term::Modifiers {
    rvw_term::Modifiers {
        shift: held.shift,
        ctrl: held.control,
        alt: held.alt,
    }
}

fn reported(button: MouseButton) -> Option<rvw_term::MouseButton> {
    match button {
        MouseButton::Left => Some(rvw_term::MouseButton::Left),
        MouseButton::Middle => Some(rvw_term::MouseButton::Middle),
        MouseButton::Right => Some(rvw_term::MouseButton::Right),
        MouseButton::Navigate(_) => None,
    }
}

fn xy(point: Point<Pixels>) -> (f32, f32) {
    (f32::from(point.x), f32::from(point.y))
}

/// Puts selected text on the PRIMARY selection, where there is one.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn publish_primary(text: String, cx: &App) {
    cx.write_to_primary(ClipboardItem::new_string(text));
}

#[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
fn publish_primary(_: String, _: &App) {}

impl TerminalView {
    /// The cell under `position`; with `clamp`, the nearest one.
    fn hit(&self, position: Point<Pixels>, clamp: bool) -> Option<Hit> {
        let (bounds, width, height) = self.painted?;
        let snapshot = self.snapshot()?;
        pointer::cell_at(
            xy(position),
            xy(bounds.origin),
            (f32::from(width), f32::from(height)),
            (snapshot.cols, snapshot.rows),
            clamp,
        )
    }

    fn report(&self, kind: MouseEventKind, hit: Hit, held: Held) {
        let Some(live) = self.active_live() else {
            return;
        };
        let event = MouseEvent {
            kind,
            col: hit.point.col,
            row: hit.point.row,
            mods: mods(held),
        };
        if let Some(bytes) = rvw_term::encode_mouse(event, &live.snapshot.mode) {
            // A full queue drops the report; the program sees the next one.
            let _ = live.term.write_report(&bytes);
        }
    }

    /// A button went down. `hovered`: the pointer is over the grid and
    /// nothing is in front of it.
    pub fn mouse_down(&mut self, event: &MouseDownEvent, hovered: bool, cx: &mut Context<Self>) {
        if !hovered || self.exited() {
            return;
        }
        let Some(hit) = self.hit(event.position, false) else {
            return;
        };
        let Some(snapshot) = self.snapshot() else {
            return;
        };
        let reporting = snapshot.mode.mouse_reporting && !event.modifiers.shift;
        let url = (event.button == MouseButton::Left && event.modifiers.secondary())
            .then(|| snapshot.url_at(hit.point.row, hit.point.col))
            .flatten();
        // The click that opens a link is not a selection or a report.
        if let Some(url) = url {
            self.open_url(&url.url, cx);
            cx.stop_propagation();
            return;
        }
        if reporting {
            let Some(button) = reported(event.button) else {
                return;
            };
            self.report(MouseEventKind::Press(button), hit, event.modifiers);
            self.pointer.drag = Some(Drag::Report(button));
            self.pointer.last_reported = Some((hit.point.row, hit.point.col));
            cx.stop_propagation();
            return;
        }
        match event.button {
            MouseButton::Left => self.press_select(event, hit),
            MouseButton::Middle => self.paste_primary(cx),
            _ => {}
        }
        cx.stop_propagation();
    }

    fn press_select(&mut self, event: &MouseDownEvent, hit: Hit) {
        let kind = pointer::selection_kind(event.click_count, event.modifiers.alt);
        let Some(live) = self.active_live() else {
            return;
        };
        match kind {
            rvw_term::SelectionKind::Simple | rvw_term::SelectionKind::Block => {
                // A click clears; the selection starts when the pointer moves.
                live.term.clear_selection();
                self.pointer.anchor = Some(Anchor { hit, kind });
            }
            _ => {
                live.term.start_selection(kind, hit.point, hit.side);
                self.pointer.anchor = None;
            }
        }
        self.pointer.drag = Some(Drag::Select);
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    fn paste_primary(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_primary().and_then(|item| item.text()) {
            self.paste_text(&text, cx);
        }
    }

    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    fn paste_primary(&mut self, _: &mut Context<Self>) {}

    /// The pointer moved, over the grid or, during a drag, anywhere.
    pub fn mouse_move(&mut self, event: &MouseMoveEvent, hovered: bool, cx: &mut Context<Self>) {
        match self.pointer.drag {
            Some(Drag::Select) => self.drag_select(event, cx),
            Some(Drag::Report(button)) => {
                let Some(hit) = self.hit(event.position, true) else {
                    return;
                };
                let mode = self.snapshot().map(|shot| shot.mode);
                let wants = mode.is_some_and(|m| m.mouse_drag || m.mouse_motion);
                let cell = (hit.point.row, hit.point.col);
                if wants && self.pointer.last_reported != Some(cell) {
                    self.pointer.last_reported = Some(cell);
                    self.report(MouseEventKind::Motion(Some(button)), hit, event.modifiers);
                }
            }
            None => self.hover(event, hovered, cx),
        }
    }

    fn drag_select(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(hit) = self.hit(event.position, true) else {
            return;
        };
        let Some((bounds, _, _)) = self.painted else {
            return;
        };
        let anchor = self.pointer.anchor;
        if let Some(anchor) = anchor {
            if !anchor.moved(hit) {
                return;
            }
            self.pointer.anchor = None;
        }
        let Some(live) = self.active_live() else {
            return;
        };
        if let Some(anchor) = anchor {
            live.term
                .start_selection(anchor.kind, anchor.hit.point, anchor.hit.side);
        }
        // Past the top or bottom edge the viewport follows the drag.
        if event.position.y < bounds.top() {
            live.term.scroll(Scroll::Up(1));
        } else if event.position.y > bounds.bottom() {
            live.term.scroll(Scroll::Down(1));
        }
        live.term.update_selection(hit.point, hit.side);
        cx.notify();
    }

    fn hover(&mut self, event: &MouseMoveEvent, hovered: bool, cx: &mut Context<Self>) {
        let hit = if hovered {
            self.hit(event.position, false)
        } else {
            None
        };
        let mode = self.snapshot().map(|shot| shot.mode);
        if let (Some(hit), Some(mode)) = (hit, mode) {
            let cell = (hit.point.row, hit.point.col);
            let local = event.modifiers.shift;
            if mode.mouse_motion && !local && self.pointer.last_reported != Some(cell) {
                self.pointer.last_reported = Some(cell);
                self.report(MouseEventKind::Motion(None), hit, event.modifiers);
            }
        }
        let url: Option<UrlMatch> = match (hit, self.snapshot()) {
            (Some(hit), Some(shot)) if event.modifiers.secondary() => {
                shot.url_at(hit.point.row, hit.point.col)
            }
            _ => None,
        };
        if url != self.hover_url {
            self.hover_url = url;
            cx.notify();
        }
    }

    /// A button came up, anywhere in the window.
    pub fn mouse_up(&mut self, event: &MouseUpEvent, cx: &mut Context<Self>) {
        match self.pointer.drag {
            Some(Drag::Report(button)) if reported(event.button) == Some(button) => {
                self.pointer.drag = None;
                self.pointer.last_reported = None;
                if let Some(hit) = self.hit(event.position, true) {
                    self.report(MouseEventKind::Release(button), hit, event.modifiers);
                }
            }
            Some(Drag::Select) if event.button == MouseButton::Left => {
                self.pointer.drag = None;
                self.pointer.anchor = None;
                let text = self
                    .active_live()
                    .and_then(|live| live.term.selected_text());
                if let Some(text) = text {
                    publish_primary(text, cx);
                }
            }
            _ => {}
        }
    }

    /// The wheel or trackpad moved over the grid.
    pub fn wheel(&mut self, event: &ScrollWheelEvent, hovered: bool, cx: &mut Context<Self>) {
        if !hovered {
            return;
        }
        let height = self.painted.map_or(0., |(_, _, height)| f32::from(height));
        let lines = match event.delta {
            ScrollDelta::Lines(delta) => self.pointer.wheel.push_notches(delta.y),
            ScrollDelta::Pixels(delta) => {
                self.pointer.wheel.push_pixels(f32::from(delta.y), height)
            }
        };
        cx.stop_propagation();
        if lines == 0 {
            return;
        }
        // Positive is toward older output.
        let (wheel, count) = (
            if lines > 0 { Wheel::Up } else { Wheel::Down },
            lines.unsigned_abs() as usize,
        );
        let Some(live) = self.active_live() else {
            return;
        };
        let mode = live.snapshot.mode;
        let hit = self.hit(event.position, true);
        let reports = mode.mouse_reporting && !event.modifiers.shift && !self.exited();
        if reports {
            let Some(hit) = hit else {
                return;
            };
            let one = MouseEvent {
                kind: MouseEventKind::Wheel(wheel),
                col: hit.point.col,
                row: hit.point.row,
                mods: mods(event.modifiers),
            };
            if let Some(bytes) = rvw_term::encode_mouse(one, &mode) {
                let all: Vec<u8> = bytes
                    .iter()
                    .copied()
                    .cycle()
                    .take(bytes.len() * count)
                    .collect();
                let _ = live.term.write_report(&all);
            }
            return;
        }
        if !self.exited() {
            if let Some(bytes) = rvw_term::encode_alternate_scroll(wheel, count, &mode) {
                let _ = live.term.write_report(&bytes);
                return;
            }
        }
        live.term.scroll(match wheel {
            Wheel::Up => Scroll::Up(count),
            _ => Scroll::Down(count),
        });
        cx.notify();
    }

    /// Hands a link to the platform opener. Only on a click of the person's.
    fn open_url(&mut self, url: &str, cx: &mut Context<Self>) {
        if !pointer::openable(url) {
            return;
        }
        let spawned = Command::new(pointer::opener())
            .arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        match spawned {
            Ok(mut child) => {
                // Reaped off this thread, so the opener leaves no zombie.
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
            }
            Err(error) => {
                self.notice = Some(format!("The link did not open: {error}"));
                cx.notify();
            }
        }
    }
}
