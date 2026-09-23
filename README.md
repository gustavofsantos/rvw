# rvw

Code review for humans/AI.

`rvw` is a local, per-workspace queue of review comments, the layer through
which a human and an agent, or two agents, talk about code. Someone leaves a
comment on a line range of a file, from an editor or a shell. Whichever agent
works in that workspace pulls the comment, acts on it, and records what became
of it.

It replaces the `review` script from my dotfiles and keeps its command line.
It stores everything in one SQLite database instead of per-workspace JSON files.

## Install

```sh
go install github.com/gustavofsantos/rvw/cmd/rvw@latest
# or, from a checkout
go build -o ~/.local/bin/rvw ./cmd/rvw
```

Pure Go (no cgo); Linux and macOS.

## Use

```sh
rvw add --file src/api.py --lines 40-58 --comment "extract this branch"
rvw submit --id r1 --id r2 --decision request-changes --summary "Fix both"
rvw pull                                   # dequeue everything, as markdown
rvw resolve r1 --note "extracted, test added"
rvw reject r2 --note "intentional: the caller validates"
rvw display rv1                            # the review sheet with its evidence
rvw count                                  # pending handoffs, for a statusline
```

`rvw --help` and `rvw <command> --help` cover every command and flag.

The lifecycle of a comment is `pending → pulled → done | rejected`:

- A pull hands a comment over exactly once.
- A decision is final, and a rejection requires a reason.
- Resolving a comment on a git-tracked file saves the file's exact before and
  after versions as blobs, so `rvw display` can show the diff the comment
  produced.

Workspaces and lanes:

- A **workspace** is the git worktree root of the current directory, or the
  directory itself.
- A **lane** divides one workspace between branches that share a working tree
  (GitButler). A pull pinned to a lane never takes another lane's comments.

## Configuration

| Variable | Meaning |
| --- | --- |
| `RVW_WORKSPACE` | workspace to act on (default: git toplevel of `$PWD`) |
| `RVW_LANE` | lane new comments land in and reads are pinned to |
| `RVW_AUTHOR` | who is speaking (default: `$USER`); agents set this |
| `RVW_DB` | database file, overriding the XDG location |

Each `RVW_*` variable falls back to the `REVIEW_*` spelling the old script used.
Existing editor plugins and agent settings keep working unchanged.

### Where the data lives

The database is `$XDG_DATA_HOME/rvw/rvw.db`. If `XDG_DATA_HOME` is unset or
relative, it is `~/.local/share/rvw/rvw.db`, on macOS as well. The queue is data,
not configuration, so it follows `XDG_DATA_HOME` rather than `XDG_CONFIG_HOME`.
`rvw path` prints the location in use.

The data is not migrated from the old `~/.reviews/*/queue.json` files.

## Layout

```text
cmd/rvw             entry point
internal/review     domain: types, typed operation inputs/outputs, Service
internal/store      SQLite adapter for review.Repository (schema.sql)
internal/gitx       git plumbing: worktree root, blob snapshots
internal/workspace  workspace resolution and path canonicalization
internal/render     text, markdown, JSON envelopes, fixed-width display
internal/cli        cobra commands: flags/env/stdin → inputs, outputs → stdout
```

Every operation is one method on `review.Service`, taking one input struct and
returning one output struct. The service never reads the environment, stdin or
the cwd. Messages the old script printed to stderr, such as "N pending in
other lanes", are fields of the output. Errors carry a kind: `invalid`,
`not_found`, `conflict` or `internal`.

Two consequences:

- A second adapter, such as an MCP server, only has to resolve the workspace
  and forward the call.
- The structs are ready to use as MCP tool schemas: a `jsonschema` tag is the
  property's description, and a field without `omitempty` is required.
  `schema_test.go` checks this.

## Test

```sh
go test ./...
bats test/rvw.bats   # CLI contract, ported from the original script's suite
```
