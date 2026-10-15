//! Headless terminals for rvw: a PTY with a child on it, the emulation of its
//! output, and owned snapshots of the screen. No window toolkit, so the GUI
//! paints a [`Snapshot`] and forwards keys encoded by [`encode_key`].
//!
//! Each terminal has a stable [`TerminalId`], one reader thread, capped
//! scrollback, and an event channel that queues at most one repaint request
//! until the owner takes a snapshot.

mod keys;
mod kitty;
mod mouse;
mod notify;
mod pump;
mod scroll;
mod search;
mod select;
mod snapshot;
mod terminal;
mod url;

pub use keys::{Key, Modifiers, encode_key, encode_paste};
pub use kitty::KittyFlags;
pub use mouse::{
    MAX_WHEEL_LINES, MouseButton, MouseEvent, MouseEventKind, Wheel, encode_alternate_scroll,
    encode_mouse,
};
pub use notify::Notification;
pub use scroll::Scroll;
pub use search::{MAX_MATCHES, MAX_QUERY};
pub use select::{GridPoint, SelectionKind, Side};
pub use snapshot::{
    Cell, CellFlags, Color, Cursor, CursorKind, MatchSpan, Mode, RowSpan, SearchStatus, Snapshot,
};
pub use terminal::{
    Error, Event, Exit, MAX_NOTIFICATIONS, MAX_SCROLLBACK, MAX_WRITE, SpawnOptions, Terminal,
    TerminalId,
};
pub use url::UrlMatch;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_grid;
