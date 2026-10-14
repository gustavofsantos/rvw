//! gpui keystrokes to what a terminal receives. The bytes are rvw-term's
//! (`encode_key`); this module only decides, per keystroke, whether it is a
//! key for the program, text for the input method, or neither.

use gpui::Keystroke;
use rvw_term::{Key, Modifiers};

/// What one keystroke becomes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Translated {
    /// Write these bytes; the keystroke is consumed.
    Send(Vec<u8>),
    /// Plain text: left to the platform, which delivers it, with dead keys,
    /// compose sequences and input methods applied, through the input handler.
    Text,
    /// Not for the terminal: command or super chords, and keys with no
    /// encoding. The keystroke goes on to the window.
    Pass,
}

/// Names gpui gives the keys that are not text.
fn named_key(name: &str) -> Option<Key> {
    Some(match name {
        "enter" => Key::Enter,
        "tab" => Key::Tab,
        "backspace" => Key::Backspace,
        "escape" => Key::Escape,
        "up" => Key::Up,
        "down" => Key::Down,
        "left" => Key::Left,
        "right" => Key::Right,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        "insert" => Key::Insert,
        "delete" => Key::Delete,
        _ => {
            let number: u8 = name.strip_prefix('f')?.parse().ok()?;
            if !(1..=20).contains(&number) {
                return None;
            }
            Key::F(number)
        }
    })
}

/// The character a key press types, for the encoder: the typed character
/// when it is printable, else the key's own (the ASCII one on layouts that
/// have none).
fn character(keystroke: &Keystroke) -> Option<char> {
    let typed = keystroke
        .key_char
        .as_deref()
        .and_then(single)
        .filter(|c| !c.is_control());
    if let Some(c) = typed {
        return Some(c);
    }
    if keystroke.key == "space" {
        return Some(' ');
    }
    single(&keystroke.key)
}

fn single(text: &str) -> Option<char> {
    let mut chars = text.chars();
    let first = chars.next()?;
    chars.next().is_none().then_some(first)
}

/// Translates `keystroke`. `app_cursor` is the terminal's application cursor
/// mode. On macOS, Option composes characters (Option-2 types a symbol on
/// many layouts), so Option with a character is text there; elsewhere Alt
/// is Meta and sends ESC before the key.
pub fn translate(keystroke: &Keystroke, app_cursor: bool, is_mac: bool) -> Translated {
    let held = keystroke.modifiers;
    // The command key never reaches a program, and the super key on Linux is
    // the window manager's.
    if held.platform {
        return Translated::Pass;
    }
    let mods = Modifiers {
        shift: held.shift,
        ctrl: held.control,
        alt: held.alt,
    };
    if let Some(key) = named_key(&keystroke.key) {
        return encoded(key, mods, app_cursor);
    }
    let Some(c) = character(keystroke) else {
        return Translated::Pass;
    };
    let composes = is_mac && mods.alt && !mods.ctrl && keystroke.key_char.is_some();
    if mods.ctrl || (mods.alt && !composes) {
        return encoded(Key::Char(c), mods, app_cursor);
    }
    if keystroke.key_char.is_some() {
        Translated::Text
    } else {
        Translated::Pass
    }
}

fn encoded(key: Key, mods: Modifiers, app_cursor: bool) -> Translated {
    match rvw_term::encode_key(key, mods, app_cursor) {
        Some(bytes) => Translated::Send(bytes),
        None => Translated::Pass,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::Modifiers as Held;

    fn press(key: &str, key_char: Option<&str>, held: Held) -> Keystroke {
        Keystroke {
            modifiers: held,
            key: key.to_owned(),
            key_char: key_char.map(str::to_owned),
        }
    }

    fn ctrl() -> Held {
        Held {
            control: true,
            ..Held::default()
        }
    }

    fn alt() -> Held {
        Held {
            alt: true,
            ..Held::default()
        }
    }

    fn send(bytes: &[u8]) -> Translated {
        Translated::Send(bytes.to_vec())
    }

    #[test]
    fn plain_and_shifted_characters_are_text_for_the_platform() {
        let none = Held::default();
        assert_eq!(
            translate(&press("a", Some("a"), none), false, false),
            Translated::Text
        );
        assert_eq!(
            translate(&press("a", Some("A"), Held::shift()), false, false),
            Translated::Text
        );
        assert_eq!(
            translate(&press("space", Some(" "), none), false, false),
            Translated::Text
        );
    }

    #[test]
    fn control_keys_are_control_bytes() {
        assert_eq!(
            translate(&press("c", None, ctrl()), false, false),
            send(&[3])
        );
        assert_eq!(
            translate(&press("c", Some("\u{3}"), ctrl()), false, false),
            send(&[3])
        );
        assert_eq!(
            translate(&press("space", Some(" "), ctrl()), false, false),
            send(&[0])
        );
        assert_eq!(
            translate(&press("[", None, ctrl()), false, false),
            send(&[0x1b])
        );
    }

    #[test]
    fn alt_is_meta_except_where_the_mac_composes() {
        assert_eq!(
            translate(&press("b", Some("b"), alt()), false, false),
            send(b"\x1bb")
        );
        assert_eq!(
            translate(&press("b", Some("\u{222b}"), alt()), false, true),
            Translated::Text
        );
        assert_eq!(
            translate(&press("left", None, alt()), false, true),
            send(b"\x1b[1;3D")
        );
    }

    #[test]
    fn named_keys_follow_the_cursor_mode() {
        let none = Held::default();
        assert_eq!(
            translate(&press("up", None, none), false, false),
            send(b"\x1b[A")
        );
        assert_eq!(
            translate(&press("up", None, none), true, false),
            send(b"\x1bOA")
        );
        assert_eq!(
            translate(&press("enter", Some("\n"), none), false, false),
            send(b"\r")
        );
        assert_eq!(
            translate(&press("tab", None, Held::shift()), false, false),
            send(b"\x1b[Z")
        );
        assert_eq!(
            translate(&press("backspace", None, none), false, false),
            send(&[0x7f])
        );
        assert_eq!(
            translate(&press("f5", None, none), false, false),
            send(b"\x1b[15~")
        );
        assert_eq!(
            translate(&press("f21", None, none), false, false),
            Translated::Pass
        );
    }

    #[test]
    fn command_chords_go_to_the_window() {
        let command = Held {
            platform: true,
            ..Held::default()
        };
        assert_eq!(
            translate(&press("j", None, command), false, true),
            Translated::Pass
        );
        assert_eq!(
            translate(&press("up", None, command), false, true),
            Translated::Pass
        );
    }

    #[test]
    fn a_key_with_no_text_and_no_name_passes() {
        assert_eq!(
            translate(&press("a", None, Held::default()), false, false),
            Translated::Pass
        );
        assert_eq!(
            translate(&press("shift", None, Held::default()), false, false),
            Translated::Pass
        );
    }
}
