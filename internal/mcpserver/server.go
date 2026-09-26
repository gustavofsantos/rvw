// Package mcpserver exposes the review [review.Service] as MCP tools, for agents
// that prefer tool calls to shelling out to the CLI. Each tool is one service
// operation: its input and output are the operation's typed input and output,
// so the schemas an agent sees are the ones in internal/review/operations.go.
package mcpserver

import (
	"context"
	"path/filepath"
	"strings"

	"github.com/modelcontextprotocol/go-sdk/mcp"

	"github.com/gustavofsantos/rvw/internal/review"
	"github.com/gustavofsantos/rvw/internal/workspace"
)

// Options are the server's defaults, set by `rvw mcp serve` flags.
type Options struct {
	// Workspace is used when a tool call leaves workspace empty.
	Workspace string
	// Lane pins every call that does not name a lane itself.
	Lane string
	// Author speaks for every call that does not name an author itself.
	Author  string
	Version string
}

const instructions = `rvw is a local queue of code review comments, keyed by workspace (the git
root of a directory; a directory outside git has none). Pass your project
directory as "workspace" on every call.

To address review comments: call pull once (pulled comments leave the queue;
use peek to look without taking). Work both the standalone "comments" and the
comments inside each of "reviews"; read each review's summary as context. Then
call resolve on every comment id (r3, not rv1) right after you finish it:
outcome "done" with a note of what you did, or "rejected" with the reason.
Never leave a pulled comment undecided; list with status "pulled" shows them.

To review code for another agent: add comments on line ranges, then submit
them as one review with a decision (comment, approve, request-changes).

Pass "author" with your name on writes. A pull pinned to a lane takes only
that lane; "elsewhere" says when work waits in other lanes.`

// New builds an MCP server whose tools call svc.
func New(svc *review.Service, opts Options) *mcp.Server {
	s := mcp.NewServer(
		&mcp.Implementation{Name: "rvw", Title: "rvw review queue", Version: opts.Version},
		&mcp.ServerOptions{Instructions: instructions},
	)
	h := &handlers{opts: opts}

	readOnly := &mcp.ToolAnnotations{ReadOnlyHint: true}
	tool(s, "add", "Enqueue one review comment on a line range of one file. The code is snapshotted from disk, or from source when given.", nil,
		func(ctx context.Context, in review.AddInput) (review.Comment, error) {
			if err := h.scope(ctx, &in.Workspace, &in.Lane, &in.Author); err != nil {
				return review.Comment{}, err
			}
			return svc.Add(ctx, in)
		})
	tool(s, "submit", "Submit one review with a decision and summary over pending comments. Without comment_ids it takes this author's pending, unlinked comments in this exact lane.", nil,
		func(ctx context.Context, in review.SubmitInput) (review.Review, error) {
			if err := h.scope(ctx, &in.Workspace, &in.Lane, &in.Author); err != nil {
				return review.Review{}, err
			}
			return svc.Submit(ctx, in)
		})
	tool(s, "list", "List review comments, oldest first, without dequeuing them.", readOnly,
		func(ctx context.Context, in review.QueryInput) (review.ListOutput, error) {
			if err := h.scope(ctx, &in.Workspace, &in.Lane, nil); err != nil {
				return review.ListOutput{}, err
			}
			return svc.List(ctx, in)
		})
	tool(s, "list_reviews", "List submitted review sheets with their state (pending, pulled, complete), oldest first.", readOnly,
		func(ctx context.Context, in review.SheetsInput) (review.SheetsOutput, error) {
			if err := h.scope(ctx, &in.Workspace, &in.Lane, nil); err != nil {
				return review.SheetsOutput{}, err
			}
			return svc.Sheets(ctx, in)
		})
	tool(s, "count", "Count pending handoffs (a submitted review counts once), or comments for any other status.", readOnly,
		func(ctx context.Context, in review.QueryInput) (review.CountOutput, error) {
			if err := h.scope(ctx, &in.Workspace, &in.Lane, nil); err != nil {
				return review.CountOutput{}, err
			}
			return svc.Count(ctx, in)
		})
	tool(s, "pull", "Dequeue submitted reviews and standalone comments to act on. Pulled handoffs leave the queue; peek reads without dequeuing. Record a decision on every pulled comment with resolve.", nil,
		func(ctx context.Context, in review.PullInput) (review.PullOutput, error) {
			if err := h.scope(ctx, &in.Workspace, &in.Lane, nil); err != nil {
				return review.PullOutput{}, err
			}
			return svc.Pull(ctx, in)
		})
	tool(s, "show_comment", "Show one comment (r3) with its decision and, once done, the diff it produced.", readOnly,
		func(ctx context.Context, in review.GetInput) (review.Evidence, error) {
			if err := h.scope(ctx, &in.Workspace, nil, nil); err != nil {
				return review.Evidence{}, err
			}
			return svc.Evidence(ctx, in)
		})
	tool(s, "show_review", "Show one submitted review sheet (rv1) with its linked comments and state.", readOnly,
		func(ctx context.Context, in review.GetInput) (review.ReviewSheet, error) {
			if err := h.scope(ctx, &in.Workspace, nil, nil); err != nil {
				return review.ReviewSheet{}, err
			}
			return svc.Sheet(ctx, in)
		})
	tool(s, "resolve", "Record the decision on a comment: done (what you did; the comment must be pulled) or rejected (why not; a note is required). A decision is final.", nil,
		func(ctx context.Context, in review.ResolveInput) (review.Comment, error) {
			if err := h.scope(ctx, &in.Workspace, nil, &in.Author); err != nil {
				return review.Comment{}, err
			}
			return svc.Resolve(ctx, in)
		})
	tool(s, "edit", "Replace the text of a queued comment. File, range and code snapshot are unchanged.", nil,
		func(ctx context.Context, in review.EditInput) (review.Comment, error) {
			if err := h.scope(ctx, &in.Workspace, nil, nil); err != nil {
				return review.Comment{}, err
			}
			return svc.Edit(ctx, in)
		})
	tool(s, "workspaces", "List every workspace in the store with its pending handoff count.", readOnly,
		func(ctx context.Context, in review.WorkspacesInput) (review.WorkspacesOutput, error) {
			return svc.Workspaces(ctx, in)
		})
	return s
}

// tool registers one operation. A service error becomes a tool error result
// carrying the same message the CLI prints.
func tool[In, Out any](s *mcp.Server, name, description string, ann *mcp.ToolAnnotations, op func(context.Context, In) (Out, error)) {
	mcp.AddTool(s, &mcp.Tool{Name: name, Description: description, Annotations: ann},
		func(ctx context.Context, _ *mcp.CallToolRequest, in In) (*mcp.CallToolResult, Out, error) {
			out, err := op(ctx, in)
			return nil, out, err
		})
}

type handlers struct{ opts Options }

// scope fills a call's workspace, lane and author from the server defaults and
// resolves the workspace to its canonical git root, as the CLI does.
func (h *handlers) scope(ctx context.Context, ws, lane, author *string) error {
	dir := strings.TrimSpace(*ws)
	if dir == "" {
		dir = h.opts.Workspace
	}
	if dir == "" {
		return review.Invalidf("workspace is required: pass the absolute path of your project directory")
	}
	if !filepath.IsAbs(dir) {
		return review.Invalidf("workspace '%s' is not an absolute path", dir)
	}
	if !workspace.IsDir(dir) {
		return review.Invalidf("workspace '%s' is not a directory", dir)
	}
	resolved, err := workspace.Resolve(ctx, dir)
	if err != nil {
		return review.Invalidf("workspace %s", err)
	}
	*ws = resolved
	if lane != nil {
		*lane = strings.TrimSpace(*lane)
		if *lane == "" {
			*lane = h.opts.Lane
		}
	}
	if author != nil {
		*author = strings.TrimSpace(*author)
		if *author == "" {
			*author = h.opts.Author
		}
	}
	return nil
}
