package tui

import (
	"regexp"
	"testing"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/x/ansi"
	"github.com/charmbracelet/x/exp/golden"
	"github.com/charmbracelet/x/exp/teatest/v2"
)

// Golden views at 100x30, colors stripped and dates masked: they pin the
// layout. Regenerate with `go test ./internal/tui -update`.

var dates = regexp.MustCompile(`\d{4}-\d{2}-\d{2}`)

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
	final, ok := tm.FinalModel(t, teatest.WithFinalTimeout(5*time.Second)).(*model)
	if !ok {
		t.Fatal("final model is not a *model")
	}
	return dates.ReplaceAll([]byte(ansi.Strip(final.screen())), []byte("YYYY-MM-DD"))
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
	t.Helper()
	f := setup(t)
	f.add("src/api/parse.py", 5, 8, "extract this branch into parse_x()", "gustavo")
	f.add("src/api/parse.py", 12, 12, "typo", "reviewer")
	return f
}

func TestGoldenMainView(t *testing.T) {
	f := commented(t)
	golden.RequireEqual(t, view(t, f, seq("leader", "p", "apipar", "enter", "6G")...))
}

func TestGoldenVisualMode(t *testing.T) {
	f := commented(t)
	golden.RequireEqual(t, view(t, f, seq("leader", "p", "apipar", "enter", "2G", "V", "2j")...))
}

func TestGoldenQuickOpen(t *testing.T) {
	f := commented(t)
	golden.RequireEqual(t, view(t, f, seq("leader", "p", "apipar")...))
}

func TestGoldenCommentPicker(t *testing.T) {
	f := commented(t)
	golden.RequireEqual(t, view(t, f, seq("leader", "l")...))
}
