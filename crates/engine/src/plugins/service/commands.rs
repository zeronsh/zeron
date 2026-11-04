//! A service's programs (`zeron.run`, "commands"): the shared runner the
//! window uses too (crates/plugin-guard's `commands`), so a plugin is held
//! to the same rules wherever it runs a program.
//!
//!   { program, args = { … }, cwd?, stdin?, timeout?, env? }
//!     → { status, stdout, stderr }   (status -1: stopped at its timeout)

pub use zeron_plugin_guard::commands::run_trusting;
