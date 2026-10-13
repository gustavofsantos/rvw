//! Keys and pastes to the bytes a program on the PTY expects.
//!
//! No window toolkit here: the GUI maps its key events to [`Key`] and
//! [`Modifiers`], and writes the result with `Terminal::write`. The tables
//! follow xterm, as Zed's `terminal/src/mappings/keys.rs` does. The kitty
//! keyboard protocol is not implemented.

/// A key that is not just text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// A character key, as typed (already shifted: `A`, not `a` plus shift).
    Char(char),
    Enter,
    Tab,
    Backspace,
    Escape,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    /// A function key, `F(1)` to `F(20)`.
    F(u8),
}

/// The modifiers that change the bytes. The platform's command key is not
/// here: it never reaches a program.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

impl Modifiers {
    pub const NONE: Self = Self {
        shift: false,
        ctrl: false,
        alt: false,
    };

    fn is_none(self) -> bool {
        self == Self::NONE
    }

    /// The xterm modifier parameter: 1 plus shift, alt (2) and ctrl (4).
    fn param(self) -> u8 {
        1 + u8::from(self.shift) + 2 * u8::from(self.alt) + 4 * u8::from(self.ctrl)
    }
}

const ESC: u8 = 0x1b;

/// The bytes for a key press, or `None` when the key has no encoding (the
/// caller then drops it). `app_cursor` is the terminal's application cursor
/// mode (`Mode::app_cursor` of a snapshot). Alt sends an ESC prefix.
#[must_use]
pub fn encode_key(key: Key, mods: Modifiers, app_cursor: bool) -> Option<Vec<u8>> {
    match key {
        Key::Char(c) => encode_char(c, mods),
        Key::Enter => Some(alt_prefix(mods, vec![b'\r'])),
        Key::Escape => Some(alt_prefix(mods, vec![ESC])),
        Key::Tab if mods.shift => Some(vec![ESC, b'[', b'Z']),
        Key::Tab => Some(alt_prefix(mods, vec![b'\t'])),
        Key::Backspace => {
            let byte = if mods.ctrl { 0x08 } else { 0x7f };
            Some(alt_prefix(mods, vec![byte]))
        }
        Key::Up => Some(cursor_key(b'A', mods, app_cursor)),
        Key::Down => Some(cursor_key(b'B', mods, app_cursor)),
        Key::Right => Some(cursor_key(b'C', mods, app_cursor)),
        Key::Left => Some(cursor_key(b'D', mods, app_cursor)),
        Key::Home => Some(cursor_key(b'H', mods, app_cursor)),
        Key::End => Some(cursor_key(b'F', mods, app_cursor)),
        Key::PageUp => Some(tilde_key(5, mods)),
        Key::PageDown => Some(tilde_key(6, mods)),
        Key::Insert => Some(tilde_key(2, mods)),
        Key::Delete => Some(tilde_key(3, mods)),
        Key::F(n) => function_key(n, mods),
    }
}

fn alt_prefix(mods: Modifiers, mut bytes: Vec<u8>) -> Vec<u8> {
    if mods.alt {
        bytes.insert(0, ESC);
    }
    bytes
}

fn encode_char(c: char, mods: Modifiers) -> Option<Vec<u8>> {
    if !mods.ctrl {
        let mut buf = [0u8; 4];
        return Some(alt_prefix(
            mods,
            c.encode_utf8(&mut buf).as_bytes().to_vec(),
        ));
    }
    let byte = match c {
        'a'..='z' => c as u8 - b'a' + 1,
        'A'..='Z' => c as u8 - b'A' + 1,
        ' ' | '@' | '2' => 0,
        '[' | '3' => 0x1b,
        '\\' | '4' => 0x1c,
        ']' | '5' => 0x1d,
        '^' | '6' => 0x1e,
        '_' | '/' | '7' => 0x1f,
        '?' | '8' => 0x7f,
        _ => return None,
    };
    Some(alt_prefix(mods, vec![byte]))
}

/// Arrows, Home and End: `ESC O x` in application cursor mode, `ESC [ x`
/// otherwise, `ESC [ 1 ; m x` with modifiers.
fn cursor_key(final_byte: u8, mods: Modifiers, app_cursor: bool) -> Vec<u8> {
    if !mods.is_none() {
        let mut out = format!("\x1b[1;{}", mods.param()).into_bytes();
        out.push(final_byte);
        out
    } else if app_cursor {
        vec![ESC, b'O', final_byte]
    } else {
        vec![ESC, b'[', final_byte]
    }
}

/// `ESC [ n ~`, or `ESC [ n ; m ~` with modifiers.
fn tilde_key(n: u8, mods: Modifiers) -> Vec<u8> {
    if mods.is_none() {
        format!("\x1b[{n}~").into_bytes()
    } else {
        format!("\x1b[{n};{}~", mods.param()).into_bytes()
    }
}

fn function_key(n: u8, mods: Modifiers) -> Option<Vec<u8>> {
    if (1..=4).contains(&n) {
        let final_byte = b'P' + (n - 1);
        return Some(if mods.is_none() {
            vec![ESC, b'O', final_byte]
        } else {
            let mut out = format!("\x1b[1;{}", mods.param()).into_bytes();
            out.push(final_byte);
            out
        });
    }
    let code = match n {
        5 => 15,
        6 => 17,
        7 => 18,
        8 => 19,
        9 => 20,
        10 => 21,
        11 => 23,
        12 => 24,
        13 => 25,
        14 => 26,
        15 => 28,
        16 => 29,
        17 => 31,
        18 => 32,
        19 => 33,
        20 => 34,
        _ => return None,
    };
    Some(tilde_key(code, mods))
}

/// The bytes for pasted text. In bracketed paste mode the text is wrapped in
/// `ESC [ 200 ~` and `ESC [ 201 ~` with every ESC removed, so the text cannot
/// end the paste early. Otherwise line breaks become carriage returns, as
/// Enter sends.
#[must_use]
pub fn encode_paste(text: &str, bracketed: bool) -> Vec<u8> {
    if bracketed {
        let clean = text.replace('\x1b', "");
        let mut out = Vec::with_capacity(clean.len() + 12);
        out.extend_from_slice(b"\x1b[200~");
        out.extend_from_slice(clean.as_bytes());
        out.extend_from_slice(b"\x1b[201~");
        out
    } else {
        text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Modifiers = Modifiers::NONE;
    const CTRL: Modifiers = Modifiers {
        shift: false,
        ctrl: true,
        alt: false,
    };
    const ALT: Modifiers = Modifiers {
        shift: false,
        ctrl: false,
        alt: true,
    };
    const SHIFT: Modifiers = Modifiers {
        shift: true,
        ctrl: false,
        alt: false,
    };

    fn enc(key: Key, mods: Modifiers, app: bool) -> Vec<u8> {
        encode_key(key, mods, app).expect("encodes")
    }

    #[test]
    fn text_is_utf8_and_alt_prefixes_escape() {
        assert_eq!(enc(Key::Char('a'), NONE, false), b"a");
        assert_eq!(enc(Key::Char('é'), NONE, false), "é".as_bytes());
        assert_eq!(enc(Key::Char('x'), ALT, false), b"\x1bx");
    }

    #[test]
    fn control_characters() {
        assert_eq!(enc(Key::Char('c'), CTRL, false), [3]);
        assert_eq!(enc(Key::Char('C'), CTRL, false), [3]);
        assert_eq!(enc(Key::Char('['), CTRL, false), [0x1b]);
        assert_eq!(enc(Key::Char(' '), CTRL, false), [0]);
        assert_eq!(enc(Key::Char('/'), CTRL, false), [0x1f]);
        assert_eq!(encode_key(Key::Char('1'), CTRL, false), None);
    }

    #[test]
    fn editing_keys() {
        assert_eq!(enc(Key::Enter, NONE, false), b"\r");
        assert_eq!(enc(Key::Enter, ALT, false), b"\x1b\r");
        assert_eq!(enc(Key::Tab, NONE, false), b"\t");
        assert_eq!(enc(Key::Tab, SHIFT, false), b"\x1b[Z");
        assert_eq!(enc(Key::Backspace, NONE, false), [0x7f]);
        assert_eq!(enc(Key::Backspace, CTRL, false), [0x08]);
        assert_eq!(enc(Key::Backspace, ALT, false), [0x1b, 0x7f]);
        assert_eq!(enc(Key::Escape, NONE, false), [0x1b]);
        assert_eq!(enc(Key::Delete, NONE, false), b"\x1b[3~");
        assert_eq!(enc(Key::PageUp, NONE, false), b"\x1b[5~");
        assert_eq!(enc(Key::PageDown, CTRL, false), b"\x1b[6;5~");
    }

    #[test]
    fn arrows_follow_application_cursor_mode() {
        assert_eq!(enc(Key::Up, NONE, false), b"\x1b[A");
        assert_eq!(enc(Key::Up, NONE, true), b"\x1bOA");
        assert_eq!(enc(Key::Home, NONE, false), b"\x1b[H");
        assert_eq!(enc(Key::Home, NONE, true), b"\x1bOH");
        assert_eq!(enc(Key::End, NONE, true), b"\x1bOF");
    }

    #[test]
    fn modified_arrows_use_the_parameter_form_in_both_modes() {
        assert_eq!(enc(Key::Left, CTRL, false), b"\x1b[1;5D");
        assert_eq!(enc(Key::Left, CTRL, true), b"\x1b[1;5D");
        assert_eq!(enc(Key::Right, SHIFT, false), b"\x1b[1;2C");
        assert_eq!(enc(Key::Right, ALT, false), b"\x1b[1;3C");
        let all = Modifiers {
            shift: true,
            ctrl: true,
            alt: true,
        };
        assert_eq!(enc(Key::Down, all, false), b"\x1b[1;8B");
    }

    #[test]
    fn function_keys() {
        assert_eq!(enc(Key::F(1), NONE, false), b"\x1bOP");
        assert_eq!(enc(Key::F(4), NONE, false), b"\x1bOS");
        assert_eq!(enc(Key::F(1), SHIFT, false), b"\x1b[1;2P");
        assert_eq!(enc(Key::F(5), NONE, false), b"\x1b[15~");
        assert_eq!(enc(Key::F(12), NONE, false), b"\x1b[24~");
        assert_eq!(enc(Key::F(12), CTRL, false), b"\x1b[24;5~");
        assert_eq!(encode_key(Key::F(0), NONE, false), None);
        assert_eq!(encode_key(Key::F(21), NONE, false), None);
    }

    #[test]
    fn bracketed_paste_wraps_and_strips_escapes() {
        assert_eq!(encode_paste("ls\n", true), b"\x1b[200~ls\n\x1b[201~");
        assert_eq!(
            encode_paste("a\x1b[201~b", true),
            b"\x1b[200~a[201~b\x1b[201~"
        );
    }

    #[test]
    fn plain_paste_turns_line_breaks_into_carriage_returns() {
        assert_eq!(encode_paste("a\nb\r\nc", false), b"a\rb\rc");
    }
}
