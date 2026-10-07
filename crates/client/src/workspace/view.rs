//! Workspace view models — plain data, rebuilt from the registry replica and
//! cheap to diff (every row carries a content-hash `revision`, and unchanged
//! rows keep their `Arc` across snapshots).
//!
//! Ordering mirrors the desktop sidebar (`Shell::sidebar_visible_order`,
//! "In one list" + "Last updated"): pinned sessions in their shared manual
//! order, then the user's sections (each in recency order), then everything
//! else by recency ([`zeron_proto::view::sort_active`] keys). Archived and
//! child (side/subagent) chats never appear in the active lists, and a chat
//! pointing at a deleted project is hidden (projectless chats are first-class).

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use zeron_doc::WorkspaceState;
use zeron_proto::view::{attention_rank, display_status, project_key, representative_space};
use zeron_proto::{
    ChangeRequestState, ChangeRequestSummary, Chat, ChatIndicator, CheckoutChangeRequestStatus,
    Device, Session, SessionStatus, SidebarPreferences, Space,
};

use crate::catalog;
use crate::connectivity::{PRESENCE_FRESH_MS, SendState};

/// Size of the project color palette [`ProjectRef::color_index`] indexes —
/// the desktop's monogram palette (`ui/src/shell/project_icon.rs`).
pub const PROJECT_COLOR_COUNT: u32 = 8;

/// Stable palette slot for a project, exactly as the desktop picks its
/// monogram tone: 32-bit FNV-1a of the project's path (`"home"` without a
/// project), so a project has the same color on every device.
pub fn project_color_index(space_path: &str) -> u32 {
    let hash = space_path.bytes().fold(2_166_136_261u32, |h, b| {
        (h ^ u32::from(b)).wrapping_mul(16_777_619)
    });
    hash % PROJECT_COLOR_COUNT
}

/// Compact age label for list rows: `now`, `34m`, `4h`, `2d` (legacy
/// `relativeTime`). Future timestamps read as `now`.
pub fn relative_time_label(at_ms: i64, now_ms: i64) -> String {
    let secs = (now_ms - at_ms).max(0) / 1000;
    match secs {
        0..60 => "now".to_owned(),
        60..3_600 => format!("{}m", secs / 60),
        3_600..86_400 => format!("{}h", secs / 3_600),
        _ => format!("{}d", secs / 86_400),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProjectRef {
    pub id: String,
    pub name: String,
    pub color_index: u32,
}

/// One session row, as every list renders it.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionRow {
    pub id: String,
    /// Content hash of every field below — equal ⇒ nothing to redraw.
    pub revision: u64,
    /// Display title ("New session" when untitled).
    pub title: String,
    pub has_title: bool,
    pub preview: Option<String>,
    /// `None` = projectless session (runs in the host's home folder).
    pub project: Option<ProjectRef>,
    pub device_id: String,
    pub device_name: Option<String>,
    pub device_online: bool,
    pub harness: Option<String>,
    pub harness_label: Option<String>,
    pub model: Option<String>,
    pub model_label: Option<String>,
    pub reasoning: Option<String>,
    pub branch: Option<String>,
    pub cwd: Option<String>,
    pub indicator: ChatIndicator,
    /// The host-reported status alone (45s staleness-gated) — `indicator`
    /// minus the "a send of mine is in flight" override. What stop/busy
    /// logic keys off (a send parked for an offline host is not a turn).
    pub host_indicator: ChatIndicator,
    /// How the chat's last run ended, NOT gated on the seen marker: live
    /// states as `host_indicator`; `Errored` while the host's session row
    /// still reports an errored turn; `Completed` for any other chat that has
    /// activity; `Idle` for a chat that never ran. `indicator` clears
    /// Completed/Errored as soon as the chat is seen on any device (the tab
    /// dot's "news" semantics); lists that want a persistent outcome glyph
    /// read this instead.
    pub last_outcome: ChatIndicator,
    /// Run start of the live turn while Working/AwaitingInput.
    pub working_since_ms: Option<i64>,
    /// `last_message_at`, falling back to `created_at` (the sort key).
    pub last_activity_ms: i64,
    /// [`relative_time_label`] of `last_activity_ms` at derive time (the 1 Hz
    /// re-derive refreshes it; it changes at most once a minute).
    pub time_label: String,
    pub created_at_ms: i64,
    pub unseen: bool,
    pub archived: bool,
    pub pinned: bool,
    pub section_id: Option<String>,
    pub pull_request: Option<ChangeRequestSummary>,
    /// Oldest unadopted send from this device, if any.
    pub send_state: Option<SendState>,
    pub parent_chat_id: Option<String>,
    /// Sync room generation (2 = chat2; 1 = legacy, not dialable).
    pub room_gen: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SectionView {
    pub id: String,
    pub name: String,
    pub collapsed: bool,
    pub sessions: Vec<Arc<SessionRow>>,
}

/// The "Threads" page — the desktop sidebar.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FrontPage {
    pub pinned: Vec<Arc<SessionRow>>,
    pub sections: Vec<SectionView>,
    pub recent: Vec<Arc<SessionRow>>,
}

#[derive(Debug, Clone, PartialEq)]
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
    /// The repository project this checkout belongs to
    /// ([`zeron_proto::view::project_key`]): clones and worktrees of one
    /// repository, on any device, share it.
    pub group_key: String,
    /// The group's name — its representative checkout's — shared by every
    /// member, like `color_index`.
    pub group_name: String,
    /// Most urgent indicator among its active sessions (Idle when none).
    pub indicator: ChatIndicator,
    pub unseen_count: u32,
    /// Active sessions, recency order (the desktop's by-project grouping).
    pub sessions: Vec<Arc<SessionRow>>,
}

/// Sessions with a change request, by provider state. (The proto carries no
/// draft flag yet — drafts read as Open.)
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PullRequestGroups {
    pub open: Vec<Arc<SessionRow>>,
    pub merged: Vec<Arc<SessionRow>>,
    pub closed: Vec<Arc<SessionRow>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// This device.
    pub is_self: bool,
    pub session_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchField {
    Title,
    Project,
    Branch,
    Preview,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    pub session: Arc<SessionRow>,
    pub score: u32,
    /// The strongest field that matched.
    pub field: SearchField,
}

/// Everything the workspace screens render. Immutable; replaced wholesale.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WorkspaceSnapshot {
    /// Bumped only when content changed.
    pub revision: u64,
    pub computed_at_ms: i64,
    /// Sidebar preferences are known (cached or synced) — pin/section edits
    /// are allowed. False on a fresh install before the first registry sync.
    pub pins_ready: bool,
    /// The registry has reached this device at least once (or demo).
    pub synced: bool,
    pub front: FrontPage,
    pub projects: Vec<ProjectView>,
    /// Active sessions without a project.
    pub projectless: Vec<Arc<SessionRow>>,
    pub pull_requests: PullRequestGroups,
    pub archived: Vec<Arc<SessionRow>>,
    pub devices: Vec<DeviceView>,
    /// Every chat row (archived and children included), by id.
    pub sessions: HashMap<String, Arc<SessionRow>>,
    children: HashMap<String, Vec<Arc<SessionRow>>>,
    content_hash: u64,
}

impl WorkspaceSnapshot {
    pub fn session(&self, chat_id: &str) -> Option<&Arc<SessionRow>> {
        self.sessions.get(chat_id)
    }

    pub fn project(&self, space_id: &str) -> Option<&ProjectView> {
        self.projects.iter().find(|p| p.id == space_id)
    }

    pub fn device(&self, device_id: &str) -> Option<&DeviceView> {
        self.devices.iter().find(|d| d.id == device_id)
    }

    /// Execution hosts (new-session / new-project pickers).
    pub fn execution_devices(&self) -> Vec<DeviceView> {
        self.devices
            .iter()
            .filter(|d| d.is_execution_host)
            .cloned()
            .collect()
    }

    /// Side chats / subagent chats hanging off `parent_id`, recency order.
    pub fn children(&self, parent_id: &str) -> &[Arc<SessionRow>] {
        self.children.get(parent_id).map_or(&[], Vec::as_slice)
    }

    pub(crate) fn content_hash(&self) -> u64 {
        self.content_hash
    }

    /// Case-insensitive search over titles, project names, branches and
    /// previews. Every whitespace-separated term must match some field.
    /// Active sessions rank above archived ones; ties break by recency.
    pub fn search(&self, query: &str, limit: usize) -> Vec<SearchHit> {
        let terms: Vec<String> = query.split_whitespace().map(|t| t.to_lowercase()).collect();
        if terms.is_empty() {
            return Vec::new();
        }
        let mut hits: Vec<SearchHit> = self
            .sessions
            .values()
            .filter(|row| row.parent_chat_id.is_none())
            .filter_map(|row| score_row(row, &terms).map(|(score, field)| (row, score, field)))
            .map(|(row, score, field)| SearchHit {
                session: row.clone(),
                score: if row.archived { score / 2 } else { score },
                field,
            })
            .collect();
        hits.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then(b.session.last_activity_ms.cmp(&a.session.last_activity_ms))
                .then(a.session.id.cmp(&b.session.id))
        });
        hits.truncate(limit);
        hits
    }
}

fn score_row(row: &SessionRow, terms: &[String]) -> Option<(u32, SearchField)> {
    let title = row.title.to_lowercase();
    let project = row
        .project
        .as_ref()
        .map(|p| p.name.to_lowercase())
        .unwrap_or_default();
    let branch = row.branch.as_deref().unwrap_or("").to_lowercase();
    let preview = row.preview.as_deref().unwrap_or("").to_lowercase();
    let mut total = 0u32;
    let mut best: Option<(u32, SearchField)> = None;
    for term in terms {
        let candidates = [
            (&title, SearchField::Title, 100u32),
            (&project, SearchField::Project, 40),
            (&branch, SearchField::Branch, 30),
            (&preview, SearchField::Preview, 20),
        ];
        let mut term_best: Option<(u32, SearchField)> = None;
        for (haystack, field, weight) in candidates {
            if let Some(at) = haystack.find(term.as_str()) {
                let prefix_bonus =
                    if at == 0 || !haystack.as_bytes()[at - 1].is_ascii_alphanumeric() {
                        weight / 2
                    } else {
                        0
                    };
                let score = weight + prefix_bonus;
                if term_best.is_none_or(|(s, _)| score > s) {
                    term_best = Some((score, field));
                }
            }
        }
        let (score, field) = term_best?;
        total += score;
        if best.is_none_or(|(s, _)| score > s) {
            best = Some((score, field));
        }
    }
    best.map(|(_, field)| (total, field))
}

/// Inputs that are not in the registry replica.
pub(crate) struct DeriveContext<'a> {
    pub self_device_id: &'a str,
    pub now: DateTime<Utc>,
    /// Newest presence heartbeat per device (epoch ms).
    pub presence: &'a HashMap<String, i64>,
    /// Engine device id → when this phone last heard from that engine over
    /// a live direct link (epoch ms). See [`status_now`].
    pub heard: &'a HashMap<String, i64>,
    pub change_requests: &'a [CheckoutChangeRequestStatus],
    /// Oldest-unadopted-send state per chat (open sessions only).
    pub send_states: &'a HashMap<String, SendState>,
    pub synced: bool,
    pub previous: Option<&'a WorkspaceSnapshot>,
}

/// A device's display name, or `None` when it has none worth showing (blank,
/// or the engine's legacy `unknown-device` sentinel). Never an id.
pub(crate) fn device_display_name(device: &Device) -> Option<String> {
    let name = device.name.trim();
    (!name.is_empty() && name != "unknown-device").then(|| name.to_owned())
}

fn is_execution_host(device: &Device) -> bool {
    !matches!(device.platform.as_str(), "ios" | "android" | "ipados")
}

pub(crate) fn device_online(
    device_id: &str,
    presence: &HashMap<String, i64>,
    self_device_id: &str,
    now_ms: i64,
) -> bool {
    device_id == self_device_id
        || presence
            .get(device_id)
            .is_some_and(|at| now_ms - at < PRESENCE_FRESH_MS)
}

fn sort_key(chat: &Chat) -> DateTime<Utc> {
    chat.last_message_at.unwrap_or(chat.created_at)
}

fn sort_recency(rows: &mut [&Chat]) {
    rows.sort_by(|a, b| sort_key(b).cmp(&sort_key(a)).then_with(|| a.id.cmp(&b.id)));
}

/// desktop `change_request_for_chat`: the latest resolution for the chat's
/// own (device, repo root, branch, checkout).
pub(crate) fn change_request_for_chat<'a>(
    chat: &Chat,
    statuses: &'a [CheckoutChangeRequestStatus],
) -> Option<&'a ChangeRequestSummary> {
    let source = chat.source_context.as_ref()?;
    let branch = source.branch.trim();
    if branch.is_empty() {
        return None;
    }
    statuses
        .iter()
        .find(|status| {
            status.device_id == chat.device_id
                && status.cwd == source.repo_root
                && status.branch == branch
                && !status.checkout_id.is_empty()
                && status.checkout_id == source.checkout_id
        })
        .and_then(|status| status.change_request.as_ref())
}

fn hash_row(row: &SessionRow) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    row.id.hash(&mut h);
    row.title.hash(&mut h);
    row.has_title.hash(&mut h);
    row.preview.hash(&mut h);
    row.project.hash(&mut h);
    row.device_id.hash(&mut h);
    row.device_name.hash(&mut h);
    row.device_online.hash(&mut h);
    row.harness.hash(&mut h);
    row.model.hash(&mut h);
    row.reasoning.hash(&mut h);
    row.branch.hash(&mut h);
    row.cwd.hash(&mut h);
    attention_rank(row.indicator).hash(&mut h);
    attention_rank(row.host_indicator).hash(&mut h);
    attention_rank(row.last_outcome).hash(&mut h);
    row.working_since_ms.hash(&mut h);
    row.last_activity_ms.hash(&mut h);
    row.time_label.hash(&mut h);
    row.created_at_ms.hash(&mut h);
    row.unseen.hash(&mut h);
    row.archived.hash(&mut h);
    row.pinned.hash(&mut h);
    row.section_id.hash(&mut h);
    if let Some(pr) = &row.pull_request {
        pr.number.hash(&mut h);
        pr.url.hash(&mut h);
        pr.title.hash(&mut h);
        (pr.state as u8).hash(&mut h);
    }
    row.send_state.hash(&mut h);
    row.parent_chat_id.hash(&mut h);
    row.room_gen.hash(&mut h);
    h.finish()
}

struct RowContext<'a> {
    all_spaces: &'a [Space],
    spaces: HashMap<&'a str, &'a Space>,
    devices: HashMap<&'a str, &'a Device>,
    sessions: HashMap<&'a str, &'a Session>,
    pinned: HashSet<&'a str>,
    section_of: HashMap<&'a str, &'a str>,
}

/// How long after the phone last heard from a direct engine its session
/// rows keep ageing. The link probes a silent engine well inside this.
pub(crate) const STATUS_HOLD_MS: i64 = 20_000;

/// The time a session row's freshness (the 45 s staleness gate) is judged
/// at. Normally now. For an engine reached over a live direct link it is
/// capped at `heard + STATUS_HOLD_MS`, where `heard` is how far the feed has
/// caught up: when it delivers nothing (the app was frozen in the
/// background, the tunnel stalled, or the heartbeat is queued behind
/// megabytes of transcript on a slow relay) the rows' heartbeats can't
/// arrive either, and that delay must not read as "the run ended" (the
/// home list showed a running session as done). While
/// the link is delivering, a row whose own heartbeat stops still goes stale
/// on time; when the link is down the entry is gone and real time applies.
pub(crate) fn status_now(
    session: Option<&Session>,
    now: DateTime<Utc>,
    heard: &HashMap<String, i64>,
) -> DateTime<Utc> {
    session
        .and_then(|s| heard.get(&s.device_id))
        .and_then(|at| DateTime::from_timestamp_millis(at.saturating_add(STATUS_HOLD_MS)))
        .map_or(now, |held| held.min(now))
}

/// [`SessionRow::last_outcome`]: the run outcome without the seen gate.
fn last_outcome(chat: &Chat, session: Option<&Session>, host: ChatIndicator) -> ChatIndicator {
    match host {
        ChatIndicator::Working | ChatIndicator::AwaitingInput => host,
        // The host's row still says running but its heartbeat went stale:
        // nobody saw this run end, so claim no outcome (no green check).
        _ if session.is_some_and(|s| {
            matches!(
                s.status,
                SessionStatus::Working | SessionStatus::AwaitingInput
            )
        }) =>
        {
            ChatIndicator::Idle
        }
        _ if session.is_some_and(|s| s.status == SessionStatus::Errored) => ChatIndicator::Errored,
        _ if chat.last_message_at.is_some() => ChatIndicator::Completed,
        _ => ChatIndicator::Idle,
    }
}

fn build_row(chat: &Chat, rc: &RowContext<'_>, cx: &DeriveContext<'_>) -> Arc<SessionRow> {
    let now_ms = cx.now.timestamp_millis();
    let session = rc.sessions.get(chat.id.as_str()).copied();
    let send_state = cx.send_states.get(&chat.id).copied();
    let host_indicator = display_status(chat, session, status_now(session, cx.now, cx.heard));
    let last_outcome = last_outcome(chat, session, host_indicator);
    let mut indicator = host_indicator;
    let mut working_since_ms = match indicator {
        ChatIndicator::Working | ChatIndicator::AwaitingInput => session
            .and_then(|s| s.started_at)
            .map(|t| t.timestamp_millis()),
        _ => None,
    };
    // A send in flight reads as Working (desktop `display_status_for`).
    if matches!(send_state, Some(SendState::Sending | SendState::Queued))
        && indicator != ChatIndicator::Working
        && indicator != ChatIndicator::AwaitingInput
    {
        indicator = ChatIndicator::Working;
        working_since_ms = None;
    }
    let project = chat
        .space_id
        .as_deref()
        .and_then(|id| rc.spaces.get(id))
        .map(|space| ProjectRef {
            id: space.id.clone(),
            name: space.display_name().to_owned(),
            // Clones of one repository share their representative's color.
            color_index: project_color_index(&representative_space(rc.all_spaces, space).path),
        });
    let device = rc.devices.get(chat.device_id.as_str());
    let config = chat.config.as_ref();
    let harness = config.and_then(|c| {
        serde_json::to_value(c.harness)
            .ok()?
            .as_str()
            .map(str::to_owned)
    });
    let model = config.and_then(|c| c.model.clone());
    let reasoning = config
        .and_then(|c| c.reasoning)
        .and_then(|r| serde_json::to_value(r).ok()?.as_str().map(str::to_owned));
    let title = chat
        .title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty());
    let mut row = SessionRow {
        id: chat.id.clone(),
        revision: 0,
        title: title.unwrap_or("New session").to_owned(),
        has_title: title.is_some(),
        preview: chat
            .last_message_preview
            .as_deref()
            .map(zeron_proto::view::single_line)
            .filter(|p| !p.is_empty()),
        project,
        device_id: chat.device_id.clone(),
        device_name: device.and_then(|d| device_display_name(d)),
        device_online: device_online(&chat.device_id, cx.presence, cx.self_device_id, now_ms),
        harness_label: harness.as_deref().map(catalog::harness_label),
        model_label: match (harness.as_deref(), model.as_deref()) {
            (Some(h), Some(m)) => Some(catalog::model_label(h, m)),
            (None, Some(m)) => Some(m.to_owned()),
            _ => None,
        },
        harness,
        model,
        reasoning,
        branch: chat
            .source_context
            .as_ref()
            .map(|s| s.branch.clone())
            .or_else(|| chat.branch.clone())
            .filter(|b| !b.trim().is_empty()),
        cwd: chat.cwd.clone(),
        indicator,
        host_indicator,
        last_outcome,
        working_since_ms,
        last_activity_ms: sort_key(chat).timestamp_millis(),
        time_label: relative_time_label(sort_key(chat).timestamp_millis(), now_ms),
        created_at_ms: chat.created_at.timestamp_millis(),
        unseen: chat.unseen(),
        archived: chat.archived,
        pinned: rc.pinned.contains(chat.id.as_str()),
        section_id: rc.section_of.get(chat.id.as_str()).map(|s| (*s).to_owned()),
        pull_request: change_request_for_chat(chat, cx.change_requests).cloned(),
        send_state,
        parent_chat_id: chat.parent_chat_id.clone(),
        room_gen: chat.room_gen.unwrap_or(1),
    };
    row.revision = hash_row(&row);
    // Unchanged rows keep their Arc across snapshots (pointer-equal diffing).
    if let Some(previous) = cx.previous.and_then(|p| p.sessions.get(&chat.id))
        && previous.revision == row.revision
        && **previous == row
    {
        return previous.clone();
    }
    Arc::new(row)
}

/// Build the whole snapshot. Pure given its inputs.
pub(crate) fn derive(
    state: &WorkspaceState,
    prefs: Option<&SidebarPreferences>,
    cx: &DeriveContext<'_>,
) -> WorkspaceSnapshot {
    let now_ms = cx.now.timestamp_millis();
    let pinned_order: &[String] = prefs.map_or(&[], |p| p.pinned_session_ids.as_slice());
    let sections: &[zeron_proto::SidebarSection] = prefs.map_or(&[], |p| p.sections.as_slice());
    let mut section_of: HashMap<&str, &str> = HashMap::new();
    for section in sections {
        for id in &section.session_ids {
            section_of.entry(id.as_str()).or_insert(section.id.as_str());
        }
    }
    let rc = RowContext {
        all_spaces: &state.spaces,
        spaces: state.spaces.iter().map(|s| (s.id.as_str(), s)).collect(),
        devices: state.devices.iter().map(|d| (d.id.as_str(), d)).collect(),
        sessions: state
            .sessions
            .iter()
            .map(|s| (s.chat_id.as_str(), s))
            .collect(),
        pinned: pinned_order.iter().map(String::as_str).collect(),
        section_of,
    };

    let rows: HashMap<String, Arc<SessionRow>> = state
        .chats
        .iter()
        .map(|chat| (chat.id.clone(), build_row(chat, &rc, cx)))
        .collect();

    // Active = not archived, top-level, project alive (or projectless).
    let mut active: Vec<&Chat> = state
        .chats
        .iter()
        .filter(|c| !c.archived && c.is_top_level())
        .filter(|c| {
            c.space_id
                .as_deref()
                .is_none_or(|id| rc.spaces.contains_key(id))
        })
        .collect();
    sort_recency(&mut active);
    let row = |chat: &Chat| rows[&chat.id].clone();

    // Front page.
    let active_ids: HashSet<&str> = active.iter().map(|c| c.id.as_str()).collect();
    let mut seen: HashSet<&str> = HashSet::new();
    let pinned: Vec<Arc<SessionRow>> = pinned_order
        .iter()
        .filter(|id| active_ids.contains(id.as_str()) && seen.insert(id.as_str()))
        .map(|id| rows[id].clone())
        .collect();
    let section_views: Vec<SectionView> = sections
        .iter()
        .map(|section| SectionView {
            id: section.id.clone(),
            name: section.name.clone(),
            collapsed: section.collapsed,
            sessions: active
                .iter()
                .filter(|c| !rc.pinned.contains(c.id.as_str()))
                .filter(|c| rc.section_of.get(c.id.as_str()) == Some(&section.id.as_str()))
                .map(|c| row(c))
                .collect(),
        })
        .collect();
    let recent: Vec<Arc<SessionRow>> = active
        .iter()
        .filter(|c| {
            !rc.pinned.contains(c.id.as_str()) && !rc.section_of.contains_key(c.id.as_str())
        })
        .map(|c| row(c))
        .collect();

    // Projects (creation order), sessions by recency.
    let mut spaces: Vec<&Space> = state.spaces.iter().collect();
    spaces.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    let projects: Vec<ProjectView> = spaces
        .iter()
        .map(|space| {
            let sessions: Vec<Arc<SessionRow>> = active
                .iter()
                .filter(|c| c.space_id.as_deref() == Some(space.id.as_str()))
                .map(|c| row(c))
                .collect();
            let indicator = sessions
                .iter()
                .map(|r| r.indicator)
                .min_by_key(|i| attention_rank(*i))
                .unwrap_or(ChatIndicator::Idle);
            let representative = representative_space(&state.spaces, space);
            ProjectView {
                id: space.id.clone(),
                name: space.display_name().to_owned(),
                path: space.path.clone(),
                color_index: project_color_index(&representative.path),
                device_id: space.device_id.clone(),
                device_name: rc
                    .devices
                    .get(space.device_id.as_str())
                    .and_then(|d| device_display_name(d)),
                device_online: device_online(
                    &space.device_id,
                    cx.presence,
                    cx.self_device_id,
                    now_ms,
                ),
                git_detected: space.git_detected,
                created_at_ms: space.created_at.timestamp_millis(),
                group_key: project_key(space),
                group_name: representative.display_name().to_owned(),
                indicator,
                unseen_count: sessions.iter().filter(|r| r.unseen).count() as u32,
                sessions,
            }
        })
        .collect();
    let projectless: Vec<Arc<SessionRow>> = active
        .iter()
        .filter(|c| c.space_id.is_none())
        .map(|c| row(c))
        .collect();

    let mut pull_requests = PullRequestGroups::default();
    for chat in &active {
        let row = row(chat);
        match row.pull_request.as_ref().map(|pr| pr.state) {
            Some(ChangeRequestState::Open) => pull_requests.open.push(row),
            Some(ChangeRequestState::Merged) => pull_requests.merged.push(row),
            Some(ChangeRequestState::Closed) => pull_requests.closed.push(row),
            None => {}
        }
    }

    let mut archived_chats: Vec<&Chat> = state
        .chats
        .iter()
        .filter(|c| c.archived && c.is_top_level())
        .collect();
    sort_recency(&mut archived_chats);
    let archived: Vec<Arc<SessionRow>> = archived_chats.iter().map(|c| row(c)).collect();

    let mut children: HashMap<String, Vec<Arc<SessionRow>>> = HashMap::new();
    let mut child_chats: Vec<&Chat> = state
        .chats
        .iter()
        .filter(|c| c.parent_chat_id.is_some())
        .collect();
    sort_recency(&mut child_chats);
    for chat in child_chats {
        if let Some(parent) = &chat.parent_chat_id {
            children.entry(parent.clone()).or_default().push(row(chat));
        }
    }

    let mut devices: Vec<DeviceView> = state
        .devices
        .iter()
        .map(|device| DeviceView {
            id: device.id.clone(),
            name: device_display_name(device).unwrap_or_else(|| "Unknown device".to_owned()),
            platform: device.platform.clone(),
            online: device_online(&device.id, cx.presence, cx.self_device_id, now_ms),
            last_seen_ms: cx
                .presence
                .get(&device.id)
                .copied()
                .or_else(|| device.last_seen_at.map(|t| t.timestamp_millis())),
            version: device.version.clone(),
            capabilities: device.capabilities.clone(),
            is_execution_host: is_execution_host(device),
            is_self: device.id == cx.self_device_id,
            session_count: active.iter().filter(|c| c.device_id == device.id).count() as u32,
        })
        .collect();
    devices.sort_by(|a, b| {
        b.is_execution_host
            .cmp(&a.is_execution_host)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.id.cmp(&b.id))
    });

    let mut snapshot = WorkspaceSnapshot {
        revision: 0,
        computed_at_ms: now_ms,
        pins_ready: prefs.is_some(),
        synced: cx.synced,
        front: FrontPage {
            pinned,
            sections: section_views,
            recent,
        },
        projects,
        projectless,
        pull_requests,
        archived,
        devices,
        sessions: rows,
        children,
        content_hash: 0,
    };
    snapshot.content_hash = snapshot_hash(&snapshot);
    snapshot
}

fn snapshot_hash(s: &WorkspaceSnapshot) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let ids = |h: &mut std::collections::hash_map::DefaultHasher, rows: &[Arc<SessionRow>]| {
        rows.len().hash(h);
        for row in rows {
            row.revision.hash(h);
        }
    };
    s.pins_ready.hash(&mut h);
    s.synced.hash(&mut h);
    ids(&mut h, &s.front.pinned);
    for section in &s.front.sections {
        section.id.hash(&mut h);
        section.name.hash(&mut h);
        section.collapsed.hash(&mut h);
        ids(&mut h, &section.sessions);
    }
    ids(&mut h, &s.front.recent);
    for project in &s.projects {
        project.id.hash(&mut h);
        project.name.hash(&mut h);
        project.path.hash(&mut h);
        project.color_index.hash(&mut h);
        project.group_key.hash(&mut h);
        project.group_name.hash(&mut h);
        project.device_name.hash(&mut h);
        project.device_online.hash(&mut h);
        project.git_detected.hash(&mut h);
        ids(&mut h, &project.sessions);
    }
    ids(&mut h, &s.projectless);
    ids(&mut h, &s.archived);
    for device in &s.devices {
        device.hash(&mut h);
    }
    let mut all: Vec<(&String, u64)> = s.sessions.iter().map(|(k, v)| (k, v.revision)).collect();
    all.sort();
    all.hash(&mut h);
    h.finish()
}

impl Hash for DeviceView {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.id.hash(h);
        self.name.hash(h);
        self.platform.hash(h);
        self.online.hash(h);
        self.version.hash(h);
        self.capabilities.hash(h);
        self.is_execution_host.hash(h);
        self.session_count.hash(h);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_labels() {
        let now = 10 * 86_400_000;
        assert_eq!(relative_time_label(now - 59_000, now), "now");
        assert_eq!(relative_time_label(now + 5_000, now), "now");
        assert_eq!(relative_time_label(now - 34 * 60_000, now), "34m");
        assert_eq!(relative_time_label(now - 4 * 3_600_000 - 1, now), "4h");
        assert_eq!(relative_time_label(now - 2 * 86_400_000, now), "2d");
    }

    fn chat(last_message: bool, seen: bool) -> Chat {
        let mut v = serde_json::json!({
            "id": "c", "deviceId": "d", "archived": false,
            "createdAt": "2026-09-30T00:00:00Z",
        });
        if last_message {
            v["lastMessageAt"] = "2026-09-30T01:00:00Z".into();
        }
        if seen {
            v["lastSeenAt"] = "2026-09-30T02:00:00Z".into();
        }
        serde_json::from_value(v).unwrap()
    }

    fn session(status: SessionStatus) -> Session {
        serde_json::from_value(serde_json::json!({
            "chatId": "c", "deviceId": "d", "status": status,
            "updatedAt": "2026-09-30T01:00:00Z",
        }))
        .unwrap()
    }

    #[test]
    fn last_outcome_survives_the_seen_marker() {
        let now: DateTime<Utc> = "2026-09-30T03:00:00Z".parse().unwrap();
        // Seen on some device: the tab-dot indicator reads Idle, the outcome
        // still says how the last run ended.
        let seen = chat(true, true);
        let errored = session(SessionStatus::Errored);
        assert_eq!(
            display_status(&seen, Some(&errored), now),
            ChatIndicator::Idle
        );
        assert_eq!(
            last_outcome(&seen, Some(&errored), ChatIndicator::Idle),
            ChatIndicator::Errored
        );
        let idle = session(SessionStatus::Idle);
        assert_eq!(display_status(&seen, Some(&idle), now), ChatIndicator::Idle);
        assert_eq!(
            last_outcome(&seen, Some(&idle), ChatIndicator::Idle),
            ChatIndicator::Completed
        );
        assert_eq!(
            last_outcome(&seen, None, ChatIndicator::Idle),
            ChatIndicator::Completed
        );
        // Unseen agrees with the indicator.
        let unseen = chat(true, false);
        assert_eq!(
            last_outcome(&unseen, Some(&errored), ChatIndicator::Errored),
            ChatIndicator::Errored
        );
        // Live states pass through; a chat that never ran has no outcome.
        assert_eq!(
            last_outcome(&seen, Some(&errored), ChatIndicator::Working),
            ChatIndicator::Working
        );
        assert_eq!(
            last_outcome(&seen, None, ChatIndicator::AwaitingInput),
            ChatIndicator::AwaitingInput
        );
        assert_eq!(
            last_outcome(&chat(false, false), None, ChatIndicator::Idle),
            ChatIndicator::Idle
        );
    }

    /// The home row of a session that is still running, as the phone sees
    /// it: the user has read it (seen), the engine's last heartbeat for it
    /// reached the phone 50 s ago and nothing at all has arrived since (app
    /// frozen in the background, tunnel stalled). round5-11 turned that
    /// into a green "done" check and dropped it from the running count.
    fn home_row(
        heard: &HashMap<String, i64>,
        now: DateTime<Utc>,
        beat_age_s: i64,
    ) -> Arc<SessionRow> {
        let mut chat = chat(true, true);
        chat.last_seen_at = Some(now - chrono::TimeDelta::seconds(5));
        chat.last_message_at = Some(now - chrono::TimeDelta::seconds(60));
        let mut running = session(SessionStatus::Working);
        running.started_at = Some(now - chrono::TimeDelta::minutes(10));
        running.updated_at = now - chrono::TimeDelta::seconds(beat_age_s);
        let state = WorkspaceState {
            devices: vec![],
            spaces: vec![],
            chats: vec![chat],
            sessions: vec![running],
        };
        let presence = HashMap::new();
        let send_states = HashMap::new();
        let snapshot = derive(
            &state,
            None,
            &DeriveContext {
                self_device_id: "phone",
                now,
                presence: &presence,
                heard,
                change_requests: &[],
                send_states: &send_states,
                synced: true,
                previous: None,
            },
        );
        snapshot.session("c").unwrap().clone()
    }

    #[test]
    fn a_silent_link_does_not_turn_a_running_session_into_done() {
        let now: DateTime<Utc> = "2026-10-01T10:15:00Z".parse().unwrap();
        let ms = |s: i64| (now - chrono::TimeDelta::seconds(s)).timestamp_millis();
        // Direct link up, last heard from the engine with that heartbeat.
        let heard = HashMap::from([("d".to_owned(), ms(50))]);
        let row = home_row(&heard, now, 50);
        assert_eq!(
            row.indicator,
            ChatIndicator::Working,
            "still running on the home list"
        );
        assert_eq!(row.last_outcome, ChatIndicator::Working);
        // The link keeps delivering (heard 1 s ago) but this run's own
        // heartbeat stopped 50 s ago: a hung run still goes stale, and as
        // nobody saw it end it shows no outcome, not a green check.
        let heard = HashMap::from([("d".to_owned(), ms(1))]);
        let row = home_row(&heard, now, 50);
        assert_eq!(row.indicator, ChatIndicator::Idle);
        assert_eq!(
            row.last_outcome,
            ChatIndicator::Idle,
            "no done check for an unseen end"
        );
        // Link down (no entry): real time, same as before.
        let row = home_row(&HashMap::new(), now, 50);
        assert_eq!(row.indicator, ChatIndicator::Idle);
        assert_eq!(row.last_outcome, ChatIndicator::Idle);
        // A fresh heartbeat is Working either way.
        assert_eq!(
            home_row(&HashMap::new(), now, 10).indicator,
            ChatIndicator::Working
        );
    }

    #[test]
    fn status_now_holds_only_for_the_heard_engine() {
        let now: DateTime<Utc> = "2026-10-01T10:15:00Z".parse().unwrap();
        let heard = HashMap::from([(
            "d".to_owned(),
            (now - chrono::TimeDelta::seconds(60)).timestamp_millis(),
        )]);
        let mine = session(SessionStatus::Working);
        let mut other = session(SessionStatus::Working);
        other.device_id = "elsewhere".into();
        assert_eq!(
            status_now(Some(&mine), now, &heard),
            now - chrono::TimeDelta::seconds(40)
        );
        assert_eq!(status_now(Some(&other), now, &heard), now);
        assert_eq!(status_now(None, now, &heard), now);
        let recent = HashMap::from([("d".to_owned(), now.timestamp_millis())]);
        assert_eq!(
            status_now(Some(&mine), now, &recent),
            now,
            "never in the future"
        );
    }
}
