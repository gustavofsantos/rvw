package tui

import (
	"bytes"
	"path/filepath"
	"strings"

	"charm.land/lipgloss/v2"
	"github.com/alecthomas/chroma/v2"
	"github.com/alecthomas/chroma/v2/lexers"
	"github.com/alecthomas/chroma/v2/styles"
	"github.com/charmbracelet/x/ansi"

	term "github.com/gustavofsantos/rvw/internal/render"
)

const (
	tabWidth = 4
	// maxHighlight is the largest file highlighted; bigger ones are plain text.
	maxHighlight = 1 << 20
	// binaryProbe is how much of a file is searched for a NUL byte.
	binaryProbe = 8 << 10
)

// segment is a run of text on one line with one token type.
type segment struct {
	text string
	kind chroma.TokenType
}

// isBinary reports a NUL byte in the first 8 KB, the way git decides.
func isBinary(data []byte) bool {
	return bytes.IndexByte(data[:min(len(data), binaryProbe)], 0) >= 0
}

// sourceLines counts lines the way the service does: a trailing newline does
// not start another line.
func sourceLines(text string) []string {
	lines := strings.Split(text, "\n")
	if lines[len(lines)-1] == "" {
		lines = lines[:len(lines)-1]
	}
	for i, l := range lines {
		lines[i] = strings.TrimSuffix(l, "\r")
	}
	return lines
}

// highlight tokenizes a file once and splits it into lines of segments, one
// per file line. The lexer is picked by file name, then by content, then plain.
func highlight(name, text string) [][]segment {
	text = strings.ToValidUTF8(text, "\uFFFD")
	want := len(sourceLines(text))
	var tokens []chroma.Token
	if len(text) <= maxHighlight {
		tokens = tokenize(name, text)
	}
	if tokens == nil {
		tokens = []chroma.Token{{Type: chroma.Text, Value: text}}
	}
	lines := splitLines(tokens)
	// Lexers may add a final newline; the line count follows the file.
	for len(lines) < want {
		lines = append(lines, nil)
	}
	return lines[:want]
}

func tokenize(name, text string) []chroma.Token {
	lexer := lexers.Match(filepath.Base(name))
	if lexer == nil {
		lexer = lexers.Analyse(text)
	}
	if lexer == nil {
		return nil
	}
	it, err := chroma.Coalesce(lexer).Tokenise(nil, text)
	if err != nil {
		return nil
	}
	return it.Tokens()
}

// splitLines breaks tokens at newlines (a token can span several lines),
// expands tabs and makes control characters visible. Carriage returns before
// a newline are dropped.
func splitLines(tokens []chroma.Token) [][]segment {
	lines := [][]segment{nil}
	col := 0
	for _, tok := range tokens {
		parts := strings.Split(tok.Value, "\n")
		for i, part := range parts {
			if i > 0 {
				lines = append(lines, nil)
				col = 0
			}
			if i < len(parts)-1 {
				part = strings.TrimSuffix(part, "\r")
			}
			if part == "" {
				continue
			}
			var text string
			text, col = displayText(part, col)
			last := &lines[len(lines)-1]
			*last = append(*last, segment{text: text, kind: tok.Type})
		}
	}
	return lines
}

// displayText expands tabs to the next tab stop from column col and replaces
// control characters, returning the text and the column after it.
func displayText(s string, col int) (string, int) {
	var b strings.Builder
	for _, r := range s {
		switch {
		case r == '\t':
			n := tabWidth - col%tabWidth
			b.WriteString(strings.Repeat(" ", n))
			col += n
			continue
		case r == '\n' || term.IsControl(r):
			r = '\uFFFD'
		}
		b.WriteRune(r)
		col += ansi.StringWidth(string(r))
	}
	return b.String(), col
}

// palette maps token types to lipgloss styles for one chroma style.
type palette struct {
	style *chroma.Style
	cache map[chroma.TokenType]lipgloss.Style
}

func newPalette(name string) *palette {
	s := styles.Get(name)
	if s == nil {
		s = styles.Fallback
	}
	return &palette{style: s, cache: map[chroma.TokenType]lipgloss.Style{}}
}

func (p *palette) get(t chroma.TokenType) lipgloss.Style {
	if st, ok := p.cache[t]; ok {
		return st
	}
	e := p.style.Get(t)
	st := lipgloss.NewStyle()
	if e.Colour.IsSet() {
		st = st.Foreground(lipgloss.Color(e.Colour.String()))
	}
	if e.Bold == chroma.Yes {
		st = st.Bold(true)
	}
	if e.Italic == chroma.Yes {
		st = st.Italic(true)
	}
	p.cache[t] = st
	return st
}
