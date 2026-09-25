//! zeron-mcp — a Model Context Protocol server over the running engine.
//!
//! `zeron mcp` speaks MCP (JSON-RPC 2.0, newline-delimited) on stdin/stdout
//! and proxies every tool into the engine's localhost IPC — the same
//! `zeron_rpc` WebSocket the headed app and `zeron sync` dial. Nothing here
//! talks to the edge or touches the filesystem: the engine stays the single
//! authority for chats, devices, projects, and the command plane.
//!
//! The server is hand-rolled rather than an SDK: the MCP surface a stdio
//! tool server needs is five methods (`initialize`, `ping`, `tools/list`,
//! `tools/call`, and the `initialized` notification), and the repo already
//! owns JSON-RPC framing for the Codex and ACP drivers. Zero new crates.
//!
//! When the engine launches a harness it can inject this server into the
//! agent's MCP config with `ZERON_CHAT_ID` / `ZERON_DEVICE_ID` in the
//! environment. Every send from such an agent is then attributed to its
//! originating chat, and a chat can never message itself.

mod jsonrpc;
mod tools;
mod transcript;
mod zeron;

pub use jsonrpc::serve_stdio;
pub use tools::{ToolDef, Tools};
pub use transcript::{RenderOptions, RenderedMessage, render_entries};
pub use zeron::{Origin, Zeron};

/// How `zeron mcp` finds the engine and who it speaks for.
#[derive(Debug, Clone)]
pub struct McpConfig {
    /// Loopback IPC port of the engine to proxy (`GLITCH_FLOW_IPC_PORT`, default 27654).
    pub ipc_port: u16,
    /// The chat whose agent spawned this server, when injected by the engine.
    pub origin: Origin,
}

impl McpConfig {
    /// Resolve branded environment first, with `ZERON_*` compatibility aliases.
    pub fn from_env() -> Self {
        let ipc_port = std::env::var("GLITCH_FLOW_IPC_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .or_else(|| std::env::var("ZERON_IPC_PORT").ok()?.parse().ok())
            .unwrap_or(27654);
        Self {
            ipc_port,
            origin: Origin::from_env(),
        }
    }
}

/// Run the MCP server on this process's stdin/stdout until stdin closes.
pub async fn run(config: McpConfig) -> anyhow::Result<()> {
    let zeron = Zeron::new(format!("ws://127.0.0.1:{}", config.ipc_port), config.origin);
    let tools = Tools::new(std::sync::Arc::new(zeron));
    serve_stdio(std::sync::Arc::new(tools)).await
}
