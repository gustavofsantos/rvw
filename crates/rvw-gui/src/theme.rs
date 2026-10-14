//! Design tokens from docs/gui-v1-design.md. `rgb` is not `const`, so colours
//! are functions; sizes are constants.
//!
//! The palette is complete before every screen uses it, hence the allow.
#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use gpui::{px, rgb, rgba, Pixels, Rgba, SharedString, WindowAppearance};
use rvw_view::highlight::Class;

pub const WINDOW_RADIUS: Pixels = px(14.);
pub const CONTROL_RADIUS: Pixels = px(8.);
pub const ROW_RADIUS: Pixels = px(6.);

pub const TOP_BAR_HEIGHT: Pixels = px(48.);
pub const RAIL_WIDTH: Pixels = px(56.);
pub const LEFT_PANEL_WIDTH: Pixels = px(300.);
pub const RIGHT_PANEL_WIDTH: Pixels = px(340.);
pub const KEY_STRIP_HEIGHT: Pixels = px(56.);
pub const PANEL_PADDING: Pixels = px(24.);

pub const BASE_FONT_SIZE: f32 = 13.5;
pub const CODE_FONT_SIZE: f32 = 12.5;
pub const KEY_FONT_SIZE: f32 = 11.;

const UI_PREFERENCES: &[&str] = &[
    "Hanken Grotesk",
    "Inter",
    "SF Pro Text",
    "Noto Sans",
    "Ubuntu",
    "DejaVu Sans",
    "Liberation Sans",
];
const UI_FALLBACK: &str = "Noto Sans";
const CODE_PREFERENCES: &[&str] = &[
    "JetBrains Mono",
    "SF Mono",
    "Menlo",
    "Cascadia Mono",
    "Fira Code",
    "Noto Sans Mono",
    "DejaVu Sans Mono",
    "Liberation Mono",
    "Ubuntu Mono",
    "Consolas",
    "Courier New",
];
const CODE_FALLBACK: &str = "Noto Sans Mono";

/// The families chosen at startup. Unset means the fallbacks are used.
static UI_FAMILY: OnceLock<String> = OnceLock::new();
static CODE_FAMILY: OnceLock<String> = OnceLock::new();

/// Returns the first preference that is installed (an exact name, compared
/// case-insensitively), or the fallback when none is.
pub fn pick<'a>(installed: &[String], preferences: &[&'a str], fallback: &'a str) -> &'a str {
    preferences
        .iter()
        .copied()
        .find(|wanted| {
            installed
                .iter()
                .any(|name| name.eq_ignore_ascii_case(wanted))
        })
        .unwrap_or(fallback)
}

/// Chooses the UI and code families from the installed ones. Called once at
/// startup; a second call keeps the first choice.
pub fn choose_fonts(installed: &[String]) {
    let _ = UI_FAMILY.set(pick(installed, UI_PREFERENCES, UI_FALLBACK).to_owned());
    let _ = CODE_FAMILY.set(pick(installed, CODE_PREFERENCES, CODE_FALLBACK).to_owned());
}

pub fn ui_font() -> SharedString {
    UI_FAMILY
        .get()
        .map_or(UI_FALLBACK, String::as_str)
        .to_owned()
        .into()
}

pub fn code_font() -> SharedString {
    CODE_FAMILY
        .get()
        .map_or(CODE_FALLBACK, String::as_str)
        .to_owned()
        .into()
}

static DARK: AtomicBool = AtomicBool::new(false);

/// Follows the operating system: called at startup and whenever the window
/// appearance changes. Returns whether the mode changed.
pub fn follow(appearance: WindowAppearance) -> bool {
    let dark = matches!(
        appearance,
        WindowAppearance::Dark | WindowAppearance::VibrantDark
    );
    DARK.swap(dark, Ordering::Relaxed) != dark
}

fn by_mode(light: Rgba, dark: Rgba) -> Rgba {
    if DARK.load(Ordering::Relaxed) {
        dark
    } else {
        light
    }
}

pub fn canvas() -> Rgba {
    by_mode(rgb(0x00e6_e5df), rgb(0x0014_1413))
}
pub fn paper() -> Rgba {
    by_mode(rgb(0x00f7_f7f4), rgb(0x001c_1c1a))
}
pub fn panel() -> Rgba {
    by_mode(rgb(0x00ee_ede7), rgb(0x0023_2321))
}
pub fn hairline() -> Rgba {
    by_mode(rgb(0x00e4_e3dc), rgb(0x002c_2b28))
}
pub fn hairline_strong() -> Rgba {
    by_mode(rgb(0x00db_d9d2), rgb(0x0037_3632))
}
pub fn hairline_control() -> Rgba {
    by_mode(rgb(0x00c9_c7be), rgb(0x004a_4842))
}
/// The background of a selected diff line: the palette's blue at 20%, so it
/// reads as a selection on paper and over the green and red diff tints.
pub fn selected_line() -> Rgba {
    by_mode(rgba(0x006c_bc33), rgba(0x4a9e_ff40))
}
pub fn selected_panel() -> Rgba {
    by_mode(rgb(0x00de_dcd4), rgb(0x0033_322d))
}
/// The background of selected text in a field.
pub fn text_selection() -> Rgba {
    by_mode(rgba(0x006c_bc40), rgba(0x4a9e_ff55))
}
/// A search match in the terminal.
pub fn terminal_match() -> Rgba {
    by_mode(rgba(0xd9a4_0066), rgba(0xd9a4_0055))
}
/// The focused search match in the terminal.
pub fn terminal_match_current() -> Rgba {
    by_mode(rgba(0xf08c_00cc), rgba(0xf5a6_2399))
}
/// The dim layer behind a dialog or overlay.
pub fn scrim() -> Rgba {
    by_mode(rgba(0x1614_1433), rgba(0x0000_0099))
}
/// The shadow under a dialog card.
pub fn shadow() -> Rgba {
    by_mode(rgba(0x1616_1426), rgba(0x0000_0099))
}
pub fn ink() -> Rgba {
    by_mode(rgb(0x0016_1614), rgb(0x00f1_f0ea))
}
pub fn text() -> Rgba {
    by_mode(rgb(0x0031_302d), rgb(0x00d8_d7d0))
}
pub fn muted() -> Rgba {
    by_mode(rgb(0x0069_6761), rgb(0x00a3_a199))
}
pub fn faint() -> Rgba {
    by_mode(rgb(0x008a_8882), rgb(0x007c_7a73))
}
pub fn pending() -> Rgba {
    by_mode(rgb(0x0084_6600), rgb(0x00d9_a900))
}
pub fn agent() -> Rgba {
    by_mode(rgb(0x0078_56c0), rgb(0x00a8_8ff0))
}
pub fn done() -> Rgba {
    by_mode(rgb(0x0000_7b2f), rgb(0x0042_c274))
}
pub fn rejected() -> Rgba {
    by_mode(rgb(0x00ba_3e4b), rgb(0x00f0_7480))
}
/// Orange means "you are needed". Nothing else is orange.
pub fn needs_you() -> Rgba {
    by_mode(rgb(0x00f5_4e00), rgb(0x00ff_6a2a))
}
/// The tint behind an added diff line: the green at about 9% opacity.
pub fn added_tint() -> Rgba {
    by_mode(rgba(0x007b_2f17), rgba(0x42c2_7424))
}
/// The tint behind a removed diff line: the red at about 9% opacity.
pub fn removed_tint() -> Rgba {
    by_mode(rgba(0xba3e_4b17), rgba(0xf074_8024))
}

/// The colour of a syntax class in a diff line. `None` is ink: functions,
/// operators and punctuation stay quiet so colour marks meaning only.
pub fn syntax(class: Class) -> Option<Rgba> {
    match class {
        Class::Keyword => Some(by_mode(rgb(0x0000_6cbc), rgb(0x006c_b6ff))),
        Class::String | Class::Number => Some(by_mode(rgb(0x00aa_5100), rgb(0x00e9_a15c))),
        Class::Constant => Some(by_mode(rgb(0x00a9_438d), rgb(0x00d6_7fc0))),
        Class::Type => Some(by_mode(rgb(0x0000_7677), rgb(0x004f_c4c4))),
        Class::Comment => Some(muted()),
        Class::Function | Class::Operator | Class::Punctuation | Class::Plain => None,
    }
}

/// The terminal's 16 ANSI colours: 0 to 7 normal, 8 to 15 bright. The theme
/// has no terminal palette, so the two sets live here; the foreground and
/// background of a terminal are `ink` and `paper`.
pub fn ansi(index: u8) -> Rgba {
    const LIGHT: [u32; 16] = [
        0x001b_1b1a,
        0x00c0_392b,
        0x002e_7d32,
        0x009a_6700,
        0x001f_5fbf,
        0x008e_44ad,
        0x0000_838f,
        0x0066_6660,
        0x0077_7772,
        0x00d9_4a3a,
        0x003d_9a42,
        0x00b8_7a00,
        0x0030_74e0,
        0x00a8_5bc8,
        0x0000_9fae,
        0x00a0_a09a,
    ];
    const DARK: [u32; 16] = [
        0x0048_4845,
        0x00e0_6c5f,
        0x0066_bb6a,
        0x00e0_b050,
        0x0066_a3f5,
        0x00c0_82e0,
        0x004f_c4c4,
        0x00c8_c8c2,
        0x0070_706b,
        0x00f2_8b80,
        0x0085_d68a,
        0x00f0_c870,
        0x0088_bbff,
        0x00d6_a4ee,
        0x0075_dede,
        0x00ee_eee8,
    ];
    let i = usize::from(index & 15);
    by_mode(rgb(LIGHT[i]), rgb(DARK[i]))
}

/// Spells a macOS-style chord ("⇧⌘R", "⌘↵", "↵") the way this platform names
/// its keys: as is on macOS, "Ctrl+Shift+R" elsewhere. Text with no modifier
/// or return glyph ("J K", "O") is returned unchanged.
pub fn chord(mac: &str) -> String {
    spell(mac, cfg!(target_os = "macos"))
}

fn spell(mac: &str, is_mac: bool) -> String {
    if is_mac {
        return mac.to_owned();
    }
    let (mut ctrl, mut shift) = (false, false);
    let mut rest = String::new();
    for c in mac.chars() {
        match c {
            '\u{2318}' => ctrl = true,
            '\u{21e7}' => shift = true,
            '\u{21b5}' => rest.push_str("Enter"),
            other => rest.push(other),
        }
    }
    let mut parts = Vec::new();
    if ctrl {
        parts.push("Ctrl");
    }
    if shift {
        parts.push("Shift");
    }
    parts.push(&rest);
    parts.join("+")
}

/// The chord that submits a multi-line field, as the key hints spell it.
pub fn submit_chord() -> String {
    chord("\u{2318}\u{21b5}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installed(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn only_meaningful_classes_have_a_colour() {
        for class in [
            Class::Keyword,
            Class::String,
            Class::Number,
            Class::Constant,
        ] {
            assert!(syntax(class).is_some());
        }
        for class in [
            Class::Function,
            Class::Operator,
            Class::Punctuation,
            Class::Plain,
        ] {
            assert!(syntax(class).is_none());
        }
        assert_eq!(syntax(Class::Keyword), Some(rgb(0x0000_6cbc)));
    }

    #[test]
    fn chords_are_spelled_for_the_platform() {
        assert_eq!(spell("\u{21e7}\u{2318}R", true), "\u{21e7}\u{2318}R");
        assert_eq!(spell("\u{21e7}\u{2318}R", false), "Ctrl+Shift+R");
        assert_eq!(spell("\u{2318}\u{21b5}", false), "Ctrl+Enter");
        assert_eq!(spell("\u{2318}P", false), "Ctrl+P");
        assert_eq!(spell("\u{21b5}", false), "Enter");
        assert_eq!(spell("J K", false), "J K");
    }

    #[test]
    fn follow_reports_only_a_change_of_mode() {
        assert!(!follow(WindowAppearance::Light));
        assert!(follow(WindowAppearance::Dark));
        assert!(!follow(WindowAppearance::VibrantDark));
        assert_ne!(paper(), rgb(0x00f7_f7f4));
        assert!(follow(WindowAppearance::Light));
        assert_eq!(paper(), rgb(0x00f7_f7f4));
    }

    #[test]
    fn exact_match_is_picked() {
        let names = installed(&["Ubuntu", "JetBrains Mono"]);
        assert_eq!(
            pick(&names, CODE_PREFERENCES, CODE_FALLBACK),
            "JetBrains Mono"
        );
    }

    #[test]
    fn match_ignores_case() {
        let names = installed(&["noto sans mono"]);
        assert_eq!(
            pick(&names, CODE_PREFERENCES, CODE_FALLBACK),
            "Noto Sans Mono"
        );
    }

    #[test]
    fn earlier_preference_wins_over_installed_order() {
        let names = installed(&["DejaVu Sans Mono", "Fira Code", "Menlo"]);
        assert_eq!(pick(&names, CODE_PREFERENCES, CODE_FALLBACK), "Menlo");
    }

    #[test]
    fn partial_name_does_not_match() {
        let names = installed(&["JetBrains Mono Light"]);
        assert_eq!(
            pick(&names, CODE_PREFERENCES, CODE_FALLBACK),
            "Noto Sans Mono"
        );
    }

    #[test]
    fn nothing_installed_gives_the_fallback() {
        assert_eq!(pick(&[], UI_PREFERENCES, UI_FALLBACK), "Noto Sans");
        assert_eq!(pick(&[], CODE_PREFERENCES, CODE_FALLBACK), "Noto Sans Mono");
    }

    #[test]
    fn unrelated_fonts_give_the_fallback() {
        let names = installed(&["Wingdings", "Symbol"]);
        assert_eq!(pick(&names, UI_PREFERENCES, "Fallback"), "Fallback");
    }

    #[test]
    fn the_real_preference_lists_are_nonempty_and_unique() {
        for list in [UI_PREFERENCES, CODE_PREFERENCES] {
            let mut sorted = list.to_vec();
            sorted.sort_unstable();
            sorted.dedup();
            assert_eq!(sorted.len(), list.len());
        }
    }
}
