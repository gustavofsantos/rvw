package tui

import (
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	"github.com/charmbracelet/x/ansi"
	"github.com/charmbracelet/x/exp/golden"
	"github.com/gustavofsantos/rvw/internal/review"
)

// reviewed is the fixture after a round of review: rv1 asks for two changes,
// one done with an edit to parse.py and one rejected; rv2 approves with no
// comments; r3 is a standalone comment pulled but not decided, and r4 one
// still pending.
func reviewed(t *testing.T) *fixture {
	t.Helper()
	f := setup(t)
	ctx, ws := f.ctx, f.ws
	f.add("src/api/parse.py", 5, 8, "extract this branch into parse_x()", "reviewer")
	f.add("src/util.py", 2, 2, "typo", "reviewer")
	must := func(err error) {
		t.Helper()
		if err != nil {
			t.Fatal(err)
		}
	}
	_, err := f.svc.Submit(ctx, review.SubmitInput{Workspace: ws, Decision: review.DecisionRequestChanges, Summary: "Fix both before merging.", Author: "reviewer"})
	must(err)
	_, err = f.svc.Submit(ctx, review.SubmitInput{Workspace: ws, Decision: review.DecisionApprove, Summary: "Docs look good.", NoComments: true, Author: "reviewer"})
	must(err)
	f.add("README.md", 1, 1, "title case", "gustavo")
	_, err = f.svc.Pull(ctx, review.PullInput{Workspace: ws})
	must(err)
	f.add("src/util.py", 1, 1, "name it better", "gustavo")

	f.write("src/api/parse.py", strings.Replace(parsePy, "        body = req.body\n        return parse(body)\n", "        return parse_x(req)\n", 1))
	_, err = f.svc.Resolve(ctx, review.ResolveInput{Workspace: ws, ID: "r1", Outcome: review.OutcomeDone, Note: "extracted parse_x()", Author: "claude"})
	must(err)
	_, err = f.svc.Resolve(ctx, review.ResolveInput{Workspace: ws, ID: "r2", Outcome: review.OutcomeRejected, Note: "not a typo: pass is intended", Author: "claude"})
	must(err)
	return f
}

// screenText is the screen with colors stripped.
func screenText(m *model) string { return ansi.Strip(m.screen()) }

func TestReviewsSideListsReviewsNewestFirstThenStandaloneComments(t *testing.T) {
	m := reviewed(t).model()
	keys(m, "t", "t")
	if m.side != sideReviews || m.sideTitle() != "reviews" {
		t.Fatalf("t t shows the reviews: side %v", m.side)
	}
	want := []string{"rv2/", "rv1/", "  r1", "  r2", "comments/", "  r4", "  r3"}
	if got := rowNames(m.rows); !slices.Equal(got, want) {
		t.Fatalf("rows = %q, want %q", got, want)
	}
	s := screenText(m)
	for _, row := range []string{
		"▾ rv2 approve         ✓", "▾ rv1 request-changes ✓",
		"  r1 parse.py:5-8     ✓", "  r2 util.py:2        ✗",
		"  r4 util.py:1        ○", "  r3 README.md:1      ◐",
	} {
		if !strings.Contains(s, row) {
			t.Errorf("the sidebar has no row %q:\n%s", row, s)
		}
	}
	keys(m, "t")
	if m.side != sideFiles {
		t.Fatalf("t goes round to the files, side %v", m.side)
	}
}

func TestOpeningADoneCommentShowsHowItWasAddressed(t *testing.T) {
	m := reviewed(t).model()
	opened := m.file.rel
	keys(m, "t", "t", "tab", "j", "j", "l")
	if m.detail == nil || m.detail.id != "r1" || m.focus != paneViewer || m.file.rel != opened {
		t.Fatalf("l shows r1's page over %s, not a file: %+v", opened, m.detail)
	}
	s := screenText(m)
	for _, want := range []string{
		"r1 · src/api/parse.py:5-8",
		"done by @claude on ",
		"extract this branch into parse_x()",
		"   5      if req.kind == \"x\":",
		"extracted parse_x()",
		"-        body = req.body",
		"+        return parse_x(req)",
		"o opens src/api/parse.py at line 5",
	} {
		if !strings.Contains(s, want) {
			t.Errorf("the page has no %q:\n%s", want, s)
		}
	}

	if cmd := m.key(press("c")); cmd != nil || m.detail == nil {
		t.Fatal("c comments on nothing while a page is shown")
	}
	keys(m, "V", ":")
	if m.visual || m.goLine != nil {
		t.Fatal("V and : act on no file while a page is shown")
	}

	keys(m, "o")
	if m.detail != nil || m.file.rel != "src/api/parse.py" || m.file.cursor+1 != 5 {
		t.Fatalf("o opens the file on the comment's first line: %s:%d", m.file.rel, m.file.cursor+1)
	}
}

func TestARejectedCommentShowsWhyAndAnOpenOneItsState(t *testing.T) {
	m := reviewed(t).model()
	keys(m, "t", "t", "tab", "j", "j", "j", "l")
	s := screenText(m)
	if !strings.Contains(s, "rejected by @claude") || !strings.Contains(s, "not a typo: pass is intended") || strings.Contains(s, "Changes while resolving") {
		t.Fatalf("r2's page:\n%s", s)
	}
	keys(m, "tab", "G", "l")
	if s := screenText(m); !strings.Contains(s, "pulled on ") || !strings.Contains(s, "not decided yet") || strings.Contains(s, "Resolution") {
		t.Fatalf("r3's page:\n%s", s)
	}
	keys(m, "esc")
	if m.detail != nil || m.file == nil {
		t.Fatal("esc goes back to the file")
	}
}

func TestClickingAReviewShowsItsSheet(t *testing.T) {
	m := reviewed(t).model()
	keys(m, "t", "t", "tab", "h", "j", "h")
	if got := rowNames(m.rows); !slices.Equal(got[:2], []string{"rv2/", "rv1/"}) || len(got) != 5 {
		t.Fatalf("h collapses a review: %q", got)
	}
	m.Update(click(3, 2))
	if m.detail == nil || m.detail.id != "rv1" || len(m.rows) != 7 {
		t.Fatalf("a click shows rv1 and expands it: %+v, %d rows", m.detail, len(m.rows))
	}
	s := screenText(m)
	for _, want := range []string{"rv1 · request-changes · @reviewer", "Fix both before merging.", "✓ r1 src/api/parse.py:5-8", "@claude: extracted parse_x()", "✗ r2 src/util.py:2"} {
		if !strings.Contains(s, want) {
			t.Errorf("rv1's page has no %q:\n%s", want, s)
		}
	}
	keys(m, "o")
	if m.detail == nil || !strings.Contains(m.flash, "open one of its comments") {
		t.Fatalf("o on a review says to open a comment: %q", m.flash)
	}
	m.Update(click(3, 1))
	if s := screenText(m); !strings.Contains(s, "summary only: no comments linked") {
		t.Fatalf("rv2's page:\n%s", s)
	}
}

func TestASubmittedReviewShowsUpInTheReviewsSide(t *testing.T) {
	f := reviewed(t)
	m := f.model()
	keys(m, "t", "t", "tab", "j")
	path := filepath.Join(t.TempDir(), "summary")
	if err := os.WriteFile(path, []byte("Rename it.\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	m.editorDone(editorDoneMsg{req: editRequest{kind: editSummary, decision: review.DecisionComment}, path: path})
	if got := rowNames(m.rows); !slices.Equal(got[:1], []string{"rv3/"}) {
		t.Fatalf("the new review heads the list: %q", got)
	}
	if got := m.rows[m.treeCur].node.path; got != "rv1" {
		t.Fatalf("the cursor stays on rv1, not on %s", got)
	}
}

func TestReviewsSideWithNothingSaysSo(t *testing.T) {
	m := setup(t).model()
	keys(m, "t", "t")
	if !strings.Contains(screenText(m), " no reviews or comments") {
		t.Fatalf("screen:\n%s", screenText(m))
	}
}

func TestGoldenReviewPage(t *testing.T) {
	f := reviewed(t)
	golden.RequireEqual(t, view(t, f, seq("t", "t", "tab", "j", "j", "l")...))
}
