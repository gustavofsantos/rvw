---
name: rvw
description: Work the local rvw code review queue. Use when the user asks to pull, check, address or work through review comments or feedback, mentions rvw, or asks you to review code and leave comments for another agent.
allowed-tools: Bash(rvw pull *), Bash(rvw list *), Bash(rvw resolve *), Bash(rvw reject *), Bash(rvw add *), Bash(rvw submit *), Bash(rvw display *), Bash(rvw count *)
---

# rvw

`rvw` is a local queue of code review comments, keyed by workspace (the git
root of the current directory). Each comment points at a file, a line range
and the code as it stood when the comment was written.

If `rvw` is not on `PATH`, tell the user and stop. Install it with
`go install github.com/gustavofsantos/rvw/cmd/rvw@latest`.

## Address review comments

1. Pull the queue. Pulled comments leave it, so pull once and keep the output.

   ```sh
   rvw pull --format json
   ```

   - Use `rvw pull --peek` to look without taking anything.
   - Use `--lane NAME` when the user names a lane or branch. A pinned pull
     takes only that lane. If it comes back empty, stderr says whether work
     waits in another lane. Report that to the user; do not pull the other
     lane on your own.
   - The JSON holds comments in two places. Work both:
     - `reviews`: standalone comments.
     - `submitted_reviews[].comments`: comments linked to a submitted
       review. Each review also has a `decision` and a `summary`; read the
       summary as context for its comments.
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

   ```sh
   rvw resolve r3 --author claude --note "extracted parse_header(), added a test"
   rvw reject r4 --author claude --note "intentional: the caller validates"
   ```

   - `--note` says what you did. For `reject` it is required: give the reason.
   - Reject only when you have a reason. If a comment is unclear, ask the
     user instead of guessing.
   - Never leave a pulled comment without a decision. Check with
     `rvw list --status pulled`.

4. Summarize for the user: what you resolved, what you rejected and why.

## Leave a review for another agent

When the user asks you to review code and hand it to another agent:

```sh
rvw add --author reviewer --file src/api.py --lines 40-58 --comment "extract this branch"
rvw submit --author reviewer --decision request-changes --summary "Fix both before merging"
```

- `--lines` is 1-indexed and inclusive: `N` or `N-M`.
- For a long comment, pipe it in or use `--comment-file PATH`.
- `submit` with no `--id` takes this author's pending comments in this exact
  lane. `--decision` is `comment`, `approve` or `request-changes`.
- Use `--no-comments` for a summary-only review.

## Other commands

```sh
rvw list --status open      # pending + pulled, not yet decided
rvw count                   # pending handoffs
rvw display r3              # one comment, its decision and its diff
rvw display rv1             # a whole review sheet
rvw <command> --help        # every flag
```

Every failure is one `rvw: ...` line on stderr with exit status 1. Read it;
it says what to fix.
