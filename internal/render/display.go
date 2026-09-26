package render

import (
	"fmt"
	"strconv"
	"strings"
	"unicode"
	"unicode/utf8"

	"github.com/gustavofsantos/rvw/internal/review"
)

// width is the display column limit; no rendered line exceeds it.
const width = 80

type sheet []string

// line appends text wrapped to the display width, continuation lines indented
// under the prefix.
func (s *sheet) line(text, prefix string) {
	chunks := wrap(strings.ReplaceAll(text, "\t", "        "), max(1, width-utf8.RuneCountInString(prefix)))
	*s = append(*s, prefix+chunks[0])
	indent := strings.Repeat(" ", utf8.RuneCountInString(prefix))
	for _, c := range chunks[1:] {
		*s = append(*s, indent+c)
	}
}

func (s *sheet) raw(lines ...string) { *s = append(*s, lines...) }

func (s *sheet) rule() { s.raw(strings.Repeat("─", width)) }

func (s sheet) String() string { return strings.TrimRight(strings.Join(s, "\n"), " \t\n") + "\n" }

func (s *sheet) source(c review.Comment) {
	s.raw("SOURCE")
	digits := len(strconv.Itoa(c.EndLine))
	for i, code := range strings.Split(c.Code, "\n") {
		n := c.StartLine + i
		marker := " "
		if n <= c.EndLine {
			marker = ">"
		}
		s.line(code, fmt.Sprintf("%s %*d | ", marker, digits, n))
	}
}

func (s *sheet) note(text string) {
	s.raw("", "NOTE")
	for l := range strings.SplitSeq(text, "\n") {
		s.line(l, "  ")
	}
}

// Display is one comment as a fixed-width review line: its source (or, once
// done, the diff it produced), and its note or resolution.
func Display(ev review.Evidence) string {
	c := ev.Comment
	var s sheet
	s.line(fmt.Sprintf("REVIEW LINE %s  [%s]  %s", c.ID, strings.ToUpper(string(c.Status)), c.Location()), "")
	if who := Attribution(c.Author, c.Lane); who != "" {
		s.line("REVIEWER "+who, "")
	}
	s.rule()

	if c.Status == review.StatusDone {
		s.raw("DIFF")
		switch {
		case !ev.DiffAvailable:
			s.line("snapshot evidence unavailable", "  ")
		case len(ev.Diff) == 0:
			s.line("no changes recorded", "  ")
		default:
			for _, d := range ev.Diff {
				s.line(d, "  ")
			}
		}
	} else {
		s.source(c)
	}

	if !c.Status.Resolved() {
		s.note(c.Comment)
		return s.String()
	}
	s.raw("", "RESOLUTION")
	if c.ResolvedBy != nil || c.ResolvedAt != nil {
		details := string(c.Status)
		if c.ResolvedBy != nil {
			details += " by @" + *c.ResolvedBy
		}
		if c.ResolvedAt != nil {
			details += " at " + *c.ResolvedAt
		}
		s.line(details, "  ")
	}
	if c.ResolutionNote != nil {
		for l := range strings.SplitSeq(*c.ResolutionNote, "\n") {
			s.line(l, "  ")
		}
	}
	return s.String()
}

// Sheet is a submitted review: decision, summary, a ledger of its comments in
// order (missing ones named), then each comment's evidence.
func Sheet(rs review.ReviewSheet) string {
	r := rs.Review
	byID := map[string]review.Comment{}
	for _, c := range rs.Comments {
		byID[c.ID] = c
	}
	var s sheet
	s.line(fmt.Sprintf("REVIEW SHEET %s  [%s]", r.ID, strings.ToUpper(string(rs.State))), "")
	if who := Attribution(r.Author, r.Lane); who != "" {
		s.line("REVIEWER "+who, "")
	}
	s.line("DECISION  "+strings.ToUpper(strings.ReplaceAll(string(r.Decision), "-", " ")), "")
	s.rule()
	s.raw("SUMMARY")
	for l := range strings.SplitSeq(r.Summary, "\n") {
		s.line(l, "  ")
	}

	s.raw("", "LEDGER")
	if len(r.CommentIDs) == 0 {
		s.line("(no linked comments)", "  ")
	}
	for i, id := range r.CommentIDs {
		c, ok := byID[id]
		if !ok {
			s.line("MISSING "+id, "  ")
			continue
		}
		s.line(fmt.Sprintf("%d. %s  %s  [%s]", i+1, c.ID, c.Location(), strings.ToUpper(string(c.Status))), "  ")
	}

	s.rule()
	for i, id := range r.CommentIDs {
		c, ok := byID[id]
		if !ok {
			continue
		}
		if i > 0 {
			s.raw("")
		}
		s.line(fmt.Sprintf("EVIDENCE %d  %s  %s", i+1, c.ID, c.Location()), "")
		s.source(c)
		s.note(c.Comment)
	}
	return s.String()
}

// wrap fills text into lines of at most limit runes, breaking at whitespace
// and splitting words longer than a line. Whitespace is kept as written.
func wrap(text string, limit int) []string {
	var lines []string
	var cur []rune
	for _, tok := range tokens(text) {
		for len(tok) > 0 {
			if len(cur)+len(tok) <= limit {
				cur = append(cur, tok...)
				break
			}
			if len(tok) <= limit && len(cur) > 0 {
				lines = append(lines, string(cur))
				cur = nil
				continue
			}
			room := limit - len(cur)
			cur = append(cur, tok[:room]...)
			tok = tok[room:]
			lines = append(lines, string(cur))
			cur = nil
		}
	}
	if len(cur) > 0 || len(lines) == 0 {
		lines = append(lines, string(cur))
	}
	return lines
}

// tokens splits text into alternating runs of whitespace and non-whitespace.
func tokens(text string) [][]rune {
	var out [][]rune
	var cur []rune
	space := false
	for i, r := range text {
		isSpace := unicode.IsSpace(r)
		if i > 0 && isSpace != space && len(cur) > 0 {
			out = append(out, cur)
			cur = nil
		}
		space = isSpace
		cur = append(cur, r)
	}
	if len(cur) > 0 {
		out = append(out, cur)
	}
	return out
}
