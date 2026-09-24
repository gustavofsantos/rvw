---
name: rvw
description: Work the local rvw code review queue. Use when the user asks to pull, check, address or work through review comments or feedback, mentions rvw, or asks you to review code and leave comments for another agent.
allowed-tools: mcp__rvw__pull, mcp__rvw__list, mcp__rvw__list_reviews, mcp__rvw__count, mcp__rvw__show_comment, mcp__rvw__show_review, mcp__rvw__resolve, mcp__rvw__add, mcp__rvw__submit, Bash(rvw pull *), Bash(rvw list *), Bash(rvw resolve *), Bash(rvw reject *), Bash(rvw add *), Bash(rvw submit *), Bash(rvw show *), Bash(rvw count *)
---

# rvw

`rvw` is a local queue of code review comments, keyed by workspace (the git
root of the current directory). Each comment points at a file, a line range
and the code as it stood when the comment was written.

## MCP tools or CLI

Prefer the rvw MCP tools (`mcp__rvw__pull`, `mcp__rvw__resolve`, ...). Use
them whenever they are connected, and use the CLI only when they are not.

- Pass your project directory, as an absolute path, as `workspace` on every
  call.
- Pass `author` (your name, e.g. `claude`) on `add`, `submit` and `resolve`.
- Wait for each call to return before the next one. The server runs calls
  concurrently, so a `pull` sent alongside an `add` may not see it.
- A failed call returns a tool error whose text says what to fix.

If the MCP tools are not connected, fall back to the `rvw` CLI (the
**CLI** lines below) and mention to the user that `rvw mcp config` prints how
to connect them. If `rvw` is not on `PATH` either, tell the user and stop.
Install it with `go install github.com/gustavofsantos/rvw/cmd/rvw@latest`.

## Address review comments

1. Pull the queue. Pulled comments leave it, so pull once and keep the result.

   - **MCP:** `pull` with `{"workspace": "/abs/project"}`.
   - **CLI:** `rvw pull --format json`.

   - Set `peek: true` (CLI `--peek`) to look without taking anything.
   - Set `lane` (CLI `--lane NAME`) when the user names a lane or branch. A
     pinned pull takes only that lane. If it comes back empty, `elsewhere`
     (CLI: stderr) says whether work waits in another lane. Report that to the
     user; do not pull the other lane on your own.
   - The result holds comments in two places. Work both:

     | | MCP `pull` | CLI `pull --format json` |
     |---|---|---|
     | standalone comments | `comments` | `reviews` |
     | submitted reviews | `reviews` | `submitted_reviews` |

     Each submitted review has a `decision`, a `summary` and its linked
     `comments`; read the summary as context for its comments.
   - Each comment has an `id`, `file`, `start_line`, `end_line`, the `code`
     snapshot and the `comment` text.
   - A review with no comments (a summary-only review) needs no decision.
     Report its summary to the user.

2. Work each comment. The snapshot in the comment is the code the reviewer
   saw. If the file has changed since, find the same code in the current file.

3. Record a decision on every comment you pulled, standalone and linked,
   right after you finish it. Decisions go on comment ids (`r3`), not on
   review ids (`rv1`).
   `resolve` snapshots the file as it is now, so the saved diff shows only
   that comment's change if you resolve before you edit the file again.

   - **MCP:** `resolve` with
     `{"workspace": "/abs/project", "id": "r3", "outcome": "done", "author": "claude", "note": "extracted parse_header(), added a test"}`,
     or `"outcome": "rejected"` with the reason as `note`.
   - **CLI:**

     ```sh
     rvw resolve r3 --author claude --note "extracted parse_header(), added a test"
     rvw reject r4 --author claude --note "intentional: the caller validates"
     ```

   - The note says what you did. To reject, it is required: give the reason.
   - Reject only when you have a reason. If a comment is unclear, ask the
     user instead of guessing.
   - Never leave a pulled comment without a decision. Check with `list` and
     `status: "pulled"` (CLI `rvw list --status pulled`).

4. Summarize for the user: what you resolved, what you rejected and why.

## Leave a review for another agent

When the user asks you to review code and hand it to another agent:

- **MCP:** `add` once per comment, then `submit`:

  ```json
  {"workspace": "/abs/project", "author": "reviewer", "file": "src/api.py", "start_line": 40, "end_line": 58, "comment": "extract this branch"}
  {"workspace": "/abs/project", "author": "reviewer", "decision": "request-changes", "summary": "Fix both before merging"}
  ```

- **CLI:**

  ```sh
  rvw add --author reviewer --file src/api.py --lines 40-58 --comment "extract this branch"
  rvw submit --author reviewer --decision request-changes --summary "Fix both before merging"
  ```

- Lines are 1-indexed and inclusive; `end_line` defaults to `start_line`.
- `submit` without `comment_ids` (CLI `--id`) takes this author's pending
  comments in this exact lane. `decision` is `comment`, `approve` or
  `request-changes`.
- Set `no_comments: true` (CLI `--no-comments`) for a summary-only review.

## Other operations

| MCP tool | CLI | |
|---|---|---|
| `list` with `status: "open"` | `rvw list --status open` | pending + pulled, not yet decided |
| `list_reviews` | `rvw list --reviews` | submitted review sheets; `status: "all"` for every one |
| `count` | `rvw count` | pending handoffs |
| `show_comment` with `id: "r3"` | `rvw show r3` | one comment, its decision and its diff |
| `show_review` with `id: "rv1"` | `rvw show rv1` | a whole review sheet |

`rvw mcp config` prints how to add the MCP server to Claude Code, and
`rvw <command> --help` lists every CLI flag. A CLI failure is one `rvw: ...`
line on stderr with exit status 1. Read it; it says what to fix.
