// Package gitx runs the handful of git plumbing commands rvw needs: finding a
// worktree root, keeping exact file versions as blobs in the object store, and
// reading the uncommitted changes of a worktree.
// Every call is `git -C <dir> <subcommand> ...`, with git taken from $PATH.
package gitx

import (
	"bytes"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
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

// Hunk is one uncommitted change to a file, in the line numbers of the old
// (HEAD) and new (worktree) versions, as in a unified diff header. A count of
// 0 means no lines on that side: NewCount 0 is a deletion after line
// NewStart, OldCount 0 an addition.
type Hunk struct {
	OldStart int
	OldCount int
	NewStart int
	NewCount int
}

// Change is a file that differs between HEAD and the worktree.
type Change struct {
	// Added is a file HEAD does not have: untracked, or staged as new.
	Added bool
	// Hunks are in file order. A binary or empty file has none.
	Hunks []Hunk
}

// Changes compares the worktree at dir with HEAD, staged and unstaged edits
// together, and returns every changed file still on disk, keyed by its path
// relative to dir, slash-separated. An untracked file that is not ignored is
// one hunk of all its lines. Paths, if given, limit it to those pathspecs. In
// a repository with no commit yet, everything is compared with an empty tree.
func Changes(dir string, paths ...string) (map[string]Change, error) {
	base, err := headTree(dir)
	if err != nil {
		return nil, err
	}
	args := []string{
		"-c", "core.quotePath=false", "diff", "--no-color", "--no-ext-diff", "--no-textconv",
		"--no-renames", "--ignore-submodules=all", "--relative", "-U0",
		"--src-prefix=a/", "--dst-prefix=b/", base, "--",
	}
	out, _, err := run(dir, "", append(args, paths...)...)
	if err != nil {
		return nil, err
	}
	changes := parseDiff(out)

	out, _, err = run(dir, "", append([]string{"ls-files", "-z", "--others", "--exclude-standard", "--"}, paths...)...)
	if err != nil {
		return nil, err
	}
	for rel := range strings.SplitSeq(out, "\x00") {
		if rel == "" {
			continue
		}
		data, err := os.ReadFile(filepath.Join(dir, filepath.FromSlash(rel)))
		if err != nil {
			continue // gone, or not a regular file
		}
		c := Change{Added: true}
		if n := countLines(data); n > 0 {
			c.Hunks = []Hunk{{NewStart: 1, NewCount: n}}
		}
		changes[rel] = c
	}
	return changes, nil
}

// headTree is what the worktree is compared with: HEAD, else the empty tree.
func headTree(dir string) (string, error) {
	if out, _, err := run(dir, "", "rev-parse", "--verify", "--quiet", "HEAD^{commit}"); err == nil {
		return strings.TrimSpace(out), nil
	}
	out, _, err := run(dir, "", "hash-object", "-t", "tree", "--stdin")
	return blobID(out, err)
}

// parseDiff reads the file names and hunk headers of a unified diff made
// with -U0 and the a/ and b/ prefixes. Deleted files are left out.
func parseDiff(out string) map[string]Change {
	changes := map[string]Change{}
	var file string
	var added, header bool
	for line := range strings.SplitSeq(out, "\n") {
		switch {
		case strings.HasPrefix(line, "diff --git "):
			file, added, header = "", false, true
		case header && strings.HasPrefix(line, "--- "):
			added = line == "--- /dev/null"
		case header && strings.HasPrefix(line, "+++ "):
			file = diffPath(strings.TrimPrefix(line, "+++ "))
			if file != "" {
				changes[file] = Change{Added: added}
			}
		case strings.HasPrefix(line, "@@ "):
			header = false
			if h, ok := parseHunk(line); ok && file != "" {
				c := changes[file]
				c.Hunks = append(c.Hunks, h)
				changes[file] = c
			}
		}
	}
	return changes
}

// diffPath is the file a "+++ " line names, without its b/ prefix, or "" for
// /dev/null. Git quotes unusual names C-style and ends a name holding a space
// with a tab.
func diffPath(s string) string {
	s = strings.TrimSuffix(s, "\t")
	if s == "/dev/null" {
		return ""
	}
	if strings.HasPrefix(s, `"`) {
		if u, err := strconv.Unquote(s); err == nil {
			s = u
		}
	}
	return strings.TrimPrefix(s, "b/")
}

// parseHunk reads "@@ -a[,b] +c[,d] @@ ...".
func parseHunk(line string) (Hunk, bool) {
	fields := strings.Fields(line)
	if len(fields) < 3 || !strings.HasPrefix(fields[1], "-") || !strings.HasPrefix(fields[2], "+") {
		return Hunk{}, false
	}
	oldStart, oldCount, ok1 := parseRange(fields[1][1:])
	newStart, newCount, ok2 := parseRange(fields[2][1:])
	return Hunk{OldStart: oldStart, OldCount: oldCount, NewStart: newStart, NewCount: newCount}, ok1 && ok2
}

func parseRange(s string) (start, count int, ok bool) {
	a, b, hasCount := strings.Cut(s, ",")
	start, err := strconv.Atoi(a)
	if err != nil {
		return 0, 0, false
	}
	if !hasCount {
		return start, 1, true
	}
	count, err = strconv.Atoi(b)
	return start, count, err == nil
}

// countLines counts lines the way the service does: a trailing newline does
// not start another line.
func countLines(data []byte) int {
	n := bytes.Count(data, []byte("\n"))
	if len(data) > 0 && data[len(data)-1] != '\n' {
		n++
	}
	return n
}
