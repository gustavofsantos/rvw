//! The compare picker's entries: the three comparisons, each with the base it
//! resolves to or the reason it cannot be used.

use std::path::Path;

use crate::changes::{Base, Compare};

/// The comparisons in menu order.
pub const COMPARES: [Compare; 3] = [Compare::Head, Compare::Default, Compare::Prev];

/// One line of the menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub compare: Compare,
    pub label: &'static str,
    /// The resolved base's display name, when the comparison resolves.
    pub base: Option<String>,
    /// Why the comparison cannot be chosen, when it cannot.
    pub reason: Option<String>,
    /// This is the comparison in use.
    pub current: bool,
}

/// The menu for resolution `results` (one per entry of [`COMPARES`]) with
/// `current` marked. An unresolved comparison carries its error as its reason.
#[must_use]
pub fn items(results: &[Result<Base, String>], current: Option<Compare>) -> Vec<Item> {
    COMPARES
        .iter()
        .zip(results)
        .map(|(&compare, result)| Item {
            compare,
            label: compare.label(),
            base: result.as_ref().ok().map(|base| base.name.clone()),
            reason: result.as_ref().err().cloned(),
            current: current == Some(compare),
        })
        .collect()
}

/// Resolves every comparison in the worktree at `dir`; runs git.
#[must_use]
pub fn resolve_all(dir: &Path, current: Option<Compare>) -> Vec<Item> {
    let results: Vec<Result<Base, String>> = COMPARES.iter().map(|c| c.base(dir)).collect();
    items(&results, current)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base(name: &str) -> Base {
        Base {
            id: format!("id-{name}"),
            name: name.to_owned(),
        }
    }

    #[test]
    fn resolved_entries_show_their_base_and_the_current_one_is_marked() {
        let got = items(
            &[Ok(base("HEAD")), Ok(base("main")), Ok(base("HEAD~1"))],
            Some(Compare::Default),
        );
        assert_eq!(got[1].label, "default branch");
        assert_eq!(got[1].base.as_deref(), Some("main"));
        assert!(got[1].current && !got[0].current && !got[2].current);
        assert!(got.iter().all(|item| item.reason.is_none()));
    }

    #[test]
    fn an_unresolved_entry_carries_its_reason() {
        let got = items(
            &[
                Ok(base("HEAD")),
                Err("no main or master branch to compare with".into()),
                Err("no previous commit to compare with".into()),
            ],
            None,
        );
        assert_eq!(got[1].base, None);
        assert_eq!(
            got[1].reason.as_deref(),
            Some("no main or master branch to compare with")
        );
        assert!(got.iter().all(|item| !item.current));
    }

    #[test]
    fn the_order_is_uncommitted_default_previous() {
        let got = items(&[Ok(base("a")), Ok(base("b")), Ok(base("c"))], None);
        let order: Vec<Compare> = got.iter().map(|item| item.compare).collect();
        assert_eq!(order, COMPARES);
    }
}
