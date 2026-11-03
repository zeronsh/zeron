//! The rules a plugin is held to wherever it reaches past its sandbox,
//! shared by Zeron's engine (plugin services, package installs) and its zgx
//! app (plugin windows), so the two can't drift apart:
//!
//! - [`path`]: package and manifest paths, before any join to a folder;
//! - [`commands`]: the programs a plugin runs (`zeron.run`), and the one
//!   runner both sides use;
//! - [`net`]: where a plugin's requests may go (`zeron.fetch`).

pub mod commands;
pub mod net;
pub mod path;
