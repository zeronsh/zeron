//! Dynamic workflows: the pure half.
//!
//! A workflow is a Starlark script that orchestrates many child agent chats.
//! This crate holds everything about it that needs no engine: the interpreter
//! and its host API ([`runtime`], [`globals`]), static analysis and
//! diagnostics ([`analysis`]), call-site identity for the journal ([`site`]),
//! and the pure run-state reducer ([`reducer`]). Effects reach the world only
//! through [`host::Host`]; the engine implements it over its scheduler and
//! journal, tests implement it with fakes.
//!
//! Only `zeron-engine` depends on this crate. Wire and state types live in
//! `zeron-proto` so clients render runs without linking an interpreter.

pub mod analysis;
pub mod diagnostic;
mod globals;
pub mod host;
pub mod limits;
pub mod reducer;
pub mod runtime;
pub mod site;
pub mod testing;

pub use analysis::{Analysis, CallSites, SiteKind, analyze, dialect};
pub use diagnostic::Diagnostic;
pub use limits::Limits;
pub use reducer::reduce;
pub use runtime::{RunError, run_script};
