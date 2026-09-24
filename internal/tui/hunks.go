package tui

import (
	"fmt"
	"slices"

	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"
	"github.com/charmbracelet/x/ansi"
	"github.com/gustavofsantos/rvw/internal/gitx"
)

// Signs mark the uncommitted changes in the gutter, left of the line number.
const (
	signAdded      = "+"
	signChanged    = "~"
	signDeleted    = "_" // lines were deleted below this one
	signDeletedTop = "‾" // lines were deleted above the first line
)

// loadChanges re-reads the workspace's uncommitted changes. Outside a git
// worktree there are none, and changes stays nil.
func (m *model) loadChanges() {
	changes, err := gitx.Changes(m.opts.Workspace)
	if err != nil {
		changes = nil
	}
	m.changes = changes
}

// refreshChanges re-reads the changes of one file, as it is opened.
func (m *model) refreshChanges(rel string) []gitx.Hunk {
	if m.changes == nil {
		return nil
	}
	one, err := gitx.Changes(m.opts.Workspace, ":(literal)"+rel)
	if err != nil {
		return m.changes[rel].Hunks
	}
	if c, ok := one[rel]; ok {
		m.changes[rel] = c
	} else {
		delete(m.changes, rel)
	}
	return m.changes[rel].Hunks
}

// hunkLine is the 1-indexed line a hunk is marked and jumped to: its first
// line, or, for a deletion, the line above the deleted ones (line 1 when they
// were at the top).
func hunkLine(h gitx.Hunk) int {
	if h.NewCount > 0 {
		return h.NewStart
	}
	return max(h.NewStart, 1)
}

// hunkEnd is the last line a hunk marks.
func hunkEnd(h gitx.Hunk) int {
	if h.NewCount > 0 {
		return h.NewStart + h.NewCount - 1
	}
	return hunkLine(h)
}

// hunkAt is the index of the first hunk marking a 1-indexed line, or -1.
func hunkAt(hs []gitx.Hunk, line int) int {
	return slices.IndexFunc(hs, func(h gitx.Hunk) bool { return hunkLine(h) <= line && line <= hunkEnd(h) })
}

// signAt is the gutter sign on a 1-indexed line, and its style: a space when
// the line is unchanged.
func signAt(hs []gitx.Hunk, line int) (string, lipgloss.Style) {
	i := hunkAt(hs, line)
	if i < 0 {
		return " ", stPlain
	}
	switch h := hs[i]; {
	case h.NewCount == 0 && h.NewStart == 0:
		return signDeletedTop, stSignDeleted
	case h.NewCount == 0:
		return signDeleted, stSignDeleted
	case h.OldCount == 0:
		return signAdded, stSignAdded
	}
	return signChanged, stSignChanged
}

// nextHunk is the line of the nearest hunk after line (dir 1) or before it
// (dir -1); ok is false when there is none.
func nextHunk(hs []gitx.Hunk, line, dir int) (int, bool) {
	best, ok := 0, false
	for _, h := range hs {
		s := hunkLine(h)
		if (dir > 0 && s > line || dir < 0 && s < line) && (!ok || (s-best)*dir < 0) {
			best, ok = s, true
		}
	}
	return best, ok
}

// changedFiles lists the files with hunks, in tree order.
func (m *model) changedFiles() []string {
	var out []string
	for _, r := range m.root.allRows() {
		if !r.node.dir && len(m.changes[r.node.path].Hunks) > 0 {
			out = append(out, r.node.path)
		}
	}
	return out
}

// jumpHunk moves to the next (dir 1) or previous (dir -1) change: in the open
// file, else the first (last) change of the nearest changed file in tree
// order.
func (m *model) jumpHunk(dir int) tea.Cmd {
	f := m.file
	if f == nil || m.focus != paneViewer {
		return nil
	}
	if m.changes == nil {
		return m.setFlash("not a git worktree: no changes to show", false)
	}
	if line, ok := nextHunk(f.hunks, f.cursor+1, dir); ok {
		m.gotoLine(line)
		return nil
	}
	if rel, line, ok := m.neighborChange(dir); ok {
		if err := m.openFile(rel, line); err != nil {
			return m.fail(err)
		}
		return nil
	}
	word := "next"
	if dir < 0 {
		word = "previous"
	}
	return m.setFlash("no "+word+" change", false)
}

// neighborChange is the file and line of the first change in the nearest
// changed file after the open one (dir 1), or of the last change in the
// nearest one before it (dir -1).
func (m *model) neighborChange(dir int) (string, int, bool) {
	var files []string
	for _, r := range m.root.allRows() {
		if !r.node.dir {
			files = append(files, r.node.path)
		}
	}
	i := slices.Index(files, m.file.rel)
	if i < 0 {
		return "", 0, false
	}
	for j := i + dir; j >= 0 && j < len(files); j += dir {
		if hs := m.changes[files[j]].Hunks; len(hs) > 0 {
			h := hs[0]
			if dir < 0 {
				h = hs[len(hs)-1]
			}
			return files[j], hunkLine(h), true
		}
	}
	return "", 0, false
}

// hunkBar describes the change under the cursor, if any.
func (m *model) hunkBar(w int) []string {
	f := m.file
	i := hunkAt(f.hunks, f.cursor+1)
	if i < 0 {
		return nil
	}
	h := f.hunks[i]
	row := []piece{
		{fmt.Sprintf("change %d/%d", i+1, len(f.hunks)), stMode},
		{" · " + lineRange(hunkLine(h), hunkEnd(h)) + " · ", stDim},
		{fmt.Sprintf("+%d", h.NewCount), stSignAdded},
		{" ", stPlain},
		{fmt.Sprintf("-%d", h.OldCount), stSignDeleted},
		{" · ]h next · [h previous · C-g all changes", stDim},
	}
	return []string{render(fitPieces(row, w), nil)}
}

// changeCandidates fills a picker with every hunk of the workspace, file by
// file in tree order.
func (m *model) changeCandidates(p *picker) {
	type entry struct {
		loc   string
		stats string
		t     target
	}
	var entries []entry
	width := 0
	for _, rel := range m.changedFiles() {
		for _, h := range m.changes[rel].Hunks {
			loc := location(rel, hunkLine(h), hunkEnd(h))
			width = max(width, ansi.StringWidth(loc))
			entries = append(entries, entry{loc, fmt.Sprintf("+%d -%d", h.NewCount, h.OldCount), target{file: rel, line: hunkLine(h)}})
		}
	}
	width = min(width, 60)
	for _, e := range entries {
		p.cands = append(p.cands, candidate{label: fit(e.loc, max(width, ansi.StringWidth(e.loc))) + "  " + e.stats})
		p.targets = append(p.targets, e.t)
	}
}
