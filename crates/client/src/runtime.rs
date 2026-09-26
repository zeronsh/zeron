//! The client's own tokio runtime.
//!
//! One small multi-thread runtime per process, created on first use and never
//! dropped: platform UI threads never block on it, and a signed-out client
//! shuts down its *tasks* (cancellation token) rather than a runtime — dropping
//! a runtime from inside async context panics, and sign-out/sign-in cycles
//! would otherwise churn OS threads.
//!
//! Every public `async fn` in this crate spawns its work onto this runtime and
//! awaits the join handle, so the returned futures are executor-agnostic:
//! UniFFI's Swift/Kotlin executors (or any other) can poll them directly.

use std::future::Future;
use std::sync::OnceLock;

use tokio::runtime::{Handle, Runtime};

use crate::error::ClientError;

static RUNTIME: OnceLock<Runtime> = OnceLock::new();

/// The shared runtime (2 worker threads — sync is I/O bound; decode work is
/// per-changed-entry and small).
pub fn shared() -> &'static Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("zeron-client")
            .enable_all()
            .build()
            .expect("zeron-client runtime")
    })
}

pub fn handle() -> Handle {
    shared().handle().clone()
}

/// Run `fut` on the client runtime and await its result from any executor.
pub async fn run<F, T>(fut: F) -> Result<T, ClientError>
where
    F: Future<Output = Result<T, ClientError>> + Send + 'static,
    T: Send + 'static,
{
    match shared().spawn(fut).await {
        Ok(result) => result,
        Err(err) if err.is_cancelled() => Err(ClientError::Closed),
        Err(err) => Err(ClientError::Internal(format!("task panicked: {err}"))),
    }
}
