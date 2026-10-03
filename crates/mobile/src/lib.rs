//! Zeron mobile core — the UniFFI surface shared by the iOS and Android apps.
//!
//! - [`client_ffi`]: account, workspace and session state (wraps `zeron-client`).
//! - [`layout`]: analytic transcript layout — markdown → measured display lists
//!   (wraps `zeron-markdown` + `zeron-text`).

uniffi::setup_scaffolding!("zeron_core");

mod client_ffi;
pub mod layout;
pub mod wallpaper;

/// Version handshake: the Swift/Kotlin bindings must match the linked library.
#[uniffi::export]
pub fn core_version() -> String {
    env!("CARGO_PKG_VERSION").to_owned()
}

/// Process clock for platform countdowns and durations sharing Rust deadlines.
/// This is an epoch label plus monotonic elapsed time, not trusted civil UTC.
#[uniffi::export]
pub fn runtime_now_ms() -> i64 {
    zeron_proto::time::now_ms()
}
