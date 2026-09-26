package mcpserver_test

import (
	"context"
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	"github.com/modelcontextprotocol/go-sdk/mcp"

	"github.com/gustavofsantos/rvw/internal/mcpserver"
	"github.com/gustavofsantos/rvw/internal/review"
	"github.com/gustavofsantos/rvw/internal/store"
)

type fixture struct {
	t       *testing.T
	ctx     context.Context
	session *mcp.ClientSession
	ws      string // as the agent names it: a subdirectory of the git root
	root    string // the canonical workspace the service keys on
}

func setup(t *testing.T, opts mcpserver.Options) *fixture {
	t.Helper()
	return setupWith(t, func(string) mcpserver.Options { return opts })
}

// setupWith builds the server options once the workspace exists.
func setupWith(t *testing.T, options func(ws string) mcpserver.Options) *fixture {
	t.Helper()
	dir := t.TempDir()
	root := filepath.Join(dir, "proj")
	sub := filepath.Join(root, "src")
	if err := os.MkdirAll(sub, 0o755); err != nil {
		t.Fatal(err)
	}
	if out, err := exec.Command("git", "-C", root, "init", "-q").CombinedOutput(); err != nil {
		t.Fatalf("git init: %v\n%s", err, out)
	}
	if err := os.WriteFile(filepath.Join(root, "app.py"), []byte("one\ntwo\nthree\nfour\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	st, err := store.Open(context.Background(), filepath.Join(dir, "rvw.db"))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { st.Close() })

	ctx := context.Background()
	server := mcpserver.New(review.NewService(st), options(sub))
	serverT, clientT := mcp.NewInMemoryTransports()
	ss, err := server.Connect(ctx, serverT, nil)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { ss.Close() })
	cs, err := mcp.NewClient(&mcp.Implementation{Name: "test"}, nil).Connect(ctx, clientT, nil)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { cs.Close() })
	canon, _ := filepath.EvalSymlinks(root)
	return &fixture{t: t, ctx: ctx, session: cs, ws: sub, root: canon}
}

// call invokes a tool and decodes its structured output into out.
func (f *fixture) call(name string, args map[string]any, out any) {
	f.t.Helper()
	res := f.callRaw(name, args)
	if res.IsError {
		f.t.Fatalf("%s: tool error: %s", name, text(res))
	}
	data, err := json.Marshal(res.StructuredContent)
	if err != nil {
		f.t.Fatal(err)
	}
	if err := json.Unmarshal(data, out); err != nil {
		f.t.Fatalf("%s: %v\n%s", name, err, data)
	}
}

func (f *fixture) callRaw(name string, args map[string]any) *mcp.CallToolResult {
	f.t.Helper()
	res, err := f.session.CallTool(f.ctx, &mcp.CallToolParams{Name: name, Arguments: args})
	if err != nil {
		f.t.Fatalf("%s: %v", name, err)
	}
	return res
}

func text(res *mcp.CallToolResult) string {
	var b strings.Builder
	for _, c := range res.Content {
		if t, ok := c.(*mcp.TextContent); ok {
			b.WriteString(t.Text)
		}
	}
	return b.String()
}

func TestToolsCoverEveryOperation(t *testing.T) {
	f := setup(t, mcpserver.Options{})
	res, err := f.session.ListTools(f.ctx, nil)
	if err != nil {
		t.Fatal(err)
	}
	var names []string
	for _, tool := range res.Tools {
		names = append(names, tool.Name)
		if tool.OutputSchema == nil {
			t.Errorf("%s: no output schema", tool.Name)
		}
	}
	slices.Sort(names)
	want := []string{"add", "count", "edit", "list", "list_reviews", "pull", "resolve", "show_comment", "show_review", "submit", "workspaces"}
	if !slices.Equal(names, want) {
		t.Errorf("tools = %v, want %v", names, want)
	}
}

// The agent's round trip: add, pull, resolve, then read the evidence back.
func TestAddPullResolve(t *testing.T) {
	f := setup(t, mcpserver.Options{Author: "claude"})

	var c review.Comment
	f.call("add", map[string]any{
		"workspace": f.ws, "file": "app.py", "start_line": 2, "end_line": 3, "comment": "rename",
	}, &c)
	if c.ID != "r1" || c.Workspace != f.root {
		t.Fatalf("add = %s in %s, want r1 in %s", c.ID, c.Workspace, f.root)
	}
	if c.Author == nil || *c.Author != "claude" {
		t.Errorf("author = %v, want the server default claude", c.Author)
	}

	var pulled review.PullOutput
	f.call("pull", map[string]any{"workspace": f.ws}, &pulled)
	if !pulled.Drained || len(pulled.Comments) != 1 || pulled.Comments[0].ID != "r1" {
		t.Fatalf("pull = %+v", pulled)
	}
	f.call("pull", map[string]any{"workspace": f.ws}, &pulled)
	if pulled.Count() != 0 {
		t.Fatalf("second pull took %d, want nothing", pulled.Count())
	}

	var done review.Comment
	f.call("resolve", map[string]any{"workspace": f.ws, "id": "r1", "outcome": "done", "note": "renamed", "author": "bot"}, &done)
	if done.Status != review.StatusDone || *done.ResolvedBy != "bot" {
		t.Fatalf("resolve = %s by %v", done.Status, done.ResolvedBy)
	}

	var ev review.Evidence
	f.call("show_comment", map[string]any{"workspace": f.ws, "id": "r1"}, &ev)
	if ev.Comment.Status != review.StatusDone {
		t.Errorf("show_comment status = %s", ev.Comment.Status)
	}
}

func TestServiceErrorsAreToolErrors(t *testing.T) {
	f := setup(t, mcpserver.Options{})
	res := f.callRaw("resolve", map[string]any{"workspace": f.ws, "id": "r9", "outcome": "done"})
	if !res.IsError || !strings.Contains(text(res), "r9") {
		t.Errorf("resolve r9 = error %v %q, want a tool error naming r9", res.IsError, text(res))
	}
	res = f.callRaw("count", map[string]any{"workspace": "relative/dir"})
	if !res.IsError || !strings.Contains(text(res), "absolute") {
		t.Errorf("relative workspace = error %v %q, want a tool error", res.IsError, text(res))
	}
}

// An empty workspace falls back to the server's; an empty lane to its pin.
func TestServerDefaults(t *testing.T) {
	f := setupWith(t, func(ws string) mcpserver.Options { return mcpserver.Options{Workspace: ws, Lane: "auth"} })

	var c review.Comment
	f.call("add", map[string]any{"workspace": "", "file": "app.py", "start_line": 1, "comment": "pinned"}, &c)
	if c.Lane == nil || *c.Lane != "auth" || c.Workspace != f.root {
		t.Fatalf("defaulted add = lane %v in %s, want auth in %s", c.Lane, c.Workspace, f.root)
	}
	f.call("add", map[string]any{"workspace": f.ws, "file": "app.py", "start_line": 2, "comment": "elsewhere", "lane": "ui"}, &c)

	var pulled review.PullOutput
	f.call("pull", map[string]any{"workspace": ""}, &pulled)
	if pulled.Count() != 1 || pulled.Comments[0].ID != "r1" {
		t.Fatalf("pinned pull = %+v, want only r1", pulled)
	}
	f.call("pull", map[string]any{"workspace": ""}, &pulled)
	if pulled.Elsewhere == nil || !slices.Equal(pulled.Elsewhere.Lanes, []string{"ui"}) {
		t.Errorf("empty pinned pull elsewhere = %+v, want lane ui", pulled.Elsewhere)
	}
}
