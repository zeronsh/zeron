//! A plugin's programs ("commands"): the ones its manifest names
//! (`"commands": ["gh"]`), run as you when it asks (`zeron.run(program,
//! args, { cwd, stdin, timeout, env })`). The engine's runner, shared
//! (crates/plugin-guard's `commands`): no shell, the program found by its
//! name on your login shell's PATH, harmless variables only, git and gh
//! held to their rules, a timeout and a cap on what it prints.
//!
//!   { program, args = { … }, cwd?, stdin?, timeout?, env? }
//!     → { status, stdout, stderr }   (status -1: stopped at its timeout)

pub use zeron_plugin_guard::commands::run;
