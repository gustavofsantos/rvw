// Package store keeps rvw's queue in one local SQLite database, shared by every
// workspace on the machine.
package store

import (
	"context"
	"database/sql"
	_ "embed"
	"errors"
	"fmt"
	"net/url"
	"os"
	"path/filepath"
	"strconv"
	"strings"

	"github.com/gustavofsantos/rvw/internal/review"
	_ "modernc.org/sqlite"
)

//go:embed schema.sql
var schemaV1 string

const schemaVersion = 1

// DefaultPath is where the database lives: $XDG_DATA_HOME/rvw/rvw.db, else
// ~/.local/share/rvw/rvw.db. The XDG base directory spec applies on macOS as
// well; a relative $XDG_DATA_HOME is ignored, as the spec requires.
func DefaultPath() (string, error) {
	data := os.Getenv("XDG_DATA_HOME")
	if data == "" || !filepath.IsAbs(data) {
		home, err := os.UserHomeDir()
		if err != nil {
			return "", fmt.Errorf("cannot locate the data directory: %w", err)
		}
		data = filepath.Join(home, ".local", "share")
	}
	return filepath.Join(data, "rvw", "rvw.db"), nil
}

// Store is a [review.Repository] backed by SQLite.
type Store struct {
	db   *sql.DB
	path string
}

var _ review.Repository = (*Store)(nil)

// Open opens (creating when needed) the database at path and brings its schema
// up to date. Every transaction takes the write lock up front, so concurrent
// processes queue behind each other instead of failing on upgrade. The journal
// stays in its default rollback mode: switching a fresh file to WAL does not wait
// on the busy handler, so concurrent first opens would fail with SQLITE_BUSY.
func Open(ctx context.Context, path string) (*Store, error) {
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return nil, fmt.Errorf("cannot create %s: %w", filepath.Dir(path), err)
	}
	q := url.Values{}
	q.Add("_pragma", "busy_timeout(10000)")
	q.Add("_pragma", "foreign_keys(1)")
	q.Set("_txlock", "immediate")
	db, err := sql.Open("sqlite", "file:"+path+"?"+q.Encode())
	if err != nil {
		return nil, fmt.Errorf("cannot open %s: %w", path, err)
	}
	s := &Store{db: db, path: path}
	if err := s.migrate(ctx); err != nil {
		db.Close()
		return nil, fmt.Errorf("cannot prepare %s: %w", path, err)
	}
	return s, nil
}

// Close releases the database.
func (s *Store) Close() error { return s.db.Close() }

// Location is the database file.
func (s *Store) Location() string { return s.path }

func (s *Store) migrate(ctx context.Context) error {
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return err
	}
	defer tx.Rollback()
	var version int
	if err := tx.QueryRowContext(ctx, "PRAGMA user_version").Scan(&version); err != nil {
		return err
	}
	switch {
	case version == schemaVersion:
		return nil
	case version > schemaVersion:
		return fmt.Errorf("schema version %d is newer than this rvw understands (%d)", version, schemaVersion)
	}
	if _, err := tx.ExecContext(ctx, schemaV1); err != nil {
		return err
	}
	if _, err := tx.ExecContext(ctx, "PRAGMA user_version = "+strconv.Itoa(schemaVersion)); err != nil {
		return err
	}
	return tx.Commit()
}

// Update implements [review.Repository].
func (s *Store) Update(ctx context.Context, workspace string, fn func(review.Tx) error) error {
	return s.run(ctx, workspace, true, fn)
}

// View implements [review.Repository].
func (s *Store) View(ctx context.Context, workspace string, fn func(review.Tx) error) error {
	return s.run(ctx, workspace, false, fn)
}

func (s *Store) run(ctx context.Context, workspace string, create bool, fn func(review.Tx) error) error {
	sqlTx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return err
	}
	defer sqlTx.Rollback()

	t := &tx{ctx: ctx, tx: sqlTx, workspace: workspace}
	err = sqlTx.QueryRowContext(ctx, "SELECT id FROM workspaces WHERE path = ?", workspace).Scan(&t.wsID)
	switch {
	case errors.Is(err, sql.ErrNoRows) && create:
		res, err := sqlTx.ExecContext(ctx, "INSERT INTO workspaces (path) VALUES (?)", workspace)
		if err != nil {
			return err
		}
		if t.wsID, err = res.LastInsertId(); err != nil {
			return err
		}
	case errors.Is(err, sql.ErrNoRows):
		// an unknown workspace reads as empty: no row has workspace_id 0
	case err != nil:
		return err
	}

	if err := fn(t); err != nil {
		return err
	}
	return sqlTx.Commit()
}

// Workspaces implements [review.Repository].
func (s *Store) Workspaces(ctx context.Context) ([]string, error) {
	rows, err := s.db.QueryContext(ctx, "SELECT path FROM workspaces ORDER BY path")
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var paths []string
	for rows.Next() {
		var p string
		if err := rows.Scan(&p); err != nil {
			return nil, err
		}
		paths = append(paths, p)
	}
	return paths, rows.Err()
}

// ── transaction ──────────────────────────────────────────────────────────────

type tx struct {
	ctx       context.Context
	tx        *sql.Tx
	workspace string
	wsID      int64
}

func (t *tx) exec(query string, args ...any) error {
	_, err := t.tx.ExecContext(t.ctx, query, args...)
	return err
}

func (t *tx) nextSeq(column string) (int64, error) {
	var seq int64
	err := t.tx.QueryRowContext(t.ctx,
		"UPDATE workspaces SET "+column+" = "+column+" + 1 WHERE id = ? RETURNING "+column+" - 1",
		t.wsID).Scan(&seq)
	return seq, err
}

func (t *tx) NextCommentSeq() (int64, error) { return t.nextSeq("next_comment_seq") }
func (t *tx) NextReviewSeq() (int64, error)  { return t.nextSeq("next_review_seq") }

const commentColumns = `c.seq, c.status, c.lane, c.author, c.file, c.path, c.start_line, c.end_line,
	c.filetype, c.code, c.comment, c.created_at, c.pulled_at, c.resolved_at, c.resolved_by,
	c.resolution_note, c.file_version, c.resolved_file_version, c.edited_at, l.review_seq`

const commentFrom = ` FROM comments c
	LEFT JOIN review_comments l ON l.workspace_id = c.workspace_id AND l.comment_seq = c.seq
	WHERE c.workspace_id = ?`

func (t *tx) Comments(statuses ...review.Status) ([]review.Comment, error) {
	query := "SELECT " + commentColumns + commentFrom
	args := []any{t.wsID}
	if len(statuses) > 0 {
		query += " AND c.status IN (" + placeholders(len(statuses)) + ")"
		for _, s := range statuses {
			args = append(args, string(s))
		}
	}
	rows, err := t.tx.QueryContext(t.ctx, query+" ORDER BY c.seq", args...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	comments := []review.Comment{}
	for rows.Next() {
		c, err := t.scanComment(rows)
		if err != nil {
			return nil, err
		}
		comments = append(comments, c)
	}
	return comments, rows.Err()
}

func (t *tx) Comment(id string) (review.Comment, bool, error) {
	seq, ok := parseSeq(id, "r")
	if !ok {
		return review.Comment{}, false, nil
	}
	row := t.tx.QueryRowContext(t.ctx, "SELECT "+commentColumns+commentFrom+" AND c.seq = ?", t.wsID, seq)
	c, err := t.scanComment(row)
	if errors.Is(err, sql.ErrNoRows) {
		return review.Comment{}, false, nil
	}
	return c, err == nil, err
}

type scanner interface{ Scan(dest ...any) error }

func (t *tx) scanComment(row scanner) (review.Comment, error) {
	var (
		c                                                  review.Comment
		seq                                                int64
		status                                             string
		lane, author, pulled, resolvedAt, resolvedBy, note sql.NullString
		fileVersion, resolvedFileVersion, edited           sql.NullString
		reviewSeq                                          sql.NullInt64
	)
	err := row.Scan(&seq, &status, &lane, &author, &c.File, &c.Path, &c.StartLine, &c.EndLine,
		&c.Filetype, &c.Code, &c.Comment, &c.CreatedAt, &pulled, &resolvedAt, &resolvedBy,
		&note, &fileVersion, &resolvedFileVersion, &edited, &reviewSeq)
	if err != nil {
		return c, err
	}
	c.ID = review.CommentID(seq)
	c.Status = review.Status(status)
	c.Workspace = t.workspace
	c.Lane, c.Author = nullable(lane), nullable(author)
	c.PulledAt, c.ResolvedAt, c.ResolvedBy, c.ResolutionNote = nullable(pulled), nullable(resolvedAt), nullable(resolvedBy), nullable(note)
	c.FileVersion, c.ResolvedFileVersion, c.EditedAt = fileVersion.String, resolvedFileVersion.String, edited.String
	if reviewSeq.Valid {
		c.ReviewID = review.ReviewID(reviewSeq.Int64)
	}
	return c, nil
}

func (t *tx) InsertComment(c review.Comment) error {
	seq, ok := parseSeq(c.ID, "r")
	if !ok {
		return fmt.Errorf("malformed comment id %q", c.ID)
	}
	return t.exec(`INSERT INTO comments (workspace_id, seq, status, lane, author, file, path,
		start_line, end_line, filetype, code, comment, created_at, pulled_at, resolved_at,
		resolved_by, resolution_note, file_version, resolved_file_version, edited_at)
		VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
		t.wsID, seq, string(c.Status), c.Lane, c.Author, c.File, c.Path,
		c.StartLine, c.EndLine, c.Filetype, c.Code, c.Comment, c.CreatedAt, c.PulledAt, c.ResolvedAt,
		c.ResolvedBy, c.ResolutionNote, emptyNull(c.FileVersion), emptyNull(c.ResolvedFileVersion), emptyNull(c.EditedAt))
}

// UpdateComment rewrites the mutable part of a comment: its lifecycle and text.
// Where it points and what it snapshotted never change.
func (t *tx) UpdateComment(c review.Comment) error {
	seq, ok := parseSeq(c.ID, "r")
	if !ok {
		return fmt.Errorf("malformed comment id %q", c.ID)
	}
	return t.exec(`UPDATE comments SET status = ?, comment = ?, pulled_at = ?, resolved_at = ?,
		resolved_by = ?, resolution_note = ?, resolved_file_version = ?, edited_at = ?
		WHERE workspace_id = ? AND seq = ?`,
		string(c.Status), c.Comment, c.PulledAt, c.ResolvedAt,
		c.ResolvedBy, c.ResolutionNote, emptyNull(c.ResolvedFileVersion), emptyNull(c.EditedAt),
		t.wsID, seq)
}

const reviewColumns = "seq, status, lane, author, decision, summary, created_at, pulled_at"

func (t *tx) Reviews(statuses ...review.ReviewStatus) ([]review.Review, error) {
	query := "SELECT " + reviewColumns + " FROM reviews WHERE workspace_id = ?"
	args := []any{t.wsID}
	if len(statuses) > 0 {
		query += " AND status IN (" + placeholders(len(statuses)) + ")"
		for _, s := range statuses {
			args = append(args, string(s))
		}
	}
	rows, err := t.tx.QueryContext(t.ctx, query+" ORDER BY seq", args...)
	if err != nil {
		return nil, err
	}
	reviews := []review.Review{}
	for rows.Next() {
		r, err := t.scanReview(rows)
		if err != nil {
			rows.Close()
			return nil, err
		}
		reviews = append(reviews, r)
	}
	rows.Close()
	if err := rows.Err(); err != nil {
		return nil, err
	}
	for i := range reviews {
		if reviews[i].CommentIDs, err = t.links(reviews[i].ID); err != nil {
			return nil, err
		}
	}
	return reviews, nil
}

func (t *tx) Review(id string) (review.Review, bool, error) {
	seq, ok := parseSeq(id, "rv")
	if !ok {
		return review.Review{}, false, nil
	}
	row := t.tx.QueryRowContext(t.ctx, "SELECT "+reviewColumns+" FROM reviews WHERE workspace_id = ? AND seq = ?", t.wsID, seq)
	r, err := t.scanReview(row)
	if errors.Is(err, sql.ErrNoRows) {
		return review.Review{}, false, nil
	}
	if err != nil {
		return r, false, err
	}
	r.CommentIDs, err = t.links(r.ID)
	return r, err == nil, err
}

func (t *tx) scanReview(row scanner) (review.Review, error) {
	var (
		r                    review.Review
		seq                  int64
		status, decision     string
		lane, author, pulled sql.NullString
	)
	if err := row.Scan(&seq, &status, &lane, &author, &decision, &r.Summary, &r.CreatedAt, &pulled); err != nil {
		return r, err
	}
	r.ID = review.ReviewID(seq)
	r.Status = review.ReviewStatus(status)
	r.Workspace = t.workspace
	r.Lane, r.Author, r.PulledAt = nullable(lane), nullable(author), nullable(pulled)
	r.Decision = review.Decision(decision)
	return r, nil
}

func (t *tx) links(reviewID string) ([]string, error) {
	seq, _ := parseSeq(reviewID, "rv")
	rows, err := t.tx.QueryContext(t.ctx,
		"SELECT comment_seq FROM review_comments WHERE workspace_id = ? AND review_seq = ? ORDER BY position",
		t.wsID, seq)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	ids := []string{}
	for rows.Next() {
		var c int64
		if err := rows.Scan(&c); err != nil {
			return nil, err
		}
		ids = append(ids, review.CommentID(c))
	}
	return ids, rows.Err()
}

func (t *tx) InsertReview(r review.Review) error {
	seq, ok := parseSeq(r.ID, "rv")
	if !ok {
		return fmt.Errorf("malformed review id %q", r.ID)
	}
	err := t.exec(`INSERT INTO reviews (workspace_id, seq, status, lane, author, decision, summary, created_at, pulled_at)
		VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)`,
		t.wsID, seq, string(r.Status), r.Lane, r.Author, string(r.Decision), r.Summary, r.CreatedAt, r.PulledAt)
	if err != nil {
		return err
	}
	for pos, id := range r.CommentIDs {
		cseq, ok := parseSeq(id, "r")
		if !ok {
			return fmt.Errorf("malformed comment id %q", id)
		}
		if err := t.exec("INSERT INTO review_comments (workspace_id, review_seq, position, comment_seq) VALUES (?, ?, ?, ?)",
			t.wsID, seq, pos, cseq); err != nil {
			return err
		}
	}
	return nil
}

// UpdateReview rewrites a review's lifecycle. Its links never change.
func (t *tx) UpdateReview(r review.Review) error {
	seq, ok := parseSeq(r.ID, "rv")
	if !ok {
		return fmt.Errorf("malformed review id %q", r.ID)
	}
	return t.exec("UPDATE reviews SET status = ?, pulled_at = ? WHERE workspace_id = ? AND seq = ?",
		string(r.Status), r.PulledAt, t.wsID, seq)
}

// ── helpers ──────────────────────────────────────────────────────────────────

// parseSeq reads the number out of an id like r12 or rv3; the prefix must match
// exactly, so r12 is not a review and rv3 is not a comment.
func parseSeq(id, prefix string) (int64, bool) {
	digits, ok := strings.CutPrefix(id, prefix)
	if !ok || digits == "" || strings.Trim(digits, "0123456789") != "" {
		return 0, false
	}
	seq, err := strconv.ParseInt(digits, 10, 64)
	return seq, err == nil
}

func placeholders(n int) string { return strings.TrimSuffix(strings.Repeat("?, ", n), ", ") }

func nullable(s sql.NullString) *string {
	if !s.Valid {
		return nil
	}
	return &s.String
}

func emptyNull(s string) any {
	if s == "" {
		return nil
	}
	return s
}
