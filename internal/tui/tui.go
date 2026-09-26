// Package tui is rvw's terminal review UI: browse the workspace, read
// highlighted source, and comment on line ranges. It is an adapter over
// [review.Service], like the CLI and the MCP server, and holds no domain
// logic: every action is one service call.
package tui

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/gustavofsantos/rvw/internal/gitx"
	"github.com/gustavofsantos/rvw/internal/review"
)

// Options configure a session. Workspace is the canonical path an adapter
// resolved; Lane and Author stamp what the session writes.
type Options struct {
	Service   *review.Service
	Workspace string
	Lane      string
	Author    string
	// Editor is the command line comments are written in, run through sh.
	Editor string
	// Style is the chroma style for source code; empty follows the terminal's
	// background: monokai on dark, github on light.
	Style string
	// Leader is the key that starts leader mappings, as Bubble Tea names it:
	// "space", or one character such as ",". Empty is space.
	Leader string
}

// Run shows the UI on a terminal until the user quits.
func Run(ctx context.Context, opts Options, in io.Reader, out io.Writer) error {
	m, err := newModel(ctx, opts)
	if err != nil {
		return err
	}
	p := tea.NewProgram(m, tea.WithContext(ctx), tea.WithInput(in), tea.WithOutput(out))
	if _, err := p.Run(); err != nil && (!errors.Is(err, tea.ErrProgramKilled) || ctx.Err() == nil) {
		return err
	}
	return nil
}

type pane int

const (
	paneTree pane = iota
	paneViewer
)

// sidebar is what the left pane lists.
type sidebar int

const (
	sideFiles   sidebar = iota // every file in the workspace
	sideChanges                // the files that differ from the compared commit
	sideReviews                // submitted reviews and comments, resolved or not
	sides
)

// flashFor is how long a transient message holds the bottom bar.
const flashFor = 3 * time.Second

type model struct {
	ctx   context.Context
	opts  Options
	pal   *palette
	theme theme

	width, height int
	focus         pane

	side        sidebar
	fileTree    *node
	chgTree     *node                         // nil until the changes view is first shown
	revTree     *node                         // nil until the reviews side is first shown
	sheets      map[string]review.ReviewSheet // the reviews side's reviews by id
	revComments map[string]review.Comment     // and its comments by id
	status      map[string]byte               // git status letter per changed file
	chgErr      string                        // why the changes cannot be listed, if they cannot
	rows        []row                         // visible rows of the shown tree
	treeCur     int
	treeOff     int

	branch string    // the checked-out branch, for the status line
	wd     wdChanges // uncommitted changes, for the status line

	cmp      compare
	baseID   string // the revision cmp resolved to; "" for none: no gutter signs
	baseName string // its short name: HEAD, main, HEAD~1

	files  []string         // every file, relative
	open   []review.Comment // every open comment in the workspace
	counts map[string]int   // open comments per file

	file      *fileView
	detail    *detail        // a review or comment page shown over the file, if any
	positions map[string]int // last cursor line per file this session
	recent    []string       // files opened this session, most recent first

	visual      bool
	anchor      int
	dragging    bool    // the left button went down in the viewer and is held
	count       string  // a pending {count}
	leader      bool    // the leader key was pressed; the next key completes it
	prefix      string  // a pending g, ] or [
	prefixCount int     // the count before a prefix, for {count}gg
	goLine      *string // the :N prompt, when open

	picker  *picker
	submit  *submitMenu
	compare *compareMenu
	help    bool

	flash    string
	flashErr bool
	flashSeq int
}

// fileView is the open file. Lines are 0-indexed here, 1-indexed on screen.
type fileView struct {
	rel      string
	abs      string
	lines    [][]segment
	plain    []string
	source   string // the text as read, for a comment's snapshot
	binary   bool
	comments []review.Comment // open comments on this file, oldest first
	signs    []sign           // git change per line, nil for none
	hunks    []int            // the line each git hunk starts on
	cursor   int
	offset   int
}

func (f *fileView) len() int {
	if f.binary {
		return 0
	}
	return len(f.lines)
}

// selection is the 1-indexed range a comment would cover.
func (m *model) selection() (int, int) {
	c := m.file.cursor + 1
	if !m.visual {
		return c, c
	}
	a := m.anchor + 1
	return min(a, c), max(a, c)
}

func newModel(ctx context.Context, opts Options) (*model, error) {
	m := &model{
		ctx: ctx, opts: opts, focus: paneViewer, positions: map[string]int{}, counts: map[string]int{},
	}
	m.setTheme(true)
	m.baseID, m.baseName, _ = m.cmp.base(ctx, opts.Workspace)
	m.loadStatus()
	if err := m.loadTree(); err != nil {
		return nil, err
	}
	if err := m.loadComments(); err != nil {
		return nil, err
	}
	if first := m.fileTree.firstFile(); first != "" {
		if err := m.openFile(first, 0); err != nil {
			m.flash, m.flashErr = "rvw: "+err.Error(), true
		}
	} else {
		m.focus = paneTree
	}
	return m, nil
}

// ── loading ──────────────────────────────────────────────────────────────────

// loadTree lists the workspace, keeping expanded directories expanded.
func (m *model) loadTree() error {
	files, err := listFiles(m.ctx, m.opts.Workspace)
	if err != nil {
		return fmt.Errorf("cannot list workspace %s: %s", m.opts.Workspace, gitx.Stderr(err))
	}
	var expanded []string
	if m.fileTree != nil {
		expanded = m.fileTree.expandedDirs()
	}
	m.files = files
	m.fileTree = buildTree(files)
	for _, d := range expanded {
		if n := m.fileTree.find(d); n != nil && n.dir {
			n.expanded = true
		}
	}
	if m.file != nil {
		m.fileTree.reveal(m.file.rel)
	}
	m.refreshRows()
	return nil
}

// loadChanges lists the files that differ from the compared commit. A
// failure is kept to show in the pane, not returned: the rest of the UI works
// without it.
func (m *model) loadChanges() {
	var changes []gitx.Change
	var err error
	if m.baseID == "" { // perhaps there is one by now; else it says why not
		m.baseID, m.baseName, err = m.cmp.base(m.ctx, m.opts.Workspace)
	}
	if err == nil {
		changes, err = gitx.ChangedFiles(m.ctx, m.opts.Workspace, m.baseID)
	}
	m.chgErr = ""
	if err != nil {
		m.chgErr = gitx.Stderr(err)
	}
	m.chgTree, m.status = changeTree(changes)
	m.refreshRows()
}

// wdChanges counts the uncommitted changes since HEAD: lines added (every
// line of an untracked text file too) and deleted. dirty is set when any file
// differs, even one whose change has no lines. err says why there are no counts.
type wdChanges struct {
	added, deleted int
	dirty          bool
	err            string
}

// loadStatus reads the branch and the uncommitted changes for the status
// line. Like the changes view, it never fails the UI.
func (m *model) loadStatus() {
	ws := m.opts.Workspace
	m.branch = gitx.Branch(m.ctx, ws)
	m.wd = wdChanges{}
	if _, ok := gitx.Commit(m.ctx, ws, "HEAD"); !ok {
		m.wd.err = "no commits yet"
		return
	}
	changes, err := gitx.ChangedFiles(m.ctx, ws, "HEAD")
	if err == nil {
		m.wd.added, m.wd.deleted, err = gitx.LineChanges(m.ctx, ws, "HEAD")
	}
	if err != nil {
		m.wd = wdChanges{err: gitx.Stderr(err)}
		return
	}
	m.wd.dirty = len(changes) > 0
	for _, c := range changes {
		if c.Status == '?' {
			m.wd.added += textLines(filepath.Join(ws, filepath.FromSlash(c.Path)))
		}
	}
}

// textLines counts the lines of a text file; a binary or unreadable one has
// none.
func textLines(path string) int {
	data, err := os.ReadFile(path)
	if err != nil || len(data) == 0 || isBinary(data) {
		return 0
	}
	n := bytes.Count(data, []byte{'\n'})
	if data[len(data)-1] != '\n' {
		n++
	}
	return n
}

// tree is the tree the left pane shows.
func (m *model) tree() *node {
	switch {
	case m.side == sideChanges && m.chgTree != nil:
		return m.chgTree
	case m.side == sideReviews && m.revTree != nil:
		return m.revTree
	}
	return m.fileTree
}

// showSide switches the left pane to side, re-listing the changes or the
// reviews when it is them, and puts the tree cursor on what the viewer shows
// when it is listed.
func (m *model) showSide(side sidebar) error {
	m.side = side
	m.loadStatus()
	var err error
	switch side {
	case sideFiles:
	case sideChanges:
		m.loadChanges()
	case sideReviews:
		err = m.loadReviews()
	}
	m.refreshRows()
	m.treeCur, m.treeOff = 0, 0
	m.selectShown()
	return err
}

// selectShown puts the tree cursor on the row of what the viewer shows: the
// review or comment on the reviews side, else the open file.
func (m *model) selectShown() {
	switch {
	case m.side == sideReviews && m.detail != nil:
		m.selectTreeRow(m.detail.id)
	case m.side != sideReviews && m.file != nil:
		m.selectTreeRow(m.file.rel)
	}
}

// setCompare compares the worktree with what c names from now on: the
// changes view lists against it and the gutter marks against it. When c
// cannot be resolved, nothing changes but the error.
func (m *model) setCompare(c compare) error {
	id, name, err := c.base(m.ctx, m.opts.Workspace)
	if err != nil {
		return err
	}
	m.cmp, m.baseID, m.baseName = c, id, name
	if err := m.showSide(sideChanges); err != nil {
		return err
	}
	return m.reload(false)
}

// loadComments re-reads the open comments: the workspace's, for counts and
// the picker, and the open file's, for its gutter. Every lane is shown.
func (m *model) loadComments() error {
	svc, ws := m.opts.Service, m.opts.Workspace
	all, err := svc.List(m.ctx, review.QueryInput{Workspace: ws, Status: review.FilterOpen})
	if err != nil {
		return err
	}
	m.open = all.Comments
	m.counts = map[string]int{}
	for _, c := range m.open {
		m.counts[c.File]++
	}
	if m.file != nil {
		mine, err := svc.List(m.ctx, review.QueryInput{Workspace: ws, File: m.file.abs, Status: review.FilterOpen})
		if err != nil {
			return err
		}
		m.file.comments = mine.Comments
	}
	return nil
}

func (m *model) loadFile(rel string) (*fileView, error) {
	abs := rel
	if !filepath.IsAbs(rel) {
		abs = filepath.Join(m.opts.Workspace, filepath.FromSlash(rel))
	}
	data, err := os.ReadFile(abs)
	if err != nil {
		return nil, fmt.Errorf("cannot read %s", rel)
	}
	f := &fileView{rel: rel, abs: abs}
	if isBinary(data) {
		f.binary = true
		return f, nil
	}
	text := string(data)
	f.source = text
	f.plain = sourceLines(text)
	f.lines = highlight(rel, text)
	f.signs, f.hunks = fileChanges(m.ctx, m.opts.Workspace, abs, m.baseID, len(f.lines))
	return f, nil
}

// openFile shows a file with the cursor on line (1-indexed), or, for line 0,
// where it was left this session, else on line 1.
func (m *model) openFile(rel string, line int) error {
	f, err := m.loadFile(rel)
	if err != nil {
		return err
	}
	if m.file != nil {
		m.positions[m.file.rel] = m.file.cursor
	}
	switch p, seen := m.positions[rel]; {
	case line > 0:
		f.cursor = line - 1
	case seen:
		f.cursor = p
	}
	f.cursor = clamp(f.cursor, 0, f.len()-1)
	f.offset = max(0, f.cursor-m.bodyHeight()/3)
	m.file, m.detail = f, nil
	m.visual = false
	m.recent = append([]string{rel}, slices.DeleteFunc(m.recent, func(r string) bool { return r == rel })...)
	m.fileTree.reveal(rel)
	m.refreshRows()
	m.selectTreeRow(rel)
	return m.loadComments()
}

// reload re-reads the tree, the open file and the comments from disk and the
// queue; the cursor stays on its line.
func (m *model) reload(walk bool) error {
	if walk {
		m.baseID, m.baseName, _ = m.cmp.base(m.ctx, m.opts.Workspace)
		m.loadStatus()
		if err := m.loadTree(); err != nil {
			return err
		}
		if m.chgTree != nil {
			m.loadChanges()
		}
	}
	if old := m.file; old != nil {
		f, err := m.loadFile(old.rel)
		if err != nil {
			return err
		}
		f.cursor, f.offset = clamp(old.cursor, 0, f.len()-1), old.offset
		if m.visual {
			m.anchor = clamp(m.anchor, 0, f.len()-1)
		}
		m.file = f
	}
	if m.revTree != nil {
		if err := m.loadReviews(); err != nil {
			return err
		}
	}
	if d := m.detail; d != nil {
		if err := m.showDetail(d.id); err != nil {
			m.detail = nil
		}
	}
	return m.loadComments()
}

func (m *model) refreshRows() {
	m.rows = m.tree().rows()
	m.treeCur = clamp(m.treeCur, 0, len(m.rows)-1)
}

func (m *model) selectTreeRow(path string) {
	for i, r := range m.rows {
		if r.node.path == path {
			m.treeCur = i
			return
		}
	}
}

// ── update ───────────────────────────────────────────────────────────────────

type flashDoneMsg struct{ seq int }

// editKind is what the editor was opened for.
type editKind int

const (
	editAdd editKind = iota
	editEdit
	editSummary
)

type editRequest struct {
	kind       editKind
	file       string  // editAdd: absolute path
	source     *string // editAdd: the file as the viewer showed it
	start, end int
	id         string          // editEdit
	decision   review.Decision // editSummary
	ids        []string        // editSummary: the comments it links
}

type editorDoneMsg struct {
	req  editRequest
	path string
	err  error
}

// Init asks the terminal for its background, without waiting: until it
// answers, if ever, the UI assumes a dark one.
func (m *model) Init() tea.Cmd { return tea.RequestBackgroundColor }

func (m *model) setTheme(dark bool) {
	m.theme = newTheme(dark)
	style := m.opts.Style
	if style == "" {
		style = "github"
		if dark {
			style = "monokai"
		}
	}
	m.pal = newPalette(style)
}

func (m *model) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	var cmd tea.Cmd
	margins := true
	switch msg := msg.(type) {
	case tea.WindowSizeMsg:
		m.width, m.height = msg.Width, msg.Height
	case tea.BackgroundColorMsg:
		m.setTheme(msg.IsDark())
	case flashDoneMsg:
		if msg.seq == m.flashSeq {
			m.flash = ""
		}
	case editorDoneMsg:
		cmd = m.editorDone(msg)
	case tea.KeyPressMsg:
		cmd = m.key(msg)
	case tea.MouseMsg:
		cmd = m.mouse(msg)
		margins = false // keep the text still under the pointer
	}
	m.scroll(margins)
	return m, cmd
}

func (m *model) setFlash(text string, isErr bool) tea.Cmd {
	m.flashSeq++
	seq := m.flashSeq
	m.flash, m.flashErr = text, isErr
	return tea.Tick(flashFor, func(time.Time) tea.Msg { return flashDoneMsg{seq} })
}

// fail shows an error the way the CLI prints it.
func (m *model) fail(err error) tea.Cmd { return m.setFlash("rvw: "+err.Error(), true) }

func (m *model) key(k tea.KeyPressMsg) tea.Cmd {
	s := k.String()
	if s == "ctrl+c" {
		return tea.Quit
	}
	switch {
	case m.help:
		m.help = false
		return nil
	case m.submit != nil:
		return m.submitKey(s)
	case m.compare != nil:
		return m.compareKey(s)
	case m.picker != nil:
		return m.pickerKey(k)
	case m.goLine != nil:
		m.goLineKey(k)
		return nil
	}

	if m.leader {
		m.leader = false
		switch s {
		case "p":
			m.openPicker(pickFiles)
		case "l":
			m.openPicker(pickComments)
		}
		return nil
	}
	if s == m.leaderKey() {
		m.leader, m.count = true, ""
		return nil
	}

	if p := m.prefix; p != "" {
		count := m.prefixCount
		m.prefix, m.prefixCount = "", 0
		switch p + s {
		case "gg":
			switch {
			case m.focus == paneTree:
				m.treeCur = 0
			case m.detail != nil:
				m.detail.offset = 0
			case m.file != nil:
				m.gotoLine(max(count, 1))
			}
		case "]c":
			return m.jumpComment(1)
		case "[c":
			return m.jumpComment(-1)
		case "]h":
			return m.jumpHunk(1)
		case "[h":
			return m.jumpHunk(-1)
		}
		return nil
	}
	if len(s) == 1 && s[0] >= '0' && s[0] <= '9' && (s != "0" || m.count != "") {
		m.count += s
		return nil
	}
	count, _ := strconv.Atoi(m.count)
	m.count = ""

	switch s {
	case "tab":
		m.focus = 1 - m.focus
		if m.focus == paneTree {
			m.selectShown()
		}
		return nil
	case "?":
		m.help = true
		return nil
	case "q":
		return tea.Quit
	case "s":
		m.submit = &submitMenu{}
		return nil
	case "t":
		if err := m.showSide((m.side + 1) % sides); err != nil {
			return m.fail(err)
		}
		return nil
	case "b":
		m.compare = &compareMenu{cursor: int(m.cmp)}
		return nil
	case "r":
		if err := m.reload(true); err != nil {
			return m.fail(err)
		}
		return m.setFlash("reloaded", false)
	case "g", "]", "[":
		m.prefix, m.prefixCount = s, count
		return nil
	case "esc":
		m.visual = false
		if m.focus == paneViewer && m.detail != nil {
			m.detail = nil
		}
		return nil
	}
	if m.focus == paneTree {
		return m.treeKey(s, count)
	}
	return m.viewerKey(s, count)
}

func (m *model) treeKey(s string, count int) tea.Cmd {
	if len(m.rows) == 0 {
		return nil
	}
	n := max(count, 1)
	cur := m.rows[m.treeCur].node
	switch s {
	case "j", "down":
		m.treeCur = clamp(m.treeCur+n, 0, len(m.rows)-1)
	case "k", "up":
		m.treeCur = clamp(m.treeCur-n, 0, len(m.rows)-1)
	case "G":
		m.treeCur = len(m.rows) - 1
	case "l", "right", "enter":
		if m.side == sideReviews && cur.path != standalone {
			return m.openReviewRow(cur)
		}
		if !cur.dir {
			return m.openFromTree(cur.path)
		}
		cur.expanded = !cur.expanded || s != "enter"
		m.refreshRows()
	case "h", "left":
		if cur.dir && cur.expanded {
			cur.expanded = false
			m.refreshRows()
		} else if cur.parent != nil && cur.parent != m.tree() {
			m.selectTreeRow(cur.parent.path)
		}
	}
	return nil
}

// openReviewRow shows a review or comment of the reviews side in the viewer,
// expanding a review to list its comments.
func (m *model) openReviewRow(n *node) tea.Cmd {
	if n.dir && !n.expanded {
		n.expanded = true
		m.refreshRows()
	}
	return m.openFromTree(n.path)
}

// openFromTree opens a file picked in the tree and moves to the viewer. From
// the changes view it lands on the file's first change; a file listed there
// as deleted has nothing on disk to open.
func (m *model) openFromTree(path string) tea.Cmd {
	if m.side == sideReviews {
		if err := m.showDetail(path); err != nil {
			return m.fail(err)
		}
		m.focus = paneViewer
		return nil
	}
	if m.side == sideChanges && m.status[path] == 'D' {
		return m.setFlash(path+" was deleted: nothing to show", false)
	}
	if err := m.openFile(path, 0); err != nil {
		return m.fail(err)
	}
	if f := m.file; m.side == sideChanges && len(f.hunks) > 0 {
		m.gotoLine(slices.Min(f.hunks))
		f.offset = max(0, f.cursor-m.bodyHeight()/3)
	}
	m.focus = paneViewer
	return nil
}

func (m *model) viewerKey(s string, count int) tea.Cmd {
	if m.detail != nil {
		return m.detailKey(s, count)
	}
	f := m.file
	if f == nil {
		return nil
	}
	n := max(count, 1)
	switch s {
	case "j", "down":
		f.cursor = clamp(f.cursor+n, 0, f.len()-1)
	case "k", "up":
		f.cursor = clamp(f.cursor-n, 0, f.len()-1)
	case "ctrl+d", "pgdown":
		m.halfPage(1)
	case "ctrl+u", "pgup":
		m.halfPage(-1)
	case "G":
		if count > 0 {
			m.gotoLine(count)
		} else {
			m.gotoLine(f.len())
		}
	case ":":
		empty := ""
		m.goLine = &empty
	case "V":
		if f.len() > 0 {
			m.visual, m.anchor = !m.visual, f.cursor
		}
	case "c":
		return m.startComment()
	case "e":
		return m.startEdit()
	}
	return nil
}

// leaderKey is the key that starts leader mappings.
func (m *model) leaderKey() string {
	if m.opts.Leader == "" {
		return "space"
	}
	return m.opts.Leader
}

// detailKey scrolls a review or comment page, or leaves it: o for the file
// the comment is on, esc for the file underneath.
func (m *model) detailKey(s string, count int) tea.Cmd {
	d := m.detail
	n := max(count, 1)
	switch s {
	case "j", "down":
		d.offset += n
	case "k", "up":
		d.offset -= n
	case "ctrl+d", "pgdown":
		d.offset += max(1, m.bodyHeight()/2)
	case "ctrl+u", "pgup":
		d.offset -= max(1, m.bodyHeight()/2)
	case "G":
		d.offset = len(d.layout(m.viewWidth()))
	case "o":
		if d.ev == nil {
			return m.setFlash("a review is on no one file: open one of its comments", false)
		}
		c := d.ev.Comment
		if err := m.openFile(c.File, c.StartLine); err != nil {
			return m.fail(err)
		}
		m.selectShown()
	}
	return nil
}

func (m *model) gotoLine(n int) {
	m.file.cursor = clamp(n-1, 0, m.file.len()-1)
}

func (m *model) halfPage(dir int) {
	f := m.file
	h := max(1, m.bodyHeight()/2)
	f.cursor = clamp(f.cursor+dir*h, 0, f.len()-1)
	f.offset = clamp(f.offset+dir*h, 0, max(0, f.len()-m.bodyHeight()))
}

func (m *model) jumpComment(dir int) tea.Cmd {
	f := m.file
	if f == nil || m.detail != nil || m.focus != paneViewer {
		return nil
	}
	line, ok := nextComment(f.comments, f.cursor+1, dir)
	return m.jumpTo(line, ok, dir, "comment")
}

func (m *model) jumpHunk(dir int) tea.Cmd {
	f := m.file
	if f == nil || m.detail != nil || m.focus != paneViewer {
		return nil
	}
	if len(f.hunks) == 0 {
		return m.setFlash("no changes in this file", false)
	}
	if line, ok := nearest(f.hunks, f.cursor+1, dir); ok {
		m.gotoLine(line)
		return nil
	}
	// Past the last change, go round to the first; before the first, to the last.
	if dir > 0 {
		m.gotoLine(slices.Min(f.hunks))
		return m.setFlash("wrapped to the first change", false)
	}
	m.gotoLine(slices.Max(f.hunks))
	return m.setFlash("wrapped to the last change", false)
}

// jumpTo moves the cursor to line, or, when there is none (!ok), says there
// is no next or previous what.
func (m *model) jumpTo(line int, ok bool, dir int, what string) tea.Cmd {
	if !ok {
		word := "next"
		if dir < 0 {
			word = "previous"
		}
		return m.setFlash("no "+word+" "+what+" in this file", false)
	}
	m.gotoLine(line)
	return nil
}

func (m *model) goLineKey(k tea.KeyPressMsg) {
	switch s := k.String(); {
	case s == "esc":
		m.goLine = nil
	case s == "enter":
		if n, err := strconv.Atoi(*m.goLine); err == nil && m.file != nil {
			m.gotoLine(n)
		}
		m.goLine = nil
	case s == "backspace":
		if *m.goLine == "" {
			m.goLine = nil
		} else {
			*m.goLine = (*m.goLine)[:len(*m.goLine)-1]
		}
	case len(s) == 1 && s[0] >= '0' && s[0] <= '9':
		*m.goLine += s
	}
}

// scroll keeps every cursor on screen after an update, with a few lines of
// context around it when margins is set.
func (m *model) scroll(margins bool) {
	if m.height == 0 {
		return
	}
	body := m.bodyHeight()
	margin := func(n int) int {
		if !margins {
			return 0
		}
		return min(n, (body-1)/2)
	}
	if d := m.detail; d != nil {
		d.offset = clamp(d.offset, 0, max(0, len(d.layout(m.viewWidth()))-body))
	} else if f := m.file; f != nil {
		f.offset = follow(f.cursor, f.offset, body, f.len(), margin(3))
	}
	m.treeOff = follow(m.treeCur, m.treeOff, body, len(m.rows), margin(2))
	if p := m.picker; p != nil {
		p.cursor = clamp(p.cursor, 0, len(p.matches)-1)
		p.offset = follow(p.cursor, p.offset, m.pickerRows(), len(p.matches), 0)
	}
}

// follow scrolls a window of height rows over n items so cur stays at least
// margin rows from its edges.
func follow(cur, off, height, n, margin int) int {
	margin = max(0, margin)
	if cur < off+margin {
		off = cur - margin
	}
	if cur > off+height-1-margin {
		off = cur - height + 1 + margin
	}
	return clamp(off, 0, max(0, n-height))
}

func clamp(v, lo, hi int) int {
	if hi < lo {
		return lo
	}
	return max(lo, min(v, hi))
}

// ── writing ──────────────────────────────────────────────────────────────────

func (m *model) startComment() tea.Cmd {
	f := m.file
	switch {
	case f.binary:
		return m.setFlash("rvw: a binary file cannot be commented on", true)
	case f.len() == 0:
		return m.setFlash("rvw: an empty file has no line to comment on", true)
	}
	req := m.commentRequest()
	loc := location(f.rel, req.start, req.end)
	context := append([]string{"New comment on " + loc}, quoteCode(f.plain[req.start-1:req.end], req.start)...)
	return m.edit(template("", context), req)
}

// commentRequest is a new comment on the selection, carrying the file as the
// viewer shows it: the snapshot is what the reviewer read, even if the file
// changes while the editor is open.
func (m *model) commentRequest() editRequest {
	start, end := m.selection()
	return editRequest{kind: editAdd, file: m.file.abs, source: &m.file.source, start: start, end: end}
}

func (m *model) startEdit() tea.Cmd {
	f := m.file
	cs := covering(f.comments, f.cursor+1)
	if len(cs) == 0 {
		return m.setFlash("no comment on this line", false)
	}
	c := cs[len(cs)-1]
	context := append([]string{"Editing " + c.ID + " on " + c.Location()}, quoteCode(strings.Split(c.Code, "\n"), c.StartLine)...)
	return m.edit(template(c.Comment, context), editRequest{kind: editEdit, id: c.ID})
}

func (m *model) startSummary(d review.Decision) tea.Cmd {
	req := m.summaryRequest(d)
	context := []string{"Review: " + string(d)}
	if len(req.ids) > 0 {
		context = append(context, "", "Links your pending comments:")
		for _, c := range m.myPending() {
			context = append(context, fmt.Sprintf("  %s  %s  %s", c.ID, c.Location(), firstLine(c.Comment)))
		}
	} else {
		context = append(context, "", "You have no pending comments here: the review is summary-only.")
	}
	return m.edit(template("", context), req)
}

// summaryRequest is a review over the pending comments the editor will list,
// named by id, so one queued meanwhile is not linked unseen.
func (m *model) summaryRequest(d review.Decision) editRequest {
	ids := []string{}
	for _, c := range m.myPending() {
		ids = append(ids, c.ID)
	}
	return editRequest{kind: editSummary, decision: d, ids: ids}
}

// myPending is what an id-less submit will link, to show before submitting.
func (m *model) myPending() []review.Comment {
	var out []review.Comment
	for _, c := range m.open {
		if c.Status == review.StatusPending && c.ReviewID == "" &&
			deref(c.Author) == m.opts.Author && deref(c.Lane) == m.opts.Lane {
			out = append(out, c)
		}
	}
	return out
}

// edit opens the user's editor on a temp file; editorDone reads it back.
func (m *model) edit(content string, req editRequest) tea.Cmd {
	path, err := writeTemp(content)
	if err != nil {
		return m.fail(err)
	}
	return tea.ExecProcess(editorCmd(m.opts.Editor, path), func(err error) tea.Msg {
		return editorDoneMsg{req: req, path: path, err: err}
	})
}

// editorDone saves what the user wrote. The note's file goes only once it is
// saved: when saving fails, the error says where the note was kept.
func (m *model) editorDone(msg editorDoneMsg) tea.Cmd {
	if msg.err != nil {
		return m.fail(fmt.Errorf("editor '%s' failed: %w — note kept at %s", m.opts.Editor, msg.err, msg.path))
	}
	data, err := os.ReadFile(msg.path)
	if err != nil {
		return m.fail(err)
	}
	text := stripTemplate(string(data))
	if text == "" {
		os.Remove(msg.path)
		return m.setFlash("cancelled: empty note", false)
	}
	flash, err := m.save(msg.req, text)
	if err != nil {
		return m.fail(fmt.Errorf("%w — note kept at %s", err, msg.path))
	}
	os.Remove(msg.path)
	if err := m.reload(false); err != nil {
		return m.fail(err)
	}
	return m.setFlash(flash, false)
}

// save makes the one service call an edit request stands for, and says what
// it did.
func (m *model) save(req editRequest, text string) (string, error) {
	svc, ws := m.opts.Service, m.opts.Workspace
	switch req.kind {
	case editAdd:
		c, err := svc.Add(m.ctx, review.AddInput{
			Workspace: ws, File: req.file, StartLine: req.start, EndLine: req.end, Source: req.source,
			Comment: text, Lane: m.opts.Lane, Author: m.opts.Author,
		})
		if err != nil {
			return "", err
		}
		m.visual = false
		return "queued " + c.ID + " · " + c.Location(), nil
	case editEdit:
		c, err := svc.Edit(m.ctx, review.EditInput{Workspace: ws, ID: req.id, Comment: text})
		if err != nil {
			return "", err
		}
		return "edited " + c.ID + " · " + c.Location(), nil
	default:
		r, err := svc.Submit(m.ctx, review.SubmitInput{
			Workspace: ws, Decision: req.decision, Summary: text, Lane: m.opts.Lane, Author: m.opts.Author,
			CommentIDs: req.ids, NoComments: len(req.ids) == 0,
		})
		if err != nil {
			return "", err
		}
		return fmt.Sprintf("submitted %s · %s · %s", r.ID, r.Decision, plural(len(r.CommentIDs), "comment")), nil
	}
}

// ── submit menu ──────────────────────────────────────────────────────────────

type submitMenu struct{ cursor int }

func (m *model) submitKey(s string) tea.Cmd {
	menu := m.submit
	switch s {
	case "esc", "q":
		m.submit = nil
	case "j", "down", "ctrl+n":
		menu.cursor = min(menu.cursor+1, len(review.Decisions)-1)
	case "k", "up", "ctrl+p":
		menu.cursor = max(menu.cursor-1, 0)
	case "1", "2", "3":
		menu.cursor = int(s[0] - '1')
		fallthrough
	case "enter":
		m.submit = nil
		return m.startSummary(review.Decisions[menu.cursor])
	}
	return nil
}

// ── compare menu ─────────────────────────────────────────────────────────────

type compareMenu struct{ cursor int }

func (m *model) compareKey(s string) tea.Cmd {
	menu := m.compare
	switch s {
	case "esc", "q":
		m.compare = nil
	case "j", "down", "ctrl+n":
		menu.cursor = min(menu.cursor+1, len(compareLabels)-1)
	case "k", "up", "ctrl+p":
		menu.cursor = max(menu.cursor-1, 0)
	case "1", "2", "3":
		menu.cursor = int(s[0] - '1')
		fallthrough
	case "enter":
		m.compare = nil
		if err := m.setCompare(compare(menu.cursor)); err != nil {
			return m.fail(err)
		}
	}
	return nil
}

// ── helpers ──────────────────────────────────────────────────────────────────

func location(file string, start, end int) string {
	if start == end {
		return fmt.Sprintf("%s:%d", file, start)
	}
	return fmt.Sprintf("%s:%d-%d", file, start, end)
}

// firstLine is the first line of s, ready to draw.
func firstLine(s string) string {
	s, _, _ = strings.Cut(strings.TrimSpace(s), "\n")
	return expandTabs(s)
}

func deref(s *string) string {
	if s == nil {
		return ""
	}
	return *s
}

func plural(n int, word string) string {
	if n == 1 {
		return "1 " + word
	}
	return fmt.Sprintf("%d %ss", n, word)
}
