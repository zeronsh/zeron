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
    CloudSession, CloudSessions, CloudState, CloudStatus, Device, GithubRepo, VaultConnection,
    VaultConnectionStatus, VaultDevice, VaultProvider, VaultStatus,
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

/// The ranges the Usage section shows credits over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UsageRange {
    Last7Days,
    #[default]
    Last30Days,
    ThisMonth,
    LastMonth,
    Last90Days,
}

impl UsageRange {
    pub const ALL: [UsageRange; 5] = [
        UsageRange::Last7Days,
        UsageRange::Last30Days,
        UsageRange::ThisMonth,
        UsageRange::LastMonth,
        UsageRange::Last90Days,
    ];

    pub fn label(self) -> &'static str {
        match self {
            UsageRange::Last7Days => "Last 7 days",
            UsageRange::Last30Days => "Last 30 days",
            UsageRange::ThisMonth => "This month",
            UsageRange::LastMonth => "Last month",
            UsageRange::Last90Days => "Last 90 days",
        }
    }

    /// `[from, to]`, inclusive UTC days, for `today` (UTC).
    pub fn days(self, today: chrono::NaiveDate) -> (chrono::NaiveDate, chrono::NaiveDate) {
        use chrono::Datelike;
        let back = |days: i64| today - chrono::Duration::days(days - 1);
        let month_start = today.with_day(1).unwrap_or(today);
        match self {
            UsageRange::Last7Days => (back(7), today),
            UsageRange::Last30Days => (back(30), today),
            UsageRange::Last90Days => (back(90), today),
            UsageRange::ThisMonth => (month_start, today),
            UsageRange::LastMonth => {
                let end = month_start - chrono::Duration::days(1);
                (end.with_day(1).unwrap_or(end), end)
            }
        }
    }

    /// The `CloudUsage` params for this range.
    pub fn params(self, today: chrono::NaiveDate) -> serde_json::Value {
        let (from, to) = self.days(today);
        serde_json::json!({
            "from": from.format("%Y-%m-%d").to_string(),
            "to": to.format("%Y-%m-%d").to_string(),
        })
    }
}

/// "1,240 credits" — whole credits (a minute each), with a decimal only
/// under ten so a short session still reads as something.
pub fn format_credits(credits: f64) -> String {
    let negative = credits < 0.0;
    let value = credits.abs();
    let number = if value > 0.0 && value < 10.0 && value.fract() >= 0.05 {
        format!("{value:.1}")
    } else {
        let whole = value.round() as u64;
        let digits = whole.to_string();
        let mut grouped = String::new();
        for (i, c) in digits.chars().enumerate() {
            if i > 0 && (digits.len() - i) % 3 == 0 {
                grouped.push(',');
            }
            grouped.push(c);
        }
        grouped
    };
    let unit = if number == "1" { "credit" } else { "credits" };
    format!("{}{number} {unit}", if negative { "-" } else { "" })
}

/// Each day's bar height as a fraction of the busiest day's (0 when nothing
/// ran in the whole range).
pub fn bar_fractions(days: &[zeron_proto::CloudUsageDay]) -> Vec<f32> {
    let peak = days.iter().map(|d| d.credits).fold(0.0_f64, f64::max);
    days.iter()
        .map(|d| {
            if peak > 0.0 {
                (d.credits / peak) as f32
            } else {
                0.0
            }
        })
        .collect()
}

/// "Oct 5" for a `YYYY-MM-DD` day.
pub fn short_day(day: &str) -> String {
    chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d")
        .map(|date| date.format("%b %-d").to_string())
        .unwrap_or_else(|_| day.to_string())
}

/// Whether Cloud can't start or wake a session for lack of credits.
pub fn out_of_credits(status: &CloudStatus) -> bool {
    status.credits.is_some_and(|credits| credits <= 0.0)
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
            accounts: 1,
            has_active: true,
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
    fn usage_ranges_and_credits_read_naturally() {
        let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 5).unwrap();
        let day = |m, d| chrono::NaiveDate::from_ymd_opt(2026, m, d).unwrap();
        assert_eq!(UsageRange::Last7Days.days(today), (day(9, 29), today));
        assert_eq!(UsageRange::Last30Days.days(today), (day(9, 6), today));
        assert_eq!(UsageRange::ThisMonth.days(today), (day(10, 1), today));
        assert_eq!(UsageRange::LastMonth.days(today), (day(9, 1), day(9, 30)));
        assert_eq!(
            UsageRange::ThisMonth.params(today),
            serde_json::json!({"from": "2026-10-01", "to": "2026-10-05"})
        );
        assert_eq!(format_credits(1.0), "1 credit");
        assert_eq!(format_credits(0.0), "0 credits");
        assert_eq!(format_credits(2.5), "2.5 credits");
        assert_eq!(format_credits(1240.4), "1,240 credits");
        assert_eq!(format_credits(1_234_567.0), "1,234,567 credits");
        assert_eq!(format_credits(-12.0), "-12 credits");
        let usage_day = |credits| zeron_proto::CloudUsageDay {
            day: "2026-10-05".into(),
            credits,
        };
        assert_eq!(
            bar_fractions(&[usage_day(0.0), usage_day(30.0), usage_day(60.0)]),
            [0.0, 0.5, 1.0]
        );
        assert_eq!(bar_fractions(&[usage_day(0.0)]), [0.0]);
        assert_eq!(short_day("2026-10-05"), "Oct 5");
        let status = |credits| CloudStatus {
            credits,
            ..Default::default()
        };
        assert!(out_of_credits(&status(Some(0.0))));
        assert!(!out_of_credits(&status(Some(3.0))));
        assert!(!out_of_credits(&status(None)));
    }
}
