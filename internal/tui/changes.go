package tui

import (
	"context"
	"errors"
	"slices"

	"github.com/gustavofsantos/rvw/internal/gitx"
)

// compare is what the worktree is compared with, for the changes view and
// the gutter signs: always the files on disk against one commit.
type compare int

const (
	compareHEAD    compare = iota // uncommitted changes: against HEAD
	compareDefault                // the branch: against its merge-base with main or master
	comparePrev                   // against the commit before HEAD
)

var compareLabels = [...]string{compareHEAD: "uncommitted", compareDefault: "default branch", comparePrev: "previous commit"}

// base resolves what c compares against to a revision git can diff with,
// and a short name for it: HEAD, main, HEAD~1. HEAD and HEAD~1 stay names,
// so a commit made meanwhile moves them; a merge-base is a commit id.
func (c compare) base(ctx context.Context, dir string) (id, name string, err error) {
	switch c {
	case compareDefault:
		branch, ok := gitx.DefaultBranch(ctx, dir)
		if !ok {
			return "", "", errors.New("no main or master branch to compare with")
		}
		if _, ok := gitx.Commit(ctx, dir, "HEAD"); !ok {
			return "", "", errors.New("no commit to compare with yet")
		}
		id, err := gitx.MergeBase(ctx, dir, branch, "HEAD")
		if err != nil || id == "" {
			return "", "", errors.New("the branch shares no history with " + branch)
		}
		return id, branch, nil
	case comparePrev:
		if _, ok := gitx.Commit(ctx, dir, "HEAD~1"); !ok {
			return "", "", errors.New("no previous commit to compare with")
		}
		return "HEAD~1", "HEAD~1", nil
	default:
		if _, ok := gitx.Commit(ctx, dir, "HEAD"); !ok {
			return "", "", errors.New("no commit to compare with yet")
		}
		return "HEAD", "HEAD", nil
	}
}

// changeTree is the changed files as a tree, every directory expanded, and
// each path's status letter.
func changeTree(changes []gitx.Change) (*node, map[string]byte) {
	status := map[string]byte{}
	var paths []string
	for _, c := range changes {
		if _, seen := status[c.Path]; !seen {
			paths = append(paths, c.Path)
		}
		status[c.Path] = c.Status
	}
	slices.Sort(paths)
	root := buildTree(paths)
	for _, r := range root.allRows() {
		r.node.expanded = r.node.dir
	}
	return root, status
}

// sign is a line's git change against the compared commit, drawn left of its
// number.
type sign int

const (
	signNone       sign = iota
	signAdded           // a new line
	signChanged         // a line that replaced another
	signDeleted         // lines were deleted below this one
	signDeletedTop      // lines were deleted above this one, the first
)

var signGlyphs = [...]string{signNone: " ", signAdded: "▎", signChanged: "▎", signDeleted: "▁", signDeletedTop: "▔"}

// fileChanges is the change sign of each of a file's n lines against the
// commit base, 0-indexed, and the line each hunk starts on, 1-indexed; both
// are nil when git has nothing to say: without a base commit, or on any git
// failure.
func fileChanges(ctx context.Context, dir, path, base string, n int) ([]sign, []int) {
	if base == "" {
		return nil, nil
	}
	hunks, untracked, err := gitx.Changes(ctx, dir, path, base)
	if err != nil {
		return nil, nil
	}
	if untracked {
		hunks = []gitx.Hunk{{NewStart: 1, NewLines: n}}
	}
	return signs(hunks, n), hunkStarts(hunks, n)
}

// hunkStarts is the line each hunk is marked on first: a deletion's sign
// line, else its first new line.
func hunkStarts(hunks []gitx.Hunk, n int) []int {
	if n == 0 {
		return nil
	}
	var out []int
	for _, h := range hunks {
		out = append(out, clamp(h.NewStart, 1, n))
	}
	return out
}

// signs marks n lines with the hunks of a zero-context diff.
func signs(hunks []gitx.Hunk, n int) []sign {
	if len(hunks) == 0 || n == 0 {
		return nil
	}
	out := make([]sign, n)
	set := func(line int, s sign) { // 1-indexed
		if line >= 1 && line <= n {
			out[line-1] = s
		}
	}
	for _, h := range hunks {
		switch {
		case h.NewLines == 0 && h.NewStart == 0:
			set(1, signDeletedTop)
		case h.NewLines == 0:
			set(h.NewStart, signDeleted)
		default:
			s := signChanged
			if h.OldLines == 0 {
				s = signAdded
			}
			for l := h.NewStart; l < h.NewStart+h.NewLines; l++ {
				set(l, s)
			}
		}
	}
	return out
}
