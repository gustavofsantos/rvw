# Rust rewrite plan

Goal: replace the Go implementation with Rust, keeping behavior. `test/rvw.bats`
is the contract. At the end there is no Go code. The GUI (GPUI) comes after the
rewrite merges.

## Ground rules

- Same `schema.sql` and `user_version` (1): existing databases open unchanged. No
  schema change on this branch; the GUI adds a `side` column later.
- Flags only; no new environment variables. `USER` (default author) and
  `XDG_DATA_HOME` are read exactly as the Go code does.
- Every failure is one `rvw: ...` line on stderr, exit 1. clap's own errors are
  mapped to that (no usage dump, no exit 2).
- Git is shelled out exactly as `internal/gitx` does, `--literal-pathspecs`
  included. Swapping to gix is a later refactor.
- Non-GUI crates compile on rustc 1.93 (`rust-version`).
- Status, decision, outcome and the filters stay validated strings in inputs.
- Nullable fields serialize as `null`; `omitempty` fields are skipped.
- Reference outputs from the Go binary live in `test/reference/`.

## Slices and gates

| # | Slice | Gate |
|---|---|---|
| 0 | Workspace scaffold, domain types, interfaces, reference fixtures | `cargo check` |
| 1 | `rvw-git`, `rvw-store`, `rvw-render`, `Service` | `cargo test` (ported unit tests) |
| 2 | `rvw-cli` (clap) | every bats test outside the `mcp` and `tui` sections |
| 3 | MCP server (`rvw mcp serve`, stdio and `--http`, `mcp config`) | bats `mcp` tests; tool schemas match `test/reference/mcp-tools-list.jsonl` after normalization (properties, required, types, descriptions) |
| 4 | TUI (ratatui) | bats `tui` tests; TUI unit tests; goldens regenerated and reviewed |
| 5 | Remove Go; CI, goreleaser, Makefile, AGENTS.md, README, skill | `make check` equivalent green |

`rvw mcp serve --http` is in scope: bats asserts loopback-only binding.

## Layout

`crates/rvw-core` (types, operations, errors, `Repository`, `Service`),
`rvw-git` (gitx + workspace), `rvw-store` (rusqlite), `rvw-render`, then
`rvw-cli` (binary `rvw`), `rvw-mcp`, `rvw-tui`.
