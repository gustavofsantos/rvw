package tui

import (
	"fmt"
	"image/color"
	"strings"

	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"
	"github.com/charmbracelet/x/ansi"
	"github.com/gustavofsantos/rvw/internal/review"
)

var (
	colAccent = lipgloss.Color("12")
	colSign   = lipgloss.Color("11")
	colDim    = lipgloss.Color("8")

	stPlain     = lipgloss.NewStyle()
	stBorder    = lipgloss.NewStyle().Foreground(colDim)
	stTitle     = lipgloss.NewStyle().Foreground(colAccent).Bold(true)
	stDim       = lipgloss.NewStyle().Foreground(colDim)
	stFaint     = lipgloss.NewStyle().Faint(true)
	stLineNr    = lipgloss.NewStyle().Foreground(colDim)
	stLineNrCur = lipgloss.NewStyle().Bold(true)
	stRailOn    = lipgloss.NewStyle().Foreground(colSign).Bold(true)
	stRailOff   = lipgloss.NewStyle().Foreground(lipgloss.Color("3"))
	stRailAway  = lipgloss.NewStyle().Foreground(colDim).Faint(true)
	stDir       = lipgloss.NewStyle().Foreground(lipgloss.Color("4"))
	stCurrent   = lipgloss.NewStyle().Foreground(colAccent).Bold(true)
	stBadge     = lipgloss.NewStyle().Foreground(lipgloss.Color("3"))
	stMatch     = lipgloss.NewStyle().Foreground(colSign).Bold(true)
	stID        = lipgloss.NewStyle().Foreground(colSign)
	stError     = lipgloss.NewStyle().Foreground(lipgloss.Color("9"))
	stNotice    = lipgloss.NewStyle().Foreground(lipgloss.Color("10"))
	stMode      = lipgloss.NewStyle().Foreground(colAccent).Bold(true)

	signStyles = [...]lipgloss.Style{
		signNone:       stPlain,
		signAdded:      lipgloss.NewStyle().Foreground(lipgloss.Color("2")),
		signChanged:    lipgloss.NewStyle().Foreground(lipgloss.Color("4")),
		signDeleted:    lipgloss.NewStyle().Foreground(lipgloss.Color("1")),
		signDeletedTop: lipgloss.NewStyle().Foreground(lipgloss.Color("1")),
	}
)

// theme holds the colors that depend on the terminal's background.
type theme struct {
	cursor, selection color.Color
}

func newTheme(dark bool) theme {
	pick := lipgloss.LightDark(dark)
	return theme{
		cursor:    pick(lipgloss.Color("254"), lipgloss.Color("236")),
		selection: pick(lipgloss.Color("153"), lipgloss.Color("24")),
	}
}

// maxBar caps the bottom bar's height.
const maxBar = 5

// piece is styled text; a row is pieces, so a background can span them all.
type piece struct {
	text string
	st   lipgloss.Style
}

func render(ps []piece, bg color.Color) string {
	var b strings.Builder
	for _, p := range ps {
		if p.text == "" {
			continue
		}
		st := p.st
		if bg != nil {
			st = st.Background(bg)
		}
		b.WriteString(st.Render(p.text))
	}
	return b.String()
}

func width(ps []piece) int {
	w := 0
	for _, p := range ps {
		w += ansi.StringWidth(p.text)
	}
	return w
}

// fit pads or truncates plain text to exactly w columns.
func fit(s string, w int) string {
	if sw := ansi.StringWidth(s); sw <= w {
		return s + strings.Repeat(" ", w-sw)
	}
	s = ansi.Truncate(s, w, "…")
	return s + strings.Repeat(" ", max(0, w-ansi.StringWidth(s)))
}

// fitPieces truncates pieces to w columns, marking the cut with "…", and
// pads them to exactly w.
func fitPieces(ps []piece, w int) []piece {
	var out []piece
	left := w
	for _, p := range ps {
		pw := ansi.StringWidth(p.text)
		if pw <= left {
			out = append(out, p)
			left -= pw
			continue
		}
		t := ansi.Truncate(p.text, left, "…")
		out = append(out, piece{t, p.st})
		left -= ansi.StringWidth(t)
		break
	}
	return append(out, piece{strings.Repeat(" ", max(0, left)), stPlain})
}

// ── layout ───────────────────────────────────────────────────────────────────

func (m *model) treeWidth() int { return clamp(m.width/4, 16, 36) }

func (m *model) viewWidth() int { return m.width - m.treeWidth() - 3 }

func (m *model) bodyHeight() int { return max(1, m.height-3-len(m.bar())) }

func (m *model) View() tea.View {
	v := tea.NewView(m.screen())
	v.AltScreen = true
	v.MouseMode = tea.MouseModeCellMotion
	return v
}

// screen is the whole terminal, one string per row joined by newlines.
func (m *model) screen() string {
	if m.width < 40 || m.height < 10 {
		return "rvw: the terminal is too small"
	}
	lines := m.main()
	var box []string
	switch {
	case m.picker != nil:
		box = m.pickerBox()
	case m.submit != nil:
		box = m.submitBox()
	case m.help:
		box = m.helpBox()
	}
	if box != nil {
		lines = overlay(lines, box, m.width)
	}
	return strings.Join(lines, "\n")
}

func (m *model) main() []string {
	tw, vw, body := m.treeWidth(), m.viewWidth(), m.bodyHeight()
	title := "no file"
	if m.file != nil {
		title = m.file.rel
	}
	out := []string{
		stBorder.Render("┌") + paneTitle("files", tw, m.focus == paneTree) + stBorder.Render("┬") +
			paneTitle(title, vw, m.focus == paneViewer) + stBorder.Render("┐"),
	}
	edge := stBorder.Render("│")
	for i := range body {
		out = append(out, edge+m.treeLine(i, tw)+edge+m.viewLine(i, vw)+edge)
	}
	out = append(out, stBorder.Render("├"+strings.Repeat("─", tw)+"┴"+strings.Repeat("─", vw)+"┤"))
	for _, b := range m.bar() {
		out = append(out, edge+" "+b+" "+edge)
	}
	return append(out, stBorder.Render("└"+strings.Repeat("─", m.width-2)+"┘"))
}

// paneTitle is a border segment w wide carrying a title: "─ files ─────".
func paneTitle(t string, w int, focused bool) string {
	t, _ = truncateLeft(t, w-4)
	st := stDim
	if focused {
		st = stTitle
	}
	rest := w - 3 - ansi.StringWidth(t)
	return stBorder.Render("─ ") + st.Render(t) + stBorder.Render(" "+strings.Repeat("─", max(0, rest)))
}

// ── tree ─────────────────────────────────────────────────────────────────────

func (m *model) treeLine(i, w int) string {
	idx := m.treeOff + i
	if idx >= len(m.rows) {
		if len(m.rows) == 0 && i == 0 {
			return render(fitPieces([]piece{{" empty workspace", stDim}}, w), nil)
		}
		return strings.Repeat(" ", w)
	}
	r := m.rows[idx]
	n := r.node
	indent := strings.Repeat("  ", r.depth)
	var name piece
	switch {
	case n.dir && n.expanded:
		name = piece{"▾ " + n.name + "/", stDir}
	case n.dir:
		name = piece{"▸ " + n.name + "/", stDir}
	case m.file != nil && n.path == m.file.rel:
		name = piece{"  " + n.name, stCurrent}
	default:
		name = piece{"  " + n.name, stPlain}
	}
	badge := ""
	if c := m.counts[n.path]; c > 0 && !n.dir {
		badge = fmt.Sprintf("💬%d", c)
	}
	avail := w - 2
	if badge != "" {
		avail -= ansi.StringWidth(badge) + 1
	}
	text := fitPieces([]piece{{" " + indent, stPlain}, name}, avail+1)
	row := append(text, piece{" ", stPlain})
	if badge != "" {
		row = append(row, piece{badge, stBadge}, piece{" ", stPlain})
	}
	var bg color.Color
	if idx == m.treeCur && m.focus == paneTree {
		bg = m.theme.cursor
	}
	return render(fitPieces(row, w), bg)
}

// ── viewer ───────────────────────────────────────────────────────────────────

func (m *model) viewLine(i, w int) string {
	f := m.file
	switch {
	case f == nil:
		if i == 0 {
			return render(fitPieces([]piece{{" No file open. C-p to find one.", stDim}}, w), nil)
		}
		return strings.Repeat(" ", w)
	case f.binary:
		if i == 0 {
			return render(fitPieces([]piece{{" binary file: nothing to show", stDim}}, w), nil)
		}
		return strings.Repeat(" ", w)
	}
	idx := f.offset + i
	if idx >= len(f.lines) {
		if len(f.lines) == 0 && i == 0 {
			return render(fitPieces([]piece{{" empty file", stDim}}, w), nil)
		}
		return strings.Repeat(" ", w)
	}
	line := idx + 1
	numWidth := max(3, len(fmt.Sprint(len(f.lines))))
	cursor := f.cursor + 1

	numSt, glyph, railSt := stLineNr, " ", stPlain
	if line == cursor {
		numSt = stLineNrCur
	}
	if c, g, ok := railAt(f.comments, line); ok {
		glyph = g
		switch {
		case covers(c, cursor):
			railSt = stRailOn
		case c.Status == review.StatusPulled:
			railSt = stRailAway
		default:
			railSt = stRailOff
		}
	}
	sg := signNone
	if idx < len(f.signs) {
		sg = f.signs[idx]
	}
	row := []piece{
		{signGlyphs[sg], signStyles[sg]},
		{fmt.Sprintf("%*d ", numWidth, line), numSt},
		{glyph, railSt},
		{" ", stPlain},
	}
	for _, seg := range f.lines[idx] {
		row = append(row, piece{seg.text, m.pal.get(seg.kind)})
	}
	var bg color.Color
	if lo, hi := m.selection(); m.visual && lo <= line && line <= hi {
		bg = m.theme.selection
	} else if line == cursor && m.focus == paneViewer {
		bg = m.theme.cursor
	}
	return render(fitPieces(row, w), bg)
}

// ── bottom bar ───────────────────────────────────────────────────────────────

// bar is the bottom bar's rows, each exactly as wide as the bar.
func (m *model) bar() []string {
	w := max(1, m.width-4)
	one := func(ps ...piece) []string { return []string{render(fitPieces(ps, w), nil)} }
	switch {
	case m.width == 0:
		return []string{""}
	case m.flash != "":
		if m.flashErr {
			return one(piece{m.flash, stError})
		}
		return one(piece{m.flash, stNotice})
	case m.goLine != nil:
		return one(piece{":" + *m.goLine + "▏", stPlain})
	case m.visual && m.file != nil:
		lo, hi := m.selection()
		return one(piece{"-- VISUAL --", stMode},
			piece{fmt.Sprintf("  %s (%s) · c comment · esc cancel", lineRange(lo, hi), plural(hi-lo+1, "line")), stDim})
	case m.focus == paneTree:
		return one(piece{"l open · h collapse · Tab viewer · C-p files · C-l comments · ? help", stDim})
	}
	if f := m.file; f != nil && !f.binary {
		if cs := covering(f.comments, f.cursor+1); len(cs) > 0 {
			return commentBar(cs, w)
		}
	}
	return one(piece{"V select · c comment · C-p files · C-l comments · s submit · ? help", stDim})
}

// commentBar lists every comment covering the cursor line, the text wrapped
// under its header, at most maxBar rows.
func commentBar(cs []review.Comment, w int) []string {
	var rows [][]piece
	for _, c := range cs {
		head := []piece{{c.ID, stID}, {" · " + lineRange(c.StartLine, c.EndLine), stDim}}
		if a := deref(c.Author); a != "" {
			head = append(head, piece{" · @" + a, stDim})
		}
		if c.Status == review.StatusPulled {
			head = append(head, piece{" (pulled)", stDim})
		}
		head = append(head, piece{"   ", stPlain})
		hw := width(head)
		textW := max(10, w-hw)
		text := strings.Split(ansi.Wrap(strings.TrimSpace(c.Comment), textW, ""), "\n")
		for i, t := range text {
			lead := head
			if i > 0 {
				lead = []piece{{strings.Repeat(" ", hw), stPlain}}
			}
			rows = append(rows, append(append([]piece{}, lead...), piece{t, stPlain}))
		}
	}
	if len(rows) > maxBar {
		rows = rows[:maxBar]
		last := rows[maxBar-1]
		rows[maxBar-1] = append(fitPieces(last, w-2), piece{" …", stDim})
	}
	out := make([]string, len(rows))
	for i, r := range rows {
		out[i] = render(fitPieces(r, w), nil)
	}
	return out
}

func lineRange(lo, hi int) string {
	if lo == hi {
		return fmt.Sprint(lo)
	}
	return fmt.Sprintf("%d-%d", lo, hi)
}

// ── overlays ─────────────────────────────────────────────────────────────────

// overlay draws a box over the dimmed main view, centered horizontally in
// the upper third.
func overlay(bg, box []string, w int) []string {
	bw := ansi.StringWidth(box[0])
	x := max(0, (w-bw)/2)
	y := max(0, (len(bg)-len(box))/3)
	out := make([]string, len(bg))
	for i, l := range bg {
		plain := ansi.Strip(l)
		if i < y || i >= y+len(box) {
			out[i] = stFaint.Render(plain)
			continue
		}
		left := fit(ansi.Truncate(plain, x, ""), x)
		right := ansi.TruncateLeft(plain, x+bw, "")
		right = strings.Repeat(" ", max(0, w-x-bw-ansi.StringWidth(right))) + right
		out[i] = stFaint.Render(left) + box[i-y] + stFaint.Render(right)
	}
	return out
}

// frame boxes rows of pieces, each padded to the inner width iw, with a title
// and an optional footer below a rule.
func frame(title string, iw int, body [][]piece, footer []piece, bodyBg map[int]color.Color) []string {
	edge := stBorder.Render("│")
	out := []string{stBorder.Render("┌─ ") + stTitle.Render(title) + stBorder.Render(" "+strings.Repeat("─", max(0, iw-3-ansi.StringWidth(title)))+"┐")}
	for i, r := range body {
		if r == nil {
			out = append(out, stBorder.Render("├"+strings.Repeat("─", iw)+"┤"))
			continue
		}
		out = append(out, edge+render(fitPieces(r, iw), bodyBg[i])+edge)
	}
	if footer != nil {
		out = append(out, stBorder.Render("├"+strings.Repeat("─", iw)+"┤"))
		out = append(out, edge+render(fitPieces(footer, iw), nil)+edge)
	}
	return append(out, stBorder.Render("└"+strings.Repeat("─", iw)+"┘"))
}

func (m *model) pickerBox() []string {
	p := m.picker
	bw := clamp(m.width-8, 30, 90)
	iw := bw - 2
	body := [][]piece{{{" > ", stTitle}, {p.query, stPlain}, {"▏", stDim}}, nil}
	bg := map[int]color.Color{}
	for i := range m.pickerRows() {
		idx := p.offset + i
		if idx >= len(p.matches) {
			body = append(body, []piece{})
			continue
		}
		if idx == p.cursor {
			bg[len(body)] = m.theme.cursor
		}
		body = append(body, p.row(p.matches[idx], idx == p.cursor, iw))
	}
	hint := "↵ open · C-n/C-p · esc "
	count := fmt.Sprintf(" %d/%d", len(p.matches), len(p.cands))
	footer := []piece{{count, stDim}, {strings.Repeat(" ", max(1, iw-ansi.StringWidth(count)-ansi.StringWidth(hint))), stPlain}, {hint, stDim}}
	return frame(p.title(), iw, body, footer, bg)
}

// row is one match: the marker, the id for a comment, the label with its
// matched characters highlighted, and a file's comment count on the right.
func (p *picker) row(r ranked, selected bool, iw int) []piece {
	marker := "   "
	if selected {
		marker = " ▸ "
	}
	out := []piece{{marker, stTitle}}
	c := p.cands[r.index]
	badge := ""
	if p.kind == pickFiles && c.comments > 0 {
		badge = fmt.Sprintf("💬%d", c.comments)
	}
	textW := iw - 3 - 1
	if p.kind == pickFiles {
		textW -= 5
	} else {
		id := p.ids[r.index]
		out = append(out, piece{fmt.Sprintf("%-5s ", id), stID})
		textW -= 6
	}
	label, pos := c.label, r.pos
	if p.kind == pickFiles {
		var cut int
		label, cut = truncateLeft(label, textW)
		if cut > 0 {
			shifted := []int{}
			for _, q := range pos {
				if q >= cut {
					shifted = append(shifted, q-cut+1)
				}
			}
			pos = shifted
		}
	}
	text := highlightMatches(label, pos)
	text = fitPieces(text, textW)
	out = append(out, text...)
	if p.kind == pickFiles {
		out = append(out, piece{fmt.Sprintf("%5s", ""), stPlain})
		if badge != "" {
			out[len(out)-1] = piece{strings.Repeat(" ", max(0, 5-ansi.StringWidth(badge))) + badge, stBadge}
		}
	}
	return out
}

// highlightMatches splits text into runs, matched runes styled as matches.
func highlightMatches(text string, pos []int) []piece {
	set := map[int]bool{}
	for _, p := range pos {
		set[p] = true
	}
	var out []piece
	var run []rune
	matched := false
	flush := func() {
		if len(run) > 0 {
			st := stPlain
			if matched {
				st = stMatch
			}
			out = append(out, piece{string(run), st})
			run = nil
		}
	}
	for i, r := range []rune(text) {
		if set[i] != matched {
			flush()
			matched = set[i]
		}
		run = append(run, r)
	}
	flush()
	return out
}

func (m *model) submitBox() []string {
	labels := []string{"comment", "approve", "request changes"}
	var body [][]piece
	bg := map[int]color.Color{}
	for i, l := range labels {
		marker := "   "
		if i == m.submit.cursor {
			marker = " ▸ "
			bg[i] = m.theme.cursor
		}
		body = append(body, []piece{{marker, stTitle}, {fmt.Sprintf("%d", i+1), stID}, {"  " + l, stPlain}})
	}
	n := len(m.myPending())
	footer := []piece{{fmt.Sprintf(" links %s · ↵ · esc ", plural(n, "pending comment")), stDim}}
	return frame("Submit review", 36, body, footer, bg)
}

var helpKeys = [][2]string{
	{"", "Global"},
	{"C-p", "go to file"},
	{"C-l", "open comments"},
	{"Tab", "switch pane"},
	{"s", "submit a review"},
	{"r", "reload files and comments"},
	{"q", "quit (everything is saved)"},
	{"", "Tree"},
	{"j/k  gg/G", "move"},
	{"l  ↵", "open file, expand directory"},
	{"h", "collapse, go to parent"},
	{"", "Viewer"},
	{"j/k  C-d/C-u", "move, half page"},
	{"gg/G  NG  :N", "first, last, line N"},
	{"]c  [c", "next, previous comment"},
	{"]h  [h", "next, previous git change"},
	{"V", "select lines (esc cancels)"},
	{"c", "comment on the line or selection"},
	{"e", "edit the comment on this line"},
	{"", "Mouse"},
	{"click", "open file, toggle directory, move"},
	{"drag", "select lines"},
	{"", "Overlays"},
	{"C-n/C-p  ↓/↑", "move"},
	{"↵  esc", "open, close"},
}

func (m *model) helpBox() []string {
	var body [][]piece
	for _, k := range helpKeys {
		if k[0] == "" {
			body = append(body, []piece{{" " + k[1], stTitle}})
			continue
		}
		body = append(body, []piece{{fmt.Sprintf("   %-14s", k[0]), stID}, {k[1], stPlain}})
	}
	body = body[:min(len(body), m.height-5)]
	return frame("Keys", 50, body, []piece{{" any key closes", stDim}}, nil)
}
