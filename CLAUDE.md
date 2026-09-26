# rvw

A local, per-workspace queue of code review comments. Humans and agents add
comments on line ranges; a coding agent pulls them and records a decision on
each. See README.md for the pitch and examples.

## Commands

```sh
go build -o bin/rvw ./cmd/rvw   # bin/ is gitignored
go vet ./...
go test ./...                   # service, store, MCP schema and MCP server tests
bats test/rvw.bats              # CLI contract; builds its own binary
make lint                       # golangci-lint, config in .golangci.yml
make fmt                        # gofumpt + goimports (not plain gofmt)
make vuln                       # govulncheck
make check                      # lint, vuln, vet, go test and bats
make hooks                      # once per clone: pre-push runs make check
```

Run `make check` before calling a change done.

`make lint` must stay at 0 issues. Fix the finding; do not raise a threshold.
A `//nolint:<linter> // why` is for a finding that is wrong in context, never
for convenience. The complexity exclusions in `.golangci.yml` are old debt:
remove one when you split that function, and do not add to them.
Tool versions are pinned in the Makefile and `.github/workflows/ci.yml`;
change both together.

When you try `bin/rvw` by hand, pass `--db` with a scratch file. The default
database is the user's real queue.

## Layout

`cmd/rvw` → `internal/cli` → `internal/review` → `internal/store`, with
`internal/mcpserver` (`rvw mcp serve`) and `internal/tui` (`rvw tui`) two more
adapters beside `internal/cli`.

- `internal/cli` — cobra commands. Turns flags and stdin into service inputs,
  and service outputs into text. No domain logic here.
- `internal/review` — the `Service` and the domain. `operations.go` holds the
  typed input/output of every operation.
- `internal/store` — SQLite (modernc, no cgo). Schema in `schema.sql`.
- `internal/mcpserver` — MCP tools over the `Service` (official go-sdk). One
  tool per operation; its input and output are the operation types. No domain
  logic here either.
- `internal/tui` — the terminal UI (Bubble Tea v2, Lip Gloss v2, chroma). One
  model; every action is one `Service` call. No domain logic here either.
  Golden views live in `testdata/`; regenerate with
  `go test ./internal/tui -update`.
- `internal/render` (text/markdown/JSON output), `internal/gitx` (git blobs
  for code snapshots and diffs), `internal/workspace` (resolve the git root).

## Invariants

- `test/rvw.bats` is the CLI contract. A behavior change starts there.
- Every failure is one `rvw: ...` line on stderr and exit status 1. Notices
  go to stderr; stdout carries only the result.
- Every operation input and output is a JSON object that infers an MCP schema
  (`internal/review/schema_test.go`). Add new operation types to that test,
  and a tool for each new operation in `internal/mcpserver`.
- A field without `omitempty` is required; its `jsonschema` tag is its
  description.
- Configuration is flags only (`--workspace`, `--lane`, `--author`, `--db`).
  Do not add environment variables.
- A pulled comment leaves the queue. Lane scoping is strict: a pinned pull
  never takes another lane's or an unlaned comment.

## Conventions

- Conventional commits: `feat:`, `fix:`, `test:`, `docs:`, `feat!:` for
  breaking changes.
- `skills/rvw/SKILL.md` teaches agents the MCP tools only; the CLI is for
  people. When a tool changes, update the skill; when a command or flag
  changes, update the README.
- `bats` needs `sqlite3` and a UTF-8 locale (`LC_ALL=C.UTF-8`) for a few tests.
