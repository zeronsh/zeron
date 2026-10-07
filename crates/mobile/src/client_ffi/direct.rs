//! Direct mode (SSH to the user's own machine): targets, key helpers and
//! the pre-connect status probe.

use zeron_client as zc;
use zeron_client::direct as zd;

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
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

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SshTarget {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub auth: SshAuth,
    /// The engine's IPC port on the machine's loopback (default 27654).
    pub engine_port: u16,
    /// Pinned host key (`SHA256:…`); `None` until the user trusts it.
    pub host_key_fingerprint: Option<String>,
    /// Every address to try, in order; empty = just `host:port`.
    pub endpoints: Vec<SshEndpoint>,
}

/// Another address of the same machine (LAN IP, Tailscale IP, …).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SshEndpoint {
    pub host: String,
    pub port: u16,
    /// The app's name for it ("lan", "tailscale", …), echoed in the status.
    pub kind: String,
    /// Tried alone this long before the next address joins (0 = until it fails).
    pub head_start_ms: u32,
    /// Give up reaching it (TCP + SSH handshake) after this long; 0 = 20 s.
    pub connect_timeout_ms: u32,
}

impl From<SshEndpoint> for zd::SshEndpoint {
    fn from(e: SshEndpoint) -> Self {
        zd::SshEndpoint {
            host: e.host,
            port: e.port,
            kind: e.kind,
            head_start_ms: e.head_start_ms,
            connect_timeout_ms: e.connect_timeout_ms,
        }
    }
}

/// How one address last fared.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct DirectEndpointStat {
    pub host: String,
    pub port: u16,
    pub kind: String,
    /// The current link runs over it.
    pub active: bool,
    pub last_attempt_ms: Option<i64>,
    pub last_ok_ms: Option<i64>,
    pub last_error: Option<String>,
    pub latency_ms: Option<u64>,
}

impl From<SshTarget> for zd::SshTarget {
    fn from(t: SshTarget) -> Self {
        zd::SshTarget {
            host: t.host,
            port: t.port,
            user: t.user,
            auth: match t.auth {
                SshAuth::Key {
                    private_key,
                    passphrase,
                } => zd::SshAuth::Key {
                    private_key,
                    passphrase,
                },
                SshAuth::Password { password } => zd::SshAuth::Password { password },
            },
            engine_port: t.engine_port,
            host_key_fingerprint: t.host_key_fingerprint,
            endpoints: t.endpoints.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum SshError {
    /// First contact: show the fingerprint and ask to trust it.
    #[error("unknown host key {algorithm} {fingerprint}")]
    HostKeyUnknown {
        fingerprint: String,
        algorithm: String,
    },
    /// The pinned key changed.
    #[error("host key changed: expected {expected}, got {actual}")]
    HostKeyMismatch {
        expected: String,
        actual: String,
        algorithm: String,
    },
    #[error("{reason}")]
    Connect { reason: String },
    #[error("{reason}")]
    Auth { reason: String },
    #[error("{reason}")]
    Engine { reason: String },
    #[error("{reason}")]
    Key { reason: String },
}

impl From<zd::SshError> for SshError {
    fn from(e: zd::SshError) -> Self {
        match e {
            zd::SshError::HostKeyUnknown {
                fingerprint,
                algorithm,
            } => SshError::HostKeyUnknown {
                fingerprint,
                algorithm,
            },
            zd::SshError::HostKeyMismatch {
                expected,
                actual,
                algorithm,
            } => SshError::HostKeyMismatch {
                expected,
                actual,
                algorithm,
            },
            zd::SshError::Connect(reason) => SshError::Connect { reason },
            zd::SshError::Auth(reason) => SshError::Auth { reason },
            zd::SshError::Engine(reason) => SshError::Engine { reason },
            zd::SshError::Key(reason) => SshError::Key { reason },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SshKeyPair {
    pub private_openssh: String,
    /// The authorized_keys line (`ssh-ed25519 AAAA… comment`).
    pub public_openssh: String,
    pub fingerprint: String,
}

impl From<zd::SshKeyPair> for SshKeyPair {
    fn from(k: zd::SshKeyPair) -> Self {
        Self {
            private_openssh: k.private_openssh,
            public_openssh: k.public_openssh,
            fingerprint: k.fingerprint,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ProbeResult {
    pub host_key_fingerprint: String,
    pub host_key_algorithm: String,
    pub engine_device_id: String,
    pub engine_version: Option<String>,
    pub latency_ms: u64,
}

/// Where the direct link stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum DirectPhase {
    Connecting,
    Syncing,
    Live,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct DirectStreamStat {
    pub name: String,
    pub frames: u64,
    pub rows: u32,
    pub skipped_rows: u32,
    /// Rows kept after dropping/defaulting values this app doesn't know.
    pub repaired_rows: u32,
    pub last_frame_ms: Option<i64>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct DirectLogLine {
    pub at_ms: i64,
    pub message: String,
}

/// Link phase, last error, engine identity, per-stream counters and a short
/// event log (the Machines diagnostics readout).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct DirectStatus {
    pub phase: DirectPhase,
    pub last_error: Option<String>,
    pub retry_at_ms: Option<i64>,
    pub engine_version: Option<String>,
    pub engine_device_id: Option<String>,
    /// Non-blocking note (e.g. the engine is a newer minor version).
    pub notice: Option<String>,
    pub connected_at_ms: Option<i64>,
    pub synced_at_ms: Option<i64>,
    pub streams: Vec<DirectStreamStat>,
    pub log: Vec<DirectLogLine>,
    /// Phone clock minus the computer's (ms), from session heartbeats.
    pub clock_offset_ms: Option<i64>,
    /// The machine's addresses in dial order, with their last outcome.
    pub endpoints: Vec<DirectEndpointStat>,
}

impl From<zd::DirectStatus> for DirectStatus {
    fn from(s: zd::DirectStatus) -> Self {
        Self {
            phase: match s.phase {
                zd::DirectPhase::Connecting => DirectPhase::Connecting,
                zd::DirectPhase::Syncing => DirectPhase::Syncing,
                zd::DirectPhase::Live => DirectPhase::Live,
                zd::DirectPhase::Failed => DirectPhase::Failed,
            },
            last_error: s.last_error,
            retry_at_ms: s.retry_at_ms,
            engine_version: s.engine_version,
            engine_device_id: s.engine_device_id,
            notice: s.notice,
            connected_at_ms: s.connected_at_ms,
            synced_at_ms: s.synced_at_ms,
            streams: s
                .streams
                .into_iter()
                .map(|t| DirectStreamStat {
                    name: t.name,
                    frames: t.frames,
                    rows: t.rows,
                    skipped_rows: t.skipped_rows,
                    repaired_rows: t.repaired_rows,
                    last_frame_ms: t.last_frame_ms,
                    error: t.error,
                })
                .collect(),
            log: s
                .log
                .into_iter()
                .map(|l| DirectLogLine {
                    at_ms: l.at_ms,
                    message: l.message,
                })
                .collect(),
            clock_offset_ms: s.clock_offset_ms,
            endpoints: s
                .endpoints
                .into_iter()
                .map(|e| DirectEndpointStat {
                    host: e.host,
                    port: e.port,
                    kind: e.kind,
                    active: e.active,
                    last_attempt_ms: e.last_attempt_ms,
                    last_ok_ms: e.last_ok_ms,
                    last_error: e.last_error,
                    latency_ms: e.latency_ms,
                })
                .collect(),
        }
    }
}

/// New ed25519 key (the phone's identity for SSH).
#[uniffi::export]
pub fn ssh_generate_key(comment: String) -> Result<SshKeyPair, SshError> {
    Ok(zd::generate_ed25519(&comment)?.into())
}

/// Parse a pasted private key; returns it unencrypted plus its public half.
#[uniffi::export]
pub fn ssh_import_key(
    private_key: String,
    passphrase: Option<String>,
) -> Result<SshKeyPair, SshError> {
    Ok(zd::import_key(&private_key, passphrase.as_deref())?.into())
}

/// SSH auth + tunnel + `EngineInfo`. An unpinned target fails with
/// `HostKeyUnknown` carrying the fingerprint to confirm.
#[uniffi::export]
pub async fn ssh_probe(target: SshTarget) -> Result<ProbeResult, SshError> {
    let target: zd::SshTarget = target.into();
    let joined = zc::runtime::shared()
        .spawn(async move { zd::probe(&target).await })
        .await;
    match joined {
        Ok(Ok(p)) => Ok(ProbeResult {
            host_key_fingerprint: p.host_key_fingerprint,
            host_key_algorithm: p.host_key_algorithm,
            engine_device_id: p.engine_device_id,
            engine_version: p.engine_version,
            latency_ms: p.latency_ms,
        }),
        Ok(Err(e)) => Err(e.into()),
        Err(e) => Err(SshError::Connect {
            reason: format!("probe task failed: {e}"),
        }),
    }
}

/// Default engine IPC port.
#[uniffi::export]
pub fn ssh_default_engine_port() -> u16 {
    zd::DEFAULT_ENGINE_PORT
}
