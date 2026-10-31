//! One stream from a host engine: the relay's subscription in live mode, a
//! channel the demo host feeds offline. Dropping it ends the stream (live:
//! the subscription cancels host-side).

use serde_json::Value;

pub(crate) enum HostStream {
    Live(zeron_rpc::RpcSubscription),
    Local(tokio::sync::mpsc::Receiver<Value>),
}

impl HostStream {
    /// The next item; `None` once the stream has ended.
    pub(crate) async fn recv(&mut self) -> Option<Value> {
        match self {
            Self::Live(subscription) => subscription.recv().await,
            Self::Local(rx) => rx.recv().await,
        }
    }
}
