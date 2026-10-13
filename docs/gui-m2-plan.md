# GUI M2 plan: daily commit loop

Branch `feat/gui-m2`. Scope: [gui-v1-product-brief.md](gui-v1-product-brief.md)
milestone M2 (B1 to B4, B8, B9, A6). Look: board 5, "Changes and commit", in
[gui-v1-design.md](gui-v1-design.md).

*Exit:* the user stages, commits and pushes from rvw for a full week, and sees
the agent's edits land live.

## Architecture

- Git operations live in `rvw-git`, one module per area: `index` (status, the
  two staging diffs, stage, unstage, apply a patch to the index, write and read
  a tree), `commits` (commit, amend, discard), `stash`, `refs` (branches and
  tags), `remote` (fetch, pull, push). They are not `Service` operations: git
  is not review domain, and the CLI and MCP do not need them.
- Pure logic lives in `rvw-view`, with unit tests: `patch` (a patch for a hunk
  or for chosen lines), `checkpoints` (the list and the restore confirmation), `commit_screen` (the
  Changes screen's lists, counts and notices).
- `rvw-gui` draws the Changes screen and wires each action to one call.

## Decisions

- **Staging diffs.** Unstaged is index to worktree (`git diff`), staged is HEAD
  to index (`git diff --cached`). Status is `status --porcelain=v2 -z`. The
  review screen's "disk against a commit" diff is not reused.
- **Line staging** builds a patch from the raw diff text and applies it with
  `git apply --cached` (`--reverse` to unstage) after `--check`. Untracked,
  binary, renamed and mode-changed files stage as whole files only.
- **Checkpoints, not undo** (changed on 2026-10-13, at the user's request).
  The first design undid the newest action and refused when the repository
  had moved since. With an agent editing in parallel that refusal is the
  common case, so undo was unpredictable. Now `rvw-git::checkpoint` saves the
  whole worktree state (branch and commit, the index, every file not ignored)
  as git objects under `refs/rvw/checkpoints/<worktree>/` (shared refs, so `gc`
  in any worktree keeps them), before every action
  that changes the repository and on demand. Restoring always works: it saves
  the current state first, then puts back the branch (moving it back over
  newer commits, which the user chose), the index and the files. Refs and
  stashes an action deletes are kept in the checkpoint before it and
  recreated when missing. Checkpoints outlive the session.
- **Amend** is offered only when HEAD is on no remote branch: amending a pushed
  commit would need a force push, and v1 has none.
- **Commit** always reads the message from stdin (`commit -F -`), so no editor
  opens. A hook's failure shows as one error line.
- **Network** runs off the UI thread and can be cancelled. Pull is
  `--ff-only`; push never forces. Every git child gets
  `GIT_TERMINAL_PROMPT=0` so a credential prompt cannot hang unseen.
- **Live tree (A6)** polls, once a second, `git status` (with
  `GIT_OPTIONAL_LOCKS=0`, so rvw never takes the index lock from an agent) and
  the database file and its `-wal`. Polling has no watch limit and no new
  dependency. A refresh keeps the selection, the open composer and the commit
  message.
- **Open comments** that touch a hunk are shown on the Changes screen and never
  block. A recorded override belongs to the commit gate (C2, M4).
- **Keys.** With a text field focused, `S` types and `⌘Z` is the field's undo;
  otherwise `⌘Z` opens the restore of the newest checkpoint.

## Slices

Each slice ends green (`cargo test`, clippy, `fmt --check`) and is one commit.

| # | Slice | Proof |
|---|---|---|
| 1 | `rvw-git::index`: status, unstaged and staged diffs, stage and unstage paths, apply to the index, write and read tree | temp-repo tests |
| 2 | `rvw-view::patch`: patch for a hunk or chosen lines, both directions | tests that apply the patch with git |
| 3 | `rvw-git::commits`: commit, amend, HEAD pushed or not, discard and restore | temp-repo tests |
| 4 | `rvw-view::undo`, removed when checkpoints replaced it | — |
| 4b | `rvw-git::checkpoint`, then the GUI swap and removing `undo` | temp-repo tests incl. worktrees and gc |
| 5 | `rvw-view::commit_screen`: lists, counts, commit label, open comments per hunk | unit tests |
| 6 | Changes screen: lists, diff, stage file, hunk and line, commit, amend, recent actions with undo | manual run |
| 7 | `rvw-git::stash` and stash actions | temp-repo tests; manual run |
| 8 | `rvw-git::refs`: branch create, delete, rename, track; tags | temp-repo tests; manual run |
| 9 | `rvw-git::remote`: fetch, pull, push, publish, in the background | tests against a bare remote; manual run |
| 10 | Live tree: poll and refresh without losing work in progress | unit tests on the change test; manual run |
| 11 | Close-out: key strip and palette audit, notes | `make check` |

## Later

- **Command log.** Every git call goes through `gitx::exec`, so a panel that
  lists what rvw ran in the repository, with its output, needs only a hook
  there. Asked for by the user; not in M2.

## Working rules for the agents

- A subagent edits only the files named in its task and never commits.
- The orchestrator sets up shared files (`lib.rs`, `Cargo.toml`, `mod` lines)
  before agents run, reviews each diff, runs the checks and commits.
- Each agent builds in its own `CARGO_TARGET_DIR=target/agent-<name>`, never
  under `/tmp`.
- Tests come first for git and view logic. The GUI layer is checked by
  compiling and by a manual run.
