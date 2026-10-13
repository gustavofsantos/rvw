//! workspaces: the workspace picker's list. The workspaces rvw holds a queue
//! for, the open one first, then the ones with the most work waiting. Pure;
//! the caller reads the summaries and drops the ones whose directory is gone.

use rvw_core::operations::WorkspaceSummary;

/// One workspace the picker lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// The workspace as the queue stores it: the repository's root.
    pub path: String,
    /// The path as shown, with the home directory written `~`.
    pub label: String,
    /// Comments and reviews waiting for the agent.
    pub pending: i64,
    /// Comments the agent has taken and not decided.
    pub pulled: i64,
    /// The window shows this workspace.
    pub current: bool,
}

/// The picker's items: `current` first, then by work waiting (pending, then
/// pulled), then by label. `current` is listed even when the queue holds
/// nothing for it yet.
#[must_use]
pub fn items(found: &[WorkspaceSummary], current: &str, home: Option<&str>) -> Vec<Item> {
    let mut items: Vec<Item> = found
        .iter()
        .map(|row| Item {
            path: row.workspace.clone(),
            label: label(&row.workspace, home),
            pending: row.pending,
            pulled: row.pulled,
            current: row.workspace == current,
        })
        .collect();
    if !items.iter().any(|item| item.current) {
        items.push(Item {
            path: current.to_owned(),
            label: label(current, home),
            pending: 0,
            pulled: 0,
            current: true,
        });
    }
    items.sort_by(|a, b| {
        b.current
            .cmp(&a.current)
            .then_with(|| (b.pending, b.pulled).cmp(&(a.pending, a.pulled)))
            .then_with(|| a.label.cmp(&b.label))
    });
    items
}

/// `path` with a leading `home` written `~`.
#[must_use]
pub fn label(path: &str, home: Option<&str>) -> String {
    let Some(home) = home
        .map(|h| h.trim_end_matches('/'))
        .filter(|h| !h.is_empty())
    else {
        return path.to_owned();
    };
    if path == home {
        return "~".to_owned();
    }
    match path.strip_prefix(home) {
        Some(rest) if rest.starts_with('/') => format!("~{rest}"),
        _ => path.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(workspace: &str, pending: i64, pulled: i64) -> WorkspaceSummary {
        WorkspaceSummary {
            workspace: workspace.to_owned(),
            pending,
            pulled,
        }
    }

    fn labels(items: &[Item]) -> Vec<&str> {
        items.iter().map(|item| item.label.as_str()).collect()
    }

    #[test]
    fn the_open_workspace_comes_first_then_the_busiest() {
        let found = [
            summary("/w/quiet", 0, 0),
            summary("/w/busy", 5, 0),
            summary("/w/open", 0, 0),
            summary("/w/taken", 0, 3),
        ];
        let got = items(&found, "/w/open", None);
        assert_eq!(labels(&got), ["/w/open", "/w/busy", "/w/taken", "/w/quiet"]);
        assert!(got[0].current);
        assert!(got[1..].iter().all(|item| !item.current));
    }

    #[test]
    fn the_open_workspace_is_listed_without_a_queue() {
        let got = items(&[summary("/w/other", 1, 0)], "/w/new", None);
        assert_eq!(labels(&got), ["/w/new", "/w/other"]);
        assert_eq!((got[0].pending, got[0].pulled), (0, 0));
    }

    #[test]
    fn ties_are_ordered_by_label() {
        let found = [summary("/w/b", 1, 0), summary("/w/a", 1, 0)];
        assert_eq!(labels(&items(&found, "/x", None))[1..], ["/w/a", "/w/b"]);
    }

    #[test]
    fn home_is_written_as_a_tilde() {
        assert_eq!(label("/home/me/src/rvw", Some("/home/me")), "~/src/rvw");
        assert_eq!(label("/home/me/src/rvw", Some("/home/me/")), "~/src/rvw");
        assert_eq!(label("/home/me", Some("/home/me")), "~");
        assert_eq!(label("/home/meg/rvw", Some("/home/me")), "/home/meg/rvw");
        assert_eq!(label("/srv/rvw", Some("/home/me")), "/srv/rvw");
        assert_eq!(label("/srv/rvw", None), "/srv/rvw");
        assert_eq!(label("/srv/rvw", Some("")), "/srv/rvw");
    }
}
