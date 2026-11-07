//! The terminal emulator core shared by the desktop panel and the mobile
//! apps: [`emulator`] folds PTY bytes into a grid; [`predict`] paints typing
//! ahead of a remote host's echo once the host has proven it echoes.

pub mod emulator;
#[cfg(feature = "ghostty")]
pub mod ghostty;
pub mod predict;
