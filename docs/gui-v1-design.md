# rvw GUI v1: interface design

Status: locked on 2026-10-12. Changes need a reason written here.
Companion to [gui-v1-product-brief.md](gui-v1-product-brief.md).
Source boards: [docs/design/gui-v1/](design/gui-v1/) (Design canvas files).
Live canvas: https://claude.ai/artifact/QYsNciHKbFnyuwgeF8xEZA (private).

## Character

Calm, paper-like, ink first. Three influences, each used for one thing:

- **Dieter Rams, early Apple**: as little design as possible. Structure comes
  from spacing and hairlines, not boxes, chips or fills. Color marks meaning,
  never decoration.
- **Jony Ive**: depth through layers. A paper window with one soft shadow,
  translucent side panels, rounder controls, light type weights.
- **Teenage Engineering**: a static surface that lights up. Controls sit in
  fixed places and are lit only when the context allows them. The product
  must stay honest: every control is labelled, discoverable and standard.

## Tokens

Palette is the `cursorized` light variant from the author's dot-files.

| Role | Value |
|---|---|
| Canvas behind the window | `#e6e5df` |
| Paper (window, reading area) | `#f7f7f4` |
| Side panels | `#eeede7` at 75%, backdrop blur 20px |
| Hairline | `#e4e3dc` (strong `#dbd9d2`, control `#c9c7be`) |
| Selected row | `#dedcd4` (panel) or `#e4e3dc` (paper) |
| Ink | emphasis `#161614`, text `#31302d` |
| Muted / faint | `#696761` / `#8a8882` |
| Pending | yellow `#846600` |
| Pulled, agent | violet `#7856c0` |
| Done | green `#007b2f` |
| Rejected | red `#ba3e4b` |
| Needs you, caret | orange `#f54e00` |
| Diff tint | green `rgba(0,123,47,.09)`, red `rgba(186,62,75,.09)` |

Type: Hanken Grotesk (UI), JetBrains Mono (code, ids, keys, paths). UI 13.5px,
labels 13.5px muted, code 12.5px / 21px line, keys 11px. Weights 400 and 500
only. Titles 22px, 500.

Shape: window 14px, controls 8px, selected rows 6px. One shadow:
`0 1px 2px rgba(22,22,20,.08), 0 14px 44px rgba(22,22,20,.14)`. Spacing on a
4px grid, panel padding 24px.

## Rules

1. **One shell on every screen.** Top bar 48, rail 56, left panel 300,
   right panel 340, key strip 56. Only the content changes.
2. **The review is the main object.** The queue is always one glance away.
3. **Ink first.** Color marks meaning, never decoration.
4. **One status vocabulary**, color and shape: `○` pending, `◐` pulled,
   `✓` done, `✗` rejected. Shape carries the meaning when color cannot.
5. **Violet is an agent. Orange is "you are needed". Nothing else is orange.**
6. **Left to right:** where am I, what changed, what is asked.
7. **Every action has a key.** ⌘K finds every action, ⌘P finds every file.
8. **Nothing destructive without a checkpoint.** Every action that changes the
   repository first saves one; any checkpoint can be restored.
9. **A control that is not available stays in place, dim.** Hovering it says
   what unlocks it.

## The shell

- **Top bar:** workspace, branch, "compared with", search hint, then three
  counters: waiting on you (orange dot), with an agent (violet), in your draft
  (yellow). The same on every screen.
- **Rail:** Review, Changes, History, Branches. Files is a tab of the left
  panel inside Review, not a rail item.
- **Left panel:** navigation for the current view (files, comments, filters).
- **Right panel:** what is asked of you, or the detail of the selection.
- **Key strip:** see below.

## The key strip

Thirteen keys, always in this order, in four groups:

| Group | Keys |
|---|---|
| Move | `J K` move, `↵` open, `O` open file |
| Review | `C` comment, `⌘↵` add, `⇧⌘R` submit, `A` accept, `R` reopen |
| Git | `S` stage, `⇧⌘C` commit, `⌘Z` restore |
| Find | `⌘P` find file, `⌘K` actions, `⌘O` open a workspace |

Each key has a light: dim (not here), dark (available), orange (needs you now).
State is also shown by label weight, not by color alone. At most one key is
orange at a time. Each action has one key; a key never changes meaning.

## Screens

| # | Board | Milestone | Purpose |
|---|---|---|---|
| 1 | Review desk | M1 | Diff with inline threads and the composer; your review on the right. |
| 2 | Outcome of one comment | M1, M4 | Asked → code as reviewed → agent's note → resolution diff, with Accept and Reopen. |
| 3 | Queue home | M1, M4 | Open work by review, filtered by state, lane, author; next-pull preview. |
| 4 | Submit a review | M1 | Verdict, summary for the agent, comments, heads-up about comments still with an agent. |
| 5 | Changes and commit | M2 | Staged and unstaged, hunk and line staging, commit box, checkpoints with restore. |
| 6 | History with provenance | M3, M5 | Graph with human/agent and review status; the story of a commit. |
| 7 | Files and context | M1 | Whole-repo tree; comment on a file outside the change as context. |
| 8 | Find a file | M1 | ⌘P: fuzzy search once you type, changed files first, preview, open beside the diff. |
| 9 | The key strip | all | The strip in three contexts. |

## Behaviour that is easy to get wrong

- **Draft first.** A comment goes into your draft review. A bare comment is
  the exception. "Submit review" is the primary action of the right panel.
- **Verdict is a judgement, not a stray click.** Accept and Reopen live in the
  queue and on the outcome page, never inline on the diff.
- **Context comments.** A comment on a file not changed on this branch is
  labelled "context". It may point at a changed location ("Points at
  service.rs:90-96"). The agent receives it with the review.
- **Open comments never block a commit.** The commit screen says which open
  comments touch the hunk and leaves the decision to the user.
- **Pulled means gone from the queue.** The queue shows what the agent holds so
  nothing is taken twice; the next-pull preview shows what a pull would take,
  per lane, and says when work waits in another lane.
- **Going back is a checkpoint, not an undo.** The commit screen lists the
  checkpoints, newest first. Restoring one always works and is shown first as
  a before and after (files, staged state, branch and the commits that leave
  it); it saves the current state as a checkpoint too, so it can be reversed.
  ⌘Z opens the newest. History edits show a before and after list.

## Core changes the design needs

| Need | Why | Milestone |
|---|---|---|
| Accept and reopen verdicts on a done comment | Screens 1, 2, 3 | M4 |
| A comment may carry a "points at" reference to another file range | Screen 7 | M4 |
| An operation for an agent to report its session | Screens 2, 6 | M5 |
| A change notification from the service | Live counters, working-tree updates | M2 |

A comment on an unchanged file needs no core change: a comment is already a
file and a line range.

## Not designed yet

Empty and error states, the actions palette (⌘K), the checkpoint list as
its own view, a dark variant (the same tokens map onto cursorized dark:
background `#1a1918`, ink `#d2cfc5`), and a compact layout for narrow windows.

## Decisions

- Light theme only in v1.
- Unified diff drawn; split is a toggle to design before M1 ships.
- Sound and motion layer: not in v1.
- The name stays `rvw`.
- **Checkpoints replace per-action undo** (2026-10-13). An undo that refuses
  whenever the repository moved since the action fails exactly when an agent is
  editing in parallel, so it is unpredictable. A checkpoint saves the whole
  worktree state (branch and commit, index, every file not ignored) and
  restores it exactly, every time. Restoring moves the branch back; the newer
  commits stay in the checkpoint taken before the restore. Decided by the
  user.
- **Decided comments are hidden in the diff** (2026-10-13). On a project with
  many comments, done and rejected threads filled the file and made it hard
  to read. The diff draws only open comments; the file header says
  "N resolved hidden · Show" and shows them for the session. The queue and
  the outcome screen still list every comment. Decided by the user.
- **The window can change workspace** (2026-10-13). One window was fixed to
  the repository it was launched in, and a launch from the Dock picks the
  busiest queue, so there was no way to reach another one. ⌘O, the palette
  or a click on the workspace in the top bar opens a picker of the
  workspaces with a queue, and "Open a folder…" for any other repository.
  Choosing one rebuilds the window for it. Asked for by the user.
- **The file finder lists nothing until you type** (2026-10-13). With an
  empty field it listed twelve files that did not fit or scroll, which
  helped no one. Asked for by the user.
- **Worktrees are switched in the workspace picker** (2026-10-13). A
  worktree is a workspace, so ⌘O lists the repository's other worktrees
  first, marked "worktree · branch". Approved by the user.
- **The history edit (B14) is a card like the restore confirmation**
  (2026-10-13): each unpushed commit is kept, reworded, squashed into the one
  below or dropped by key, then the before and after, then Apply after a
  checkpoint. Approved by the user.
