# rvw

A local, per-workspace queue of code review comments. Humans and agents add
comments on line ranges; a coding agent pulls them and records a decision on
each. See README.md for the pitch and examples.

## Commands

```sh
cargo build -p rvw-cli          # target/debug/rvw; target/ is gitignored
cargo test --workspace          # domain, store, git, render, MCP and TUI tests
bats test/rvw.bats              # CLI contract; builds its own binary
make lint                       # clippy (-D warnings, pedantic) and rustfmt --check
make fmt                        # cargo fmt --all
make audit                      # cargo audit
make check                      # lint, audit, cargo test, bats and the parity scripts
make hooks                      # once per clone: pre-push runs make check
bash scripts/setup-dev.sh       # toolchain, apt packages, cargo-audit; the web environment's setup script
bash test/help-compare.sh       # --help of every command against rvw's own capture in test/reference/help; in make check
bash test/mcp-compare.sh        # initialize and tools/list against rvw's own capture in test/reference; in make check (needs jq)
```

Releases: push a `v*` tag. `.github/workflows/release.yml` runs CI, builds one
archive per platform, and publishes the GitHub release. Archive names carry no
version (`rvw_linux_amd64.tar.gz`); `install.sh` relies on that and on
`checksums.txt`. The release build sets `RVW_VERSION` for `rvw --version`.

Run `make check` before calling a change done.

`make lint` must stay at 0 warnings. Fix the finding; do not relax a lint. An
`#[allow(clippy::...)]` carries a one-line reason and is for a finding that is
wrong in context, never for convenience.
The non-GUI crates must compile on `rust-version` (1.93, the workspace
`Cargo.toml`); CI checks it.

When you try `target/debug/rvw` by hand, pass `--db` with a scratch file. The
default database is the user's real queue.

## Layout

`crates/rvw-cli` (binary `rvw`) → `rvw-core` → `rvw-store`, with `rvw-mcp`
(`rvw mcp serve`) and `rvw-tui` (`rvw tui`) two more adapters beside the CLI.

- `rvw-cli` — clap commands and the cobra-format help renderer. Turns flags
  and stdin into service inputs, and service outputs into text. No domain
  logic here.
- `rvw-core` — the `Service`, the domain types, errors and the `Repository`
  trait. `operations.rs` holds the typed input/output of every operation.
- `rvw-store` — SQLite (rusqlite, bundled). Schema in `src/schema.sql` (v1),
  then `src/v2.sql`, applied after it to a new or an upgraded database.
- `rvw-mcp` — MCP tools over the `Service` (rmcp). One tool per operation;
  its input and output are the operation types. No domain logic here either.
- `rvw-tui` — the terminal UI (ratatui, syntect). A pure `Model` with
  `update(Event) -> Vec<Effect>` and `screen()`; `shell.rs` is the only part
  that touches the terminal. No domain logic: every action is one `Service`
  call. Golden screens live in `testdata/` and are compared byte for byte.
- `rvw-render` (text/markdown/JSON output), `rvw-git` (git blobs for code
  snapshots and diffs, shelling out to `git`; workspace resolution).
- `test/reference/` — rvw's own `--help` output and MCP tool list, captured
  from rvw, kept as fixtures for the parity scripts. The Go baseline is retired.

## Invariants

- `test/rvw.bats` is the CLI contract. A behavior change starts there.
- Every failure is one `rvw: ...` line on stderr and exit status 1. Notices
  go to stderr; stdout carries only the result.
- Every operation input and output is a JSON object that derives an MCP
  schema (`schemars`). Add new operation types to the `rvw-mcp` tests, and a
  tool for each new operation in `crates/rvw-mcp`; `test/mcp-compare.sh` diffs
  the tool list against the reference.
- A field without `#[serde(default)]` is required; its doc comment is its
  description. Nullable fields serialize as `null`, never skipped.
- Configuration is flags only (`--workspace`, `--lane`, `--author`, `--db`).
  Do not add environment variables.
- A pulled comment leaves the queue. Lane scoping is strict: a pinned pull
  never takes another lane's or an unlaned comment.
- A verdict's actor is the surface it came through (MCP = agent, CLI/TUI/GUI =
  human), never a field an adapter accepts.

## Conventions

- Conventional commits: `feat:`, `fix:`, `test:`, `docs:`, `feat!:` for
  breaking changes.
- `skills/rvw/SKILL.md` teaches agents the MCP tools only; the CLI is for
  people. When a tool changes, update the skill; when a command or flag
  changes, update the README.
- `bats` needs `sqlite3` and a UTF-8 locale (`LC_ALL=C.UTF-8`) for a few tests.
