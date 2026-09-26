package cli

import (
	"fmt"
	"os"
	"strings"

	"github.com/spf13/cobra"

	"github.com/gustavofsantos/rvw/internal/render"
	"github.com/gustavofsantos/rvw/internal/review"
)

const about = `rvw — a per-workspace queue of code review feedback, written in an editor or
by a reviewing agent and pulled by a coding agent.

You annotate code where you read it (an editor plugin, ` + "`rvw tui`" + `, or ` + "`rvw add`" + `
straight from a shell); every comment lands in a durable queue keyed by the
workspace it was written in. Whatever agent you are running later drains that
queue with ` + "`rvw pull`" + ` — pulled comments leave the queue, so the same note is
never worked twice.

Comments stay standalone by default. ` + "`rvw submit`" + ` can group zero or more
pending comments under one review with a decision and summary. Pulling any
linked comment hands over the complete review as one unit.

A comment carries who raised it and which lane it belongs to. It ends in a
recorded resolution: ` + "`resolve`" + ` (done) or ` + "`reject`" + ` (with a reason).

Everything is set with flags:
  --workspace PATH  the queue to act on: the git toplevel of this directory
                    (default: the current directory); worktrees are their
                    own workspace, and a directory outside git has none
  --lane NAME       the lane a comment lands in and a read is pinned to
                    (default: every lane). Scoping is strict — a pull pinned
                    to a lane never swallows another lane's comments, and
                    says on stderr when comments are waiting elsewhere.
  --author WHO      who is speaking (default: $USER); agents pass their name
  --db PATH         the database (default: $XDG_DATA_HOME/rvw/rvw.db, else
                    ~/.local/share/rvw/rvw.db)`

const examples = `  rvw add --file src/api.py --lines 40-58 --comment "extract this branch"
  rvw add --file src/api.py --lines 12 --comment "typo" --lane refactor-auth
  rvw submit --id r1 --id r2 --decision request-changes --summary "Fix both"
  rvw submit --no-comments --decision comment --summary "No findings"
  rvw list                              # what is queued here, oldest first
  rvw list --file src/api.py --format json
  rvw list --reviews                    # submitted review sheets and their state
  rvw pull                              # dequeue everything, as markdown
  rvw pull --limit 1 --format json      # dequeue one, machine-readable
  rvw pull --peek                       # look without dequeuing
  rvw resolve r3 --note "renamed, test added"
  rvw reject r4 --note "intentional: the caller validates"
  rvw show r3                           # one comment, its decision and its diff
  rvw show rv1                          # a whole review sheet
  rvw count                             # pending handoff count, for a statusline
  rvw workspaces                        # every workspace holding comments
  rvw mcp config                        # connect Claude Code to the MCP server
  rvw tui                               # read code and comment in a terminal UI`

func (a *app) root() *cobra.Command {
	root := &cobra.Command{
		Use:           prog + " COMMAND",
		Short:         "a per-workspace queue of code review feedback",
		Long:          about,
		Example:       examples,
		SilenceUsage:  true,
		SilenceErrors: true,
		Args:          cobra.NoArgs,
		Run:           func(cmd *cobra.Command, _ []string) { cmd.Help() },
	}
	root.CompletionOptions.DisableDefaultCmd = true
	root.PersistentFlags().StringVar(&a.workspaceFlag, "workspace", "",
		"queue to act on: the git toplevel of this directory (default: $PWD)")
	root.PersistentFlags().StringVar(&a.dbFlag, "db", "",
		"database file (default: $XDG_DATA_HOME/rvw/rvw.db, else ~/.local/share/rvw/rvw.db)")
	root.AddCommand(a.addCmd(), a.submitCmd(), a.listCmd(), a.pullCmd(), a.showCmd(),
		a.editCmd(), a.decideCmd(review.OutcomeDone), a.decideCmd(review.OutcomeRejected),
		a.countCmd(), a.workspacesCmd(), a.pathCmd(), a.mcpCmd(), a.tuiCmd())
	return root
}

// ── add / submit ─────────────────────────────────────────────────────────────

func (a *app) addCmd() *cobra.Command {
	var (
		file, lines, comment, commentFile, codeFile, filetype, lane, author string
		format                                                              formatFlag
	)
	cmd := &cobra.Command{
		Use:   "add --file PATH --lines N|N-M [--comment TEXT]",
		Short: "enqueue a review comment on a file range",
		Long: `Enqueue one review comment on a line range of one file.

The code shown to the agent is snapshotted at add time: by default it is read
from the file on disk, or from --code-file when the editor holds unsaved
changes (pass the WHOLE buffer; --lines slices it). The comment comes from
--comment, --comment-file, or stdin when it is piped.`,
		Example: `  rvw add --file src/api.py --lines 40-58 --comment "extract this branch"
  rvw add --file src/api.py --lines 12 --comment "typo" --format json
  nvim-buffer | rvw add --file src/api.py --lines 40-58 --code-file - --comment "reads oddly"
  echo "long note" | rvw add --file a.py --lines 3-9`,
		Args: cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			if err := format.check(); err != nil {
				return err
			}
			ws, err := a.workspace()
			if err != nil {
				return err
			}
			rng, err := review.ParseLineRange(lines)
			if err != nil {
				return err
			}
			text, err := a.commentText(cmd.Flags().Changed("comment"), comment, commentFile, codeFile)
			if err != nil {
				return err
			}
			source, err := a.codeSource(codeFile)
			if err != nil {
				return err
			}
			path, err := filePath(file)
			if err != nil {
				return err
			}
			svc, err := a.svc()
			if err != nil {
				return err
			}
			c, err := svc.Add(a.ctx, review.AddInput{
				Workspace: ws, File: path, StartLine: rng.Start, EndLine: rng.End, Comment: text,
				Source: source, Filetype: filetype, Lane: laneOf(lane), Author: authorOf(author),
			})
			if err != nil {
				return err
			}
			switch format.value {
			case "json":
				return render.JSON(a.stdout, c)
			case "ids":
				fmt.Fprintln(a.stdout, c.ID)
			default:
				fmt.Fprintf(a.stdout, "%s  %s  %s\n", c.ID, c.Location(), render.FirstLine(c.Comment))
			}
			return nil
		},
	}
	f := cmd.Flags()
	f.StringVar(&file, "file", "", "file the comment is about")
	f.StringVar(&lines, "lines", "", "1-indexed, inclusive line range: N or N-M")
	f.StringVar(&comment, "comment", "", "the comment text (may be multi-line)")
	f.StringVar(&commentFile, "comment-file", "", "read the comment from a file, or - for stdin")
	f.StringVar(&codeFile, "code-file", "", "snapshot the code from here instead of disk, or - for stdin (whole file)")
	f.StringVar(&filetype, "filetype", "", "fence language (default: guessed from the extension)")
	f.StringVar(&lane, "lane", "", "branch/lane this comment belongs to")
	f.StringVar(&author, "author", "", "who raised it (default: $USER)")
	format.register(cmd, "text", "text", "json", "ids")
	cmd.MarkFlagRequired("file")
	cmd.MarkFlagRequired("lines")
	return cmd
}

// commentText finds the note: --comment, else --comment-file, else piped stdin.
func (a *app) commentText(given bool, comment, commentFile, codeFile string) (string, error) {
	switch {
	case given:
	case commentFile == "-":
		if codeFile == "-" {
			return "", usageError("--comment-file - conflicts with --code-file - (one stdin)")
		}
		text, _, err := a.readStdinIfPiped()
		if err != nil {
			return "", err
		}
		comment = text
	case commentFile != "":
		data, err := os.ReadFile(expandHome(commentFile))
		if err != nil {
			return "", usageError("--comment-file '%s' does not exist", commentFile)
		}
		comment = string(data)
	case codeFile != "-":
		text, _, err := a.readStdinIfPiped()
		if err != nil {
			return "", err
		}
		comment = text
	}
	if strings.TrimSpace(comment) == "" {
		return "", usageError("a review comment needs text: pass --comment TEXT, --comment-file PATH, or pipe it in")
	}
	return comment, nil
}

// codeSource is the whole-file snapshot from --code-file, nil to read the disk.
func (a *app) codeSource(codeFile string) (*string, error) {
	switch codeFile {
	case "":
		return nil, nil
	case "-":
		text, _, err := a.readStdinIfPiped()
		return &text, err
	}
	data, err := os.ReadFile(expandHome(codeFile))
	if err != nil {
		if isNotExist(err) {
			return nil, usageError("--code-file '%s' does not exist", codeFile)
		}
		return nil, err
	}
	text := string(data)
	return &text, nil
}

func (a *app) submitCmd() *cobra.Command {
	var (
		ids                             []string
		noComments                      bool
		summary, decision, lane, author string
		format                          formatFlag
	)
	cmd := &cobra.Command{
		Use:   "submit --decision DECISION --summary TEXT [--id ID]... | --no-comments",
		Short: "submit a review over pending review comments",
		Long: `Submit one review with a decision and summary over explicitly selected
pending review comments. Without --id it takes this author's pending, unlinked
comments in this exact lane; --no-comments submits a summary-only review.`,
		Args: cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			if err := format.check(); err != nil {
				return err
			}
			if err := oneOf("--decision", decision, strs(review.Decisions)); err != nil {
				return err
			}
			ws, err := a.workspace()
			if err != nil {
				return err
			}
			svc, err := a.svc()
			if err != nil {
				return err
			}
			r, err := svc.Submit(a.ctx, review.SubmitInput{
				Workspace: ws, Decision: review.Decision(decision), Summary: summary,
				CommentIDs: ids, NoComments: noComments, Lane: laneOf(lane), Author: authorOf(author),
			})
			if err != nil {
				return err
			}
			switch format.value {
			case "json":
				return render.JSON(a.stdout, r)
			case "ids":
				fmt.Fprintln(a.stdout, r.ID)
			default:
				fmt.Fprintf(a.stdout, "%s  %s  %d review comment(s)\n", r.ID, r.Decision, len(r.CommentIDs))
			}
			return nil
		},
	}
	f := cmd.Flags()
	f.StringArrayVar(&ids, "id", nil, "comment id (repeatable; default: this author and exact lane)")
	f.BoolVar(&noComments, "no-comments", false, "submit a summary-only review without selecting pending comments")
	f.StringVar(&summary, "summary", "", "review assessment")
	f.StringVar(&decision, "decision", "", "review decision: "+strings.Join(strs(review.Decisions), ", "))
	f.StringVar(&lane, "lane", "", "review lane")
	f.StringVar(&author, "author", "", "reviewer (default: $USER)")
	format.register(cmd, "text", "text", "json", "ids")
	cmd.MarkFlagsMutuallyExclusive("id", "no-comments")
	cmd.MarkFlagRequired("summary")
	cmd.MarkFlagRequired("decision")
	return cmd
}

// ── reads ────────────────────────────────────────────────────────────────────

func (a *app) query(lanes *laneFlags, file, status string) (review.QueryInput, error) {
	if err := checkStatus(status); err != nil {
		return review.QueryInput{}, err
	}
	ws, err := a.workspace()
	if err != nil {
		return review.QueryInput{}, err
	}
	path, err := filePath(file)
	if err != nil {
		return review.QueryInput{}, err
	}
	return review.QueryInput{Workspace: ws, Status: review.StatusFilter(status), File: path, Lane: lanes.pinned()}, nil
}

func (a *app) listCmd() *cobra.Command {
	var (
		lanes        laneFlags
		file, status string
		reviews      bool
		format       formatFlag
	)
	cmd := &cobra.Command{
		Use:   "list",
		Short: "show queued review comments or submitted reviews without dequeuing them",
		Long: `Show this workspace's review comments, oldest first. Nothing is dequeued.

With --reviews it shows submitted review sheets instead, each with its state:
pending, pulled, or complete once every linked comment is decided. --status
then takes pending, pulled, complete, open (not yet complete) or all.`,
		Example: `  rvw list
  rvw list --file src/api.py --format json   # what an editor draws signs from
  rvw list --status pulled                   # what has already been handed over
  rvw list --reviews                         # submitted reviews still pending
  rvw list --reviews --status all            # every submitted review`,
		Args: cobra.NoArgs,
		RunE: func(*cobra.Command, []string) error {
			if err := format.check(); err != nil {
				return err
			}
			if reviews {
				return a.listSheets(&lanes, file, status, format.value)
			}
			in, err := a.query(&lanes, file, status)
			if err != nil {
				return err
			}
			svc, err := a.svc()
			if err != nil {
				return err
			}
			out, err := svc.List(a.ctx, in)
			if err != nil {
				return err
			}
			switch format.value {
			case "json":
				return render.JSON(a.stdout, render.Envelope{Workspace: out.Workspace, Count: len(out.Comments), Comments: out.Comments})
			case "ids":
				render.IDs(a.stdout, nil, out.Comments)
			case "count":
				fmt.Fprintln(a.stdout, len(out.Comments))
			case "markdown":
				fmt.Fprint(a.stdout, render.Markdown(out.Workspace, nil, out.Comments, false))
			default:
				if !render.Text(a.stdout, nil, out.Comments) {
					a.emptyNotice(out.Workspace, in.Lane)
				}
			}
			return nil
		},
	}
	cmd.Flags().StringVar(&file, "file", "", "only comments on this file")
	cmd.Flags().BoolVar(&reviews, "reviews", false, "list submitted review sheets instead of comments")
	statusFlag(cmd, &status, "which comments to show: pending, pulled, done, rejected, open (pending + pulled), all")
	lanes.register(cmd)
	format.register(cmd, "text", "text", "json", "ids", "count", "markdown")
	return cmd
}

// listSheets is `list --reviews`: submitted review sheets, oldest first.
func (a *app) listSheets(lanes *laneFlags, file, status, format string) error {
	if err := oneOf("--status", status, strs(review.SheetFilters)); err != nil {
		return err
	}
	ws, err := a.workspace()
	if err != nil {
		return err
	}
	path, err := filePath(file)
	if err != nil {
		return err
	}
	svc, err := a.svc()
	if err != nil {
		return err
	}
	out, err := svc.Sheets(a.ctx, review.SheetsInput{
		Workspace: ws, Status: review.SheetFilter(status), File: path, Lane: lanes.pinned(),
	})
	if err != nil {
		return err
	}
	switch format {
	case "json":
		return render.JSON(a.stdout, out)
	case "ids":
		for _, rs := range out.Sheets {
			fmt.Fprintln(a.stdout, rs.Review.ID)
		}
	case "count":
		fmt.Fprintln(a.stdout, out.Count)
	case "markdown":
		handoffs := make([]review.Handoff, len(out.Sheets))
		for i, rs := range out.Sheets {
			handoffs[i] = review.Handoff{Review: rs.Review, Comments: rs.Comments}
		}
		fmt.Fprint(a.stdout, render.Markdown(out.Workspace, handoffs, nil, false))
	default:
		for _, rs := range out.Sheets {
			fmt.Fprintln(a.stdout, render.SheetLine(rs))
		}
		if out.Count == 0 {
			scope := ""
			if l := lanes.pinned(); l != "" {
				scope = " in lane " + l
			}
			a.notice("no submitted reviews for %s%s", out.Workspace, scope)
		}
	}
	return nil
}

func (a *app) emptyNotice(ws, lane string) {
	scope := ""
	if lane != "" {
		scope = " in lane " + lane
	}
	a.notice("no review comments for %s%s", ws, scope)
}

func (a *app) countCmd() *cobra.Command {
	var (
		lanes        laneFlags
		file, status string
	)
	cmd := &cobra.Command{
		Use:   "count",
		Short: "print the pending handoff count (statusline-friendly)",
		Long: `Print how many review handoffs are pending here: a submitted review counts
once, with its linked comments. For any other --status it counts comments.`,
		Args: cobra.NoArgs,
		RunE: func(*cobra.Command, []string) error {
			in, err := a.query(&lanes, file, status)
			if err != nil {
				return err
			}
			svc, err := a.svc()
			if err != nil {
				return err
			}
			out, err := svc.Count(a.ctx, in)
			if err != nil {
				return err
			}
			fmt.Fprintln(a.stdout, out.Count)
			return nil
		},
	}
	cmd.Flags().StringVar(&file, "file", "", "only count comments on this file")
	statusFlag(cmd, &status, "which comments to count (default: pending)")
	lanes.register(cmd)
	return cmd
}

func (a *app) pullCmd() *cobra.Command {
	var (
		lanes  laneFlags
		file   string
		ids    []string
		limit  int
		peek   bool
		format formatFlag
	)
	cmd := &cobra.Command{
		Use:   "pull",
		Short: "dequeue review handoffs for an agent to act on",
		Long: `Dequeue this workspace's submitted reviews and standalone comments.

A submitted review and its linked comments move as one handoff. Pulled
handoffs do not appear again. Use --peek to read without draining.

Lane scoping is strict: a pull pinned to a lane takes only that lane's
comments — never an unlaned one, never another lane's — and reports on
stderr when comments are waiting elsewhere, so nothing starves unseen.`,
		Example: `  rvw pull                     # everything pending, as markdown
  rvw pull --format json       # same, machine-readable
  rvw pull --limit 1           # oldest handoff only
  rvw pull --id r3 --id r7     # named comments only
  rvw pull --lane auth         # only that branch's comments
  rvw pull --peek              # read without dequeuing`,
		Args: cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			if err := format.check(); err != nil {
				return err
			}
			if cmd.Flags().Changed("limit") && limit < 1 {
				return usageError("--limit must be >= 1")
			}
			ws, err := a.workspace()
			if err != nil {
				return err
			}
			path, err := filePath(file)
			if err != nil {
				return err
			}
			svc, err := a.svc()
			if err != nil {
				return err
			}
			out, err := svc.Pull(a.ctx, review.PullInput{
				Workspace: ws, Lane: lanes.pinned(), File: path, IDs: ids, Limit: limit, Peek: peek,
			})
			if err != nil {
				return err
			}
			switch format.value {
			case "json":
				if err := render.JSON(a.stdout, render.Envelope{
					Workspace: out.Workspace, Count: out.Count(), Comments: out.Comments, SubmittedReviews: out.Reviews,
				}); err != nil {
					return err
				}
			case "ids":
				render.IDs(a.stdout, out.Reviews, out.Comments)
			case "text":
				if !render.Text(a.stdout, out.Reviews, out.Comments) {
					a.emptyNotice(out.Workspace, lanes.pinned())
				}
			default:
				fmt.Fprint(a.stdout, render.Markdown(out.Workspace, out.Reviews, out.Comments, out.Drained))
			}
			if e := out.Elsewhere; e != nil {
				a.notice("%d pending in other lanes: %s", e.Count, strings.Join(e.Lanes, ", "))
			}
			return nil
		},
	}
	cmd.Flags().StringVar(&file, "file", "", "only handoffs touching this file")
	cmd.Flags().StringArrayVar(&ids, "id", nil, "pull only this id (repeatable)")
	cmd.Flags().IntVar(&limit, "limit", 0, "pull at most N handoffs, oldest first")
	cmd.Flags().BoolVar(&peek, "peek", false, "print without dequeuing (leaves the queue intact)")
	lanes.register(cmd)
	format.register(cmd, "markdown", "markdown", "json", "text", "ids")
	return cmd
}

func (a *app) showCmd() *cobra.Command {
	var format formatFlag
	cmd := &cobra.Command{
		Use:   "show ID",
		Short: "show one review comment (or review sheet) with its source evidence",
		Long: `Show one review comment as a fixed-width review line: its source, or the
diff it produced once done, and its note or resolution. A submitted review id
shows the whole review sheet. Nothing is dequeued.`,
		Example: `  rvw show r3
  rvw show rv1
  rvw show r3 --format json`,
		Args: cobra.ExactArgs(1),
		RunE: func(_ *cobra.Command, args []string) error {
			if err := format.check(); err != nil {
				return err
			}
			in, svc, err := a.get(args[0])
			if err != nil {
				return err
			}
			if review.IsReviewID(in.ID) {
				sheet, err := svc.Sheet(a.ctx, in)
				if err != nil {
					return err
				}
				switch format.value {
				case "json":
					return render.JSON(a.stdout, sheet)
				case "markdown":
					handoff := review.Handoff{Review: sheet.Review, Comments: sheet.Comments}
					fmt.Fprint(a.stdout, render.Markdown(in.Workspace, []review.Handoff{handoff}, nil, false))
				default:
					fmt.Fprint(a.stdout, render.Sheet(sheet))
				}
				return nil
			}
			ev, err := svc.Evidence(a.ctx, in)
			if err != nil {
				return err
			}
			switch format.value {
			case "json":
				return render.JSON(a.stdout, ev)
			case "markdown":
				fmt.Fprint(a.stdout, render.Markdown(in.Workspace, nil, []review.Comment{ev.Comment}, false))
			default:
				fmt.Fprint(a.stdout, render.Display(ev))
			}
			return nil
		},
	}
	format.register(cmd, "text", "text", "json", "markdown")
	return cmd
}

func (a *app) get(id string) (review.GetInput, *review.Service, error) {
	ws, err := a.workspace()
	if err != nil {
		return review.GetInput{}, nil, err
	}
	svc, err := a.svc()
	return review.GetInput{Workspace: ws, ID: id}, svc, err
}

func (a *app) workspacesCmd() *cobra.Command {
	var (
		all    bool
		format formatFlag
	)
	cmd := &cobra.Command{
		Use:   "workspaces",
		Short: "list workspaces holding review handoffs",
		Long:  "List every workspace in the store, with its pending handoff count.",
		Args:  cobra.NoArgs,
		RunE: func(*cobra.Command, []string) error {
			if err := format.check(); err != nil {
				return err
			}
			svc, err := a.svc()
			if err != nil {
				return err
			}
			out, err := svc.Workspaces(a.ctx, review.WorkspacesInput{All: all})
			if err != nil {
				return err
			}
			rows := out.Workspaces
			switch {
			case format.value == "json":
				return render.JSON(a.stdout, rows)
			case len(rows) == 0:
				a.notice("no workspace has pending review comments")
			default:
				width := 0
				for _, r := range rows {
					width = max(width, len(fmt.Sprint(r.Pending)))
				}
				for _, r := range rows {
					fmt.Fprintf(a.stdout, "%*d pending  %s\n", width, r.Pending, r.Workspace)
				}
			}
			return nil
		},
	}
	cmd.Flags().BoolVar(&all, "all", false, "include workspaces with nothing pending")
	format.register(cmd, "text", "text", "json")
	return cmd
}

func (a *app) pathCmd() *cobra.Command {
	var workspaceOnly bool
	cmd := &cobra.Command{
		Use:   "path",
		Short: "print where the queue lives",
		Long:  "Print the database backing every queue, or with --workspace-only the resolved workspace.",
		Args:  cobra.NoArgs,
		RunE: func(*cobra.Command, []string) error {
			if workspaceOnly {
				ws, err := a.workspace()
				if err != nil {
					return err
				}
				fmt.Fprintln(a.stdout, ws)
				return nil
			}
			svc, err := a.svc()
			if err != nil {
				return err
			}
			fmt.Fprintln(a.stdout, svc.Location())
			return nil
		},
	}
	cmd.Flags().BoolVar(&workspaceOnly, "workspace-only", false, "print the resolved workspace directory")
	return cmd
}

// ── decisions and edits ──────────────────────────────────────────────────────

func (a *app) decideCmd(outcome review.Outcome) *cobra.Command {
	var note, author string
	cmd := &cobra.Command{
		Use:   "resolve ID",
		Short: "record that a comment was acted on",
		Long: `Record that a review comment was acted on, and by whom. For a tracked file
that still exists, resolve also saves its current contents as a Git blob.
` + "`pull`" + ` says a comment was handed over; ` + "`resolve`" + ` says what became of it.`,
		Example: `  rvw resolve r3
  rvw resolve r3 --note "renamed to first(), added a test"`,
	}
	if outcome == review.OutcomeRejected {
		cmd.Use = "reject ID --note TEXT"
		cmd.Short = "record that a comment will not be actioned, and why"
		cmd.Long = `Record that a review comment will not be actioned. The reason is required —
a silent decline is exactly what this queue exists to prevent.

It is also how a reviewer withdraws a pending comment: it leaves the queue but
stays on record. Nothing in rvw is deleted.`
		cmd.Example = `  rvw reject r3 --note "intentional: the caller already validates"
  rvw reject r5 --note "withdrawn: I misread the diff"`
	}
	cmd.Args = cobra.ExactArgs(1)
	cmd.RunE = func(_ *cobra.Command, args []string) error {
		if outcome == review.OutcomeRejected && strings.TrimSpace(note) == "" {
			return usageError("rejecting a comment needs a reason: pass --note TEXT")
		}
		in, svc, err := a.get(args[0])
		if err != nil {
			return err
		}
		c, err := svc.Resolve(a.ctx, review.ResolveInput{
			Workspace: in.Workspace, ID: in.ID, Outcome: outcome, Note: note, Author: authorOf(author),
		})
		if err != nil {
			return err
		}
		line := fmt.Sprintf("%s  %s  %s", c.ID, c.Location(), c.Status)
		if c.ResolvedBy != nil {
			line += " by @" + *c.ResolvedBy
		}
		if note := deref(c.ResolutionNote); note != "" {
			line += " — " + render.FirstLine(note)
		}
		fmt.Fprintln(a.stdout, line)
		return nil
	}
	help := "what you did"
	if outcome == review.OutcomeRejected {
		help = "why not (required)"
	}
	cmd.Flags().StringVar(&note, "note", "", help)
	cmd.Flags().StringVar(&author, "author", "", "who decided (default: $USER)")
	return cmd
}

func (a *app) editCmd() *cobra.Command {
	var (
		comment string
		format  formatFlag
	)
	cmd := &cobra.Command{
		Use:   "edit ID [--comment TEXT]",
		Short: "replace the text of a queued review comment",
		Long: `Replace the text of one review comment. The file, range, and code snapshot
are unchanged — reject and re-add to move a comment.`,
		Example: `  rvw edit r3 --comment "split this function"
  pbpaste | rvw edit r3`,
		Args: cobra.ExactArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			if err := format.check(); err != nil {
				return err
			}
			if !cmd.Flags().Changed("comment") {
				text, piped, err := a.readStdinIfPiped()
				if err != nil {
					return err
				}
				if !piped {
					return usageError("pass the new text with --comment TEXT, or pipe it in")
				}
				comment = text
			}
			in, svc, err := a.get(args[0])
			if err != nil {
				return err
			}
			c, err := svc.Edit(a.ctx, review.EditInput{Workspace: in.Workspace, ID: in.ID, Comment: comment})
			if err != nil {
				return err
			}
			if format.value == "json" {
				return render.JSON(a.stdout, c)
			}
			fmt.Fprintf(a.stdout, "%s  %s  %s\n", c.ID, c.Location(), render.FirstLine(c.Comment))
			return nil
		},
	}
	cmd.Flags().StringVar(&comment, "comment", "", "new text (default: read stdin)")
	format.register(cmd, "text", "text", "json")
	return cmd
}
