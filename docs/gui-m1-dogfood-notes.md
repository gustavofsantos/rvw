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

## Later in the milestone

Orchestrating:

- Two agents editing the same files in one working tree (the submit dialog and
  the file finder both touched `shell.rs`) worked because each was told to
  re-read right before editing and to keep hunks small. The cost: the two
  features could only be committed together.
- An agent that finishes with background work still running can re-send its
  report many times. The harness reported non-delivery each time; nothing was
  lost, but the orchestrator saw the same report eleven times.
- Agents were denied deleting their own scratch build directories under
  `/tmp`. The per-user quota there filled up and tests that create temporary
  repositories failed with `Disk quota exceeded`, which looked like a code
  failure. Tell agents to build outside `/tmp` from the start.
- Each agent's report named what it could not verify, always the same item:
  nothing has been seen in a window. The binary starts and stays up for ten
  seconds against a scratch queue, but layout, click and focus behaviour are
  unchecked.

Product:

- Single-line comment field. `TextInput` is a port of gpui's single-line
  example. Review comments are often several lines; a multi-line editor is the
  first thing to build before M1 is called done.
- "Waiting on you" never clears: a done comment stays waiting until the verdict
  exists (M4). The queue model already takes an acknowledged set for this.
- Rejected is shown by looking the comment up by id because `Row` carries no
  glyph; fold that into `Row`.
- `review_screen.rs`, `shell.rs` and `diff.rs` are over 350 lines and want
  splitting.

How to build and run on Linux today:

1. Install the development packages `libxcb1-dev`, `libxkbcommon-dev` and
   `libxkbcommon-x11-dev`, or give the linker a directory with the unversioned
   `libxcb.so`, `libxkbcommon.so` and `libxkbcommon-x11.so` symlinks:
   `RUSTFLAGS="-L <dir>" cargo build -p rvw-gui`.
2. `rvw-gui --workspace <repo> --db <scratch.db>`. Use a scratch database
   while the GUI writes comments.

## The multi-line editor round

- The pure model went to a Haiku agent and came back right: 35 tests that pin
  the behaviour (word moves, preferred column, coalesced undo). Every decision
  the agent had to make was listed in its report, which made review quick.
- The GPUI text area went to a Sonnet agent, a deliberate exception to "Haiku
  mostly": it is the one place a mistake shows up as visual jitter, and nobody
  can look at the window. It split the work into six files, found that
  gpui's `position_for_index` puts a wrap boundary at the end of the upper row
  and wrote its own caret placement, and put the height, scroll and click-count
  rules into a tested pure module. That is the kind of care that was missing
  from the earlier, cheaper slices.
- A bad `KeyBinding` string panics at startup. Running the binary for a few
  seconds after each change that touches bindings is the cheapest check there
  is; it caught nothing here, but it is the only guard.
- The key strip lit two keys that did nothing (`J K`, `O`). The strip is a
  promise, so the follow-up made both honest: J/K now move through files, and
  Open file stays off with a hover hint until it exists. Lesson: when a status
  surface is built before the behaviour behind it, every lit control is a
  debt; audit the strip against behaviour before each milestone is called done.

Still unverified by eye, in priority order for whoever runs it first:

1. The text area: wrap edges, selection look, caret and blink, growth between
   2 and 8 lines.
2. The diff: scrolling a large diff (every line is rebuilt per frame).
3. Click targets and focus: selecting lines, opening the composer with C,
   Escape and focus return, the finder with the keyboard.
4. The key strip tooltips and the J/K scroll-into-view.

## First run by a person (release build, 2026-10-12)

- The first run was a debug build and looked slow, with a thick border and code
  in a proportional font. Three of those four came from writing a mockup
  literally: a canvas margin around a "window", fonts named but not installed.
  The fourth was a debug build of a GPU toolkit.
- After the release build with virtualised lists, a hard manual session (many
  clicks, file search, scrolling) felt fast and worked. The direction holds.
- Still to learn from use: text area feel, syntax highlighting, and what a
  real review session needs next.
