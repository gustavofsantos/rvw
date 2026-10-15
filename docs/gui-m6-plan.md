# GUI M6 plan: ledger

Status: approved on 2026-10-15 with the defaults of the open questions. Started.
Scope: [gui-v1-product-brief.md](gui-v1-product-brief.md) milestone M6: C1, C3,
C4, P4, P5, with the content matching of D2. Like M4 and M5 it touches the
core, CLI and GUI. [gui-m5-plan.md](gui-m5-plan.md) built the sessions this
milestone reads.

*Exit:* a one-click audit export of a real branch that a reviewer outside the
team can read.

## Decisions

Settled by the brief (D2, constraints):

- rvw owns the ledger; git is not the audit store. rvw never writes a marker
  or note into a commit. C4 is an export file, not a write to history.
- A comment links to a commit by content, never by SHA or by time window:
  exact when the file at the commit equals the blob the comment resolved to,
  or when the patch-id of the commit's change to that file equals the
  patch-id of the comment's resolution diff. Anything else is "unlinked",
  never guessed.
- Local only. Nothing leaves the machine.

Proposed here:

- **No schema change.** Links are computed when the ledger is read, from
  `file_version`, `resolved_file_version` and git. Nothing is cached.
- **One read operation, `ledger`.** Input: `workspace`, `range` (a git rev
  range, default `<default branch>..HEAD`, resolved with the merge base),
  optional `since` and `until` dates (RFC 3339 or `YYYY-MM-DD`) that narrow
  the range's commits by committer date. Output, a JSON object:
  - `commits`: per commit in range, oldest first: id, subject, author, time,
    provenance (human, agent with tool, model and session id, or marked), and
    the linked comments with link kind (`exact` or `same-change`), their
    decision, note, verdict and the resolution diff;
  - `unlinked`: comments in the workspace and lane scope whose resolution
    matched no commit in range, with the reason ("no commit in range
    touches the file", "no content match");
  - `overrides`: gate overrides in the date span of the range, with reason,
    by, actor and the ids that were open;
  - `sessions`: the reported sessions that own a commit in range.
- **Export is a rendering of that object.** `rvw ledger` prints it as
  `--format markdown|json|html` to stdout, or to `--output FILE` (the one
  command that writes a file the user names; the file is created, never
  overwritten without `--force`). `html` is one self-contained page with a
  print stylesheet; it is the "printable" of C3. No environment variable,
  flags only.
- **Not an MCP tool.** Agents report; the human exports (as with
  `list_sessions`). The operation types still derive schemas so it can be
  added later.
- **GUI.** (C1) the commit detail gains a "Review" section: linked comments
  with decision and verdict, or "unlinked" reasons; the History badge from
  M5 stays. (P4) the Sessions list and the session's History source
  interleave decided comments, by the commits the session reported and
  nothing else. (C3/C4) a palette action "Export audit…" for the current
  branch writes markdown or HTML through a path field. (P5) while composing
  a comment on a line, the composer shows the summary of the reported
  session whose commit last changed that line (from blame), and nothing when
  no session reported it.
- **Manual links are out of M6.** D2's "manual links recorded with who and
  when" needs a store table; the matching rule covers rewrites by content, and
  a link the user would add by hand is a v4 schema and an operation of its
  own. See the open questions.

## Slices

Each slice is one commit and ends green. "Orch." is the orchestrator.

| # | Slice | Files owned | Proof |
|---|---|---|---|
| 0 | Contract (Orch.) | `rvw-core/operations.rs` (`LedgerInput`, `LedgerOutput`, entry types), this file | build, lint |
| 1 | Patch ids | `rvw-git` (`patch_id` of a commit's change to one file, of two blobs) | same change in two commits gives one id; whitespace and a different change do not |
| 2 | Matching | `rvw-core/src/ledger_match.rs` (core cannot depend on `rvw-view`) pure rules, incl. commit-id prefix match: exact blob, same patch-id, unlinked reason | table tests for each rule and for a rebased and a squashed commit |
| 3 | Ledger op | `rvw-core/ledger.rs`; overrides read path | range default, `since`/`until`, lane scope, empty range |
| 4 | Renderers | `rvw-render` markdown, html (escaped, print CSS), json | golden outputs; html escapes comment text |
| 5 | CLI (bats first) | `rvw-cli`, `test/rvw.bats`, help fixtures, README | bats, `test/help-compare.sh`; `--force`, no overwrite |
| 6 | Commit review section (C1) | `rvw-gui/history_screen.rs`, `rvw-view` | tests on the pure model; manual run |
| 7 | Review on the session timeline (P4) | `rvw-gui`, `rvw-view/provenance_sessions.rs` | tests; manual run |
| 8 | Export action (C3, C4) | `rvw-gui` palette and a path card | writes the file; refuses to overwrite |
| 9 | Why this line (P5) | `rvw-gui/composer.rs`, `rvw-view` | pure lookup tests; manual run |
| 10 | Skill, README, brief | `skills/rvw/SKILL.md` untouched (no MCP tool), README, brief | `make check` |
| 11 | Close-out | this file | `make check` |

Slices 1 and 2 run in parallel after 0 on disjoint files; 3 follows both; 4
follows 0; 5 follows 3 and 4; 6 to 9 are GUI slices, run one at a time.

## Settled in review (2026-10-15)

The defaults: no manual links in M6; the default branch needs an explicit
`--range` or `--since`; a squashed commit shows "unlinked"; HTML is the
printable; the export includes each comment's snapshot and diff.

## Open (resolved above)

1. Manual links: build them in M6 (a v4 table, `link` operation, GUI row
   action) or leave them for a later milestone? The exit needs none.
2. Default range: `<default branch>..HEAD` suits a feature branch. For the
   default branch itself there is no base; proposal: the ledger asks for an
   explicit `--range` or `--since` and says so in one line.
3. A comment resolved in a commit that is later squashed: patch-id of the
   squash differs from the comment's resolution diff. Proposal: it shows as
   "unlinked: no content match" in an audit, which is honest, rather than
   guessed. Accept?
4. HTML as the printable: acceptable, or should the export be PDF? HTML
   needs no dependency; PDF is a new one.
5. Should the export include the code snapshot and resolution diff of every
   comment, or only decisions? Proposal: include them, since a reviewer
   outside the team has no queue to look them up in.
