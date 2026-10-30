//! Small suites in one test binary: each tests/*.rs file is its own binary,
//! linked against the whole crate, so a handful of tests each cost more to
//! link than to run.

mod claude_models;
mod model_catalog;
mod model_refresh;
mod run_env;
mod pi_mcp;
mod claude_availability;
mod acp_hardening;
