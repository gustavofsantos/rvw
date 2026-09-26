// Package review is the domain of rvw: a per-workspace queue of review comments,
// raised by a human or an agent and pulled by whichever agent does the work.
//
// Every operation is a method on [Service] taking one typed input and returning
// one typed output. The CLI and a future MCP server are thin adapters over these:
// they resolve flags, environment and stdin into an input, and render the output.
// The service itself never touches stdin, stdout, the environment or the cwd.
package review

import (
	"fmt"
	"slices"
	"strconv"
	"strings"
	"time"
)

// Status is where a comment stands: pending → pulled → done | rejected.
// A comment is "open" until someone records what became of it.
type Status string

const (
	StatusPending  Status = "pending"
	StatusPulled   Status = "pulled"
	StatusDone     Status = "done"
	StatusRejected Status = "rejected"
)

// Resolved reports whether the comment carries a final decision.
func (s Status) Resolved() bool { return s == StatusDone || s == StatusRejected }

// StatusFilter selects comments by status. Besides every [Status] it accepts
// "open" (pending + pulled) and "all".
type StatusFilter string

const (
	FilterPending  StatusFilter = "pending"
	FilterPulled   StatusFilter = "pulled"
	FilterDone     StatusFilter = "done"
	FilterRejected StatusFilter = "rejected"
	FilterOpen     StatusFilter = "open"
	FilterAll      StatusFilter = "all"
)

// StatusFilters lists every accepted filter, in help order.
var StatusFilters = []StatusFilter{FilterPending, FilterPulled, FilterDone, FilterRejected, FilterOpen, FilterAll}

// Statuses expands the filter to the concrete statuses it matches.
func (f StatusFilter) Statuses() ([]Status, error) {
	switch f {
	case "", FilterPending:
		return []Status{StatusPending}, nil
	case FilterPulled, FilterDone, FilterRejected:
		return []Status{Status(f)}, nil
	case FilterOpen:
		return []Status{StatusPending, StatusPulled}, nil
	case FilterAll:
		return []Status{StatusPending, StatusPulled, StatusDone, StatusRejected}, nil
	}
	return nil, Invalidf("status %q is not one of %s", f, joinValues(StatusFilters))
}

// Decision is the verdict a submitted review carries.
type Decision string

const (
	DecisionComment        Decision = "comment"
	DecisionApprove        Decision = "approve"
	DecisionRequestChanges Decision = "request-changes"
)

// Decisions lists every accepted decision, in help order.
var Decisions = []Decision{DecisionComment, DecisionApprove, DecisionRequestChanges}

func (d Decision) validate() error {
	if slices.Contains(Decisions, d) {
		return nil
	}
	return Invalidf("decision %q is not one of %s", d, joinValues(Decisions))
}

// Outcome is the decision recorded on a single comment.
type Outcome string

const (
	OutcomeDone     Outcome = Outcome(StatusDone)
	OutcomeRejected Outcome = Outcome(StatusRejected)
)

// Comment is one note on a line range of one file. The JSON shape is the public
// contract read by editors and agents: nullable fields are always present,
// optional ones are omitted when absent.
type Comment struct {
	ID        string  `json:"id" jsonschema:"stable id, e.g. r3; never reused within a workspace"`
	Status    Status  `json:"status" jsonschema:"pending, pulled, done or rejected"`
	Workspace string  `json:"workspace" jsonschema:"absolute path of the workspace the comment belongs to"`
	Lane      *string `json:"lane" jsonschema:"branch or lane the comment belongs to, null when unscoped"`
	Author    *string `json:"author" jsonschema:"who raised the comment"`
	File      string  `json:"file" jsonschema:"path relative to the workspace (absolute only on comments recorded outside it by older versions)"`
	Path      string  `json:"path" jsonschema:"absolute path of the file"`
	StartLine int     `json:"start_line" jsonschema:"first line of the range, 1-indexed"`
	EndLine   int     `json:"end_line" jsonschema:"last line of the range, inclusive"`
	Filetype  string  `json:"filetype" jsonschema:"language of the code snapshot, for fencing"`
	Code      string  `json:"code" jsonschema:"the reviewed lines as they stood when the comment was written"`
	Comment   string  `json:"comment" jsonschema:"the reviewer's note"`
	CreatedAt string  `json:"created_at" jsonschema:"RFC 3339 UTC timestamp"`

	PulledAt       *string `json:"pulled_at" jsonschema:"when the comment was handed over"`
	ResolvedAt     *string `json:"resolved_at" jsonschema:"when a decision was recorded"`
	ResolvedBy     *string `json:"resolved_by" jsonschema:"who recorded the decision"`
	ResolutionNote *string `json:"resolution_note" jsonschema:"what was done, or why not"`

	FileVersion         string `json:"file_version,omitempty" jsonschema:"git blob of the whole file as reviewed"`
	ResolvedFileVersion string `json:"resolved_file_version,omitempty" jsonschema:"git blob of the whole file when resolved"`
	ReviewID            string `json:"review_id,omitempty" jsonschema:"submitted review this comment is linked to"`
	EditedAt            string `json:"edited_at,omitempty" jsonschema:"when the note was last edited"`
}

// Location is `file:N` or `file:N-M`.
func (c Comment) Location() string {
	if c.StartLine == c.EndLine {
		return fmt.Sprintf("%s:%d", c.File, c.StartLine)
	}
	return fmt.Sprintf("%s:%d-%d", c.File, c.StartLine, c.EndLine)
}

// ReviewStatus is where a submitted review stands. Its linked comments carry
// their own resolution.
type ReviewStatus string

const (
	ReviewPending ReviewStatus = "pending"
	ReviewPulled  ReviewStatus = "pulled"
)

// Review groups zero or more comments under one decision and summary. Pulling
// any linked comment hands over the whole review.
type Review struct {
	ID         string       `json:"id" jsonschema:"stable id, e.g. rv2"`
	Status     ReviewStatus `json:"status" jsonschema:"pending or pulled"`
	Workspace  string       `json:"workspace" jsonschema:"absolute path of the workspace"`
	Lane       *string      `json:"lane" jsonschema:"lane the review belongs to"`
	Author     *string      `json:"author" jsonschema:"who submitted the review"`
	Decision   Decision     `json:"decision" jsonschema:"comment, approve or request-changes"`
	Summary    string       `json:"summary" jsonschema:"the reviewer's overall assessment"`
	CommentIDs []string     `json:"comment_ids" jsonschema:"linked comment ids, in review order"`
	CreatedAt  string       `json:"created_at" jsonschema:"RFC 3339 UTC timestamp"`
	PulledAt   *string      `json:"pulled_at" jsonschema:"when the review was handed over"`
}

// Handoff is a review as it is pulled: the review plus its linked comments.
type Handoff struct {
	Review
	Comments []Comment `json:"comments" jsonschema:"the linked comments that still exist, in review order"`
}

// ── ids ──────────────────────────────────────────────────────────────────────

const (
	commentPrefix = "r"
	reviewPrefix  = "rv"
)

// CommentID formats a comment sequence number as its public id.
func CommentID(seq int64) string { return commentPrefix + strconv.FormatInt(seq, 10) }

// ReviewID formats a review sequence number as its public id.
func ReviewID(seq int64) string { return reviewPrefix + strconv.FormatInt(seq, 10) }

// IsReviewID reports whether id names a submitted review rather than a comment.
func IsReviewID(id string) bool { return strings.HasPrefix(id, reviewPrefix) }

// ── line ranges ──────────────────────────────────────────────────────────────

// LineRange is a 1-indexed inclusive range.
type LineRange struct {
	Start int `json:"start_line" jsonschema:"first line, 1-indexed"`
	End   int `json:"end_line" jsonschema:"last line, inclusive"`
}

// ParseLineRange accepts `N` or `N-M` (also `N:M`, `N,M`), swapping a reversed range.
func ParseLineRange(spec string) (LineRange, error) {
	s := strings.TrimSpace(spec)
	sep := strings.IndexAny(s, "-:,")
	first, second := s, ""
	if sep >= 0 {
		first, second = strings.TrimSpace(s[:sep]), strings.TrimSpace(s[sep+1:])
	}
	start, err := parseLine(first)
	if err != nil {
		return LineRange{}, Invalidf("--lines '%s' is not N or N-M", spec)
	}
	end := start
	if sep >= 0 {
		if end, err = parseLine(second); err != nil {
			return LineRange{}, Invalidf("--lines '%s' is not N or N-M", spec)
		}
	}
	return LineRange{Start: start, End: end}.normalized()
}

func parseLine(s string) (int, error) {
	if s == "" || strings.Trim(s, "0123456789") != "" {
		return 0, fmt.Errorf("not a number")
	}
	return strconv.Atoi(s)
}

func (r LineRange) normalized() (LineRange, error) {
	if r.Start < 1 || r.End < 1 {
		return r, Invalidf("--lines is 1-indexed; N must be >= 1")
	}
	if r.Start > r.End {
		r.Start, r.End = r.End, r.Start
	}
	return r, nil
}

// ── time ─────────────────────────────────────────────────────────────────────

// timestampLayout matches the stored format: second resolution, explicit offset.
const timestampLayout = "2006-01-02T15:04:05-07:00"

func formatTime(t time.Time) string { return t.UTC().Truncate(time.Second).Format(timestampLayout) }

func joinValues[T ~string](values []T) string {
	parts := make([]string, len(values))
	for i, v := range values {
		parts[i] = string(v)
	}
	return strings.Join(parts, ", ")
}

func strPtr(s string) *string {
	if s == "" {
		return nil
	}
	return &s
}
