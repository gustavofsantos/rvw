// Package gitx runs the handful of git plumbing commands rvw needs: finding a
// worktree root, and keeping exact file versions as blobs in the object store.
// Every call is `git -C <dir> <subcommand> ...`, with git taken from $PATH.
package gitx

import (
	"bytes"
	"errors"
	"fmt"
	"os/exec"
	"strings"
)

// Error is a git command that ran and failed; Stderr is its trimmed message.
type Error struct {
	Args   []string
	Stderr string
}

func (e *Error) Error() string {
	return fmt.Sprintf("git %s: %s", strings.Join(e.Args, " "), e.Stderr)
}

// Stderr returns what git said when err is a failed git command, else err's text.
func Stderr(err error) string {
	var gerr *Error
	if errors.As(err, &gerr) {
		return gerr.Stderr
	}
	return err.Error()
}

func run(dir, stdin string, args ...string) (string, int, error) {
	full := append([]string{"-C", dir}, args...)
	cmd := exec.Command("git", full...)
	if stdin != "" {
		cmd.Stdin = strings.NewReader(stdin)
	}
	var out, errOut bytes.Buffer
	cmd.Stdout, cmd.Stderr = &out, &errOut
	err := cmd.Run()
	var exit *exec.ExitError
	switch {
	case errors.As(err, &exit):
		return out.String(), exit.ExitCode(), &Error{Args: args, Stderr: strings.TrimSpace(errOut.String())}
	case err != nil:
		return "", -1, err
	}
	return out.String(), 0, nil
}

// Toplevel is the root of the worktree holding dir, if dir is inside one.
func Toplevel(dir string) (string, bool) {
	out, _, err := run(dir, "", "rev-parse", "--show-toplevel")
	top := strings.TrimSpace(out)
	return top, err == nil && top != ""
}

// Tracked reports whether rel is in the index of the repository at dir. A git
// failure other than "not tracked" is returned as an error.
func Tracked(dir, rel string) (bool, error) {
	_, code, err := run(dir, "", "ls-files", "--error-unmatch", "--", rel)
	switch code {
	case 0:
		return true, nil
	case 1:
		return false, nil
	}
	return false, err
}

// StoreContent writes content to the object store and returns its blob id.
func StoreContent(dir, content string) (string, error) {
	out, _, err := run(dir, content, "hash-object", "-w", "--stdin")
	return blobID(out, err)
}

// StoreFile writes the file at rel (relative to dir) to the object store.
func StoreFile(dir, rel string) (string, error) {
	out, _, err := run(dir, "", "hash-object", "-w", "--", rel)
	return blobID(out, err)
}

func blobID(out string, err error) (string, error) {
	id := strings.TrimSpace(out)
	if err == nil && id == "" {
		err = &Error{Args: []string{"hash-object"}, Stderr: "no object id returned"}
	}
	return id, err
}

// Blob reads a blob back, if the repository at dir still has it.
func Blob(dir, id string) (string, bool) {
	out, _, err := run(dir, "", "cat-file", "blob", id)
	return out, err == nil
}

// Files lists the files git sees in the worktree at dir, relative to dir and
// slash-separated: tracked files plus untracked ones that are not ignored by
// .gitignore, .git/info/exclude or the global excludes file. A tracked file
// deleted from disk is still listed; callers that need it on disk must check.
func Files(dir string) ([]string, error) {
	out, _, err := run(dir, "", "ls-files", "-z", "--cached", "--others", "--exclude-standard")
	if err != nil {
		return nil, err
	}
	var files []string
	for f := range strings.SplitSeq(out, "\x00") {
		if f != "" {
			files = append(files, f)
		}
	}
	return files, nil
}
