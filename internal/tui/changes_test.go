package tui

import (
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	"github.com/charmbracelet/x/ansi"
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
	keys(m, "leader", "p", "apipar", "enter")
	const _, A, C, D, T = signNone, signAdded, signChanged, signDeleted, signDeletedTop
	want := []sign{T, 0, 0, 0, C, 0, A, D, 0, 0, 0, 0}
	if !slices.Equal(m.file.signs, want) {
		t.Fatalf("parse.py signs = %v, want %v", m.file.signs, want)
	}

	keys(m, "leader", "p", "srcnew", "enter")
	if !slices.Equal(m.file.signs, []sign{A, A}) {
		t.Fatalf("an untracked file is all added: %v", m.file.signs)
	}

	keys(m, "leader", "p", "util", "enter")
	if m.file.signs != nil {
		t.Fatalf("an unchanged file has no signs: %v", m.file.signs)
	}

	f.write("src/util.py", "def util():\n    return 1\n")
	keys(m, "r")
	if !slices.Equal(m.file.signs, []sign{0, C}) {
		t.Fatalf("reload recomputes the signs: %v", m.file.signs)
	}
}

func TestHunkJumpsVisitEveryChangeAndGoRound(t *testing.T) {
	m := gitFixture(t).model()
	keys(m, "leader", "p", "apipar", "enter")
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
	if m.file.cursor+1 != 1 || m.flash != "wrapped to the first change" {
		t.Fatalf("]h past the last change goes round to line 1, cursor on %d, flash %q", m.file.cursor+1, m.flash)
	}
	keys(m, "[", "h")
	if m.file.cursor+1 != 8 || m.flash != "wrapped to the last change" {
		t.Fatalf("[h before the first change goes round to line 8, cursor on %d, flash %q", m.file.cursor+1, m.flash)
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

func TestHunkJumpsGoRoundASingleChange(t *testing.T) {
	f := setup(t)
	f.write("src/util.py", "def util():\n    return 1\n")
	m := f.model()
	keys(m, "leader", "p", "srcutil", "enter", "]", "h")
	if m.file.cursor+1 != 2 {
		t.Fatalf("]h goes to the change, cursor on %d", m.file.cursor+1)
	}
	keys(m, "]", "h")
	if m.file.cursor+1 != 2 || m.flash != "wrapped to the first change" {
		t.Fatalf("]h on the only change stays on it, cursor on %d, flash %q", m.file.cursor+1, m.flash)
	}
}

func TestHunkJumpsWithoutChangesSayThereIsNone(t *testing.T) {
	m := setup(t).model()
	keys(m, "leader", "p", "apipar", "enter", "]", "h")
	if m.file.cursor != 0 || m.flash != "no changes in this file" {
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
	if _, _, err := compareHEAD.base(ws); err == nil {
		t.Fatal("without a commit there is no HEAD to compare with")
	}
	if got, hunks := fileChanges(ws, filepath.Join(ws, "a.txt"), "HEAD", 1); got != nil || hunks != nil {
		t.Fatalf("without a commit there are no signs: %v %v", got, hunks)
	}
}

func TestGoldenGitChanges(t *testing.T) {
	f := gitFixture(t)
	golden.RequireEqual(t, view(t, f, seq("leader", "p", "apipar", "enter")...))
}

// ── changes view ─────────────────────────────────────────────────────────────

func TestChangesViewListsUncommittedChangesByDefault(t *testing.T) {
	f := gitFixture(t)
	if err := os.Remove(filepath.Join(f.ws, "README.md")); err != nil {
		t.Fatal(err)
	}
	m := f.model()
	if m.side != sideFiles {
		t.Fatalf("the sidebar starts on the files, got %v", m.side)
	}
	keys(m, "t")
	if m.side != sideChanges || m.cmp != compareHEAD || m.sideTitle() != "changes vs HEAD" {
		t.Fatalf("t shows the uncommitted changes: side %v, compare %v, title %q", m.side, m.cmp, m.sideTitle())
	}
	want := []string{"src/", "  api/", "    parse.py", "  new.py", "README.md"}
	if got := rowNames(m.rows); !slices.Equal(got, want) {
		t.Fatalf("changes rows = %q, want %q", got, want)
	}
	if m.status["src/api/parse.py"] != 'M' || m.status["src/new.py"] != '?' || m.status["README.md"] != 'D' {
		t.Fatalf("status = %q", m.status)
	}

	keys(m, "tab", "G", "l")
	if m.file.rel == "README.md" || m.flash != "README.md was deleted: nothing to show" {
		t.Fatalf("opening a deleted file stays on %s and says %q", m.file.rel, m.flash)
	}
	keys(m, "gg", "j", "j", "l")
	if m.file.rel != "src/api/parse.py" || m.focus != paneViewer {
		t.Fatalf("l on a changed file opens it: %s, focus %v", m.file.rel, m.focus)
	}

	keys(m, "t", "t")
	if m.side != sideFiles || m.sideTitle() != "files" || rowNames(m.rows)[0] != "docs/" {
		t.Fatalf("t goes back to the files: %v %q", m.side, rowNames(m.rows))
	}
}

// branched is the fixture on a feature branch off main: util.py changed in a
// commit on the branch, papers.md in a commit on main after the branch point,
// and parse.py edited but not committed.
func branched(t *testing.T) *fixture {
	t.Helper()
	f := setup(t)
	f.git("checkout", "-qb", "feature")
	f.write("src/util.py", "def util():\n    return 1\n")
	f.git("commit", "-qam", "util")
	f.git("checkout", "-q", "main")
	f.write("docs/api/papers.md", "# Papers\n\nmore\n")
	f.git("commit", "-qam", "papers")
	f.git("checkout", "-q", "feature")
	f.write("src/api/parse.py", "# edited\n"+parsePy)
	return f
}

func TestChangesAgainstTheDefaultBranchStartAtTheMergeBase(t *testing.T) {
	m := branched(t).model()
	keys(m, "leader", "p", "srcutil", "enter")
	if m.file.signs != nil {
		t.Fatalf("against HEAD the committed util.py is unchanged: %v", m.file.signs)
	}

	keys(m, "b", "2")
	if m.compare != nil || m.cmp != compareDefault || m.sideTitle() != "changes vs main" {
		t.Fatalf("b 2 compares with main: menu %v, compare %v, title %q", m.compare, m.cmp, m.sideTitle())
	}
	want := []string{"src/", "  api/", "    parse.py", "  util.py"}
	if got := rowNames(m.rows); !slices.Equal(got, want) {
		t.Fatalf("changes vs main = %q, want %q (not main's own papers.md)", got, want)
	}
	if !slices.Equal(m.file.signs, []sign{0, signChanged}) {
		t.Fatalf("the gutter marks against main too: %v", m.file.signs)
	}

	keys(m, "b", "1")
	if got := rowNames(m.rows); !slices.Equal(got, []string{"src/", "  api/", "    parse.py"}) {
		t.Fatalf("b 1 goes back to the uncommitted changes: %q", got)
	}
	if m.file.signs != nil {
		t.Fatalf("and the gutter with it: %v", m.file.signs)
	}
}

func TestChangesAgainstThePreviousCommitIncludeTheLastCommit(t *testing.T) {
	m := branched(t).model()
	keys(m, "b", "down", "down", "enter")
	if m.cmp != comparePrev || m.sideTitle() != "changes vs HEAD~1" {
		t.Fatalf("compare %v, title %q", m.cmp, m.sideTitle())
	}
	if got := rowNames(m.rows); !slices.Equal(got, []string{"src/", "  api/", "    parse.py", "  util.py"}) {
		t.Fatalf("changes vs HEAD~1 = %q", got)
	}
}

func TestACompareThatCannotResolveKeepsThePreviousOne(t *testing.T) {
	m := setup(t).model()
	keys(m, "b", "3")
	if m.cmp != compareHEAD || m.side != sideFiles || m.flash != "rvw: no previous commit to compare with" {
		t.Fatalf("compare %v, side %v, flash %q", m.cmp, m.side, m.flash)
	}
	m.flash = ""
	keys(m, "b", "2")
	if m.cmp != compareDefault {
		t.Fatalf("on main itself the default branch resolves: flash %q", m.flash)
	}
	if got := rowNames(m.rows); len(got) != 0 {
		t.Fatalf("main against itself has no changes: %q", got)
	}
}

func TestChangesViewWithoutACommitSaysWhy(t *testing.T) {
	f := setup(t)
	f.git("update-ref", "-d", "HEAD")
	m := f.model()
	keys(m, "t")
	if len(m.rows) != 0 || m.chgErr != "no commit to compare with yet" {
		t.Fatalf("rows %q, error %q", rowNames(m.rows), m.chgErr)
	}
	if !strings.Contains(ansi.Strip(m.screen()), " no commit to compare") {
		t.Fatal("the pane shows why there are no changes")
	}
}

func TestGoldenChangesView(t *testing.T) {
	f := gitFixture(t)
	golden.RequireEqual(t, view(t, f, seq("t", "tab", "b")...))
}

func TestACommitMadeMeanwhileMovesTheComparison(t *testing.T) {
	f := setup(t)
	m := f.model()
	f.write("src/util.py", "def util():\n    return 1\n")
	f.git("commit", "-qam", "util")
	keys(m, "leader", "p", "srcutil", "enter")
	if m.file.signs != nil {
		t.Fatalf("the gutter compares with HEAD as it is now: %v", m.file.signs)
	}
	keys(m, "t")
	if len(m.rows) != 0 {
		t.Fatalf("so does the changes view: %q", rowNames(m.rows))
	}
}

func TestChangesViewFindsTheFirstCommitMadeMeanwhile(t *testing.T) {
	f := setup(t)
	f.git("update-ref", "-d", "HEAD")
	m := f.model()
	f.git("commit", "-qm", "first")
	f.write("README.md", "# Demo\n")
	keys(m, "t")
	if m.chgErr != "" || !slices.Equal(rowNames(m.rows), []string{"README.md"}) {
		t.Fatalf("rows %q, error %q", rowNames(m.rows), m.chgErr)
	}
}

func TestHelpFitsTheTestTerminal(t *testing.T) {
	m := setup(t).model()
	keys(m, "?")
	if !strings.Contains(ansi.Strip(m.screen()), "move, open, close") {
		t.Fatal("the last help line shows at 100x30")
	}
}

// ── status line ──────────────────────────────────────────────────────────────

// status is the status line at width w, colors stripped, with the gap before
// "? help" squeezed to one space; it fails unless the line is w wide.
func status(t *testing.T, m *model, w int) string {
	t.Helper()
	line := ansi.Strip(render(m.statusLine(w), nil))
	if got := ansi.StringWidth(line); got != w {
		t.Fatalf("status %q is %d wide, want %d", line, got, w)
	}
	left, found := strings.CutSuffix(line, "? help")
	if !found {
		t.Fatalf("status %q does not end with ? help", line)
	}
	return strings.TrimRight(left, " ") + " ? help"
}

func TestStatusLineShowsTheBranchAndUncommittedChanges(t *testing.T) {
	f := gitFixture(t)
	if err := os.Remove(filepath.Join(f.ws, "README.md")); err != nil {
		t.Fatal(err)
	}
	m := f.model()
	if got := status(t, m, 60); got != "⎇ main · uncommitted +4 -6 ? help" {
		t.Fatalf("status = %q", got)
	}

	f.git("add", "-A")
	f.git("commit", "-qm", "all")
	keys(m, "r")
	if got := status(t, m, 30); got != "⎇ main · clean ? help" {
		t.Fatalf("r re-reads the status: %q", got)
	}

	f.git("checkout", "-qb", "a-very-long-feature-branch-name")
	keys(m, "r")
	if got := status(t, m, 30); got != "⎇ a-very-long-… · clean ? help" {
		t.Fatalf("a long branch is cut, not the rest: %q", got)
	}

	f.git("checkout", "-q", "--detach")
	keys(m, "r")
	if got := status(t, m, 40); !strings.HasPrefix(got, "⎇ detached ") {
		t.Fatalf("a detached HEAD says so: %q", got)
	}
}

func TestUntrackedFilesCountTheirTextLines(t *testing.T) {
	f := setup(t)
	f.write("a.txt", "one\ntwo\nthree") // no newline at the end
	f.write("b.bin", "\x00\x01\x02\n")
	f.write("empty.txt", "")
	m := f.model()
	if m.wd != (wdChanges{added: 3, dirty: true}) {
		t.Fatalf("wd = %+v", m.wd)
	}
	if got := status(t, m, 60); got != "⎇ main · uncommitted +3 -0 ? help" {
		t.Fatalf("status = %q", got)
	}
}

func TestStatusLineWithoutACommit(t *testing.T) {
	f := setup(t)
	f.git("update-ref", "-d", "HEAD")
	m := f.model()
	if got := status(t, m, 40); got != "⎇ main · no commits yet ? help" {
		t.Fatalf("status = %q", got)
	}
}

func TestCommentsOnTheCursorLineTakeTheBarOverTheStatus(t *testing.T) {
	f := commented(t)
	m := f.model()
	keys(m, "leader", "p", "apipar", "enter", "5G")
	if bar := ansi.Strip(strings.Join(m.bar(), "\n")); !strings.HasPrefix(bar, "r1 · 5-8") {
		t.Fatalf("bar = %q", bar)
	}
	keys(m, "tab")
	if bar := ansi.Strip(strings.Join(m.bar(), "\n")); !strings.HasPrefix(bar, "⎇ main") {
		t.Fatalf("with the tree focused the status shows: %q", bar)
	}
}

func TestOpeningFromTheChangesViewLandsOnTheFirstChange(t *testing.T) {
	f := setup(t)
	f.write("src/util.py", "def util():\n    return 1\n")
	m := f.model()
	keys(m, "leader", "p", "srcutil", "enter")
	if m.file.cursor != 0 {
		t.Fatalf("the finder opens util.py on line 1, cursor on %d", m.file.cursor+1)
	}

	keys(m, "t", "tab")
	if got := rowNames(m.rows); !slices.Equal(got, []string{"src/", "  util.py"}) {
		t.Fatalf("rows = %q", got)
	}
	keys(m, "gg", "j", "l")
	if m.file.rel != "src/util.py" || m.file.cursor+1 != 2 || m.focus != paneViewer {
		t.Fatalf("l opens util.py on its change: %s line %d", m.file.rel, m.file.cursor+1)
	}

	keys(m, "gg")
	m.Update(click(3, 2))
	if m.file.cursor+1 != 2 {
		t.Fatalf("a click opens it on its change again, cursor on %d", m.file.cursor+1)
	}

	keys(m, "gg", "t", "t", "tab", "l") // tab puts the tree cursor on the open file
	if m.side != sideFiles {
		t.Fatalf("t t goes round to the files, side %v", m.side)
	}
	if m.file.rel != "src/util.py" || m.file.cursor != 0 {
		t.Fatalf("the file tree opens it where it was left: %s line %d", m.file.rel, m.file.cursor+1)
	}
}
