# GUI M7 plan: lanes

Status: planned on 2026-10-15, after M6. Not started.
Scope: the first half of the lanes direction in "rvw: decisões e direção do
produto" (Claude Docs, private). M8 takes the second half (below).
Look: boards E and F of the lanes canvas
(https://claude.ai/artifact/4P2qZLjRTufXJnp6nNNDJW, private). Boards A to D
are the alternatives that were not taken.

*Exit:* the user works for a week in one project with at least two lanes in
at least two repositories, switches lanes from the sidebar, and opens no other
terminal emulator and no second rvw window.

## What changes

The v1 shell is one workspace, a rail of screens and a terminal dock. M7
replaces it with three columns:

- **Left:** projects, each with its lanes. The open project is expanded; the
  others are collapsed.
- **Centre:** the open lane's tabs: its terminals, and the files and wide views
  opened from it.
- **Right:** the lane's Files, Changes, Review and History, and its commands.

A lane is the main object. The agent and the review are views of it. This
replaces rules 1 and 2 of [gui-v1-design.md](gui-v1-design.md); slice 12
records the amendment there.

## Words

- **Project:** a TOML file the user keeps outside every repository. It names
  repositories, lanes, terminals and commands. It is called a project, never a
  workspace, so that "workspace" keeps its one meaning.
- **Workspace:** unchanged. The root of a git worktree, the key of the queue,
  `--workspace` in the CLI and the `workspace` field in MCP.
- **Lane:** one git worktree of one repository in a project. Each lane is a
  workspace. The `lane` field of a comment is unchanged and orthogonal; the
  GUI does not set it for project lanes.
- **Fact:** a passive statement about a lane ("2 comments open", "↑2",
  "conflict"). Facts never form steps, a progress bar or a "next step".

## Decisions

Settled with the user on 2026-10-15:

- **M7 and M8 come after M6.** M5 and M6 are built on the v1 shell. Code that
  M5 or M6 add to `rvw-view` takes a workspace (one worktree) or a branch, not
  the window's state, so History and the audit export move into a lane without
  a rewrite.
- **The manifest is a project.** `--workspace` and the MCP contract do not
  change.
- **Two milestones.** M7: projects, lanes as worktrees, the three columns,
  terminals per lane, facts from git, the queue and the terminals. M8: stacks,
  reconcile, agent hooks, the MCP snapshot and intents.

Design, proposed in this plan:

- **No core change.** A lane is a worktree, and a worktree is already a
  workspace of its own (`rvw-git::worktree`). An agent started in a lane's
  terminal resolves the lane's worktree as its workspace, so its pulls and
  decisions land in that lane's queue. No new operation, no MCP tool, no
  schema change. `test/rvw.bats` and `test/reference` do not change.
- **Project files.** One file per project in `$XDG_CONFIG_HOME/rvw/projects/`,
  else `~/.config/rvw/projects/`, on every OS, as the database follows
  `$XDG_DATA_HOME`. A `--config DIR` flag on `rvw gui` overrides the
  directory. No new environment variable. The AGENTS invariant becomes
  "configuration is flags and the project files".
- **Strict format.** An unknown key, a lane whose `repo` is not listed, two
  lanes with one name, a missing repository path, or an `after` that names no
  lane or makes a cycle is one error line naming the file and the key; the
  project does not open. `after` and `[hooks]` are read and kept in M7 but do
  nothing until M8; the sidebar shows `after` as the `↳` indent.
- **No project file, no change.** `rvw gui` in a repository with no project
  opens an implicit project of that one repository, with its existing
  worktrees as lanes. Nothing is written until the user creates a lane.
- **The main worktree is a lane.** Each repository's main checkout is listed
  as a lane named after its branch, marked "main". Lanes rvw creates live
  outside every repository, beside the database:
  `$XDG_DATA_HOME/rvw/lanes/<project>/<repo>/<lane>`, else
  `~/.local/share/rvw/lanes/…`.
- **New lane.** A new branch named after the lane, at the repository's HEAD
  (or at the `after` lane's branch). rvw adds the `[[lane]]` entry to the
  project with `toml_edit`, which keeps the user's comments and order.
- **Archive lane.** Takes a checkpoint, then removes the worktree and the
  `[[lane]]` entry. The branch stays. A dirty lane or one with running
  terminals asks first, naming what would be lost.
- **Facts in M7** come only from what rvw can see without hooks:
  - git: files changed, ↑/↓ against the upstream, a merge or rebase in
    progress, a conflict;
  - the queue: comments open (pending or pulled), decisions awaiting a
    verdict;
  - the terminals: a process running, a process exited with its status, a
    terminal that asked for attention (OSC 9 or OSC 777, already in
    `rvw-term::notify`);
  - reading: files not yet read.

  "Waiting for you" (the orange dot) is a conflict, a decision awaiting a
  verdict, or a lane terminal that asked for attention and is not on screen.
  An attention request is the program's own signal, not inference. Richer
  agent state ("claude stopped") is M8, from hooks: nothing is inferred from a
  terminal's output.
- **Read marks.** A file counts as read for a lane when the user marks it (or
  scrolls its whole diff); the mark keeps the file's blob id, so a later
  change makes it unread again. Marks, the last tab of each lane and the
  collapsed groups live in a GUI state file in the data directory, one per
  project. Losing it loses only these marks.
- **Order.** Projects and lanes keep the order of the project file. Nothing is
  sorted by urgency.
- **Centre tabs.** On the first open a lane shows its first terminal; after
  that it returns to the last tab used. A fact never switches the tab. Wide
  views (Review desk of a file, History, the queue) open as centre tabs.
- **Right panel.** Lists whose items open in the centre: Files (changes against
  the base, read and unread), Changes (staging and the commit box, from M2),
  Review (draft, then decided comments with Accept and Reopen, from M4),
  History (the lane's log; the graph opens in the centre). Its header runs the
  project's `[commands]` (`run`, `push`, `pr`) in a new terminal tab of the
  lane. Push from the GUI keeps the M4 gate card.
- **Terminals.** The dock becomes the centre. Each lane owns its terminals and
  keeps them while hidden. A lane's `terminals = ["claude", "repl"]` names
  entries of the project's `[terminals]` table (`claude = "claude"`); they
  start when the lane is first opened, never when the app starts. The quit
  guard names the running terminals of every lane. Terminals and project
  commands get the shell environment the GUI already reads for git.
- **The Git log tab** stays one log for the window. Each entry gains the lane
  whose worktree it ran in; the palette opens it in the centre of any lane.
- **The GUI is in daily use.** Every slice leaves it usable for the day's
  work. Slice 7 swaps the frame only when the implicit project already gives
  today's one-repository flow inside it: the same review desk, changes, history
  and terminals, one lane.
- **Keys.** `⌘1`..`⌘9` stay the centre tabs. `⌥⌘↑`/`⌥⌘↓` move to the
  previous or next lane, `⌘L` finds a lane, `⌘N` creates one. The key strip
  stays under the centre column.

Project file, as M7 reads it:

```toml
name = "payments-platform"

[repos.proto]
path = "~/work/proto"
[repos.api]
path = "~/work/api"

[terminals]
claude = "claude"
repl   = "cargo watch -x test"

[commands]
run  = "make run"
push = "git push -u origin HEAD"
pr   = "gh pr create --fill"

[[lane]]
name  = "likes-endpoint"
repo  = "api"
after = "like-schema"          # read in M7, acted on in M8
terminals = ["claude", "repl"]
```

## Slices

Each slice is one commit and ends green. "Orch." means the orchestrator.

| # | Slice | Files owned | Proof |
|---|---|---|---|
| 0 | Contract (Orch.) | new crate `rvw-project` (types, stubs), AGENTS invariant, this file | build, lint |
| 1 | Read a project | `rvw-project` | every refusal in the strict format; implicit project; `~` expansion |
| 2 | Write a project | `rvw-project` (toml_edit) | add and remove a lane keep comments and order |
| 3 | Lane worktrees | `rvw-git::worktree`, `rvw-git::lanes` | create at HEAD or at a branch; archive takes a checkpoint first; refuses a dirty lane without consent |
| 4 | Sidebar model | `rvw-view/src/lanes.rs` | groups, file order, `↳` depth, dot state |
| 5 | Facts | `rvw-view/src/facts.rs` | one table test per fact; "waiting for you" rule; fixed wording |
| 6 | Read marks and GUI state | `rvw-view/src/reading.rs`, `rvw-gui` state file | a changed blob is unread again; a lost file opens clean |
| 7 | Three-column frame | `rvw-gui::frame`, `shell` | manual run; the rail is gone |
| 8 | Centre tabs and terminals per lane | `rvw-gui::{terminal_dock, shell}` | manual run; switching lanes keeps terminals; quit guard lists every lane |
| 9 | Right panel and commands | `rvw-gui` (files, changes, review, history panels) | manual run; push keeps the gate card |
| 10 | New and archive lane | `rvw-gui`, palette | manual run; the project file keeps its comments |
| 11 | Projects, lane keys, last lane | `rvw-gui::{last_workspace, palette_overlay}` | manual run |
| 12 | Docs | README, gui-v1-design.md amendment, brief | `make check` |
| 13 | Close-out | this file | `make check` |

Slices 1, 3, 4 and 5 run in parallel after 0, on disjoint files. 2 follows 1;
6 follows 5. 7 to 11 are GUI slices and run one at a time.

## Open

1. Is `--config DIR` the right flag name, given that `config.toml` (fonts,
   sizes) will live in the same directory?
2. Should read marks move into the core later? M6's audit export could want
   "a human read this file" as evidence; a GUI state file cannot give that.
3. A repository listed in two projects: M7 opens both and warns. M8's
   reconcile needs "one active project per repository" enforced.
4. What a lane shows when its worktree directory is gone (deleted by hand):
   a "missing" fact with Restore and Archive, or nothing.

## M8, in one paragraph

Stacks and the agent. `after` becomes a stack edge: when a base lane moves or
is merged (squash included, found by patch id), `rvw reconcile` rebases a lane
that is idle and clean, under a lock per repository; a conflict becomes a fact
and the agent is not interrupted. Hooks emit normalized events into an events
table with a `seq` cursor (one adapter per agent), which gives agent state
facts. MCP gains a read-only snapshot of what the window shows and intents
(rebase this lane, propose a project) that the user approves. A global undo
restores branches from the reflog of rvw's own operations. The lanes board
(alternative C) returns as a zoom-out on `⌘0`. M8 gets its own plan when M7
is done.
