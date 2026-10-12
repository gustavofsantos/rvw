# GUI M1: working notes

Notes taken while building the review desk with an orchestrator and Haiku
subagents, using `rvw` itself against a scratch database. Newest last.

## rvw, used as the review tool

What worked:

- `add`, `submit`, `list`, `count`, `pull`, `resolve` and `show` did what the
  help says. A Haiku agent pulled a two-comment review, edited the code and
  recorded a decision on each with no extra instructions beyond "use the CLI
  with this `--db`".
- `rvw show <id>` keeps the code as reviewed and the diff made while
  resolving, which made the orchestrator's check of the agent's work quick.

What did not:

1. **Pull JSON is easy to misread.** For a submitted review, `count` is 1,
   `reviews` is `[]` and the review is under `submitted_reviews`. Nothing in
   the help explains the split.
2. **An empty list reads like an error.** `list --status pulled` on nothing
   prints `rvw: no review comments for ...` and exits 0.
3. **`--status` values differ between comments and reviews.** `complete` is a
   review state only, but the help lists it next to the comment statuses.
4. **The resolution diff is the whole file against the snapshot.** The agent
   fixed r1 and r2 and resolved both afterwards, so `show r1` also contains
   r2's change. The skill already says to resolve right after each comment;
   agents batch anyway.
5. **No way to attach context.** The review said "the spec is ambiguous about
   the title cut"; the reference to the spec lived only in prose. This is the
   "points at" idea from the GUI design (E4).

## Orchestrating Haiku agents

- A task that names exact files, exact signatures and exact verification
  commands came back right the first time (key strip, queue model, shell).
- Agents copy surrounding module docs into new files. One pasted the crate's
  doc into `queue.rs`; the orchestrator fixed it.
- "Test first" is claimed but not observed: an agent wrote tests and code in
  one write. Tests were still good, so this was accepted.
- Specs with an ambiguous number ("cut to 60") get decided by the agent. Say
  the exact rule, with an example.
- Agents ran `cargo fmt`-clean code but did not run `fmt --check` across the
  workspace; the orchestrator ran it before each commit.
- Parallel agents on disjoint files worked. Anything that needs the same
  `lib.rs` or `Cargo.toml` had to be set up by the orchestrator first.

## Machine and toolchain

1. `/usr/bin/rustc` 1.93.1 comes before `~/.cargo/bin` 1.99.0 on `PATH`.
   Clippy fails in dependency crates unless `RUSTC` and a separate
   `CARGO_TARGET_DIR` are set.
2. `xattr` 0.2.3, pulled in by gpui, needs `libc::ENOATTR`, which libc 0.2.190
   no longer exports on Linux. `Cargo.lock` pins libc to 0.2.186.
3. Linking `rvw-gui` needs `libxcb`, `libxkbcommon` and `libxkbcommon-x11`
   development packages. Only the runtime libraries are installed. This
   blocks `cargo build -p rvw-gui` on this machine until they are installed.
