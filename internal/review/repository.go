package review

import "context"

// Repository is the storage the service runs on. Every read-modify-write goes
// through one Update so concurrent editors and agents serialize on it.
type Repository interface {
	// Update runs fn in one write transaction on the workspace, creating the
	// workspace record when it does not exist yet. fn's error rolls back.
	Update(ctx context.Context, workspace string, fn func(Tx) error) error
	// View runs fn in one transaction on the workspace without creating it; an
	// unknown workspace reads as empty.
	View(ctx context.Context, workspace string, fn func(Tx) error) error
	// Workspaces lists every workspace ever recorded, by path.
	Workspaces(ctx context.Context) ([]string, error)
	// Location names where the data lives, for humans.
	Location() string
}

// Tx is the per-workspace view of the store inside one transaction. Comments
// and reviews come back in creation order, with ReviewID / CommentIDs filled in.
type Tx interface {
	NextCommentSeq() (int64, error)
	NextReviewSeq() (int64, error)

	Comments(statuses ...Status) ([]Comment, error)
	Comment(id string) (Comment, bool, error)
	InsertComment(Comment) error
	UpdateComment(Comment) error

	Reviews(statuses ...ReviewStatus) ([]Review, error)
	Review(id string) (Review, bool, error)
	// InsertReview stores the review and links its CommentIDs in order.
	InsertReview(Review) error
	UpdateReview(Review) error
}
