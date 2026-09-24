package tui

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	tea "charm.land/bubbletea/v2"
	"github.com/alecthomas/chroma/v2"
	"github.com/charmbracelet/x/ansi"
	"github.com/gustavofsantos/rvw/internal/gitx"
	"github.com/gustavofsantos/rvw/internal/review"
	"github.com/gustavofsantos/rvw/internal/store"
	"github.com/gustavofsantos/rvw/internal/workspace"
)

// ── fixtures ─────────────────────────────────────────────────────────────────

const parsePy = `import parse


def handle(req):
    if req.kind == "x":
        body = req.body
        return parse(body)
    return default(req)


def default(req):
	return None
`

// fixture is a plain directory, no git, with a store beside it.
type fixture struct {
	t   *testing.T
	ctx context.Context
	svc *review.Service
	ws  string
}

func setup(t *testing.T) *fixture {
	t.Helper()
	root := t.TempDir()
	ws, err := workspace.Canonical(filepath.Join(root, "proj"))
	if err != nil {
		t.Fatal(err)
	}
	f := &fixture{t: t, ctx: context.Background(), ws: ws}
	f.write("src/api/parse.py", parsePy)
	f.write("src/util.py", "def util():\n    pass\n")
	f.write("test/api/api_parser_test.go", "package api\n")
	f.write("docs/api/papers.md", "# Papers\n")
	f.write("README.md", "# Demo\n\nhello\n")
	f.write("node_modules/dep/index.js", "skipped\n")
	f.write(".git/HEAD", "skipped\n")
	st, err := store.Open(f.ctx, filepath.Join(root, "rvw.db"))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { st.Close() })
	f.svc = review.NewService(st)
	return f
}

func (f *fixture) write(rel, content string) {
	f.t.Helper()
	path := filepath.Join(f.ws, rel)
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		f.t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(content), 0o644); err != nil {
		f.t.Fatal(err)
	}
}

func (f *fixture) add(file string, start, end int, text, author string) review.Comment {
	f.t.Helper()
	c, err := f.svc.Add(f.ctx, review.AddInput{
		Workspace: f.ws, File: file, StartLine: start, EndLine: end, Comment: text, Author: author,
	})
	if err != nil {
		f.t.Fatal(err)
	}
	return c
}

func (f *fixture) model() *model {
	f.t.Helper()
	m, err := newModel(f.ctx, Options{Service: f.svc, Workspace: f.ws, Author: "tester", Editor: "true"})
	if err != nil {
		f.t.Fatal(err)
	}
	m.Update(tea.WindowSizeMsg{Width: 100, Height: 30})
	return m
}

// press is a key press: a named key, else one printable character.
func press(k string) tea.KeyPressMsg {
	switch k {
	case "ctrl+p":
		return tea.KeyPressMsg{Code: 'p', Mod: tea.ModCtrl}
	case "ctrl+l":
		return tea.KeyPressMsg{Code: 'l', Mod: tea.ModCtrl}
	case "ctrl+g":
		return tea.KeyPressMsg{Code: 'g', Mod: tea.ModCtrl}
	case "enter":
		return tea.KeyPressMsg{Code: tea.KeyEnter}
	case "esc":
		return tea.KeyPressMsg{Code: tea.KeyEscape}
	case "tab":
		return tea.KeyPressMsg{Code: tea.KeyTab}
	case "down":
		return tea.KeyPressMsg{Code: tea.KeyDown}
	}
	r := []rune(k)[0]
	return tea.KeyPressMsg{Code: r, Text: k}
}

// keys presses named keys, and types any other string one character at a time.
func keys(m *model, ks ...string) {
	for _, k := range ks {
		if press(k).Text == "" {
			m.Update(press(k))
			continue
		}
		for _, r := range k {
			m.Update(press(string(r)))
		}
	}
}

// ── tree ─────────────────────────────────────────────────────────────────────

func TestWalkSkipsVCSAndDependencyDirectories(t *testing.T) {
	f := setup(t)
	files, err := walkFiles(f.ws)
	if err != nil {
		t.Fatal(err)
	}
	want := []string{"README.md", "docs/api/papers.md", "src/api/parse.py", "src/util.py", "test/api/api_parser_test.go"}
	if !slices.Equal(files, want) {
		t.Fatalf("files = %q, want %q", files, want)
	}
}

func TestListFilesObeysGitignore(t *testing.T) {
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("needs git")
	}
	ws := t.TempDir()
	write := func(rel, content string) {
		path := filepath.Join(ws, rel)
		if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, []byte(content), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	git := func(args ...string) {
		cmd := exec.Command("git", append([]string{"-C", ws}, args...)...)
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v\n%s", args, err, out)
		}
	}
	git("init", "-q")
	write(".gitignore", "*.log\nout/\n")
	write("src/.gitignore", "gen.go\n")
	write("main.go", "package main\n")
	write("gone.go", "package main\n")
	write("vendor/dep/dep.go", "package dep\n")
	write("debug.log", "ignored\n")
	write("out/bin", "ignored\n")
	write("src/gen.go", "ignored\n")
	write("src/lib.go", "package src\n")
	git("add", ".")
	if err := os.Remove(filepath.Join(ws, "gone.go")); err != nil {
		t.Fatal(err)
	}
	write("new.txt", "untracked\n")
	write("forced.log", "tracked though ignored\n")
	git("add", "-f", "forced.log")

	files, err := listFiles(ws)
	if err != nil {
		t.Fatal(err)
	}
	want := []string{".gitignore", "forced.log", "main.go", "new.txt", "src/.gitignore", "src/lib.go", "vendor/dep/dep.go"}
	if !slices.Equal(files, want) {
		t.Fatalf("files = %q, want %q", files, want)
	}
}

func TestTreeSortsDirectoriesFirstAndFlattensVisibleRows(t *testing.T) {
	root := buildTree([]string{"b.txt", "a/z.go", "a/b/c.go", "A.md", "c/d.go"})
	if got := rowNames(root.rows()); !slices.Equal(got, []string{"a/", "c/", "A.md", "b.txt"}) {
		t.Fatalf("collapsed rows = %q", got)
	}

	root.reveal("a/b/c.go")
	if got := rowNames(root.rows()); !slices.Equal(got, []string{"a/", "  b/", "    c.go", "  z.go", "c/", "A.md", "b.txt"}) {
		t.Fatalf("revealed rows = %q", got)
	}
	if got := root.expandedDirs(); !slices.Equal(got, []string{"a", "a/b"}) {
		t.Fatalf("expanded = %q", got)
	}
	if root.firstFile() != "A.md" {
		t.Fatalf("first file = %q", root.firstFile())
	}
	if buildTree([]string{"x/y.go"}).firstFile() != "x/y.go" {
		t.Fatal("first file falls back to tree order")
	}
}

func rowNames(rows []row) []string {
	var out []string
	for _, r := range rows {
		name := strings.Repeat("  ", r.depth) + r.node.name
		if r.node.dir {
			name += "/"
		}
		out = append(out, name)
	}
	return out
}

// ── rail ─────────────────────────────────────────────────────────────────────

func TestRailMarksEveryLineOfARange(t *testing.T) {
	cs := []review.Comment{
		{ID: "r1", StartLine: 2, EndLine: 5},
		{ID: "r2", StartLine: 7, EndLine: 7},
		{ID: "r3", StartLine: 4, EndLine: 4}, // overlaps r1, newer
	}
	want := []string{" ", "╭", "│", "●", "╰", " ", "●", " "}
	for line := 1; line <= len(want); line++ {
		_, g, ok := railAt(cs, line)
		if !ok {
			g = " "
		}
		if g != want[line-1] {
			t.Errorf("line %d: glyph %q, want %q", line, g, want[line-1])
		}
	}
	if c, _, _ := railAt(cs, 4); c.ID != "r3" {
		t.Errorf("overlap draws %s, want the most recent r3", c.ID)
	}
	if got := covering(cs, 4); len(got) != 2 || got[0].ID != "r1" || got[1].ID != "r3" {
		t.Errorf("covering(4) = %v, want r1 and r3", got)
	}
	if l, ok := nextComment(cs, 2, 1); !ok || l != 4 {
		t.Errorf("next after 2 = %d %v, want 4", l, ok)
	}
	if l, ok := nextComment(cs, 4, -1); !ok || l != 2 {
		t.Errorf("previous before 4 = %d %v, want 2", l, ok)
	}
	if _, ok := nextComment(cs, 7, 1); ok {
		t.Error("nothing after the last comment")
	}
}

// ── fuzzy ────────────────────────────────────────────────────────────────────

func labels(query string, paths []string, comments map[string]int) []string {
	cands := make([]candidate, len(paths))
	for i, p := range paths {
		cands[i] = candidate{label: p, comments: comments[p]}
	}
	var out []string
	for _, r := range rank(query, cands) {
		out = append(out, paths[r.index])
	}
	return out
}

func TestRankPrefersSegmentStartsAndTheFileName(t *testing.T) {
	paths := []string{
		"test/api/api_parser_test", "docs/api/papers.md", "src/api/api.py",
		"src/handlers/v2/internal/api_parser.go", "src/api/parse.py",
	}
	got := labels("apipar", paths, nil)
	if len(got) == 0 || got[0] != "src/api/parse.py" {
		t.Fatalf("apipar ranks %q", got)
	}
	if slices.Contains(got, "src/api/api.py") {
		t.Fatalf("api.py has no 'par' after 'api': %q", got)
	}

	got = labels("util", []string{"util/src/main.go", "src/util.go"}, nil)
	if got[0] != "src/util.go" {
		t.Fatalf("a match in the file name wins: %q", got)
	}
}

func TestRankBreaksTiesByCommentsThenLength(t *testing.T) {
	got := labels("x", []string{"b/x.go", "a/x.go", "c/x.gox"}, map[string]int{"c/x.gox": 1})
	if !slices.Equal(got, []string{"c/x.gox", "b/x.go", "a/x.go"}) {
		t.Fatalf("ties = %q", got)
	}
	got = labels("x", []string{"aa/x.go", "a/x.go"}, nil)
	if got[0] != "a/x.go" {
		t.Fatalf("shorter first: %q", got)
	}
	if got := labels("", []string{"b", "a"}, nil); !slices.Equal(got, []string{"b", "a"}) {
		t.Fatalf("an empty query keeps the input order: %q", got)
	}
}

func TestFuzzyMatchReportsMatchedRunes(t *testing.T) {
	_, pos, ok := fuzzyMatch("sPar", "src/api/parse.py")
	if !ok || !slices.Equal(pos, []int{0, 8, 9, 10}) {
		t.Fatalf("pos = %v %v", pos, ok)
	}
	if _, _, ok := fuzzyMatch("zz", "src/api.py"); ok {
		t.Fatal("no match")
	}
}

func TestTruncateLeftKeepsTheFileName(t *testing.T) {
	cases := []struct {
		in   string
		w    int
		want string
		cut  int
	}{
		{"src/api.py", 20, "src/api.py", 0},
		{"src/handlers/v2/internal/api_parser.go", 30, "…/v2/internal/api_parser.go", 12},
		{"a/very_long_file_name.go", 10, "…e_name.go", 15},
	}
	for _, c := range cases {
		got, cut := truncateLeft(c.in, c.w)
		if got != c.want || cut != c.cut {
			t.Errorf("truncateLeft(%q, %d) = %q %d, want %q %d", c.in, c.w, got, cut, c.want, c.cut)
		}
	}
}

// ── highlighting ─────────────────────────────────────────────────────────────

func TestSplitLinesBreaksTokensAtNewlines(t *testing.T) {
	lines := splitLines([]chroma.Token{
		{Type: chroma.Keyword, Value: "def"},
		{Type: chroma.LiteralString, Value: " \"a\nb\"\r\n"},
		{Type: chroma.Text, Value: "\tx\x1b"},
	})
	want := [][]segment{
		{{"def", chroma.Keyword}, {` "a`, chroma.LiteralString}},
		{{`b"`, chroma.LiteralString}},
		{{"    x\uFFFD", chroma.Text}},
	}
	if len(lines) != len(want) {
		t.Fatalf("lines = %v", lines)
	}
	for i := range want {
		if !slices.Equal(lines[i], want[i]) {
			t.Errorf("line %d = %v, want %v", i+1, lines[i], want[i])
		}
	}
	if got, _ := displayText("a\tb", 0); got != "a   b" {
		t.Errorf("tab stop: %q", got)
	}
	if got, _ := displayText("日\tb", 0); got != "日  b" {
		t.Errorf("wide tab stop: %q", got)
	}
}

func TestHighlightKeepsOneEntryPerFileLine(t *testing.T) {
	for text, n := range map[string]int{"": 0, "a": 1, "a\n": 1, "a\nb": 2, "a\n\n": 2, "x = 1\ny = 2\n": 2} {
		if got := len(highlight("f.py", text)); got != n {
			t.Errorf("highlight(%q) has %d lines, want %d", text, got, n)
		}
	}
	lines := highlight("f.py", "def f():\n    return 1\n")
	if lines[0][0].kind != chroma.Keyword {
		t.Errorf("python is highlighted: %v", lines[0])
	}
	if !isBinary([]byte("ab\x00c")) || isBinary([]byte("abc")) {
		t.Error("a NUL byte makes a file binary")
	}
}

// ── editor ───────────────────────────────────────────────────────────────────

func TestStripTemplate(t *testing.T) {
	tmpl := template("", []string{"New comment on a.py:1", "", "  1  x = 1"})
	if got := stripTemplate(tmpl); got != "" {
		t.Fatalf("an untouched template is empty: %q", got)
	}
	if got := stripTemplate("# Title\n\nbody\n" + tmpl); got != "# Title\n\nbody" {
		t.Fatalf("text above the scissors is kept, headings too: %q", got)
	}
	if got := stripTemplate("note\n# context\nmore\n"); got != "note\nmore" {
		t.Fatalf("without scissors, # lines go: %q", got)
	}
	if got := template("old text", nil); !strings.HasPrefix(got, "old text\n\n"+scissors) {
		t.Fatalf("an edit starts with the old text: %q", got)
	}
}

func TestEditorAddsEditsAndSubmits(t *testing.T) {
	f := setup(t)
	m := f.model()
	keys(m, "ctrl+p", "apipar", "enter", "4G", "V", "3j")
	if lo, hi := m.selection(); lo != 4 || hi != 7 || m.file.rel != "src/api/parse.py" {
		t.Fatalf("selected %s:%d-%d", m.file.rel, lo, hi)
	}

	done := func(req editRequest, text string) {
		t.Helper()
		path := filepath.Join(t.TempDir(), "note.md")
		if err := os.WriteFile(path, []byte(text+"\n"+template("", nil)), 0o644); err != nil {
			t.Fatal(err)
		}
		m.Update(editorDoneMsg{req: req, path: path})
		if _, err := os.Stat(path); !os.IsNotExist(err) {
			t.Error("the temp file is removed")
		}
	}
	req := editRequest{kind: editAdd, file: m.file.abs, start: 4, end: 7}

	done(req, "")
	if m.flash != "cancelled: empty note" || len(m.file.comments) != 0 || !m.visual {
		t.Fatalf("an empty note cancels: %q %v", m.flash, m.file.comments)
	}

	done(req, "extract this branch")
	if m.flash != "queued r1 · src/api/parse.py:4-7" || m.visual {
		t.Fatalf("flash %q, visual %v", m.flash, m.visual)
	}
	if len(m.file.comments) != 1 || m.counts["src/api/parse.py"] != 1 {
		t.Fatalf("the gutter and counts reload: %v %v", m.file.comments, m.counts)
	}
	c := m.file.comments[0]
	if c.StartLine != 4 || c.EndLine != 7 || deref(c.Author) != "tester" || c.Comment != "extract this branch" {
		t.Fatalf("comment = %+v", c)
	}

	done(editRequest{kind: editEdit, id: "r1"}, "extract into parse_x()")
	if m.flash != "edited r1 · src/api/parse.py:4-7" || m.file.comments[0].Comment != "extract into parse_x()" {
		t.Fatalf("edit: %q %q", m.flash, m.file.comments[0].Comment)
	}

	done(editRequest{kind: editEdit, id: "r9"}, "nope")
	if !m.flashErr || !strings.HasPrefix(m.flash, "rvw: ") {
		t.Fatalf("a service error is shown as rvw: ...: %q", m.flash)
	}

	done(editRequest{kind: editSummary, decision: review.DecisionRequestChanges}, "fix it")
	if m.flash != "submitted rv1 · request-changes · 1 comment" {
		t.Fatalf("submit: %q", m.flash)
	}
	sheet, err := f.svc.Sheet(f.ctx, review.GetInput{Workspace: f.ws, ID: "rv1"})
	if err != nil || !slices.Equal(sheet.Review.CommentIDs, []string{"r1"}) || sheet.Review.Decision != review.DecisionRequestChanges {
		t.Fatalf("sheet = %+v %v", sheet, err)
	}
}

func TestReloadPicksUpCommentsAddedElsewhere(t *testing.T) {
	f := setup(t)
	m := f.model()
	keys(m, "ctrl+p", "parse.py", "enter")
	f.add("src/api/parse.py", 10, 10, "from another shell", "someone")
	if len(m.file.comments) != 0 {
		t.Fatal("nothing is watched")
	}
	keys(m, "r")
	if len(m.file.comments) != 1 || m.counts["src/api/parse.py"] != 1 {
		t.Fatalf("r reloads: %v", m.file.comments)
	}
	keys(m, "]", "c")
	if m.file.cursor != 9 {
		t.Fatalf("]c goes to line 10, cursor on %d", m.file.cursor+1)
	}
}

func TestOpeningAFileRemembersWhereItWasLeft(t *testing.T) {
	f := setup(t)
	m := f.model()
	keys(m, "ctrl+p", "parse.py", "enter", "9G", "ctrl+p", "util", "enter")
	if m.file.rel != "src/util.py" || m.file.cursor != 0 {
		t.Fatalf("a new file opens on line 1: %s:%d", m.file.rel, m.file.cursor+1)
	}
	keys(m, "ctrl+p")
	if got := m.picker.cands[m.picker.matches[0].index].label; got != "src/util.py" {
		t.Fatalf("an empty query lists recent files first: %q", got)
	}
	keys(m, "down", "enter")
	if m.file.rel != "src/api/parse.py" || m.file.cursor != 8 {
		t.Fatalf("reopened at %s:%d, want line 9", m.file.rel, m.file.cursor+1)
	}
}

func TestBinaryFilesCannotBeCommentedOn(t *testing.T) {
	f := setup(t)
	f.write("blob.bin", "a\x00b")
	m := f.model()
	keys(m, "ctrl+p", "blob", "enter", "c")
	if !m.file.binary || !m.flashErr {
		t.Fatalf("binary %v, flash %q", m.file.binary, m.flash)
	}
	if !strings.Contains(m.screen(), "binary file") {
		t.Fatal("a placeholder is shown")
	}
}

// ── mouse ────────────────────────────────────────────────────────────────────

// At 100x30 the tree's rows start at (1, 1) and the viewer's at (27, 1).
const viewerX = 40

func click(x, y int) tea.MouseClickMsg { return tea.MouseClickMsg{X: x, Y: y, Button: tea.MouseLeft} }

func TestClickingTheTreeTogglesDirectoriesAndOpensFiles(t *testing.T) {
	f := setup(t)
	m := f.model()
	if got := rowNames(m.rows); !slices.Equal(got, []string{"docs/", "src/", "test/", "README.md"}) {
		t.Fatalf("rows = %q", got)
	}
	m.Update(click(3, 2))
	if got := rowNames(m.rows); !slices.Equal(got, []string{"docs/", "src/", "  api/", "  util.py", "test/", "README.md"}) {
		t.Fatalf("a click expands src/: %q", got)
	}
	if m.focus != paneTree || m.treeCur != 1 {
		t.Fatalf("focus %v, tree cursor %d", m.focus, m.treeCur)
	}
	m.Update(click(3, 4))
	if m.file.rel != "src/util.py" || m.focus != paneViewer {
		t.Fatalf("a click opens util.py: %s, focus %v", m.file.rel, m.focus)
	}
	m.Update(click(3, 2))
	if got := rowNames(m.rows); !slices.Equal(got, []string{"docs/", "src/", "test/", "README.md"}) {
		t.Fatalf("a second click collapses src/: %q", got)
	}
	m.Update(click(3, 20))
	if m.file.rel != "src/util.py" || m.focus != paneTree {
		t.Fatal("a click below the rows only focuses the tree")
	}
}

func TestDraggingInTheViewerSelectsLines(t *testing.T) {
	f := setup(t)
	m := f.model()
	keys(m, "ctrl+p", "parse.py", "enter")
	m.Update(click(viewerX, 3))
	m.Update(tea.MouseReleaseMsg{X: viewerX, Y: 3, Button: tea.MouseLeft})
	if m.file.cursor != 2 || m.visual {
		t.Fatalf("a click moves the cursor to line 3: line %d, visual %v", m.file.cursor+1, m.visual)
	}

	m.Update(click(viewerX, 8))
	m.Update(tea.MouseMotionMsg{X: viewerX, Y: 5, Button: tea.MouseLeft})
	m.Update(tea.MouseReleaseMsg{X: viewerX, Y: 5, Button: tea.MouseLeft})
	if lo, hi := m.selection(); !m.visual || lo != 5 || hi != 8 {
		t.Fatalf("dragging up from line 8 to 5 selects 5-8: %d-%d, visual %v", lo, hi, m.visual)
	}
	m.Update(tea.MouseMotionMsg{X: viewerX, Y: 10, Button: tea.MouseLeft})
	if lo, hi := m.selection(); lo != 5 || hi != 8 {
		t.Fatalf("motion after the release changes nothing: %d-%d", lo, hi)
	}
	if !strings.Contains(m.screen(), "5-8 (4 lines)") {
		t.Fatal("the bar shows the selection")
	}

	m.Update(click(viewerX, 2))
	m.Update(tea.MouseMotionMsg{X: viewerX, Y: 29, Button: tea.MouseLeft})
	if !m.visual || m.file.cursor != m.file.len()-1 {
		t.Fatalf("dragging past the bottom stops at the last line: line %d", m.file.cursor+1)
	}
}

func TestWheelScrollsThePaneUnderThePointer(t *testing.T) {
	f := setup(t)
	var long strings.Builder
	for range 100 {
		long.WriteString("x\n")
	}
	f.write("long.txt", long.String())
	m := f.model()
	keys(m, "ctrl+p", "long.txt", "enter")
	m.Update(tea.MouseWheelMsg{X: viewerX, Y: 5, Button: tea.MouseWheelDown})
	m.Update(tea.MouseWheelMsg{X: viewerX, Y: 5, Button: tea.MouseWheelDown})
	if m.file.offset != 6 || m.file.cursor != 6 {
		t.Fatalf("two notches scroll 6 lines, carrying the cursor: offset %d, cursor %d", m.file.offset, m.file.cursor)
	}
	m.Update(tea.MouseWheelMsg{X: viewerX, Y: 5, Button: tea.MouseWheelUp})
	if m.file.offset != 3 || m.file.cursor != 6 {
		t.Fatalf("scrolling back leaves the cursor: offset %d, cursor %d", m.file.offset, m.file.cursor)
	}
}

// ── changes ──────────────────────────────────────────────────────────────────

// changed is the fixture as a git worktree with uncommitted changes: in
// src/api/parse.py line 2 is added, line 6 changed and the lines after 9
// deleted; src/new.py is untracked; line 2 of src/util.py is changed.
func changed(t *testing.T) *fixture {
	t.Helper()
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("needs git")
	}
	f := setup(t)
	git := func(args ...string) {
		cmd := exec.Command("git", append([]string{"-C", f.ws, "-c", "user.name=t", "-c", "user.email=t@t"}, args...)...)
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v\n%s", args, err, out)
		}
	}
	if err := os.RemoveAll(filepath.Join(f.ws, ".git")); err != nil {
		t.Fatal(err)
	}
	git("init", "-q")
	f.write(".gitignore", "node_modules/\n")
	git("add", ".")
	git("commit", "-q", "-m", "init")
	f.write("src/api/parse.py", `import parse
import json


def handle(req):
    if req.kind == "y":
        body = req.body
        return parse(body)
    return default(req)
`)
	f.write("src/new.py", "a\nb\n")
	f.write("src/util.py", "def util():\n    return 1\n")
	return f
}

func TestGutterSignsMarkUncommittedChanges(t *testing.T) {
	f := changed(t)
	m := f.model()
	keys(m, "ctrl+p", "apipar", "enter")
	var signs []string
	for line := 1; line <= m.file.len(); line++ {
		s, _ := signAt(m.file.hunks, line)
		signs = append(signs, s)
	}
	if want := []string{" ", "+", " ", " ", " ", "~", " ", " ", "_"}; !slices.Equal(signs, want) {
		t.Fatalf("signs = %q, want %q", signs, want)
	}
	if s, _ := signAt([]gitx.Hunk{{OldStart: 1, OldCount: 2}}, 1); s != signDeletedTop {
		t.Fatalf("a deletion at the top marks line 1 with %q", s)
	}
	keys(m, "6G")
	if !strings.Contains(ansi.Strip(m.screen()), "change 2/3 · 6 · +1 -1") {
		t.Fatalf("the bar describes the change under the cursor:\n%s", m.screen())
	}
}

func TestJumpingBetweenChangesCrossesFiles(t *testing.T) {
	f := changed(t)
	m := f.model()
	keys(m, "ctrl+p", "apipar", "enter")
	at := func(want string) {
		t.Helper()
		if got := location(m.file.rel, m.file.cursor+1, m.file.cursor+1); got != want {
			t.Fatalf("at %s, want %s", got, want)
		}
	}
	for _, want := range []string{"src/api/parse.py:2", "src/api/parse.py:6", "src/api/parse.py:9", "src/new.py:1", "src/util.py:2"} {
		keys(m, "]", "h")
		at(want)
	}
	keys(m, "]", "h")
	if at("src/util.py:2"); m.flash != "no next change" {
		t.Fatalf("flash = %q", m.flash)
	}
	keys(m, "[", "h")
	at("src/new.py:1")
	keys(m, "[", "h")
	at("src/api/parse.py:9")
}

func TestChangesPickerListsEveryHunk(t *testing.T) {
	f := changed(t)
	m := f.model()
	keys(m, "ctrl+g")
	if m.picker == nil || m.picker.title() != "Uncommitted changes" {
		t.Fatal("C-g opens the changes picker")
	}
	var got []string
	for _, c := range m.picker.cands {
		got = append(got, strings.Join(strings.Fields(c.label), " "))
	}
	want := []string{
		"src/api/parse.py:2 +1 -0", "src/api/parse.py:6 +1 -1", "src/api/parse.py:9 +0 -4",
		"src/new.py:1-2 +2 -0", "src/util.py:2 +1 -1",
	}
	if !slices.Equal(got, want) {
		t.Fatalf("changes = %q, want %q", got, want)
	}
	keys(m, "util", "enter")
	if m.file.rel != "src/util.py" || m.file.cursor != 1 {
		t.Fatalf("enter opens %s:%d", m.file.rel, m.file.cursor+1)
	}
}

func TestChangesOutsideGit(t *testing.T) {
	f := setup(t)
	m := f.model()
	keys(m, "ctrl+g")
	if m.picker != nil || m.flash != "not a git worktree: no changes to show" {
		t.Fatalf("picker %v, flash %q", m.picker, m.flash)
	}
	keys(m, "]", "h")
	if m.flash != "not a git worktree: no changes to show" {
		t.Fatalf("flash %q", m.flash)
	}
}

func TestReloadPicksUpNewChanges(t *testing.T) {
	f := changed(t)
	m := f.model()
	keys(m, "ctrl+p", "README", "enter")
	if len(m.file.hunks) != 0 {
		t.Fatalf("README.md is unchanged: %v", m.file.hunks)
	}
	f.write("README.md", "# Demo\n\nhello, world\n")
	keys(m, "r")
	if len(m.file.hunks) != 1 || m.file.hunks[0].NewStart != 3 {
		t.Fatalf("r re-reads the changes: %v", m.file.hunks)
	}
}
