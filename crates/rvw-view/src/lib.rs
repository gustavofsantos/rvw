//! View logic shared by the terminal and graphical front ends: no drawing, no
//! windowing, no I/O beyond what the services already do. Everything here is
//! testable without a screen.

pub mod changes;
pub mod diff;
pub mod fuzzy;
pub mod keys;
pub mod queue;
pub mod tree;
