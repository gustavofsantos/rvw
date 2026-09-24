package tui

import (
	"io/fs"
	"os"
	"path/filepath"
	"slices"
	"strings"
)

// skipDirs are never walked: version control metadata, and the dependency and
// build directories that would drown the tree. No .gitignore parsing in v1.
var skipDirs = map[string]bool{
	".git": true, ".hg": true, ".jj": true,
	"node_modules": true, "vendor": true, "dist": true, "build": true,
	"target": true, ".venv": true, "__pycache__": true,
}

// maxFiles caps a walk, so a huge directory cannot stall the UI.
const maxFiles = 50000

// walkFiles lists every file under root as a slash-separated relative path,
// sorted. Unreadable entries are skipped; only an unreadable root fails.
func walkFiles(root string) ([]string, error) {
	var files []string
	err := filepath.WalkDir(root, func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			if path == root {
				return err
			}
			return nil
		}
		if d.IsDir() {
			if path != root && skipDirs[d.Name()] {
				return filepath.SkipDir
			}
			return nil
		}
		if !d.Type().IsRegular() {
			if info, err := os.Stat(path); err != nil || !info.Mode().IsRegular() {
				return nil
			}
		}
		rel, err := filepath.Rel(root, path)
		if err != nil {
			return nil
		}
		files = append(files, filepath.ToSlash(rel))
		if len(files) >= maxFiles {
			return filepath.SkipAll
		}
		return nil
	})
	slices.Sort(files)
	return files, err
}

// node is a file or a directory of the workspace tree. The root is the
// workspace itself and is never drawn.
type node struct {
	name     string
	path     string // relative, slash-separated; "" for the root
	dir      bool
	expanded bool
	parent   *node
	children []*node
}

// buildTree turns relative file paths into a tree, directories before files,
// then alphabetical. Every directory starts collapsed.
func buildTree(files []string) *node {
	root := &node{dir: true, expanded: true}
	dirs := map[string]*node{"": root}
	for _, f := range files {
		parts := strings.Split(f, "/")
		parent := root
		for i, name := range parts[:len(parts)-1] {
			p := strings.Join(parts[:i+1], "/")
			d, ok := dirs[p]
			if !ok {
				d = &node{name: name, path: p, dir: true, parent: parent}
				parent.children = append(parent.children, d)
				dirs[p] = d
			}
			parent = d
		}
		parent.children = append(parent.children, &node{name: parts[len(parts)-1], path: f, parent: parent})
	}
	root.sort()
	return root
}

func (n *node) sort() {
	slices.SortFunc(n.children, func(a, b *node) int {
		if a.dir != b.dir {
			if a.dir {
				return -1
			}
			return 1
		}
		return strings.Compare(a.name, b.name)
	})
	for _, c := range n.children {
		if c.dir {
			c.sort()
		}
	}
}

// row is one visible line of the tree.
type row struct {
	node  *node
	depth int
}

// rows flattens the visible part of the tree: the children of every expanded
// directory, depth first.
func (n *node) rows() []row {
	var out []row
	var walk func(*node, int)
	walk = func(d *node, depth int) {
		for _, c := range d.children {
			out = append(out, row{c, depth})
			if c.dir && c.expanded {
				walk(c, depth+1)
			}
		}
	}
	walk(n, 0)
	return out
}

// find returns the node at a relative path, or nil.
func (n *node) find(path string) *node {
	cur := n
	for _, name := range strings.Split(path, "/") {
		var next *node
		for _, c := range cur.children {
			if c.name == name {
				next = c
				break
			}
		}
		if next == nil {
			return nil
		}
		cur = next
	}
	return cur
}

// reveal expands every directory above path, so its row is visible.
func (n *node) reveal(path string) {
	found := n.find(path)
	if found == nil {
		return
	}
	for d := found.parent; d != nil; d = d.parent {
		d.expanded = true
	}
}

// expandedDirs lists the expanded directories, to carry them across a rebuild.
func (n *node) expandedDirs() []string {
	var out []string
	for _, r := range n.allRows() {
		if r.node.dir && r.node.expanded {
			out = append(out, r.node.path)
		}
	}
	return out
}

// allRows is every node below n, visible or not.
func (n *node) allRows() []row {
	var out []row
	var walk func(*node, int)
	walk = func(d *node, depth int) {
		for _, c := range d.children {
			out = append(out, row{c, depth})
			walk(c, depth+1)
		}
	}
	walk(n, 0)
	return out
}

// firstFile is the file to show on start: the first file at the top level,
// else the first file in tree order.
func (n *node) firstFile() string {
	for _, c := range n.children {
		if !c.dir {
			return c.path
		}
	}
	for _, r := range n.allRows() {
		if !r.node.dir {
			return r.node.path
		}
	}
	return ""
}
