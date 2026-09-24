package tui

import (
	"slices"
	"strings"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"
)

type pickerKind int

const (
	pickFiles pickerKind = iota
	pickComments
)

// picker is the fuzzy-finder overlay: quick open over files, or the list of
// open comments.
type picker struct {
	kind    pickerKind
	query   string
	cands   []candidate
	targets []target
	ids     []string // comment ids, shown before the label
	matches []ranked
	cursor  int
	offset  int
}

// target is where choosing a row goes.
type target struct {
	file string
	line int // 0: where the file was left
}

func (m *model) openPicker(kind pickerKind) {
	p := &picker{kind: kind}
	switch kind {
	case pickFiles:
		seen := map[string]bool{}
		add := func(f string) {
			if !seen[f] {
				seen[f] = true
				p.cands = append(p.cands, candidate{label: f, comments: m.counts[f]})
				p.targets = append(p.targets, target{file: f})
			}
		}
		for _, f := range m.recent {
			if slices.Contains(m.files, f) {
				add(f)
			}
		}
		for _, f := range m.files {
			add(f)
		}
	case pickComments:
		width := 0
		for _, c := range m.open {
			width = max(width, ansi.StringWidth(c.Location()))
		}
		width = min(width, 40)
		for _, c := range m.open {
			loc := c.Location()
			label := loc + strings.Repeat(" ", max(0, width-ansi.StringWidth(loc))) + "  " + firstLine(c.Comment)
			p.cands = append(p.cands, candidate{label: label})
			p.targets = append(p.targets, target{file: c.File, line: c.StartLine})
			p.ids = append(p.ids, c.ID)
		}
	}
	p.filter()
	m.picker = p
}

func (p *picker) filter() {
	p.matches = rank(p.query, p.cands)
	p.cursor, p.offset = 0, 0
}

func (m *model) pickerKey(k tea.KeyPressMsg) tea.Cmd {
	p := m.picker
	switch k.String() {
	case "esc":
		m.picker = nil
	case "enter":
		m.picker = nil
		if len(p.matches) == 0 {
			return nil
		}
		t := p.targets[p.matches[p.cursor].index]
		if err := m.openFile(t.file, t.line); err != nil {
			return m.fail(err)
		}
		m.focus = paneViewer
	case "ctrl+n", "down":
		p.cursor = min(p.cursor+1, len(p.matches)-1)
	case "ctrl+p", "up":
		p.cursor = max(p.cursor-1, 0)
	case "backspace":
		if r := []rune(p.query); len(r) > 0 {
			p.query = string(r[:len(r)-1])
			p.filter()
		}
	case "ctrl+u":
		p.query = ""
		p.filter()
	case "ctrl+w":
		q := strings.TrimRight(p.query, " /")
		p.query = q[:strings.LastIndexAny(q, " /")+1]
		p.filter()
	default:
		if k.Text != "" && k.Mod&^tea.ModShift == 0 {
			p.query += k.Text
			p.filter()
		}
	}
	return nil
}

// pickerRows is how many rows the overlay lists.
func (m *model) pickerRows() int { return clamp(m.height-12, 3, 15) }

// truncateLeft cuts a path from the left to fit width w, keeping the file
// name, and prefers to cut at a directory boundary: "…/handlers/v2/x.go".
// It returns the text and how many runes of s were dropped.
func truncateLeft(s string, w int) (string, int) {
	if ansi.StringWidth(s) <= w {
		return s, 0
	}
	if w < 1 {
		return "", len([]rune(s))
	}
	rs := []rune(s)
	cut := 0
	for cut < len(rs) && ansi.StringWidth(string(rs[cut:])) > w-1 {
		cut++
	}
	if cut < len(rs) && rs[cut] != '/' {
		if k := slices.Index(rs[cut:], '/'); k >= 0 {
			cut += k
		}
	}
	return "…" + string(rs[cut:]), cut
}

func (p *picker) title() string {
	if p.kind == pickComments {
		return "Open comments"
	}
	return "Go to file"
}
