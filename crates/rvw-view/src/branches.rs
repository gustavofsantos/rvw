//! branches: the branch picker's rows, ranked against the query by the fuzzy
//! matcher. Pure; the caller supplies the branch names.

use crate::fuzzy::fuzzy_match;

/// One row of the branch picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    /// The branch's index in the input.
    pub index: usize,
    pub name: String,
    /// Char indices in `name` of the characters the query matched.
    pub matches: Vec<usize>,
    pub score: i64,
}

/// The branches matching `query`, best first. An empty query lists every
/// branch in input order with no matches. Ties keep input order.
#[must_use]
pub fn rank(query: &str, branches: &[String]) -> Vec<Hit> {
    if query.is_empty() {
        return branches
            .iter()
            .enumerate()
            .map(|(index, name)| Hit {
                index,
                name: name.clone(),
                matches: Vec::new(),
                score: 0,
            })
            .collect();
    }
    let mut hits: Vec<Hit> = branches
        .iter()
        .enumerate()
        .filter_map(|(index, name)| {
            let found = fuzzy_match(query, name)?;
            Some(Hit {
                index,
                name: name.clone(),
                matches: found.positions,
                score: found.score,
            })
        })
        .collect();
    hits.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.index.cmp(&b.index)));
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(hits: &[Hit]) -> Vec<&str> {
        hits.iter().map(|h| h.name.as_str()).collect()
    }

    fn list(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| (*n).to_owned()).collect()
    }

    #[test]
    fn empty_query_lists_every_branch_in_input_order() {
        let hits = rank("", &list(&["main", "feat/x", "dev"]));
        assert_eq!(names(&hits), vec!["main", "feat/x", "dev"]);
        assert!(hits.iter().all(|h| h.matches.is_empty() && h.score == 0));
        assert_eq!(hits[1].index, 1);
    }

    #[test]
    fn subsequence_match_reports_positions() {
        // "fx" matches f then x in "feature/fix"; the f after '/' starts a
        // segment, so it is preferred.
        let hits = rank("fx", &list(&["main", "feature/fix"]));
        assert_eq!(names(&hits), vec!["feature/fix"]);
        assert_eq!(hits[0].index, 1);
        assert_eq!(hits[0].matches, vec![8, 10]);
    }

    #[test]
    fn prefix_match_ranks_before_inner_match() {
        let hits = rank("main", &list(&["xmain-fix", "main"]));
        assert_eq!(names(&hits), vec!["main", "xmain-fix"]);
        assert_eq!(hits[0].matches, vec![0, 1, 2, 3]);
    }

    #[test]
    fn no_match_is_dropped() {
        let hits = rank("zq", &list(&["main", "dev"]));
        assert!(hits.is_empty());
    }
}
