package tui

import (
	"testing"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"
	"github.com/charmbracelet/x/exp/golden"
	"github.com/charmbracelet/x/exp/teatest/v2"
)

// Golden views at 100x30, colors stripped: they pin the layout. Regenerate
// with `go test ./internal/tui -update`.

// view drives a program through keys and returns its final screen.
func view(t *testing.T, f *fixture, msgs ...tea.Msg) []byte {
	t.Helper()
	tm := teatest.NewTestModel(t, f.model(), teatest.WithInitialTermSize(100, 30))
	for _, msg := range msgs {
		tm.Send(msg)
	}
	if err := tm.Quit(); err != nil {
		t.Fatal(err)
	}
	final := tm.FinalModel(t, teatest.WithFinalTimeout(5*time.Second))
	return []byte(ansi.Strip(final.(*model).screen()))
}

// seq is key presses: named keys (see press), and strings typed out.
func seq(ks ...string) []tea.Msg {
	var out []tea.Msg
	for _, k := range ks {
		if press(k).Text == "" {
			out = append(out, press(k))
			continue
		}
		for _, r := range k {
			out = append(out, press(string(r)))
		}
	}
	return out
}

func commented(t *testing.T) *fixture {
	f := setup(t)
	f.add("src/api/parse.py", 5, 8, "extract this branch into parse_x()", "gustavo")
	f.add("src/api/parse.py", 12, 12, "typo", "reviewer")
	return f
}

func TestGoldenMainView(t *testing.T) {
	f := commented(t)
	golden.RequireEqual(t, view(t, f, seq("ctrl+p", "apipar", "enter", "6G")...))
}

func TestGoldenVisualMode(t *testing.T) {
	f := commented(t)
	golden.RequireEqual(t, view(t, f, seq("ctrl+p", "apipar", "enter", "2G", "V", "2j")...))
}

func TestGoldenQuickOpen(t *testing.T) {
	f := commented(t)
	golden.RequireEqual(t, view(t, f, seq("ctrl+p", "apipar")...))
}

func TestGoldenCommentPicker(t *testing.T) {
	f := commented(t)
	golden.RequireEqual(t, view(t, f, seq("ctrl+l")...))
}

func TestGoldenChanges(t *testing.T) {
	f := changed(t)
	golden.RequireEqual(t, view(t, f, seq("ctrl+p", "apipar", "enter", "6G")...))
}

func TestGoldenChangesPicker(t *testing.T) {
	f := changed(t)
	golden.RequireEqual(t, view(t, f, seq("ctrl+g")...))
}
