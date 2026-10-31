//! Muse's account surface over MSP (`account/*`, `usage/read`): who is signed
//! in, the subscription's usage, and the device-code sign-in Muse runs itself
//! and hands over for native rendering. Experimental-tier in Muse 1.4: the
//! handshake opts into `experimentalApi`.
//!
//! Muse owns its tokens end to end — it mints, refreshes and locks its own
//! credential store across processes — so zeron only asks; it never reads,
//! copies or refreshes a token. Which store Muse uses is the caller's
//! environment (see the engine's agent accounts: one config root per account,
//! file-backed).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Value, json};

use super::host::{Host, Mode};
use super::rpc::Inbound;
use crate::HarnessError;

/// How long a sign-in waits for the browser approval before giving up.
const LOGIN_WAIT: Duration = Duration::from_secs(15 * 60);
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

/// Which credential Muse is using (`account/read`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MuseAccount {
    /// `loggedOut`, `envKey`, `apiKey`, `accountLogin` — open vocabulary.
    pub state: String,
    /// The account identity for `accountLogin` (display only).
    pub label: Option<String>,
    pub avatar_url: Option<String>,
    /// `false` when the endpoint needs no credential (keyless deployments).
    pub credential_required: bool,
}

impl MuseAccount {
    fn parse(value: &Value) -> Self {
        let text = |key: &str| {
            value
                .get(key)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        };
        Self {
            state: text("state").unwrap_or_else(|| "loggedOut".into()),
            label: text("label"),
            avatar_url: text("avatarUrl"),
            credential_required: value
                .get("credentialRequired")
                .and_then(Value::as_bool)
                .unwrap_or(true),
        }
    }

    pub fn signed_in(&self) -> bool {
        self.state != "loggedOut"
    }
}

/// One usage window of the subscription (`usage/read`).
#[derive(Debug, Clone, PartialEq)]
pub struct MuseUsageWindow {
    /// Percent used; above 100 when over quota.
    pub used_percent: f64,
    pub resets_at_ms: Option<i64>,
    /// The window's length (the weekly block has none).
    pub duration_mins: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MuseUsage {
    pub tier: String,
    pub window: MuseUsageWindow,
    pub weekly: MuseUsageWindow,
}

impl MuseUsage {
    /// The meters as the accounts page shows them: the current window, then
    /// the week; over-quota percentages pin at full.
    pub fn windows(&self) -> Vec<zeron_proto::AgentUsageWindow> {
        let resets = |ms: Option<i64>| ms.and_then(chrono::DateTime::from_timestamp_millis);
        let fraction = |percent: f64| (percent / 100.0).clamp(0.0, 1.0) as f32;
        vec![
            zeron_proto::AgentUsageWindow {
                label: zeron_proto::plan_usage::codex_window_label(
                    self.window.duration_mins.unwrap_or(0) * 60,
                )
                .to_string(),
                used_fraction: fraction(self.window.used_percent),
                resets_at: resets(self.window.resets_at_ms),
            },
            zeron_proto::AgentUsageWindow {
                label: "Week".into(),
                used_fraction: fraction(self.weekly.used_percent),
                resets_at: resets(self.weekly.resets_at_ms),
            },
        ]
    }

    pub(super) fn parse(value: &Value) -> Option<Self> {
        let window = |v: &Value| {
            Some(MuseUsageWindow {
                used_percent: v.get("usedPercent")?.as_f64()?,
                resets_at_ms: v.get("resetsAtMs").and_then(Value::as_i64),
                duration_mins: v.get("windowDurationMins").and_then(Value::as_i64),
            })
        };
        Some(Self {
            tier: value.get("tier")?.as_str()?.to_owned(),
            window: window(value.get("window")?)?,
            weekly: window(value.get("weekly")?)?,
        })
    }
}

/// An account-surface host: no shell, no writes, no session log.
async fn open(exe: &std::path::Path, env: &BTreeMap<String, String>) -> Result<Host, HarnessError> {
    let cwd: PathBuf = std::env::temp_dir();
    let mut host = Host::spawn(exe, &cwd, Mode::ReadOnly, env)?;
    host.initialize_with(true, true).await?;
    Ok(host)
}

async fn ask(host: &mut Host, method: &str, params: Value) -> Result<Value, HarnessError> {
    host.call(method, params, &mut Vec::new())
        .await?
        .map_err(|failure| super::host::failed(method, &failure))
}

/// Who `env`'s Muse is signed in as.
pub async fn read(
    exe: &std::path::Path,
    env: &BTreeMap<String, String>,
) -> Result<MuseAccount, HarnessError> {
    let mut host = open(exe, env).await?;
    let state = ask(&mut host, "account/read", json!({})).await;
    host.shutdown(SHUTDOWN_GRACE).await;
    Ok(MuseAccount::parse(&state?))
}

/// The subscription's last-observed usage, when Muse has seen one.
pub async fn usage(
    exe: &std::path::Path,
    env: &BTreeMap<String, String>,
) -> Result<Option<MuseUsage>, HarnessError> {
    let mut host = open(exe, env).await?;
    let usage = ask(&mut host, "usage/read", json!({})).await;
    host.shutdown(SHUTDOWN_GRACE).await;
    Ok(usage?.get("usage").and_then(MuseUsage::parse))
}

/// Sign `env`'s Muse out (Muse revokes the session first, best-effort).
pub async fn logout(
    exe: &std::path::Path,
    env: &BTreeMap<String, String>,
) -> Result<MuseAccount, HarnessError> {
    let mut host = open(exe, env).await?;
    let state = ask(&mut host, "account/logout", json!({})).await;
    host.shutdown(SHUTDOWN_GRACE).await;
    Ok(MuseAccount::parse(&state?))
}

/// A device-code sign-in Muse is running: show `verification_url` and
/// `user_code`, then [`DeviceLogin::finish`]. Dropping it cancels the flow.
pub struct DeviceLogin {
    pub verification_url: String,
    pub user_code: String,
    host: Option<Host>,
}

/// Start the RFC 8628 device-code sign-in in `env`'s Muse.
pub async fn start_device_login(
    exe: &std::path::Path,
    env: &BTreeMap<String, String>,
) -> Result<DeviceLogin, HarnessError> {
    let mut host = open(exe, env).await?;
    let started = ask(
        &mut host,
        "account/loginStart",
        json!({ "type": "deviceCode" }),
    )
    .await;
    let started = match started {
        Ok(started) => started,
        Err(error) => {
            host.shutdown(SHUTDOWN_GRACE).await;
            return Err(error);
        }
    };
    let text = |key: &str| {
        started
            .get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let (Some(verification_url), Some(user_code)) = (text("verificationUrl"), text("userCode"))
    else {
        host.shutdown(SHUTDOWN_GRACE).await;
        return Err(HarnessError::Protocol(
            "Muse didn't return a sign-in code".into(),
        ));
    };
    Ok(DeviceLogin {
        verification_url,
        user_code,
        host: Some(host),
    })
}

impl DeviceLogin {
    /// Wait for the approval; the signed-in account on success, Muse's own
    /// reason (denied, expired, failed) otherwise.
    pub async fn finish(mut self) -> Result<MuseAccount, String> {
        let mut host = self.host.take().expect("finished once");
        let outcome = tokio::time::timeout(LOGIN_WAIT, async {
            loop {
                match host.next().await {
                    Ok(Inbound::Notification { method, params })
                        if method == "account/loginCompleted" =>
                    {
                        tracing::info!(target: "zeron_harness::muse", outcome = ?params.get("outcome"), message = ?params.get("message"), "Muse sign-in completed");
                        return Ok(params);
                    }
                    Ok(Inbound::Notification { method, .. }) => {
                        tracing::info!(target: "zeron_harness::muse", %method, "Muse sign-in host notification");
                    }
                    Ok(_) => {}
                    Err(error) => {
                        tracing::warn!(target: "zeron_harness::muse", %error, stderr = ?host.tail.snapshot(), "Muse sign-in host failed");
                        return Err(error.to_string());
                    }
                }
            }
        })
        .await
        .unwrap_or_else(|_| Err("The sign-in timed out.".into()));
        let result = match outcome {
            Ok(params) => {
                let outcome = params
                    .get("outcome")
                    .and_then(Value::as_str)
                    .unwrap_or("failed");
                if outcome == "granted" {
                    ask(&mut host, "account/read", json!({}))
                        .await
                        .map(|state| MuseAccount::parse(&state))
                        .map_err(|error| error.to_string())
                } else {
                    Err(params
                        .get("message")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .unwrap_or_else(|| match outcome {
                            "denied" => "The sign-in was denied.".into(),
                            "expired" => "The sign-in code expired before it was approved.".into(),
                            "cancelled" => "The sign-in was cancelled.".into(),
                            _ => "The sign-in failed.".into(),
                        }))
                }
            }
            Err(error) => {
                let _ = ask(&mut host, "account/loginCancel", json!({})).await;
                Err(error)
            }
        };
        host.shutdown(SHUTDOWN_GRACE).await;
        result
    }
}

impl Drop for DeviceLogin {
    fn drop(&mut self) {
        // Abandoned before finishing: the host dies with it (owned child),
        // which leaves the device code to expire server-side.
        self.host.take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_and_usage_parse_the_schema_shapes() {
        let account = MuseAccount::parse(&json!({
            "state": "accountLogin", "label": "a@example.com",
            "avatarUrl": "https://x/y.png", "credentialRequired": true
        }));
        assert!(account.signed_in());
        assert_eq!(account.label.as_deref(), Some("a@example.com"));
        assert!(
            !MuseAccount::parse(&json!({"state": "loggedOut", "credentialRequired": true}))
                .signed_in()
        );

        let usage = MuseUsage::parse(&json!({
            "observedAtMs": 1, "tier": "pro",
            "window": {"usedPercent": 42, "resetsAtMs": 1000, "windowDurationMins": 300},
            "weekly": {"usedPercent": 105, "resetsAtMs": 2000}
        }))
        .unwrap();
        assert_eq!(usage.window.duration_mins, Some(300));
        assert_eq!(usage.weekly.used_percent, 105.0);
        assert!(MuseUsage::parse(&json!({"tier": "pro"})).is_none());
    }
}
