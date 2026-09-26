package tui

import tea "charm.land/bubbletea/v2"

// wheelStep is how many rows one wheel notch scrolls.
const wheelStep = 3

// hit is where a mouse event landed: a pane and the body row under it, 0 at
// the top of the pane. ok is false off the panes: borders, the bar, overlays.
func (m *model) hit(x, y int) (p pane, i int, ok bool) {
	tw, vw, body := m.treeWidth(), m.viewWidth(), m.bodyHeight()
	if y < 1 || y > body {
		return 0, 0, false
	}
	switch {
	case x >= 1 && x <= tw:
		return paneTree, y - 1, true
	case x >= tw+2 && x <= tw+1+vw:
		return paneViewer, y - 1, true
	}
	return 0, 0, false
}

// mouse handles a click, a drag, a release or a wheel notch. A click on a
// file in the tree opens it, on a directory expands or collapses it; a click
// in the viewer moves the cursor, and dragging from there selects lines.
func (m *model) mouse(msg tea.MouseMsg) tea.Cmd {
	e := msg.Mouse()
	if m.width < 40 || m.height < 10 || m.picker != nil || m.submit != nil || m.compare != nil || m.goLine != nil {
		return nil
	}
	if m.help {
		if _, ok := msg.(tea.MouseClickMsg); ok {
			m.help = false
		}
		return nil
	}
	switch msg.(type) {
	case tea.MouseReleaseMsg:
		m.dragging = false
		return nil
	case tea.MouseMotionMsg:
		if m.dragging && e.Button == tea.MouseLeft {
			m.drag(e.Y)
		}
		return nil
	}
	p, i, ok := m.hit(e.X, e.Y)
	if !ok {
		return nil
	}
	switch msg.(type) {
	case tea.MouseWheelMsg:
		switch e.Button {
		case tea.MouseWheelUp:
			m.wheel(p, -wheelStep)
		case tea.MouseWheelDown:
			m.wheel(p, wheelStep)
		}
		return nil
	case tea.MouseClickMsg:
		if e.Button != tea.MouseLeft {
			return nil
		}
		m.count, m.prefix = "", ""
		if p == paneTree {
			return m.clickTree(i)
		}
		m.clickViewer(i)
	}
	return nil
}

func (m *model) clickTree(i int) tea.Cmd {
	m.focus = paneTree
	idx := m.treeOff + i
	if idx >= len(m.rows) {
		return nil
	}
	m.treeCur = idx
	n := m.rows[idx].node
	if n.dir {
		n.expanded = !n.expanded
		m.refreshRows()
		return nil
	}
	return m.openFromTree(n.path)
}

func (m *model) clickViewer(i int) {
	f := m.file
	if f == nil {
		return
	}
	m.focus = paneViewer
	if f.len() == 0 || f.offset+i >= f.len() {
		return
	}
	f.cursor = f.offset + i
	m.visual, m.anchor, m.dragging = false, f.cursor, true
}

// drag moves the cursor to the line under row y, selecting from where the
// drag started. Past the top or bottom of the pane it scrolls one line.
func (m *model) drag(y int) {
	f := m.file
	if f == nil || f.len() == 0 {
		return
	}
	body := m.bodyHeight()
	line := f.offset + y - 1
	switch {
	case y < 1:
		line = f.offset - 1
	case y > body:
		line = f.offset + body
	}
	f.cursor = clamp(line, 0, f.len()-1)
	m.visual = f.cursor != m.anchor
}

// wheel scrolls a pane by n rows, carrying its cursor along when it would
// leave the screen.
func (m *model) wheel(p pane, n int) {
	body := m.bodyHeight()
	if p == paneTree {
		m.treeOff = clamp(m.treeOff+n, 0, max(0, len(m.rows)-body))
		m.treeCur = clamp(m.treeCur, m.treeOff, min(len(m.rows)-1, m.treeOff+body-1))
		return
	}
	f := m.file
	if f == nil || f.len() == 0 {
		return
	}
	f.offset = clamp(f.offset+n, 0, max(0, f.len()-body))
	f.cursor = clamp(f.cursor, f.offset, min(f.len()-1, f.offset+body-1))
}
