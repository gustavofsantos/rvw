package store

import (
	"context"
	"fmt"
	"path/filepath"
	"sync"
	"testing"

	"github.com/gustavofsantos/rvw/internal/review"
)

const ws = "/work/proj"

func open(t *testing.T, path string) *Store {
	t.Helper()
	s, err := Open(context.Background(), path)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { s.Close() })
	return s
}

func comment(seq int64, lane string) review.Comment {
	c := review.Comment{
		ID: review.CommentID(seq), Status: review.StatusPending, File: "a.go", Path: ws + "/a.go",
		StartLine: 1, EndLine: 2, Code: "x", Comment: "note", CreatedAt: "2026-01-01T00:00:00+00:00",
	}
	if lane != "" {
		c.Lane = &lane
	}
	return c
}

func TestRoundTripKeepsNullsLinksAndOrder(t *testing.T) {
	s := open(t, filepath.Join(t.TempDir(), "rvw.db"))
	ctx := context.Background()

	err := s.Update(ctx, ws, func(tx review.Tx) error {
		for range 3 {
			seq, err := tx.NextCommentSeq()
			if err != nil {
				return err
			}
			if err := tx.InsertComment(comment(seq, "auth")); err != nil {
				return err
			}
		}
		seq, err := tx.NextReviewSeq()
		if err != nil {
			return err
		}
		return tx.InsertReview(review.Review{
			ID: review.ReviewID(seq), Status: review.ReviewPending, Decision: review.DecisionComment,
			Summary: "s", CommentIDs: []string{"r3", "r1"}, CreatedAt: "2026-01-01T00:00:00+00:00",
		})
	})
	if err != nil {
		t.Fatal(err)
	}

	err = s.View(ctx, ws, func(tx review.Tx) error {
		cs, err := tx.Comments(review.StatusPending)
		if err != nil {
			return err
		}
		if len(cs) != 3 || cs[0].ID != "r1" || cs[2].ID != "r3" {
			t.Fatalf("comments out of order: %+v", cs)
		}
		if cs[0].ReviewID != "rv1" || cs[1].ReviewID != "" {
			t.Errorf("review links not derived: %q %q", cs[0].ReviewID, cs[1].ReviewID)
		}
		if cs[0].Author != nil || cs[0].PulledAt != nil || *cs[0].Lane != "auth" || cs[0].Workspace != ws {
			t.Errorf("nullable fields not round-tripped: %+v", cs[0])
		}
		rv, ok, err := tx.Review("rv1")
		if err != nil || !ok {
			t.Fatalf("review rv1: ok=%v err=%v", ok, err)
		}
		if fmt.Sprint(rv.CommentIDs) != "[r3 r1]" {
			t.Errorf("links lost their order: %v", rv.CommentIDs)
		}
		return nil
	})
	if err != nil {
		t.Fatal(err)
	}
}

func TestUnknownWorkspaceReadsEmptyWithoutBeingCreated(t *testing.T) {
	s := open(t, filepath.Join(t.TempDir(), "rvw.db"))
	ctx := context.Background()
	err := s.View(ctx, "/nowhere", func(tx review.Tx) error {
		cs, err := tx.Comments()
		if len(cs) != 0 {
			t.Errorf("expected nothing, got %v", cs)
		}
		return err
	})
	if err != nil {
		t.Fatal(err)
	}
	if paths, _ := s.Workspaces(ctx); len(paths) != 0 {
		t.Errorf("a read created a workspace: %v", paths)
	}
}

// Separate handles on a fresh file stand in for concurrent processes: they race
// on creating the schema and on the id sequence.
func TestConcurrentOpenersShareOneSchemaAndSequence(t *testing.T) {
	path := filepath.Join(t.TempDir(), "rvw.db")
	const n = 8
	ids := make(chan string, n)
	var wg sync.WaitGroup
	for range n {
		wg.Go(func() {
			s, err := Open(context.Background(), path)
			if err != nil {
				t.Error(err)
				return
			}
			defer s.Close()
			err = s.Update(context.Background(), ws, func(tx review.Tx) error {
				seq, err := tx.NextCommentSeq()
				if err != nil {
					return err
				}
				ids <- review.CommentID(seq)
				return tx.InsertComment(comment(seq, ""))
			})
			if err != nil {
				t.Error(err)
			}
		})
	}
	wg.Wait()
	close(ids)
	seen := map[string]bool{}
	for id := range ids {
		if seen[id] {
			t.Errorf("duplicate id %s", id)
		}
		seen[id] = true
	}
	if len(seen) != n {
		t.Errorf("expected %d ids, got %d", n, len(seen))
	}
}
