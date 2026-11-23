//! The one switch every orchestration gate reads: the Orchestrators plugin
//! (`engine.plugins.is_on("orchestrators")`, account-wide, off unless the
//! person turned it on). Off, nothing of orchestration runs or is written:
//! no loops, no orchestration rows, its RPCs refused with [`off_error`],
//! no role tools or prompts in a chat's MCP, no crew or Brain-dump
//! handling on the send path; a crew or orchestrator chat is a plain chat.
//! Its rows are kept as they are. On again, it all comes back without a
//! restart.

use tokio::sync::watch;
use zeron_rpc::{RpcError, methods};

/// The plugin whose switch this is.
pub const PLUGIN_ID: &str = "orchestrators";

/// What an orchestration RPC answers while the switch is off.
pub const OFF: &str = "Orchestrators is off. Turn it on in Settings → Plugins.";

pub(crate) fn off_error() -> RpcError {
    RpcError::Failed(OFF.into())
}

/// The RPC methods that are orchestration's alone: refused while it's off.
pub(crate) const METHODS: &[&str] = &[
    methods::WATCH_ORCHESTRATION,
    methods::ORCHESTRATE,
    methods::SCRATCHPAD,
    methods::WATCH_SCRATCHPAD,
    methods::WATCH_DEVICE_LOADS,
    methods::WATCH_PROGRESS,
    methods::WATCH_COMMS,
];

/// The device capabilities that tell other devices to route orchestration
/// work here: advertised only while the switch is on.
pub(crate) const CAPABILITIES: &[&str] = &[
    zeron_proto::capabilities::ORCHESTRATION_PROGRESS_V1,
    zeron_proto::capabilities::ORCHESTRATION_ROUTES_V1,
    zeron_proto::capabilities::ORCHESTRATION_PLACES_V1,
];

/// `capabilities` with orchestration's dropped while it's off.
pub(crate) fn capabilities(mut capabilities: Vec<String>, on: bool) -> Vec<String> {
    if !on {
        capabilities.retain(|c| !CAPABILITIES.contains(&c.as_str()));
    }
    capabilities
}

/// Whether Orchestrators is on, now and on every change. Cheap to clone.
#[derive(Clone, Debug)]
pub struct OrchestrationSwitch {
    rx: watch::Receiver<bool>,
}

impl OrchestrationSwitch {
    /// Following `rx` (the plugin's `Plugins::watch`).
    pub fn from_watch(rx: watch::Receiver<bool>) -> Self {
        Self { rx }
    }

    /// One that never changes (a bare assembly: off).
    pub fn fixed(on: bool) -> Self {
        let (_tx, rx) = watch::channel(on);
        Self { rx }
    }

    pub fn is_on(&self) -> bool {
        *self.rx.borrow()
    }

    /// Told every time it changes.
    pub fn watch(&self) -> watch::Receiver<bool> {
        self.rx.clone()
    }
}

impl Default for OrchestrationSwitch {
    fn default() -> Self {
        Self::fixed(false)
    }
}
