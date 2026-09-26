package review_test

import (
	"context"
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"testing"

	"github.com/gustavofsantos/rvw/internal/review"
	"github.com/gustavofsantos/rvw/internal/store"
	"github.com/gustavofsantos/rvw/internal/workspace"
)

type fixture struct {
	t   *testing.T
	svc *review.Service
	ws  string
	ctx context.Context
}

func setup(t *testing.T) *fixture {
	t.Helper()
	root := t.TempDir()
	ws := filepath.Join(root, "proj")
	if err := os.MkdirAll(ws, 0o755); err != nil {
		t.Fatal(err)
	}
	git(t, ws, "init", "-q")
	write(t, filepath.Join(ws, "app.py"), "one\ntwo\nthree\nfour\n")
	st, err := store.Open(context.Background(), filepath.Join(root, "rvw.db"))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { st.Close() })
	canon, err := workspace.Resolve(ws)
	if err != nil {
		t.Fatal(err)
	}
	return &fixture{t: t, svc: review.NewService(st), ws: canon, ctx: context.Background()}
}

func git(t *testing.T, dir string, args ...string) {
	t.Helper()
	out, err := exec.Command("git", append([]string{"-C", dir}, args...)...).CombinedOutput()
	if err != nil {
		t.Fatalf("git %v: %v\n%s", args, err, out)
	}
}

func write(t *testing.T, path, content string) {
	t.Helper()
	if err := os.WriteFile(path, []byte(content), 0o644); err != nil {
		t.Fatal(err)
	}
}

func (f *fixture) add(line int, text, lane, author string) review.Comment {
	f.t.Helper()
	c, err := f.svc.Add(f.ctx, review.AddInput{
		Workspace: f.ws, File: "app.py", StartLine: line, Comment: text, Lane: lane, Author: author,
	})
	if err != nil {
		f.t.Fatal(err)
	}
	return c
}

func (f *fixture) pending(lane string) int {
	f.t.Helper()
	out, err := f.svc.Count(f.ctx, review.QueryInput{Workspace: f.ws, Lane: lane})
	if err != nil {
		f.t.Fatal(err)
	}
	return out.Count
}

func ids(cs []review.Comment) []string {
	out := []string{}
	for _, c := range cs {
		out = append(out, c.ID)
	}
	return out
}

func TestPullHandsEachCommentOverOnce(t *testing.T) {
	f := setup(t)
	f.add(1, "first", "", "")
	f.add(2, "second", "", "")

	out, err := f.svc.Pull(f.ctx, review.PullInput{Workspace: f.ws})
	if err != nil {
		t.Fatal(err)
	}
	if got := ids(out.Comments); !slices.Equal(got, []string{"r1", "r2"}) || !out.Drained {
		t.Fatalf("first pull took %v drained=%v", got, out.Drained)
	}
	if out.Comments[0].Status != review.StatusPulled || out.Comments[0].PulledAt == nil {
		t.Errorf("pulled comment not stamped: %+v", out.Comments[0])
	}
	again, _ := f.svc.Pull(f.ctx, review.PullInput{Workspace: f.ws})
	if again.Count() != 0 || f.pending("") != 0 {
		t.Errorf("a comment was handed over twice: %+v", again)
	}
}

func TestPinnedPullIsStrictAndSaysWhatWaitsElsewhere(t *testing.T) {
	f := setup(t)
	f.add(1, "unlaned", "", "")
	f.add(2, "payments", "payments", "")

	out, err := f.svc.Pull(f.ctx, review.PullInput{Workspace: f.ws, Lane: "auth"})
	if err != nil {
		t.Fatal(err)
	}
	if out.Count() != 0 {
		t.Fatalf("pinned pull swallowed another lane: %+v", out)
	}
	want := &review.Elsewhere{Count: 2, Lanes: []string{"(no lane)", "payments"}}
	if out.Elsewhere == nil || out.Elsewhere.Count != want.Count || !slices.Equal(out.Elsewhere.Lanes, want.Lanes) {
		t.Errorf("elsewhere = %+v, want %+v", out.Elsewhere, want)
	}
	if f.pending("") != 2 {
		t.Errorf("other lanes were drained")
	}
}

func TestSubmittedReviewMovesAsOneHandoff(t *testing.T) {
	f := setup(t)
	f.add(1, "first finding", "", "alice")
	f.add(2, "second finding", "", "alice")
	f.add(3, "bob's finding", "", "bob")

	r, err := f.svc.Submit(f.ctx, review.SubmitInput{
		Workspace: f.ws, Decision: review.DecisionRequestChanges, Summary: "Fix both.", Author: "alice",
	})
	if err != nil {
		t.Fatal(err)
	}
	if !slices.Equal(r.CommentIDs, []string{"r1", "r2"}) {
		t.Fatalf("default selection took %v, want only alice's", r.CommentIDs)
	}
	if n := f.pending(""); n != 2 {
		t.Errorf("pending handoffs = %d, want the review plus bob's comment", n)
	}

	out, err := f.svc.Pull(f.ctx, review.PullInput{Workspace: f.ws, IDs: []string{"r2"}})
	if err != nil {
		t.Fatal(err)
	}
	if len(out.Reviews) != 1 || len(out.Comments) != 0 || !slices.Equal(ids(out.Reviews[0].Comments), []string{"r1", "r2"}) {
		t.Fatalf("pulling a linked comment did not bring its review: %+v", out)
	}

	sheet, _ := f.svc.Sheet(f.ctx, review.GetInput{Workspace: f.ws, ID: "rv1"})
	if sheet.State != review.SheetPulled {
		t.Errorf("state = %s, want pulled", sheet.State)
	}
	for _, id := range []string{"r1", "r2"} {
		if _, err := f.svc.Resolve(f.ctx, review.ResolveInput{Workspace: f.ws, ID: id, Outcome: review.OutcomeDone}); err != nil {
			t.Fatal(err)
		}
	}
	sheet, _ = f.svc.Sheet(f.ctx, review.GetInput{Workspace: f.ws, ID: "rv1"})
	if sheet.State != review.SheetComplete {
		t.Errorf("state = %s, want complete once every comment is decided", sheet.State)
	}
}

func TestResolveKeepsBothVersionsAndDiffsThem(t *testing.T) {
	f := setup(t)
	git(t, f.ws, "add", "app.py")
	c := f.add(2, "change this", "", "")
	if c.FileVersion == "" {
		t.Fatal("a tracked file was enqueued without its reviewed version")
	}
	write(t, filepath.Join(f.ws, "app.py"), "one\nchanged-two\nthree\nfour\n")
	if _, err := f.svc.Pull(f.ctx, review.PullInput{Workspace: f.ws}); err != nil {
		t.Fatal(err)
	}

	done, err := f.svc.Resolve(f.ctx, review.ResolveInput{
		Workspace: f.ws, ID: "r1", Outcome: review.OutcomeDone, Note: "renamed", Author: "impl",
	})
	if err != nil {
		t.Fatal(err)
	}
	if *done.ResolvedBy != "impl" || *done.ResolutionNote != "renamed" || done.ResolvedFileVersion == "" {
		t.Errorf("resolution not recorded: %+v", done)
	}
	ev, err := f.svc.Evidence(f.ctx, review.GetInput{Workspace: f.ws, ID: "r1"})
	if err != nil {
		t.Fatal(err)
	}
	if !ev.DiffAvailable || !slices.Contains(ev.Diff, "-two") || !slices.Contains(ev.Diff, "+changed-two") {
		t.Errorf("diff = %v (available=%v)", ev.Diff, ev.DiffAvailable)
	}

	_, err = f.svc.Resolve(f.ctx, review.ResolveInput{Workspace: f.ws, ID: "r1", Outcome: review.OutcomeRejected, Note: "no"})
	if review.KindOf(err) != review.KindConflict {
		t.Errorf("re-deciding: err = %v, want a conflict", err)
	}
}

func TestErrorsAreTyped(t *testing.T) {
	f := setup(t)
	cases := []struct {
		name string
		call func() error
		kind review.ErrorKind
	}{
		{"unknown id", func() error {
			_, err := f.svc.Get(f.ctx, review.GetInput{Workspace: f.ws, ID: "r9"})
			return err
		}, review.KindNotFound},
		{"past the end", func() error {
			_, err := f.svc.Add(f.ctx, review.AddInput{Workspace: f.ws, File: "app.py", StartLine: 99, Comment: "x"})
			return err
		}, review.KindInvalid},
		{"reject without a reason", func() error {
			_, err := f.svc.Resolve(f.ctx, review.ResolveInput{Workspace: f.ws, ID: "r1", Outcome: review.OutcomeRejected})
			return err
		}, review.KindInvalid},
		{"unresolved workspace", func() error {
			_, err := f.svc.List(f.ctx, review.QueryInput{Workspace: ""})
			return err
		}, review.KindInvalid},
		{"relative workspace", func() error {
			_, err := f.svc.Add(f.ctx, review.AddInput{Workspace: "proj", File: "app.py", StartLine: 1, Comment: "x"})
			return err
		}, review.KindInvalid},
	}
	for _, c := range cases {
		if err := c.call(); err == nil || review.KindOf(err) != c.kind {
			t.Errorf("%s: err = %v, want kind %s", c.name, err, c.kind)
		}
	}
}

// The JSON shape is a contract with editors and agents: nullable fields are
// present as null, optional ones are absent, and lists are never null.
func TestCommentJSONShape(t *testing.T) {
	f := setup(t)
	c := f.add(1, "a <b> & c", "", "")
	raw, err := json.Marshal(c)
	if err != nil {
		t.Fatal(err)
	}
	var fields map[string]any
	if err := json.Unmarshal(raw, &fields); err != nil {
		t.Fatal(err)
	}
	for _, k := range []string{"lane", "author", "pulled_at", "resolved_at", "resolved_by", "resolution_note"} {
		if v, ok := fields[k]; !ok || v != nil {
			t.Errorf("%s should be present and null, got %v (present=%v)", k, v, ok)
		}
	}
	for _, k := range []string{"file_version", "resolved_file_version", "review_id", "edited_at"} {
		if _, ok := fields[k]; ok {
			t.Errorf("%s should be absent", k)
		}
	}
	list, _ := f.svc.List(f.ctx, review.QueryInput{Workspace: f.ws, Status: review.FilterDone})
	if list.Comments == nil {
		t.Error("an empty list must marshal as [], not null")
	}
}
