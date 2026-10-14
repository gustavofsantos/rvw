//! The screens of the app.

/// The screen the user is looking at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Diff,
    Outcome,
    Queue,
    Files,
    Search,
    Changes,
    History,
}
