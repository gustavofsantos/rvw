# rvw GUI v1: product brief

Status: draft for discussion. Owner: Gustavo.

## Thesis

Every Git client is organised around the **commit**. rvw's GUI is organised
around the **review**: a request, who made it, who acted, what they decided
and the diff that resulted. Git is the substrate, not the product.

> The Git client where every change an agent makes is accountable.

## Who it is for

Engineers who answer for the code they ship, in legacy and critical systems
(finance first), and who now work with AI coding tools. They need to:

1. See what an agent changed *before* it is committed.
2. Tell the agent exactly what to fix, anchored to code, and be sure it was
   handed over once.
3. Prove later what was asked, what was done, and what was refused and why.

They do not need another Git power tool. They already have one.

## Where the market leaves a gap

| Tool | What to take | What it does not do |
|---|---|---|
| Tower | Clarity of the everyday flows: status, stage, commit, branches, history. Safe defaults. | No notion of an agent or of review feedback as a queue. |
| GitButler | Working on several lines of work at once; agent hooks. | Rewrites the mental model of Git (virtual branches). We will not. |
| Entire | Linking AI sessions to commits; provenance. | Records what happened; does not let you steer it with structured feedback. |
| Sublime Merge | Speed, diff quality, search, keyboard-first. | Human-only. |

None of them has: comment on a range → hand over once → recorded decision →
resolution diff. That loop already exists in `rvw` (CLI, TUI, MCP). The GUI
makes it the front door.

## Principles

- **Review is the primary object.** The home screen is the queue, not the
  history.
- **No layer over Git.** Branches, index and history are Git's. rvw never
  rewrites history or hides state. Any other Git client still works.
- **Local-first.** No account, no server, no telemetry by default.
- **One operation, four surfaces.** Each GUI action is one `Service` call, so
  CLI, TUI, MCP and GUI never diverge.
- **Keyboard-first**, mouse-complete.
- **Accountable by default.** Authors, timestamps and decisions are never
  editable after the fact (edits are recorded as edits).

## v1 feature pack

### A. Review desk (the differentiator)

| # | Feature | Notes |
|---|---|---|
| A1 | **Queue home**: pending / pulled / done / rejected, grouped by review and lane, filter by author, file, status | Replaces the TUI reviews pane. Badge for "waiting on agent" and "waiting on you". |
| A2 | **Diff view with inline comments**: select a range in side-by-side or unified, write, add | Works on uncommitted, vs default branch, vs previous commit (same bases as the TUI). |
| A3 | **Compose a review**: gather comments, set decision (comment / approve / request-changes) and summary, submit | Draft review stays editable until submitted. |
| A4 | **Outcome page**: comment, code as reviewed, agent's note, and the *resolution diff* | Exists as a TUI page; the GUI shows it next to the live diff. |
| A5 | **Accept or reopen** a done comment | New. Today the agent's `done` is final. The human needs a verdict on the agent's work. |
| A6 | **Live working tree**: file watcher; changes an agent makes appear in the diff as they happen | Review while the agent is still running. |
| A7 | **Authors and lanes visible**: who said what, which branch or lane it belongs to | Human vs agent is a first-class badge. |
| A8 | **Handoff preview**: exactly what the next `pull` will give the agent, per lane | Builds trust that nothing is lost or taken twice. |

### B. Git essentials (just enough to leave the other client)

| # | Feature | Notes |
|---|---|---|
| B1 | Status, stage / unstage by file, hunk and line, commit, amend | Line staging is the Sublime Merge feature people stay for. |
| B2 | Branches: list, create, checkout, delete; worktrees listed as workspaces | Worktree is already rvw's unit of queue. |
| B3 | History: commit graph, commit diff, file history, blame | Read-only. |
| B4 | Fetch, pull (ff-only by default), push | No force push in v1. |
| B5 | Search: commits, files, content; fuzzy file open | Speed is a feature. |
| B6 | Stash | Simple list, apply, drop. |

### C. Accountability

| # | Feature | Notes |
|---|---|---|
| C1 | **Review ledger on commits**: a commit and a branch show the reviews and decisions that preceded them | The link Entire makes for sessions, we make for reviews. |
| C2 | **Gate before commit/push**: warn on open or undecided comments for the lane; never block silently, always overridable with a recorded reason | |
| C3 | **Audit export** for a range of commits or a date span: markdown, JSON, printable | Commit → review → comment → decision → resolution diff. |
| C4 | **Portable ledger export**: write the ledger to a file or a metadata transport on demand | Lets the evidence travel with the repo without touching commit messages. See decision D2. |

## Explicitly not in v1

- Virtual branches, stacked branches, interactive rebase UI.
- Merge conflict resolution (open the user's merge tool).
- Hosting integrations: PRs, issues, CI status.
- Built-in agent runner, terminal or chat. rvw talks to agents through MCP.
- Windows. v1 supports macOS and Linux only (D4).
- Multi-user sync, accounts, cloud.
- Submodules, LFS, partial clone special cases.

## Core dependencies the GUI exposes

Found while reading `rvw-core`; each needs a decision before the GUI work
that depends on it.

1. **Verdict on a done comment** (A5): the status set is
   pending → pulled → done | rejected. Needs `accepted` and `reopened`, or a
   reopen that creates a linked follow-up comment. The verdict records who
   gave it (human or reviewing agent), per decision D3.
2. **Durability of the ledger** (C1, C3): see decision D2. The ledger must
   survive rebases, which means it cannot be keyed by commit SHA.
3. **Pre-submit drafts** (A3): comments are already stored before submission;
   confirm the draft/edit rules match what the GUI shows.
4. **Change notifications** (A6, A1 live counts): the core is pull-based. The GUI
   needs a cheap "something changed" signal (file watcher on the DB or an
   event from the service).

## Milestones

Each ends with something usable daily.

1. **M1 Review desk.** A1 to A4, A7, plus read-only diff and branch switch.
   *Exit:* the TUI is no longer needed for review.
2. **M2 Git essentials.** B1 to B6, A6.
   *Exit:* the user commits and pushes from rvw for a full week.
3. **M3 Accountability.** A5, A8, C1 to C4.
   *Exit:* a one-click audit export of a real branch that a reviewer outside
   the team can read.

## How we will know it works

- Time from "comment added" to "agent pulled" and to "decision recorded".
- Share of pulled comments that end with a decision (target 100%).
- Share of agent `done` comments that the human accepts without reopening.
- Dogfood: the author's own legacy/financial work goes through rvw for two
  weeks with no fallback to another Git client.

## Decisions

- **D1. Toolkit: GPUI** (the toolkit Zed uses). Settled. Consequences: Rust
  all the way down, so the GUI sits beside `rvw-tui` as one more adapter over
  `Service`. Risks to burn down early: GPUI's API churn (pin a revision),
  Linux (X11 and Wayland) maturity, and accessibility. M1 starts with a spike that
  renders a large diff with range selection.
- **D2. Ledger: rvw owns it; Git is not the audit store.** Native trailers
  add noise to every commit message, and notes do not reliably survive
  rebases. So:
  1. The ledger stays in rvw's store, the single source of truth.
  2. Records are keyed by a stable change identity, not a commit SHA: the
     file blob versions already stored (`file_version`,
     `resolved_file_version`), plus the patch-id of the resolution diff. A
     commit is linked to a review by matching content, so a rebased commit
     still finds its reviews.
  3. Matching is medium strictness: a commit links to a review when its
     code equals what was reviewed (exact blob) or its diff has the same
     patch-id. Anything else shows as "unlinked", never guessed. The user may
     link a commit by hand; it is recorded as a manual link with who and when.
  4. A post-rewrite hook re-links records after rebase and amend as an
     optimisation, not a requirement.
  5. C4 is an export, not a write to history: JSON/markdown file, with a
     git-meta adapter as a possible transport later. git-meta is third party,
     and its docs key metadata to commit IDs, so it would need the same
     re-linking; adopt it only after a spike proves it handles rebases.
- **D3. "Accepted" may come from a human or a second reviewing agent.** The
  verdict records the actor kind. Policy hook for later: a workspace setting
  that requires a human accept before the commit gate (C2) clears.

- **D4. Platforms: macOS and Linux only.** Settled. Windows is out of
  scope, so the M1 spike covers macOS and Linux (X11 and Wayland) only.

## Open questions

1. **Name of the surface**: still "rvw", or a product name for the GUI client?
