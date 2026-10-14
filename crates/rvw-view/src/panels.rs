//! Which of a window's two side panels are shown. The state is the window's,
//! not a screen's, so a panel collapsed on one screen stays collapsed on the
//! next. It lives in memory only.

/// A side panel of the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

/// The panels shown; both are, to begin with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Panels {
    pub left: bool,
    pub right: bool,
}

impl Default for Panels {
    fn default() -> Self {
        Self {
            left: true,
            right: true,
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
}
