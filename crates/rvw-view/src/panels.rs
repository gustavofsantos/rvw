//! Which of a window's two side panels are shown. The state is the window's,
//! not a screen's, so a panel collapsed on one screen stays collapsed on the
//! next, and so is a panel's width, which is kept while it is collapsed. It
//! lives in memory only.

use crate::resize::{LEFT_DEFAULT, RIGHT_DEFAULT, SIDE};

/// A side panel of the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

/// The panels shown, and how wide each is; both shown at their default
/// widths to begin with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Panels {
    pub left: bool,
    pub right: bool,
    pub left_width: f32,
    pub right_width: f32,
}

impl Default for Panels {
    fn default() -> Self {
        Self {
            left: true,
            right: true,
            left_width: LEFT_DEFAULT,
            right_width: RIGHT_DEFAULT,
        }
    }
}

impl Panels {
    /// Collapses the panel when shown, shows it when collapsed.
    pub fn toggle(&mut self, side: Side) {
        match side {
            Side::Left => self.left = !self.left,
            Side::Right => self.right = !self.right,
        }
    }

    /// Whether the panel is shown.
    #[must_use]
    pub fn shown(self, side: Side) -> bool {
        match side {
            Side::Left => self.left,
            Side::Right => self.right,
        }
    }
}

impl Panels {
    /// The panel's width in a window `extent` wide: the chosen one, kept
    /// within what the window allows now.
    #[must_use]
    pub fn width(self, side: Side, extent: f32) -> f32 {
        let chosen = match side {
            Side::Left => self.left_width,
            Side::Right => self.right_width,
        };
        SIDE.clamp(chosen, extent)
    }

    /// Remembers a width the reviewer dragged the panel to.
    pub fn set_width(&mut self, side: Side, width: f32) {
        match side {
            Side::Left => self.left_width = width,
            Side::Right => self.right_width = width,
        }
    }

    /// Gives the panel its default width back.
    pub fn reset_width(&mut self, side: Side) {
        match side {
            Side::Left => self.left_width = LEFT_DEFAULT,
            Side::Right => self.right_width = RIGHT_DEFAULT,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_panels_start_shown() {
        let panels = Panels::default();
        assert!(panels.shown(Side::Left));
        assert!(panels.shown(Side::Right));
    }

    #[test]
    fn toggling_one_side_leaves_the_other() {
        let mut panels = Panels::default();
        panels.toggle(Side::Left);
        assert!(!panels.shown(Side::Left));
        assert!(panels.shown(Side::Right));
        panels.toggle(Side::Right);
        assert!(!panels.shown(Side::Right));
        panels.toggle(Side::Left);
        assert!(panels.shown(Side::Left));
        assert!(!panels.shown(Side::Right));
    }

    #[test]
    fn a_width_is_kept_while_the_panel_is_collapsed() {
        let mut panels = Panels::default();
        panels.set_width(Side::Left, 420.);
        panels.toggle(Side::Left);
        panels.toggle(Side::Left);
        assert_eq!(panels.width(Side::Left, 1600.), 420.);
        assert_eq!(panels.width(Side::Right, 1600.), 340.);
    }

    #[test]
    fn a_width_is_clamped_to_the_window_and_resets() {
        let mut panels = Panels::default();
        panels.set_width(Side::Right, 900.);
        assert_eq!(panels.width(Side::Right, 1000.), 400.);
        assert_eq!(panels.width(Side::Right, 3000.), 900.);
        panels.reset_width(Side::Right);
        assert_eq!(panels.width(Side::Right, 1600.), 340.);
    }
}
