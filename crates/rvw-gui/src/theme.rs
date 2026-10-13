//! Design tokens from docs/gui-v1-design.md. `rgb` is not `const`, so colours
//! are functions; sizes are constants.
//!
//! The palette is complete before every screen uses it, hence the allow.
#![allow(dead_code)]

use std::sync::OnceLock;

use gpui::{px, rgb, rgba, Pixels, Rgba, SharedString};
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

pub fn canvas() -> Rgba {
    rgb(0x00e6_e5df)
}
pub fn paper() -> Rgba {
    rgb(0x00f7_f7f4)
}
pub fn panel() -> Rgba {
    rgb(0x00ee_ede7)
}
pub fn hairline() -> Rgba {
    rgb(0x00e4_e3dc)
}
pub fn hairline_strong() -> Rgba {
    rgb(0x00db_d9d2)
}
pub fn hairline_control() -> Rgba {
    rgb(0x00c9_c7be)
}
/// The background of a selected diff line: the palette's blue at 20%, so it
/// reads as a selection on paper and over the green and red diff tints.
pub fn selected_line() -> Rgba {
    rgba(0x006c_bc33)
}
pub fn selected_panel() -> Rgba {
    rgb(0x00de_dcd4)
}
pub fn ink() -> Rgba {
    rgb(0x0016_1614)
}
pub fn text() -> Rgba {
    rgb(0x0031_302d)
}
pub fn muted() -> Rgba {
    rgb(0x0069_6761)
}
pub fn faint() -> Rgba {
    rgb(0x008a_8882)
}
pub fn pending() -> Rgba {
    rgb(0x0084_6600)
}
pub fn agent() -> Rgba {
    rgb(0x0078_56c0)
}
pub fn done() -> Rgba {
    rgb(0x0000_7b2f)
}
pub fn rejected() -> Rgba {
    rgb(0x00ba_3e4b)
}
/// Orange means "you are needed". Nothing else is orange.
pub fn needs_you() -> Rgba {
    rgb(0x00f5_4e00)
}
/// The tint behind an added diff line: the green at about 9% opacity.
pub fn added_tint() -> Rgba {
    rgba(0x007b_2f17)
}
/// The tint behind a removed diff line: the red at about 9% opacity.
pub fn removed_tint() -> Rgba {
    rgba(0xba3e_4b17)
}

/// The colour of a syntax class in a diff line. `None` is ink: functions,
/// operators and punctuation stay quiet so colour marks meaning only.
pub fn syntax(class: Class) -> Option<Rgba> {
    match class {
        Class::Keyword => Some(rgb(0x0000_6cbc)),
        Class::String | Class::Number => Some(rgb(0x00aa_5100)),
        Class::Constant => Some(rgb(0x00a9_438d)),
        Class::Type => Some(rgb(0x0000_7677)),
        Class::Comment => Some(rgb(0x0069_6761)),
        Class::Function | Class::Operator | Class::Punctuation | Class::Plain => None,
    }
}

/// The chord that submits a multi-line field, as the key hints spell it.
pub fn submit_chord() -> &'static str {
    if cfg!(target_os = "macos") {
        "\u{2318}\u{21b5}"
    } else {
        "Ctrl+Enter"
    }
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
