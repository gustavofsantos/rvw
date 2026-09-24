// Package tui is rvw's terminal review UI: browse the workspace, read
// highlighted source, and comment on line ranges. It is an adapter over
// [review.Service], like the CLI and the MCP server, and holds no domain
// logic: every action is one service call.
package tui

import (
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
}

// Run shows the UI on a terminal until the user quits.
func Run(ctx context.Context, opts Options, in io.Reader, out io.Writer) error {
	m, err := newModel(ctx, opts)
	if err != nil {
		return err
	}
	p := tea.NewProgram(m, tea.WithContext(ctx), tea.WithInput(in), tea.WithOutput(out))
	if _, err := p.Run(); err != nil && !(errors.Is(err, tea.ErrProgramKilled) && ctx.Err() != nil) {
		return err
	}
	return nil
}

type pane int

const (
	paneTree pane = iota
	paneViewer
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

	root    *node
	rows    []row // visible tree rows
	treeCur int
	treeOff int

	files  []string         // every file, relative
	open   []review.Comment // every open comment in the workspace
	counts map[string]int   // open comments per file

	file      *fileView
	positions map[string]int // last cursor line per file this session
	recent    []string       // files opened this session, most recent first

	visual      bool
	anchor      int
	count       string  // a pending {count}
	prefix      string  // a pending g, ] or [
	prefixCount int     // the count before a prefix, for {count}gg
	goLine      *string // the :N prompt, when open

	picker *picker
	submit *submitMenu
	help   bool

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
	binary   bool
	comments []review.Comment // open comments on this file, oldest first
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
	if err := m.loadTree(); err != nil {
		return nil, err
	}
	if err := m.loadComments(); err != nil {
		return nil, err
	}
	if first := m.root.firstFile(); first != "" {
		if err := m.openFile(first, 0); err != nil {
			m.flash, m.flashErr = "rvw: "+err.Error(), true
		}
	} else {
		m.focus = paneTree
	}
	return m, nil
}

// ── loading ──────────────────────────────────────────────────────────────────

// loadTree walks the workspace, keeping expanded directories expanded.
func (m *model) loadTree() error {
	files, err := walkFiles(m.opts.Workspace)
	if err != nil {
		return fmt.Errorf("cannot read workspace %s: %w", m.opts.Workspace, errors.Unwrap(err))
	}
	var expanded []string
	if m.root != nil {
		expanded = m.root.expandedDirs()
	}
	m.files = files
	m.root = buildTree(files)
	for _, d := range expanded {
		if n := m.root.find(d); n != nil && n.dir {
			n.expanded = true
		}
	}
	if m.file != nil {
		m.root.reveal(m.file.rel)
	}
	m.refreshRows()
	return nil
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
	f.plain = sourceLines(text)
	f.lines = highlight(rel, text)
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
	m.file = f
	m.visual = false
	m.recent = append([]string{rel}, slices.DeleteFunc(m.recent, func(r string) bool { return r == rel })...)
	m.root.reveal(rel)
	m.refreshRows()
	m.selectTreeRow(rel)
	return m.loadComments()
}

// reload re-reads the tree, the open file and the comments from disk and the
// queue; the cursor stays on its line.
func (m *model) reload(walk bool) error {
	if walk {
		if err := m.loadTree(); err != nil {
			return err
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
	return m.loadComments()
}

func (m *model) refreshRows() {
	m.rows = m.root.rows()
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
	file       string // editAdd: absolute path
	start, end int
	id         string          // editEdit
	decision   review.Decision // editSummary
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
	}
	m.scroll()
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
	case m.picker != nil:
		return m.pickerKey(k)
	case m.goLine != nil:
		m.goLineKey(k)
		return nil
	}

	if p := m.prefix; p != "" {
		count := m.prefixCount
		m.prefix, m.prefixCount = "", 0
		switch p + s {
		case "gg":
			if m.focus == paneTree {
				m.treeCur = 0
			} else if m.file != nil {
				m.gotoLine(max(count, 1))
			}
		case "]c":
			return m.jumpComment(1)
		case "[c":
			return m.jumpComment(-1)
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
	case "ctrl+p":
		m.openPicker(pickFiles)
		return nil
	case "ctrl+l":
		m.openPicker(pickComments)
		return nil
	case "tab":
		m.focus = 1 - m.focus
		if m.focus == paneTree && m.file != nil {
			m.selectTreeRow(m.file.rel)
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
		if !cur.dir {
			if err := m.openFile(cur.path, 0); err != nil {
				return m.fail(err)
			}
			m.focus = paneViewer
			return nil
		}
		cur.expanded = !cur.expanded || s != "enter"
		m.refreshRows()
	case "h", "left":
		if cur.dir && cur.expanded {
			cur.expanded = false
			m.refreshRows()
		} else if cur.parent != nil && cur.parent != m.root {
			m.selectTreeRow(cur.parent.path)
		}
	}
	return nil
}

func (m *model) viewerKey(s string, count int) tea.Cmd {
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
	if f == nil || m.focus != paneViewer {
		return nil
	}
	line, ok := nextComment(f.comments, f.cursor+1, dir)
	if !ok {
		word := "next"
		if dir < 0 {
			word = "previous"
		}
		return m.setFlash("no "+word+" comment in this file", false)
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

// scroll keeps every cursor on screen after an update.
func (m *model) scroll() {
	if m.height == 0 {
		return
	}
	body := m.bodyHeight()
	if f := m.file; f != nil {
		f.offset = follow(f.cursor, f.offset, body, f.len(), min(3, (body-1)/2))
	}
	m.treeOff = follow(m.treeCur, m.treeOff, body, len(m.rows), min(2, (body-1)/2))
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
	start, end := m.selection()
	loc := location(f.rel, start, end)
	context := append([]string{"New comment on " + loc}, quoteCode(f.plain[start-1:end], start)...)
	return m.edit(template("", context), editRequest{kind: editAdd, file: f.abs, start: start, end: end})
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
	context := []string{"Review: " + string(d)}
	if mine := m.myPending(); len(mine) > 0 {
		context = append(context, "", "Links your pending comments:")
		for _, c := range mine {
			context = append(context, fmt.Sprintf("  %s  %s  %s", c.ID, c.Location(), firstLine(c.Comment)))
		}
	} else {
		context = append(context, "", "You have no pending comments here: the review is summary-only.")
	}
	return m.edit(template("", context), editRequest{kind: editSummary, decision: d})
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

func (m *model) editorDone(msg editorDoneMsg) tea.Cmd {
	defer os.Remove(msg.path)
	if msg.err != nil {
		return m.fail(fmt.Errorf("editor '%s' failed: %v", m.opts.Editor, msg.err))
	}
	data, err := os.ReadFile(msg.path)
	if err != nil {
		return m.fail(err)
	}
	text := stripTemplate(string(data))
	if text == "" {
		return m.setFlash("cancelled: empty note", false)
	}
	svc, ws, req := m.opts.Service, m.opts.Workspace, msg.req
	var flash string
	switch req.kind {
	case editAdd:
		c, err := svc.Add(m.ctx, review.AddInput{
			Workspace: ws, File: req.file, StartLine: req.start, EndLine: req.end,
			Comment: text, Lane: m.opts.Lane, Author: m.opts.Author,
		})
		if err != nil {
			return m.fail(err)
		}
		m.visual = false
		flash = "queued " + c.ID + " · " + c.Location()
	case editEdit:
		c, err := svc.Edit(m.ctx, review.EditInput{Workspace: ws, ID: req.id, Comment: text})
		if err != nil {
			return m.fail(err)
		}
		flash = "edited " + c.ID + " · " + c.Location()
	case editSummary:
		r, err := svc.Submit(m.ctx, review.SubmitInput{
			Workspace: ws, Decision: req.decision, Summary: text, Lane: m.opts.Lane, Author: m.opts.Author,
		})
		if err != nil {
			return m.fail(err)
		}
		flash = fmt.Sprintf("submitted %s · %s · %s", r.ID, r.Decision, plural(len(r.CommentIDs), "comment"))
	}
	if err := m.reload(false); err != nil {
		return m.fail(err)
	}
	return m.setFlash(flash, false)
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

// ── helpers ──────────────────────────────────────────────────────────────────

func location(file string, start, end int) string {
	if start == end {
		return fmt.Sprintf("%s:%d", file, start)
	}
	return fmt.Sprintf("%s:%d-%d", file, start, end)
}

func firstLine(s string) string {
	s, _, _ = strings.Cut(strings.TrimSpace(s), "\n")
	return s
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
