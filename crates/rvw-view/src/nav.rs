//! Keyboard movement through a list: one step up or down, stopping at the
//! ends instead of wrapping.

/// The index `delta` steps from `current`, clamped to `0..len`.
///
/// With no current index, a negative delta selects the last item and any
/// other delta the first. Returns `None` when the list is empty.
#[must_use]
pub fn step(len: usize, current: Option<usize>, delta: i32) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let last = len - 1;
    let Some(current) = current.map(|index| index.min(last)) else {
        return Some(if delta < 0 { last } else { 0 });
    };
    let distance = usize::try_from(delta.unsigned_abs()).unwrap_or(usize::MAX);
    let target = if delta < 0 {
        current.saturating_sub(distance)
    } else {
        current.saturating_add(distance)
    };
    Some(target.min(last))
}

/// The index of the path `delta` steps from `current` in `paths`, clamped.
/// `current` is the path selected now; a path not in the list counts as no
/// current index.
#[must_use]
pub fn file_step<S: AsRef<str>>(paths: &[S], current: Option<&str>, delta: i32) -> Option<usize> {
    let current = current.and_then(|path| paths.iter().position(|p| p.as_ref() == path));
    step(paths.len(), current, delta)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_list_has_no_step() {
        assert_eq!(step(0, None, 1), None);
        assert_eq!(step(0, Some(0), -1), None);
        assert_eq!(step(0, None, -1), None);
    }

    #[test]
    fn moves_by_delta_within_the_list() {
        assert_eq!(step(5, Some(2), 1), Some(3));
        assert_eq!(step(5, Some(2), -1), Some(1));
        assert_eq!(step(5, Some(0), 3), Some(3));
        assert_eq!(step(5, Some(4), -3), Some(1));
        assert_eq!(step(5, Some(2), 0), Some(2));
    }

    #[test]
    fn clamps_at_both_ends_without_wrapping() {
        assert_eq!(step(5, Some(4), 1), Some(4));
        assert_eq!(step(5, Some(0), -1), Some(0));
        assert_eq!(step(5, Some(1), -10), Some(0));
        assert_eq!(step(5, Some(3), 10), Some(4));
        assert_eq!(step(5, Some(3), i32::MAX), Some(4));
        assert_eq!(step(5, Some(3), i32::MIN), Some(0));
    }

    #[test]
    fn a_stale_current_index_is_clamped_to_the_list() {
        assert_eq!(step(3, Some(9), -1), Some(1));
        assert_eq!(step(3, Some(9), 1), Some(2));
    }

    #[test]
    fn no_current_selects_first_on_positive_and_last_on_negative_delta() {
        assert_eq!(step(4, None, 1), Some(0));
        assert_eq!(step(4, None, -1), Some(3));
        assert_eq!(step(1, None, -1), Some(0));
    }

    #[test]
    fn file_step_finds_the_current_path() {
        let paths = ["a.rs", "src/b.rs", "src/c.rs"];
        assert_eq!(file_step(&paths, Some("src/b.rs"), 1), Some(2));
        assert_eq!(file_step(&paths, Some("a.rs"), -1), Some(0));
    }

    #[test]
    fn file_step_treats_a_path_outside_the_list_as_no_current() {
        let paths = vec!["a.rs".to_owned(), "b.rs".to_owned()];
        assert_eq!(file_step(&paths, Some("elsewhere.rs"), 1), Some(0));
        assert_eq!(file_step(&paths, Some("elsewhere.rs"), -1), Some(1));
        assert_eq!(file_step(&paths, None, 1), Some(0));
        let none: [&str; 0] = [];
        assert_eq!(file_step(&none, None, 1), None);
    }
}
