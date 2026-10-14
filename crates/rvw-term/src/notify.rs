//! Desktop notifications a program asks its terminal for: OSC 9
//! (`ESC ] 9 ; text`, iTerm2 and Ghostty) and OSC 777
//! (`ESC ] 777 ; notify ; title ; body`, urxvt and Ghostty), ended by BEL or
//! `ESC \`. The emulation ignores both, so this scans the raw bytes the
//! emulation is given, across read boundaries. Nothing here paints or plays
//! anything: the owner takes [`Notification`]s and decides what to show.

/// The longest OSC payload looked at, in bytes; a longer one is dropped whole.
const MAX_PAYLOAD: usize = 4096;
/// The longest title or body kept, in characters.
pub(crate) const MAX_TEXT: usize = 256;

/// One request for the user's attention.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notification {
    /// Who is asking, as the program named itself. Empty for OSC 9.
    pub title: String,
    /// What it says. Never empty.
    pub body: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Ground,
    /// After `ESC` outside a string.
    Esc,
    /// Inside `ESC ]` ... before the terminator.
    Osc,
    /// After `ESC` inside the string: `\` ends it.
    OscEsc,
    /// A string past `MAX_PAYLOAD`, skipped to its end.
    Skip,
    SkipEsc,
}

/// Finds notification requests in a byte stream, one chunk at a time.
pub(crate) struct Scanner {
    state: State,
    payload: Vec<u8>,
}

impl Scanner {
    pub(crate) fn new() -> Self {
        Self {
            state: State::Ground,
            payload: Vec::new(),
        }
    }

    /// Feed the next bytes; `found` gets each complete request.
    pub(crate) fn feed(&mut self, bytes: &[u8], mut found: impl FnMut(Notification)) {
        for &byte in bytes {
            self.state = match (self.state, byte) {
                (State::Ground | State::Esc, 0x1b) => State::Esc,
                // A new `ESC ]` inside a string starts over, as a terminal does.
                (State::Esc | State::OscEsc, b']') => {
                    self.payload.clear();
                    State::Osc
                }
                (State::Ground | State::Esc, _) | (State::Skip, 0x07) | (State::SkipEsc, b'\\') => {
                    State::Ground
                }
                (State::Osc | State::OscEsc, 0x07) | (State::OscEsc, b'\\') => {
                    if let Some(notification) = parse(&self.payload) {
                        found(notification);
                    }
                    State::Ground
                }
                (State::Osc | State::OscEsc, 0x1b) => State::OscEsc,
                (State::Osc | State::OscEsc, _) if self.payload.len() >= MAX_PAYLOAD => {
                    self.payload.clear();
                    State::Skip
                }
                (State::Osc | State::OscEsc, _) => {
                    if self.state == State::OscEsc {
                        self.payload.push(0x1b);
                    }
                    self.payload.push(byte);
                    State::Osc
                }
                (State::Skip | State::SkipEsc, 0x1b) => State::SkipEsc,
                (State::Skip | State::SkipEsc, _) => State::Skip,
            };
        }
    }
}

fn parse(payload: &[u8]) -> Option<Notification> {
    let text = String::from_utf8_lossy(payload);
    if let Some(rest) = text.strip_prefix("9;") {
        // ConEmu's `9;4;...` (progress) and `9;9;...` (cwd) share the number.
        let numeric = rest.split(';').next().unwrap_or("");
        if !numeric.is_empty() && numeric.bytes().all(|b| b.is_ascii_digit()) && rest.contains(';')
        {
            return None;
        }
        return make("", rest);
    }
    let rest = text.strip_prefix("777;notify;")?;
    let (title, body) = rest.split_once(';')?;
    make(title, body)
}

fn make(title: &str, body: &str) -> Option<Notification> {
    let body = clean(body);
    if body.is_empty() {
        return None;
    }
    Some(Notification {
        title: clean(title),
        body,
    })
}

/// Control characters dropped, whitespace trimmed, length capped.
fn clean(text: &str) -> String {
    let kept: String = text.chars().filter(|c| !c.is_control()).collect();
    kept.trim().chars().take(MAX_TEXT).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(chunks: &[&[u8]]) -> Vec<Notification> {
        let mut scanner = Scanner::new();
        let mut out = Vec::new();
        for chunk in chunks {
            scanner.feed(chunk, |n| out.push(n));
        }
        out
    }

    fn note(title: &str, body: &str) -> Notification {
        Notification {
            title: title.into(),
            body: body.into(),
        }
    }

    #[test]
    fn osc_9_with_bel_or_st() {
        assert_eq!(scan(&[b"\x1b]9;done\x07"]), vec![note("", "done")]);
        assert_eq!(scan(&[b"\x1b]9;done\x1b\\"]), vec![note("", "done")]);
    }

    #[test]
    fn osc_777_has_a_title_and_a_body() {
        assert_eq!(
            scan(&[b"x\x1b]777;notify;Claude;needs input\x07y"]),
            vec![note("Claude", "needs input")]
        );
    }

    #[test]
    fn a_request_split_across_reads_is_found() {
        let found = scan(&[b"\x1b", b"]9;wa", b"it\x1b", b"\\"]);
        assert_eq!(found, vec![note("", "wait")]);
    }

    #[test]
    fn other_sequences_are_ignored() {
        assert!(scan(&[b"\x1b]0;title\x07\x1b]9;4;1;50\x07\x1b]9;9;/tmp\x07"]).is_empty());
        assert!(scan(&[b"\x1b[31mred\x1b[0m \x1b]777;preexec\x07"]).is_empty());
        assert!(scan(&[b"\x1b]9;\x07"]).is_empty());
    }

    #[test]
    fn a_digit_led_message_with_no_semicolon_is_text() {
        assert_eq!(scan(&[b"\x1b]9;42\x07"]), vec![note("", "42")]);
    }

    #[test]
    fn text_is_cleaned_and_capped() {
        let long = format!("\x1b]9;{}\x07", "a".repeat(MAX_PAYLOAD - 4));
        let found = scan(&[long.as_bytes()]);
        assert_eq!(found[0].body.chars().count(), MAX_TEXT);
        assert_eq!(
            scan(&[b"\x1b]9;a\x1b[31mb\n\x07"]),
            vec![note("", "a[31mb")]
        );
    }

    #[test]
    fn an_oversized_string_is_dropped_and_the_next_one_still_works() {
        let mut big = b"\x1b]9;".to_vec();
        big.extend(std::iter::repeat_n(b'a', MAX_PAYLOAD + 10));
        big.extend_from_slice(b"\x07\x1b]9;ok\x07");
        assert_eq!(scan(&[&big]), vec![note("", "ok")]);
    }
}
