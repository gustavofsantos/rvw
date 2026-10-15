# GUI M5 plan: provenance

Status: built on 2026-10-15; awaiting the user's manual run of the GUI.
Scope: [gui-v1-product-brief.md](gui-v1-product-brief.md) milestone M5: D5, P1,
P2, P3. Like M4, this milestone changes the core: CLI, MCP, TUI-neutral, GUI.

*Exit:* history shows human vs agent commits and opens the session an agent
reported.

## Decisions

Settled by the brief (D5, P1 to P3, constraints):

- Session context is agent-reported over MCP. No transcripts, no inference
  from time windows. An agent that does not report leaves no provenance.
- rvw never writes a marker into a commit. It reads a `Claude-Session` trailer
  already in a commit message as a link, nothing more.
- Reported fields are attributed to the reporter and are not editable
  afterwards.
- Human by default; agent only when reported (or when a marker is read).

Proposed here:

- **One write operation, `report_session`.** Input: `session_id`, `tool`,
  `model`, `summary` (nullable), `commits` (full or abbreviated ids). Output:
  the stored session. MCP only; the actor is always the agent. No CLI write
  command (a human has no session to report).
- **One read operation, `sessions`.** Lists a workspace's sessions, newest
  first, with their commits. Exposed as `rvw sessions` (text, markdown, JSON)
  and to the GUI through the `Service`. No MCP tool for it in M5: agents only
  report.
- **Append-only.** A second report for the same `session_id` in a workspace
  adds commits and refreshes nothing else; if `tool` or `model` differ it is
  an error, so the first report's attribution stays.
- **Schema v3** (`v3.sql`): `sessions` (workspace, session_id, tool, model,
  summary, reported_at; unique on workspace + session_id) and
  `session_commits` (session, commit id). The migration chain becomes
  per-version (today it hard-codes v2 for anything below 2).
- **Commit ids are stored as reported.** The core has no git. The GUI matches
  a commit to a session by prefix (at least 7 characters) in `rvw-view`.
- **Authorship kind is a view concern.** `rvw-git` `LogEntry` gains the parsed
  `Claude-Session` trailer values; `rvw-view` merges git + sessions into a
  `Provenance` per commit: `Human`, `Agent{tool, model, session}` or
  `Marked{session_id}` (trailer without a report).
- **GUI.** History rows get an authorship badge; the commit detail shows the
  session (tool, model, summary, its other commits) and opens it; a Sessions
  view lists the timeline, interleaving reported sessions with the review
  comments decided in them, time-ordered.

## Slices

Each slice is one commit and ends green. "Orch." is the orchestrator.

| # | Slice | Files owned | Proof |
|---|---|---|---|
| 0 | Contract (Orch.) | `rvw-core/operations.rs` types and stubs, this file | build, lint |
| 1 | Store v3 | `rvw-store` (`v3.sql`, `lib.rs` migrate chain, `tx.rs`), `Tx` trait, fake Tx | upgrade tests v1 to v3 and v2 to v3; fresh equals upgraded |
| 2 | Core ops | `rvw-core/session.rs` | table tests: append-only, attribution mismatch error, empty commits |
| 3 | Trailer parse | `rvw-git/history.rs` | `Claude-Session` trailer, absent, multiple, in body |
| 4 | MCP | `rvw-mcp` (`report_session`), `test/reference` recaptured | tool tests; `test/mcp-compare.sh` |
| 5 | CLI (bats first) | `rvw-cli`, `help.rs`, `test/rvw.bats`, help fixtures | bats, `test/help-compare.sh` |
| 6 | Provenance model | `rvw-view/src/provenance.rs` | Human, Agent, Marked; prefix matching |
| 7 | Skill and docs | `skills/rvw/SKILL.md`, README | `make check` |
| 8 | History badge and detail | `rvw-gui/history_*` | manual run; goldens if any |
| 9 | Sessions view | `rvw-gui`, palette entry | manual run |
| 10 | Close-out | this file | `make check` |

Slices 1, 3 and 6 can run in parallel after 0 (disjoint files). 2 follows 1;
4 and 5 follow 2; 8 and 9 follow 6.

## Settled in review (2026-10-15)

1. **Append-only.** A repeat report for a session id adds commits; a `tool`
   or `model` mismatch is an error. Agents report as they commit, so a
   one-report rule would lose later commits.
2. **Workspace like every tool.** The operation input carries `workspace`,
   filled by the adapter from `--workspace` or the cwd, exactly as the other
   MCP tools do.
3. **No `list_sessions` MCP tool** for now; CLI and GUI only.
4. **Comments relate to sessions only through reported commits.** No
   time-window inference; decided comments show the verdict's actor alone.

## Status at the end of the session (2026-10-15)

| Done | Where |
|---|---|
| Report types | `rvw-core` types and operations (slice 0) |
| Store v3, per-version migrate chain | `rvw-store` (slice 1) |
| `report_session`, `sessions` | `rvw-core/session.rs` (slice 2) |
| `Claude-Session` trailer into `LogEntry` | `rvw-git/history.rs` (slice 3) |
| `report_session` MCP tool, fixture | `rvw-mcp`, `test/reference` (slice 4) |
| `rvw sessions`, README | `rvw-cli`, `rvw-render`, bats (slice 5) |
| Provenance model | `rvw-view/provenance*.rs` (slice 6) |
| Skill | `skills/rvw/SKILL.md` (slice 7) |
| History badge and session facts | `rvw-gui/history_*` (slice 8) |
| Session history source, picker, palette `Sessions` | `rvw-gui` (slice 9) |

Open after the run:

- Not run by hand: no display was available to the workers. The user must
  open History and the `Sessions` palette action against a real report.
- Blame's `HISTORY_FORMAT` carries no trailer field, so blame rows show no
  markers.
- Sessions load synchronously on the UI thread, and a read error shows as
  an empty list in History.
- The installed `~/.local/bin/rvw` is v1-only; the user's database is still
  v2 and migrates to v3 on first use of a current build, with no backup.
