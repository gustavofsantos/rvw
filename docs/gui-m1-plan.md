# GUI M1 plan: review desk

Branch `feat/gui-m1`. Scope: [gui-v1-product-brief.md](gui-v1-product-brief.md)
milestone M1. Look: [gui-v1-design.md](gui-v1-design.md).

## Architecture

- `rvw-view` (new, pure): view logic with no drawing and no windowing. Fuzzy
  matcher and file tree (moved from `rvw-tui`), the key-strip state, the queue
  grouping, the shell counters. Everything here has unit tests.
- `rvw-gui` (new, GPUI): window, theme tokens and the screens. One `Service`
  call per action, no domain logic, as in the other adapters.
- The CLI, TUI, MCP and GUI keep one contract: the `Service` operations.

## Slices

Each slice ends green (`cargo test`, `cargo clippy`) and is one commit.

| # | Slice | Proof |
|---|---|---|
| 0 | Extract `fuzzy` and `tree` into `rvw-view` | existing tui tests still pass |
| 1 | `rvw-view::keys`: the 13 keys, their groups and the lit state per context | unit tests on each context |
| 2 | `rvw-view::queue`: group comments by review, state, lane; header counters | unit tests on a fixture queue |
| 3 | `rvw-gui` window: theme tokens and the shell (top bar, rail, panels, key strip) with real counters | opens; `cargo check` |
| 4 | Queue screen on real data | view-model tests; manual run |
| 5 | Changed files and the read-only diff | view-model tests; manual run |
| 6 | Comments on the diff: show, compose, add to draft, submit | `Service` calls asserted |
| 7 | File tree, find a file, context comment | fuzzy and tree tests; manual run |

Out of M1: accept and reopen (M4), staging and commit (M2), history (M3).

## Working rules for the agents

- A subagent edits only the files named in its task and never commits.
- The orchestrator reviews the diff, runs the checks and commits.
- Tests come first for view logic. The GUI layer is checked by compiling and
  by a manual run, because a window is not a test seam.
- Review feedback goes through `rvw` against a scratch database, never the
  user's real queue.
