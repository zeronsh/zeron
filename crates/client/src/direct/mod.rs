//! Direct mode: the phone talks to the user's own machine over SSH — no
//! edge, no account. The engine's loopback IPC (the same `EngineRpc` the
//! desktop UI uses) rides a `direct-tcpip` channel; [`host::DirectHost`]
//! mirrors its registry and transcript streams into the phone's local docs,
//! so the rest of the client (views, composer, commands) is unchanged.

mod clock;
mod desktop_pins;
pub(crate) mod host;
pub(crate) mod lenient;
mod ssh;
#[cfg(test)]
mod tests;

pub use ssh::{ProbeResult, SshKeyPair, generate_ed25519, import_key, probe};

/// Tests only: how often an open transcript's own tunnel channel is checked
/// for backlog and how long the check may take (defaults 10 s / 15 s).
#[doc(hidden)]
pub fn set_transcript_lag_check(every: std::time::Duration, limit: std::time::Duration) {
    host::set_lag_check(every, limit);
}

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
    /// Phone clock minus the computer's, measured from session heartbeats
    /// (receipt latency included). `None` until a running session beats.
    pub clock_offset_ms: Option<i64>,
    /// Every address of the machine, in dial order, with how it last fared.
    pub endpoints: Vec<EndpointStat>,
}

/// One address of the machine as the phone last saw it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EndpointStat {
    pub host: String,
    pub port: u16,
    /// The app's name for the address ("lan", "tailscale", …), echoed back.
    pub kind: String,
    /// The current link runs over this address.
    pub active: bool,
    pub last_attempt_ms: Option<i64>,
    pub last_ok_ms: Option<i64>,
    /// Why the last attempt failed (`None` after a success, or never tried).
    pub last_error: Option<String>,
    /// SSH connect + auth time of the last success.
    pub latency_ms: Option<u64>,
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
    /// Every address to try, in order (LAN, Tailscale, …). Empty = just
    /// `host:port`. They share the credentials and the pinned host key.
    pub endpoints: Vec<SshEndpoint>,
}

/// Another address of the same SSH server (LAN IP, Tailscale IP, a DNS
/// name). It must present the machine's pinned host key like the others.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshEndpoint {
    pub host: String,
    pub port: u16,
    /// The app's name for the address ("lan", "tailscale", …); shown back
    /// in [`EndpointStat`] and the link log.
    pub kind: String,
    /// How long this address is tried alone before the next one is dialled
    /// alongside it (Happy Eyeballs); 0 = only once this one has failed.
    pub head_start_ms: u32,
    /// Give up reaching this address (TCP + SSH handshake) after this long,
    /// so a silent LAN IP fails in seconds; 0 = the default (20 s).
    pub connect_timeout_ms: u32,
}

impl SshEndpoint {
    pub fn same_address(&self, host: &str, port: u16) -> bool {
        self.port == port && self.host.trim().eq_ignore_ascii_case(host.trim())
    }

    pub(crate) fn describe(&self) -> String {
        let at = format!("{}:{}", self.host.trim(), self.port);
        if self.kind.is_empty() {
            at
        } else {
            format!("{} {at}", self.kind)
        }
    }
}

impl SshTarget {
    /// The addresses to dial, in order (never empty).
    pub fn addresses(&self) -> Vec<SshEndpoint> {
        if self.endpoints.is_empty() {
            vec![SshEndpoint {
                host: self.host.clone(),
                port: self.port,
                kind: String::new(),
                head_start_ms: 0,
                connect_timeout_ms: 0,
            }]
        } else {
            self.endpoints.clone()
        }
    }

    /// This target at one of its addresses.
    pub(crate) fn at(&self, endpoint: &SshEndpoint) -> SshTarget {
        SshTarget {
            host: endpoint.host.clone(),
            port: endpoint.port,
            endpoints: Vec::new(),
            ..self.clone()
        }
    }
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
    /// Which of several addresses' failures to report for the whole dial
    /// (earliest address wins a tie). Credentials and first-contact trust
    /// come first: every address shares them. A host-key mismatch comes
    /// last: away from home another device can own the LAN IP, so it only
    /// counts (and asks the user) when every address answered with it.
    pub(crate) fn most_telling(errors: Vec<(usize, SshError)>) -> Option<SshError> {
        let rank = |e: &SshError| match e {
            SshError::Auth(_) | SshError::Key(_) => 0,
            SshError::HostKeyUnknown { .. } => 1,
            SshError::Engine(_) => 2,
            SshError::Connect(_) => 3,
            SshError::HostKeyMismatch { .. } => 4,
        };
        errors
            .into_iter()
            .min_by_key(|(i, e)| (rank(e), *i))
            .map(|(_, e)| e)
    }

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
