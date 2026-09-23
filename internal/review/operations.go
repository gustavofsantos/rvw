package review

// Inputs and outputs of every [Service] operation. Each one is a JSON object so
// it can double as an MCP tool's input or structured output. A `jsonschema` tag
// is the property's description; a field without `omitempty` is required.
//
// Workspace is always the canonical absolute path an adapter resolved (see
// package workspace). Lane "" means every lane; Author "" means anonymous.

// AddInput enqueues one comment on a line range of one file.
type AddInput struct {
	Workspace string  `json:"workspace" jsonschema:"absolute workspace path"`
	File      string  `json:"file" jsonschema:"file the comment is about, absolute or relative to the workspace"`
	StartLine int     `json:"start_line" jsonschema:"first line of the range, 1-indexed"`
	EndLine   int     `json:"end_line,omitempty" jsonschema:"last line of the range, inclusive; defaults to start_line"`
	Comment   string  `json:"comment" jsonschema:"the review note, markdown allowed"`
	Source    *string `json:"source,omitempty" jsonschema:"whole-file content to snapshot instead of reading the file from disk (e.g. an unsaved editor buffer)"`
	Filetype  string  `json:"filetype,omitempty" jsonschema:"language for fencing the code; guessed from the extension when empty"`
	Lane      string  `json:"lane,omitempty" jsonschema:"branch or lane the comment belongs to"`
	Author    string  `json:"author,omitempty" jsonschema:"who raises the comment"`
}

// SubmitInput groups pending comments under one review.
type SubmitInput struct {
	Workspace string   `json:"workspace" jsonschema:"absolute workspace path"`
	Decision  Decision `json:"decision" jsonschema:"comment, approve or request-changes"`
	Summary   string   `json:"summary" jsonschema:"overall assessment"`
	// CommentIDs selects the comments to link. Empty selects every pending,
	// unlinked comment by this author in this exact lane, unless NoComments.
	CommentIDs []string `json:"comment_ids,omitempty" jsonschema:"comments to link, in order; empty takes this author's pending comments in this lane"`
	NoComments bool     `json:"no_comments,omitempty" jsonschema:"submit a summary-only review, linking nothing"`
	Lane       string   `json:"lane,omitempty" jsonschema:"lane of the review"`
	Author     string   `json:"author,omitempty" jsonschema:"reviewer"`
}

// QueryInput scopes a read of the queue.
type QueryInput struct {
	Workspace string       `json:"workspace" jsonschema:"absolute workspace path"`
	Status    StatusFilter `json:"status,omitempty" jsonschema:"pending (default), pulled, done, rejected, open or all"`
	File      string       `json:"file,omitempty" jsonschema:"only comments on this file, absolute or relative to the workspace"`
	Lane      string       `json:"lane,omitempty" jsonschema:"only this lane's comments; empty means every lane"`
}

// ListOutput is the comments matching a [QueryInput], oldest first.
type ListOutput struct {
	Workspace string    `json:"workspace" jsonschema:"absolute workspace path"`
	Comments  []Comment `json:"comments" jsonschema:"matching comments, oldest first"`
}

// CountOutput counts pending handoffs: a submitted review counts once, with its
// linked comments; for any other status it counts comments.
type CountOutput struct {
	Workspace string `json:"workspace" jsonschema:"absolute workspace path"`
	Count     int    `json:"count" jsonschema:"number of handoffs (pending) or comments (other statuses)"`
}

// PullInput dequeues handoffs for an agent to act on.
type PullInput struct {
	Workspace string   `json:"workspace" jsonschema:"absolute workspace path"`
	Lane      string   `json:"lane,omitempty" jsonschema:"only this lane; a pinned pull never takes another lane's or an unlaned comment"`
	File      string   `json:"file,omitempty" jsonschema:"only handoffs touching this file"`
	IDs       []string `json:"ids,omitempty" jsonschema:"only these comment or review ids; a linked comment brings its whole review"`
	Limit     int      `json:"limit,omitempty" jsonschema:"at most this many handoffs, oldest first; 0 means no limit"`
	Peek      bool     `json:"peek,omitempty" jsonschema:"read without dequeuing"`
}

// PullOutput is what was handed over.
type PullOutput struct {
	Workspace string    `json:"workspace" jsonschema:"absolute workspace path"`
	Drained   bool      `json:"drained" jsonschema:"true when the handoffs left the queue (not a peek)"`
	Reviews   []Handoff `json:"reviews" jsonschema:"submitted reviews, each with its linked comments"`
	Comments  []Comment `json:"comments" jsonschema:"standalone comments"`
	// Elsewhere is set when a lane-pinned pull came back empty while handoffs
	// wait in other lanes, so an empty result never hides work.
	Elsewhere *Elsewhere `json:"elsewhere,omitempty" jsonschema:"handoffs waiting in other lanes, when this pinned pull took nothing"`
}

// Count is the number of handoffs taken.
func (o PullOutput) Count() int { return len(o.Reviews) + len(o.Comments) }

// Elsewhere counts handoffs pending outside the pinned lane.
type Elsewhere struct {
	Count int      `json:"count" jsonschema:"handoffs pending in other lanes"`
	Lanes []string `json:"lanes" jsonschema:"those lanes, sorted; (no lane) for unlaned comments"`
}

// ResolveInput records what became of a comment.
type ResolveInput struct {
	Workspace string  `json:"workspace" jsonschema:"absolute workspace path"`
	ID        string  `json:"id" jsonschema:"comment id, e.g. r3"`
	Outcome   Outcome `json:"outcome" jsonschema:"done, or rejected (which needs a note)"`
	Note      string  `json:"note,omitempty" jsonschema:"what was done, or why not"`
	Author    string  `json:"author,omitempty" jsonschema:"who decides"`
}

// GetInput names one comment or review.
type GetInput struct {
	Workspace string `json:"workspace" jsonschema:"absolute workspace path"`
	ID        string `json:"id" jsonschema:"comment id (r3) or review id (rv2)"`
}

// Evidence is a comment with what changed while resolving it.
type Evidence struct {
	Comment Comment `json:"comment"`
	// Diff is the unified diff between the reviewed and the resolved file, for
	// a comment that is done. DiffAvailable is false when either snapshot is
	// missing; an available empty diff means nothing changed.
	Diff          []string `json:"diff" jsonschema:"unified diff lines between the reviewed and resolved file versions"`
	DiffAvailable bool     `json:"diff_available" jsonschema:"whether both file snapshots exist"`
}

// SheetState is a submitted review's progress, derived from its comments.
type SheetState string

const (
	SheetPending  SheetState = "pending"
	SheetPulled   SheetState = "pulled"
	SheetComplete SheetState = "complete" // pulled, and every linked comment decided
)

// ReviewSheet is a submitted review with the comments it links.
type ReviewSheet struct {
	Review Review     `json:"review"`
	State  SheetState `json:"state" jsonschema:"pending, pulled, or complete once every linked comment is decided"`
	// Comments holds the linked comments that still exist, in review order;
	// a CommentID without a match here is missing evidence.
	Comments []Comment `json:"comments" jsonschema:"linked comments that still exist, in review order"`
}

// EditInput replaces a comment's text.
type EditInput struct {
	Workspace string `json:"workspace" jsonschema:"absolute workspace path"`
	ID        string `json:"id" jsonschema:"comment id"`
	Comment   string `json:"comment" jsonschema:"new text"`
}

// DropInput deletes comments without handing them over.
type DropInput struct {
	Workspace string   `json:"workspace" jsonschema:"absolute workspace path"`
	IDs       []string `json:"ids" jsonschema:"comment ids"`
}

// DropOutput names what was deleted.
type DropOutput struct {
	Workspace string   `json:"workspace" jsonschema:"absolute workspace path"`
	Dropped   []string `json:"dropped" jsonschema:"deleted comment ids"`
}

// ClearInput deletes comments in bulk.
type ClearInput struct {
	Workspace string       `json:"workspace" jsonschema:"absolute workspace path"`
	Status    StatusFilter `json:"status,omitempty" jsonschema:"which comments to delete; pending by default, all also deletes reviews"`
}

// ClearOutput counts what was deleted.
type ClearOutput struct {
	Workspace string       `json:"workspace" jsonschema:"absolute workspace path"`
	Status    StatusFilter `json:"status" jsonschema:"the status that was cleared"`
	Cleared   int          `json:"cleared" jsonschema:"number of comments deleted"`
}

// WorkspacesInput lists workspaces in the store.
type WorkspacesInput struct {
	All bool `json:"all,omitempty" jsonschema:"include workspaces with nothing pending"`
}

// WorkspacesOutput is every workspace holding handoffs.
type WorkspacesOutput struct {
	Workspaces []WorkspaceSummary `json:"workspaces"`
}

// WorkspaceSummary is one workspace's queue at a glance.
type WorkspaceSummary struct {
	Workspace string `json:"workspace" jsonschema:"absolute workspace path"`
	Pending   int    `json:"pending" jsonschema:"pending handoffs"`
	Pulled    int    `json:"pulled" jsonschema:"comments handed over and not yet decided"`
}
