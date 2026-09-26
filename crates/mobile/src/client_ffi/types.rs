//! FFI records/enums mirroring `zeron-client`'s view models, plus the
//! conversions both ways. Plain data: Swift/Kotlin get value types.
//!
//! Wire-string ids (harness `claude-code`, effort `xhigh`) stay strings so a
//! newer host's values never fail to cross the boundary.

use std::collections::HashMap;
use std::sync::Arc;

use zeron_client as zc;

// ── errors ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum CoreError {
    #[error("not found: {message}")]
    NotFound { message: String },
    #[error("invalid argument: {message}")]
    InvalidArgument { message: String },
    /// The host device could not be reached over the relay.
    #[error("host unavailable: {message}")]
    HostUnavailable { message: String },
    #[error("not supported by the host: {message}")]
    Unsupported { message: String },
    /// The host answered with an error (bad request, git failure, …).
    #[error("host error: {message}")]
    HostError { message: String },
    #[error("network: {message}")]
    Network { message: String },
    /// Credentials rejected. Irrecoverable cases also raise `AuthExpired`.
    #[error("auth: {message}")]
    Auth { message: String },
    #[error("storage: {message}")]
    Storage { message: String },
    #[error("not implemented yet: {message}")]
    NotImplemented { message: String },
    #[error("client is shut down")]
    Closed,
    #[error("{message}")]
    Internal { message: String },
}

impl From<zc::ClientError> for CoreError {
    fn from(err: zc::ClientError) -> Self {
        use zc::ClientError as E;
        match err {
            E::NotFound(message) => CoreError::NotFound { message },
            E::InvalidArgument(message) => CoreError::InvalidArgument { message },
            E::HostUnavailable(message) => CoreError::HostUnavailable { message },
            E::Unsupported(message) => CoreError::Unsupported { message },
            E::Network(message) => CoreError::Network { message },
            E::HostError(message) => CoreError::HostError { message },
            E::Auth(message) => CoreError::Auth { message },
            E::Storage(message) => CoreError::Storage { message },
            E::NotImplemented(message) => CoreError::NotImplemented { message },
            E::Closed => CoreError::Closed,
            E::Internal(message) => CoreError::Internal { message },
        }
    }
}

pub type CoreResult<T> = Result<T, CoreError>;

// ── config / credentials ──────────────────────────────────────────────────

/// Static per-install configuration.
#[derive(Debug, Clone, uniffi::Record)]
pub struct CoreConfig {
    /// Edge base URL (`auth_production_edge_url()` for prod).
    pub edge_url: String,
    /// Writable directory for registry/doc snapshots and caches, scoped by
    /// the platform to the signed-in identity (sign-out wipes it).
    pub data_dir: String,
    /// Stable device id (`ios-xxxxxxxx`), stamped on every command/queue row.
    pub device_id: String,
    /// Human name for presence/attribution ("Wing's iPhone").
    pub device_name: String,
    /// `ios` / `android`.
    pub platform: String,
    pub app_version: String,
}

impl From<CoreConfig> for zc::ClientConfig {
    fn from(c: CoreConfig) -> Self {
        zc::ClientConfig {
            edge_url: c.edge_url,
            data_dir: c.data_dir.into(),
            device_id: c.device_id,
            device_name: c.device_name,
            platform: c.platform,
            app_version: c.app_version,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AuthTokens {
    pub access_token: String,
    pub refresh_token: String,
}

impl From<zc::AuthTokens> for AuthTokens {
    fn from(t: zc::AuthTokens) -> Self {
        Self {
            access_token: t.access_token,
            refresh_token: t.refresh_token,
        }
    }
}

impl From<AuthTokens> for zc::AuthTokens {
    fn from(t: AuthTokens) -> Self {
        Self {
            access_token: t.access_token,
            refresh_token: t.refresh_token,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum DemoFixture {
    /// Devices, projects, pinned + sectioned sessions, PRs in every state.
    Standard,
    /// No projects and no sessions (empty states).
    NoProjects,
    /// Only this phone — no execution hosts.
    IosOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum TranscriptScale {
    Normal,
    /// 120 synthetic turns.
    Big,
    /// 600 synthetic turns.
    Huge,
    Turns {
        count: u32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum StreamSpeed {
    /// Word by word at 30–140ms.
    Realistic,
    /// ~4ms per word (benchmarks).
    Fast,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct DemoOptions {
    pub fixture: DemoFixture,
    /// Size of the flagship chat's (`chat-veil`) transcript.
    pub transcript_scale: TranscriptScale,
    pub stream_speed: StreamSpeed,
    /// Repeat the scripted reply 12× (long streaming stress).
    pub long_reply: bool,
}

impl From<DemoOptions> for zc::DemoOptions {
    fn from(o: DemoOptions) -> Self {
        zc::DemoOptions {
            fixture: match o.fixture {
                DemoFixture::Standard => zc::DemoFixture::Standard,
                DemoFixture::NoProjects => zc::DemoFixture::NoProjects,
                DemoFixture::IosOnly => zc::DemoFixture::IosOnly,
            },
            transcript_scale: match o.transcript_scale {
                TranscriptScale::Normal => zc::TranscriptScale::Normal,
                TranscriptScale::Big => zc::TranscriptScale::Big,
                TranscriptScale::Huge => zc::TranscriptScale::Huge,
                TranscriptScale::Turns { count } => zc::TranscriptScale::Turns(count),
            },
            stream_speed: match o.stream_speed {
                StreamSpeed::Realistic => zc::StreamSpeed::Realistic,
                StreamSpeed::Fast => zc::StreamSpeed::Fast,
            },
            long_reply: o.long_reply,
        }
    }
}

/// How the client authenticates against the edge.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum Credentials {
    /// WorkOS session scoped to `org_id`.
    WorkOs {
        user_id: String,
        org_id: String,
        tokens: AuthTokens,
    },
    /// `AUTH_MODE=dev` edge: the bearer is `userId@orgId`.
    Dev { user_id: String, org_id: String },
    /// Fully offline dataset with a simulated host.
    Demo { options: DemoOptions },
}

impl From<Credentials> for zc::Credentials {
    fn from(c: Credentials) -> Self {
        match c {
            Credentials::WorkOs {
                user_id,
                org_id,
                tokens,
            } => zc::Credentials::WorkOs {
                user_id,
                org_id,
                tokens: tokens.into(),
            },
            Credentials::Dev { user_id, org_id } => zc::Credentials::Dev { user_id, org_id },
            Credentials::Demo { options } => zc::Credentials::Demo(options.into()),
        }
    }
}

// ── auth ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AuthUser {
    pub id: String,
    pub email: Option<String>,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
}

impl From<zc::auth::AuthUser> for AuthUser {
    fn from(u: zc::auth::AuthUser) -> Self {
        Self {
            id: u.id,
            email: u.email,
            first_name: u.first_name,
            last_name: u.last_name,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AuthOrg {
    pub id: String,
    pub organization_id: String,
    pub name: String,
}

impl From<zc::auth::AuthOrg> for AuthOrg {
    fn from(o: zc::auth::AuthOrg) -> Self {
        Self {
            id: o.id,
            organization_id: o.organization_id,
            name: o.name,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AuthExchange {
    pub user: AuthUser,
    /// Unscoped pair: refresh with an organization id before use.
    pub tokens: AuthTokens,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum AuthCallback {
    Code {
        code: String,
        state: Option<String>,
    },
    Error {
        error: String,
        description: Option<String>,
    },
}

impl From<zc::auth::AuthCallback> for AuthCallback {
    fn from(c: zc::auth::AuthCallback) -> Self {
        match c {
            zc::auth::AuthCallback::Code { code, state } => AuthCallback::Code { code, state },
            zc::auth::AuthCallback::Error { error, description } => {
                AuthCallback::Error { error, description }
            }
        }
    }
}

// ── events / connectivity ─────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ConnectivityState {
    /// Demo / no edge transports — hide the pill.
    Disabled,
    /// No network path (graced): sends are saved locally.
    Offline,
    /// The registry room is down (graced).
    Reconnecting,
    Connected,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Connectivity {
    pub state: ConnectivityState,
    /// Epoch ms of the next scheduled redial (countdown), if any.
    pub retry_at_ms: Option<i64>,
    /// The failure that started the current outage.
    pub last_failure: Option<String>,
    /// Open chats whose room is graced-degraded.
    pub degraded_chats: Vec<String>,
}

impl From<zc::Connectivity> for Connectivity {
    fn from(c: zc::Connectivity) -> Self {
        Self {
            state: match c.state {
                zc::ConnectivityState::Disabled => ConnectivityState::Disabled,
                zc::ConnectivityState::Offline => ConnectivityState::Offline,
                zc::ConnectivityState::Reconnecting => ConnectivityState::Reconnecting,
                zc::ConnectivityState::Connected => ConnectivityState::Connected,
            },
            retry_at_ms: c.retry_at_ms,
            last_failure: c.last_failure,
            degraded_chats: c.degraded_chats,
        }
    }
}

/// Coalesced change notification. Pull the new state after receiving one;
/// delivered on a client thread — hop to the main thread and return fast.
#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum ClientEvent {
    /// `workspace()` advanced.
    WorkspaceChanged {
        revision: u64,
    },
    /// A session's transcript advanced (the layout engine consumes it in Rust;
    /// this is informational for the platform).
    SessionChanged {
        chat_id: String,
        revision: u64,
    },
    /// A session's `composer()` advanced.
    ComposerChanged {
        chat_id: String,
        revision: u64,
    },
    ConnectivityChanged {
        connectivity: Connectivity,
    },
    /// The WorkOS pair rotated — persist it now (the old refresh token is spent).
    AuthRefreshed {
        tokens: AuthTokens,
    },
    /// The session can't be refreshed any more — sign out.
    AuthExpired {
        reason: String,
    },
}

impl From<zc::ClientEvent> for ClientEvent {
    fn from(e: zc::ClientEvent) -> Self {
        match e {
            zc::ClientEvent::WorkspaceChanged { revision } => {
                ClientEvent::WorkspaceChanged { revision }
            }
            zc::ClientEvent::SessionChanged { chat_id, revision } => {
                ClientEvent::SessionChanged { chat_id, revision }
            }
            zc::ClientEvent::ComposerChanged { chat_id, revision } => {
                ClientEvent::ComposerChanged { chat_id, revision }
            }
            zc::ClientEvent::ConnectivityChanged(c) => ClientEvent::ConnectivityChanged {
                connectivity: c.into(),
            },
            zc::ClientEvent::AuthRefreshed(tokens) => ClientEvent::AuthRefreshed {
                tokens: tokens.into(),
            },
            zc::ClientEvent::AuthExpired { reason } => ClientEvent::AuthExpired { reason },
        }
    }
}

// ── workspace ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ChatIndicator {
    Working,
    AwaitingInput,
    Errored,
    /// Finished but not seen yet on any device.
    Completed,
    Idle,
}

impl From<zc::ChatIndicator> for ChatIndicator {
    fn from(i: zc::ChatIndicator) -> Self {
        match i {
            zc::ChatIndicator::Working => ChatIndicator::Working,
            zc::ChatIndicator::AwaitingInput => ChatIndicator::AwaitingInput,
            zc::ChatIndicator::Errored => ChatIndicator::Errored,
            zc::ChatIndicator::Completed => ChatIndicator::Completed,
            zc::ChatIndicator::Idle => ChatIndicator::Idle,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SendState {
    Sending,
    /// Durable, waiting on a degraded path.
    Queued,
    /// "Not delivered — retry".
    Failed,
}

impl From<zc::SendState> for SendState {
    fn from(s: zc::SendState) -> Self {
        match s {
            zc::SendState::Sending => SendState::Sending,
            zc::SendState::Queued => SendState::Queued,
            zc::SendState::Failed => SendState::Failed,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum PullRequestState {
    Open,
    Merged,
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PullRequest {
    pub provider: String,
    pub number: u64,
    pub title: String,
    pub url: String,
    pub state: PullRequestState,
    pub base_ref: String,
    pub head_ref: String,
}

impl From<&zc::ChangeRequestSummary> for PullRequest {
    fn from(p: &zc::ChangeRequestSummary) -> Self {
        Self {
            provider: p.provider.clone(),
            number: p.number,
            title: p.title.clone(),
            url: p.url.clone(),
            state: match p.state {
                zc::ChangeRequestState::Open => PullRequestState::Open,
                zc::ChangeRequestState::Merged => PullRequestState::Merged,
                zc::ChangeRequestState::Closed => PullRequestState::Closed,
            },
            base_ref: p.base_ref.clone(),
            head_ref: p.head_ref.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ProjectRef {
    pub id: String,
    pub name: String,
    /// Stable palette slot (0..PROJECT_COLOR_COUNT).
    pub color_index: u32,
}

/// One session row, as every list renders it.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SessionRow {
    pub id: String,
    /// Content hash — equal ⇒ nothing to redraw.
    pub revision: u64,
    /// Display title ("New session" when untitled).
    pub title: String,
    pub has_title: bool,
    pub preview: Option<String>,
    /// `None` = projectless (host's home folder).
    pub project: Option<ProjectRef>,
    pub device_id: String,
    pub device_name: Option<String>,
    pub device_online: bool,
    /// Wire harness id (`claude-code`).
    pub harness: Option<String>,
    pub harness_label: Option<String>,
    pub model: Option<String>,
    pub model_label: Option<String>,
    /// Wire effort (`xhigh`).
    pub reasoning: Option<String>,
    pub branch: Option<String>,
    pub cwd: Option<String>,
    /// Staleness-gated (45s) live status; an in-flight send reads Working.
    pub indicator: ChatIndicator,
    /// Host-reported status only (no local-send override).
    pub host_indicator: ChatIndicator,
    /// Run start of the live turn while Working/AwaitingInput.
    pub working_since_ms: Option<i64>,
    pub last_activity_ms: i64,
    /// `now` / `34m` / `4h` / `2d` at derive time.
    pub time_label: String,
    pub created_at_ms: i64,
    pub unseen: bool,
    pub archived: bool,
    pub pinned: bool,
    pub section_id: Option<String>,
    pub pull_request: Option<PullRequest>,
    /// Oldest unadopted send from this device (open sessions only).
    pub send_state: Option<SendState>,
    pub parent_chat_id: Option<String>,
    /// 2 = chat2; 1 = legacy (not dialable).
    pub room_gen: u32,
}

impl From<&zc::SessionRow> for SessionRow {
    fn from(r: &zc::SessionRow) -> Self {
        Self {
            id: r.id.clone(),
            revision: r.revision,
            title: r.title.clone(),
            has_title: r.has_title,
            preview: r.preview.clone(),
            project: r.project.as_ref().map(|p| ProjectRef {
                id: p.id.clone(),
                name: p.name.clone(),
                color_index: p.color_index,
            }),
            device_id: r.device_id.clone(),
            device_name: r.device_name.clone(),
            device_online: r.device_online,
            harness: r.harness.clone(),
            harness_label: r.harness_label.clone(),
            model: r.model.clone(),
            model_label: r.model_label.clone(),
            reasoning: r.reasoning.clone(),
            branch: r.branch.clone(),
            cwd: r.cwd.clone(),
            indicator: r.indicator.into(),
            host_indicator: r.host_indicator.into(),
            working_since_ms: r.working_since_ms,
            last_activity_ms: r.last_activity_ms,
            time_label: r.time_label.clone(),
            created_at_ms: r.created_at_ms,
            unseen: r.unseen,
            archived: r.archived,
            pinned: r.pinned,
            section_id: r.section_id.clone(),
            pull_request: r.pull_request.as_ref().map(PullRequest::from),
            send_state: r.send_state.map(Into::into),
            parent_chat_id: r.parent_chat_id.clone(),
            room_gen: r.room_gen,
        }
    }
}

pub(crate) fn rows(rows: &[Arc<zc::SessionRow>]) -> Vec<SessionRow> {
    rows.iter().map(|r| SessionRow::from(&**r)).collect()
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SectionView {
    pub id: String,
    pub name: String,
    pub collapsed: bool,
    /// Recency order (pinned sessions excluded).
    pub sessions: Vec<SessionRow>,
}

/// The Threads page — the desktop sidebar order.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct FrontPage {
    /// Shared manual pin order.
    pub pinned: Vec<SessionRow>,
    /// User sections ("folders"), each in recency order.
    pub sections: Vec<SectionView>,
    /// Everything else, by recency.
    pub recent: Vec<SessionRow>,
}

impl From<&zc::FrontPage> for FrontPage {
    fn from(f: &zc::FrontPage) -> Self {
        Self {
            pinned: rows(&f.pinned),
            sections: f
                .sections
                .iter()
                .map(|s| SectionView {
                    id: s.id.clone(),
                    name: s.name.clone(),
                    collapsed: s.collapsed,
                    sessions: rows(&s.sessions),
                })
                .collect(),
            recent: rows(&f.recent),
        }
    }
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ProjectView {
    pub id: String,
    pub name: String,
    pub path: String,
    pub color_index: u32,
    pub device_id: String,
    pub device_name: Option<String>,
    pub device_online: bool,
    pub git_detected: bool,
    pub created_at_ms: i64,
    /// Most urgent indicator among its active sessions.
    pub indicator: ChatIndicator,
    pub unseen_count: u32,
    /// Active sessions, recency order.
    pub sessions: Vec<SessionRow>,
}

impl From<&zc::ProjectView> for ProjectView {
    fn from(p: &zc::ProjectView) -> Self {
        Self {
            id: p.id.clone(),
            name: p.name.clone(),
            path: p.path.clone(),
            color_index: p.color_index,
            device_id: p.device_id.clone(),
            device_name: p.device_name.clone(),
            device_online: p.device_online,
            git_detected: p.git_detected,
            created_at_ms: p.created_at_ms,
            indicator: p.indicator.into(),
            unseen_count: p.unseen_count,
            sessions: rows(&p.sessions),
        }
    }
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct PullRequestGroups {
    pub open: Vec<SessionRow>,
    pub merged: Vec<SessionRow>,
    pub closed: Vec<SessionRow>,
}

impl From<&zc::PullRequestGroups> for PullRequestGroups {
    fn from(p: &zc::PullRequestGroups) -> Self {
        Self {
            open: rows(&p.open),
            merged: rows(&p.merged),
            closed: rows(&p.closed),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct DeviceView {
    pub id: String,
    pub name: String,
    pub platform: String,
    pub online: bool,
    pub last_seen_ms: Option<i64>,
    pub version: Option<String>,
    pub capabilities: Vec<String>,
    /// Can run sessions (desktop/server engines — not phones).
    pub is_execution_host: bool,
    pub is_self: bool,
    pub session_count: u32,
}

impl From<&zc::DeviceView> for DeviceView {
    fn from(d: &zc::DeviceView) -> Self {
        Self {
            id: d.id.clone(),
            name: d.name.clone(),
            platform: d.platform.clone(),
            online: d.online,
            last_seen_ms: d.last_seen_ms,
            version: d.version.clone(),
            capabilities: d.capabilities.clone(),
            is_execution_host: d.is_execution_host,
            is_self: d.is_self,
            session_count: d.session_count,
        }
    }
}

/// Everything the workspace screens render.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct WorkspaceSnapshot {
    pub revision: u64,
    pub computed_at_ms: i64,
    /// Pin/section edits allowed (sidebar prefs known).
    pub pins_ready: bool,
    /// The registry reached this device at least once (or demo).
    pub synced: bool,
    pub front: FrontPage,
    /// Creation order.
    pub projects: Vec<ProjectView>,
    /// Active sessions without a project.
    pub projectless: Vec<SessionRow>,
    pub pull_requests: PullRequestGroups,
    pub archived: Vec<SessionRow>,
    pub devices: Vec<DeviceView>,
}

impl From<&zc::WorkspaceSnapshot> for WorkspaceSnapshot {
    fn from(w: &zc::WorkspaceSnapshot) -> Self {
        Self {
            revision: w.revision,
            computed_at_ms: w.computed_at_ms,
            pins_ready: w.pins_ready,
            synced: w.synced,
            front: (&w.front).into(),
            projects: w.projects.iter().map(Into::into).collect(),
            projectless: rows(&w.projectless),
            pull_requests: (&w.pull_requests).into(),
            archived: rows(&w.archived),
            devices: w.devices.iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SearchField {
    Title,
    Project,
    Branch,
    Preview,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SearchHit {
    pub session: SessionRow,
    pub score: u32,
    /// The strongest field that matched.
    pub field: SearchField,
}

impl From<&zc::SearchHit> for SearchHit {
    fn from(h: &zc::SearchHit) -> Self {
        Self {
            session: (&*h.session).into(),
            score: h.score,
            field: match h.field {
                zc::SearchField::Title => SearchField::Title,
                zc::SearchField::Project => SearchField::Project,
                zc::SearchField::Branch => SearchField::Branch,
                zc::SearchField::Preview => SearchField::Preview,
            },
        }
    }
}

// ── chat config / new sessions ────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SandboxLevel {
    ReadOnly,
    WorkspaceWrite,
    DangerFullAccess,
}

/// A chat's run configuration. Ids are wire strings.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ChatConfig {
    /// `claude-code`, `codex`, …
    pub harness: String,
    pub model: Option<String>,
    /// `low` … `ultrathink`.
    pub reasoning: Option<String>,
    /// Model option id → choice id (`contextWindow` → `1m`).
    pub model_options: HashMap<String, String>,
    pub sandbox: SandboxLevel,
}

fn wire<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn from_wire<T: serde::de::DeserializeOwned>(value: &str, what: &str) -> CoreResult<T> {
    serde_json::from_value(serde_json::Value::String(value.to_owned())).map_err(|_| {
        CoreError::InvalidArgument {
            message: format!("unknown {what} `{value}`"),
        }
    })
}

impl From<&zc::ChatConfig> for ChatConfig {
    fn from(c: &zc::ChatConfig) -> Self {
        Self {
            harness: wire(&c.harness),
            model: c.model.clone(),
            reasoning: c.reasoning.as_ref().map(wire),
            model_options: c
                .model_options
                .iter()
                .map(|(k, v)| {
                    let value = v.as_str().map_or_else(|| v.to_string(), str::to_owned);
                    (k.clone(), value)
                })
                .collect(),
            sandbox: match c.sandbox {
                zeron_proto::SandboxLevel::ReadOnly => SandboxLevel::ReadOnly,
                zeron_proto::SandboxLevel::WorkspaceWrite => SandboxLevel::WorkspaceWrite,
                zeron_proto::SandboxLevel::DangerFullAccess => SandboxLevel::DangerFullAccess,
            },
        }
    }
}

impl TryFrom<ChatConfig> for zc::ChatConfig {
    type Error = CoreError;

    fn try_from(c: ChatConfig) -> CoreResult<Self> {
        Ok(zc::ChatConfig {
            harness: from_wire(&c.harness, "harness")?,
            model: c.model,
            reasoning: c
                .reasoning
                .as_deref()
                .map(|r| from_wire(r, "reasoning level"))
                .transpose()?,
            model_options: c
                .model_options
                .into_iter()
                .map(|(k, v)| (k, serde_json::Value::String(v)))
                .collect(),
            sandbox: match c.sandbox {
                SandboxLevel::ReadOnly => zeron_proto::SandboxLevel::ReadOnly,
                SandboxLevel::WorkspaceWrite => zeron_proto::SandboxLevel::WorkspaceWrite,
                SandboxLevel::DangerFullAccess => zeron_proto::SandboxLevel::DangerFullAccess,
            },
        })
    }
}

/// Where a new session runs.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum SessionTarget {
    /// In a project (host = its device, cwd = its folder or `cwd` override).
    Project { space_id: String },
    /// No project: the chosen host's home folder.
    Projectless { device_id: String },
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct NewSession {
    pub target: SessionTarget,
    pub config: Option<ChatConfig>,
    /// Branch stamped from the first frame (picked ref).
    pub branch: Option<String>,
    /// Checkout override (an existing worktree's path).
    pub cwd: Option<String>,
    pub title: Option<String>,
}

impl TryFrom<NewSession> for zc::NewSession {
    type Error = CoreError;

    fn try_from(n: NewSession) -> CoreResult<Self> {
        Ok(zc::NewSession {
            target: match n.target {
                SessionTarget::Project { space_id } => zc::SessionTarget::Project { space_id },
                SessionTarget::Projectless { device_id } => {
                    zc::SessionTarget::Projectless { device_id }
                }
            },
            config: n.config.map(TryInto::try_into).transpose()?,
            branch: n.branch,
            cwd: n.cwd,
            title: n.title,
        })
    }
}

// ── catalogs / host RPC ───────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct HarnessInfo {
    pub id: String,
    pub label: String,
    pub supports_steering: Option<bool>,
    /// `step-boundary` / `turn-boundary`.
    pub steering_mode: Option<String>,
    pub reasoning_levels: Vec<String>,
    pub installed: bool,
    pub enabled: Option<bool>,
    /// Installed and not disabled on the device.
    pub offered: bool,
    /// Steers mid-turn (unknown until a live catalog).
    pub mid_turn_steering: Option<bool>,
}

impl From<zc::catalog::HarnessInfo> for HarnessInfo {
    fn from(h: zc::catalog::HarnessInfo) -> Self {
        Self {
            offered: h.offered(),
            mid_turn_steering: h.mid_turn_steering(),
            id: h.id,
            label: h.label,
            supports_steering: h.supports_steering,
            steering_mode: h.steering_mode,
            reasoning_levels: h.reasoning_levels,
            installed: h.installed,
            enabled: h.enabled,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ModelOptionChoice {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ModelOption {
    pub id: String,
    pub label: String,
    pub choices: Vec<ModelOptionChoice>,
    pub default_choice: String,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ModelInfo {
    pub id: String,
    pub label: String,
    pub description: Option<String>,
    /// Wire effort values; empty = no effort ladder.
    pub reasoning_levels: Vec<String>,
    pub options: Vec<ModelOption>,
    /// The picker's default effort for this model.
    pub default_reasoning: Option<String>,
}

impl From<zc::catalog::ModelInfo> for ModelInfo {
    fn from(m: zc::catalog::ModelInfo) -> Self {
        Self {
            default_reasoning: zc::catalog::default_reasoning(&m),
            id: m.id,
            label: m.label,
            description: m.description,
            reasoning_levels: m.reasoning_levels,
            options: m
                .options
                .into_iter()
                .map(|o| ModelOption {
                    id: o.id,
                    label: o.label,
                    choices: o
                        .choices
                        .into_iter()
                        .map(|c| ModelOptionChoice {
                            id: c.id,
                            label: c.label,
                        })
                        .collect(),
                    default_choice: o.default_choice,
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RepoRef {
    pub name: String,
    /// Checked out in the repo's main folder right now.
    pub current: bool,
    /// Linked worktree this branch is checked out in, if any.
    pub worktree_path: Option<String>,
}

impl From<zc::rpc::RepoRef> for RepoRef {
    fn from(r: zc::rpc::RepoRef) -> Self {
        Self {
            name: r.name,
            current: r.current,
            worktree_path: r.worktree_path,
        }
    }
}

impl From<RepoRef> for zc::rpc::RepoRef {
    fn from(r: RepoRef) -> Self {
        Self {
            name: r.name,
            current: r.current,
            worktree_path: r.worktree_path,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FolderEntry {
    pub name: String,
    pub is_dir: bool,
    pub is_repo: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FolderListing {
    pub path: String,
    pub entries: Vec<FolderEntry>,
    pub truncated: bool,
}

impl From<zc::rpc::FolderListing> for FolderListing {
    fn from(l: zc::rpc::FolderListing) -> Self {
        Self {
            path: l.path,
            entries: l
                .entries
                .into_iter()
                .map(|e| FolderEntry {
                    name: e.name,
                    is_dir: e.is_dir,
                    is_repo: e.is_repo,
                })
                .collect(),
            truncated: l.truncated,
        }
    }
}

// ── attachments ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AppshotLabel {
    pub app_name: String,
    pub window_title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct UserImage {
    /// Host path (or `pending://` ref) — read via `read_attachment`.
    pub path: String,
    pub name: String,
    pub appshot: Option<AppshotLabel>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ParsedUserMessage {
    /// Visible prompt (trailer + Appshot context stripped).
    pub text: String,
    pub images: Vec<UserImage>,
}

impl From<zc::attachments::ParsedUserMessage> for ParsedUserMessage {
    fn from(p: zc::attachments::ParsedUserMessage) -> Self {
        Self {
            text: p.text,
            images: p
                .images
                .into_iter()
                .map(|i| UserImage {
                    path: i.path,
                    name: i.name,
                    appshot: i.appshot.map(|a| AppshotLabel {
                        app_name: a.app_name,
                        window_title: a.window_title,
                    }),
                })
                .collect(),
        }
    }
}
