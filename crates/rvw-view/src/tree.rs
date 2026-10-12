//! Port of internal/tui/tree.go: the workspace's files as a tree of directories
//! and files, and the rows of it that are visible.
//!
//! The Go tree links nodes with parent pointers. Here every node lives in one
//! arena, [`Tree`], and refers to others by [`NodeId`]; the root is [`ROOT`]
//! and is never drawn.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use rvw_git::gitx::{self, GitError};

/// The listing is capped, so a huge worktree cannot stall the UI.
pub const MAX_FILES: usize = 50_000;

/// Index of a node in a [`Tree`].
pub type NodeId = usize;

/// The workspace itself. It has an empty name and path and is never drawn.
pub const ROOT: NodeId = 0;

/// A file or a directory of the workspace tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    /// The last path segment; empty for the root.
    pub name: String,
    /// Relative, slash-separated; empty for the root.
    pub path: String,
    pub dir: bool,
    /// Whether a directory's children are shown. Every directory starts
    /// collapsed, except the root.
    pub expanded: bool,
    parent: Option<NodeId>,
    children: Vec<NodeId>,
}

/// One visible line of the tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    pub node: NodeId,
    /// How many directories enclose the node; top-level rows are 0.
    pub depth: usize,
}

/// A workspace tree. Build one with [`build_tree`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tree {
    nodes: Vec<Node>,
}

/// Lists the files git sees in the worktree at `root`, as sorted,
/// slash-separated relative paths: tracked files and untracked files that
/// git does not ignore, kept only when they are regular files on disk (a
/// symlink counts when it points at one). A tracked file deleted from disk is
/// dropped. At most [`MAX_FILES`] are returned.
///
/// # Errors
///
/// Returns the git error when `root` is not a worktree or git cannot list it.
pub fn list_files(root: &Path) -> Result<Vec<String>, GitError> {
    let mut listed = gitx::files(root)?;
    listed.sort();
    listed.dedup();
    let mut files = Vec::new();
    for rel in listed {
        if !fs::metadata(root.join(&rel)).is_ok_and(|m| m.is_file()) {
            continue;
        }
        files.push(rel);
        if files.len() >= MAX_FILES {
            break;
        }
    }
    Ok(files)
}

/// Builds the tree of relative file paths. Directories come before files,
/// then names sort alphabetically (bytewise). Every directory starts
/// collapsed; the root is expanded.
#[must_use]
pub fn build_tree<S: AsRef<str>>(files: &[S]) -> Tree {
    let mut tree = Tree {
        nodes: vec![Node {
            name: String::new(),
            path: String::new(),
            dir: true,
            expanded: true,
            parent: None,
            children: Vec::new(),
        }],
    };
    let mut dirs: HashMap<String, NodeId> = HashMap::from([(String::new(), ROOT)]);
    for file in files {
        let file = file.as_ref();
        let parts: Vec<&str> = file.split('/').collect();
        let Some((leaf, dir_names)) = parts.split_last() else {
            continue;
        };
        let mut parent = ROOT;
        for (i, name) in dir_names.iter().enumerate() {
            let path = parts[..=i].join("/");
            parent = if let Some(id) = dirs.get(&path).copied() {
                id
            } else {
                let id = tree.add(parent, name, &path, true);
                dirs.insert(path, id);
                id
            };
        }
        tree.add(parent, leaf, file, false);
    }
    tree.sort_all();
    tree
}

impl Tree {
    pub fn add(&mut self, parent: NodeId, name: &str, path: &str, dir: bool) -> NodeId {
        let id = self.nodes.len();
        self.nodes.push(Node {
            name: name.to_owned(),
            path: path.to_owned(),
            dir,
            expanded: false,
            parent: Some(parent),
            children: Vec::new(),
        });
        self.nodes[parent].children.push(id);
        id
    }

    /// Sorts every node's children: directories first, then by name.
    fn sort_all(&mut self) {
        for id in 0..self.nodes.len() {
            let mut kids = std::mem::take(&mut self.nodes[id].children);
            kids.sort_by(|a, b| {
                let (a, b) = (&self.nodes[*a], &self.nodes[*b]);
                b.dir.cmp(&a.dir).then_with(|| a.name.cmp(&b.name))
            });
            self.nodes[id].children = kids;
        }
    }

    /// The node with this id. Panics on an id this tree did not return.
    #[must_use]
    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id]
    }

    /// The children of a node, in display order.
    #[must_use]
    pub fn children(&self, id: NodeId) -> &[NodeId] {
        &self.nodes[id].children
    }

    /// The parent of a node; `None` for the root.
    #[must_use]
    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.nodes[id].parent
    }

    /// Expands or collapses a node. Only a directory's flag matters for rows.
    pub fn set_expanded(&mut self, id: NodeId, expanded: bool) {
        self.nodes[id].expanded = expanded;
    }

    /// Expands every directory below the root, so every row is visible.
    pub fn expand_all_dirs(&mut self) {
        for node in &mut self.nodes {
            if node.dir {
                node.expanded = true;
            }
        }
    }

    /// The visible rows: the children of every expanded directory, depth
    /// first, in display order.
    #[must_use]
    pub fn rows(&self) -> Vec<Row> {
        let mut out = Vec::new();
        self.walk(ROOT, 0, true, &mut out);
        out
    }

    /// Every node below the root, visible or not, in display order.
    #[must_use]
    pub fn all_rows(&self) -> Vec<Row> {
        let mut out = Vec::new();
        self.walk(ROOT, 0, false, &mut out);
        out
    }

    /// Appends the children of `dir`, and, when `only_expanded` is false or a
    /// child directory is expanded, its descendants.
    fn walk(&self, dir: NodeId, depth: usize, only_expanded: bool, out: &mut Vec<Row>) {
        for &child in &self.nodes[dir].children {
            out.push(Row { node: child, depth });
            let node = &self.nodes[child];
            if node.dir && (!only_expanded || node.expanded) {
                self.walk(child, depth + 1, only_expanded, out);
            }
        }
    }

    /// The node at a relative path, or `None`. Matches segment by segment
    /// against the children; an empty path looks for a child named "".
    #[must_use]
    pub fn find(&self, path: &str) -> Option<NodeId> {
        let mut cur = ROOT;
        for name in path.split('/') {
            cur = *self.nodes[cur]
                .children
                .iter()
                .find(|&&c| self.nodes[c].name == name)?;
        }
        Some(cur)
    }

    /// Expands every directory above `path`, so its row is visible. Does
    /// nothing when there is no such node.
    pub fn reveal(&mut self, path: &str) {
        let Some(found) = self.find(path) else {
            return;
        };
        let mut up = self.nodes[found].parent;
        while let Some(id) = up {
            self.nodes[id].expanded = true;
            up = self.nodes[id].parent;
        }
    }

    /// The paths of the expanded directories, in tree order, to carry them
    /// across a rebuild.
    #[must_use]
    pub fn expanded_dirs(&self) -> Vec<String> {
        self.all_rows()
            .into_iter()
            .map(|r| &self.nodes[r.node])
            .filter(|n| n.dir && n.expanded)
            .map(|n| n.path.clone())
            .collect()
    }

    /// The file to show on start: the first file at the top level, else the
    /// first file in tree order. `None` when the tree has no file.
    #[must_use]
    pub fn first_file(&self) -> Option<&str> {
        let id = self.nodes[ROOT]
            .children
            .iter()
            .copied()
            .find(|&c| !self.nodes[c].dir)
            .or_else(|| {
                self.all_rows()
                    .into_iter()
                    .map(|r| r.node)
                    .find(|&c| !self.nodes[c].dir)
            })?;
        Some(&self.nodes[id].path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::Command;
    use tempfile::TempDir;

    /// The display names of rows: indented by depth, directories suffixed "/".
    fn row_names(tree: &Tree, rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|r| {
                let node = tree.node(r.node);
                let mut name = "  ".repeat(r.depth) + &node.name;
                if node.dir {
                    name.push('/');
                }
                name
            })
            .collect()
    }

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn write(root: &Path, rel: &str, content: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    #[test]
    fn list_files_obeys_gitignore_and_drops_deleted_tracked_files() {
        let ws = TempDir::new().unwrap();
        let root = ws.path();
        git(root, &["init", "-q"]);
        write(root, ".gitignore", "*.log\nout/\n");
        write(root, "src/.gitignore", "gen.go\n");
        write(root, "main.go", "package main\n");
        write(root, "gone.go", "package main\n");
        write(root, "vendor/dep/dep.go", "package dep\n");
        write(root, "debug.log", "ignored\n");
        write(root, "out/bin", "ignored\n");
        write(root, "src/gen.go", "ignored\n");
        write(root, "src/lib.go", "package src\n");
        git(root, &["add", "."]);
        fs::remove_file(root.join("gone.go")).unwrap();
        write(root, "new.txt", "untracked\n");
        write(root, "forced.log", "tracked though ignored\n");
        git(root, &["add", "-f", "forced.log"]);

        let files = list_files(root).unwrap();
        assert_eq!(
            files,
            [
                ".gitignore",
                "forced.log",
                "main.go",
                "new.txt",
                "src/.gitignore",
                "src/lib.go",
                "vendor/dep/dep.go",
            ]
        );
    }

    #[test]
    fn list_files_keeps_binary_files_and_symlinks_to_files_only() {
        let ws = TempDir::new().unwrap();
        let root = ws.path();
        git(root, &["init", "-q"]);
        fs::write(root.join("blob.bin"), [0u8, 1, 2, 0]).unwrap();
        write(root, "dir/inner.txt", "x\n");
        git(root, &["add", "."]);
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.join("dir/inner.txt"), root.join("link.txt")).unwrap();
            std::os::unix::fs::symlink(root.join("dir"), root.join("dirlink")).unwrap();
            git(root, &["add", "link.txt", "dirlink"]);
        }
        let files = list_files(root).unwrap();
        assert!(files.contains(&"blob.bin".to_owned()), "{files:?}");
        #[cfg(unix)]
        {
            assert!(files.contains(&"link.txt".to_owned()), "{files:?}");
            assert!(!files.contains(&"dirlink".to_owned()), "{files:?}");
        }
    }

    #[test]
    fn list_files_fails_outside_a_worktree() {
        let outside = TempDir::new().unwrap();
        let no_repo: PathBuf = outside.path().join("missing");
        assert!(list_files(&no_repo).is_err());
    }

    #[test]
    fn tree_sorts_directories_first_and_flattens_visible_rows() {
        let tree = build_tree(&["b.txt", "a/z.go", "a/b/c.go", "A.md", "c/d.go"]);
        let rows = tree.rows();
        assert_eq!(row_names(&tree, &rows), ["a/", "c/", "A.md", "b.txt"]);

        let mut tree = tree;
        tree.reveal("a/b/c.go");
        let rows = tree.rows();
        assert_eq!(
            row_names(&tree, &rows),
            ["a/", "  b/", "    c.go", "  z.go", "c/", "A.md", "b.txt"]
        );
        assert_eq!(tree.expanded_dirs(), ["a", "a/b"]);
        assert_eq!(tree.first_file(), Some("A.md"));
        assert_eq!(build_tree(&["x/y.go"]).first_file(), Some("x/y.go"));
    }

    #[test]
    fn all_rows_include_collapsed_children_with_depths() {
        let tree = build_tree(&["a/b/c.go", "a/z.go"]);
        let all = tree.all_rows();
        assert_eq!(row_names(&tree, &all), ["a/", "  b/", "    c.go", "  z.go"]);
        let depths: Vec<usize> = all.iter().map(|r| r.depth).collect();
        assert_eq!(depths, [0, 1, 2, 1]);
    }

    #[test]
    fn find_walks_segments_and_rejects_partial_names() {
        let tree = build_tree(&["src/api/parse.py"]);
        let id = tree.find("src/api/parse.py").unwrap();
        assert_eq!(tree.node(id).path, "src/api/parse.py");
        assert!(!tree.node(id).dir);
        assert!(tree.find("src/api").is_some());
        assert!(tree.find("src/ap").is_none());
        assert!(tree.find("src/api/parse").is_none());
        assert!(tree.find("nope").is_none());
    }

    #[test]
    fn find_of_empty_path_looks_for_an_empty_name() {
        let tree = build_tree(&["a.txt"]);
        assert_eq!(tree.find(""), None);
    }

    #[test]
    fn reveal_of_missing_path_changes_nothing() {
        let mut tree = build_tree(&["a/b.txt"]);
        let before = tree.clone();
        tree.reveal("zzz/q.txt");
        assert_eq!(tree, before);
    }

    #[test]
    fn reveal_expands_the_root_and_every_ancestor_only() {
        let mut tree = build_tree(&["a/b/c.go", "a/other/x.go"]);
        tree.reveal("a/b/c.go");
        let a = tree.find("a").unwrap();
        let b = tree.find("a/b").unwrap();
        let other = tree.find("a/other").unwrap();
        assert!(tree.node(a).expanded);
        assert!(tree.node(b).expanded);
        assert!(!tree.node(other).expanded);
        assert!(tree.node(ROOT).expanded);
    }

    #[test]
    fn parent_links_match_the_tree() {
        let tree = build_tree(&["a/b.txt"]);
        let a = tree.find("a").unwrap();
        let b = tree.find("a/b.txt").unwrap();
        assert_eq!(tree.parent(b), Some(a));
        assert_eq!(tree.parent(a), Some(ROOT));
        assert_eq!(tree.parent(ROOT), None);
        assert_eq!(tree.children(a), &[b]);
    }

    #[test]
    fn expand_all_dirs_shows_every_row() {
        let mut tree = build_tree(&["a/b/c.go", "d.txt"]);
        tree.expand_all_dirs();
        assert_eq!(tree.rows(), tree.all_rows());
    }

    #[test]
    fn first_file_is_none_for_an_empty_tree() {
        assert_eq!(build_tree::<&str>(&[]).first_file(), None);
        assert_eq!(
            build_tree(&["only/dirs/.keep"]).first_file(),
            Some("only/dirs/.keep")
        );
    }

    #[test]
    fn first_file_prefers_the_top_level_over_tree_order() {
        let tree = build_tree(&["a/x.go", "z.txt"]);
        assert_eq!(tree.first_file(), Some("z.txt"));
    }

    #[test]
    fn sort_is_bytewise_so_uppercase_comes_first() {
        let tree = build_tree(&["b", "B", "a", "A"]);
        let names: Vec<&str> = tree
            .children(ROOT)
            .iter()
            .map(|&c| tree.node(c).name.as_str())
            .collect();
        assert_eq!(names, ["A", "B", "a", "b"]);
    }

    #[test]
    fn a_name_can_be_both_a_file_and_a_directory() {
        // "a" is a file and also the parent of "a/b": both nodes exist.
        let tree = build_tree(&["a", "a/b"]);
        let rows = tree.rows();
        assert_eq!(row_names(&tree, &rows), ["a/", "a"]);
    }

    #[test]
    fn empty_listing_gives_an_empty_tree() {
        let tree = build_tree::<&str>(&[]);
        assert!(tree.rows().is_empty());
        assert!(tree.all_rows().is_empty());
        assert!(tree.expanded_dirs().is_empty());
    }

    #[test]
    fn set_expanded_controls_visible_rows() {
        let mut tree = build_tree(&["a/b.txt"]);
        let a = tree.find("a").unwrap();
        assert_eq!(tree.rows().len(), 1);
        tree.set_expanded(a, true);
        assert_eq!(tree.rows().len(), 2);
        tree.set_expanded(a, false);
        assert_eq!(tree.rows().len(), 1);
    }
}
