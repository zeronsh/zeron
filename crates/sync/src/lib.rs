//! zeron-sync — the edge room clients (registry rows + chat2 row protocol over
//! WebSocket against the TS edge) and the local `DocsStore` (SQLite snapshots +
//! processed-command ledger).
//!
//! - [`ChatClient`]: joins a ChatRoom DO (`wss://…/chat2/{chatId}/ws?token=`),
//!   catches up via checkpoint + row backfill, pushes local loro updates as
//!   rows, and reconnects with exponential backoff.
//! - [`RegistryClient`]: the per-profile workspace registry room (sidebar rows,
//!   presence).
//! - [`DocsStore`]: snapshot persistence (the doc IS the outbox — commands + user entries
//!   flush immediately) and the processed-command ledger with mark-BEFORE-execute semantics.

pub mod attachment_store;
pub mod budget;
pub mod chat_client;
pub mod chat_relay;
pub mod chat_frames;
pub mod dial;
mod held_sends;
pub mod net_path;
pub mod registry;
pub mod socket;
mod store;
mod store_recovery;
mod sync_jobs;
mod types;
pub mod wake;

pub use chat_client::{
    ChatClient, ChatDocSink, ChatEvent, ChatStatsSnapshot, ChatTuning, CheckpointFetcher,
};
pub use registry::{
    ReconnectState, RegistryClient, RegistryEvent, RegistryTransport, RegistryTuning,
};
pub use store::{DocsStore, StoreError};
pub use store_recovery::{Recovery as StoreRecovery, corruption_seen, marker_path as store_check_marker, request_check as request_store_check};
pub use types::{RoomStatsSnapshot, StaticUrl, SyncError, UrlProvider};
