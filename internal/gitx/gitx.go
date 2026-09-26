// Package gitx runs the handful of git plumbing commands rvw needs: finding a
// worktree root, and keeping exact file versions as blobs in the object store.
// Every call is `git -C <dir> <subcommand> ...`, with git taken from $PATH.
package gitx

import (
	"bytes"
	"errors"
	"fmt"
	"os/exec"
	"strconv"
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

// Hunk is one hunk header of a zero-context diff: the old file's lines
// OldStart..OldStart+OldLines-1 became the new file's NewStart..NewStart+NewLines-1.
// A count of 0 puts its start just before the hunk: a pure deletion lies after
// new line NewStart, 0 being the top of the file.
type Hunk struct {
	OldStart, OldLines int
	NewStart, NewLines int
}

// Changes diffs the file at path, inside the worktree at dir, against the
// commit base: committed, staged and unstaged edits together. untracked is
// set, with no hunks, for a file git does not track and does not ignore. A
// binary file has no hunks.
func Changes(dir, path, base string) (hunks []Hunk, untracked bool, err error) {
	others, _, err := run(dir, "", "--literal-pathspecs", "ls-files", "--others", "--exclude-standard", "--", path)
	if err != nil {
		return nil, false, err
	}
	if strings.TrimSpace(others) != "" {
		return nil, true, nil
	}
	out, _, err := run(dir, "", "--literal-pathspecs", "diff", "--no-color", "--no-ext-diff", "--no-textconv", "-U0", base, "--", path)
	if err != nil {
		return nil, false, err
	}
	return parseHunks(out), false, nil
}

// parseHunks reads the "@@ -a,b +c,d @@" headers of a unified diff; a count
// left out is 1.
func parseHunks(diff string) []Hunk {
	var hunks []Hunk
	for line := range strings.SplitSeq(diff, "\n") {
		rest, ok := strings.CutPrefix(line, "@@ -")
		if !ok {
			continue
		}
		old, rest, _ := strings.Cut(rest, " +")
		nu, _, _ := strings.Cut(rest, " @@")
		var h Hunk
		h.OldStart, h.OldLines = span(old)
		h.NewStart, h.NewLines = span(nu)
		hunks = append(hunks, h)
	}
	return hunks
}

func span(s string) (start, count int) {
	a, b, found := strings.Cut(s, ",")
	start, _ = strconv.Atoi(a)
	count = 1
	if found {
		count, _ = strconv.Atoi(b)
	}
	return start, count
}

// Change is a file that differs between a commit and the worktree. Status is
// git's letter: A added, M modified, D deleted, T type changed, U unmerged,
// and ? for an untracked file.
type Change struct {
	Path   string
	Status byte
}

// ChangedFiles lists what differs between the commit base and the worktree
// at dir, committed or not, plus the untracked files that are not ignored;
// paths are relative to dir and slash-separated, in git's order, untracked
// last. A rename shows as a deletion and an addition.
func ChangedFiles(dir, base string) ([]Change, error) {
	diff, _, err := run(dir, "", "diff", "--name-status", "-z", "--no-renames", "--no-ext-diff", base, "--")
	if err != nil {
		return nil, err
	}
	var out []Change
	fields := strings.Split(diff, "\x00")
	for i := 0; i+1 < len(fields); i += 2 {
		if fields[i] != "" {
			out = append(out, Change{Path: fields[i+1], Status: fields[i][0]})
		}
	}
	others, _, err := run(dir, "", "ls-files", "-z", "--others", "--exclude-standard")
	if err != nil {
		return nil, err
	}
	for f := range strings.SplitSeq(others, "\x00") {
		if f != "" {
			out = append(out, Change{Path: f, Status: '?'})
		}
	}
	return out, nil
}

// Commit resolves rev to a commit id; ok is false when there is no such commit.
func Commit(dir, rev string) (string, bool) {
	out, _, err := run(dir, "", "rev-parse", "--verify", "--quiet", "--end-of-options", rev+"^{commit}")
	id := strings.TrimSpace(out)
	return id, err == nil && id != ""
}

// MergeBase is the best common ancestor of commits a and b.
func MergeBase(dir, a, b string) (string, error) {
	out, _, err := run(dir, "", "merge-base", a, b)
	return strings.TrimSpace(out), err
}

// DefaultBranch names the repository's main line: the branch origin/HEAD
// points at, else the first of main, master, origin/main and origin/master
// that exists. ok is false when none does.
func DefaultBranch(dir string) (string, bool) {
	if out, _, err := run(dir, "", "symbolic-ref", "--quiet", "--short", "refs/remotes/origin/HEAD"); err == nil {
		if ref := strings.TrimSpace(out); ref != "" {
			if _, ok := Commit(dir, ref); ok {
				return ref, true
			}
		}
	}
	for _, ref := range []string{"main", "master", "origin/main", "origin/master"} {
		if _, ok := Commit(dir, ref); ok {
			return ref, true
		}
	}
	return "", false
}
