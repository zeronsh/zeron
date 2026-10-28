//! Frontend-agnostic view logic: the derivations every viewport needs and none
//! of them should own — sort orders, staleness gating, sidebar grouping, the
//! boot gate, relative times.
//!
//! This lives in `proto` rather than in the viewport crate so the rules stay
//! pure and independently testable: the same workspace doc must produce the
//! same row order on every surface, and there is exactly one implementation
//! and one test suite per rule.
//!
//! Everything in this module is pure. `chat_indicator` (the status derivation
//! these gate on) is in [`crate::entities`].

use chrono::{DateTime, Utc};

use crate::{AuthState, Chat, ChatIndicator, Session, SessionStatus, Space, WorkspaceScope};

// ---------------------------------------------------------------------------
// Connection + status
// ---------------------------------------------------------------------------

/// Viewport ⇄ engine connection lifecycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionStatus {
    Connecting,
    Ready,
    Failed(String),
}

/// What a chat's status dot / working indicator should show right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Indicator {
    None,
    Working,
    AwaitingInput,
    Errored,
}

/// A `Working`/`AwaitingInput` session older than this is treated as dead — a
/// crashed backend must never show an eternal "Working" (feature-inventory
/// §1.12). Engines heartbeat sessions well inside this window.
pub const SESSION_STALE_MS: i64 = 45_000;

/// Only live activity expires. Completed/error records remain useful after
/// process exit; a lost heartbeat is never evidence of successful completion.
pub fn session_stale(session: &Session, now: DateTime<Utc>) -> bool {
    matches!(
        session.activity_status(),
        SessionStatus::Working | SessionStatus::AwaitingInput
    ) && now
        .signed_duration_since(session.updated_at)
        .num_milliseconds()
        > SESSION_STALE_MS
}

/// Staleness-checked indicator for a session row. Pure.
pub fn effective_indicator(session: Option<&Session>, now: DateTime<Utc>) -> Indicator {
    indicator_for_status(session, now, true)
}

/// Native execution controls must not offer Stop/steer for a terminal report.
pub fn native_indicator(session: Option<&Session>, now: DateTime<Utc>) -> Indicator {
    indicator_for_status(session, now, false)
}

fn indicator_for_status(
    session: Option<&Session>,
    now: DateTime<Utc>,
    terminal: bool,
) -> Indicator {
    let Some(session) = session else {
        return Indicator::None;
    };
    let status = if terminal {
        session.activity_status()
    } else {
        session.status
    };
    match status {
        SessionStatus::Idle => Indicator::None,
        SessionStatus::Errored => Indicator::Errored,
        SessionStatus::Working | SessionStatus::AwaitingInput => {
            if now
                .signed_duration_since(session.updated_at)
                .num_milliseconds()
                > SESSION_STALE_MS
            {
                Indicator::None
            } else if status == SessionStatus::Working {
                Indicator::Working
            } else {
                Indicator::AwaitingInput
            }
        }
    }
}

/// Subagents running under a chat, staleness-checked like
/// [`effective_indicator`]: a session row nobody has refreshed inside
/// [`SESSION_STALE_MS`] belongs to a dead engine and counts nothing. Pure.
pub fn running_subagents(session: Option<&Session>, now: DateTime<Utc>) -> u32 {
    session
        .filter(|s| now.signed_duration_since(s.updated_at).num_milliseconds() <= SESSION_STALE_MS)
        .map_or(0, |s| s.running_subagents)
}

/// Background processes of a chat, staleness-checked like
/// [`running_subagents`]: engines heartbeat a live runtime's row, so a row
/// past [`SESSION_STALE_MS`] belongs to an engine that took its processes
/// with it. Pure.
pub fn background_tasks(session: Option<&Session>, now: DateTime<Utc>) -> &[crate::BackgroundTask] {
    session
        .filter(|s| now.signed_duration_since(s.updated_at).num_milliseconds() <= SESSION_STALE_MS)
        .map_or(&[], |s| s.background_tasks.as_slice())
}

/// The full display status for a chat row / tab dot: live states win, then the
/// synced seen marker decides completed-vs-idle. Staleness gating rides on
/// [`effective_indicator`]; the derivation itself is [`crate::chat_indicator`].
pub fn display_status(chat: &Chat, session: Option<&Session>, now: DateTime<Utc>) -> ChatIndicator {
    if session.is_some_and(|s| session_stale(s, now)) {
        return ChatIndicator::Idle;
    }
    crate::chat_indicator(chat, session)
}

/// Same seen/outcome rules, restricted to native work for composer controls.
pub fn native_display_status(
    chat: &Chat,
    session: Option<&Session>,
    now: DateTime<Utc>,
) -> ChatIndicator {
    if session.is_some_and(|s| {
        matches!(
            s.status,
            SessionStatus::Working | SessionStatus::AwaitingInput
        ) && native_indicator(Some(s), now) == Indicator::None
    }) {
        return ChatIndicator::Idle;
    }
    crate::entities::chat_indicator_inner(chat, session, false)
}

/// Includes terminal-only results, which do not append an assistant message.
pub fn has_unseen_activity(chat: &Chat, session: Option<&Session>) -> bool {
    chat.unseen()
        || session
            .and_then(|s| s.terminal_activity.as_ref())
            .is_some_and(|activity| {
                matches!(
                    activity.report.state,
                    crate::ProgramState::Done | crate::ProgramState::Error
                ) && chat
                    .last_seen_at
                    .is_none_or(|seen| activity.updated_at > seen)
            })
}

/// Attention bucket for the sidebar's Active list — lower is more urgent.
pub fn attention_rank(status: ChatIndicator) -> u8 {
    match status {
        ChatIndicator::AwaitingInput => 0,
        ChatIndicator::Errored => 1,
        ChatIndicator::Working => 2,
        ChatIndicator::Completed => 3,
        ChatIndicator::Idle => 4,
    }
}

// ---------------------------------------------------------------------------
// Sort orders
// ---------------------------------------------------------------------------

/// Active-list order: pure recency (`last_message_at` desc, `created_at`
/// fallback), id tiebreak so the sort is total. Deliberately NOT
/// attention-bucketed: status drives the DOT, never the position — bucketing
/// meant that merely OPENING a completed session (completed → seen → idle)
/// dropped its row under the pointer (user report: "their position in the
/// scrollbar changes"). Matches the old sidebar, which rendered chats in
/// recency order and let the dots carry urgency; [`attention_rank`] still
/// aggregates the space rows' urgency dot.
pub fn sort_active(rows: &mut Vec<(ChatIndicator, &Chat)>) {
    rows.sort_by(|(_, a), (_, b)| {
        let ka = a.last_message_at.unwrap_or(a.created_at);
        let kb = b.last_message_at.unwrap_or(b.created_at);
        kb.cmp(&ka).then_with(|| a.id.cmp(&b.id))
    });
}

/// Session-tab order for a space: creation order (activity never reorders
/// tabs), id tiebreak. Pure.
pub fn sort_tabs(chats: &mut [&Chat]) {
    chats.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.id.cmp(&b.id))
    });
}

/// Spaces list order: creation order, id tiebreak — total and stable across
/// devices. Pure.
pub fn sort_spaces(spaces: &mut [Space]) {
    spaces.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.id.cmp(&b.id))
    });
}

/// The project a space belongs to: spaces sharing a repository identity —
/// clones and worktrees of one repository, on any device — are one project.
/// A space without an identity is a project of its own. Pure.
pub fn project_key(space: &Space) -> String {
    match space.repository_id.as_deref() {
        Some(repository_id) => format!("repo:{repository_id}"),
        None => space.id.clone(),
    }
}

/// The repository identity for a Cloud project of `github_repo`. A Cloud
/// project lives on the logical Cloud device, where no engine runs, so
/// whoever creates it stamps it: with the identity a clone of the same
/// repository already has (a full clone's `commit:<root>`), so the two read
/// as one project, else the normalized remote form. Pure.
pub fn cloud_repository_id(spaces: &[Space], github_repo: &str) -> String {
    spaces
        .iter()
        .filter(|s| !s.device_id.starts_with(crate::CLOUD_DEVICE_PREFIX))
        .filter(|s| {
            s.github_repo
                .as_deref()
                .is_some_and(|repo| repo.eq_ignore_ascii_case(github_repo))
        })
        .filter_map(|s| s.repository_id.as_deref().map(|id| (s, id)))
        // The repository itself, not a folder inside it (`<identity>:<path>`).
        .filter(|(_, id)| !id.strip_prefix("commit:").unwrap_or(id).contains(':'))
        .min_by_key(|(s, id)| (!id.starts_with("commit:"), s.created_at, s.id.as_str()))
        .map(|(_, id)| id.to_string())
        .unwrap_or_else(|| format!("github.com/{}", github_repo.to_lowercase()))
}

/// The space that speaks for `space`'s whole project: the oldest member, id
/// tiebreak. Its name and color stand for every member, and the choice is
/// the same on every device, so a repository looks alike everywhere. Pure.
pub fn representative_space<'a>(spaces: &'a [Space], space: &'a Space) -> &'a Space {
    let Some(repository_id) = space.repository_id.as_deref() else {
        return space;
    };
    spaces
        .iter()
        .filter(|s| s.repository_id.as_deref() == Some(repository_id))
        .min_by_key(|s| (s.created_at, s.id.as_str()))
        .unwrap_or(space)
}

/// Sidebar order: `last_message_at` desc, falling back to `created_at`; ties
/// break by `created_at` desc then id so the sort is total and stable across
/// devices. Pure.
pub fn sort_chats(chats: &mut [Chat]) {
    chats.sort_by(|a, b| {
        let ka = a.last_message_at.unwrap_or(a.created_at);
        let kb = b.last_message_at.unwrap_or(b.created_at);
        kb.cmp(&ka)
            .then_with(|| b.created_at.cmp(&a.created_at))
            .then_with(|| a.id.cmp(&b.id))
    });
}

// ---------------------------------------------------------------------------
// Boot gate
// ---------------------------------------------------------------------------

/// The app gate (zeron's App.tsx phases). Pure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GatePhase {
    /// Booting / probing — splash covers this.
    Loading,
    /// Engine unreachable and embedding failed.
    Failed(String),
    /// Engine up, but signed out — show the sign-in card.
    SignIn,
    /// Signed in but no organization selected — "Create your workspace".
    OrgGate,
    /// Render the shell.
    Ready,
}

/// Missing scope is treated as synced. Current engines always publish
/// [`WorkspaceScope`] before becoming ready, while old daemons are deliberately
/// kept behind the account gate instead of being mistaken for local runtimes.
pub fn gate_phase(
    connection: &ConnectionStatus,
    workspace_scope: Option<WorkspaceScope>,
    auth: Option<&AuthState>,
) -> GatePhase {
    match connection {
        ConnectionStatus::Connecting => GatePhase::Loading,
        ConnectionStatus::Failed(err) => GatePhase::Failed(err.clone()),
        ConnectionStatus::Ready => match workspace_scope.unwrap_or(WorkspaceScope::Synced) {
            WorkspaceScope::Local | WorkspaceScope::Development => GatePhase::Ready,
            WorkspaceScope::Synced => match auth {
                Some(AuthState::NeedsOrganization { .. }) => GatePhase::OrgGate,
                Some(AuthState::SignedIn { .. }) => GatePhase::Ready,
                Some(AuthState::SignedOut) | None => GatePhase::SignIn,
            },
        },
    }
}

#[cfg(test)]
mod project_tests {
    use super::*;

    fn space(id: &str, device: &str, repository: Option<&str>, minutes: i64) -> Space {
        Space {
            id: id.into(),
            device_id: device.into(),
            path: format!("/{device}/{id}"),
            name: None,
            git_detected: repository.is_some(),
            git_checked_at: None,
            checkout_id: None,
            repository_id: repository.map(str::to_string),
            github_repo: None,
            created_at: DateTime::<Utc>::UNIX_EPOCH + chrono::TimeDelta::minutes(minutes),
        }
    }

    #[test]
    fn a_cloud_project_takes_the_identity_of_a_clone_of_its_repository() {
        let github = |mut s: Space, repo: &str| {
            s.github_repo = Some(repo.into());
            s
        };
        // No clone yet: the remote form.
        assert_eq!(cloud_repository_id(&[], "Acme/App"), "github.com/acme/app");
        let spaces = vec![
            github(
                space("shallow", "vps", Some("github.com/acme/app"), 0),
                "acme/app",
            ),
            github(space("full", "mac", Some("commit:abc"), 1), "Acme/App"),
            github(space("sub", "mac", Some("commit:abc:web"), 0), "acme/app"),
            github(
                space("cloud", "cloud-1", Some("github.com/acme/app"), 0),
                "acme/app",
            ),
            github(space("other", "mac", Some("commit:def"), 0), "acme/other"),
        ];
        // A full clone's commit identity wins over a shallow clone's remote
        // form; a folder inside the repository and Cloud's own rows don't count.
        assert_eq!(cloud_repository_id(&spaces, "acme/app"), "commit:abc");
        assert_eq!(
            cloud_repository_id(&spaces[..1], "acme/app"),
            "github.com/acme/app"
        );
    }

    #[test]
    fn spaces_sharing_a_repository_are_one_project_led_by_the_oldest() {
        let spaces = vec![
            space("laptop", "mac", Some("github.com/o/r"), 2),
            space("server", "vps", Some("github.com/o/r"), 1),
            space("notes", "mac", None, 0),
        ];
        assert_eq!(project_key(&spaces[0]), "repo:github.com/o/r");
        assert_eq!(project_key(&spaces[0]), project_key(&spaces[1]));
        assert_eq!(project_key(&spaces[2]), "notes");
        assert_eq!(representative_space(&spaces, &spaces[0]).id, "server");
        assert_eq!(representative_space(&spaces, &spaces[1]).id, "server");
        assert_eq!(representative_space(&spaces, &spaces[2]).id, "notes");
    }
}

#[cfg(test)]
mod gate_tests {
    use super::*;
    use crate::UserProfile;

    fn user() -> UserProfile {
        UserProfile {
            id: "user-1".into(),
            email: "user@example.com".into(),
            name: None,
        }
    }

    fn running_session(running: u32, updated_at: DateTime<Utc>) -> Session {
        Session {
            last_completed_turn: None,
            chat_id: "chat".into(),
            device_id: "dev".into(),
            status: SessionStatus::Working,
            started_at: None,
            updated_at,
            last_turn_outcome: None,
            terminal_activity: None,
            running_subagents: running,
            background_tasks: Vec::new(),
        }
    }

    #[test]
    fn running_subagents_count_only_while_the_session_row_is_fresh() {
        let now = Utc::now();
        let fresh = running_session(3, now - chrono::Duration::seconds(10));
        assert_eq!(running_subagents(Some(&fresh), now), 3);
        // A crashed engine's row must not badge a chat forever.
        let stale = running_session(
            3,
            now - chrono::Duration::milliseconds(SESSION_STALE_MS + 1),
        );
        assert_eq!(running_subagents(Some(&stale), now), 0);
        assert_eq!(running_subagents(None, now), 0);
    }

    #[test]
    fn native_and_terminal_activity_share_attention_and_seen_rules() {
        use crate::{DoneStatus, ProgramInputKind, ProgramState, ProgramStatus, TerminalActivity};
        let now = Utc::now();
        let mut chat: Chat = serde_json::from_value(serde_json::json!({
            "id": "chat", "deviceId": "dev", "archived": false,
            "createdAt": now, "lastMessageAt": now,
        }))
        .unwrap();
        let mut session = running_session(0, now);
        assert_eq!(
            display_status(&chat, Some(&session), now),
            ChatIndicator::Working
        );
        session.terminal_activity = Some(TerminalActivity {
            terminal_id: "pty".into(),
            updated_at: now,
            report: ProgramStatus {
                id: "child".into(),
                state: ProgramState::Blocked,
                kind: Some(ProgramInputKind::Question),
                app: None,
                title: None,
                message: Some("Which branch?".into()),
                progress: None,
            },
        });
        assert_eq!(
            display_status(&chat, Some(&session), now),
            ChatIndicator::AwaitingInput
        );
        assert_eq!(
            effective_indicator(Some(&session), now),
            Indicator::AwaitingInput
        );
        let later = now + chrono::Duration::milliseconds(SESSION_STALE_MS + 1);
        assert_eq!(
            display_status(&chat, Some(&session), later),
            ChatIndicator::Idle
        );
        assert!(session_stale(&session, later));
        session.terminal_activity = None;
        session.status = SessionStatus::Idle;
        session.last_turn_outcome = Some(crate::NativeTurnOutcome {
            status: DoneStatus::Interrupted,
            finished_at: now,
        });
        assert_eq!(
            display_status(&chat, Some(&session), now),
            ChatIndicator::Idle
        );
        session.last_turn_outcome = Some(crate::NativeTurnOutcome {
            status: DoneStatus::Completed,
            finished_at: now,
        });
        assert_eq!(
            display_status(&chat, Some(&session), now),
            ChatIndicator::Completed
        );
        chat.last_seen_at = Some(now);
        assert_eq!(
            display_status(&chat, Some(&session), now),
            ChatIndicator::Idle
        );
        session.terminal_activity = Some(TerminalActivity {
            terminal_id: "pty".into(),
            updated_at: later,
            report: ProgramStatus {
                id: "".into(),
                state: ProgramState::Done,
                kind: None,
                app: None,
                title: None,
                message: None,
                progress: None,
            },
        });
        assert!(has_unseen_activity(&chat, Some(&session)));
        assert_eq!(
            display_status(&chat, Some(&session), later),
            ChatIndicator::Completed
        );
        session.terminal_activity.as_mut().unwrap().report.state = ProgramState::Error;
        assert_eq!(
            display_status(&chat, Some(&session), later),
            ChatIndicator::Errored
        );
        chat.last_seen_at = Some(later);
        assert!(!has_unseen_activity(&chat, Some(&session)));
        assert_eq!(
            display_status(&chat, Some(&session), later),
            ChatIndicator::Idle
        );
        session.terminal_activity.as_mut().unwrap().report.state = ProgramState::Blocked;
        session.updated_at = later;
        assert_eq!(
            display_status(&chat, Some(&session), later),
            ChatIndicator::AwaitingInput
        );
        assert_eq!(native_indicator(Some(&session), later), Indicator::None);
        assert_eq!(
            native_display_status(&chat, Some(&session), later),
            ChatIndicator::Idle
        );
    }

    #[test]
    fn workspace_scope_controls_the_auth_gate() {
        assert_eq!(
            gate_phase(
                &ConnectionStatus::Ready,
                Some(WorkspaceScope::Local),
                Some(&AuthState::SignedOut),
            ),
            GatePhase::Ready
        );
        assert_eq!(
            gate_phase(
                &ConnectionStatus::Ready,
                Some(WorkspaceScope::Synced),
                Some(&AuthState::SignedOut),
            ),
            GatePhase::SignIn
        );
        assert_eq!(
            gate_phase(
                &ConnectionStatus::Ready,
                Some(WorkspaceScope::Synced),
                Some(&AuthState::NeedsOrganization { user: user() }),
            ),
            GatePhase::OrgGate
        );
    }

    #[test]
    fn development_and_local_never_use_the_workos_gate() {
        for scope in [WorkspaceScope::Local, WorkspaceScope::Development] {
            for auth in [
                AuthState::SignedOut,
                AuthState::NeedsOrganization { user: user() },
                AuthState::SignedIn {
                    user: user(),
                    org_id: Some("org-1".into()),
                },
            ] {
                assert_eq!(
                    gate_phase(&ConnectionStatus::Ready, Some(scope), Some(&auth)),
                    GatePhase::Ready
                );
            }
        }
    }

    #[test]
    fn missing_scope_falls_back_to_a_synced_gate() {
        assert_eq!(
            gate_phase(&ConnectionStatus::Ready, None, None),
            GatePhase::SignIn
        );
    }
}

/// Parse an `AuthStatus` frame tolerantly. The engine currently serializes its
/// own enum (`{"_tag": "SignedIn", ...}`) while the proto type expects
/// `{"state": "signedIn", ...}` — accept both so either side can converge
/// without breaking a viewport.
pub fn parse_auth_state(value: &serde_json::Value) -> Option<AuthState> {
    if let Ok(state) = serde_json::from_value::<AuthState>(value.clone()) {
        return Some(state);
    }
    let tag = value.get("_tag").and_then(|t| t.as_str())?;
    let user = || -> Option<crate::UserProfile> {
        let u = value.get("user")?;
        Some(crate::UserProfile {
            id: u.get("id")?.as_str()?.to_string(),
            email: u.get("email")?.as_str()?.to_string(),
            name: u.get("name").and_then(|n| n.as_str()).map(str::to_string),
        })
    };
    match tag {
        "SignedOut" => Some(AuthState::SignedOut),
        "NeedsOrganization" => Some(AuthState::NeedsOrganization { user: user()? }),
        "SignedIn" => Some(AuthState::SignedIn {
            user: user()?,
            org_id: value
                .get("orgId")
                .and_then(|v| v.as_str())
                .map(str::to_string),
        }),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Sidebar grouping
// ---------------------------------------------------------------------------

/// One grouped-by-project sidebar section.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatGroup<'a> {
    pub label: String,
    pub chats: Vec<&'a Chat>,
}

/// Project label for a chat: the basename of its cwd, or "No project".
pub fn project_label(cwd: Option<&str>) -> String {
    let Some(cwd) = cwd.map(str::trim).filter(|c| !c.is_empty()) else {
        return "No project".to_string();
    };
    if crate::projectless::is_projectless_cwd(Some(cwd)) {
        return "No project".to_string();
    }
    std::path::Path::new(cwd.trim_end_matches(['/', '\\']))
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| cwd.to_string())
}

/// Group chats by project label, preserving the incoming (recency) order both
/// for groups (by their most recent chat) and rows within a group. Pure.
pub fn group_chats<'a>(chats: impl IntoIterator<Item = &'a Chat>) -> Vec<ChatGroup<'a>> {
    let mut groups: Vec<ChatGroup<'a>> = Vec::new();
    for chat in chats {
        let label = project_label(chat.cwd.as_deref());
        match groups.iter_mut().find(|g| g.label == label) {
            Some(group) => group.chats.push(chat),
            None => groups.push(ChatGroup {
                label,
                chats: vec![chat],
            }),
        }
    }
    groups
}

/// Compact relative time ("now", "5m", "3h", "2d", "1w", …) — no "ago" suffix;
/// port of zeron's `formatTimeAgo`.
pub fn format_time_ago(then: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let s = now.signed_duration_since(then).num_seconds().max(0);
    // Under a minute reads as "now" — otherwise 45–59s floors to a bare "0m".
    if s < 60 {
        return "now".to_string();
    }
    let m = s / 60;
    if m < 60 {
        return format!("{m}m");
    }
    let h = m / 60;
    if h < 24 {
        return format!("{h}h");
    }
    let d = h / 24;
    if d < 7 {
        return format!("{d}d");
    }
    let w = d / 7;
    if w < 5 {
        return format!("{w}w");
    }
    let mo = d / 30;
    if mo < 12 {
        return format!("{mo}mo");
    }
    format!("{}y", d / 365)
}

/// Session-row sub-line, "project · branch" (zeron `chatLocation`): the repo
/// checkout identity. Either part may be missing; empty when both are. A
/// projectless chat names no project and no folder.
pub fn chat_location(chat: &Chat) -> Option<String> {
    let project = chat
        .cwd
        .as_deref()
        .map(str::trim)
        .filter(|c| !crate::projectless::is_projectless_cwd(Some(c)))
        .map(|c| project_label(Some(c)));
    let reference = chat
        .branch
        .as_deref()
        .map(str::trim)
        .filter(|b| !b.is_empty());
    match (project, reference) {
        (Some(p), Some(r)) => Some(format!("{p} · {r}")),
        (Some(p), None) => Some(p),
        (None, Some(r)) => Some(r.to_string()),
        (None, None) => None,
    }
}

// ---------------------------------------------------------------------------
// Tool summaries (pure)
// ---------------------------------------------------------------------------

/// Collapse model-generated text onto ONE line for single-line surfaces (tool
/// chips, titles, previews): newlines, tabs and runs of whitespace become
/// single spaces, trimmed.
///
/// Both viewports need this for the same reason from opposite directions — gpui
/// breaks on a literal `\n` before its ellipsis logic, and a terminal cell grid
/// would take an embedded newline as a cursor move.
pub fn single_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("{n} {one}")
    } else {
        format!("{n} {many}")
    }
}

/// Per-kind chip label + one-line detail. Labels match zeron's `describeTool`
/// (tool-chip.tsx) exactly, so the two viewports name a tool identically.
pub fn tool_chip_content(call: &crate::ToolCall) -> (&'static str, String) {
    let (label, detail) = tool_chip_content_raw(call);
    (label, single_line(&detail))
}

fn tool_chip_content_raw(call: &crate::ToolCall) -> (&'static str, String) {
    use crate::ToolCall;
    match call {
        ToolCall::Exec { command } => ("Run", command.clone()),
        ToolCall::ReadFile { path } => ("Read", path.clone()),
        ToolCall::WriteFile { path, .. } => ("Write", path.clone()),
        ToolCall::EditFile { path, .. } => ("Edit", path.clone()),
        ToolCall::ApplyPatch { path } => {
            ("Patch", path.clone().unwrap_or_else(|| "workspace".into()))
        }
        ToolCall::Search { pattern, path } => (
            "Search",
            match path {
                Some(path) => format!("{pattern} in {path}"),
                None => pattern.clone(),
            },
        ),
        ToolCall::Glob { pattern } => ("Glob", pattern.clone()),
        ToolCall::WebFetch { url, .. } => ("Fetch", url.clone()),
        ToolCall::WebSearch { query } => ("Web", query.clone()),
        ToolCall::Todo { items } => {
            let done = items.iter().filter(|i| i.done).count();
            ("Todo", format!("{done}/{} done", items.len()))
        }
        ToolCall::Mcp { server, tool, .. } => ("MCP", format!("{server} · {tool}")),
        // Subagent spawns decode as Unknown named "Agent[: <description>]"
        // (every native driver's convention): label them "Agent" with the
        // description as the detail — "Tool · Agent: scan repo" read as two
        // labels fighting.
        // A Cloud machine's own setup ("Cloned acme/app").
        call @ ToolCall::Unknown { name, .. } if crate::cloud_setup_step(call).is_some() => {
            ("Cloud", name.clone())
        }
        ToolCall::Unknown { name, .. } => match name.strip_prefix("Agent: ") {
            Some(description) => ("Agent", description.to_owned()),
            None if name == "Agent" => ("Agent", String::new()),
            None => ("Tool", name.clone()),
        },
    }
}

/// The ToolGroup summary line — "Ran 3 commands · edited 2 files".
///
/// Takes `(call, is_error)` pairs so each viewport can keep its own row model;
/// the summary itself is one implementation for both.
pub fn tool_group_summary(tools: &[(crate::ToolCall, bool)]) -> String {
    use crate::ToolCall;
    // A Cloud machine's setup steps read as what happened to the machine.
    if !tools.is_empty() {
        let steps: Option<Vec<&str>> = tools
            .iter()
            .map(|(call, _)| crate::cloud_setup_step(call))
            .collect();
        if let Some(steps) = steps {
            let mut summary = if steps.contains(&"wake") {
                "Woke the Cloud machine".to_string()
            } else {
                "Set up the Cloud machine".to_string()
            };
            let failed = tools.iter().filter(|(_, is_error)| *is_error).count();
            if failed > 0 {
                summary.push_str(&format!(" · {failed} failed"));
            }
            return summary;
        }
    }
    let mut commands = 0usize;
    let mut edited: Vec<&str> = Vec::new();
    let mut reads = 0usize;
    let mut searches = 0usize;
    let mut fetches = 0usize;
    let mut todos = 0usize;
    let mut other = 0usize;
    let mut failed = 0usize;
    for (call, is_error) in tools {
        if *is_error {
            failed += 1;
        }
        match call {
            ToolCall::Exec { .. } => commands += 1,
            ToolCall::WriteFile { path, .. } | ToolCall::EditFile { path, .. } => {
                if !edited.contains(&path.as_str()) {
                    edited.push(path);
                }
            }
            ToolCall::ApplyPatch { path } => {
                let p = path.as_deref().unwrap_or("patch");
                if !edited.contains(&p) {
                    edited.push(p);
                }
            }
            ToolCall::ReadFile { .. } => reads += 1,
            ToolCall::Search { .. } | ToolCall::Glob { .. } | ToolCall::WebSearch { .. } => {
                searches += 1
            }
            ToolCall::WebFetch { .. } => fetches += 1,
            ToolCall::Todo { .. } => todos += 1,
            ToolCall::Mcp { .. } | ToolCall::Unknown { .. } => other += 1,
        }
    }
    let mut segments: Vec<String> = Vec::new();
    if commands > 0 {
        segments.push(format!("ran {}", plural(commands, "command", "commands")));
    }
    if !edited.is_empty() {
        segments.push(format!("edited {}", plural(edited.len(), "file", "files")));
    }
    if reads > 0 {
        segments.push(format!("read {}", plural(reads, "file", "files")));
    }
    if searches > 0 {
        segments.push(format!("searched {}", plural(searches, "time", "times")));
    }
    if fetches > 0 {
        segments.push(format!("fetched {}", plural(fetches, "page", "pages")));
    }
    if todos > 0 {
        segments.push("updated todos".to_string());
    }
    if other > 0 {
        segments.push(format!("called {}", plural(other, "tool", "tools")));
    }
    if segments.is_empty() {
        segments.push(plural(tools.len(), "tool", "tools"));
    }
    if failed > 0 {
        segments.push(format!("{failed} failed"));
    }
    let mut summary = segments.join(" · ");
    // Capitalize the first segment only (zeron's style).
    if let Some(first) = summary.get(0..1) {
        let upper = first.to_uppercase();
        summary.replace_range(0..1, &upper);
    }
    summary
}

// ---------------------------------------------------------------------------
// Question answers (pure)
// ---------------------------------------------------------------------------

/// Longest answer or label a one-line surface shows, in chars. The doc keeps
/// the whole answer; a chip only summarizes it.
const ANSWER_DISPLAY_MAX: usize = 160;

/// What became of the questions an agent asked — one derivation for every
/// surface that shows them (the desktop chip, the iOS chip, `read_chat`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputOutcome {
    /// Still waiting on the person.
    Pending,
    /// Resolved before answers were recorded on the transcript: only that it
    /// was resolved is known.
    Unrecorded,
    /// Closed without an answer: dismissed, or the run stopped waiting —
    /// `Recherche → Not answered`.
    Closed(AnswerLine),
    /// One line per question, in the order asked.
    Answered(Vec<AnswerLine>),
}

/// One question and what came back, ready for a one-line surface:
/// `Recherche → Deep dive`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnswerLine {
    /// The question's header — or its text, when headers don't tell the
    /// questions apart (an MCP form names every field "<server> asks").
    pub label: String,
    /// What the agent received: picked options joined with ", ", typed text
    /// in quotes, "Skipped" when nothing was given.
    pub answer: String,
    /// Some of the answer is text the person typed rather than an offered
    /// option.
    pub typed: bool,
    /// Nothing was given for this question.
    pub skipped: bool,
}

impl std::fmt::Display for AnswerLine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} → {}", self.label, self.answer)
    }
}

/// [`InputOutcome`] from an input part's fields. `answers` is `None` on parts
/// written before answers were recorded, `Some([])` when the questions closed
/// unanswered.
pub fn input_outcome(
    questions: &[crate::UserInputQuestion],
    resolved: bool,
    answers: Option<&[crate::UserInputAnswer]>,
) -> InputOutcome {
    if !resolved {
        return InputOutcome::Pending;
    }
    let Some(answers) = answers else {
        return InputOutcome::Unrecorded;
    };
    if answers.is_empty() {
        return InputOutcome::Closed(AnswerLine {
            label: input_header(questions),
            answer: "Not answered".to_owned(),
            typed: false,
            skipped: true,
        });
    }
    let lines: Vec<AnswerLine> = questions
        .iter()
        .map(|question| {
            let given: Vec<&String> = answers
                .iter()
                .find(|a| a.question_id == question.id)
                .map(|a| a.labels.iter().filter(|l| !l.trim().is_empty()).collect())
                .unwrap_or_default();
            let typed = given.iter().any(|l| !question.options.contains(l));
            let answer = if given.is_empty() {
                "Skipped".to_owned()
            } else {
                given
                    .iter()
                    .map(|l| {
                        let text = clip(&single_line(l), ANSWER_DISPLAY_MAX);
                        if question.options.contains(l) {
                            text
                        } else {
                            format!("“{text}”")
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            AnswerLine {
                label: answer_label(question, questions),
                answer,
                typed,
                skipped: given.is_empty(),
            }
        })
        .collect();
    if lines.is_empty() {
        return InputOutcome::Unrecorded;
    }
    InputOutcome::Answered(lines)
}

/// What a one-line surface calls a set of questions: the first one's
/// header, "Question" when it has none.
pub fn input_header(questions: &[crate::UserInputQuestion]) -> String {
    questions
        .first()
        .map(|q| single_line(&q.header))
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "Question".to_owned())
}

fn answer_label(question: &crate::UserInputQuestion, all: &[crate::UserInputQuestion]) -> String {
    let header = single_line(&question.header);
    let shared = all
        .iter()
        .filter(|q| single_line(&q.header) == header)
        .count()
        > 1;
    if !header.is_empty() && !shared {
        return header;
    }
    let text = single_line(&question.question);
    if !text.is_empty() {
        return clip(&text, ANSWER_DISPLAY_MAX);
    }
    if header.is_empty() {
        "Question".to_owned()
    } else {
        header
    }
}

fn clip(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((end, _)) => format!("{}…", text[..end].trim_end()),
        None => text.to_owned(),
    }
}

/// The status-dot palette, as oklch triples (L, C, H°).
///
/// Colors live here rather than in the viewport because the *meaning* of a
/// dot is part of the protocol, not the presentation — a given status must
/// read the same on every surface. `zeron-ui` has the oklch→sRGB math.
pub mod dot {
    /// Running. Pink, not amber: the harsh yellow read as a warning, and running
    /// is routine (user request).
    pub const WORKING: (f32, f32, f32) = (0.718, 0.202, 349.761);
    /// Asking a question. Indigo — must read differently from "busy" at a glance.
    pub const AWAITING: (f32, f32, f32) = (0.673, 0.182, 276.935);
    /// Errored. Red-400.
    pub const ERRORED: (f32, f32, f32) = (0.704, 0.191, 22.216);
    /// Finished but unseen. Emerald — reads as "ready for you".
    pub const COMPLETED: (f32, f32, f32) = (0.765, 0.177, 163.223);
}

// ---------------------------------------------------------------------------
// Checkout selection (new sessions)
// ---------------------------------------------------------------------------

/// Where a new session runs (t3code's env-mode: `local | worktree`).
///
/// "Current worktree" is deliberately **not** a third mode — it is `Local` when
/// the picked ref already happens to be materialized as a worktree, in which
/// case the session reuses that checkout's path. Modelling it as three states
/// would let the UI hold a combination the engine cannot honour.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CheckoutKind {
    /// The space's own folder — or the picked ref's existing worktree.
    #[default]
    Local,
    /// A fresh isolated worktree created off the picked base ref on send.
    NewWorktree,
}

/// The resolved on-send checkout action.
#[derive(Debug, Clone, PartialEq)]
pub enum CheckoutPlan {
    /// Run in the space folder as-is. `branch` is the checkout's branch (the
    /// picked or current ref), carried onto `createChat` so the session names
    /// it from the first frame; `None` = refs never loaded.
    CurrentCheckout { branch: Option<String> },
    /// Reuse the picked ref's existing worktree (a cwd override; no git).
    ReuseWorktree { path: String, branch: String },
    /// `CreateWorktree` off `base` on send (the engine mints a `zeron/<name>`
    /// branch). `base: None` = refs never loaded — send falls back to the space
    /// folder rather than failing.
    NewWorktree { base: Option<String> },
}

/// Resolve the on-send action from the mode and the picked ref.
pub fn checkout_plan(kind: CheckoutKind, picked: Option<&crate::RepoRef>) -> CheckoutPlan {
    let name = picked.map(|r| r.name.clone());
    match kind {
        CheckoutKind::NewWorktree => CheckoutPlan::NewWorktree { base: name },
        CheckoutKind::Local => match picked.and_then(|r| r.worktree_path.clone()) {
            Some(path) => CheckoutPlan::ReuseWorktree {
                path,
                branch: name.unwrap_or_default(),
            },
            None => CheckoutPlan::CurrentCheckout { branch: name },
        },
    }
}

/// Label of the checkout-kind trigger (t3code `resolveEnvModeLabel`).
pub fn checkout_label(kind: CheckoutKind, picked: Option<&crate::RepoRef>) -> &'static str {
    match kind {
        CheckoutKind::NewWorktree => "New worktree",
        CheckoutKind::Local => {
            if picked.is_some_and(|r| r.worktree_path.is_some()) {
                "Current worktree"
            } else {
                "Current checkout"
            }
        }
    }
}

#[cfg(test)]
mod checkout_tests {
    use super::*;
    use crate::RepoRef;

    fn plain(name: &str) -> RepoRef {
        RepoRef {
            name: name.into(),
            current: false,
            worktree_path: None,
        }
    }

    fn materialized(name: &str, path: &str) -> RepoRef {
        RepoRef {
            name: name.into(),
            current: false,
            worktree_path: Some(path.into()),
        }
    }

    #[test]
    fn local_resolves_by_whether_the_ref_has_a_worktree() {
        // The same mode means two different things depending on the ref — which
        // is exactly why "current worktree" is not its own state.
        assert_eq!(
            checkout_plan(CheckoutKind::Local, Some(&plain("main"))),
            CheckoutPlan::CurrentCheckout {
                branch: Some("main".into())
            }
        );
        assert_eq!(
            checkout_plan(CheckoutKind::Local, Some(&materialized("feat", "/wt/feat"))),
            CheckoutPlan::ReuseWorktree {
                path: "/wt/feat".into(),
                branch: "feat".into()
            }
        );
        // No ref picked at all is still the space folder — with no branch to
        // stamp until refs load.
        assert_eq!(
            checkout_plan(CheckoutKind::Local, None),
            CheckoutPlan::CurrentCheckout { branch: None }
        );
    }

    #[test]
    fn new_worktree_carries_its_base_and_tolerates_none() {
        assert_eq!(
            checkout_plan(CheckoutKind::NewWorktree, Some(&plain("main"))),
            CheckoutPlan::NewWorktree {
                base: Some("main".into())
            }
        );
        // Refs never loaded: send falls back to the space folder rather than
        // failing, so the base is allowed to be absent.
        assert_eq!(
            checkout_plan(CheckoutKind::NewWorktree, None),
            CheckoutPlan::NewWorktree { base: None }
        );
    }

    #[test]
    fn labels_say_which_of_the_three_outcomes_you_will_get() {
        assert_eq!(
            checkout_label(CheckoutKind::Local, Some(&plain("main"))),
            "Current checkout"
        );
        assert_eq!(
            checkout_label(CheckoutKind::Local, Some(&materialized("f", "/wt/f"))),
            "Current worktree"
        );
        assert_eq!(
            checkout_label(CheckoutKind::NewWorktree, Some(&plain("main"))),
            "New worktree"
        );
    }
}

#[cfg(test)]
mod input_outcome_tests {
    use super::*;
    use crate::{UserInputAnswer, UserInputQuestion};

    fn question(id: &str, header: &str, text: &str, options: &[&str]) -> UserInputQuestion {
        UserInputQuestion {
            id: id.into(),
            header: header.into(),
            question: text.into(),
            options: options.iter().map(|o| (*o).to_owned()).collect(),
            multi_select: false,
            prefill: None,
            multiline: false,
        }
    }

    fn answer(id: &str, labels: &[&str]) -> UserInputAnswer {
        UserInputAnswer {
            question_id: id.into(),
            labels: labels.iter().map(|l| (*l).to_owned()).collect(),
        }
    }

    fn lines(outcome: InputOutcome) -> Vec<String> {
        match outcome {
            InputOutcome::Answered(lines) => lines.iter().map(ToString::to_string).collect(),
            other => panic!("not answered: {other:?}"),
        }
    }

    #[test]
    fn resolution_without_answers_reads_as_what_is_known() {
        let qs = [question("q", "Recherche", "How deep?", &["Deep dive"])];
        assert_eq!(input_outcome(&qs, false, None), InputOutcome::Pending);
        // Answers may already be stamped while the part still reads open.
        assert_eq!(
            input_outcome(&qs, false, Some(&[answer("q", &["Deep dive"])])),
            InputOutcome::Pending
        );
        // Older docs: resolved, but never recorded what was answered — must
        // not claim it went unanswered.
        assert_eq!(input_outcome(&qs, true, None), InputOutcome::Unrecorded);
        let InputOutcome::Closed(closed) = input_outcome(&qs, true, Some(&[])) else {
            panic!("closed");
        };
        assert_eq!(closed.to_string(), "Recherche → Not answered");
        assert!(closed.skipped);
        // No header to name the questions by.
        let bare = [question("q", " ", "How deep?", &[])];
        let InputOutcome::Closed(closed) = input_outcome(&bare, true, Some(&[])) else {
            panic!("closed");
        };
        assert_eq!(closed.to_string(), "Question → Not answered");
    }

    #[test]
    fn answers_pair_with_their_questions_in_the_order_asked() {
        let qs = [
            question("depth", "Recherche", "How deep?", &["Quick", "Deep dive"]),
            question(
                "format",
                "Format",
                "Deliver as?",
                &["Table", "Diagram", "Prose"],
            ),
            question("scope", "Scope", "Anything else?", &["No"]),
            question("notes", "Notes", "Notes for me?", &[]),
        ];
        let given = [
            answer("notes", &["  "]),
            answer("scope", &["Include the\niOS app"]),
            answer("format", &["Table", "Diagram"]),
            answer("depth", &["Deep dive"]),
        ];
        assert_eq!(
            lines(input_outcome(&qs, true, Some(&given))),
            vec![
                "Recherche → Deep dive",
                "Format → Table, Diagram",
                "Scope → “Include the iOS app”",
                "Notes → Skipped",
            ]
        );
        let InputOutcome::Answered(parsed) = input_outcome(&qs, true, Some(&given)) else {
            unreachable!()
        };
        assert_eq!(
            parsed
                .iter()
                .map(|l| (l.typed, l.skipped))
                .collect::<Vec<_>>(),
            vec![(false, false), (false, false), (true, false), (false, true)]
        );
    }

    #[test]
    fn shared_headers_fall_back_to_the_question_text() {
        // An MCP form asks every field under one "<server> asks" header.
        let qs = [
            question("a", "figma asks", "File name", &[]),
            question("b", "figma asks", "Make it public?", &["Yes", "No"]),
        ];
        let given = [answer("a", &["Landing v2"]), answer("b", &["No"])];
        assert_eq!(
            lines(input_outcome(&qs, true, Some(&given))),
            vec!["File name → “Landing v2”", "Make it public? → No"]
        );
    }

    #[test]
    fn long_typed_answers_are_clipped_for_one_line_surfaces() {
        let qs = [question("q", "Notes", "Notes?", &[])];
        let long = "word ".repeat(80);
        let InputOutcome::Answered(parsed) =
            input_outcome(&qs, true, Some(&[answer("q", &[long.as_str()])]))
        else {
            panic!("answered");
        };
        assert!(parsed[0].answer.ends_with("…”"), "{}", parsed[0].answer);
        assert!(parsed[0].answer.chars().count() <= ANSWER_DISPLAY_MAX + 3);
    }
}

#[cfg(test)]
mod cloud_setup_tests {
    use super::*;
    use crate::ToolCall;

    fn step(step: &str, name: &str) -> ToolCall {
        ToolCall::Unknown {
            name: name.into(),
            input: Some(
                serde_json::json!({ crate::CLOUD_SETUP_KEY: step, "command": "git clone x" }),
            ),
        }
    }

    #[test]
    fn cloud_setup_steps_read_as_what_happened_to_the_machine() {
        let start = step("start", "Started a machine");
        let clone = step("clone", "Cloned acme/app");
        assert_eq!(crate::cloud_setup_step(&clone), Some("clone"));
        assert_eq!(
            tool_chip_content(&clone),
            ("Cloud", "Cloned acme/app".to_string())
        );
        assert_eq!(
            tool_group_summary(&[(start.clone(), false), (clone.clone(), false)]),
            "Set up the Cloud machine"
        );
        assert_eq!(
            tool_group_summary(&[(step("wake", "Woke the machine"), false)]),
            "Woke the Cloud machine"
        );
        assert_eq!(
            tool_group_summary(&[(start, false), (clone.clone(), true)]),
            "Set up the Cloud machine · 1 failed"
        );
        // Mixed with ordinary tools: the ordinary summary.
        let exec = ToolCall::Exec {
            command: "ls".into(),
        };
        assert_eq!(
            tool_group_summary(&[(clone, false), (exec, false)]),
            "Ran 1 command · called 1 tool"
        );
        let other = ToolCall::Unknown {
            name: "x".into(),
            input: Some(serde_json::json!({"a": 1})),
        };
        assert_eq!(crate::cloud_setup_step(&other), None);
    }
}
