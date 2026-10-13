//! View logic shared by the terminal and graphical front ends: no drawing, no
//! windowing, no I/O beyond what the services already do. Everything here is
//! testable without a screen.

pub mod branches;
pub mod changes;
pub mod commit_screen;
pub mod compare_menu;
pub mod context;
pub mod diff;
pub mod diffrows;
pub mod editor;
pub mod files;
pub mod finder;
pub mod fuzzy;
pub mod highlight;
pub mod keys;
pub mod nav;
pub mod outcome;
pub mod palette;
pub mod patch;
pub mod queue;
pub mod selection;
pub mod split;
pub mod tree;
pub mod viewmode;
pub mod workspaces;
