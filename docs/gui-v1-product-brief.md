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

Scope taken from Tower's help taxonomy and tips page. Tower is the reference
for *what is table stakes*; v1 takes the daily-driver subset.

| # | Feature | Tower area | Notes |
|---|---|---|---|
| B1 | Inspect changes, stage and unstage by file, hunk and line | Working Copy: Inspecting, Staging | Line staging is the feature people stay for. |
| B2 | Commit, quick amend, commit without the mouse | Committing, Tips | Commit templates included. |
| B3 | Discard changes with undo, and **an Undo history for the last Git actions** | Undoing Changes, Undoing Things | Backed by reflog. Accountability users fear destructive actions most. |
| B4 | Stash: list, apply, drop | Stashes & Snapshots | Snapshots (auto-stash before risky ops) is v1.1. |
| B5 | Commit history: graph, details, historic file tree, file history, blame | Commit History | Read-only, plus P1 provenance markers. |
| B6 | Search commits (message, author, content) and files; quick open | Searching, Tips | Speed is a feature. |
| B7 | Compare branches and revisions | Comparing Branches & Revisions | Same diff view as the review desk. |
| B8 | Branches: create, check out, delete, track, rename; tags create and publish | Branches & Tags | |
| B9 | Fetch, pull (ff-only default), push, publish a branch | Remote Repositories | No force push in v1. |
| B10 | Worktrees: list, add, switch, remove | Worktrees | A worktree is rvw's unit of queue, so this is core, not extra. |
| B11 | Merge, with conflicts handed to the user's merge tool | Merging, Merge Conflicts | Conflict resolution UI is out of scope. |
| B12 | Open diff/merge tool and terminal from anywhere; CLI hand-off | Integration with Other Tools | Cheap, and keeps users from leaving. |
| B13 | Cherry-pick | Working with Commits | Small, common in hotfix work on legacy branches. |
| B14 | **Narrow history edit**: reword, squash and drop commits that are not yet pushed | Interactive Rebase, narrowed | See rules below. |

B14 rules:

- Only commits not on any remote. Pushed history is never rewritten in v1.
- Three operations: reword, squash, drop. No reorder, split, or edit-stop.
- Each edit is one undoable step in the B3 history, and shows the before and
  after list before applying.
- A conflict aborts the edit and restores the branch. No conflict UI.
- Reviews follow rewritten commits through the content match of D2, so a
  squash or reword does not orphan a review. A dropped commit's reviews stay in
  the ledger, marked "commit dropped".

Not in v1, with the Tower area it comes from: full interactive rebase
(reorder, split, edit, rewriting pushed history; B14 is the narrow subset), reflog
browser (B3 covers the safety use), patches, submodules, pull requests and
hosting services, stacked branches and Graphite, custom workflows, "commit
with AI" (an agent's commit message is not what rvw is for), exporting
branches and tags, SSH key management, user profiles.

### C. Accountability

| # | Feature | Notes |
|---|---|---|
| C1 | **Review ledger on commits**: a commit and a branch show the reviews and decisions that preceded them | The link Entire makes for sessions, we make for reviews. |
| C2 | **Gate before commit/push**: warn on open or undecided comments for the lane; never block silently, always overridable with a recorded reason | |
| C3 | **Audit export** for a range of commits or a date span: markdown, JSON, printable | Commit → review → comment → decision → resolution diff. |
| C4 | **Portable ledger export**: write the ledger to a file or a metadata transport on demand | Lets the evidence travel with the repo without touching commit messages. See decision D2. |

### D. Provenance: the commit and the conversation that made it

Taken from Entire's commit and session views, joined to rvw's review data.
Source of context is **agent-reported through MCP** (decision D5), not read
from transcripts.

| # | Feature | Notes |
|---|---|---|
| P1 | **Commit history with authorship kind**: each commit marks human or agent, plus the tool and model the agent reported | Human by default; agent only when reported. |
| P2 | **Session timeline**: the sessions an agent reported, with the commits and review comments they relate to, in one time-ordered view | As rich as what agents report: no raw transcripts. |
| P3 | **Commit ↔ session link**: open any commit's session, and any session's commits | From what the agent reports; also reads a marker already in the commit message (e.g. `Claude-Session`) but never writes one. |
| P4 | **Reviews on the timeline**: comments and decisions sit beside the session and commits, so prompt → code → review → fix reads as one story | Entire has no review layer. |
| P5 | **Why this line**: while commenting on a line, see what the reporting agent said about it | Only when the agent reported. |

Constraints:

- Local only. Nothing leaves the machine.
- rvw never writes markers into commits (D2).
- A link rvw did not receive from an agent or read from a marker does not
  exist. No inference by time window, so no false provenance in an audit.
- Reported fields are attributed to the reporter and are not editable
  afterwards.

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

Each milestone is small enough to finish in a few weeks and ends with
something usable daily.

1. **M1 Review desk.** A1 to A4, A7, read-only diff, branch switch,
   command palette.
   *Exit:* the TUI is no longer needed for review.
2. **M2 Daily commit loop.** B1 to B4, B8, B9, A6.
   *Exit:* the user stages, commits and pushes from rvw for a full week, and
   sees the agent's edits land live.
3. **M3 Navigate history.** B5 to B7, B10 to B14.
   *Exit:* the user no longer opens another Git client for history, search,
   worktrees, cherry-picks or tidying local commits.
4. **M4 Close the loop.** A5, A8, C2, and the verdict core change (D3).
   *Exit:* every agent `done` ends accepted or reopened, and the commit gate
   warns on undecided comments.
5. **M5 Provenance.** The session-report MCP operation (D5) and its skill
   update, P1 to P3.
   *Exit:* history shows human vs agent commits and opens the session an
   agent reported.
6. **M6 Ledger.** C1, C3, C4, P4, P5, with the content-matching of D2.
   *Exit:* a one-click audit export of a real branch that a reviewer outside
   the team can read.

## How we will know it works

- Time from "comment added" to "agent pulled" and to "decision recorded".
- Share of pulled comments that end with a decision (target 100%).
- Share of agent `done` comments that the human accepts without reopening.
- Dogfood: the author's own legacy/financial work goes through rvw for two
  weeks with no fallback to another Git client.

## Benchmark

Read on 2026-10-12 from each product's own pages: Tower's help taxonomy and
tips page, GitButler's docs (desktop, AI agents, CLI), Sublime Merge's
product page, and Entire's commit, session and analytics views on a real
repository. Marketing pages were not tested against the software. `?` marks
something not seen. `●` has it, `◐` partly, `○` does not.

### Git client

| Capability | Tower | Sublime Merge | GitButler | Entire | rvw v1 |
|---|---|---|---|---|---|
| Hunk and line staging | ● | ● | ● | ○ | ● M2 |
| History graph, blame, file history | ● | ● | ● | ◐ web, read-only | ● M3 |
| Search commits as you type | ● | ● | ● | ○ | ● M3 |
| Undo history of Git actions | ● | ? | ● oplog | ○ | ● M2 |
| Worktrees | ● | ? | ◐ offers parallel branches instead | ○ | ● M3 |
| Interactive rebase / history editing | ● | ● | ● direct commands | ○ | ◐ reword, squash, drop, unpushed only, M3 |
| Conflict resolution UI | ● | ? | ● first class | ○ | ○ hand off |
| Pull requests, hosting accounts | ● | ? | ● `but pr` | ◐ GitHub repo view | ○ |
| Stacked or parallel branches | ● | ? | ● core idea | ○ | ○ by design |
| Submodules | ● | ● | ? | ○ | ○ |
| Speed as a headline feature | ◐ | ● | ◐ | n/a | must match (spike done) |
| Command palette, theming | ◐ quick open | ● | ? | n/a | add to M1 |

### Review and agents

| Capability | Tower | Sublime Merge | GitButler | Entire | rvw v1 |
|---|---|---|---|---|---|
| Marks which commits an agent made | ○ | ○ | ◐ branch per agent session | ● | ● M5 if reported |
| LLM context linked to commits | ○ | ○ | ○ seen | ● transcripts, checkpoints | ◐ reported summary, M5 |
| Review code before it is committed | ○ | ○ | ◐ inspect and adjust | ○ | ● |
| Anchored comments handed to an agent | ○ | ○ | ○ | ○ | ● |
| Handed over exactly once | ○ | ○ | ○ | ○ | ● |
| A recorded decision on each comment | ○ | ○ | ○ | ○ | ● |
| Diff that resolved each comment | ○ | ○ | ○ | ○ | ● |
| Human accept or reopen of agent work | ○ | ○ | ○ | ○ | ● M4 |
| Agent integration | ○ | ○ | ● CLI, skill | ● capture | ● MCP, skill |
| Audit export | ○ | ○ | ○ | ◐ analytics | ● M6 |
| Token and cost analytics | ○ | ○ | ○ | ● | ○ |
| Shared with a team on the web | ◐ | ○ | ○ | ● | ○ local only |

### What this says

1. **The review loop is open ground.** Across all four, nothing turns
   feedback into a queue the agent pulls once and answers with a recorded
   decision. GitButler is closest: it lets you "inspect and adjust" what an
   agent did, by hand, then it pushes. That is the workflow a review queue
   replaces.
2. **GitButler optimises the agent's version control; rvw holds the agent
   accountable.** GitButler measures success by agents using fewer commands
   and finishing faster. We measure it by every change having a recorded
   decision. Different axis, so no head-on fight, but it is the product most
   likely to add review comments later. Our defence is the ledger (D2) and
   the verdicts (D3).
3. **Entire is deeper on capture, shallower on judgement.** It records the
   conversation and tokens; it has no way to disagree with the agent. We took
   agent-reported context (D5), which is thinner. If provenance depth turns
   out to matter in audits, reading local transcripts is the upgrade path.
4. **Table stakes we were missing**, now added or flagged:
   - An undo history (GitButler's oplog, Tower's Undoing Things). Already B3.
   - A command palette. Sublime Merge's keyboard-first experience depends on
     it. Add to M1.
   - History editing. Narrowed into M3 (B14): reword, squash and drop on
     unpushed commits. Reorder, split and rewriting pushed history stay out.
5. **Gaps we accept**: pull requests, conflict UI, stacking, analytics,
   team-hosted views. Each belongs to a different product thesis.

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
- **D5. Session context is agent-reported over MCP.** Settled. Needs a new
  MCP operation, with typed input and output like every other operation, where an agent states
  its session id, tool, model, an optional summary and the commits it made.
  The rvw skill must teach agents to call it. Trade-off accepted: no raw transcripts, and
  an agent that does not report leaves no provenance. Reading local
  transcripts can be added later as another source for the same record.

## Open questions

1. **Name of the surface**: keep "rvw" for now (decided). Revisit before a
   public release.
