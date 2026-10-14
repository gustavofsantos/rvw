---
name: rvw
description: Work the local rvw code review queue. Use when the user asks to pull, check, address or work through review comments or feedback, mentions rvw, or asks you to review code and leave comments for another agent.
allowed-tools: mcp__rvw__pull, mcp__rvw__list, mcp__rvw__list_reviews, mcp__rvw__count, mcp__rvw__show_comment, mcp__rvw__show_review, mcp__rvw__resolve, mcp__rvw__add, mcp__rvw__submit, mcp__rvw__remove, mcp__rvw__judge, mcp__rvw__gate
---

# rvw

`rvw` is a local queue of code review comments, keyed by workspace (the git
root of the current directory; a directory outside git has no queue). Each
comment points at a file, a line range and the code as it stood when the
comment was written.

You work the queue through the rvw MCP tools (`mcp__rvw__pull`,
`mcp__rvw__resolve`, ...). If they are not connected, tell the user and stop:
`rvw mcp config` prints how to connect them. (rvw also has a CLI; it is for
the user.)

- Pass your project directory, as an absolute path, as `workspace` on every
  call.
- Pass `author` (your name, e.g. `claude`) on `add`, `submit`, `resolve` and
  `judge`.
- Wait for each call to return before the next one. The server runs calls
  concurrently, so a `pull` sent alongside an `add` may not see it.
- A failed call returns a tool error whose text says what to fix.

## Address review comments

1. Call `pull`. Pulled comments leave the queue, so pull once and keep the
   result.

   ```json
   {"workspace": "/abs/project"}
   ```

   - Set `peek: true` to look without taking anything.
   - Set `lane` when the user names a lane or branch. A pinned pull takes
     only that lane. If it comes back empty, `elsewhere` says whether work
     waits in other lanes. Report that to the user; do not pull the other
     lane on your own.
   - The result holds comments in two places. Work both:
     - `comments`: standalone comments.
     - `reviews[].comments`: comments linked to a submitted review. Each
       review also has a `decision` and a `summary`; read the summary as
       context for its comments.
   - Each comment has an `id`, `file`, `start_line`, `end_line`, the `code`
     snapshot and the `comment` text.
   - A comment with `reopens` set is a follow-up: a reviewer rejected your
     earlier answer to the comment named there. Its `comment` text is their
     note. Read it, treat the comment like any other, and resolve it.
   - `points_at`, when set, is another file range the comment refers to (its
     `file`, `start_line`, `end_line`). Read it before you change the code.
   - A review with no comments (a summary-only review) needs no decision.
     Report its summary to the user.

2. Work each comment. The snapshot in the comment is the code the reviewer
   saw. If the file has changed since, find the same code in the current file.

3. Call `resolve` on every comment you pulled, standalone and linked, right
   after you finish it. Decisions go on comment ids (`r3`), not on review ids
   (`rv1`). `resolve` snapshots the file as it is now, so the saved diff shows
   only that comment's change if you resolve before you edit the file again.

   ```json
   {"workspace": "/abs/project", "id": "r3", "outcome": "done", "author": "claude", "note": "extracted parse_header(), added a test"}
   {"workspace": "/abs/project", "id": "r4", "outcome": "rejected", "author": "claude", "note": "intentional: the caller validates"}
   ```

   - `note` says what you did. For `rejected` it is required: give the reason.
   - Reject only when you have a reason. If a comment is unclear, ask the
     user instead of guessing.
   - Never leave a pulled comment without a decision. Check with `list` and
     `"status": "pulled"`.

4. Summarize for the user: what you resolved, what you rejected and why.

## Leave a review for another agent

When the user asks you to review code and hand it to another agent, call
`add` once per comment, then `submit`:

```json
{"workspace": "/abs/project", "author": "reviewer", "file": "src/api.py", "start_line": 40, "end_line": 58, "comment": "extract this branch"}
{"workspace": "/abs/project", "author": "reviewer", "decision": "request-changes", "summary": "Fix both before merging"}
```

- Lines are 1-indexed and inclusive; `end_line` defaults to `start_line`.
- `submit` without `comment_ids` takes this author's pending comments in this
  exact lane. `decision` is `comment`, `approve` or `request-changes`.
- Set `no_comments: true` for a summary-only review.
- Retract a draft comment with `remove` (`{"workspace": ..., "id": "r3"}`).
  Only a pending comment outside a submitted review can be removed.

## Judge a decision

Judge only when the user asks you to check decisions. A comment decided after
a handoff (`done` or `rejected`) waits for a second reviewer, and only a
reviewer who did not make the decision may judge it. Give
`verdict` `accepted` (you agree with the decision; a rejection accepted means
it should not be done) or `reopened` (the decision is not good enough: the
comment goes back to the agent as a follow-up, and `note` says what is still
wrong, required):

```json
{"workspace": "/abs/project", "id": "r3", "verdict": "accepted", "author": "reviewer", "note": "checked the new test"}
{"workspace": "/abs/project", "id": "r4", "verdict": "reopened", "author": "reviewer", "note": "the rename missed the caller in api.py"}
```

- Never judge a decision you made yourself: the server refuses it. A comment
  that already has a verdict is refused too.
- A verdict is final. It cannot be given twice or changed.
- `start_line` and `end_line` on a reopen move the follow-up to another range
  of the same file.
- rvw records the surface a verdict came through, not who you are: through
  this server it is recorded as `agent`, whatever `author` says.

## Before a commit or push

Call `gate` before you commit or push, with `lane` for the lane you commit.
It returns `open` (not yet decided, drafts included) and `unjudged` (decided
after a handoff, no verdict yet). If either list is not empty, report the ids
to the user and wait. Overriding the gate is the user's decision; do not
record one. `clear: true` means nothing is left.

## Other tools

- `list` with `"status": "open"`: pending + pulled, not yet decided.
- `list_reviews`: submitted review sheets; `"status": "all"` for every one.
- `count`: pending handoffs.
- `show_comment` with `"id": "r3"`: one comment, its decision and its diff.
- `show_review` with `"id": "rv1"`: a whole review sheet.
