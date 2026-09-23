# rvw

A local, per-workspace queue of code review comments. Humans and agents add
comments on line ranges; a coding agent pulls them and records a decision on
each. See README.md for the pitch and examples.

## Commands

```sh
go build -o bin/rvw ./cmd/rvw   # bin/ is gitignored
go vet ./...
go test ./...                   # service, store and MCP-schema tests
bats test/rvw.bats              # CLI contract; builds its own binary
```

Run both `go test` and `bats` before calling a change done.

When you try `bin/rvw` by hand, pass `--db` with a scratch file. The default
database is the user's real queue.

## Layout

`cmd/rvw` → `internal/cli` → `internal/review` → `internal/store`

- `internal/cli` — cobra commands. Turns flags and stdin into service inputs,
  and service outputs into text. No domain logic here.
- `internal/review` — the `Service` and the domain. `operations.go` holds the
  typed input/output of every operation.
- `internal/store` — SQLite (modernc, no cgo). Schema in `schema.sql`.
- `internal/render` (text/markdown/JSON output), `internal/gitx` (git blobs
  for code snapshots and diffs), `internal/workspace` (resolve the git root).

## Invariants

- `test/rvw.bats` is the CLI contract. A behavior change starts there.
- Every failure is one `rvw: ...` line on stderr and exit status 1. Notices
  go to stderr; stdout carries only the result.
- Every operation input and output is a JSON object that infers an MCP schema
  (`internal/review/schema_test.go`). Add new operation types to that test.
- A field without `omitempty` is required; its `jsonschema` tag is its
  description.
- Configuration is flags only (`--workspace`, `--lane`, `--author`, `--db`).
  Do not add environment variables.
- A pulled comment leaves the queue. Lane scoping is strict: a pinned pull
  never takes another lane's or an unlaned comment.

## Conventions

- Conventional commits: `feat:`, `fix:`, `test:`, `docs:`, `feat!:` for
  breaking changes.
- `skills/rvw/SKILL.md` teaches agents the CLI. When a command or flag
  changes, update it with the README.
