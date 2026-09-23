// Package workspace decides which queue a command acts on: the git worktree
// root holding a directory, else the directory itself. Each worktree is its own
// workspace.
package workspace

import (
	"errors"
	"io/fs"
	"os"
	"path/filepath"

	"github.com/gustavofsantos/rvw/internal/gitx"
)

// Resolve returns the canonical workspace for dir: its worktree root when it is
// inside one, else dir itself, with symlinks resolved.
func Resolve(dir string) (string, error) {
	base, err := Canonical(dir)
	if err != nil {
		return "", err
	}
	if top, ok := gitx.Toplevel(base); ok {
		return Canonical(top)
	}
	return base, nil
}

// IsDir reports whether path names an existing directory.
func IsDir(path string) bool {
	info, err := os.Stat(path)
	return err == nil && info.IsDir()
}

// Canonical makes path absolute and resolves symlinks, keeping any trailing
// part that does not exist yet as written, so two spellings of one file compare
// equal whether or not it is still on disk.
func Canonical(path string) (string, error) {
	abs, err := filepath.Abs(path)
	if err != nil {
		return "", err
	}
	var missing []string
	for cur := abs; ; {
		real, err := filepath.EvalSymlinks(cur)
		if err == nil {
			for i := len(missing) - 1; i >= 0; i-- {
				real = filepath.Join(real, missing[i])
			}
			return real, nil
		}
		parent := filepath.Dir(cur)
		if !errors.Is(err, fs.ErrNotExist) || parent == cur {
			return abs, nil
		}
		missing = append(missing, filepath.Base(cur))
		cur = parent
	}
}
