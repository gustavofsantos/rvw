//! Output that a terminal cannot be tricked by: control characters are shown
//! as U+FFFD, and a UTF-8 sequence split across writes is held back until whole.

use std::io::{self, Write};

/// The longest UTF-8 sequence, in bytes.
const UTF_MAX: usize = 4;

/// Reports whether a terminal would act on `c` instead of printing it: a C0
/// control other than newline and tab, DEL, or a C1 control. Text from agents
/// and files may carry these to hide text, rewrite lines, set the window title
/// or the clipboard.
#[must_use]
pub fn is_control(c: char) -> bool {
    (c < '\u{20}' && c != '\n' && c != '\t') || ('\u{7f}'..='\u{9f}').contains(&c)
}

/// `text` with every control character shown as U+FFFD.
#[must_use]
pub fn safe(text: &str) -> String {
    text.chars()
        .map(|c| {
            if is_control(c) {
                char::REPLACEMENT_CHARACTER
            } else {
                c
            }
        })
        .collect()
}

/// Wraps a writer so that everything written through it is [`safe`]. A rune
/// split across writes is held back until it is whole; [`Terminal::finish`]
/// writes whatever is left.
#[derive(Debug)]
pub struct Terminal<W: Write> {
    w: W,
    tail: Vec<u8>,
}

impl<W: Write> Terminal<W> {
    /// A terminal writing to `w`.
    pub fn new(w: W) -> Self {
        Terminal {
            w,
            tail: Vec::new(),
        }
    }

    /// Writes a rune left incomplete by the last write, as U+FFFD.
    ///
    /// # Errors
    ///
    /// Returns any error from the underlying writer.
    pub fn finish(&mut self) -> io::Result<()> {
        if self.tail.is_empty() {
            return Ok(());
        }
        let tail = std::mem::take(&mut self.tail);
        self.emit(&tail)
    }

    fn emit(&mut self, bytes: &[u8]) -> io::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        let text = safe(&decode_lossy(bytes));
        self.w.write_all(text.as_bytes())
    }
}

impl<W: Write> Write for Terminal<W> {
    fn write(&mut self, p: &[u8]) -> io::Result<usize> {
        let mut buf = std::mem::take(&mut self.tail);
        buf.extend_from_slice(p);
        // Hold back the last rune start when its sequence is still incomplete.
        let mut cut = buf.len();
        for i in (buf.len().saturating_sub(UTF_MAX)..buf.len()).rev() {
            if is_rune_start(buf[i]) {
                if !full_rune(&buf[i..]) {
                    cut = i;
                }
                break;
            }
        }
        self.tail = buf.split_off(cut);
        self.emit(&buf)?;
        Ok(p.len())
    }

    /// Flushes the inner writer. Held-back bytes stay held until [`Terminal::finish`].
    fn flush(&mut self) -> io::Result<()> {
        self.w.flush()
    }
}

fn is_rune_start(b: u8) -> bool {
    b & 0xC0 != 0x80
}

/// Reports whether `p` begins with a whole rune, or with bytes that can never
/// become one (so waiting for more input would not help). Mirrors Go's
/// `utf8.FullRune`.
fn full_rune(p: &[u8]) -> bool {
    let Some(&b0) = p.first() else {
        return false;
    };
    let need = match b0 {
        0xC2..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF4 => 4,
        _ => 1,
    };
    let n = p.len();
    if n >= need {
        return true;
    }
    let (lo, hi) = match b0 {
        0xE0 => (0xA0, 0xBF),
        0xED => (0x80, 0x9F),
        0xF0 => (0x90, 0xBF),
        0xF4 => (0x80, 0x8F),
        _ => (0x80, 0xBF),
    };
    if n > 1 && (p[1] < lo || hi < p[1]) {
        return true;
    }
    n > 2 && !(0x80..=0xBF).contains(&p[2])
}

/// Decodes UTF-8 the way Go's string conversion and range loop do: every
/// invalid byte becomes one U+FFFD.
fn decode_lossy(mut bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    loop {
        match std::str::from_utf8(bytes) {
            Ok(valid) => {
                out.push_str(valid);
                return out;
            }
            Err(e) => {
                let v = e.valid_up_to();
                out.push_str(&String::from_utf8_lossy(&bytes[..v]));
                out.push(char::REPLACEMENT_CHARACTER);
                bytes = &bytes[v + 1..];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shows_controls_even_when_a_rune_is_split_across_writes() {
        let mut out: Vec<u8> = Vec::new();
        let mut w = Terminal::new(&mut out);
        for chunk in [
            &b"a\x1b[8m\tb\n"[..],
            b"\xc2",
            b"\x9b\xe2\x80\x9c",
            b"\xe2\x80",
            b"\x9d",
            b"\x7fz",
        ] {
            w.write_all(chunk).unwrap();
        }
        w.finish().unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "a\u{fffd}[8m\tb\n\u{fffd}\u{201c}\u{201d}\u{fffd}z"
        );
    }

    #[test]
    fn holds_a_three_byte_rune_written_one_byte_at_a_time() {
        let mut out: Vec<u8> = Vec::new();
        let mut w = Terminal::new(&mut out);
        for b in "€".as_bytes() {
            w.write_all(std::slice::from_ref(b)).unwrap();
        }
        w.finish().unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "€");
    }

    #[test]
    fn finish_replaces_an_incomplete_rune_with_one_replacement_per_byte() {
        let mut out: Vec<u8> = Vec::new();
        let mut w = Terminal::new(&mut out);
        w.write_all(b"\xe2\x82").unwrap();
        assert!(w.w.is_empty(), "an incomplete rune must be held back");
        w.finish().unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "\u{fffd}\u{fffd}");
    }

    #[test]
    fn does_not_hold_back_invalid_continuation_bytes() {
        let mut out: Vec<u8> = Vec::new();
        let mut w = Terminal::new(&mut out);
        w.write_all(&[0x80; 5]).unwrap();
        assert_eq!(w.tail.len(), 0);
        drop(w);
        assert_eq!(String::from_utf8(out).unwrap(), "\u{fffd}".repeat(5));
    }

    #[test]
    fn finish_is_a_no_op_when_nothing_is_held() {
        let mut out: Vec<u8> = Vec::new();
        let mut w = Terminal::new(&mut out);
        w.write_all(b"plain").unwrap();
        w.finish().unwrap();
        w.flush().unwrap();
        drop(w);
        assert_eq!(out, b"plain");
    }

    #[test]
    fn is_control_covers_c0_del_and_c1_only() {
        assert!(is_control('\u{1b}'));
        assert!(is_control('\u{0}'));
        assert!(is_control('\u{7f}'));
        assert!(is_control('\u{9b}'));
        assert!(is_control('\u{9f}'));
        assert!(!is_control('\n'));
        assert!(!is_control('\t'));
        assert!(!is_control('\u{a0}'));
        assert!(!is_control('é'));
        assert!(!is_control('“'));
    }

    #[test]
    fn safe_replaces_controls_and_keeps_newline_and_tab() {
        assert_eq!(safe("a\x1b[31mb\tc\n"), "a\u{fffd}[31mb\tc\n");
        assert_eq!(safe("\u{9b}“x"), "\u{fffd}“x");
        assert_eq!(safe(""), "");
    }
}
