// Package workspace decides which queue a command acts on: the git worktree
// root holding a directory. Each worktree is its own workspace; a directory
// outside git has none.
package workspace

import (
	"context"
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"slices"
	"syscall"

	"github.com/gustavofsantos/rvw/internal/gitx"
)

// Resolve returns the canonical workspace for dir: the root of the worktree
// holding it, with symlinks resolved. A dir outside git is an error.
func Resolve(ctx context.Context, dir string) (string, error) {
	base, err := Canonical(dir)
	if err != nil {
		return "", err
	}
	top, ok := gitx.Toplevel(ctx, base)
	if !ok {
		return "", fmt.Errorf("%s is not inside a git repository", base)
	}
	return Canonical(top)
}

// IsDir reports whether path names an existing directory.
func IsDir(path string) bool {
	info, err := os.Stat(path)
	return err == nil && info.IsDir()
}

// Canonical makes path absolute and resolves symlinks, keeping any trailing
// part that does not exist yet as written, so two spellings of one file compare
// equal whether or not it is still on disk. Any other failure, such as a
// directory it may not search, is an error: guessing would record one file
// under two paths.
func Canonical(path string) (string, error) {
	abs, err := filepath.Abs(path)
	if err != nil {
		return "", err
	}
	var missing []string
	for cur := abs; ; {
		resolved, err := filepath.EvalSymlinks(cur)
		if err == nil {
			for _, m := range slices.Backward(missing) {
				resolved = filepath.Join(resolved, m)
			}
			return resolved, nil
		}
		if !errors.Is(err, fs.ErrNotExist) && !errors.Is(err, syscall.ENOTDIR) {
			return "", fmt.Errorf("cannot resolve %s: %w", abs, err)
		}
		parent := filepath.Dir(cur)
		if parent == cur {
			return abs, nil
		}
		missing = append(missing, filepath.Base(cur))
		cur = parent
	}
}
