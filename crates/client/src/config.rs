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
    /// `ios` / `android` (informational: whether a device hosts sessions is
    /// what its engine's registry row advertises).
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
    /// A local edge embedded in an engine on this same device (docs/android.md
    /// — the phone's own `zeron headless`): the bearer is the shared secret
    /// verbatim, and the identity is the fixed [`LOCAL_IDENTITY`] user and
    /// org the engine runs under.
    Local { token: String },
    /// The engine on this same device owns the account (docs/android.md):
    /// the viewer shares its device id, and the bearer is whatever the engine
    /// presents to its edge, fetched over its IPC port (`EdgeBearer`, see
    /// [`crate::engine`]) — the engine alone refreshes it. `user_id` /
    /// `org_id` are the identity the engine reported with it.
    Engine {
        ipc_url: String,
        ipc_token: Option<String>,
        user_id: String,
        org_id: String,
    },
    /// Fully offline, deterministic dataset with a simulated host.
    Demo(DemoOptions),
}

/// User and org of a local-edge workspace (the engine's
/// `LOCAL_EDGE_IDENTITY`): one tenant, so the ids are constants.
pub const LOCAL_IDENTITY: &str = "local";

impl Credentials {
    pub fn is_demo(&self) -> bool {
        matches!(self, Credentials::Demo(_))
    }

    /// The viewer is its device's engine's other half: the engine publishes
    /// the device's presence, not the viewer.
    pub fn shares_engine_device(&self) -> bool {
        matches!(self, Credentials::Engine { .. })
    }

    /// The writer id this client syncs as: its chat-room sockets and row
    /// pushes, and its registry HLCs' tiebreaker. Normally the device id; a
    /// viewer sharing its engine's device id writes as `{device_id}-viewer`,
    /// because a reconnect's backfill skips the rows its own writer id pushed
    /// (`excludeOwn`) — the engine's rows must still reach the viewer and the
    /// viewer's (a sent message) the engine — and two replicas clocking as
    /// one device could mint identical HLCs.
    pub fn writer_id(&self, device_id: &str) -> String {
        if self.shares_engine_device() {
            format!("{device_id}-viewer")
        } else {
            device_id.to_owned()
        }
    }

    pub fn org_id(&self) -> &str {
        match self {
            Credentials::WorkOs { org_id, .. }
            | Credentials::Dev { org_id, .. }
            | Credentials::Engine { org_id, .. } => org_id,
            Credentials::Local { .. } => LOCAL_IDENTITY,
            Credentials::Demo(_) => "demo",
        }
    }

    pub fn user_id(&self) -> &str {
        match self {
            Credentials::WorkOs { user_id, .. }
            | Credentials::Dev { user_id, .. }
            | Credentials::Engine { user_id, .. } => user_id,
            Credentials::Local { .. } => LOCAL_IDENTITY,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The engine's and its viewer's chat rows must not look like one
    /// writer's: a resume backfill (`excludeOwn`) would skip the other's.
    #[test]
    fn a_viewer_sharing_its_engines_device_writes_chats_as_its_own_writer() {
        let engine = Credentials::Engine {
            ipc_url: "ws://127.0.0.1:27654".into(),
            ipc_token: None,
            user_id: "user_1".into(),
            org_id: "org_1".into(),
        };
        assert_eq!(engine.writer_id("dev-1"), "dev-1-viewer");
        let local = Credentials::Local {
            token: "secret".into(),
        };
        assert_eq!(local.writer_id("dev-1"), "dev-1");
    }
}
