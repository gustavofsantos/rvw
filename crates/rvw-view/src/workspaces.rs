//! workspaces: the workspace picker's list. The workspaces rvw holds a queue
//! for, the open one first, then the ones with the most work waiting. Pure;
//! the caller reads the summaries and drops the ones whose directory is gone.

use rvw_core::operations::WorkspaceSummary;
use rvw_git::worktree::Worktree;

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
    /// The branch of the repository's worktree at this path, or "detached";
    /// None when the path is not a worktree.
    pub worktree_branch: Option<String>,
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
            worktree_branch: None,
        })
        .collect();
    if !items.iter().any(|item| item.current) {
        items.push(Item {
            path: current.to_owned(),
            label: label(current, home),
            pending: 0,
            pulled: 0,
            current: true,
            worktree_branch: None,
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

/// `items` with the repository's worktrees first, in git's order. A worktree
/// that is the open one, or whose directory is gone (prunable), gets no item of
/// its own. A worktree already in `items` stays where it is and is marked with
/// its branch, so it is not listed twice. The other items follow, unchanged.
#[must_use]
pub fn with_worktrees(items: Vec<Item>, worktrees: &[Worktree], home: Option<&str>) -> Vec<Item> {
    let mut items = items;
    let mut added: Vec<Item> = Vec::new();
    for worktree in worktrees {
        let path = worktree.path.to_string_lossy().into_owned();
        let branch = worktree
            .branch
            .clone()
            .unwrap_or_else(|| "detached".to_owned());
        if let Some(item) = items.iter_mut().find(|item| item.path == path) {
            item.worktree_branch = Some(branch);
            continue;
        }
        if worktree.current || worktree.prunable || added.iter().any(|item| item.path == path) {
            continue;
        }
        added.push(Item {
            label: label(&path, home),
            path,
            pending: 0,
            pulled: 0,
            current: false,
            worktree_branch: Some(branch),
        });
    }
    added.extend(items);
    added
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

    fn worktree(path: &str, branch: Option<&str>, main: bool, current: bool) -> Worktree {
        Worktree {
            path: path.into(),
            head: None,
            branch: branch.map(str::to_owned),
            main,
            current,
            locked: false,
            prunable: false,
        }
    }

    fn plain(path: &str) -> Item {
        Item {
            path: path.to_owned(),
            label: path.to_owned(),
            pending: 0,
            pulled: 0,
            current: false,
            worktree_branch: None,
        }
    }

    #[test]
    fn new_worktrees_come_first_in_git_order_then_the_existing_items() {
        let items = vec![
            Item {
                current: true,
                ..plain("/w/open")
            },
            plain("/w/busy"),
        ];
        let worktrees = [
            worktree("/w/main", Some("main"), true, false),
            worktree("/w/open", Some("open"), false, true),
            worktree("/w/feat", Some("feat"), false, false),
        ];
        let got = with_worktrees(items, &worktrees, None);
        let paths: Vec<&str> = got.iter().map(|item| item.path.as_str()).collect();
        assert_eq!(paths, ["/w/main", "/w/feat", "/w/open", "/w/busy"]);
        assert_eq!(got[0].worktree_branch.as_deref(), Some("main"));
        assert_eq!(got[1].worktree_branch.as_deref(), Some("feat"));
        assert!(got[2].current);
        assert_eq!(got[3].worktree_branch, None);
    }

    #[test]
    fn a_worktree_already_listed_is_marked_and_not_duplicated() {
        let items = vec![
            plain("/w/quiet"),
            Item {
                pending: 2,
                ..plain("/w/busy")
            },
        ];
        let worktrees = [
            worktree("/w/main", Some("main"), true, false),
            worktree("/w/busy", Some("busy"), false, false),
        ];
        let got = with_worktrees(items, &worktrees, None);
        let paths: Vec<&str> = got.iter().map(|item| item.path.as_str()).collect();
        assert_eq!(paths, ["/w/main", "/w/quiet", "/w/busy"]);
        assert_eq!(got[2].worktree_branch.as_deref(), Some("busy"));
        assert_eq!(got[2].pending, 2);
        assert_eq!(got[1].worktree_branch, None);
    }

    #[test]
    fn the_open_and_prunable_worktrees_get_no_item() {
        let mut gone = worktree("/w/gone", Some("gone"), false, false);
        gone.prunable = true;
        let worktrees = [worktree("/w/open", Some("open"), true, true), gone];
        assert!(with_worktrees(Vec::new(), &worktrees, None).is_empty());
    }

    #[test]
    fn a_detached_worktree_is_marked_detached_and_a_branch_by_name() {
        let worktrees = [
            worktree("/home/me/src/a", None, false, false),
            worktree("/home/me/src/b", Some("feat/x"), false, false),
        ];
        let got = with_worktrees(Vec::new(), &worktrees, Some("/home/me"));
        assert_eq!(labels(&got), ["~/src/a", "~/src/b"]);
        assert_eq!(got[0].worktree_branch.as_deref(), Some("detached"));
        assert_eq!(got[1].worktree_branch.as_deref(), Some("feat/x"));
    }

    #[test]
    fn a_path_listed_twice_by_git_gets_one_item() {
        let worktrees = [
            worktree("/w/main", Some("main"), true, false),
            worktree("/w/feat", Some("feat"), false, false),
            worktree("/w/feat", Some("feat"), false, false),
        ];
        assert_eq!(with_worktrees(Vec::new(), &worktrees, None).len(), 2);
    }
}
