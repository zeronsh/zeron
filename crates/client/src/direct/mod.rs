//! Direct mode: the phone talks to the user's own machine over SSH — no
//! edge, no account. The engine's loopback IPC (the same `EngineRpc` the
//! desktop UI uses) rides a `direct-tcpip` channel; [`host::DirectHost`]
//! mirrors its registry and transcript streams into the phone's local docs,
//! so the rest of the client (views, composer, commands) is unchanged.

pub(crate) mod host;
mod lenient;
mod ssh;
#[cfg(test)]
mod tests;

pub use ssh::{ProbeResult, SshKeyPair, generate_ed25519, import_key, probe};

/// Where the link to the machine stands, for the UI and the diagnostics
/// readout (a blank sessions page must never be the only symptom).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DirectPhase {
    /// SSH / tunnel / `EngineInfo` in progress.
    #[default]
    Connecting,
    /// Tunnel up; waiting for the first frame of every registry stream.
    Syncing,
    /// Every registry stream delivered at least once.
    Live,
    /// The last attempt failed (see `last_error`); a retry may be scheduled.
    Failed,
}

impl DirectPhase {
    /// The tunnel to the engine is open (syncing or live).
    pub fn is_up(self) -> bool {
        matches!(self, DirectPhase::Syncing | DirectPhase::Live)
    }
}

/// One engine watch stream as the phone saw it on the current link.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StreamStat {
    /// RPC method (`WatchChats`, …).
    pub name: String,
    pub frames: u64,
    /// Rows in the last frame that parsed.
    pub rows: u32,
    /// Rows in the last frame that didn't parse (kept, never deleted).
    pub skipped_rows: u32,
    /// Rows in the last frame read after a repair (an unknown value this
    /// app version doesn't know was dropped or defaulted).
    pub repaired_rows: u32,
    pub last_frame_ms: Option<i64>,
    /// Last subscribe/parse problem on this stream.
    pub error: Option<String>,
}

/// A timestamped line of the link log (newest last).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectLogLine {
    pub at_ms: i64,
    pub message: String,
}

/// Snapshot of the direct link: phase, errors, engine identity, per-stream
/// counters and a short event log.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DirectStatus {
    pub phase: DirectPhase,
    pub last_error: Option<String>,
    pub retry_at_ms: Option<i64>,
    pub engine_version: Option<String>,
    pub engine_device_id: Option<String>,
    /// Non-blocking note, e.g. the engine is a newer minor version than
    /// this app was checked against.
    pub notice: Option<String>,
    pub connected_at_ms: Option<i64>,
    pub synced_at_ms: Option<i64>,
    pub streams: Vec<StreamStat>,
    pub log: Vec<DirectLogLine>,
}

/// Default engine IPC port (`ZERON_IPC_PORT` on the machine overrides it).
pub const DEFAULT_ENGINE_PORT: u16 = 27654;

/// How to reach one machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshTarget {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub auth: SshAuth,
    /// The engine's IPC port on the machine's loopback.
    pub engine_port: u16,
    /// Pinned host key (`SHA256:…`). `None` = not trusted yet: connecting
    /// fails with [`SshError::HostKeyUnknown`] so the UI can ask.
    pub host_key_fingerprint: Option<String>,
}

#[derive(Clone, PartialEq, Eq)]
pub enum SshAuth {
    /// OpenSSH/PEM private key text (+ passphrase if encrypted).
    Key {
        private_key: String,
        passphrase: Option<String>,
    },
    Password {
        password: String,
    },
}

impl std::fmt::Debug for SshAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SshAuth::Key { .. } => f.write_str("Key(<redacted>)"),
            SshAuth::Password { .. } => f.write_str("Password(<redacted>)"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SshError {
    /// First contact: the UI shows the fingerprint and asks to trust it.
    #[error("unknown host key {algorithm} {fingerprint}")]
    HostKeyUnknown {
        fingerprint: String,
        algorithm: String,
    },
    /// The pinned key changed — possible MITM (or a reinstalled machine).
    #[error("HOST KEY CHANGED: expected {expected}, got {algorithm} {actual}")]
    HostKeyMismatch {
        expected: String,
        actual: String,
        algorithm: String,
    },
    #[error("{0}")]
    Connect(String),
    #[error("{0}")]
    Auth(String),
    #[error("{0}")]
    Engine(String),
    #[error("{0}")]
    Key(String),
}

impl SshError {
    /// Retrying won't help until the user acts (trust / fix credentials).
    pub fn needs_user(&self) -> bool {
        matches!(
            self,
            SshError::HostKeyUnknown { .. }
                | SshError::HostKeyMismatch { .. }
                | SshError::Auth(_)
                | SshError::Key(_)
        )
    }
}
