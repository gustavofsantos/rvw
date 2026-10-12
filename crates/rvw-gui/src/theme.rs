//! Design tokens from docs/gui-v1-design.md. `rgb` is not `const`, so colours
//! are functions; sizes are constants.
//!
//! The palette is complete before every screen uses it, hence the allow.
#![allow(dead_code)]

use gpui::{px, rgb, rgba, Pixels, Rgba};

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

pub const UI_FONT: &str = "Hanken Grotesk";
pub const CODE_FONT: &str = "JetBrains Mono";

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
/// The background of a selected diff line.
pub fn selected_line() -> Rgba {
    rgb(0x00e4_e3dc)
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

/// The chord that submits a multi-line field, as the key hints spell it.
pub fn submit_chord() -> &'static str {
    if cfg!(target_os = "macos") {
        "\u{2318}\u{21b5}"
    } else {
        "Ctrl+Enter"
    }
}
