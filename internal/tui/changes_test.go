package tui

import (
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"testing"

	"github.com/charmbracelet/x/exp/golden"
	"github.com/gustavofsantos/rvw/internal/gitx"
	"github.com/gustavofsantos/rvw/internal/workspace"
)

func TestSignsMapZeroContextHunksToLines(t *testing.T) {
	const _, A, C, D, T = signNone, signAdded, signChanged, signDeleted, signDeletedTop
	for _, tc := range []struct {
		name  string
		hunks []gitx.Hunk
		want  []sign
	}{
		{"none", nil, nil},
		{"added", []gitx.Hunk{{OldStart: 2, OldLines: 0, NewStart: 3, NewLines: 2}}, []sign{0, 0, A, A, 0}},
		{"changed", []gitx.Hunk{{OldStart: 2, OldLines: 1, NewStart: 2, NewLines: 2}}, []sign{0, C, C, 0, 0}},
		{"deleted", []gitx.Hunk{{OldStart: 3, OldLines: 2, NewStart: 2, NewLines: 0}}, []sign{0, D, 0, 0, 0}},
		{"deleted at top", []gitx.Hunk{{OldStart: 1, OldLines: 1, NewStart: 0, NewLines: 0}}, []sign{T, 0, 0, 0, 0}},
		{"past the end", []gitx.Hunk{{OldStart: 9, OldLines: 0, NewStart: 5, NewLines: 4}}, []sign{0, 0, 0, 0, A}},
	} {
		if got := signs(tc.hunks, 5); !slices.Equal(got, tc.want) {
			t.Errorf("%s: signs = %v, want %v", tc.name, got, tc.want)
		}
	}
}

// gitFixture is the fixture with parse.py committed and then edited: a
// line added, one changed, one deleted mid-file, the first one deleted, and
// an untracked file beside it.
func gitFixture(t *testing.T) *fixture {
	t.Helper()
	f := setup(t)
	write := func(content string) { f.write("src/api/parse.py", content) }
	write("# header\n" + parsePy)
	f.git("commit", "-qam", "header")
	write(`import parse


def handle(req):
    if req.kind == "y":
        body = req.body
        log(body)
        return parse(body)


def default(req):
	return None
`)
	f.write("src/new.py", "a = 1\nb = 2\n")
	return f
}

func TestGutterSignsFollowGitChanges(t *testing.T) {
	f := gitFixture(t)
	m := f.model()
	keys(m, "ctrl+p", "apipar", "enter")
	const _, A, C, D, T = signNone, signAdded, signChanged, signDeleted, signDeletedTop
	want := []sign{T, 0, 0, 0, C, 0, A, D, 0, 0, 0, 0}
	if !slices.Equal(m.file.signs, want) {
		t.Fatalf("parse.py signs = %v, want %v", m.file.signs, want)
	}

	keys(m, "ctrl+p", "srcnew", "enter")
	if !slices.Equal(m.file.signs, []sign{A, A}) {
		t.Fatalf("an untracked file is all added: %v", m.file.signs)
	}

	keys(m, "ctrl+p", "util", "enter")
	if m.file.signs != nil {
		t.Fatalf("an unchanged file has no signs: %v", m.file.signs)
	}

	f.write("src/util.py", "def util():\n    return 1\n")
	keys(m, "r")
	if !slices.Equal(m.file.signs, []sign{0, C}) {
		t.Fatalf("reload recomputes the signs: %v", m.file.signs)
	}
}

func TestHunkJumpsVisitEveryChange(t *testing.T) {
	m := gitFixture(t).model()
	keys(m, "ctrl+p", "apipar", "enter")
	if !slices.Equal(m.file.hunks, []int{1, 5, 7, 8}) {
		t.Fatalf("hunks = %v", m.file.hunks)
	}
	var visited []int
	for range 3 {
		keys(m, "]", "h")
		visited = append(visited, m.file.cursor+1)
	}
	if !slices.Equal(visited, []int{5, 7, 8}) {
		t.Fatalf("]h from line 1 visits %v, want [5 7 8]", visited)
	}
	keys(m, "]", "h")
	if m.file.cursor+1 != 8 || m.flash != "no next change in this file" {
		t.Fatalf("]h past the last change stays on line %d and says %q", m.file.cursor+1, m.flash)
	}
	keys(m, "12G", "[", "h")
	if m.file.cursor+1 != 8 {
		t.Fatalf("[h from line 12 goes to line 8, cursor on %d", m.file.cursor+1)
	}
	keys(m, "3G", "[", "h")
	if m.file.cursor+1 != 1 {
		t.Fatalf("[h reaches a deletion at the top, cursor on %d", m.file.cursor+1)
	}
}

func TestHunkJumpsWithoutChangesSayThereIsNone(t *testing.T) {
	m := setup(t).model()
	keys(m, "ctrl+p", "apipar", "enter", "]", "h")
	if m.file.cursor != 0 || m.flash != "no next change in this file" {
		t.Fatalf("cursor on %d, flash %q", m.file.cursor+1, m.flash)
	}
}

func TestGutterSignsNeedHEAD(t *testing.T) {
	ws, err := workspace.Canonical(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	if out, err := exec.Command("git", "-C", ws, "init", "-q").CombinedOutput(); err != nil {
		t.Fatalf("git init: %v\n%s", err, out)
	}
	if err := os.WriteFile(filepath.Join(ws, "a.txt"), []byte("x\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if out, err := exec.Command("git", "-C", ws, "add", "a.txt").CombinedOutput(); err != nil {
		t.Fatalf("git add: %v\n%s", err, out)
	}
	if got, hunks := fileChanges(ws, filepath.Join(ws, "a.txt"), 1); got != nil || hunks != nil {
		t.Fatalf("without a commit there are no signs: %v %v", got, hunks)
	}
}

func TestGoldenGitChanges(t *testing.T) {
	f := gitFixture(t)
	golden.RequireEqual(t, view(t, f, seq("ctrl+p", "apipar", "enter")...))
}
