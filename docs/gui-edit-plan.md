# GUI plan: select by gutter, edit in place

Status: approved on 2026-10-15 (decisions below). Built before M7's frame.
Scope: not a milestone of the v1 brief. It sits beside M7 and touches the
diff list, the whole-file view and the selection model.

*Exit:* the user reads a file in rvw, sees a typo or a missing line, clicks
into the code, fixes it, saves, and never opens another editor for it. A
comment on lines is still one drag over the number column.

## Problem

Today a press in the code selects whole lines (`rvw-view/selection.rs`,
`diff_list/drag.rs`). That makes the code area unusable for anything else: no
caret, no text selection, no copy of a word, no typing. A quick edit means
leaving rvw, finding the file again in an editor and finding the line.

## What changes

- **The gutter selects lines for comments.** Click a line number: one line.
  Drag over the number column: a range, with the edge auto-scroll that exists
  today. Shift-click extends. The selection model and the composer are
  unchanged; only the surface that drives them moves.
- **The code area is text.** Click places a caret, drag selects characters,
  double-click a word, copy works. In the File view it also edits.
- **Edit mode is the File view of a file in the open worktree.** The Changes
  (diff) view stays read-only, because a removed line has nowhere to type.
  Its code area gets caret and text selection for copying; a key or a click
  on "Edit" switches to the File view at the same line.
- **Save is explicit.** `⌘S` writes the file; a dirty dot shows on the tab and
  the row. No autosave: an agent may be writing the same tree.

## Decisions proposed

1. **One buffer type.** Reuse `rvw_view::editor::Editor` (cursor, anchor,
   undo, word motions, coalesced typing). It is a whole-text buffer; the
   file cap stays 1 MiB, which it handles. The view stays a virtualised
   list of lines (the one the diff uses), not a `TextArea`, which lays out
   everything.
2. **Highlighting follows edits.** The existing highlight task re-runs
   debounced after a change; until it arrives, the edited line draws plain.
3. **Disk changes are never clobbered.** The buffer remembers the blob id it
   was loaded from. If the file changed on disk (`live.rs` already notices),
   a clean buffer reloads silently and a dirty one shows a card: Keep mine,
   Take theirs, Show diff. Saving over a changed file asks the same way.
4. **Comments follow by their snapshot.** rvw already stores the code a
   comment was written on and re-finds it in a changed file. A save is one more
   change to the file; nothing special is done to comments.
5. **What it is not.** No LSP, no multi-file buffers, no find and replace, no
   multi-cursor, no formatting on save. `⌘F` find in file is the one extra,
   and only if it is cheap. Larger edits belong in the user's editor; an
   "Open in editor at this line" action (the tool config exists in
   `external_tools.rs`) is the way out.
6. **Read-only places.** Binary or over-cap files, History and commit views,
   Compare against another revision, files outside the worktree.
7. **Keys.** Plain arrows, Home/End, word and line motions, `⌘Z`/`⇧⌘Z`,
   `⌘S`, `Esc` leaves the caret. The reviewer's line-selection keys keep
   working on the selection, not on the caret.

## Slices

Each slice is one commit and ends green. "Orch." is the orchestrator.

| # | Slice | Files owned | Proof |
|---|---|---|---|
| 0 | Contract (Orch.) | this file, `rvw-view` `lib.rs` stubs | build, lint |
| 1 | (folded into 5: `Selection` is already position-based; only the surface changes) | - | - |
| 2 | Text position model: pixel and column to (line, byte), char selection over lines | `rvw-view/textpos.rs` | tests: tabs, wide chars, end of line, past end |
| 3 | File buffer: `Editor` over the file with line index, dirty, loaded blob id, save plan | `rvw-view/filebuf.rs` | edit, undo, dirty clears on save, changed-on-disk states |
| 4 | Disk write: atomic save, refuses when the blob moved | `rvw-view/filesave.rs` | temp-dir tests: write, changed under it, permissions error text |
| 5 | GUI gutter: line numbers drive selection; code press stops selecting lines | `rvw-gui::{diff_view, diff_list, drag}` | manual run |
| 6 | GUI text: caret, character selection and copy in both views | `rvw-gui::{diff_view, file_view}` | manual run |
| 7 | GUI edit: typing, undo, save, dirty dot, disk-change card, Edit action | `rvw-gui::{file_view, shell}` | manual run |
| 8 | Docs and keys | README, menu drawer, gui-v1-design amendment | `make check` |

Slices 1 to 4 are pure and run in parallel on disjoint files, beside M7's
non-GUI slices. 5 to 7 are GUI slices and run one at a time; they run before
M7's frame slices (7 to 11), which then build on them.

## Settled (2026-10-15, user)

1. **Gutter-only line selection.** The code area is text; a click in it no
   longer selects a line.
2. **No editing in the diff view.** "Edit" switches to the File view at the
   same line.
3. **Order: before M7's frame.** Slices 1 to 7 run before M7 slices 7 to 11.
   M7's pure slices (1 to 6) continue beside the pure slices here.
4. **Save is `⌘S` only.** No save on focus loss.
