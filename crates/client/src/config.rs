//! Construction inputs: where the edge is, who this device is, and how it
//! authenticates. Token *persistence* stays on the platform (Keychain /
//! Keystore): the client takes tokens in and reports rotations through
//! [`crate::ClientEvent::AuthRefreshed`].

use std::path::PathBuf;

/// Static per-install configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientConfig {
    /// Edge base URL, e.g. `https://edge.zeron.sh` (no trailing slash needed).
    pub edge_url: String,
    /// Writable directory for registry/doc snapshots and caches. Scoped by the
    /// platform to the signed-in identity (sign-out wipes it).
    pub data_dir: PathBuf,
    /// This device's stable id (`ios-xxxxxxxx`) — stamped on every command
    /// and queue row this device writes.
    pub device_id: String,
    /// Human name for presence/attribution ("Wing's iPhone").
    pub device_name: String,
    /// `ios` / `android` — viewer platforms never host sessions.
    pub platform: String,
    /// App version string (diagnostics only).
    pub app_version: String,
}

impl ClientConfig {
    pub fn new(edge_url: impl Into<String>, data_dir: impl Into<PathBuf>) -> Self {
        Self {
            edge_url: edge_url.into(),
            data_dir: data_dir.into(),
            device_id: format!("ios-{}", &crate::new_id()[..8]),
            device_name: "iPhone".into(),
            platform: "ios".into(),
            app_version: env!("CARGO_PKG_VERSION").into(),
        }
    }

    pub(crate) fn edge_base(&self) -> &str {
        self.edge_url.trim_end_matches('/')
    }
}

/// WorkOS access/refresh pair. Refresh tokens are SINGLE-USE (rotated per
/// refresh) — the client single-flights every refresh and hands the rotated
/// pair back through [`crate::ClientEvent::AuthRefreshed`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthTokens {
    pub access_token: String,
    pub refresh_token: String,
}

/// How this client authenticates against the edge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Credentials {
    /// WorkOS session scoped to `org_id` (the token carries the `org_id`
    /// claim the registry room requires).
    WorkOs {
        user_id: String,
        org_id: String,
        tokens: AuthTokens,
    },
    /// `AUTH_MODE=dev` edge: the bearer is `userId@orgId`.
    Dev { user_id: String, org_id: String },
    /// Fully offline, deterministic dataset with a simulated host.
    Demo(DemoOptions),
}

impl Credentials {
    pub fn is_demo(&self) -> bool {
        matches!(self, Credentials::Demo(_))
    }

    pub fn org_id(&self) -> &str {
        match self {
            Credentials::WorkOs { org_id, .. } | Credentials::Dev { org_id, .. } => org_id,
            Credentials::Demo(_) => "demo",
        }
    }

    pub fn user_id(&self) -> &str {
        match self {
            Credentials::WorkOs { user_id, .. } | Credentials::Dev { user_id, .. } => user_id,
            Credentials::Demo(_) => "demo",
        }
    }
}

/// Demo-mode knobs (the legacy app's `-demo` launch args).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DemoOptions {
    pub fixture: DemoFixture,
    /// Size of the flagship chat's transcript (`chat-veil`).
    pub transcript_scale: TranscriptScale,
    pub stream_speed: StreamSpeed,
    /// Repeat the scripted reply 12× (long streaming stress, `-longreply`).
    pub long_reply: bool,
}

impl Default for DemoOptions {
    fn default() -> Self {
        Self {
            fixture: DemoFixture::Standard,
            transcript_scale: TranscriptScale::Normal,
            stream_speed: StreamSpeed::Realistic,
            long_reply: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DemoFixture {
    /// Devices, projects, pinned + sectioned sessions, PRs in every state.
    Standard,
    /// No projects and no sessions (`-no-projects`): empty-state screens.
    NoProjects,
    /// Only this phone — no execution hosts (`-ios-only`).
    IosOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptScale {
    /// Hand-written fixtures.
    Normal,
    /// 120 synthetic turns (`-big`).
    Big,
    /// 600 synthetic turns (`-huge`).
    Huge,
    /// Arbitrary synthetic size (benchmarks).
    Turns(u32),
}

impl TranscriptScale {
    pub fn turns(self) -> Option<u32> {
        match self {
            TranscriptScale::Normal => None,
            TranscriptScale::Big => Some(120),
            TranscriptScale::Huge => Some(600),
            TranscriptScale::Turns(n) => Some(n),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamSpeed {
    /// Word by word at 30–140ms — what a real harness feels like.
    Realistic,
    /// A word every ~4ms: stresses the per-update path (benchmarks).
    Fast,
}
