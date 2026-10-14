//! Mouse reports and the alternate-scroll wheel, as pure functions of a
//! [`Mode`]. Follows xterm: modes 1000 (click), 1002 (drag) and 1003
//! (motion), with the default byte encoding or SGR (1006). The UTF-8
//! encoding (1005) is not implemented: a program that asks for it gets the
//! default encoding, whose coordinates stop at column and row 222.

use crate::keys::Modifiers;
use crate::snapshot::Mode;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wheel {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseEventKind {
    Press(MouseButton),
    Release(MouseButton),
    /// The pointer moved, with a button held (a drag) or none.
    Motion(Option<MouseButton>),
    Wheel(Wheel),
}

/// A pointer event at a viewport cell, 0-based.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MouseEvent {
    pub kind: MouseEventKind,
    pub col: usize,
    pub row: usize,
    pub mods: Modifiers,
}

/// The most arrow keys one wheel event is turned into.
pub const MAX_WHEEL_LINES: usize = 64;
/// The largest coordinate (1-based) the default encoding carries.
const LEGACY_MAX: usize = 223;

fn button_code(button: MouseButton) -> u32 {
    match button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    }
}

/// The bytes that report `event`, or `None` when the program's mode does not
/// ask for it (or the cell does not fit the encoding).
#[must_use]
pub fn encode_mouse(event: MouseEvent, mode: &Mode) -> Option<Vec<u8>> {
    if !mode.mouse_reporting {
        return None;
    }
    let (base, release, motion) = match event.kind {
        MouseEventKind::Press(b) => (button_code(b), false, false),
        MouseEventKind::Release(b) => (button_code(b), true, false),
        MouseEventKind::Motion(Some(b)) if mode.mouse_drag || mode.mouse_motion => {
            (button_code(b), false, true)
        }
        MouseEventKind::Motion(None) if mode.mouse_motion => (3, false, true),
        MouseEventKind::Motion(_) => return None,
        MouseEventKind::Wheel(w) => (
            64 + match w {
                Wheel::Up => 0,
                Wheel::Down => 1,
                Wheel::Left => 2,
                Wheel::Right => 3,
            },
            false,
            false,
        ),
    };
    let held = u32::from(event.mods.shift) * 4
        + u32::from(event.mods.alt) * 8
        + u32::from(event.mods.ctrl) * 16;
    let motion = u32::from(motion) * 32;
    let (col, row) = (event.col + 1, event.row + 1);
    if mode.sgr_mouse {
        let end = if release { 'm' } else { 'M' };
        let code = base + held + motion;
        return Some(format!("\x1b[<{code};{col};{row}{end}").into_bytes());
    }
    if col > LEGACY_MAX || row > LEGACY_MAX {
        return None;
    }
    // The default encoding does not say which button was released.
    let code = if release { 3 } else { base } + held + motion;
    let byte = |n: usize| u8::try_from(32 + n).ok();
    Some(vec![
        0x1b,
        b'[',
        b'M',
        u8::try_from(32 + code).ok()?,
        byte(col)?,
        byte(row)?,
    ])
}

/// The arrow keys a wheel turns into on the alternate screen (alternate
/// scroll mode), `lines` of them. `None` when the mode does not do that:
/// not on the alternate screen, the mode is off, or the program takes mouse
/// reports itself.
#[must_use]
pub fn encode_alternate_scroll(wheel: Wheel, lines: usize, mode: &Mode) -> Option<Vec<u8>> {
    if !mode.alt_screen || !mode.alternate_scroll || mode.mouse_reporting || lines == 0 {
        return None;
    }
    let final_byte = match wheel {
        Wheel::Up => b'A',
        Wheel::Down => b'B',
        Wheel::Left | Wheel::Right => return None,
    };
    let intro = if mode.app_cursor { b'O' } else { b'[' };
    let mut out = Vec::new();
    for _ in 0..lines.min(MAX_WHEEL_LINES) {
        out.extend_from_slice(&[0x1b, intro, final_byte]);
    }
    Some(out)
}
