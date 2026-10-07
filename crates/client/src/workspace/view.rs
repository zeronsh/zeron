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
    Device, Session, SidebarPreferences, Space,
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

/// A list's execution-project scope. Identity is always the stable project
/// id; names and device labels are presentation only.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum SessionScope {
    #[default]
    All,
    Project {
        project_id: String,
    },
    Projectless,
}

impl SessionScope {
    pub fn contains(&self, row: &SessionRow) -> bool {
        match self {
            Self::All => true,
            Self::Project { project_id } => {
                row.project.as_ref().is_some_and(|p| p.id == *project_id)
            }
            Self::Projectless => row.project.is_none(),
        }
    }
}

/// How the remaining (non-pinned, non-section) sessions are organized.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum SessionOrganization {
    ByProject,
    ByDevice,
    #[default]
    InOneList,
}

/// Time key for the active lists. Pinned always keeps its manual order.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum SessionSort {
    #[default]
    LastUpdated,
    Created,
}

/// Requested list grouping and ordering. Separate from [`SessionScope`] so a
/// view preference never rewrites the selected project scope.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SessionViewOptions {
    pub organization: SessionOrganization,
    pub sort: SessionSort,
}

/// One project/device/home bucket of the grouped Recent list. `id` is stable
/// (`project:<id>`, `home:<device>`, `device:<device>`); `name` and the
/// optional metadata are display-only.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionGroup {
    pub id: String,
    pub name: String,
    pub device_name: Option<String>,
    pub path: Option<String>,
    pub sessions: Vec<Arc<SessionRow>>,
}

/// Delete-confirmation facts for one project. The registry delete cascades
/// over every chat pointing at the project, so archived and child sessions
/// are counted too.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectDeletionSummary {
    pub project_id: String,
    pub project_name: String,
    pub session_count: u64,
    pub archived_count: u64,
}

/// Pure projection of the sidebar and archive. Vectors retain their shared
/// pin/section/recency order, so counts and live indicators use the same rows.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionList {
    pub scope: SessionScope,
    pub front: FrontPage,
    pub archived: Vec<Arc<SessionRow>>,
    /// Grouped Recent buckets (empty for `InOneList`); `front.recent` always
    /// keeps the same complete, sorted set.
    pub groups: Vec<SessionGroup>,
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
    /// Keep a restored id while the initial sync is pending (including an
    /// offline cold start). Only an authoritative snapshot can remove it.
    pub fn resolve_scope(&self, scope: &SessionScope) -> SessionScope {
        match scope {
            SessionScope::Project { project_id }
                if self.synced && self.project(project_id).is_none() =>
            {
                SessionScope::All
            }
            _ => scope.clone(),
        }
    }

    pub fn session_list(&self, scope: &SessionScope) -> SessionList {
        self.session_list_with_options(scope, SessionViewOptions::default())
    }

    /// Scope first, then the requested time sort over sections, Recent and
    /// the flat archive; pinned keeps its shared manual order. Only Recent is
    /// grouped, and `front.recent` stays the same complete sorted set.
    pub fn session_list_with_options(
        &self,
        scope: &SessionScope,
        options: SessionViewOptions,
    ) -> SessionList {
        let scope = self.resolve_scope(scope);
        let filter = |rows: &[Arc<SessionRow>]| {
            rows.iter()
                .filter(|r| scope.contains(r))
                .cloned()
                .collect::<Vec<_>>()
        };
        let sections: Vec<SectionView> = self
            .front
            .sections
            .iter()
            .filter_map(|section| {
                let mut sessions = filter(&section.sessions);
                sort_session_rows(&mut sessions, options.sort);
                (!sessions.is_empty()).then(|| SectionView {
                    id: section.id.clone(),
                    name: section.name.clone(),
                    collapsed: section.collapsed,
                    sessions,
                })
            })
            .collect();
        let mut recent = filter(&self.front.recent);
        sort_session_rows(&mut recent, options.sort);
        let mut archived = filter(&self.archived);
        sort_session_rows(&mut archived, options.sort);
        let groups = match options.organization {
            SessionOrganization::InOneList => Vec::new(),
            organization => self.group_recent(&recent, organization),
        };
        SessionList {
            front: FrontPage {
                pinned: filter(&self.front.pinned),
                sections,
                recent,
            },
            archived,
            scope,
            groups,
        }
    }

    /// Project-picker candidates, including empty/offline projects. All
    /// terms must occur in name, execution device or path, case-insensitively.
    pub fn search_projects(&self, query: &str) -> Vec<ProjectView> {
        let terms: Vec<_> = query.split_whitespace().map(str::to_lowercase).collect();
        self.projects
            .iter()
            .filter(|p| {
                let fields = [
                    p.name.to_lowercase(),
                    p.device_name.as_deref().unwrap_or("").to_lowercase(),
                    p.path.to_lowercase(),
                ];
                terms
                    .iter()
                    .all(|term| fields.iter().any(|field| field.contains(term)))
            })
            .cloned()
            .collect()
    }

    /// Buckets an already scope-filtered, sorted Recent list. Group order is
    /// the first appearance of a session's bucket; the local device group
    /// leads `ByDevice` when it has sessions. Names come from the project or
    /// device view, never from a raw id.
    fn group_recent(
        &self,
        recent: &[Arc<SessionRow>],
        organization: SessionOrganization,
    ) -> Vec<SessionGroup> {
        let mut groups: Vec<SessionGroup> = Vec::new();
        for row in recent {
            let (id, name, device_name, path) = match organization {
                SessionOrganization::ByProject => match row.project.as_ref() {
                    Some(project) => {
                        let view = self.project(&project.id);
                        (
                            format!("project:{}", project.id),
                            view.map_or_else(|| project.name.clone(), |p| p.name.clone()),
                            view.and_then(|p| p.device_name.clone())
                                .or_else(|| row.device_name.clone()),
                            view.map(|p| p.path.clone()),
                        )
                    }
                    None => (
                        format!("home:{}", row.device_id),
                        "~".to_owned(),
                        row.device_name.clone(),
                        row.cwd.clone(),
                    ),
                },
                SessionOrganization::ByDevice => {
                    let name = self
                        .device(&row.device_id)
                        .map(|d| d.name.clone())
                        .or_else(|| row.device_name.clone())
                        .unwrap_or_else(|| "Unknown device".to_owned());
                    (
                        format!("device:{}", row.device_id),
                        name.clone(),
                        Some(name),
                        None,
                    )
                }
                SessionOrganization::InOneList => continue,
            };
            match groups.iter_mut().find(|group| group.id == id) {
                Some(group) => group.sessions.push(row.clone()),
                None => groups.push(SessionGroup {
                    id,
                    name,
                    device_name,
                    path,
                    sessions: vec![row.clone()],
                }),
            }
        }
        if organization == SessionOrganization::ByDevice {
            if let Some(self_id) = self
                .devices
                .iter()
                .find(|d| d.is_self)
                .map(|d| d.id.clone())
            {
                let self_group = format!("device:{self_id}");
                if let Some(at) = groups.iter().position(|group| group.id == self_group) {
                    let local = groups.remove(at);
                    groups.insert(0, local);
                }
            }
        }
        groups
    }

    /// Counts every chat pointing at the project (archived and child rows
    /// included), mirroring `registry::delete_space`'s cascade. `None` when
    /// the project is unknown.
    pub fn project_deletion_summary(&self, project_id: &str) -> Option<ProjectDeletionSummary> {
        let project = self.project(project_id)?;
        let mut session_count = 0u64;
        let mut archived_count = 0u64;
        for row in self.sessions.values() {
            if row.project.as_ref().is_some_and(|p| p.id == project_id) {
                session_count += 1;
                if row.archived {
                    archived_count += 1;
                }
            }
        }
        Some(ProjectDeletionSummary {
            project_id: project.id.clone(),
            project_name: project.name.clone(),
            session_count,
            archived_count,
        })
    }

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
        self.search_scoped(query, &SessionScope::All, true, limit)
    }

    /// Apply scope and archive visibility BEFORE matching/ranking/limit.
    /// Search the same valid top-level rows the lists display.
    pub fn search_scoped(
        &self,
        query: &str,
        scope: &SessionScope,
        include_archived: bool,
        limit: usize,
    ) -> Vec<SearchHit> {
        let scope = self.resolve_scope(scope);
        let terms: Vec<String> = query.split_whitespace().map(|t| t.to_lowercase()).collect();
        if terms.is_empty() {
            return Vec::new();
        }
        let mut hits: Vec<SearchHit> = self
            .front
            .pinned
            .iter()
            .chain(self.front.sections.iter().flat_map(|s| &s.sessions))
            .chain(&self.front.recent)
            .chain(self.archived.iter().filter(|_| include_archived))
            .filter(|row| scope.contains(row))
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

fn view_sort_ms(row: &SessionRow, sort: SessionSort) -> i64 {
    match sort {
        SessionSort::LastUpdated => row.last_activity_ms,
        SessionSort::Created => row.created_at_ms,
    }
}

/// Desktop tie-break: newer first, then stable chat id ascending.
fn sort_session_rows(rows: &mut [Arc<SessionRow>], sort: SessionSort) {
    rows.sort_by(|a, b| {
        view_sort_ms(b, sort)
            .cmp(&view_sort_ms(a, sort))
            .then_with(|| a.id.cmp(&b.id))
    });
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

fn build_row(chat: &Chat, rc: &RowContext<'_>, cx: &DeriveContext<'_>) -> Arc<SessionRow> {
    let now_ms = cx.now.timestamp_millis();
    let session = rc.sessions.get(chat.id.as_str()).copied();
    let send_state = cx.send_states.get(&chat.id).copied();
    let host_indicator = display_status(chat, session, cx.now);
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
        .filter(|c| !c.archived && c.parent_chat_id.is_none())
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
        .filter(|c| c.archived && c.parent_chat_id.is_none())
        .filter(|c| {
            c.space_id
                .as_deref()
                .is_none_or(|id| rc.spaces.contains_key(id))
        })
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

    fn fixture() -> WorkspaceSnapshot {
        let mut doc = zeron_doc::RegistryDoc::new("test-phone");
        crate::demo::fixtures::seed(
            &mut doc,
            crate::DemoFixture::ProjectFilter,
            "test-phone",
            "Phone",
        )
        .unwrap();
        derive(
            &doc.read_all().unwrap(),
            doc.sidebar_preferences().as_ref(),
            &DeriveContext {
                self_device_id: "test-phone",
                now: Utc::now(),
                presence: &HashMap::new(),
                change_requests: &[],
                send_states: &HashMap::new(),
                synced: true,
                previous: None,
            },
        )
    }

    fn project(id: &str) -> SessionScope {
        SessionScope::Project {
            project_id: id.into(),
        }
    }

    #[test]
    fn project_scope_uses_ids_and_preserves_sidebar_membership() {
        let ws = fixture();
        assert_eq!(
            ws.project("space-zeron").unwrap().name,
            ws.project("space-duplicate").unwrap().name
        );
        assert_ne!(
            ws.project("space-zeron").unwrap().group_key,
            ws.project("space-duplicate").unwrap().group_key
        );
        let list = ws.session_list(&project("space-zeron"));
        assert_eq!(
            list.front
                .pinned
                .iter()
                .map(|r| r.id.as_str())
                .collect::<Vec<_>>(),
            ["chat-veil", "chat-picker"]
        );
        assert_eq!(list.front.sections.len(), 1);
        assert_eq!(list.front.sections[0].id, "section-p0");
        assert_eq!(list.front.sections[0].sessions.len(), 1);
        assert_eq!(list.front.sections[0].sessions[0].id, "chat-tabs");
        assert_eq!(
            list.front
                .recent
                .iter()
                .map(|r| r.id.as_str())
                .collect::<Vec<_>>(),
            ["chat-scoped-needle"]
        );
        assert_eq!(
            ws.session_list(&project("space-zeron-vps")).front.recent[0].id,
            "chat-cjk"
        );
        assert_eq!(
            list.archived
                .iter()
                .map(|r| r.id.as_str())
                .collect::<Vec<_>>(),
            ["chat-oklch"]
        );
        let duplicate = ws.session_list(&project("space-duplicate"));
        assert!(
            duplicate.front.pinned.is_empty()
                && duplicate.front.sections.is_empty()
                && duplicate.archived.is_empty()
        );
        assert_eq!(duplicate.front.recent[0].id, "chat-duplicate");
        let home = ws.session_list(&SessionScope::Projectless);
        assert_eq!(home.front.recent[0].id, "chat-home");
        assert_eq!(home.archived[0].id, "chat-home-archived");
        assert!(home.front.pinned.is_empty() && home.front.sections.is_empty());
    }

    #[test]
    fn scope_and_archive_filter_before_search_limit() {
        let mut ws = fixture();
        assert!(
            !ws.search("needle", 60)
                .iter()
                .any(|h| h.session.id == "chat-scoped-needle")
        );
        let hits = ws.search_scoped("needle", &project("space-zeron"), false, 60);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].session.id, "chat-scoped-needle");
        assert_eq!(
            ws.search_scoped("needle", &project("space-edge"), false, 60)
                .len(),
            60
        );
        // Archived title matches outrank an active preview match. Excluding
        // archives must happen before truncation, even inside one project.
        let mut active = (**ws.session("chat-scoped-needle").unwrap()).clone();
        active.title = "Older active session".into();
        active.preview = Some("needle".into());
        ws.front.recent = vec![Arc::new(active.clone())];
        ws.front.pinned.clear();
        ws.front.sections.clear();
        ws.archived = (0..70)
            .map(|i| {
                let mut archived = active.clone();
                archived.id = format!("archived-{i}");
                archived.archived = true;
                archived.title = "Needle".into();
                Arc::new(archived)
            })
            .collect();
        assert!(
            ws.search_scoped("needle", &project("space-zeron"), true, 60)
                .iter()
                .all(|h| h.session.archived)
        );
        let active_hits = ws.search_scoped("needle", &project("space-zeron"), false, 60);
        assert_eq!(active_hits.len(), 1);
        assert_eq!(active_hits[0].session.id, "chat-scoped-needle");
        assert!(
            ws.search_scoped(" \n", &SessionScope::All, true, 60)
                .is_empty()
        );
    }

    #[test]
    fn restored_scope_waits_for_first_sync_and_empty_projects_are_searchable() {
        let pending = WorkspaceSnapshot::default();
        let scope = project("space-zeron");
        assert_eq!(pending.session_list(&scope).scope, scope);
        let ready = WorkspaceSnapshot {
            synced: true,
            ..Default::default()
        };
        assert_eq!(ready.session_list(&scope).scope, SessionScope::All);
        let ws = fixture();
        let empty = ws.session_list(&project("space-empty"));
        assert_eq!(empty.scope, project("space-empty"));
        assert!(
            empty.front.recent.is_empty()
                && empty.front.pinned.is_empty()
                && empty.front.sections.is_empty()
                && empty.archived.is_empty()
        );
        assert_eq!(
            ws.search_projects("STUDIO archive/zeron")[0].id,
            "space-duplicate"
        );
        assert_eq!(
            ws.search_projects("accessible layouts")[0].id,
            "space-empty"
        );
        assert_eq!(ws.search_projects("zeron").len(), 4);
        assert!(ws.search_projects("not-a-project").is_empty());
    }

    fn options(organization: SessionOrganization, sort: SessionSort) -> SessionViewOptions {
        SessionViewOptions { organization, sort }
    }

    fn recent_ids(list: &SessionList) -> Vec<&str> {
        list.front.recent.iter().map(|r| r.id.as_str()).collect()
    }

    fn archived_ids(list: &SessionList) -> Vec<&str> {
        list.archived.iter().map(|r| r.id.as_str()).collect()
    }

    #[test]
    fn default_list_is_flat_and_preserves_phase_one_order() {
        let ws = fixture();
        let list = ws.session_list(&project("space-zeron"));
        assert!(list.groups.is_empty());
        assert_eq!(recent_ids(&list), ["chat-scoped-needle"]);
        assert_eq!(archived_ids(&list), ["chat-oklch"]);
    }

    #[test]
    fn created_sort_reorders_recent_and_archive() {
        let mut ws = fixture();
        let base = (**ws.front.recent.first().unwrap()).clone();
        let mut fresh_update = base.clone();
        fresh_update.id = "row-fresh-update".into();
        fresh_update.last_activity_ms = 1_000;
        fresh_update.created_at_ms = 10;
        let mut fresh_create = base;
        fresh_create.id = "row-fresh-create".into();
        fresh_create.last_activity_ms = 10;
        fresh_create.created_at_ms = 1_000;
        ws.front.recent = vec![
            Arc::new(fresh_update.clone()),
            Arc::new(fresh_create.clone()),
        ];
        ws.archived = vec![Arc::new(fresh_update), Arc::new(fresh_create)];

        let updated = ws.session_list_with_options(
            &SessionScope::All,
            options(SessionOrganization::InOneList, SessionSort::LastUpdated),
        );
        assert_eq!(
            recent_ids(&updated),
            ["row-fresh-update", "row-fresh-create"]
        );
        assert_eq!(
            archived_ids(&updated),
            ["row-fresh-update", "row-fresh-create"]
        );

        let created = ws.session_list_with_options(
            &SessionScope::All,
            options(SessionOrganization::InOneList, SessionSort::Created),
        );
        assert_eq!(
            recent_ids(&created),
            ["row-fresh-create", "row-fresh-update"]
        );
        assert_eq!(
            archived_ids(&created),
            ["row-fresh-create", "row-fresh-update"]
        );
    }

    #[test]
    fn by_project_groups_recent_and_projectless_by_home_device() {
        let ws = fixture();
        let scoped = ws.session_list_with_options(
            &project("space-zeron"),
            options(SessionOrganization::ByProject, SessionSort::LastUpdated),
        );
        assert_eq!(scoped.groups.len(), 1);
        assert_eq!(scoped.groups[0].id, "project:space-zeron");
        assert_eq!(scoped.groups[0].name, "zeron");
        assert_eq!(
            scoped.groups[0].path.as_deref(),
            Some(ws.project("space-zeron").unwrap().path.as_str())
        );
        let flat: Vec<&str> = scoped
            .groups
            .iter()
            .flat_map(|g| g.sessions.iter().map(|r| r.id.as_str()))
            .collect();
        assert_eq!(flat, recent_ids(&scoped));

        let home = ws.session_list_with_options(
            &SessionScope::Projectless,
            options(SessionOrganization::ByProject, SessionSort::LastUpdated),
        );
        assert_eq!(home.groups.len(), 1);
        assert_eq!(home.groups[0].id, "home:dev-mac");
        assert_eq!(home.groups[0].name, "~");
        assert_eq!(home.groups[0].device_name.as_deref(), Some("MacBook Pro"));
        assert_eq!(home.groups[0].sessions[0].id, "chat-home");
    }

    #[test]
    fn by_device_groups_and_promotes_the_local_device() {
        let mut ws = fixture();
        let mut local = (**ws.front.recent.first().unwrap()).clone();
        local.id = "row-local".into();
        local.device_id = "test-phone".into();
        local.device_name = Some("Phone".into());
        local.last_activity_ms = -1;
        ws.front.recent.push(Arc::new(local));

        let list = ws.session_list_with_options(
            &SessionScope::All,
            options(SessionOrganization::ByDevice, SessionSort::LastUpdated),
        );
        assert_eq!(list.groups[0].id, "device:test-phone");
        assert_eq!(list.groups[0].name, "Phone");
        assert!(
            list.groups[0]
                .sessions
                .iter()
                .all(|r| r.device_id == "test-phone")
        );
        for group in &list.groups {
            let device_id = group.id.strip_prefix("device:").unwrap();
            assert_eq!(group.name, ws.device(device_id).unwrap().name);
        }
        let total: usize = list.groups.iter().map(|g| g.sessions.len()).sum();
        assert_eq!(total, list.front.recent.len());
    }

    #[test]
    fn deletion_summary_counts_archived_and_child_sessions() {
        let ws = fixture();
        let summary = ws.project_deletion_summary("space-zeron").unwrap();
        assert_eq!(summary.project_name, "zeron");
        // Top-level active (veil, picker, tabs, scoped-needle) + child
        // (side) + archived (oklch).
        assert_eq!(summary.session_count, 6);
        assert_eq!(summary.archived_count, 1);
        let edge = ws.project_deletion_summary("space-edge").unwrap();
        assert_eq!(edge.session_count, 68);
        assert_eq!(edge.archived_count, 1);
        assert!(ws.project_deletion_summary("missing").is_none());
    }

    #[test]
    fn time_labels() {
        let now = 10 * 86_400_000;
        assert_eq!(relative_time_label(now - 59_000, now), "now");
        assert_eq!(relative_time_label(now + 5_000, now), "now");
        assert_eq!(relative_time_label(now - 34 * 60_000, now), "34m");
        assert_eq!(relative_time_label(now - 4 * 3_600_000 - 1, now), "4h");
        assert_eq!(relative_time_label(now - 2 * 86_400_000, now), "2d");
    }
}
