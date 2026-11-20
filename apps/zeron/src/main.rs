//! zeron — the command line with the engine linked in: `zeron headless` runs
//! the engine alone, and the engine's other subcommands run here. It starts
//! local-only without credentials. `zeron login` and `zeron logout` select
//! the profile used by the next engine start without mutating a live runtime.
//!
//! The command line is `zeron_cli`'s (shared with the engine helper and the
//! zgx interface). The desktop app is the zgx interface (apps/zgx), which
//! ships its own `zeron`: this one opens no window.

use zeron_cli::{Engines, Launch};

fn main() -> anyhow::Result<()> {
    // The engine runs its own executable as an agent's stdio MCP forwarder
    // (`zeron headless`'s agents): first, before any setup, to stay tiny.
    if std::env::args().nth(1).as_deref() == Some("mcp-proxy") {
        return zeron_mcp::proxy::run();
    }
    match zeron_cli::run(zeron_update::current_version(), Engines::InProcess) {
        Launch::Done(result) => result,
        Launch::Engine(command) => zeron_app::run_engine_command(command),
        #[cfg(target_os = "linux")]
        Launch::Appshot => anyhow::bail!("Appshots are the desktop app's: open Zeron and use its shortcut"),
        Launch::Window { .. } => anyhow::bail!(
            "this `zeron` has no window: open the Zeron app, or run `zeron headless` for the engine alone"
        ),
    }
}
