//! Who is asking: the authority a request arrived with.
//!
//! The engine serves three kinds of caller over the same [`crate::RpcService`]:
//!
//! - [`Caller::Local`]: this device's app or CLI, holding the full IPC token
//!   (`{data_dir}/ipc-token`), or an in-process client. The default when
//!   nothing set one.
//! - [`Caller::Agent`]: a `zeron mcp` server the engine injected into an
//!   agent's run, holding the agent-scoped token
//!   (`{data_dir}/ipc-token-agent`). It reaches only what the MCP tools need
//!   ([`crate::agent_allowed`]); nothing that grants power.
//! - [`Caller::Peer`]: another device, through the device-room relay
//!   ([`crate::peer_refused`] keeps decisions and credentials local).
//!
//! The gates that matter are enforced by method at the transport guards
//! themselves (the WS server's agent guard, the relay's guard), so they
//! never depend on this value surviving a hop. This task-local exists for
//! finer checks and for the record: read it at dispatch entry, before any
//! `tokio::spawn` (a spawned task starts with no caller, i.e. `Local`).

use std::future::Future;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Caller {
    /// Full-token local IPC (the app, the CLI) or an in-process client.
    Local,
    /// Local IPC with the agent-scoped token. `chat` is the chat the MCP
    /// server says it serves (`x-zeron-chat`): claimed, for the record only.
    Agent { chat: Option<String> },
    /// Another device through the device-room relay. The relay doesn't say
    /// which device yet, so `device` is `None` until it does.
    Peer { device: Option<String> },
}

impl Caller {
    pub fn is_local(&self) -> bool {
        matches!(self, Caller::Local)
    }

    /// `{chat}` / `{device}` for a record of who asked; `None` for the
    /// device's own app.
    pub fn record(&self) -> Option<serde_json::Value> {
        match self {
            Caller::Local => None,
            Caller::Agent { chat } => Some(serde_json::json!({ "agent": true, "chat": chat })),
            Caller::Peer { device } => Some(serde_json::json!({ "peer": true, "device": device })),
        }
    }
}

tokio::task_local! {
    static CALLER: Caller;
}

/// The caller of the request this task serves (`Local` when none was set).
pub fn caller() -> Caller {
    CALLER.try_with(Clone::clone).unwrap_or(Caller::Local)
}

/// Run `f` as `caller`.
pub async fn with_caller<F: Future>(caller: Caller, f: F) -> F::Output {
    CALLER.scope(caller, f).await
}
