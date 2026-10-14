//! The math of dragging a panel's edge: where the size ends up for a pointer
//! at some position, and the limits a size is kept within. Sizes are in
//! logical pixels along one axis of the window.

/// The terminal dock's height before the reviewer drags it.
pub const DOCK_DEFAULT: f32 = 300.;
/// The left panel's width before the reviewer drags it.
pub const LEFT_DEFAULT: f32 = 300.;
/// The right panel's width before the reviewer drags it.
pub const RIGHT_DEFAULT: f32 = 340.;

/// The dock: at least 120px, at most 80% of the window's height.
pub const DOCK: Limits = Limits {
    min: 120.,
    max_fraction: 0.8,
};
/// A side panel: at least 200px, at most 40% of the window's width.
pub const SIDE: Limits = Limits {
    min: 200.,
    max_fraction: 0.4,
};

/// What a size may be: a floor, and a fraction of the window's extent.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits {
    pub min: f32,
    pub max_fraction: f32,
}

impl Limits {
    /// The largest size in a window `extent` long. Never below the floor, so
    /// a tiny window keeps the panel usable instead of inverting the range.
    #[must_use]
    pub fn max(self, extent: f32) -> f32 {
        (extent * self.max_fraction).max(self.min)
    }

    /// `size` held between the floor and the ceiling. A size that is not a
    /// number is the floor.
    #[must_use]
    pub fn clamp(self, size: f32, extent: f32) -> f32 {
        if size.is_nan() {
            return self.min;
        }
        size.clamp(self.min, self.max(extent))
    }
}

/// The size after the pointer moved from `origin` to `pointer`, along the
/// axis. `grows_with_pointer` is true when moving the pointer to larger
/// coordinates enlarges the panel (the left panel's right edge), false when
/// it shrinks it (the dock's top edge, the right panel's left edge).
#[must_use]
pub fn dragged(
    start: f32,
    origin: f32,
    pointer: f32,
    grows_with_pointer: bool,
    limits: Limits,
    extent: f32,
) -> f32 {
    let delta = if grows_with_pointer {
        pointer - origin
    } else {
        origin - pointer
    };
    limits.clamp(start + delta, extent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dock_is_held_between_120_and_80_percent() {
        assert_eq!(DOCK.clamp(50., 1000.), 120.);
        assert_eq!(DOCK.clamp(500., 1000.), 500.);
        assert_eq!(DOCK.clamp(900., 1000.), 800.);
    }

    #[test]
    fn a_small_window_keeps_the_floor() {
        assert_eq!(DOCK.max(100.), 120.);
        assert_eq!(DOCK.clamp(300., 100.), 120.);
        assert_eq!(SIDE.max(300.), 200.);
    }

    #[test]
    fn a_side_panel_is_held_between_200_and_40_percent() {
        assert_eq!(SIDE.clamp(100., 1000.), 200.);
        assert_eq!(SIDE.clamp(350., 1000.), 350.);
        assert_eq!(SIDE.clamp(700., 1000.), 400.);
        assert_eq!(SIDE.clamp(f32::NAN, 1000.), 200.);
    }

    #[test]
    fn dragging_up_grows_the_dock() {
        assert_eq!(dragged(300., 600., 500., false, DOCK, 1000.), 400.);
        assert_eq!(dragged(300., 600., 700., false, DOCK, 1000.), 200.);
    }

    #[test]
    fn dragging_is_clamped_at_both_ends() {
        assert_eq!(dragged(300., 600., 0., false, DOCK, 1000.), 800.);
        assert_eq!(dragged(300., 600., 990., false, DOCK, 1000.), 120.);
    }

    #[test]
    fn the_left_edge_follows_the_pointer_and_the_right_goes_against_it() {
        assert_eq!(dragged(300., 300., 360., true, SIDE, 1200.), 360.);
        assert_eq!(dragged(340., 900., 860., false, SIDE, 1200.), 380.);
    }

    #[test]
    fn no_movement_keeps_the_size() {
        assert_eq!(dragged(300., 42., 42., true, SIDE, 1200.), 300.);
    }
}
