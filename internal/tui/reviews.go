package tui

import (
	"fmt"
	"slices"
	"strings"

	"charm.land/lipgloss/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/gustavofsantos/rvw/internal/review"
)

// standalone is the path of the reviews side's group of comments that belong
// to no submitted review; no id looks like it.
const standalone = "·comments"

// loadReviews reads every submitted review and every comment outside one,
// resolved or not, for the reviews side: newest first, reviews before the
// standalone comments, every review expanded.
func (m *model) loadReviews() error {
	svc, ws := m.opts.Service, m.opts.Workspace
	sheets, err := svc.Sheets(m.ctx, review.SheetsInput{Workspace: ws, Status: review.SheetFilterAll})
	if err != nil {
		return err
	}
	all, err := svc.List(m.ctx, review.QueryInput{Workspace: ws, Status: review.FilterAll})
	if err != nil {
		return err
	}
	var collapsed []string
	selected := ""
	if m.side == sideReviews && m.treeCur < len(m.rows) {
		selected = m.rows[m.treeCur].node.path
	}
	if m.revTree != nil {
		for _, c := range m.revTree.children {
			if c.dir && !c.expanded {
				collapsed = append(collapsed, c.path)
			}
		}
	}
	root := &node{dir: true, expanded: true}
	m.sheets, m.revComments = map[string]review.ReviewSheet{}, map[string]review.Comment{}
	for _, s := range slices.Backward(sheets.Sheets) {
		m.sheets[s.Review.ID] = s
		r := &node{name: s.Review.ID, path: s.Review.ID, dir: true, expanded: !slices.Contains(collapsed, s.Review.ID), parent: root}
		for _, c := range s.Comments {
			m.revComments[c.ID] = c
			r.children = append(r.children, &node{name: c.ID, path: c.ID, parent: r})
		}
		root.children = append(root.children, r)
	}
	var loose []*node
	group := &node{name: "comments", path: standalone, dir: true, expanded: !slices.Contains(collapsed, standalone), parent: root}
	for _, c := range slices.Backward(all.Comments) {
		if c.ReviewID == "" {
			m.revComments[c.ID] = c
			loose = append(loose, &node{name: c.ID, path: c.ID, parent: group})
		}
	}
	if len(loose) > 0 {
		group.children = loose
		root.children = append(root.children, group)
	}
	m.revTree = root
	m.refreshRows()
	if selected != "" {
		m.selectTreeRow(selected) // a new review lands on top: keep the cursor on its item
	}
	return nil
}

var (
	stDone     = lipgloss.NewStyle().Foreground(lipgloss.Color("2"))
	stRejected = lipgloss.NewStyle().Foreground(lipgloss.Color("1"))
	stPulled   = lipgloss.NewStyle().Foreground(lipgloss.Color("3"))
)

// statusMark is a comment's status at a glance.
func statusMark(s review.Status) piece {
	switch s {
	case review.StatusDone:
		return piece{"✓", stDone}
	case review.StatusRejected:
		return piece{"✗", stRejected}
	case review.StatusPulled:
		return piece{"◐", stPulled}
	}
	return piece{"○", stDim}
}

// stateMark is a review's progress at a glance.
func stateMark(s review.SheetState) piece {
	switch s {
	case review.SheetComplete:
		return piece{"✓", stDone}
	case review.SheetPulled:
		return piece{"◐", stPulled}
	}
	return piece{"○", stDim}
}

// reviewRow is a row of the reviews side: the name and its mark on the right.
func (m *model) reviewRow(n *node) (name []piece, mark piece) {
	arrow := "▸ "
	if n.expanded {
		arrow = "▾ "
	}
	if s, ok := m.sheets[n.path]; ok {
		return []piece{{arrow, stDir}, {s.Review.ID, stID}, {" " + string(s.Review.Decision), stPlain}}, stateMark(s.State)
	}
	if n.path == standalone {
		return []piece{{arrow + "comments", stDir}}, piece{}
	}
	c := m.revComments[n.path]
	return []piece{{"  ", stPlain}, {c.ID, stID}, {" " + shortLocation(c), stPlain}}, statusMark(c.Status)
}

// shortLocation is a comment's file name and lines, without the directories.
func shortLocation(c review.Comment) string {
	return location(c.File[strings.LastIndex(c.File, "/")+1:], c.StartLine, c.EndLine)
}

// ── detail ───────────────────────────────────────────────────────────────────

// detail is a read-only page in the viewer about a review or a comment: what
// was asked and how it was addressed.
type detail struct {
	id     string
	sheet  *review.ReviewSheet
	ev     *review.Evidence
	offset int

	width int       // the width lines were laid out for
	lines [][]piece // laid out lazily, per width
}

// showDetail puts the page for a review or comment id in the viewer.
func (m *model) showDetail(id string) error {
	d := &detail{id: id}
	if s, ok := m.sheets[id]; ok {
		d.sheet = &s
	} else {
		ev, err := m.opts.Service.Evidence(m.ctx, review.GetInput{Workspace: m.opts.Workspace, ID: id})
		if err != nil {
			return err
		}
		d.ev = &ev
	}
	if old := m.detail; old != nil && old.id == id {
		d.offset = old.offset
	}
	m.detail, m.visual = d, false
	return nil
}

func (d *detail) title() string {
	if d.sheet != nil {
		return d.sheet.Review.ID + " · " + string(d.sheet.Review.Decision)
	}
	return d.ev.Comment.ID + " · " + d.ev.Comment.Location()
}

// layout is the page's lines at width w.
func (d *detail) layout(w int) [][]piece {
	if d.lines == nil || d.width != w {
		d.width = w
		if d.sheet != nil {
			d.lines = sheetPage(*d.sheet, w)
		} else {
			d.lines = evidencePage(*d.ev, w)
		}
	}
	return d.lines
}

// page builds a detail page: headed sections of wrapped text or raw lines.
type page struct {
	w     int
	lines [][]piece
}

func (p *page) line(ps ...piece) { p.lines = append(p.lines, append([]piece{{" ", stPlain}}, ps...)) }

func (p *page) blank() { p.lines = append(p.lines, nil) }

func (p *page) heading(t string) {
	if len(p.lines) > 0 {
		p.blank()
	}
	p.line(piece{t, stTitle})
}

// text wraps prose under a heading, indented by lead.
func (p *page) text(s string, lead string, st lipgloss.Style) {
	for l := range strings.SplitSeq(ansi.Wrap(strings.TrimSpace(s), max(10, p.w-2-ansi.StringWidth(lead)), ""), "\n") {
		p.line(piece{lead, stPlain}, piece{expandTabs(l), st})
	}
}

// who is "@name", or "someone" for an anonymous author.
func who(s *string) string {
	if a := deref(s); a != "" {
		return "@" + a
	}
	return "someone"
}

// day is the date of a stored timestamp.
func day(ts *string) string {
	d, _, _ := strings.Cut(deref(ts), "T")
	return d
}

func evidencePage(ev review.Evidence, w int) [][]piece {
	c := ev.Comment
	p := &page{w: w}
	head := []piece{{c.ID, stID}, {" · " + c.Location() + " · " + who(c.Author), stDim}}
	if c.ReviewID != "" {
		head = append(head, piece{" · in " + c.ReviewID, stDim})
	}
	p.line(head...)
	mark := statusMark(c.Status)
	switch c.Status {
	case review.StatusDone, review.StatusRejected:
		p.line(mark, piece{fmt.Sprintf(" %s by %s on %s", c.Status, who(c.ResolvedBy), day(c.ResolvedAt)), mark.st})
	case review.StatusPulled:
		p.line(mark, piece{" pulled on " + day(c.PulledAt) + ", not decided yet", mark.st})
	default:
		p.line(mark, piece{" pending since " + day(&c.CreatedAt), mark.st})
	}
	p.line(piece{fmt.Sprintf("o opens %s at line %d · esc goes back", c.File, c.StartLine), stDim})

	p.heading("Comment")
	p.text(c.Comment, "  ", stPlain)

	p.heading("Code as reviewed")
	code := sourceLines(c.Code)
	numW := len(fmt.Sprint(c.StartLine + len(code) - 1))
	for i, l := range code {
		p.line(piece{fmt.Sprintf("  %*d  ", numW, c.StartLine+i), stLineNr}, piece{expandTabs(l), stPlain})
	}

	if c.Status.Resolved() {
		p.heading("Resolution")
		if note := deref(c.ResolutionNote); note != "" {
			p.text(note, "  ", stPlain)
		} else {
			p.line(piece{"  no note", stDim})
		}
	}
	if c.Status == review.StatusDone {
		p.heading("Changes while resolving")
		switch {
		case !ev.DiffAvailable:
			p.line(piece{"  no snapshot of the file to compare", stDim})
		case len(ev.Diff) == 0:
			p.line(piece{"  nothing changed in this file", stDim})
		}
		for _, l := range ev.Diff {
			p.line(piece{"  ", stPlain}, diffPiece(l))
		}
	}
	return p.lines
}

func diffPiece(l string) piece {
	l = expandTabs(l)
	switch {
	case strings.HasPrefix(l, "+++"), strings.HasPrefix(l, "---"):
		return piece{l, stDim}
	case strings.HasPrefix(l, "@@"):
		return piece{l, signStyles[signChanged]}
	case strings.HasPrefix(l, "+"):
		return piece{l, signStyles[signAdded]}
	case strings.HasPrefix(l, "-"):
		return piece{l, signStyles[signDeleted]}
	}
	return piece{l, stPlain}
}

func sheetPage(s review.ReviewSheet, w int) [][]piece {
	r := s.Review
	p := &page{w: w}
	p.line(piece{r.ID, stID}, piece{" · " + string(r.Decision) + " · " + who(r.Author) + " · submitted on " + day(&r.CreatedAt), stDim})
	mark := stateMark(s.State)
	p.line(mark, piece{" " + string(s.State), mark.st})
	p.line(piece{"open a comment in the sidebar for its code and changes · esc goes back", stDim})

	p.heading("Summary")
	p.text(r.Summary, "  ", stPlain)

	p.heading("Comments")
	if len(r.CommentIDs) == 0 {
		p.line(piece{"  summary only: no comments linked", stDim})
	}
	byID := map[string]review.Comment{}
	for _, c := range s.Comments {
		byID[c.ID] = c
	}
	for _, id := range r.CommentIDs {
		c, ok := byID[id]
		if !ok {
			p.line(piece{"  ", stPlain}, piece{id, stID}, piece{" no longer exists", stDim})
			continue
		}
		p.line(piece{"  ", stPlain}, statusMark(c.Status), piece{" ", stPlain}, piece{c.ID, stID}, piece{" " + c.Location(), stDim})
		p.text(firstLine(c.Comment), "      ", stPlain)
		if note := deref(c.ResolutionNote); note != "" {
			p.text(who(c.ResolvedBy)+": "+note, "      ", stDim)
		}
	}
	return p.lines
}

// expandTabs is s the way the viewer shows source: tabs expanded, control
// characters made visible.
func expandTabs(s string) string {
	t, _ := displayText(s, 0)
	return t
}
