//! Project-scoped HTTP discovery, stable local routing, and authenticated peers.
//! Application bytes use bounded multiplexed streams; edge signaling never
//! transports preview requests or response bodies.
pub mod catalog;
pub mod discovery;
pub mod login;
pub mod mux;
pub mod peer;
pub mod proxy;
mod runtime;
pub mod signaling;
pub mod viewer;

pub mod service;
pub use service::{PreviewService, Scope};

/// Debug builds always cross-check a cached result. Release builds do it for
/// one call in 32 when `ZERON_PERF_VERIFY=1`. A mismatch is the caller's to log;
/// this only decides whether to compare.
pub(crate) fn perf_verify(tick: &std::sync::atomic::AtomicU32) -> bool {
    if cfg!(debug_assertions) {
        return true;
    }
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let enabled =
        ENABLED.get_or_init(|| std::env::var("ZERON_PERF_VERIFY").ok().as_deref() == Some("1"));
    if !*enabled {
        return false;
    }
    tick.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % 32 == 0
}
