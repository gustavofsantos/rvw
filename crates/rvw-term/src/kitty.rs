//! The kitty keyboard protocol: keys as `CSI ... u`, and the letter and tilde
//! forms of the functional keys, once a program has set its flags.
//!
//! Ported from Ghostty (`src/input/key_encode.zig`), MIT, Copyright (c) 2024 Mitchell Hashimoto, Ghostty contributors.
//!
//! rvw's keys differ from Ghostty's in three ways, and the port follows each:
//! - a [`Key::Char`] is the character typed, already shifted, so its code is
//!   the US-layout unshifted key ([`unshifted`]) and no base-layout alternate
//!   is reported (rvw does not know the physical key);
//! - there are no release or repeat events, so only presses are encoded;
//! - Option is Alt on every platform, so Alt always prevents associated text.

use crate::keys::{legacy, Key, Modifiers};

/// The kitty keyboard protocol flags a program has set with `CSI = u` or
/// pushed with `CSI > u`. The bits are the ones the protocol numbers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KittyFlags(u8);

impl KittyFlags {
    /// 0b1: keys with no text, and text keys with Ctrl or Alt, are escape
    /// codes. Enter, Tab and Backspace keep their bytes unmodified.
    pub const DISAMBIGUATE: Self = Self(1);
    /// 0b10: press, repeat and release are told apart. Only presses come
    /// from rvw, so this changes the form of the press, not which keys send.
    pub const REPORT_EVENT_TYPES: Self = Self(1 << 1);
    /// 0b100: a text key with Shift also reports its shifted character.
    pub const REPORT_ALTERNATE_KEYS: Self = Self(1 << 2);
    /// 0b1000: every key is an escape code, Enter, Tab and Backspace included.
    pub const REPORT_ALL_KEYS: Self = Self(1 << 3);
    /// 0b10000: the text a key makes is reported too. It needs
    /// [`Self::REPORT_ALL_KEYS`] to be sent with an escape code.
    pub const REPORT_ASSOCIATED_TEXT: Self = Self(1 << 4);

    /// True when no flag is set, so the legacy encoding applies.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// True when every flag in `other` is set here.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// The flags set in either.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// Encodes a press of `key` with `mods` under the protocol `flags`, which
/// must not be empty. `None` when the key has no encoding.
pub(crate) fn encode(key: Key, mods: Modifiers, flags: KittyFlags) -> Option<Vec<u8>> {
    let report_all = flags.contains(KittyFlags::REPORT_ALL_KEYS);
    // With nothing held, Enter, Tab and Backspace keep their legacy bytes, and
    // text is sent as text, unless every key is reported. A `Key::Char` is
    // already shifted, so Shift is consumed by the text it made, as Ghostty's
    // `consumed_mods` has it from the platform: Shift alone still types.
    if !report_all {
        match key {
            Key::Enter | Key::Tab | Key::Backspace if mods == Modifiers::NONE => {
                return legacy(key, mods, false);
            }
            Key::Char(c) if !mods.ctrl && !mods.alt && !is_control(c) => {
                return Some(c.to_string().into_bytes());
            }
            _ => {}
        }
    }

    let (code, final_byte) = match key {
        Key::Char(c) => (u32::from(unshifted(c)), b'u'),
        _ => named(key)?,
    };
    let mut seq = Sequence {
        key: code,
        final_byte,
        mods: mods.param(),
        event: flags.contains(KittyFlags::REPORT_EVENT_TYPES),
        shifted: None,
        text: String::new(),
    };
    if let Key::Char(c) = key {
        if flags.contains(KittyFlags::REPORT_ALTERNATE_KEYS) && mods.shift && u32::from(c) != code {
            seq.shifted = Some(u32::from(c));
        }
        if flags.contains(KittyFlags::REPORT_ASSOCIATED_TEXT) && !(mods.alt || mods.ctrl) {
            seq.text = c.to_string();
        }
    }
    Some(seq.encode())
}

/// The code and final byte of a key the protocol names. Printable keys are
/// handled by the caller.
fn named(key: Key) -> Option<(u32, u8)> {
    Some(match key {
        Key::Char(_) => return None,
        Key::Escape => (27, b'u'),
        Key::Enter => (13, b'u'),
        Key::Tab => (9, b'u'),
        Key::Backspace => (127, b'u'),
        Key::Insert => (2, b'~'),
        Key::Delete => (3, b'~'),
        Key::Left => (1, b'D'),
        Key::Right => (1, b'C'),
        Key::Up => (1, b'A'),
        Key::Down => (1, b'B'),
        Key::PageUp => (5, b'~'),
        Key::PageDown => (6, b'~'),
        Key::Home => (1, b'H'),
        Key::End => (1, b'F'),
        Key::F(n) => return function_key(n),
    })
}

fn function_key(n: u8) -> Option<(u32, u8)> {
    Some(match n {
        1 => (1, b'P'),
        2 => (1, b'Q'),
        3 => (13, b'~'),
        4 => (1, b'S'),
        5 => (15, b'~'),
        6 => (17, b'~'),
        7 => (18, b'~'),
        8 => (19, b'~'),
        9 => (20, b'~'),
        10 => (21, b'~'),
        11 => (23, b'~'),
        12 => (24, b'~'),
        // F13 to F20 are in the protocol's private-use block, from 57376.
        13..=20 => (57_376 + u32::from(n - 13), b'u'),
        _ => return None,
    })
}

/// The character a text key reports as its code: the unshifted one on a US
/// layout. Other layouts are not known here, so a character with no US key
/// is its own code.
fn unshifted(c: char) -> char {
    match c {
        'A'..='Z' => c.to_ascii_lowercase(),
        '!' => '1',
        '@' => '2',
        '#' => '3',
        '$' => '4',
        '%' => '5',
        '^' => '6',
        '&' => '7',
        '*' => '8',
        '(' => '9',
        ')' => '0',
        '_' => '-',
        '+' => '=',
        '{' => '[',
        '}' => ']',
        '|' => '\\',
        ':' => ';',
        '"' => '\'',
        '<' => ',',
        '>' => '.',
        '?' => '/',
        '~' => '`',
        _ => c,
    }
}

/// ASCII control characters and DEL, as Ghostty's `isControl`.
fn is_control(c: char) -> bool {
    c < ' ' || c == '\x7f'
}

/// One CSI sequence of the protocol:
/// `CSI key[:shifted] ; mods[:event] ; text u`, or `CSI 1 ; mods letter` and
/// `CSI key ; mods ~` for the keys that use those final bytes. Ported from
/// Ghostty's `KittySequence`.
#[derive(Debug, PartialEq, Eq)]
struct Sequence {
    key: u32,
    final_byte: u8,
    /// The modifier value the protocol sends: 1 for none, then 1 more for
    /// each of shift (1), alt (2) and ctrl (4).
    mods: u8,
    /// Report the event type. rvw sends presses, which the `u` and `~` forms
    /// leave out, and the letter form writes as `:1`.
    event: bool,
    /// The shifted character of a text key, as the first alternate.
    shifted: Option<u32>,
    /// The text the key makes, written as code points; control characters
    /// are dropped.
    text: String,
}

impl Sequence {
    fn encode(&self) -> Vec<u8> {
        let text = if matches!(self.final_byte, b'u' | b'~') {
            self.full()
        } else {
            self.letter()
        };
        text.into_bytes()
    }

    fn full(&self) -> String {
        let mut out = format!("\x1b[{}", self.key);
        if let Some(shifted) = self.shifted {
            out.push(':');
            out.push_str(&shifted.to_string());
        }

        let mut emit_prior = false;
        if self.mods > 1 {
            out.push(';');
            out.push_str(&self.mods.to_string());
            emit_prior = true;
        }

        let mut codes = self.text.chars().filter(|&c| !is_control(c)).map(u32::from);
        if let Some(first) = codes.next() {
            if !emit_prior {
                out.push(';');
            }
            out.push(';');
            out.push_str(&first.to_string());
            for code in codes {
                out.push(':');
                out.push_str(&code.to_string());
            }
        }

        out.push(char::from(self.final_byte));
        out
    }

    fn letter(&self) -> String {
        let final_byte = char::from(self.final_byte);
        if self.event {
            format!("\x1b[1;{}:1{final_byte}", self.mods)
        } else if self.mods > 1 {
            format!("\x1b[1;{}{final_byte}", self.mods)
        } else {
            format!("\x1b[{final_byte}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Modifiers = Modifiers::NONE;
    const SHIFT: Modifiers = Modifiers {
        shift: true,
        ctrl: false,
        alt: false,
    };
    const ALT: Modifiers = Modifiers {
        shift: false,
        ctrl: false,
        alt: true,
    };
    const CTRL: Modifiers = Modifiers {
        shift: false,
        ctrl: true,
        alt: false,
    };

    const DIS: KittyFlags = KittyFlags::DISAMBIGUATE;
    const ALTERNATES: KittyFlags = DIS.union(KittyFlags::REPORT_ALTERNATE_KEYS);
    const ALL_KEYS: KittyFlags = DIS.union(KittyFlags::REPORT_ALL_KEYS);
    const EVERYTHING: KittyFlags = ALL_KEYS
        .union(KittyFlags::REPORT_EVENT_TYPES)
        .union(KittyFlags::REPORT_ALTERNATE_KEYS)
        .union(KittyFlags::REPORT_ASSOCIATED_TEXT);

    fn enc(key: Key, mods: Modifiers, flags: KittyFlags) -> String {
        let bytes = encode(key, mods, flags).expect("encodes");
        String::from_utf8(bytes).expect("utf8")
    }

    fn csi(key: u32, final_byte: u8) -> Sequence {
        Sequence {
            key,
            final_byte,
            mods: 1,
            event: false,
            shifted: None,
            text: String::new(),
        }
    }

    #[test]
    fn modifier_values_are_one_plus_the_bits() {
        let value = |shift, ctrl, alt| Modifiers { shift, ctrl, alt }.param();
        assert_eq!(value(false, false, false), 1);
        assert_eq!(value(true, false, false), 2);
        assert_eq!(value(false, false, true), 3);
        assert_eq!(value(true, false, true), 4);
        assert_eq!(value(false, true, false), 5);
        assert_eq!(value(true, true, false), 6);
        assert_eq!(value(false, true, true), 7);
        assert_eq!(value(true, true, true), 8);
    }

    // Ported from Ghostty's "kitty: plain text" and "kitty: enter, backspace, tab".
    #[test]
    fn plain_text_stays_text_with_disambiguate() {
        assert_eq!(enc(Key::Char('a'), NONE, DIS), "a");
        assert_eq!(enc(Key::Char('é'), NONE, DIS), "é");
        assert_eq!(enc(Key::Enter, NONE, DIS), "\r");
        assert_eq!(enc(Key::Backspace, NONE, DIS), "\x7f");
        assert_eq!(enc(Key::Tab, NONE, DIS), "\t");
    }

    // Ported from Ghostty's "kitty: enter with all flags" and
    // "kitty: backspace (DECBKM reset) (report_all: true)".
    #[test]
    fn enter_backspace_and_tab_are_escape_codes_when_all_keys_are_reported() {
        assert_eq!(enc(Key::Enter, NONE, EVERYTHING), "\x1b[13u");
        assert_eq!(enc(Key::Backspace, NONE, EVERYTHING), "\x1b[127u");
        assert_eq!(enc(Key::Tab, NONE, ALL_KEYS), "\x1b[9u");
    }

    // Ported from Ghostty's "kitty: shift+backspace emits CSI u",
    // "kitty: alt+backspace emits CSI u", "kitty: shift+enter emits CSI u"
    // and "kitty: shift+tab emits CSI u".
    #[test]
    fn modified_enter_backspace_and_tab_are_escape_codes() {
        assert_eq!(enc(Key::Backspace, SHIFT, DIS), "\x1b[127;2u");
        assert_eq!(enc(Key::Backspace, ALT, DIS), "\x1b[127;3u");
        assert_eq!(enc(Key::Enter, SHIFT, DIS), "\x1b[13;2u");
        assert_eq!(enc(Key::Tab, SHIFT, DIS), "\x1b[9;2u");
    }

    #[test]
    fn escape_and_ctrl_or_alt_text_keys_are_escape_codes() {
        assert_eq!(enc(Key::Escape, NONE, DIS), "\x1b[27u");
        assert_eq!(enc(Key::Char('c'), CTRL, DIS), "\x1b[99;5u");
        assert_eq!(enc(Key::Char('x'), ALT, DIS), "\x1b[120;3u");
    }

    // Ported from Ghostty's "kitty: shift+a on US keyboard", whose fixture
    // has no consumed Shift. From a keyboard Shift is consumed by the text,
    // so the key types unless every key is reported.
    #[test]
    fn shifted_text_key_reports_its_shifted_character() {
        assert_eq!(enc(Key::Char('A'), SHIFT, DIS), "A");
        assert_eq!(enc(Key::Char('A'), SHIFT, ALTERNATES), "A");
        let all = ALTERNATES.union(KittyFlags::REPORT_ALL_KEYS);
        assert_eq!(enc(Key::Char('A'), SHIFT, all), "\x1b[97:65;2u");
    }

    // Ported from Ghostty's "kitty: report alternates colon (shift+';')".
    #[test]
    fn shifted_symbol_reports_the_us_key_it_comes_from() {
        assert_eq!(enc(Key::Char(':'), SHIFT, EVERYTHING), "\x1b[59:58;2;58u");
    }

    // Ported from Ghostty's "kitty: report associated", "kitty: report
    // associated with modifiers" and "kitty: report associated with alt text
    // on macOS with alt" (the Alt case, which holds on every platform here).
    #[test]
    fn associated_text_follows_the_modifiers() {
        assert_eq!(enc(Key::Char('J'), SHIFT, EVERYTHING), "\x1b[106:74;2;74u");
        assert_eq!(enc(Key::Char('j'), CTRL, EVERYTHING), "\x1b[106;5u");
        assert_eq!(enc(Key::Char('w'), ALT, EVERYTHING), "\x1b[119;3u");
    }

    // rvw has no separate text for a key: with every key reported, a plain
    // text key carries its own text as the associated text.
    #[test]
    fn plain_text_key_is_reported_with_its_text_when_all_keys_are_reported() {
        assert_eq!(enc(Key::Char('a'), NONE, EVERYTHING), "\x1b[97;;97u");
    }

    // Ported from Ghostty's "kitty: delete" and "kitty: alternates omit
    // control characters".
    #[test]
    fn tilde_keys() {
        assert_eq!(enc(Key::Delete, NONE, DIS), "\x1b[3~");
        assert_eq!(enc(Key::Delete, NONE, EVERYTHING), "\x1b[3~");
        assert_eq!(enc(Key::PageUp, CTRL, DIS), "\x1b[5;5~");
    }

    // Ported from Ghostty's "kitty: up arrow with utf8" (rvw has no arrow text).
    #[test]
    fn arrows_use_the_letter_form_and_ignore_cursor_mode() {
        assert_eq!(enc(Key::Up, NONE, DIS), "\x1b[A");
        assert_eq!(enc(Key::Left, CTRL, DIS), "\x1b[1;5D");
        assert_eq!(enc(Key::Home, NONE, DIS), "\x1b[H");
    }

    #[test]
    fn function_keys_come_from_the_protocol_table() {
        assert_eq!(enc(Key::F(1), NONE, DIS), "\x1b[P");
        assert_eq!(enc(Key::F(1), SHIFT, DIS), "\x1b[1;2P");
        assert_eq!(enc(Key::F(5), NONE, DIS), "\x1b[15~");
        assert_eq!(enc(Key::F(13), NONE, DIS), "\x1b[57376u");
        assert_eq!(encode(Key::F(21), NONE, DIS), None);
    }

    // Ported from Ghostty's "KittySequence: backspace" (the press cases).
    #[test]
    fn sequence_backspace() {
        assert_eq!(csi(127, b'u').encode(), b"\x1b[127u");
        assert_eq!(
            Sequence {
                mods: 2,
                ..csi(127, b'u')
            }
            .encode(),
            b"\x1b[127;2u"
        );
        // A press is left out of the CSI u form.
        assert_eq!(
            Sequence {
                event: true,
                ..csi(127, b'u')
            }
            .encode(),
            b"\x1b[127u"
        );
    }

    // Ported from Ghostty's "KittySequence: text" (the press cases).
    #[test]
    fn sequence_text() {
        let text = |mods, text: &str| {
            Sequence {
                mods,
                text: text.to_owned(),
                ..csi(127, b'u')
            }
            .encode()
        };
        assert_eq!(text(1, "A"), b"\x1b[127;;65u");
        assert_eq!(text(2, "A"), b"\x1b[127;2;65u");
    }

    // Ported from Ghostty's "KittySequence: text with control characters".
    #[test]
    fn sequence_text_skips_control_characters() {
        let text = |text: &str| {
            Sequence {
                text: text.to_owned(),
                ..csi(127, b'u')
            }
            .encode()
        };
        assert_eq!(text("\n"), b"\x1b[127u");
        assert_eq!(text("A\n"), b"\x1b[127;;65u");
    }

    // Ported from Ghostty's "KittySequence: special no mods" and
    // "KittySequence: special mods only".
    #[test]
    fn sequence_letter_form() {
        assert_eq!(csi(1, b'A').encode(), b"\x1b[A");
        assert_eq!(
            Sequence {
                mods: 2,
                ..csi(1, b'A')
            }
            .encode(),
            b"\x1b[1;2A"
        );
    }

    // The press analogue of Ghostty's "KittySequence: special mods and event",
    // which is a release there.
    #[test]
    fn sequence_letter_form_with_the_press_event() {
        assert_eq!(
            Sequence {
                mods: 2,
                event: true,
                ..csi(1, b'A')
            }
            .encode(),
            b"\x1b[1;2:1A"
        );
    }

    #[test]
    fn sequence_alternate_is_the_first_after_the_key() {
        assert_eq!(
            Sequence {
                mods: 2,
                shifted: Some(65),
                ..csi(97, b'u')
            }
            .encode(),
            b"\x1b[97:65;2u"
        );
    }

    #[test]
    fn unshifted_maps_us_shifted_symbols_and_capitals() {
        assert_eq!(unshifted('A'), 'a');
        assert_eq!(unshifted(':'), ';');
        assert_eq!(unshifted('?'), '/');
        assert_eq!(unshifted('é'), 'é');
    }
}
