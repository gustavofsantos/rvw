# Contributing

Thanks for helping with rvw. Issues and pull requests are welcome. For a
large change, open an issue first so we can agree on the behavior.

## Setup

You need Go (the version in `go.mod`), git, and for the CLI tests
[bats](https://github.com/bats-core/bats-core), `jq` and `sqlite3`.

```sh
go build -o bin/rvw ./cmd/rvw
go vet ./...
go test ./...
LC_ALL=C.UTF-8 bats test/rvw.bats   # builds its own binary
```

`make test` runs all of them. CI runs them on Linux and macOS, with the race
detector, on every pull request.

When you run `bin/rvw` by hand, pass `--db` with a scratch file. The default
database is your real queue.

## Ground rules

- `test/rvw.bats` is the CLI contract. A behavior change starts with a
  test there.
- Every failure is one `rvw: ...` line on stderr and exit status 1. stdout
  carries only the result.
- Configuration is flags only. Do not add environment variables.
- A new operation gets a tool in `internal/mcpserver` and an entry in
  `internal/review/schema_test.go`.
- A command or flag change updates `README.md`. An MCP tool change updates
  `skills/rvw/SKILL.md`.
- TUI golden views live in `internal/tui/testdata/`. Regenerate them with
  `go test ./internal/tui -update` and review the diff.
- Run `gofmt` and `go mod tidy` before you push. CI checks both.

`CLAUDE.md` describes the package layout.

## Commits and pull requests

Use [conventional commits](https://www.conventionalcommits.org/): `feat:`,
`fix:`, `docs:`, `test:`, `chore:`, and `feat!:` for a breaking change. The
release changelog is built from them, so write the subject for a user of rvw.

## Releasing

Maintainers cut a release by pushing a tag. See [RELEASING.md](RELEASING.md).
