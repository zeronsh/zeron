//! Self-hosted workspace synchronization. All remote access uses a paired
//! device credential; administration is exposed only as local Rust methods.

mod config;
mod hub;
mod store;
pub mod tailscale;

pub use config::{Invitation, Node, NodeRole, PrivateConfig, WorkspaceInfo};
pub use hub::{Hub, HubHandle};

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn digest(value: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

fn new_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}
