package render

import (
	"io"
	"slices"
	"strings"
	"unicode/utf8"
)

// IsControl reports whether a terminal would act on r instead of printing it:
// a C0 control other than newline and tab, DEL, or a C1 control. Text from
// agents and files may carry these to hide text, rewrite lines, set the
// window title or the clipboard.
func IsControl(r rune) bool {
	return (r < 0x20 && r != '\n' && r != '\t') || (r >= 0x7f && r <= 0x9f)
}

// Safe is text with every control character shown as U+FFFD.
func Safe(text string) string {
	return strings.Map(func(r rune) rune {
		if IsControl(r) {
			return utf8.RuneError
		}
		return r
	}, text)
}

// Terminal wraps w so that everything written through it is [Safe]. A rune
// split across writes is held back until it is whole; Flush writes what is
// left.
type Terminal struct {
	w    io.Writer
	tail []byte
}

// NewTerminal returns a [Terminal] writing to w.
func NewTerminal(w io.Writer) *Terminal { return &Terminal{w: w} }

func (t *Terminal) Write(p []byte) (int, error) {
	buf := slices.Concat(t.tail, p)
	n := len(buf)
	for i := n - 1; i >= 0 && i >= n-utf8.UTFMax; i-- {
		if utf8.RuneStart(buf[i]) {
			if !utf8.FullRune(buf[i:]) {
				n = i
			}
			break
		}
	}
	t.tail = append([]byte(nil), buf[n:]...)
	if _, err := io.WriteString(t.w, Safe(string(buf[:n]))); err != nil {
		return 0, err
	}
	return len(p), nil
}

// Flush writes a rune left incomplete by the last write.
func (t *Terminal) Flush() error {
	if len(t.tail) == 0 {
		return nil
	}
	_, err := io.WriteString(t.w, Safe(string(t.tail)))
	t.tail = nil
	return err
}
