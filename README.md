# rvw

Code review for humans/AI.

`rvw` is a local review queue. You leave comments on line ranges of files,
from an editor, a shell or an agent. The agent working on that code pulls
them, acts on them, and records what became of each one.

## Why

Code review usually happens after the code is pushed, on a hosted platform,
in a pull request. When an AI agent writes the code, feedback is needed
earlier and closer to the work: while the agent is still running, on files
that are not committed, sometimes from another agent. Pasting notes into a
chat loses where they point, what the code looked like and whether anyone
acted on them.

`rvw` keeps that conversation local and structured:

- **Anchored.** Each comment names a file, a line range and the code as it
  stood when it was written.
- **Handed over once.** A pull takes comments out of the queue, so no agent
  works the same note twice.
- **Accounted for.** Each comment ends in a recorded decision: done, or
  rejected with a reason. Done comments keep the diff they produced.
- **Scoped.** A queue belongs to a workspace (a git worktree). Lanes divide it
  when several branches share one working tree.

## Examples

Leave comments where you read the code:

```sh
rvw add --file src/api.py --lines 40-58 --comment "extract this branch"
rvw add --file src/api.py --lines 12 --comment "typo" --lane refactor-auth
```

Group them into one review with a verdict:

```sh
rvw submit --id r1 --id r2 --decision request-changes --summary "Fix both before merging"
```

Hand the review to the agent, as markdown or JSON:

```sh
rvw pull
rvw pull --lane refactor-auth --format json
rvw pull --peek                # read without dequeuing
```

Close the loop:

```sh
rvw resolve r1 --author claude --note "extracted into parse_header(), test added"
rvw reject r2 --author claude --note "intentional: the caller validates"
rvw show r1                    # the comment, its resolution and the diff
rvw show rv1                   # the whole review sheet
```

Keep an eye on it:

```sh
rvw list --status open         # raised, not yet decided
rvw list --reviews             # submitted reviews and their state
rvw count                      # pending handoffs, for a statusline
rvw workspaces                 # every workspace with something pending
```

`rvw --help` lists every command and flag.

## Claude Code

Install the binary, then the plugin. The plugin adds an `rvw` skill that
teaches Claude to pull the queue, act on each comment and resolve or reject it.

```sh
go install github.com/gustavofsantos/rvw/cmd/rvw@latest
```

Or, from a checkout, `make install` (`go install ./cmd/rvw`). `make uninstall`
removes it.

```
/plugin marketplace add gustavofsantos/rvw
/plugin install rvw@rvw
```

Then ask Claude to "address the review comments", or run `/rvw:rvw`.

## Use cases

- **Human → agent.** Read an agent's changes in your editor and leave comments
  on the lines as you go. Tell the agent to pull the review. It works through
  the comments and records a decision on each.
- **Agent → agent.** A reviewing agent adds comments and submits a review with
  `--author reviewer`. The coding agent pulls the review, fixes the code and
  resolves each comment. You read the outcome with `rvw show`.
- **Parallel branches.** With several branches in one working tree, give each
  agent its own `--lane`. A pinned pull never takes another lane's comments,
  and it reports when work is waiting in another lane.
- **Audit trail.** `rvw list --status done --format json` gives you every
  addressed comment with who resolved it and when. `rvw show` shows what
  changed.
