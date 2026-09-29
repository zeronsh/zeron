//! Host RPC vocabulary: what a viewer asks an execution host over the
//! device-room relay (catalogs, refs, folders, uploads, queue actions), and
//! the capability gates that decide whether to ask at all.

use std::sync::Arc;

pub use zeron_proto::{FolderEntry, FolderListing, RepoRef, WorktreeSpec};

/// Engine capability strings a device row advertises (`Device::capabilities`).
/// Capabilities, not semver: a personal integration build can share an
/// upstream version without the doc/RPC surface.
pub mod capability {
    pub const MESSAGE_QUEUE_V1: &str = "message-queue-v1";
    pub const MESSAGE_QUEUE_ACTIONS_V1: &str = "message-queue-actions-v1";
    pub const MESSAGE_QUEUE_ATTACHMENTS_V1: &str = "message-queue-attachments-v1";
    pub const MESSAGE_QUEUE_CLEAN_ATTACHMENT_TEXT_V1: &str =
        "message-queue-clean-attachment-text-v1";
    pub const MESSAGE_QUEUE_EDIT_LEASE_V1: &str = "message-queue-edit-lease-v1";

    /// Every queue capability (demo hosts advertise the full surface).
    pub const ALL_QUEUE: &[&str] = &[
        MESSAGE_QUEUE_V1,
        MESSAGE_QUEUE_ACTIONS_V1,
        MESSAGE_QUEUE_ATTACHMENTS_V1,
        MESSAGE_QUEUE_CLEAN_ATTACHMENT_TEXT_V1,
        MESSAGE_QUEUE_EDIT_LEASE_V1,
    ];
}

/// Queued-attachment version gate (composer.rs QUEUED_ATTACHMENTS_MIN): the
/// host must defer commands carrying `pending://` refs until the bytes land.
pub const QUEUED_ATTACHMENTS_MIN: (u64, u64, u64) = (0, 2, 12);

/// Upload progress callback: fraction in `0.0..=1.0` of the file's bytes the
/// host has committed.
pub type ProgressFn = Arc<dyn Fn(f64) + Send + Sync>;

/// Relay method names (single source of truth: `zeron_rpc::methods`).
pub mod methods {
    pub use zeron_rpc::methods::*;
}
