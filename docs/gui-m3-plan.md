# GUI M3 plan: navigate history

Branch `feat/gui-m3`, built in its own worktree (`../rvw-m3`) so the M2 build
the user runs every day is never touched. Scope:
[gui-v1-product-brief.md](gui-v1-product-brief.md) milestone M3 (B5 to B7,
B10 to B14). Look: board 6, "History with provenance", in
[gui-v1-design.md](gui-v1-design.md).

*Exit:* the user no longer opens another Git client for history, search,
worktrees, cherry-picks or tidying local commits.

## Architecture

As in M2: git reads and actions live in `rvw-git`, pure view logic with unit
tests in `rvw-view`, drawing and wiring in `rvw-gui`. None of it is a
`Service` operation; the CLI and MCP do not change.

| Module | Holds |
|---|---|
| `rvw-git::history` | a page of the log, one commit's files and diffs, two revisions compared (B5, B7) |
| `rvw-git::blame` | the tree and a file at a revision, a file's history across renames, blame (B5) |
| `rvw-git::search` | commits by message, author or content (B6) |
| `rvw-git::worktree` | list, add, remove (B10) |
| `rvw-git::merge` | merge, cherry-pick, the operation in progress, abort, continue (B11, B13) |
| `rvw-git::rewrite` | reword, squash, drop on unpushed commits (B14) |
| `rvw-view::graph` | lanes of the history graph, one row at a time |
| `rvw-gui` | History screen on rail H, commit detail, file history and blame, search, worktree picker, palette actions, the B14 card, external tools (B12) |

## Decisions

- **Slow reads are paged and stoppable.** Log, file history, blame and search
  run on the background executor, read a page (`--skip`, `-n`) at a time, and
  take a stop flag of their own (`gitx::run_stop`), so a search the next
  keystroke replaces is killed rather than left running. The graph is laid out
  incrementally, so a new page appends rows without redoing the old ones.
- **Log order is `--topo-order`.** It keeps a branch's commits together, which
  the graph needs; git streams it from the commit-graph without walking all
  history first.
- **Provenance and the ledger stay out.** The History board's review column,
  the "Made by" filter and the story panel need D2's content matching (M6) and
  agent reports (M5). M3 draws those controls in place and dim (design rule
  9), with the reason on hover. No commit is linked to a review by time window
  or SHA.
- **B14 does not touch the working tree unless it must.** Reword and squash
  keep every later commit's tree, so they write new commits with
  `commit-tree` and move the branch with a compare-and-swap `update-ref`: the
  index and the files, which an agent may be editing, are never touched. Drop
  replays later commits with `merge-tree --write-tree`; nothing is written
  until every commit replays cleanly, and a conflict aborts with the file
  named. When the new HEAD's tree differs, `read-tree -m -u` moves the index
  and the files from the old tree to the new one, and refuses when a local
  change would be overwritten. A checkpoint is taken first, as for every
  action.
- **Editable commits** are the first-parent chain from HEAD back to the first
  commit that is on a remote (`rev-list HEAD --not --remotes`) or is a merge.
  Detached HEAD and an operation in progress refuse.
- **"Commit dropped" markers wait for M6.** Reviews are keyed by file blob
  versions, not SHAs, so a reword or squash orphans nothing today.
- **Merge and cherry-pick conflicts** leave the operation in progress. The
  Changes screen names the operation and the conflicted files, and offers
  "Open merge tool", "Continue" (when nothing is conflicted) and "Abort". The
  checkpoint taken before is the fallback. No conflict UI (brief, B11).
- **Worktree remove** refuses a worktree with changes (no force from the GUI),
  and deletes that worktree's checkpoint refs with it. This closes M2's open
  item 6.
- **Worktree switching reuses the workspace picker**, since a worktree is a
  workspace: the picker lists the repository's worktrees first.
- **The B14 confirmation reuses the checkpoint restore card**: the list of
  commits before and after, then Apply.
- **The key strip keeps its thirteen keys.** M3's actions are in ⌘K. The
  History screen's commit search is focused with `/` while the list has focus,
  and from ⌘K ("Search commits"). The board's ⌘F is not used: it stays free
  for find-in-file.

## Slices

Each slice ends green (`cargo test`, clippy, `fmt --check`) and is one commit.

| # | Slice | Proof |
|---|---|---|
| 1 | `rvw-git::history`: log page, commit detail, commit diff, compare two revisions | temp-repo tests |
| 2 | `rvw-git::blame`: tree and file at a revision, file history with renames, blame | temp-repo tests |
| 3 | `rvw-git::search` | temp-repo tests, incl. a stopped search |
| 4 | `rvw-git::worktree` | temp-repo tests |
| 5 | `rvw-git::merge` | temp-repo tests with conflicts |
| 6 | `rvw-view::graph` | unit tests on linear, merge, octopus, two tips |
| 7 | `rvw-git::rewrite` | temp-repo tests: dirty tree kept, conflict leaves no trace, race refused |
| 8 | History screen: graph, list, detail, diff of a file in a commit | manual run |
| 9 | Search, file history and blame, historic tree | manual run |
| 10 | Compare any two revisions (extends the compare picker) | manual run |
| 11 | Worktree picker, merge and cherry-pick actions, conflict state | manual run |
| 12 | B14 card and actions | manual run |
| 13 | External tools (B12), close-out notes | `make check` |

Slices 1 to 6 run in parallel, one Haiku agent each, on disjoint files.

## Working rules for the agents

- A subagent edits only the files named in its task and never commits.
- The orchestrator sets up shared files (`lib.rs`, `Cargo.toml`, `mod` lines)
  before agents run, reviews each diff, runs the checks and commits.
- Each agent builds in its own `CARGO_TARGET_DIR` under the main checkout's
  `target/`, never under `/tmp`, and only the crate it works on. Only the
  orchestrator builds `rvw-gui`.
- Non-GUI crates compile on Rust 1.93.
- Tests come first for git and view logic. The GUI layer is checked by
  compiling and by a manual run.

## Status at the end of the session

Slices 1 to 13 are done and the checks are green: clippy (`-D warnings`),
`fmt --check`, `cargo test --workspace`, the doctests and `bats test/rvw.bats`.
Nothing has been looked at in a window yet: GUI slices were checked by
compiling, unit tests on their pure parts, and a run of the binary that stays
up.

| Done | Where |
|---|---|
| Log pages, commit detail and diffs, compare revisions | `rvw-git::history` |
| Tree and file at a revision, file history, blame | `rvw-git::blame` |
| Commit search, stoppable | `rvw-git::search` |
| Worktrees, with their checkpoint refs removed | `rvw-git::worktree` |
| Merge, cherry-pick, abort, finish | `rvw-git::merge` |
| Reword, squash, drop | `rvw-git::rewrite` |
| Graph lanes, history rows and labels | `rvw-view::{graph, history}` |
| History screen, search, blame, browse a commit's files, compare | `rvw-gui::{history_state, history_screen, history_files}` |
| Cherry-pick, merge, conflict panel on Changes | `rvw-gui::history_actions`, `changes_screen` |
| Worktrees in ⌘O, new and remove | `rvw-gui::{workspace_picker, worktree_ui}` |
| History edit card | `rvw-gui::rewrite_card` |
| Terminal and diff tool | `rvw-gui::external_tools` |
| Palette actions for all of it | `rvw-view::palette` |

Open, in priority order for the first person who runs it:

0. First run by the user (2026-10-13): everything works, but the window is
   laggy. Items 3 and 4 below are the first suspects; measure before fixing.

1. Look at the History screen, the blame, the compare panel, the conflict
   panel and the history edit card: layout, focus, keys.
2. Edit history on a real branch: reword with an agent editing, then drop a
   commit; restore the checkpoint taken before.
3. The History list does not refresh live when an agent commits; it re-reads
   when the screen opens and after rvw's own actions.
4. Merge, cherry-pick, worktree add and remove, and the commit-file list read
   or write git on the UI thread.
5. The review screen still compares only with its three bases; comparing two
   revisions lives on the History screen.
6. `make check` with `RUSTC` pointed at the rustup toolchain fails the rvw-cli
   doctest (rustdoc 1.93 against crates built by 1.99). Run it without that
   override, or clippy alone with it.
