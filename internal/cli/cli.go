// Package cli is rvw's command line: it turns flags, environment and stdin
// into service inputs, and service outputs into text on stdout. Informational
// notices go to stderr; every failure is one `rvw: ...` line and exit status 1.
package cli

import (
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"slices"
	"strings"

	"github.com/gustavofsantos/rvw/internal/review"
	"github.com/gustavofsantos/rvw/internal/store"
	"github.com/gustavofsantos/rvw/internal/workspace"
	"github.com/spf13/cobra"
	"golang.org/x/term"
)

const prog = "rvw"

// Environment variables. Each is read as RVW_<NAME>, falling back to the
// REVIEW_<NAME> spelling of the original `review` script.
const (
	envWorkspace = "WORKSPACE"
	envLane      = "LANE"
	envAuthor    = "AUTHOR"
)

func env(name string) string {
	if v := os.Getenv("RVW_" + name); v != "" {
		return v
	}
	return os.Getenv("REVIEW_" + name)
}

// usageError is a failure raised by the CLI itself, before the service runs.
func usageError(format string, args ...any) error { return fmt.Errorf(format, args...) }

// Main runs the command line and returns the process exit status.
func Main(ctx context.Context, args []string, stdin io.Reader, stdout, stderr io.Writer) int {
	a := &app{ctx: ctx, stdin: stdin, stdout: stdout, stderr: stderr}
	defer a.close()
	root := a.root()
	root.SetArgs(args)
	root.SetIn(stdin)
	root.SetOut(stdout)
	root.SetErr(stderr)
	if err := root.ExecuteContext(ctx); err != nil {
		fmt.Fprintf(stderr, "%s: %s\n", prog, err)
		return 1
	}
	return 0
}

type app struct {
	ctx            context.Context
	stdin          io.Reader
	stdout, stderr io.Writer

	workspaceFlag string
	store         *store.Store
	service       *review.Service
}

func (a *app) svc() (*review.Service, error) {
	if a.service != nil {
		return a.service, nil
	}
	path, err := store.DefaultPath()
	if err != nil {
		return nil, err
	}
	if a.store, err = store.Open(a.ctx, path); err != nil {
		return nil, err
	}
	a.service = review.NewService(a.store)
	return a.service, nil
}

func (a *app) close() {
	if a.store != nil {
		a.store.Close()
	}
}

func (a *app) notice(format string, args ...any) {
	fmt.Fprintf(a.stderr, "%s: "+format+"\n", append([]any{prog}, args...)...)
}

// workspace resolves which queue to act on: --workspace, else $RVW_WORKSPACE,
// else the current directory; then its git worktree root, if any.
func (a *app) workspace() (string, error) {
	dir, source := a.workspaceFlag, "--workspace"
	if dir == "" {
		dir, source = env(envWorkspace), "$RVW_WORKSPACE"
	}
	if dir == "" {
		cwd, err := os.Getwd()
		if err != nil {
			return "", err
		}
		return workspace.Resolve(cwd)
	}
	dir = expandHome(dir)
	if !workspace.IsDir(dir) {
		return "", usageError("%s '%s' is not a directory", source, dir)
	}
	return workspace.Resolve(dir)
}

// ── shared flags ─────────────────────────────────────────────────────────────

// laneFlags scope a read to one lane: --lane, else $RVW_LANE, else every lane.
type laneFlags struct {
	lane string
	all  bool
}

func (l *laneFlags) register(cmd *cobra.Command) {
	cmd.Flags().StringVar(&l.lane, "lane", "", "only this lane's comments (default: $RVW_LANE, else every lane)")
	cmd.Flags().BoolVar(&l.all, "all-lanes", false, "every lane, ignoring $RVW_LANE")
}

// pinned is the lane this invocation is pinned to, "" for every lane.
func (l *laneFlags) pinned() string {
	if l.all {
		return ""
	}
	return laneOf(l.lane)
}

// laneOf is the lane a write lands in: the flag, else $RVW_LANE.
func laneOf(flag string) string {
	return strings.TrimSpace(firstNonEmpty(flag, env(envLane)))
}

// authorOf is who speaks: the flag, else $RVW_AUTHOR (agents set it), else $USER.
func authorOf(flag string) string {
	return strings.TrimSpace(firstNonEmpty(flag, env(envAuthor), os.Getenv("USER")))
}

type formatFlag struct {
	value   string
	allowed []string
}

func (f *formatFlag) register(cmd *cobra.Command, def string, allowed ...string) {
	f.allowed = allowed
	cmd.Flags().StringVar(&f.value, "format", def, fmt.Sprintf("output shape: %s (default: %s)", strings.Join(allowed, ", "), def))
}

func (f *formatFlag) check() error { return oneOf("--format", f.value, f.allowed) }

func statusFlag(cmd *cobra.Command, target *string, help string) {
	cmd.Flags().StringVar(target, "status", string(review.FilterPending), help)
}

func checkStatus(value string) error {
	return oneOf("--status", value, strs(review.StatusFilters))
}

func oneOf(flag, value string, allowed []string) error {
	if slices.Contains(allowed, value) {
		return nil
	}
	return usageError("%s '%s' is not one of %s", flag, value, strings.Join(allowed, ", "))
}

// filePath makes a --file argument absolute against the current directory.
func filePath(file string) (string, error) {
	if file == "" {
		return "", nil
	}
	return filepath.Abs(expandHome(file))
}

// readStdinIfPiped reads stdin unless it is an interactive terminal.
func (a *app) readStdinIfPiped() (string, bool, error) {
	if f, ok := a.stdin.(*os.File); ok && term.IsTerminal(int(f.Fd())) {
		return "", false, nil
	}
	data, err := io.ReadAll(a.stdin)
	return string(data), true, err
}

func expandHome(path string) string {
	if path == "~" || strings.HasPrefix(path, "~/") {
		if home, err := os.UserHomeDir(); err == nil {
			return filepath.Join(home, path[1:])
		}
	}
	return path
}

func firstNonEmpty(values ...string) string {
	for _, v := range values {
		if v != "" {
			return v
		}
	}
	return ""
}

func strs[T ~string](values []T) []string {
	out := make([]string, len(values))
	for i, v := range values {
		out[i] = string(v)
	}
	return out
}

func deref(s *string) string {
	if s == nil {
		return ""
	}
	return *s
}

// isNotExist reports a missing file, for friendlier flag errors.
func isNotExist(err error) bool { return errors.Is(err, os.ErrNotExist) }
