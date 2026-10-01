//! Host RPC vocabulary: what a viewer asks an execution host over the
//! device-room relay (catalogs, refs, folders, uploads, queue actions), and
//! the capability gates that decide whether to ask at all.

use std::sync::Arc;

pub use zeron_proto::{
    DriveEntry, DriveListing, FolderEntry, FolderListing, RepoRef, WorktreeSpec,
};

/// One agent login's plan usage on a host (engine `ListAgentAccounts`),
/// reduced to what a phone shows: no credentials, only the meters.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentUsage {
    pub harness: String,
    pub email: Option<String>,
    pub plan_label: Option<String>,
    pub active: bool,
    pub windows: Vec<UsageWindow>,
    /// Epoch ms of the probe the windows came from (`None` = never fetched).
    pub fetched_at_ms: Option<i64>,
    /// Why the last probe failed ("Rate limited — retrying in 2m", …).
    pub error: Option<String>,
}

/// One rate-limit window ("5-hour", "Weekly", …).
#[derive(Debug, Clone, PartialEq)]
pub struct UsageWindow {
    pub label: String,
    /// 0.0..=1.0
    pub used_fraction: f32,
    pub resets_at_ms: Option<i64>,
}

/// Parse a `ListAgentAccounts` reply leniently: an account whose harness this
/// build doesn't know still shows, and a malformed one is skipped rather
/// than failing the whole list.
pub fn parse_agent_usage(value: &serde_json::Value) -> Vec<AgentUsage> {
    let str_of =
        |v: &serde_json::Value, k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_owned);
    value
        .get("accounts")
        .and_then(|a| a.as_array())
        .map(|accounts| {
            accounts
                .iter()
                .filter_map(|a| {
                    let harness = str_of(a, "harness")?;
                    let windows = a
                        .get("usageWindows")
                        .and_then(|w| w.as_array())
                        .map(|ws| {
                            ws.iter()
                                .filter_map(|w| {
                                    Some(UsageWindow {
                                        label: str_of(w, "label")?,
                                        used_fraction: w
                                            .get("usedFraction")?
                                            .as_f64()?
                                            .clamp(0.0, 1.0)
                                            as f32,
                                        resets_at_ms: str_of(w, "resetsAt")
                                            .and_then(|t| {
                                                chrono::DateTime::parse_from_rfc3339(&t).ok()
                                            })
                                            .map(|t| t.timestamp_millis()),
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    Some(AgentUsage {
                        harness,
                        email: str_of(a, "email"),
                        plan_label: str_of(a, "planLabel"),
                        active: a.get("active").and_then(|x| x.as_bool()).unwrap_or(false),
                        windows,
                        fetched_at_ms: a.get("usageFetchedAt").and_then(|x| x.as_i64()),
                        error: str_of(a, "usageError"),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Engine capability strings a device row advertises (`Device::capabilities`).
/// Capabilities, not semver: a personal integration build can share an
/// upstream version without the doc/RPC surface.
pub mod capability {
    pub const MESSAGE_QUEUE_V1: &str = "message-queue-v1";
    pub const MESSAGE_QUEUE_ACTIONS_V1: &str = "message-queue-actions-v1";
    pub const MESSAGE_QUEUE_ATTACHMENTS_V1: &str = "message-queue-attachments-v1";
    pub const MESSAGE_QUEUE_CLEAN_ATTACHMENT_TEXT_V1: &str =
        "message-queue-clean-attachment-text-v1";
    pub const MESSAGE_QUEUE_EDIT_LEASE_V1: &str = "message-queue-edit-lease-v1";

    /// Every queue capability (demo hosts advertise the full surface).
    pub const ALL_QUEUE: &[&str] = &[
        MESSAGE_QUEUE_V1,
        MESSAGE_QUEUE_ACTIONS_V1,
        MESSAGE_QUEUE_ATTACHMENTS_V1,
        MESSAGE_QUEUE_CLEAN_ATTACHMENT_TEXT_V1,
        MESSAGE_QUEUE_EDIT_LEASE_V1,
    ];
}

/// Queued-attachment version gate (composer.rs QUEUED_ATTACHMENTS_MIN): the
/// host must defer commands carrying `pending://` refs until the bytes land.
pub const QUEUED_ATTACHMENTS_MIN: (u64, u64, u64) = (0, 2, 12);

/// Upload progress callback: fraction in `0.0..=1.0` of the file's bytes the
/// host has committed.
pub type ProgressFn = Arc<dyn Fn(f64) + Send + Sync>;

/// Relay method names (single source of truth: `zeron_rpc::methods`).
pub mod methods {
    pub use zeron_rpc::methods::*;
}

#[cfg(test)]
mod usage_tests {
    use super::*;

    #[test]
    fn parses_accounts_and_skips_bad_windows() {
        let v = serde_json::json!({
            "accounts": [
                {"id": "a", "harness": "claude-code", "email": "me@example.com", "planLabel": "Max",
                 "active": true, "usageFetchedAt": 1790650000000i64,
                 "usageWindows": [
                    {"label": "5-hour", "usedFraction": 0.34, "resetsAt": "2026-09-29T08:00:00Z"},
                    {"label": "Weekly", "usedFraction": 1.7, "resetsAt": null},
                    {"usedFraction": 0.1}
                 ]},
                {"id": "b", "harness": "some-future-agent", "active": false, "usageError": "Sign in again"},
                {"id": "c"}
            ],
            "warnings": []
        });
        let list = parse_agent_usage(&v);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].plan_label.as_deref(), Some("Max"));
        assert_eq!(list[0].windows.len(), 2);
        assert_eq!(list[0].windows[0].resets_at_ms, Some(1790668800000));
        assert_eq!(list[0].windows[1].used_fraction, 1.0);
        assert_eq!(list[1].harness, "some-future-agent");
        assert_eq!(list[1].error.as_deref(), Some("Sign in again"));
    }

    /// Devin reports a weekly quota only (the desktop's "DEVIN ACCOUNTS"
    /// card: Devin Pro, In use, Week 50%). Serialized from the engine's own
    /// `AgentAccount`, so the wire shape (`"harness": "devin"`, RFC 3339
    /// `resetsAt`) is the one the engine sends.
    #[test]
    fn parses_a_devin_weekly_quota() {
        let account = zeron_proto::AgentAccount {
            id: "devin-1".into(),
            harness: zeron_proto::HarnessId::Devin,
            email: Some("dev@example.com".into()),
            plan_label: Some("Devin Pro".into()),
            active: true,
            usage_windows: vec![zeron_proto::AgentUsageWindow {
                label: "Week".into(),
                used_fraction: 0.5,
                resets_at: chrono::DateTime::parse_from_rfc3339("2026-10-05T00:00:00Z")
                    .ok()
                    .map(|t| t.with_timezone(&chrono::Utc)),
            }],
            usage_fetched_at: Some(1790650000000),
            usage_error: None,
            display_name: None,
            organization: None,
            auth_kind: None,
            switchable: true,
            saved_at: None,
            provider: None,
        };
        let v = serde_json::json!({ "accounts": [account], "warnings": [] });
        let list = parse_agent_usage(&v);
        assert_eq!(list.len(), 1);
        let devin = &list[0];
        assert_eq!(devin.harness, "devin");
        assert_eq!(devin.plan_label.as_deref(), Some("Devin Pro"));
        assert!(devin.active);
        assert_eq!(devin.windows.len(), 1);
        assert_eq!(devin.windows[0].label, "Week");
        assert!((devin.windows[0].used_fraction - 0.5).abs() < 1e-6);
        assert_eq!(devin.windows[0].resets_at_ms, Some(1791158400000));
        assert_eq!(devin.fetched_at_ms, Some(1790650000000));
    }
}
