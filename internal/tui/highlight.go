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

// palette maps token types to lipgloss styles, from a chroma style or, with
// no style named, from the terminal's own ANSI colors.
type palette struct {
	style *chroma.Style
	cache map[chroma.TokenType]lipgloss.Style
}

// newPalette returns the palette for a chroma style; an empty name uses the
// terminal's colors, and an unknown one chroma's fallback.
func newPalette(name string) *palette {
	p := &palette{cache: map[chroma.TokenType]lipgloss.Style{}}
	if name != "" {
		p.style = styles.Get(name)
		if p.style == nil {
			p.style = styles.Fallback
		}
	}
	return p
}

func (p *palette) get(t chroma.TokenType) lipgloss.Style {
	if st, ok := p.cache[t]; ok {
		return st
	}
	st := ansiStyle(t)
	if p.style != nil {
		st = chromaStyle(p.style.Get(t))
	}
	p.cache[t] = st
	return st
}

func chromaStyle(e chroma.StyleEntry) lipgloss.Style {
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
	return st
}

// ansiColors colors token types with the terminal's 16-color palette, so code
// follows the terminal's theme, light or dark. A type without an entry takes
// its subcategory's, then its category's; text and operators stay plain.
var ansiColors = map[chroma.TokenType]string{
	chroma.Keyword:             "5",
	chroma.KeywordType:         "3",
	chroma.KeywordConstant:     "3",
	chroma.NameBuiltin:         "6",
	chroma.NameClass:           "3",
	chroma.NameConstant:        "3",
	chroma.NameDecorator:       "6",
	chroma.NameException:       "3",
	chroma.NameFunction:        "4",
	chroma.NameFunctionMagic:   "4",
	chroma.NameTag:             "4",
	chroma.NameAttribute:       "3",
	chroma.NameLabel:           "6",
	chroma.LiteralString:       "2",
	chroma.LiteralStringEscape: "6",
	chroma.LiteralStringRegex:  "6",
	chroma.LiteralNumber:       "3",
	chroma.Comment:             "8",
	chroma.CommentPreproc:      "6",
	chroma.GenericDeleted:      "1",
	chroma.GenericInserted:     "2",
	chroma.GenericHeading:      "4",
	chroma.GenericSubheading:   "6",
	chroma.GenericError:        "1",
	chroma.Error:               "1",
}

func ansiStyle(t chroma.TokenType) lipgloss.Style {
	st := lipgloss.NewStyle()
	for _, k := range []chroma.TokenType{t, t.SubCategory(), t.Category()} {
		if c, ok := ansiColors[k]; ok {
			st = st.Foreground(lipgloss.Color(c))
			break
		}
	}
	switch {
	case t.InCategory(chroma.Comment) && t != chroma.CommentPreproc:
		st = st.Italic(true)
	case t == chroma.GenericHeading || t == chroma.GenericSubheading || t == chroma.GenericStrong:
		st = st.Bold(true)
	case t == chroma.GenericEmph:
		st = st.Italic(true)
	}
	return st
}
