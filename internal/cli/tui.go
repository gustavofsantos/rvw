package cli

import (
	"os"
	"strings"
	"unicode"
	"unicode/utf8"

	"github.com/gustavofsantos/rvw/internal/tui"
	"github.com/spf13/cobra"
	"golang.org/x/term"
)

func (a *app) tuiCmd() *cobra.Command {
	var lane, author, editor, leader string
	cmd := &cobra.Command{
		Use:   "tui",
		Short: "browse the workspace and leave review comments in a terminal UI",
		Long: `Open a full-screen review UI on the workspace: a file tree, the current file
with syntax highlighting, and a rail in the gutter marking every line under an
open comment. Select lines, comment on them, edit a comment, submit a review.
The workspace is a git worktree; the tree leaves out the files git ignores.
t switches the tree to the changed files only; b chooses what they, and the
gutter's change marks, compare with: uncommitted changes (against HEAD, the
default), the default branch (against its merge-base with main or master),
or the previous commit (against HEAD~1). Untracked files count as changes.
The bottom bar shows the comments on the cursor line, else the branch and
the uncommitted changes as +added -deleted line counts.

Comments are written in your editor: --editor, else $VISUAL, else $EDITOR,
else vi. Lines from the scissors line down are context and are dropped; an
empty note cancels. --lane and --author stamp what you add and submit; the
gutter shows every lane's open comments.

Nothing is watched: the file and its comments are re-read when you open a
file, press r, or add, edit or submit; the branch and the changes when you
press r, t or b.

Keys (? shows them in the UI); <leader> is space unless --leader says:
  <leader>p go to file · <leader>l open comments · Tab switch pane · s submit · q quit
  t files or changes · b compare with: uncommitted, default branch, previous commit
  tree    j/k move · l/↵ open or expand · h collapse or go to parent · gg/G
  viewer  j/k · C-d/C-u half page · gg/G · NG or :N go to line
          ]c/[c next/previous comment · ]h/[h next/previous change, around
          V select lines · c comment · e edit
          r reload`,
		Example: `  rvw tui
  rvw tui --lane refactor-auth --editor "code --wait"
  rvw tui --leader ,
  rvw --workspace ~/src/project tui`,
		Args: cobra.NoArgs,
		RunE: func(*cobra.Command, []string) error {
			leaderKey, err := leaderOf(leader)
			if err != nil {
				return err
			}
			in, inOK := a.stdin.(*os.File)
			out, outOK := a.stdout.(*os.File)
			if !inOK || !outOK || !term.IsTerminal(int(in.Fd())) || !term.IsTerminal(int(out.Fd())) {
				return usageError("tui needs an interactive terminal on stdin and stdout")
			}
			ws, err := a.workspace()
			if err != nil {
				return err
			}
			svc, err := a.svc()
			if err != nil {
				return err
			}
			return tui.Run(a.ctx, tui.Options{
				Service: svc, Workspace: ws, Lane: laneOf(lane), Author: authorOf(author),
				Editor: editorOf(editor), Leader: leaderKey,
			}, in, out)
		},
	}
	cmd.Flags().StringVar(&lane, "lane", "", "lane the comments and reviews you write land in")
	cmd.Flags().StringVar(&author, "author", "", "who is reviewing (default: $USER)")
	cmd.Flags().StringVar(&editor, "editor", "", "command to write comments in (default: $VISUAL, else $EDITOR, else vi)")
	cmd.Flags().StringVar(&leader, "leader", "space", `key that starts leader mappings: "space" or one character, such as ","`)
	return cmd
}

// leaderOf is the leader key as the UI names keys: "space", or the one
// printable character given.
func leaderOf(flag string) (string, error) {
	if flag == "space" || flag == " " {
		return "space", nil
	}
	r, size := utf8.DecodeRuneInString(flag)
	if size == 0 || size != len(flag) || !unicode.IsPrint(r) || unicode.IsSpace(r) {
		return "", usageError("--leader must be space or one character, got %q", flag)
	}
	return flag, nil
}

// editorOf is the command comments are written in: the flag, else the user's
// editor, the way git finds it.
func editorOf(flag string) string {
	return strings.TrimSpace(firstNonEmpty(strings.TrimSpace(flag), os.Getenv("VISUAL"), os.Getenv("EDITOR"), "vi"))
}
