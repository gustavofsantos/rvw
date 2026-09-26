package review

import (
	"cmp"
	"context"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"time"

	"github.com/gustavofsantos/rvw/internal/gitx"
	"github.com/gustavofsantos/rvw/internal/workspace"
)

// Service runs every review operation against a [Repository].
type Service struct {
	repo Repository
	now  func() time.Time
}

// NewService returns a service over repo.
func NewService(repo Repository) *Service {
	return &Service{repo: repo, now: time.Now}
}

// checkWorkspace refuses a workspace an adapter forgot to resolve: an empty or
// relative path would silently open a separate queue.
func checkWorkspace(ws string) error {
	if ws == "" || !filepath.IsAbs(ws) {
		return Invalidf("workspace '%s' is not an absolute path", ws)
	}
	return nil
}

func (s *Service) update(ctx context.Context, ws string, fn func(Tx) error) error {
	if err := checkWorkspace(ws); err != nil {
		return err
	}
	return s.repo.Update(ctx, ws, fn)
}

func (s *Service) view(ctx context.Context, ws string, fn func(Tx) error) error {
	if err := checkWorkspace(ws); err != nil {
		return err
	}
	return s.repo.View(ctx, ws, fn)
}

// Location is where the service keeps its data.
func (s *Service) Location() string { return s.repo.Location() }

func (s *Service) stamp() string { return formatTime(s.now()) }

// ── add / submit ─────────────────────────────────────────────────────────────

// Add enqueues one comment, snapshotting the reviewed lines, and, for a file
// tracked by git, the whole reviewed version as a blob.
func (s *Service) Add(ctx context.Context, in AddInput) (Comment, error) {
	if err := checkWorkspace(in.Workspace); err != nil {
		return Comment{}, err
	}
	if strings.TrimSpace(in.Comment) == "" {
		return Comment{}, Invalidf("a review comment needs text")
	}
	if in.EndLine == 0 {
		in.EndLine = in.StartLine
	}
	lines, err := LineRange{Start: in.StartLine, End: in.EndLine}.normalized()
	if err != nil {
		return Comment{}, err
	}
	path, err := s.filePath(in.Workspace, in.File)
	if err != nil {
		return Comment{}, err
	}
	rel, ok := inWorkspace(in.Workspace, path)
	if !ok {
		return Comment{}, Invalidf("%s is outside the workspace %s", path, in.Workspace)
	}

	source, err := readSource(path, in.Source)
	if err != nil {
		return Comment{}, err
	}
	sourceLines := splitSource(source)
	if lines.Start > len(sourceLines) {
		return Comment{}, Invalidf("lines %s is past the end of %s (%d lines)", lines.label(), path, len(sourceLines))
	}
	lines.End = min(lines.End, len(sourceLines))
	code := strings.Join(sourceLines[lines.Start-1:lines.End], "\n")

	fileVersion, err := snapshotReviewed(ctx, in.Workspace, rel, source)
	if err != nil {
		return Comment{}, err
	}

	filetype := in.Filetype
	if filetype == "" {
		filetype = filetypes[strings.ToLower(filepath.Ext(path))]
	}
	c := Comment{
		Status:      StatusPending,
		Workspace:   in.Workspace,
		Lane:        strPtr(in.Lane),
		Author:      strPtr(in.Author),
		File:        rel,
		Path:        path,
		StartLine:   lines.Start,
		EndLine:     lines.End,
		Filetype:    filetype,
		Code:        code,
		Comment:     strings.Trim(in.Comment, "\n"),
		FileVersion: fileVersion,
	}
	err = s.update(ctx, in.Workspace, func(tx Tx) error {
		seq, err := tx.NextCommentSeq()
		if err != nil {
			return err
		}
		c.ID = CommentID(seq)
		c.CreatedAt = s.stamp()
		return tx.InsertComment(c)
	})
	return c, err
}

// MaxSource is the largest file, or buffer, a comment snapshots.
const MaxSource = 4 << 20

// readSource is the reviewed content: the buffer when given, else the file at
// path. Only a regular file is opened, so a FIFO or a device never blocks or
// floods the read.
func readSource(path string, buffer *string) (string, error) {
	tooLarge := Invalidf("%s is larger than %d MiB", path, MaxSource>>20)
	if buffer != nil {
		if len(*buffer) > MaxSource {
			return "", tooLarge
		}
		return *buffer, nil
	}
	info, err := os.Stat(path)
	if err != nil {
		return "", notFoundf("%s does not exist — pass its content to snapshot unsaved changes", path)
	}
	if !info.Mode().IsRegular() {
		return "", Invalidf("%s is not a regular file", path)
	}
	f, err := os.Open(path)
	if err != nil {
		return "", notFoundf("%s cannot be read: %v", path, err)
	}
	defer f.Close()
	data, err := io.ReadAll(io.LimitReader(f, MaxSource+1))
	if err != nil {
		return "", internalf("cannot read %s: %v", path, err)
	}
	if len(data) > MaxSource {
		return "", tooLarge
	}
	return string(data), nil
}

// snapshotReviewed keeps the reviewed content of a tracked file as a git blob.
// A git failure refuses the comment rather than enqueue it without its version.
func snapshotReviewed(ctx context.Context, ws, rel, source string) (string, error) {
	tracked, err := gitx.Tracked(ctx, ws, rel)
	if err != nil {
		return "", internalf("cannot inspect Git metadata for %s: %s", rel, gitx.Stderr(err))
	}
	if !tracked {
		return "", nil
	}
	id, err := gitx.StoreContent(ctx, ws, source)
	if err != nil {
		return "", internalf("cannot store the reviewed version of %s: %s", rel, gitx.Stderr(err))
	}
	return id, nil
}

// Submit records one review over an explicit, all-or-nothing set of comments.
func (s *Service) Submit(ctx context.Context, in SubmitInput) (Review, error) {
	summary := strings.TrimSpace(in.Summary)
	if summary == "" {
		return Review{}, Invalidf("a submitted review needs a summary")
	}
	if err := in.Decision.validate(); err != nil {
		return Review{}, err
	}
	if in.NoComments && len(in.CommentIDs) > 0 {
		return Review{}, Invalidf("pass comment ids or no_comments, not both")
	}
	lane, author := strPtr(in.Lane), strPtr(in.Author)

	var r Review
	err := s.update(ctx, in.Workspace, func(tx Tx) error {
		var ids []string
		switch {
		case in.NoComments:
			ids = []string{}
		case len(in.CommentIDs) > 0:
			ids = unique(in.CommentIDs)
		default:
			pending, err := tx.Comments(StatusPending)
			if err != nil {
				return err
			}
			ids = []string{}
			for _, c := range pending {
				if sameOpt(c.Lane, lane) && sameOpt(c.Author, author) && c.ReviewID == "" {
					ids = append(ids, c.ID)
				}
			}
		}
		for _, id := range ids {
			c, err := find(tx, id)
			if err != nil {
				return err
			}
			switch {
			case c.Status != StatusPending:
				return conflictf("%s is not pending", c.ID)
			case c.ReviewID != "":
				return conflictf("%s already belongs to %s", c.ID, c.ReviewID)
			case !sameOpt(c.Lane, lane):
				return conflictf("%s belongs to a different lane", c.ID)
			case !sameOpt(c.Author, author):
				return conflictf("%s belongs to a different author", c.ID)
			}
		}
		seq, err := tx.NextReviewSeq()
		if err != nil {
			return err
		}
		r = Review{
			ID: ReviewID(seq), Status: ReviewPending, Workspace: in.Workspace,
			Lane: lane, Author: author, Decision: in.Decision, Summary: summary,
			CommentIDs: ids, CreatedAt: s.stamp(),
		}
		return tx.InsertReview(r)
	})
	return r, err
}

// ── reads ────────────────────────────────────────────────────────────────────

// List returns the comments a query scopes to, oldest first. Nothing moves.
func (s *Service) List(ctx context.Context, in QueryInput) (ListOutput, error) {
	out := ListOutput{Workspace: in.Workspace, Comments: []Comment{}}
	statuses, err := in.Status.Statuses()
	if err != nil {
		return out, err
	}
	scope, err := s.scope(in.Workspace, in.File, in.Lane)
	if err != nil {
		return out, err
	}
	err = s.view(ctx, in.Workspace, func(tx Tx) error {
		comments, err := tx.Comments(statuses...)
		out.Comments = scope.comments(comments)
		return err
	})
	return out, err
}

// Count counts what a query scopes to. For pending it counts handoffs: a
// submitted review once, with its linked comments.
func (s *Service) Count(ctx context.Context, in QueryInput) (CountOutput, error) {
	out := CountOutput{Workspace: in.Workspace}
	statuses, err := in.Status.Statuses()
	if err != nil {
		return out, err
	}
	scope, err := s.scope(in.Workspace, in.File, in.Lane)
	if err != nil {
		return out, err
	}
	err = s.view(ctx, in.Workspace, func(tx Tx) error {
		all, err := tx.Comments(statuses...)
		if err != nil {
			return err
		}
		comments := scope.comments(all)
		if in.Status != "" && in.Status != FilterPending {
			out.Count = len(comments)
			return nil
		}
		reviews, err := tx.Reviews(ReviewPending)
		if err != nil {
			return err
		}
		reviews = scope.reviews(reviews, comments)
		grouped := linkedIDs(reviews)
		out.Count = len(reviews)
		for _, c := range comments {
			if !grouped[c.ID] {
				out.Count++
			}
		}
		return nil
	})
	return out, err
}

// Get returns one comment.
func (s *Service) Get(ctx context.Context, in GetInput) (Comment, error) {
	var c Comment
	err := s.view(ctx, in.Workspace, func(tx Tx) (err error) {
		c, err = find(tx, in.ID)
		return err
	})
	return c, err
}

// Evidence returns one comment and, once it is done, the diff between the file
// as reviewed and as resolved.
func (s *Service) Evidence(ctx context.Context, in GetInput) (Evidence, error) {
	c, err := s.Get(ctx, in)
	if err != nil {
		return Evidence{}, err
	}
	ev := Evidence{Comment: c, Diff: []string{}}
	if c.Status == StatusDone {
		ev.Diff, ev.DiffAvailable = snapshotDiff(ctx, c)
	}
	return ev, nil
}

// Sheet returns a submitted review with its linked comments.
func (s *Service) Sheet(ctx context.Context, in GetInput) (ReviewSheet, error) {
	var sheet ReviewSheet
	err := s.view(ctx, in.Workspace, func(tx Tx) error {
		r, ok, err := tx.Review(in.ID)
		if err != nil {
			return err
		}
		if !ok {
			return notFoundf("no submitted review '%s' in this workspace", in.ID)
		}
		sheet, err = sheetOf(tx, r)
		return err
	})
	return sheet, err
}

// Sheets returns the submitted reviews a query scopes to, oldest first, each
// with its linked comments. Nothing moves.
func (s *Service) Sheets(ctx context.Context, in SheetsInput) (SheetsOutput, error) {
	out := SheetsOutput{Workspace: in.Workspace, Sheets: []ReviewSheet{}}
	if _, err := in.Status.Matches(SheetPending); err != nil {
		return out, err
	}
	scope, err := s.scope(in.Workspace, in.File, in.Lane)
	if err != nil {
		return out, err
	}
	err = s.view(ctx, in.Workspace, func(tx Tx) error {
		reviews, err := tx.Reviews()
		if err != nil {
			return err
		}
		// A review's comments move on without it (pulled, then decided), so
		// match it against its comments in every status.
		comments, err := tx.Comments()
		if err != nil {
			return err
		}
		for _, r := range scope.reviews(reviews, scope.comments(comments)) {
			sheet, err := sheetOf(tx, r)
			if err != nil {
				return err
			}
			if ok, _ := in.Status.Matches(sheet.State); ok {
				out.Sheets = append(out.Sheets, sheet)
			}
		}
		return nil
	})
	out.Count = len(out.Sheets)
	return out, err
}

// sheetOf gathers a review's linked comments and derives its state.
func sheetOf(tx Tx, r Review) (ReviewSheet, error) {
	sheet := ReviewSheet{Review: r, Comments: []Comment{}}
	complete := r.Status == ReviewPulled
	for _, id := range r.CommentIDs {
		c, ok, err := tx.Comment(id)
		if err != nil {
			return sheet, err
		}
		if !ok {
			complete = false
			continue
		}
		complete = complete && c.Status.Resolved()
		sheet.Comments = append(sheet.Comments, c)
	}
	switch {
	case complete:
		sheet.State = SheetComplete
	case r.Status == ReviewPulled:
		sheet.State = SheetPulled
	default:
		sheet.State = SheetPending
	}
	return sheet, nil
}

// Workspaces lists every workspace in the store with its pending handoffs.
func (s *Service) Workspaces(ctx context.Context, in WorkspacesInput) (WorkspacesOutput, error) {
	out := WorkspacesOutput{Workspaces: []WorkspaceSummary{}}
	paths, err := s.repo.Workspaces(ctx)
	if err != nil {
		return out, err
	}
	for _, ws := range paths {
		row := WorkspaceSummary{Workspace: ws}
		err := s.repo.View(ctx, ws, func(tx Tx) error {
			reviews, err := tx.Reviews(ReviewPending)
			if err != nil {
				return err
			}
			comments, err := tx.Comments(StatusPending, StatusPulled)
			if err != nil {
				return err
			}
			grouped := linkedIDs(reviews)
			row.Pending = len(reviews)
			for _, c := range comments {
				switch {
				case c.Status == StatusPulled:
					row.Pulled++
				case !grouped[c.ID]:
					row.Pending++
				}
			}
			return nil
		})
		if err != nil {
			return out, err
		}
		if in.All || row.Pending > 0 {
			out.Workspaces = append(out.Workspaces, row)
		}
	}
	return out, nil
}

// ── pull ─────────────────────────────────────────────────────────────────────

// Pull dequeues handoffs: submitted reviews (with every linked comment) and
// standalone comments. Pulled handoffs never come back, unless peeking.
func (s *Service) Pull(ctx context.Context, in PullInput) (PullOutput, error) {
	out := PullOutput{Workspace: in.Workspace, Drained: !in.Peek, Reviews: []Handoff{}, Comments: []Comment{}}
	if in.Limit < 0 {
		return out, Invalidf("limit must be >= 1")
	}
	scope, err := s.scope(in.Workspace, in.File, in.Lane)
	if err != nil {
		return out, err
	}
	err = s.update(ctx, in.Workspace, func(tx Tx) error {
		pendingReviews, err := tx.Reviews(ReviewPending)
		if err != nil {
			return err
		}
		pendingComments, err := tx.Comments(StatusPending)
		if err != nil {
			return err
		}
		reviews := scope.reviews(pendingReviews, scope.comments(pendingComments))
		grouped := linkedIDs(reviews)
		var comments []Comment
		for _, c := range scope.comments(pendingComments) {
			if !grouped[c.ID] {
				comments = append(comments, c)
			}
		}

		if len(in.IDs) > 0 {
			if reviews, comments, err = selectIDs(unique(in.IDs), reviews, comments); err != nil {
				return err
			}
		}
		if in.Limit > 0 {
			reviews, comments = oldest(in.Limit, reviews, comments)
		}

		stamp := s.stamp()
		for _, r := range reviews {
			h := Handoff{Review: r, Comments: []Comment{}}
			for _, id := range r.CommentIDs {
				c, ok, err := tx.Comment(id)
				if err != nil {
					return err
				}
				if !ok {
					continue
				}
				// Resolve refuses a comment of an unpulled review, so this
				// holds; never let a pull overwrite a recorded decision.
				if c.Status != StatusPending {
					return conflictf("%s of %s is already %s", c.ID, r.ID, c.Status)
				}
				if !in.Peek {
					c.Status, c.PulledAt = StatusPulled, &stamp
					if err := tx.UpdateComment(c); err != nil {
						return err
					}
				}
				h.Comments = append(h.Comments, c)
			}
			if !in.Peek {
				h.Status, h.PulledAt = ReviewPulled, &stamp
				if err := tx.UpdateReview(h.Review); err != nil {
					return err
				}
			}
			out.Reviews = append(out.Reviews, h)
		}
		for _, c := range comments {
			if !in.Peek {
				c.Status, c.PulledAt = StatusPulled, &stamp
				if err := tx.UpdateComment(c); err != nil {
					return err
				}
			}
			out.Comments = append(out.Comments, c)
		}

		if out.Count() == 0 && in.Lane != "" {
			out.Elsewhere = elsewhere(pendingReviews, pendingComments, in.Lane)
		}
		return nil
	})
	return out, err
}

// selectIDs narrows the pull to named ids. A linked comment brings its review.
func selectIDs(ids []string, reviews []Review, comments []Comment) ([]Review, []Comment, error) {
	owner := map[string]string{} // review or linked comment id → review id
	for _, r := range reviews {
		owner[r.ID] = r.ID
		for _, id := range r.CommentIDs {
			owner[id] = r.ID
		}
	}
	standalone := map[string]Comment{}
	for _, c := range comments {
		standalone[c.ID] = c
	}
	var missing []string
	wantReview := map[string]bool{}
	var picked []Comment
	for _, id := range ids {
		if rv, ok := owner[id]; ok {
			wantReview[rv] = true
		} else if c, ok := standalone[id]; ok {
			picked = append(picked, c)
		} else {
			missing = append(missing, id)
		}
	}
	if len(missing) > 0 {
		return nil, nil, notFoundf("not pending in this workspace: %s", strings.Join(missing, ", "))
	}
	return slices.DeleteFunc(reviews, func(r Review) bool { return !wantReview[r.ID] }), picked, nil
}

// oldest keeps the first n handoffs by creation time. Ties keep reviews ahead
// of comments, each in creation order.
func oldest(n int, reviews []Review, comments []Comment) ([]Review, []Comment) {
	type unit struct {
		created string
		review  *Review
		comment *Comment
	}
	units := make([]unit, 0, len(reviews)+len(comments))
	for i := range reviews {
		units = append(units, unit{created: reviews[i].CreatedAt, review: &reviews[i]})
	}
	for i := range comments {
		units = append(units, unit{created: comments[i].CreatedAt, comment: &comments[i]})
	}
	slices.SortStableFunc(units, func(a, b unit) int { return cmp.Compare(a.created, b.created) })
	var keptReviews []Review
	var keptComments []Comment
	for _, u := range units[:min(n, len(units))] {
		if u.review != nil {
			keptReviews = append(keptReviews, *u.review)
		} else {
			keptComments = append(keptComments, *u.comment)
		}
	}
	return keptReviews, keptComments
}

// elsewhere reports the handoffs pending outside lane, so a pinned pull that
// took nothing never reads as an empty queue.
func elsewhere(reviews []Review, comments []Comment, lane string) *Elsewhere {
	grouped := linkedIDs(reviews)
	lanes := map[string]bool{}
	count := 0
	note := func(l *string) {
		if l != nil && *l == lane {
			return
		}
		count++
		lanes[laneLabel(l)] = true
	}
	for _, r := range reviews {
		note(r.Lane)
	}
	for _, c := range comments {
		if !grouped[c.ID] {
			note(c.Lane)
		}
	}
	if count == 0 {
		return nil
	}
	names := make([]string, 0, len(lanes))
	for l := range lanes {
		names = append(names, l)
	}
	slices.Sort(names)
	return &Elsewhere{Count: count, Lanes: names}
}

func laneLabel(l *string) string {
	if l == nil {
		return "(no lane)"
	}
	return *l
}

// ── decisions and edits ──────────────────────────────────────────────────────

// Resolve records what became of a comment and who decided. A decision is
// final. Resolving as done keeps the file's current tracked version as a blob.
func (s *Service) Resolve(ctx context.Context, in ResolveInput) (Comment, error) {
	if in.Outcome != OutcomeDone && in.Outcome != OutcomeRejected {
		return Comment{}, Invalidf("outcome %q is not done or rejected", in.Outcome)
	}
	if in.Outcome == OutcomeRejected && strings.TrimSpace(in.Note) == "" {
		return Comment{}, Invalidf("rejecting a comment needs a reason")
	}
	// Git runs before the write transaction, so a slow git never holds the
	// database lock; the checks run again inside it, where they count.
	var c Comment
	err := s.view(ctx, in.Workspace, func(tx Tx) error {
		var err error
		c, err = resolvable(tx, in.ID, in.Outcome)
		return err
	})
	if err != nil {
		return Comment{}, err
	}
	var version string
	if in.Outcome == OutcomeDone {
		if version, err = snapshotResolved(ctx, in.Workspace, c); err != nil {
			return Comment{}, err
		}
	}
	err = s.update(ctx, in.Workspace, func(tx Tx) error {
		var err error
		if c, err = resolvable(tx, in.ID, in.Outcome); err != nil {
			return err
		}
		stamp := s.stamp()
		c.Status = Status(in.Outcome)
		c.ResolvedFileVersion = version
		c.ResolvedAt = &stamp
		c.ResolvedBy = strPtr(in.Author)
		c.ResolutionNote = strPtr(strings.Trim(in.Note, "\n"))
		return tx.UpdateComment(c)
	})
	return c, err
}

// resolvable finds a comment that may take this outcome: not yet decided, not
// waiting in a review nobody has pulled, and pulled before it is done. A
// pending comment may still be rejected: that retracts it.
func resolvable(tx Tx, id string, outcome Outcome) (Comment, error) {
	c, err := find(tx, id)
	if err != nil {
		return c, err
	}
	if c.ReviewID != "" {
		r, ok, err := tx.Review(c.ReviewID)
		if err != nil {
			return c, err
		}
		if ok && r.Status == ReviewPending {
			return c, conflictf("%s belongs to %s — pull %s first", c.ID, r.ID, r.ID)
		}
	}
	if c.Status.Resolved() {
		return c, conflictf("%s is already %s — it cannot be re-decided", c.ID, c.Status)
	}
	if outcome == OutcomeDone && c.Status != StatusPulled {
		return c, conflictf("%s is %s — pull it before resolving it as done", c.ID, c.Status)
	}
	return c, nil
}

// snapshotResolved keeps the current version of a comment's file, when it is
// still tracked at the recorded path inside the workspace.
func snapshotResolved(ctx context.Context, ws string, c Comment) (string, error) {
	recorded := cmp.Or(c.Path, c.File)
	if recorded == "" {
		return "", nil
	}
	if !filepath.IsAbs(recorded) {
		recorded = filepath.Join(ws, recorded)
	}
	path, err := workspace.Canonical(recorded)
	if err != nil {
		return "", nil //nolint:nilerr // a path that does not resolve has no snapshot
	}
	rel, err := filepath.Rel(ws, path)
	if err != nil || rel == ".." || strings.HasPrefix(rel, "../") || !isFile(path) {
		return "", nil //nolint:nilerr // a path outside the workspace has no snapshot
	}
	label := cmp.Or(c.File, rel)
	tracked, err := gitx.Tracked(ctx, ws, rel)
	if err != nil {
		return "", internalf("cannot inspect Git metadata for %s: %s", label, gitx.Stderr(err))
	}
	if !tracked {
		return "", nil
	}
	id, err := gitx.StoreFile(ctx, ws, rel)
	if err != nil {
		if !isFile(path) {
			return "", nil
		}
		return "", internalf("cannot store the resolved version of %s: %s", label, gitx.Stderr(err))
	}
	return id, nil
}

// Edit replaces a pending comment's text. Where it points and what it
// snapshotted stay.
func (s *Service) Edit(ctx context.Context, in EditInput) (Comment, error) {
	if strings.TrimSpace(in.Comment) == "" {
		return Comment{}, Invalidf("refusing to replace a comment with empty text")
	}
	var c Comment
	err := s.update(ctx, in.Workspace, func(tx Tx) error {
		var err error
		if c, err = find(tx, in.ID); err != nil {
			return err
		}
		// Once pulled, the text is what was asked; rewriting it would falsify
		// the record the decision answers.
		if c.Status != StatusPending {
			return conflictf("%s is %s — only a pending comment can be edited", c.ID, c.Status)
		}
		c.Comment = strings.Trim(in.Comment, "\n")
		c.EditedAt = s.stamp()
		return tx.UpdateComment(c)
	})
	return c, err
}

// ── scoping ──────────────────────────────────────────────────────────────────

// scope is what a lane-pinned, file-scoped request may see. Lane scoping is
// strict: a pinned request never sees an unlaned or another lane's comment.
type scope struct {
	path string // canonical file path, "" for every file
	lane string // "" for every lane
}

func (s *Service) scope(ws, file, lane string) (scope, error) {
	sc := scope{lane: lane}
	if file != "" {
		p, err := s.filePath(ws, file)
		if err != nil {
			return sc, err
		}
		sc.path = p
	}
	return sc, nil
}

func (sc scope) inLane(l *string) bool { return sc.lane == "" || (l != nil && *l == sc.lane) }

func (sc scope) comments(cs []Comment) []Comment {
	kept := []Comment{}
	for _, c := range cs {
		if (sc.path == "" || c.Path == sc.path) && sc.inLane(c.Lane) {
			kept = append(kept, c)
		}
	}
	return kept
}

// reviews keeps the reviews in the lane and, when scoped to a file, those
// linking at least one of the in-scope comments.
func (sc scope) reviews(rs []Review, inScope []Comment) []Review {
	matching := map[string]bool{}
	for _, c := range inScope {
		matching[c.ID] = true
	}
	kept := []Review{}
	for _, r := range rs {
		if !sc.inLane(r.Lane) {
			continue
		}
		if sc.path != "" && !slices.ContainsFunc(r.CommentIDs, func(id string) bool { return matching[id] }) {
			continue
		}
		kept = append(kept, r)
	}
	return kept
}

// ── helpers ──────────────────────────────────────────────────────────────────

func find(tx Tx, id string) (Comment, error) {
	c, ok, err := tx.Comment(id)
	if err != nil {
		return c, err
	}
	if !ok {
		return c, notFoundf("no review '%s' in this workspace", id)
	}
	return c, nil
}

// filePath canonicalizes a file named absolutely or relative to the workspace.
func (s *Service) filePath(ws, file string) (string, error) {
	if !filepath.IsAbs(file) {
		file = filepath.Join(ws, file)
	}
	return workspace.Canonical(file)
}

// inWorkspace is path relative to the workspace ws, if path lies inside it.
func inWorkspace(ws, path string) (string, bool) {
	rel, err := filepath.Rel(ws, path)
	if err != nil || !filepath.IsLocal(rel) {
		return "", false
	}
	return rel, true
}

func linkedIDs(reviews []Review) map[string]bool {
	ids := map[string]bool{}
	for _, r := range reviews {
		for _, id := range r.CommentIDs {
			ids[id] = true
		}
	}
	return ids
}

// splitSource splits file content into lines; a trailing newline is not a line.
func splitSource(text string) []string {
	lines := strings.Split(text, "\n")
	if lines[len(lines)-1] == "" {
		lines = lines[:len(lines)-1]
	}
	return lines
}

func (r LineRange) label() string {
	if r.Start == r.End {
		return strconv.Itoa(r.Start)
	}
	return fmt.Sprintf("%d-%d", r.Start, r.End)
}

func sameOpt(a, b *string) bool {
	if a == nil || b == nil {
		return a == b
	}
	return *a == *b
}

func unique(ids []string) []string {
	seen := map[string]bool{}
	out := []string{}
	for _, id := range ids {
		if !seen[id] {
			seen[id] = true
			out = append(out, id)
		}
	}
	return out
}

func isFile(path string) bool {
	info, err := os.Stat(path)
	return err == nil && info.Mode().IsRegular()
}
