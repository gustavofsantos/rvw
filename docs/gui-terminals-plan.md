# GUI terminals plan: terminals in the workspace

Status: order locked on 2026-10-13, at the user's request. Not started.
Look: boards 5 and 6 of the V2 canvas (https://claude.ai/artifact/MnJ1HhFLLqx45kzuokJWYn,
private): the review desk with a terminal dock, and the `⌘T` palette.

*Exit:* the user runs `claude`, a REPL, `git status` and the tests in rvw for a
full week and does not open another terminal emulator for them.

## Scope

In: many terminals in a dock of the current workspace; each starts in a
directory of the workspace; some run an agent harness, some a long-running
process, some a one-off command. rvw runs them as it would run any command.

Out, on purpose:

- **Persistence.** Terminals live in the rvw process and end with it. Delayed
  to a later plan (see "Later").
- **Agent state on tabs** ("working", "asking"). A tab shows the command, and
  for a finished process its exit status. Nothing is inferred from output.
- Lane attachment, authorship hints and comment delivery by typing into a
  terminal. They need this plan first.

## Architecture

- `rvw-term` (new, no gpui, compiles on `rust-version` 1.93): the PTY and the
  emulation, headless. `spawn`, `write`, `resize`, `snapshot`, `events`,
  `kill`. A terminal has a stable id; the GUI refers to it by id and never
  holds the PTY.
- `rvw-view`: pure logic with unit tests. The list of terminals, the active
  one, titles, the quit guard's wording.
- `rvw-gui`: the dock, the tabs, the palette entry and the view that paints a
  snapshot and forwards keys. No terminal logic.
- Emulation is `alacritty_terminal`. The PTY is its own `tty` and event loop,
  or `portable-pty` if its Windows story is needed. Decided in slice 1.

## Decisions

- **Own the view.** `gpui-terminal` 0.1.0 lists mouse selection and scrollback
  as planned and documents Linux only, so it cannot be the daily driver. It
  may serve a throwaway spike. Zed's `terminal` and `terminal_view` crates are
  the reference for the split.
- **Quitting with running terminals asks first**, naming them. This is the
  whole of the safety net until persistence exists.
- **Hiding the dock is not closing.** A hidden terminal keeps running and
  keeps its output.
- **Never write to a terminal unless the user did.** rvw sends no text and
  presses no Enter.
- **Keys.** With a terminal focused it receives every key except the few rvw
  chords (`⌘J`, `⌘T`, `⌘W`, `⌘1` to `⌘9`, `⌘K`). The key strip says so.
- **Child process.** Own session and process group. Closing a terminal sends
  SIGHUP to the group, then SIGKILL after a grace period, and the child is
  reaped. `TERM` and `COLORTERM` are set for the child only; rvw's own
  configuration stays flags only.
- **Bounds.** One reader thread per PTY, bounded buffering, a cap on
  scrollback, at most one repaint per frame.

## Slices

Each slice ends green (`cargo test`, clippy, `fmt --check`) and is one commit.
The three milestones are in the locked order.

| # | Slice | Proof |
|---|---|---|
| T1.1 | `rvw-term`: spawn, write, resize, snapshot, kill, exit status | tests with real PTYs: echo, resize, exit code, kill of a group |
| T1.2 | Dock with one terminal: paint a snapshot, keys, copy and paste, resize, `⌘J` | manual run |
| T2.1 | `rvw-view`: terminal list, active tab, titles, quit guard | unit tests |
| T2.2 | Tabs and `⌘T` palette: new, close, switch, exit status on a finished tab, quit guard in the window | manual run |
| T3.1 | `rvw-term`: scrollback and search, selection model | unit tests on the grid |
| T3.2 | View: mouse selection, scroll, mouse reporting to programs, URL open | manual run with `vim`, `less`, `htop`, `claude` |

## Open

1. Input method (IME) and the compose key: not decided, and gpui's support has
   to be checked on each platform before T1.2 is called done.
2. Which commands the `⌘T` palette lists: this repository's history and a
   per-repository file. The file's name and format are not decided; rvw takes
   no environment variables.
3. Windows: ConPTY through `portable-pty`, or not supported in the first cut.

## Later

- **Persistence.** A per-workspace host process that owns the terminals, with
  the window attaching and detaching. `rvw-term` keeps the interface above so
  the move is a socket, not a rewrite. Delayed by the user, 2026-10-13.
- **Agent state, lane attachment, comment delivery.** From the V2 boards.
