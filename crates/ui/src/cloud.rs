//! Cloud presentation shared by Settings, the device pickers, the composer,
//! the panes and the New project flow (docs/design/cloud-device.md "Product
//! shape", "Devices and ids", "Session lifecycle").
//!
//! Cloud is a checkout option: a session in a project whose GitHub repository
//! Cloud reaches can run on its own hidden *session device* (capability
//! `cloud-session`) in a sandbox that clones the repository and sleeps when
//! idle. Device lists hide session devices; a chat's reachability comes from
//! its session's state. Everything here is pure so
//! the mapping from wire state to copy is unit-tested in one place.

use std::time::Duration;

use chrono::{DateTime, Utc};
use zeron_proto::{
    CloudSession, CloudSessions, CloudState, CloudStatus, CloudUsage, Device, GithubRepo,
    VaultConnection, VaultConnectionStatus, VaultDevice, VaultProvider, VaultStatus,
};

use crate::icons;
use crate::state::EngineHandle;

/// The label every device list shows for Cloud.
pub const CLOUD_LABEL: &str = "Cloud";

/// While any session is being set up, woken or deleted, sessions are
/// re-read this often; otherwise they are read on focus and page open only.
pub const SESSION_POLL: Duration = Duration::from_secs(5);

/// A Cloud device of either kind (logical or session): the cloud glyph.
pub fn is_cloud(device: &Device) -> bool {
    zeron_proto::is_cloud_device(&device.id, &device.platform)
}

/// The logical Cloud device: owns projects and providers, has no engine.
pub fn is_cloud_account(device: &Device) -> bool {
    device
        .capabilities
        .iter()
        .any(|c| c == zeron_proto::CLOUD_ACCOUNT_CAPABILITY)
}

/// One session's machine. Hidden from device lists.
pub fn is_cloud_session_device(device: &Device) -> bool {
    device
        .capabilities
        .iter()
        .any(|c| c == zeron_proto::CLOUD_SESSION_CAPABILITY)
}

/// Whether a device belongs in device lists and pickers: everything but the
/// per-session machines, which the logical Cloud device stands for.
pub fn is_listed(device: &Device) -> bool {
    !is_cloud_session_device(device)
}

/// The glyph for a device row, chip or crumb.
pub fn device_glyph(device_id: &str, platform: &str) -> &'static str {
    if zeron_proto::is_cloud_device(device_id, platform) {
        return icons::CLOUD;
    }
    match platform {
        "macos" | "darwin" => icons::LAPTOP,
        "web" => icons::GLOBAL,
        "ios" | "android" => icons::SMARTPHONE,
        _ => icons::MONITOR,
    }
}

/// How a device's (or a session machine's) reachability reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence {
    Online,
    /// The logical Cloud device while Cloud is enabled: always reachable in
    /// the sense that matters (sessions start on demand).
    Available,
    /// A stopped session machine: it wakes when the chat is sent to.
    Asleep,
    /// A session machine being set up or woken.
    Waking,
    Offline,
}

impl Presence {
    pub fn label(self) -> &'static str {
        match self {
            Presence::Online => "Online",
            Presence::Available => "Available",
            Presence::Asleep => "Asleep",
            Presence::Waking => "Starting…",
            Presence::Offline => "Offline",
        }
    }

    /// Whether the UI should flag the device as unreachable. Asleep and
    /// starting machines answer on their own, and the logical device never
    /// is a machine, so only a plain offline device warns.
    pub fn warns(self) -> bool {
        self == Presence::Offline
    }
}

/// The logical Cloud device reads from the account, not a heartbeat: on
/// while Cloud is enabled. With no status read yet, the registry row itself
/// says Cloud is on (the edge removes it when Cloud is turned off).
pub fn account_presence(status: Option<&CloudStatus>) -> Presence {
    match status {
        None => Presence::Available,
        Some(status)
            if !status.available
                || matches!(status.state, CloudState::Off | CloudState::Deleting) =>
        {
            Presence::Offline
        }
        Some(_) => Presence::Available,
    }
}

/// A session machine: its lifecycle state when known, else the heartbeat
/// (an unknown stopped machine is presumed asleep, the normal reason).
pub fn session_presence(online: bool, session: Option<&CloudSession>) -> Presence {
    match session.map(|s| s.state) {
        Some(CloudState::Ready) => Presence::Online,
        Some(CloudState::Sleeping | CloudState::Stopping) => Presence::Asleep,
        Some(CloudState::Provisioning | CloudState::Starting) => Presence::Waking,
        Some(CloudState::Off | CloudState::Deleting | CloudState::Error) => Presence::Offline,
        None if online => Presence::Online,
        None => Presence::Asleep,
    }
}

/// The session hosting `chat_id`, matched by chat or (for side chats, which
/// run in their parent's machine) by host device.
pub fn session_for<'a>(
    sessions: &'a CloudSessions,
    chat_id: &str,
    device_id: &str,
) -> Option<&'a CloudSession> {
    sessions
        .sessions
        .iter()
        .find(|s| s.chat_id == chat_id)
        .or_else(|| sessions.sessions.iter().find(|s| s.device_id == device_id))
}

/// The composer's line for a chat on a session machine that isn't running.
pub fn session_notice(state: CloudState) -> Option<&'static str> {
    match state {
        CloudState::Provisioning | CloudState::Starting => {
            Some("Starting a Cloud machine for this session…")
        }
        CloudState::Sleeping | CloudState::Stopping => {
            Some("This session's Cloud machine is asleep — it wakes when you send")
        }
        _ => None,
    }
}

/// Sessions in these states settle on their own and are followed closely.
pub fn session_settling(state: CloudState) -> bool {
    matches!(
        state,
        CloudState::Provisioning | CloudState::Starting | CloudState::Deleting
    )
}

/// Whether any session is settling (the poll's only reason to run).
pub fn any_session_settling(sessions: &CloudSessions) -> bool {
    sessions.sessions.iter().any(|s| session_settling(s.state))
}

/// Account copy for the Settings → Cloud status row.
pub fn account_state_label(state: CloudState) -> &'static str {
    match state {
        CloudState::Off => "Off",
        CloudState::Deleting => "Turning off Cloud…",
        CloudState::Error => "Needs attention",
        _ => "On",
    }
}

/// How long the page waits before re-reading the account: only a delete
/// (every session machine stopped, metered and deleted) settles on its own.
pub fn account_recheck_after(state: CloudState) -> Option<Duration> {
    (state == CloudState::Deleting).then_some(SESSION_POLL)
}

/// Whether Cloud is on: there is a logical device to manage.
pub fn account_enabled(status: &CloudStatus) -> bool {
    status.device_id.is_some() && !matches!(status.state, CloudState::Off | CloudState::Deleting)
}

/// Account lifecycle calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudAction {
    Enable,
    Delete,
}

impl CloudAction {
    pub fn method(self) -> &'static str {
        use zeron_rpc::methods;
        match self {
            CloudAction::Enable => methods::CLOUD_ENABLE,
            CloudAction::Delete => methods::CLOUD_DELETE,
        }
    }

    pub fn busy_label(self) -> &'static str {
        match self {
            CloudAction::Enable => "Turning on…",
            CloudAction::Delete => "Turning off…",
        }
    }
}

/// What Retry does after an account failure: the action that failed.
pub fn retry_action(status: &CloudStatus) -> CloudAction {
    match status.failed_action.as_deref() {
        Some("delete") => CloudAction::Delete,
        _ => CloudAction::Enable,
    }
}

/// "5m ago" for a Unix-ms timestamp.
pub fn ago(ms: i64, now: DateTime<Utc>) -> String {
    crate::settings::devices::format_last_seen(DateTime::from_timestamp_millis(ms), now)
}

/// One provider's standing for Cloud.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderLink {
    NotConnected,
    /// Connected and usable on Cloud.
    Connected {
        account: Option<String>,
    },
    /// The provider refused the stored grant.
    NeedsReconnect {
        account: Option<String>,
    },
    /// Stored for the account but not allowed on Cloud.
    NotAuthorized {
        account: Option<String>,
    },
}

impl ProviderLink {
    pub fn is_stored(&self) -> bool {
        !matches!(self, ProviderLink::NotConnected)
    }
}

pub fn connection(vault: &VaultStatus, provider: VaultProvider) -> Option<&VaultConnection> {
    vault.connections.iter().find(|c| c.provider == provider)
}

/// `cloud_id` is the logical Cloud device: authorizing it covers every
/// session machine.
pub fn provider_link(vault: &VaultStatus, provider: VaultProvider, cloud_id: &str) -> ProviderLink {
    let Some(connection) = connection(vault, provider) else {
        return ProviderLink::NotConnected;
    };
    let account = connection.account.clone();
    if !connection.authorized_devices.iter().any(|d| d == cloud_id) {
        return ProviderLink::NotAuthorized { account };
    }
    match connection.status {
        VaultConnectionStatus::Connected => ProviderLink::Connected { account },
        VaultConnectionStatus::NeedsReconnect => ProviderLink::NeedsReconnect { account },
    }
}

/// The connection's device list with the Cloud device added (for
/// `VaultAuthorize`), keeping everything already allowed.
pub fn authorized_with(
    vault: &VaultStatus,
    provider: VaultProvider,
    cloud_id: &str,
) -> Vec<String> {
    let mut devices: Vec<String> = connection(vault, provider)
        .map(|c| c.authorized_devices.clone())
        .unwrap_or_default();
    if !devices.iter().any(|d| d == cloud_id) {
        devices.push(cloud_id.to_string());
    }
    devices
}

/// "Devices with access", grouped: computers one per row, Cloud's session
/// machines folded into one "Cloud · N session machines" row.
#[derive(Debug, Default, PartialEq)]
pub struct VaultDeviceGroups {
    pub computers: Vec<VaultDevice>,
    /// Enrolled, not revoked Cloud session machines.
    pub cloud_machines: usize,
}

/// Session machines enroll as kind `cloud`; any of them (and the logical
/// device, should it appear) belong to Cloud. Revoked machines are gone
/// (deleted sessions) and not counted.
pub fn group_vault_devices(devices: &[VaultDevice]) -> VaultDeviceGroups {
    let mut groups = VaultDeviceGroups::default();
    for device in devices {
        if device.kind == "cloud" {
            if device.revoked_at.is_none() {
                groups.cloud_machines += 1;
            }
        } else {
            groups.computers.push(device.clone());
        }
    }
    groups
}

/// Newest push first, then by name, so the list is stable between loads.
pub fn sort_repos(repos: &mut [GithubRepo]) {
    repos.sort_by(|a, b| {
        b.pushed_at
            .cmp(&a.pushed_at)
            .then_with(|| a.full_name.to_lowercase().cmp(&b.full_name.to_lowercase()))
    });
}

/// Client-side narrowing while the debounced server query is in flight.
/// Ranks a repository-name prefix first (people type `zeron`, not
/// `acme/zeron`), then an owner/full-name prefix, then any substring of the
/// name or description; keeps the incoming order within a rank.
pub fn filter_repos(repos: &[GithubRepo], query: &str) -> Vec<GithubRepo> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return repos.to_vec();
    }
    let mut ranked: Vec<(usize, usize)> = repos
        .iter()
        .enumerate()
        .filter_map(|(ix, repo)| {
            let full = repo.full_name.to_lowercase();
            let name = full.rsplit('/').next().unwrap_or(&full);
            let rank = if name.starts_with(&query) {
                0
            } else if full.starts_with(&query) {
                1
            } else if full.contains(&query) {
                2
            } else if repo
                .description
                .as_deref()
                .is_some_and(|d| d.to_lowercase().contains(&query))
            {
                3
            } else {
                return None;
            };
            Some((rank, ix))
        })
        .collect();
    ranked.sort();
    ranked
        .into_iter()
        .map(|(_, ix)| repos[ix].clone())
        .collect()
}

/// `ListGithubRepos` without a GitHub grant fails with this prefix.
const GITHUB_NOT_CONNECTED_PREFIX: &str = "github_not_connected:";

/// The readable part of a "GitHub isn't connected" failure (the text after
/// the machine prefix, which is never shown), or `None` for other errors.
pub fn github_not_connected(error: &str) -> Option<String> {
    let (_, message) = error.split_once(GITHUB_NOT_CONNECTED_PREFIX)?;
    let message = message.trim();
    Some(if message.is_empty() {
        "GitHub isn't connected for Cloud.".to_string()
    } else {
        message.to_string()
    })
}

/// "12.4 hours" (one decimal; "1.0 hour").
pub fn format_hours(seconds: u64) -> String {
    let hours = format!("{:.1}", seconds as f64 / 3600.0);
    let unit = if hours == "1.0" { "hour" } else { "hours" };
    format!("{hours} {unit}")
}

/// The machine cost line, left out while it rounds to nothing. It is what
/// the machine cost to run, not a charge.
pub fn format_cost(dollars: f64) -> Option<String> {
    (dollars >= 0.005).then(|| format!("${dollars:.2} machine cost"))
}

/// The Usage row: `("October 2026 · 12.4 hours", Some("$3.10 machine cost"))`.
pub fn usage_summary(usage: &CloudUsage) -> (String, Option<String>) {
    let month = chrono::NaiveDate::parse_from_str(&format!("{}-01", usage.month), "%Y-%m-%d")
        .map(|date| date.format("%B %Y").to_string())
        .unwrap_or_else(|_| usage.month.clone());
    (
        format!("{month} · {}", format_hours(usage.seconds)),
        format_cost(usage.dollars),
    )
}

/// One line of the per-session usage breakdown.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageLine {
    /// The session's chat title, or "Other" for machine time no session
    /// owns.
    pub label: String,
    pub seconds: u64,
    pub dollars: f64,
}

/// Usage per session (sandboxes summed by chat), most time first; machine
/// time with no chat is one "Other" line at the end. `title` names a chat.
pub fn usage_breakdown(usage: &CloudUsage, title: impl Fn(&str) -> String) -> Vec<UsageLine> {
    let mut by_chat: Vec<(String, u64, f64)> = Vec::new();
    let mut other = (0u64, 0f64);
    for sandbox in &usage.sandboxes {
        match &sandbox.chat_id {
            Some(chat) => match by_chat.iter_mut().find(|(id, ..)| id == chat) {
                Some(row) => {
                    row.1 += sandbox.seconds;
                    row.2 += sandbox.dollars;
                }
                None => by_chat.push((chat.clone(), sandbox.seconds, sandbox.dollars)),
            },
            None => {
                other.0 += sandbox.seconds;
                other.1 += sandbox.dollars;
            }
        }
    }
    by_chat.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let mut lines: Vec<UsageLine> = by_chat
        .into_iter()
        .map(|(chat, seconds, dollars)| UsageLine {
            label: title(&chat),
            seconds,
            dollars,
        })
        .collect();
    if other.0 > 0 || other.1 > 0.0 {
        lines.push(UsageLine {
            label: "Other".into(),
            seconds: other.0,
            dollars: other.1,
        });
    }
    lines
}

/// `CloudStatus`, or why it could not be read.
pub async fn fetch_status(engine: EngineHandle) -> Result<CloudStatus, String> {
    engine
        .client()
        .call_as::<CloudStatus>(zeron_rpc::methods::CLOUD_STATUS, serde_json::json!({}))
        .await
        .map_err(|error| error.to_string())
}

/// `CloudSessions`, or why they could not be read.
pub async fn fetch_sessions(engine: EngineHandle) -> Result<CloudSessions, String> {
    engine
        .client()
        .call_as::<CloudSessions>(zeron_rpc::methods::CLOUD_SESSIONS, serde_json::json!({}))
        .await
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(id: &str, platform: &str, capability: Option<&str>) -> Device {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "name": id,
            "platform": platform,
            "lastSeenAt": null,
            "capabilities": capability.into_iter().collect::<Vec<_>>(),
        }))
        .unwrap()
    }

    fn session(chat: &str, device: &str, state: CloudState) -> CloudSession {
        CloudSession {
            chat_id: chat.into(),
            device_id: device.into(),
            space_id: "project".into(),
            state,
            ..Default::default()
        }
    }

    fn repo(full_name: &str, pushed_at: i64, description: Option<&str>) -> GithubRepo {
        GithubRepo {
            full_name: full_name.into(),
            clone_url: format!("https://github.com/{full_name}.git"),
            default_branch: "main".into(),
            private: false,
            description: description.map(str::to_string),
            pushed_at,
        }
    }

    #[test]
    fn device_lists_show_cloud_and_hide_session_machines() {
        let account = device(
            "cloud-a",
            "cloud",
            Some(zeron_proto::CLOUD_ACCOUNT_CAPABILITY),
        );
        let machine = device(
            "cloud-s",
            "cloud",
            Some(zeron_proto::CLOUD_SESSION_CAPABILITY),
        );
        let laptop = device("mac", "macos", None);
        assert!(is_listed(&account) && is_cloud_account(&account));
        assert!(!is_listed(&machine) && is_cloud_session_device(&machine));
        assert!(is_listed(&laptop) && !is_cloud(&laptop));
        assert_eq!(device_glyph("cloud-a", "cloud"), icons::CLOUD);
        assert_eq!(device_glyph("abc", "macos"), icons::LAPTOP);
        assert_eq!(device_glyph("abc", "ios"), icons::SMARTPHONE);
        assert_eq!(device_glyph("abc", "web"), icons::GLOBAL);
        assert_eq!(device_glyph("abc", "linux"), icons::MONITOR);
    }

    #[test]
    fn the_logical_device_reads_from_the_account() {
        assert_eq!(account_presence(None), Presence::Available);
        let mut status = CloudStatus {
            state: CloudState::Ready,
            device_id: Some("cloud-a".into()),
            available: true,
            ..Default::default()
        };
        assert_eq!(account_presence(Some(&status)), Presence::Available);
        assert!(!Presence::Available.warns());
        status.state = CloudState::Deleting;
        assert_eq!(account_presence(Some(&status)), Presence::Offline);
        status.state = CloudState::Ready;
        status.available = false;
        assert_eq!(account_presence(Some(&status)), Presence::Offline);
    }

    #[test]
    fn session_machines_read_from_their_session() {
        let sleeping = session("chat", "cloud-s", CloudState::Sleeping);
        assert_eq!(session_presence(true, Some(&sleeping)), Presence::Asleep);
        let starting = session("chat", "cloud-s", CloudState::Provisioning);
        assert_eq!(session_presence(false, Some(&starting)), Presence::Waking);
        let ready = session("chat", "cloud-s", CloudState::Ready);
        assert_eq!(session_presence(false, Some(&ready)), Presence::Online);
        assert_eq!(session_presence(false, None), Presence::Asleep);
        assert_eq!(session_presence(true, None), Presence::Online);
        assert!(!Presence::Asleep.warns() && !Presence::Waking.warns());
        // Side chats run in their parent's machine: found by host device.
        let sessions = CloudSessions {
            sessions: vec![sleeping.clone()],
            available: true,
        };
        assert_eq!(session_for(&sessions, "side", "cloud-s"), Some(&sleeping));
        assert_eq!(session_for(&sessions, "chat", "x"), Some(&sleeping));
        assert_eq!(session_for(&sessions, "other", "x"), None);
    }

    #[test]
    fn composer_notice_and_polling_follow_the_session_state() {
        assert_eq!(
            session_notice(CloudState::Provisioning),
            Some("Starting a Cloud machine for this session…")
        );
        assert_eq!(
            session_notice(CloudState::Sleeping),
            Some("This session's Cloud machine is asleep — it wakes when you send")
        );
        assert_eq!(session_notice(CloudState::Ready), None);
        let mut sessions = CloudSessions {
            sessions: vec![session("a", "s", CloudState::Sleeping)],
            available: true,
        };
        assert!(!any_session_settling(&sessions));
        for state in [
            CloudState::Provisioning,
            CloudState::Starting,
            CloudState::Deleting,
        ] {
            sessions.sessions[0].state = state;
            assert!(any_session_settling(&sessions), "{state:?}");
        }
        assert_eq!(
            account_recheck_after(CloudState::Deleting),
            Some(SESSION_POLL)
        );
        assert_eq!(account_recheck_after(CloudState::Ready), None);
    }

    #[test]
    fn retries_repeat_the_failed_action() {
        let mut status = CloudStatus {
            state: CloudState::Error,
            device_id: Some("cloud-a".into()),
            failed_action: Some("delete".into()),
            ..Default::default()
        };
        assert_eq!(retry_action(&status), CloudAction::Delete);
        status.failed_action = Some("enable".into());
        assert_eq!(retry_action(&status), CloudAction::Enable);
        assert!(account_enabled(&CloudStatus {
            state: CloudState::Ready,
            device_id: Some("cloud-a".into()),
            ..Default::default()
        }));
        assert!(!account_enabled(&CloudStatus {
            state: CloudState::Deleting,
            device_id: Some("cloud-a".into()),
            ..Default::default()
        }));
    }

    #[test]
    fn provider_link_reflects_authorization_and_health() {
        let mut vault = VaultStatus {
            available: true,
            ..Default::default()
        };
        assert_eq!(
            provider_link(&vault, VaultProvider::Codex, "cloud-1"),
            ProviderLink::NotConnected
        );
        vault.connections.push(VaultConnection {
            provider: VaultProvider::Codex,
            status: VaultConnectionStatus::Connected,
            authorized_devices: vec!["cloud-old".into()],
            account: Some("me@example.com".into()),
            updated_at: 0,
        });
        assert_eq!(
            provider_link(&vault, VaultProvider::Codex, "cloud-1"),
            ProviderLink::NotAuthorized {
                account: Some("me@example.com".into())
            }
        );
        assert_eq!(
            authorized_with(&vault, VaultProvider::Codex, "cloud-1"),
            vec!["cloud-old".to_string(), "cloud-1".to_string()]
        );
        vault.connections[0]
            .authorized_devices
            .push("cloud-1".into());
        assert!(matches!(
            provider_link(&vault, VaultProvider::Codex, "cloud-1"),
            ProviderLink::Connected { .. }
        ));
        vault.connections[0].status = VaultConnectionStatus::NeedsReconnect;
        assert!(matches!(
            provider_link(&vault, VaultProvider::Codex, "cloud-1"),
            ProviderLink::NeedsReconnect { .. }
        ));
    }

    #[test]
    fn session_machines_fold_into_one_cloud_row() {
        let vault_device = |id: &str, kind: &str, revoked: bool| VaultDevice {
            device_id: id.into(),
            kind: kind.into(),
            enrolled_at: 0,
            revoked_at: revoked.then_some(1),
            parent_id: (kind == "cloud").then(|| "cloud-1".into()),
        };
        let groups = group_vault_devices(&[
            vault_device("mac", "laptop", false),
            vault_device("s1", "cloud", false),
            vault_device("s2", "cloud", false),
            vault_device("s3", "cloud", true),
        ]);
        assert_eq!(groups.computers.len(), 1);
        assert_eq!(groups.cloud_machines, 2);
    }

    #[test]
    fn repos_sort_newest_first_and_filter_by_name_prefix() {
        let mut repos = vec![
            repo("acme/zeta", 10, None),
            repo("acme/web", 30, Some("Marketing site")),
            repo("zed/tools", 20, None),
        ];
        sort_repos(&mut repos);
        let names: Vec<_> = repos.iter().map(|r| r.full_name.as_str()).collect();
        assert_eq!(names, ["acme/web", "zed/tools", "acme/zeta"]);
        let hits = filter_repos(&repos, "ze");
        let names: Vec<_> = hits.iter().map(|r| r.full_name.as_str()).collect();
        assert_eq!(names, ["acme/zeta", "zed/tools"]);
        assert_eq!(filter_repos(&repos, "MARKETING").len(), 1);
        assert_eq!(filter_repos(&repos, "  ").len(), 3);
        assert!(filter_repos(&repos, "nothing").is_empty());
    }

    #[test]
    fn github_not_connected_shows_only_the_readable_text() {
        let wire = "github_not_connected: GitHub isn't connected for this device — connect it in Settings → Cloud";
        let shown = github_not_connected(wire).unwrap();
        assert!(!shown.contains("github_not_connected"));
        assert!(shown.starts_with("GitHub isn't connected"));
        assert_eq!(
            github_not_connected("forwarded: github_not_connected: Connect it").as_deref(),
            Some("Connect it")
        );
        assert!(github_not_connected("github_not_connected:").is_some());
        assert_eq!(github_not_connected("rate limited"), None);
    }

    #[test]
    fn usage_reads_hours_first_and_breaks_down_by_session() {
        let sandbox =
            |chat: Option<&str>, seconds: u64, dollars: f64| zeron_proto::CloudSandboxUsage {
                sandbox_id: format!("bx_{seconds}"),
                chat_id: chat.map(str::to_string),
                sandbox_type: "default".into(),
                seconds,
                dollars,
                ..Default::default()
            };
        let mut usage = CloudUsage {
            month: "2026-10".into(),
            seconds: 44_640,
            dollars: 0.0,
            available: true,
            sandboxes: vec![
                sandbox(Some("a"), 3600, 0.5),
                sandbox(None, 600, 0.0),
                sandbox(Some("b"), 36_000, 2.0),
                sandbox(Some("a"), 4440, 0.25),
            ],
            ..Default::default()
        };
        assert_eq!(
            usage_summary(&usage),
            ("October 2026 · 12.4 hours".to_string(), None)
        );
        usage.dollars = 3.104;
        assert_eq!(
            usage_summary(&usage).1.as_deref(),
            Some("$3.10 machine cost")
        );
        assert_eq!(format_hours(3600), "1.0 hour");
        let lines = usage_breakdown(&usage, |chat| format!("Chat {chat}"));
        let labels: Vec<_> = lines.iter().map(|l| l.label.as_str()).collect();
        assert_eq!(labels, ["Chat b", "Chat a", "Other"]);
        assert_eq!(lines[1].seconds, 8040);
        usage.month = "bogus".into();
        assert!(usage_summary(&usage).0.starts_with("bogus · "));
    }
}
