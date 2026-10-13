//! Headless terminals for rvw: a PTY with a child on it, the emulation of its
//! output, and owned snapshots of the screen. No window toolkit, so the GUI
//! paints a [`Snapshot`] and forwards keys encoded by [`encode_key`].
//!
//! Each terminal has a stable [`TerminalId`], one reader thread, capped
//! scrollback, and an event channel that queues at most one repaint request
//! until the owner takes a snapshot.

mod keys;
mod pump;
mod snapshot;
mod terminal;

pub use keys::{encode_key, encode_paste, Key, Modifiers};
pub use snapshot::{Cell, CellFlags, Color, Cursor, CursorKind, Mode, Snapshot};
pub use terminal::{
    Error, Event, Exit, SpawnOptions, Terminal, TerminalId, MAX_SCROLLBACK, MAX_WRITE,
};

#[cfg(test)]
mod tests;
