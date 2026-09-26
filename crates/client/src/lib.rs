//! zeron-client — the engine-free thin client ("viewer device").
//!
//! A phone (or any frontend without an engine) is a *peer* on the zeron mesh:
//! it mirrors the workspace registry, joins per-chat chat2 rooms, renders the
//! session docs, and drives remote engines through the durable command ledger
//! (`zeron_doc::SessionDoc::queue_command`) plus host RPCs over the device
//! relay. No agent ever runs here.
//!
//! # Shape
//!
//! - [`Client`] — one signed-in account (or the offline Demo mode). Owns the
//!   registry mirror, open sessions, and background tasks on the crate's
//!   shared tokio runtime ([`runtime::shared`]). Every snapshot read is an
//!   `Arc` clone — never a network wait, never a whole-doc decode.
//! - [`WorkspaceSnapshot`] — the derived view models (front page with pins and
//!   sections, projects, PRs, archived, devices, search), rebuilt only when
//!   the registry, presence, or time-dependent status actually changes.
//! - [`SessionHandle`] — one chat: an incrementally maintained transcript
//!   ([`SessionSnapshot`], per-entry `Arc`s that stay pointer-equal while
//!   unchanged) plus the composer-facing [`ComposerState`] and every command.
//! - [`ClientListener`] — coalesced change notifications; platforms pull the
//!   new snapshot after an event.
//!
//! Demo mode ([`Credentials::Demo`]) runs the *same* registry/session code
//! against an in-process simulated host, so it exercises the real pipeline.

pub mod attachments;
pub mod auth;
pub mod catalog;
mod client;
pub mod config;
pub mod connectivity;
mod demo;
pub mod error;
pub mod events;
pub mod rpc;
pub mod runtime;
pub mod session;
pub mod workspace;

pub use client::{Client, NewSession, PRELOAD_CAP, SessionTarget, WARM_SESSION_CAP};
pub use config::{
    AuthTokens, ClientConfig, Credentials, DemoFixture, DemoOptions, StreamSpeed, TranscriptScale,
};
pub use connectivity::{Connectivity, ConnectivityState, SendState};
pub use error::ClientError;
pub use events::{ClientEvent, ClientListener};
pub use session::{
    AppendHint, BusyPolicy, ComposerState, Entry, HostCapabilities, HostInfo, InputRequest,
    LiveStatus, LocalEcho, OutgoingAttachment, PendingKind, PendingSend, QueueEditAction,
    QueueEditFinish, QueueEditLease, QueueEditStart, QueueGate, QueueItem, RoomState,
    SendOutcome, SendRequest, SessionHandle, SessionSnapshot, SnapshotDelta, SnapshotWatch,
};
pub use workspace::{
    DeviceView, FrontPage, ProjectRef, ProjectView, PullRequestGroups, SearchField, SearchHit,
    SectionView, SessionRow, WorkspaceSnapshot, project_color_index, relative_time_label,
};

/// Re-exported so consumers (the layout engine) name the exact doc types the
/// transcript carries without a direct `zeron-doc` dependency.
pub use zeron_doc::{MessagePart, MessageRole, MessageStatus, SessionMessageEntry};
pub use zeron_proto::{
    ChangeRequestState, ChangeRequestSummary, ChatConfig, ChatIndicator, ContextUsage,
    UserInputAnswer, UserInputQuestion, WorktreeSpec,
};

/// Wall clock in epoch millis.
pub(crate) fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

pub(crate) fn lock<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub(crate) fn read<T>(lock: &std::sync::RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub(crate) fn write<T>(lock: &std::sync::RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Lowercase v4 uuid (the id format every writer in the fleet mints).
pub(crate) fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}
