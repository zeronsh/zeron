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

pub mod computer;
mod helpers;
mod jsonrpc;
mod tools;
mod transcript;
mod zeron;

pub use computer::Computer;
pub use jsonrpc::serve_stdio;
pub use tools::{ToolDef, Tools};
pub use transcript::{RenderOptions, RenderedMessage, render_entries};
pub use zeron::{Origin, Zeron};

/// How `zeron mcp` finds the engine and who it speaks for.
#[derive(Debug, Clone)]
pub struct McpConfig {
    /// Loopback IPC port of the engine to proxy (`ZERON_IPC_PORT`, default 27664).
    pub ipc_port: u16,
    /// The engine's data dir, whose `ipc-token` authenticates the IPC dial
    /// (`ZERON_DATA_DIR`; the `zeron` binary fills in its default). The token
    /// itself is resolved at every dial — `ZERON_IPC_TOKEN`, then the
    /// `ZERON_IPC_TOKEN_FILE` the engine injects, then `{data_dir}/ipc-token`
    /// — so a long-lived server follows the engine across restarts.
    pub data_dir: Option<std::path::PathBuf>,
    /// The chat whose agent spawned this server, when injected by the engine.
    pub origin: Origin,
}

impl McpConfig {
    /// Resolve from the process environment: `ZERON_IPC_PORT` /
    /// `ZERON_DATA_DIR` for the engine, `ZERON_CHAT_ID` / `ZERON_DEVICE_ID`
    /// for the originating chat.
    pub fn from_env() -> Self {
        let ipc_port = std::env::var("ZERON_IPC_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(27664);
        Self {
            ipc_port,
            data_dir: std::env::var_os("ZERON_DATA_DIR")
                .filter(|dir| !dir.is_empty())
                .map(std::path::PathBuf::from),
            origin: Origin::from_env(),
        }
    }
}

/// Run the MCP server on this process's stdin/stdout until stdin closes.
pub async fn run(config: McpConfig) -> anyhow::Result<()> {
    let origin = config.origin.clone();
    let zeron = std::sync::Arc::new(Zeron::new(config.ipc_port, config.data_dir, config.origin));
    let computer = std::sync::Arc::new(Computer::new());
    if let Some(chat_id) = origin.chat_id {
        tokio::spawn(watch_turns(
            zeron.clone(),
            chat_id,
            origin.device_id,
            std::sync::Arc::downgrade(&computer),
        ));
    }
    let tools = Tools::new(zeron)
        .with_zeron_enabled(zeron_proto::mcp_settings::user_enabled(None))
        .with_computer(computer);
    serve_stdio(std::sync::Arc::new(tools)).await
}

/// Follow this chat's session and end the computer's turn when the
/// chat stops working — the turn finished, or the user pressed Stop. Long-
/// lived harness processes (Codex app-server, ACP, Pi) keep this bridge
/// alive across turns, so without this a running action would outlive Stop
/// and the agent cursor would linger. Ends with the bridge.
async fn watch_turns(
    zeron: std::sync::Arc<Zeron>,
    chat_id: String,
    device_id: Option<String>,
    computer: std::sync::Weak<Computer>,
) {
    use zeron_proto::{Session, SessionStatus};
    const RETRY: std::time::Duration = std::time::Duration::from_secs(5);
    let mut working = false;
    loop {
        if computer.strong_count() == 0 {
            return;
        }
        let mut sessions = match zeron.subscribe(zeron_rpc::methods::WATCH_SESSIONS, serde_json::json!({})).await {
            Ok(sessions) => sessions,
            Err(error) => {
                tracing::debug!(%error, "computer use: session watch unavailable");
                tokio::time::sleep(RETRY).await;
                continue;
            }
        };
        while let Some(item) = sessions.recv().await {
            let Ok(rows) = serde_json::from_value::<Vec<Session>>(item) else {
                continue;
            };
            let mine = rows.iter().filter(|s| s.chat_id == chat_id);
            let row = mine
                .clone()
                .find(|s| device_id.as_deref() == Some(s.device_id.as_str()))
                .or_else(|| mine.max_by_key(|s| s.updated_at));
            let now_working = row.is_some_and(|s| s.status == SessionStatus::Working);
            if working && !now_working {
                let Some(computer) = computer.upgrade() else {
                    return;
                };
                computer.turn_ended().await;
            }
            working = now_working;
        }
        tokio::time::sleep(RETRY).await;
    }
}
