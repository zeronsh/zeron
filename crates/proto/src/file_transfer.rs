//! Device-to-device file transfer (docs/file-transfer.md): the rows the
//! `WatchFileTransfers` / `ListFileTransfers` surface renders and the
//! per-device receive settings. Distinct from [`crate::TransferProgress`],
//! which tracks queued chat attachments.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FileTransferDirection {
    Outgoing,
    Incoming,
}

/// Lifecycle of one transfer. `Completed`, `Failed`, `Cancelled` and
/// `Declined` are terminal; everything else is live.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FileTransferState {
    /// Walking the sender's paths into a manifest.
    Preparing,
    /// Opening a tunnel to the other device.
    Connecting,
    /// The receiving device asks its user first (its confirmation setting).
    AwaitingAcceptance,
    Transferring,
    /// All bytes landed; whole-file digests are being checked.
    Verifying,
    /// The tunnel dropped; the sender redials and resumes from the
    /// receiver's last verified blocks.
    Reconnecting,
    Completed,
    Failed,
    Cancelled,
    Declined,
}

impl FileTransferState {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::Declined
        )
    }
}

/// How the bytes currently travel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FileTransferTransport {
    /// A direct WebRTC DataChannel between the two engines.
    P2p,
    /// The DeviceRoom relay (fallback when no direct path pairs in time).
    Relay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FileTransferItemKind {
    File,
    Folder,
    Symlink,
}

/// One top-level item the user picked (a file or a whole folder).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileTransferItem {
    pub name: String,
    pub kind: FileTransferItemKind,
    /// Bytes under this item (a folder's files summed).
    pub size: u64,
    /// Regular files under this item (1 for a file).
    pub file_count: u64,
    /// Absolute path on THIS device: the source on the sender, the final
    /// location on the receiver (set once the receiver accepted).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileTransfer {
    /// Minted by the sender; the same id on both devices.
    pub id: String,
    pub direction: FileTransferDirection,
    pub peer_device_id: String,
    pub peer_device_name: String,
    pub state: FileTransferState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<FileTransferTransport>,
    pub items: Vec<FileTransferItem>,
    pub file_count: u64,
    pub total_bytes: u64,
    /// Bytes the receiver has verified (sha-256 per block).
    pub done_bytes: u64,
    /// Recent throughput, bytes/second (0 while idle).
    pub bytes_per_sec: u64,
    /// Receiver: the folder the items land in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination: Option<String>,
    /// Entries the sender left out (sockets, devices, unreadable files).
    #[serde(default)]
    pub skipped: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Epoch millis.
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<i64>,
}

impl FileTransfer {
    pub fn is_live(&self) -> bool {
        !self.state.is_terminal()
    }

    /// 0.0–1.0; an empty transfer reads as complete once it completes.
    pub fn fraction(&self) -> f32 {
        if self.total_bytes == 0 {
            return if self.state == FileTransferState::Completed {
                1.0
            } else {
                0.0
            };
        }
        (self.done_bytes as f64 / self.total_bytes as f64).clamp(0.0, 1.0) as f32
    }
}

/// Receive-side preferences of one device (`GetFileTransferSettings` /
/// `SetFileTransferSettings`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileTransferSettings {
    /// Ask before accepting files from the account's other devices
    /// (default: accept straight into the inbox).
    #[serde(default)]
    pub require_confirmation: bool,
    /// Inbox root; `None` = `{home}/Zeron Transfers`. Each sending device
    /// gets its own subfolder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inbox_dir: Option<String>,
}

/// `SendFiles` reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendFilesReply {
    pub transfer_id: String,
    pub to_device_id: String,
    pub to_device_name: String,
}
