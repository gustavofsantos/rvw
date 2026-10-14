//! The Phosphor icons the window draws, and how to place them: one name per
//! file under `assets/icons/`, painted in the text colour of their parent.

use gpui::{div, px, svg, Div, IntoElement, ParentElement, Pixels, Rgba, Styled, Svg};

/// An icon in a row, a button or a tree.
pub const SIZE: Pixels = px(16.);
/// An icon beside small counter text.
pub const SMALL: Pixels = px(14.);
/// An icon in the left rail.
pub const RAIL_SIZE: Pixels = px(20.);

/// Every bundled icon. `Fill` variants are for the active rail item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Queue,
    QueueFill,
    Review,
    ReviewFill,
    Changes,
    ChangesFill,
    History,
    HistoryFill,
    Pending,
    WithAgent,
    Done,
    Rejected,
    Expanded,
    Collapsed,
    Back,
    Ahead,
    Behind,
    Add,
    Into,
    Checked,
    Unchecked,
    Minimize,
    Maximize,
    Restore,
    Close,
    Sidebar,
}

impl Icon {
    /// Every icon, for the tests that check each one is bundled.
    #[cfg(test)]
    pub const ALL: [Icon; 26] = [
        Self::Queue,
        Self::QueueFill,
        Self::Review,
        Self::ReviewFill,
        Self::Changes,
        Self::ChangesFill,
        Self::History,
        Self::HistoryFill,
        Self::Pending,
        Self::WithAgent,
        Self::Done,
        Self::Rejected,
        Self::Expanded,
        Self::Collapsed,
        Self::Back,
        Self::Ahead,
        Self::Behind,
        Self::Add,
        Self::Into,
        Self::Checked,
        Self::Unchecked,
        Self::Minimize,
        Self::Maximize,
        Self::Restore,
        Self::Close,
        Self::Sidebar,
    ];

    /// The asset path of the icon's SVG.
    pub fn path(self) -> &'static str {
        match self {
            Self::Queue => "icons/list-checks.svg",
            Self::QueueFill => "icons/list-checks-fill.svg",
            Self::Review => "icons/chat-text.svg",
            Self::ReviewFill => "icons/chat-text-fill.svg",
            Self::Changes => "icons/git-diff.svg",
            Self::ChangesFill => "icons/git-diff-fill.svg",
            Self::History => "icons/clock-counter-clockwise.svg",
            Self::HistoryFill => "icons/clock-counter-clockwise-fill.svg",
            Self::Pending => "icons/circle.svg",
            Self::WithAgent => "icons/circle-half.svg",
            Self::Done => "icons/check.svg",
            Self::Rejected | Self::Close => "icons/x.svg",
            Self::Expanded => "icons/caret-down.svg",
            Self::Collapsed => "icons/caret-right.svg",
            Self::Back => "icons/arrow-left.svg",
            Self::Ahead => "icons/arrow-up.svg",
            Self::Behind | Self::Into => "icons/arrow-down.svg",
            Self::Add => "icons/plus.svg",
            Self::Checked => "icons/check-square.svg",
            Self::Unchecked | Self::Maximize => "icons/square.svg",
            Self::Minimize => "icons/minus.svg",
            Self::Restore => "icons/copy.svg",
            Self::Sidebar => "icons/sidebar-simple.svg",
        }
    }

    /// The icon for a status glyph of `rvw-view` (the same marks the terminal
    /// UI prints); anything unknown reads as pending.
    pub fn from_glyph(glyph: &str) -> Self {
        match glyph {
            "\u{25d0}" => Self::WithAgent,
            "\u{2713}" => Self::Done,
            "\u{2717}" => Self::Rejected,
            _ => Self::Pending,
        }
    }
}

/// The icon as an element of `size`, painted in `colour`.
pub fn view(icon: Icon, size: Pixels, colour: Rgba) -> Svg {
    svg()
        .path(icon.path())
        .size(size)
        .flex_none()
        .text_color(colour)
}

/// The icon for a status glyph of `rvw-view`, at the row size.
pub fn status(glyph: &str, colour: Rgba) -> Svg {
    view(Icon::from_glyph(glyph), SIZE, colour)
}

/// An icon followed by a label, centred on one line.
pub fn labelled(icon: Icon, size: Pixels, colour: Rgba, label: impl IntoElement) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(5.))
        .text_color(colour)
        .child(view(icon, size, colour))
        .child(label)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_names_an_asset_path() {
        for icon in Icon::ALL {
            assert!(icon.path().starts_with("icons/"));
        }
    }

    #[test]
    fn status_glyphs_map_to_their_icons() {
        assert_eq!(Icon::from_glyph("\u{25cb}"), Icon::Pending);
        assert_eq!(Icon::from_glyph("\u{25d0}"), Icon::WithAgent);
        assert_eq!(Icon::from_glyph("\u{2713}"), Icon::Done);
        assert_eq!(Icon::from_glyph("\u{2717}"), Icon::Rejected);
    }
}
