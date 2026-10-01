//! Wire types for synced composer drafts.
//!
//! A draft is a tiny Loro doc (`zeron_doc::DraftDoc`) synced through its own room. Every
//! composer window holds a replica and exchanges Loro updates with its engine, which owns the
//! room connection. Updates are opaque bytes, base64-encoded on the JSON RPC wire.

use serde::{Deserialize, Serialize};

/// Params for `WatchDraft` and `ClearDraft`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftTarget {
    pub chat_id: String,
}

/// Params for `EditDraft`: a local commit's Loro update, from the caller's replica.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditDraft {
    pub chat_id: String,
    /// Base64 Loro update bytes.
    pub update: String,
}

/// One item of the `WatchDraft` stream.
///
/// The first frame is always a `reset` carrying the engine's full snapshot. Later frames carry
/// incremental updates (including echoes of the caller's own edits, which import as no-ops). A
/// `reset` frame after the first means the draft was discarded (sent from any device): the
/// receiver must drop its replica and adopt the snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftFrame {
    /// Discard generation of the draft room; increases each time a draft is sent.
    pub epoch: u64,
    /// Replace the local replica instead of merging into it.
    pub reset: bool,
    /// Base64 Loro snapshot (`reset`) or update bytes.
    pub update: String,
}
