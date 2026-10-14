# GUI M4 plan: close the loop

Branch `ccr-9b5d01c9-wrndkg`. Scope: [gui-v1-product-brief.md](gui-v1-product-brief.md)
milestone M4 (A5, A8, C2, E4, and the verdict core change of D3). Look: boards
2 and 3 in [gui-v1-design.md](gui-v1-design.md).

*Exit:* every agent `done` ends accepted or reopened, and the commit gate warns
on undecided comments.

Unlike M1 to M3 this milestone changes the core: the CLI, MCP, TUI and GUI all
change with it.

## Decisions

Settled with the user on 2026-10-14:

- **Parity references are re-captured from rvw.** The Go baseline is retired.
  `test/help-compare.sh` and `test/mcp-compare.sh` join `make check`. Both were
  already failing: `remove` was added without updating `test/reference`.
- **No backfill.** Done comments from before M4 wait for a verdict. The user
  clears them on purpose: `rvw list --status done --format ids | xargs -n1 rvw
  accept --note "reviewed before verdicts"`.
- **GUI drafts stay pullable.** A GUI comment is a pending standalone comment,
  so a pull can take it before the review is submitted. M4 labels it in the
  handoff preview ("your draft, a pull takes it too"). A "held" flag is later.
- **The schema moves to v2, one way.** Older binaries refuse a newer schema, so
  the CLI, GUI and MCP server must be updated together. On the first migration
  the store copies the v1 file to `<db>.v1.bak`.

Design, from the advisor's review:

- **The verdict is orthogonal to status.** `Status` (pending, pulled, done,
  rejected), its CHECK constraint and "a decision is final" do not change. A
  comment gains `verdict: Option<Verdict>`, one nested object, so a `by` can
  never exist without a verdict. In SQL the fields are flat columns.
- **Reopen creates a new comment.** A standalone pending comment with
  `reopens: "r3"`, the original's lane, file and `points_at`, and the original's
  range unless overridden. The original's verdict carries `followup: "r9"`. The
  normal pull hands it over once; the agent's original decision stays in the
  ledger untouched. The follow-up snapshots the file as it is now. If the file
  is gone it keeps the original's code and range, with an empty `file_version`.
- **What can be judged.** A `done` or `rejected` comment decided after a handoff
  (`pulled_at` set). Not a withdrawal (a pending comment rejected while
  pending). Accept on a rejection means "I agree it should not be done";
  reopen means "do it anyway" and the note becomes the new comment's text.
- **Who can judge.** A non-empty author, not equal to `resolved_by` (D3: a
  *second* reviewer). A comment whose own author decided it does not wait for a
  verdict. A verdict is final: not given twice, not changed. Reopen needs a
  note. `remove` refuses a follow-up, so a verdict never points at a deleted
  comment.
- **Actor comes from the surface, not from a field.** `Service::judge(input,
  Actor)` and `Service::override_gate(input, Actor)`. MCP is `Agent`; CLI, TUI
  and GUI are `Human`. The MCP schema has no `actor`, so an agent cannot claim
  to be human. This is the surface a verdict came through, not an
  authenticated identity; the skill and README say so.
- **Types** (`rvw_core::types`): `VerdictKind { Accepted, Reopened }`, `Actor {
  Human, Agent }`, `Verdict { kind, by, actor, at, note: Option, followup:
  Option }`, `Location { file, start_line, end_line }`, `GateOverride`.
  `Comment` gains `points_at`, `reopens`, `verdict`, all `Option`, required in
  the schema, `null` when absent. Methods `is_open()` and `awaits_verdict()`.
- **Operations** (`operations.rs`): `VerdictInput`/`VerdictOutput { comment,
  followup }`, `GateInput`/`GateOutput { workspace, lane, open, unjudged,
  clear }`, `OverrideInput`, and `AddInput.points_at`.
- **The gate: core owns the definition and the record.** `open` is pending or
  pulled (drafts included). `unjudged` is `awaits_verdict()`. A pure
  `undecided(comments, lane)` serves core, view and GUI. `override_gate`
  recomputes the ids inside its transaction and refuses when nothing is
  undecided. Overrides go in a `gate_overrides` table. An agent's accept counts
  as judged; the D3 "human required" policy is later.
- **The gate in the GUI is a card, not a block.** Before commit, amend and push,
  when `undecided` is not empty: counts, a required reason, confirm records the
  override and then runs the action. If recording fails, nothing runs. This
  amends the design doc's "open comments never block a commit".
- **A8 needs no core change.** The GUI peeks once with lane `""` and
  `rvw-view::handoff` groups by lane. `pull`'s `elsewhere` stays as it is.
- **Points at (E4).** The file must resolve inside the workspace and be a
  regular file; `start_line` within the file; a reversed range is swapped;
  stored relative. CLI: `--points-at FILE:N` or `FILE:N-M`.
- **The GUI's `seen.rs` stand-in is retired.** Opening the outcome screen no
  longer closes a done comment; only a verdict does. `gui::verdict::Verdict` is
  renamed `ReviewDecision` so "verdict" has one meaning.
- **Store v2.** `schema.sql` stays v1; a new `v2.sql` adds the columns and the
  table. A fresh database applies v1 then v2. New comment columns go after
  `l.review_seq` so the positional index does not move.

## Slices

Each slice is one commit and ends green. "Orch." means the orchestrator.

| # | Slice | Files owned | Proof |
|---|---|---|---|
| 0 | Contract (Orch.) | core types, operations, repository, lib, stubs; every `Comment { .. }` site | build, tests, lint; nullable-contract and truth-table tests |
| 1 | Store v2 | `rvw-store` | v1 upgrades intact; fresh equals upgraded; backup made; round trips |
| 2 | `judge` | `rvw-core/src/verdict.rs` | accept, reopen, every refusal, actor, lane strictness |
| 3 | points_at and `remove` guard | `rvw-core/src/service.rs` | validation table |
| 4 | Gate | `rvw-core/src/gate.rs` | `undecided` table, override |
| 5 | Render | `rvw-render` | new lines only when set; old output identical |
| 6 | View verdict states | `rvw-view/{queue,outcome}.rs`, then the GUI call sites | state tests |
| 7 | A8 preview | `rvw-view/src/handoff.rs` | grouping by lane |
| 8 | Gate wording | `rvw-view/src/commit_screen.rs` | notice text |
| 9 | MCP | `rvw-mcp` | 15 tools; actor `agent`; `actor` argument refused |
| 10 | CLI (bats first) | `test/rvw.bats`, `rvw-cli` | bats |
| 11 | Docs and references | skill, README, AGENTS, design doc, `test/reference`, Makefile | `make check` |
| 12 | Outcome screen Accept/Reopen; rename | `rvw-gui` | compiles; manual run |
| 13 | Queue A/R and per-lane preview | `rvw-gui` | manual run |
| 14 | Gate card | `rvw-gui` | manual run |
| 15 | Points-at in the composer | `rvw-gui` | manual run |
| 16 | Close-out | this file | `make check` |

Slices 1 to 8 run in parallel after 0, one Haiku agent each, on disjoint files.
9 and 10 run in parallel after 1 to 4. 12, 14 and 15 run in parallel.

## Working rules for the agents

As in M3: a subagent edits only the files named in its task and never commits;
the orchestrator reviews each diff, runs the checks and commits. Each agent
builds in its own `CARGO_TARGET_DIR` under the main checkout's `target/`, and
only the crate it works on. Only the orchestrator builds `rvw-gui`. Non-GUI
crates compile on Rust 1.93, edition 2021 (no let-chains). Tests come first.
Develop only against a scratch `--db`.
