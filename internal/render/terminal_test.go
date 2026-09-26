package render

import (
	"strings"
	"testing"
)

func TestTerminalShowsControlsEvenWhenARuneIsSplitAcrossWrites(t *testing.T) {
	var b strings.Builder
	w := NewTerminal(&b)
	for _, chunk := range []string{"a\x1b[8m\tb\n", "\xc2", "\x9b“", "\xe2\x80", "\x9d", "\x7fz"} {
		if _, err := w.Write([]byte(chunk)); err != nil {
			t.Fatal(err)
		}
	}
	if err := w.Flush(); err != nil {
		t.Fatal(err)
	}
	if got, want := b.String(), "a�[8m\tb\n�“”�z"; got != want {
		t.Errorf("got %q, want %q", got, want)
	}
}
