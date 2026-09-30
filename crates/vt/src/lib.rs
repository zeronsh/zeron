//! Terminal emulator core shared by the desktop panel and the mobile
//! terminal: [`emulator`] folds PTY bytes into a renderable grid, [`keys`]
//! encodes key presses as the bytes a PTY expects. No I/O, no UI toolkit.

pub mod emulator;
pub mod keys;

pub use emulator::*;
