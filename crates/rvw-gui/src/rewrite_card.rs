//! The narrow history edit (B14): a card over the window listing the
//! unpushed commits, where each can be kept, reworded, squashed into the one
//! below or dropped; then the before and after, and Apply.

use gpui::{AnyElement, Context, KeyDownEvent, Window};

use crate::shell::Shell;

/// The open card.
pub struct RewriteCard {}

impl RewriteCard {
    /// A message field of the card has the keyboard, so the shell must not
    /// take the focus back.
    pub fn editing(&self) -> bool {
        false
    }
}

impl Shell {
    /// Opens the card on the current branch's unpushed commits.
    pub fn open_rewrite(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}

    /// The keys while the card is open; true when the key was taken.
    pub(crate) fn rewrite_key(
        &mut self,
        _event: &KeyDownEvent,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> bool {
        false
    }

    /// The card over the window, when open.
    pub(crate) fn rewrite_overlay(&self, _cx: &Context<Self>) -> Option<AnyElement> {
        None
    }
}
