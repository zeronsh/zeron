//! Zeron mobile core — the UniFFI surface shared by the iOS and Android apps.
//!
//! - [`client_ffi`]: account, workspace and session state (wraps `zeron-client`).
//! - [`layout`]: analytic transcript layout — markdown → measured display lists
//!   (wraps `zeron-markdown` + `zeron-text`).

uniffi::setup_scaffolding!("zeron_core");

mod client_ffi;

/// Version handshake: the Swift/Kotlin bindings must match the linked library.
#[uniffi::export]
pub fn core_version() -> String {
    env!("CARGO_PKG_VERSION").to_owned()
}
