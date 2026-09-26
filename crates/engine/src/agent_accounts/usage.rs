//! Usage probes for Grok, Devin, OpenCode, Pi and Hermes — each the view the
//! agent's own CLI renders, with the same stale-while-revalidate cache,
//! [`ProbeError`] classification and backoff as the original providers.
//!
//! - **Grok**: `GET cli-chat-proxy.grok.com/v1/billing?format=credits`
//!   (`/usage` in the CLI) — the Grok Build share of the current period.
//!   Only a 401/403 refreshes, and only a SAVED slot (OIDC `refresh_token`
//!   grant against the entry's own issuer, single-flight); the live token is
//!   the CLI's to rotate.
//! - **Devin**: Connect-JSON `SeatManagementService/GetUserStatus` (what
//!   `devin auth status` calls) — daily/weekly quota, plus the identity the
//!   key file lacks, written back into the slot. Keys don't refresh.
//! - **ChatGPT logins** (OpenCode `openai`, Pi `openai-codex`, Hermes
//!   `openai-codex`): the Codex usage endpoint — the same token kind.
//! - **Claude logins** (Pi `anthropic`): Claude's `/api/oauth/usage`.
//! - **Copilot** (OpenCode / Pi `github-copilot`):
//!   `api.github.com/copilot_internal/user` — premium-request quota.
//! - **Nous** (Hermes `nous`): the portal's `/api/oauth/account` credits.
//!
//! None of these refresh a per-provider store's tokens: those rotate, and
//! the agent (or Hermes' pool) owns them — a rejected token reads "switch to
//! it to refresh" / "refreshes next time" instead.

use super::stores::{
    DEVIN_SERVERS, GROK_ISSUERS, NOUS_PORTALS, Upstream, copilot_api_base, devin_api_key,
    trusted_base, upstream_of,
};
use super::*;

pub(super) const GROK_USAGE_URL: &str = "https://cli-chat-proxy.grok.com/v1/billing?format=credits";
pub(super) const NOUS_PORTAL: &str = "https://portal.nousresearch.com";
const DEVIN_STATUS_RPC: &str = "exa.seat_management_pb.SeatManagementService/GetUserStatus";
const DEVIN_DEFAULT_API_SERVER: &str = "https://server.codeium.com";
/// The CLI version the status call identifies as (the server wants one).
const DEVIN_CLI_VERSION: &str = "3000.11.3";

impl AgentAccounts {
    pub(super) async fn grok_usage(
        &self,
        slot: &Slot,
        is_active: bool,
    ) -> Result<UsageSnapshot, ProbeError> {
        let missing = ProbeError::NoCredentials {
            why: NoCredentials::Missing,
        };
        // A Grok slot is one issuer's token set.
        let access_token = str_field(&slot.credentials, "key").ok_or(missing)?;
        match self.grok_usage_request(&access_token).await {
            // Same rule as Claude: rotate slot-owned tokens only after the
            // endpoint REJECTED them; the live pair is the CLI's.
            Err(ProbeError::Unauthorized { status })
                if !is_active && self.inner.endpoints.allow_slot_refresh =>
            {
                match self.refresh_grok_slot(slot).await? {
                    Some(fresh) => self.grok_usage_request(&fresh).await,
                    None => Err(ProbeError::Unauthorized { status }),
                }
            }
            result => result,
        }
    }

    async fn grok_usage_request(&self, access_token: &str) -> Result<UsageSnapshot, ProbeError> {
        let body = probe_json(
            "grok",
            "usage",
            self.inner
                .http
                .get(&self.inner.endpoints.grok_usage)
                .bearer_auth(access_token)
                .header("X-XAI-Token-Auth", "xai-grok-cli"),
        )
        .await?;
        grok_usage_snapshot(&body).ok_or_else(|| schema_error("grok", &body))
    }

    /// Single-flight per slot, like Claude's: a second concurrent refresh of
    /// a single-use refresh token would revoke it. `Ok(None)` = in flight.
    async fn refresh_grok_slot(&self, slot: &Slot) -> Result<Option<String>, ProbeError> {
        if !lock(&self.inner.inflight_refreshes).insert(slot.id.clone()) {
            return Ok(None);
        }
        let _release = InflightGuard {
            set: &self.inner.inflight_refreshes,
            keys: vec![slot.id.clone()],
        };
        let missing = ProbeError::NoCredentials {
            why: NoCredentials::Missing,
        };
        let entry = &slot.credentials;
        let refresh_token = str_field(entry, "refresh_token").ok_or(missing.clone())?;
        let client_id = str_field(entry, "oidc_client_id").ok_or(missing)?;
        // The issuer comes from the credential file: the refresh token only
        // goes to an xAI host.
        let issuer = trusted_base(
            &str_field(entry, "oidc_issuer").unwrap_or_else(|| "https://auth.x.ai".to_string()),
            GROK_ISSUERS,
            self.inner.endpoints.allow_loopback_http,
        )
        .ok_or(ProbeError::UntrustedEndpoint)?;
        let body = probe_json(
            "grok",
            "refresh",
            self.inner
                .http
                .post(format!(
                    "{}/oauth2/token",
                    issuer.as_str().trim_end_matches('/')
                ))
                .form(&[
                    ("grant_type", "refresh_token"),
                    ("refresh_token", refresh_token.as_str()),
                    ("client_id", client_id.as_str()),
                ]),
        )
        .await
        .map_err(|error| match error {
            // invalid_grant: the saved login is dead — "sign in again".
            ProbeError::Http { status: 400, .. } => ProbeError::Unauthorized { status: 400 },
            other => other,
        })?;
        let access_token =
            str_field(&body, "access_token").ok_or_else(|| schema_error("grok", &body))?;
        let expires_in = body
            .get("expires_in")
            .and_then(|v| v.as_i64())
            .unwrap_or(3600);
        let mut credentials = slot.credentials.clone();
        if let Some(entry) = credentials.as_object_mut() {
            entry.insert("key".into(), serde_json::json!(access_token));
            if let Some(rotated) = str_field(&body, "refresh_token") {
                entry.insert("refresh_token".into(), serde_json::json!(rotated));
            }
            entry.insert(
                "expires_at".into(),
                serde_json::json!(
                    (Utc::now() + chrono::TimeDelta::seconds(expires_in)).to_rfc3339()
                ),
            );
        }
        let mut refreshed = slot.clone();
        refreshed.credentials = credentials;
        refreshed.saved_at = now_ms();
        if let Err(err) = self.write_slot(&refreshed) {
            tracing::warn!(slot = %slot.id, error = %err, "refreshed grok slot write failed");
        }
        Ok(Some(access_token))
    }

    /// Devin's quota view, which also names the account behind the key.
    pub(super) async fn devin_usage(&self, slot: &Slot) -> Result<UsageSnapshot, ProbeError> {
        let api_key = devin_api_key(&slot.credentials).ok_or(ProbeError::NoCredentials {
            why: NoCredentials::Missing,
        })?;
        // The server comes from the credential file: the key only goes to a
        // Devin / Windsurf host.
        let server = trusted_base(
            &str_field(&slot.credentials, "api_server_url")
                .unwrap_or_else(|| DEVIN_DEFAULT_API_SERVER.to_string()),
            DEVIN_SERVERS,
            self.inner.endpoints.allow_loopback_http,
        )
        .ok_or(ProbeError::UntrustedEndpoint)?;
        let server = server.as_str();
        let body = probe_json(
            "devin",
            "usage",
            self.inner
                .http
                .post(format!(
                    "{}/{DEVIN_STATUS_RPC}",
                    server.trim_end_matches('/')
                ))
                .header("Connect-Protocol-Version", "1")
                .json(&serde_json::json!({
                    "metadata": {
                        "apiKey": api_key,
                        "ideName": "devin-cli",
                        "ideVersion": DEVIN_CLI_VERSION,
                        "extensionName": "devin-cli",
                        "extensionVersion": DEVIN_CLI_VERSION,
                        "locale": "en_US",
                        "os": std::env::consts::OS,
                        "sessionId": "zeron-accounts",
                        "requestId": "1",
                    }
                })),
        )
        .await?;
        let (snapshot, identity) =
            devin_usage_snapshot(&body).ok_or_else(|| schema_error("devin", &body))?;
        // Adopt the probed identity (best-effort; a failed write re-probes).
        let mut updated = slot.clone();
        if let Some(email) = identity.email {
            updated.profile.email = email;
        }
        if identity.name.is_some() {
            updated.profile.display_name = identity.name;
        }
        if snapshot.plan_label.is_some() {
            updated.profile.plan = snapshot.plan_label.clone();
        }
        if updated.profile.email != slot.profile.email
            || updated.profile.display_name != slot.profile.display_name
            || updated.profile.plan != slot.profile.plan
        {
            updated.saved_at = now_ms();
            if let Err(err) = self.write_slot(&updated) {
                tracing::warn!(slot = %slot.id, error = %err, "devin identity write-back failed");
            }
        }
        Ok(snapshot)
    }

    /// A per-provider login (OpenCode / Pi entry, Hermes pool entry): probe
    /// the vendor behind it with its own token.
    pub(super) async fn keyed_usage(
        &self,
        harness: HarnessId,
        slot: &Slot,
    ) -> Result<UsageSnapshot, ProbeError> {
        let missing = ProbeError::NoCredentials {
            why: NoCredentials::Missing,
        };
        let key = slot.store_key.as_deref().ok_or(missing.clone())?;
        let creds = &slot.credentials;
        if harness == HarnessId::Hermes {
            if str_field(creds, "auth_type").as_deref() == Some("api_key") {
                return Err(ProbeError::NoCredentials {
                    why: NoCredentials::ApiKey,
                });
            }
            let access = str_field(creds, "access_token").ok_or(missing)?;
            return match key {
                "openai-codex" => {
                    let account = jwt_claims(&access)
                        .and_then(|c| {
                            c.get("https://api.openai.com/auth")
                                .and_then(|a| str_field(a, "chatgpt_account_id"))
                        })
                        .unwrap_or_default();
                    self.openai_usage("hermes", &access, &account).await
                }
                "nous" => {
                    // A pool entry's own portal is honoured only on a Nous host.
                    let portal = match str_field(creds, "portal_base_url") {
                        Some(raw) => trusted_base(
                            &raw,
                            NOUS_PORTALS,
                            self.inner.endpoints.allow_loopback_http,
                        )
                        .ok_or(ProbeError::UntrustedEndpoint)?
                        .as_str()
                        .to_string(),
                        None => self.inner.endpoints.nous_portal.clone(),
                    };
                    self.nous_usage(&portal, &access).await
                }
                _ => Err(ProbeError::NoCredentials {
                    why: NoCredentials::Unsupported,
                }),
            };
        }
        match upstream_of(harness, key) {
            Some(Upstream::OpenAi) => {
                let access = str_field(creds, "access").ok_or(missing)?;
                let account = str_field(creds, "accountId").unwrap_or_default();
                self.openai_usage(stores::cli_name(harness), &access, &account)
                    .await
            }
            Some(Upstream::Anthropic) => {
                let access = str_field(creds, "access").ok_or(missing)?;
                self.claude_usage_request(&access).await
            }
            Some(Upstream::Copilot) => {
                let token = str_field(creds, "refresh").ok_or(missing)?;
                // GitHub Enterprise: a validated plain host, or nothing is sent.
                let api = copilot_api_base(
                    creds,
                    &self.inner.endpoints.github_api,
                    self.inner.endpoints.allow_loopback_http,
                )
                .ok_or(ProbeError::UntrustedEndpoint)?;
                self.copilot_usage(&api, &token).await
            }
            None => Err(ProbeError::NoCredentials {
                why: NoCredentials::Unsupported,
            }),
        }
    }

    async fn openai_usage(
        &self,
        provider: &'static str,
        access_token: &str,
        account_id: &str,
    ) -> Result<UsageSnapshot, ProbeError> {
        let body = probe_json(
            provider,
            "usage",
            self.inner
                .http
                .get(&self.inner.endpoints.codex_usage)
                .bearer_auth(access_token)
                .header("chatgpt-account-id", account_id),
        )
        .await?;
        codex_usage_snapshot(&body).ok_or_else(|| schema_error(provider, &body))
    }

    async fn copilot_usage(&self, api: &str, token: &str) -> Result<UsageSnapshot, ProbeError> {
        let body = probe_json(
            "copilot",
            "usage",
            self.inner
                .http
                .get(format!(
                    "{}/copilot_internal/user",
                    api.trim_end_matches('/')
                ))
                .header("Authorization", format!("token {token}"))
                .header("Accept", "application/json")
                .header("User-Agent", "zeron"),
        )
        .await?;
        copilot_usage_snapshot(&body).ok_or_else(|| schema_error("copilot", &body))
    }

    async fn nous_usage(
        &self,
        portal: &str,
        access_token: &str,
    ) -> Result<UsageSnapshot, ProbeError> {
        let body = probe_json(
            "nous",
            "usage",
            self.inner
                .http
                .get(format!(
                    "{}/api/oauth/account",
                    portal.trim_end_matches('/')
                ))
                .bearer_auth(access_token)
                .header("Accept", "application/json"),
        )
        .await?;
        nous_usage_snapshot(&body).ok_or_else(|| schema_error("nous", &body))
    }
}

/// Grok's `/billing?format=credits`: the current period, with the Grok Build
/// product's share of it (the blended `creditUsagePercent` as fallback).
pub(super) fn grok_usage_snapshot(body: &serde_json::Value) -> Option<UsageSnapshot> {
    let config = body.get("config")?;
    let period = config.get("currentPeriod");
    let used_percent = config
        .get("productUsage")
        .and_then(|u| u.as_array())
        .and_then(|products| {
            products.iter().find_map(|p| {
                (p.get("product").and_then(|v| v.as_str()) == Some("GrokBuild"))
                    .then(|| p.get("usagePercent").and_then(|v| v.as_f64()))
                    .flatten()
            })
        })
        .or_else(|| config.get("creditUsagePercent").and_then(|v| v.as_f64()))?;
    let label = match period.and_then(|p| p.get("type")).and_then(|v| v.as_str()) {
        Some("USAGE_PERIOD_TYPE_DAILY") => "Day",
        Some("USAGE_PERIOD_TYPE_WEEKLY") => "Week",
        Some("USAGE_PERIOD_TYPE_MONTHLY") => "Month",
        _ => "Period",
    };
    Some(UsageSnapshot {
        windows: vec![AgentUsageWindow {
            label: label.to_string(),
            used_fraction: (used_percent / 100.0) as f32,
            resets_at: parse_when(
                period
                    .and_then(|p| p.get("end"))
                    .or_else(|| config.get("billingPeriodEnd")),
            ),
        }],
        plan_label: None,
    })
}

/// Who a Devin key belongs to, from its status reply.
#[derive(Debug, Default, PartialEq)]
pub(super) struct DevinIdentity {
    pub(super) email: Option<String>,
    pub(super) name: Option<String>,
}

/// Devin's `GetUserStatus`: daily/weekly quota (reported as REMAINING
/// percent; meters show used), the plan, and the identity.
pub(super) fn devin_usage_snapshot(
    body: &serde_json::Value,
) -> Option<(UsageSnapshot, DevinIdentity)> {
    let status = body.get("userStatus")?;
    let identity = DevinIdentity {
        email: str_field(status, "email"),
        name: str_field(status, "name").or_else(|| str_field(status, "displayName")),
    };
    let plan = status.get("planStatus");
    let mut windows = Vec::new();
    if let Some(plan) = plan {
        for (percent_key, reset_key, label) in [
            ("dailyQuotaRemainingPercent", "dailyQuotaResetAtUnix", "Day"),
            (
                "weeklyQuotaRemainingPercent",
                "weeklyQuotaResetAtUnix",
                "Week",
            ),
        ] {
            if let Some(remaining) = plan.get(percent_key).and_then(json_f64) {
                windows.push(AgentUsageWindow {
                    label: label.to_string(),
                    used_fraction: ((100.0 - remaining) / 100.0).clamp(0.0, 1.0) as f32,
                    resets_at: json_secs(plan.get(reset_key)),
                });
            }
        }
    }
    let info = plan.and_then(|p| p.get("planInfo"));
    let plan_label = devin_plan_label(
        info.and_then(|i| str_field(i, "teamsTier")).as_deref(),
        info.and_then(|i| str_field(i, "planName")).as_deref(),
    );
    // Identity alone is still worth keeping (the card gets its email).
    (identity.email.is_some() || !windows.is_empty()).then_some((
        UsageSnapshot {
            windows,
            plan_label,
        },
        identity,
    ))
}

/// "TEAMS_TIER_DEVIN_PRO" → "Devin Pro"; else the plan's own name.
pub(super) fn devin_plan_label(tier: Option<&str>, plan_name: Option<&str>) -> Option<String> {
    if let Some(rest) = tier.and_then(|t| t.strip_prefix("TEAMS_TIER_"))
        && !matches!(rest, "" | "UNSPECIFIED")
    {
        let words: Vec<String> = rest
            .split('_')
            .filter(|w| !w.is_empty())
            .map(|w| {
                let mut chars = w.chars();
                match chars.next() {
                    Some(first) => format!("{}{}", first, chars.as_str().to_lowercase()),
                    None => String::new(),
                }
            })
            .collect();
        let label = words.join(" ");
        return Some(if label.starts_with("Devin") {
            label
        } else {
            format!("Devin {label}")
        });
    }
    plan_name.map(|name| {
        if name.starts_with("Devin") {
            name.to_string()
        } else {
            format!("Devin {name}")
        }
    })
}

/// GitHub's `copilot_internal/user`: each metered quota snapshot (premium
/// requests on paid plans; chat/completions on Free) as a window. An
/// all-unlimited plan has no meters but still reports its plan.
pub(super) fn copilot_usage_snapshot(body: &serde_json::Value) -> Option<UsageSnapshot> {
    let resets_at = parse_when(body.get("quota_reset_date_utc"))
        .or_else(|| day_start(body.get("quota_reset_date")))
        .or_else(|| day_start(body.get("limited_user_reset_date")));
    let mut windows = Vec::new();
    if let Some(snapshots) = body.get("quota_snapshots").and_then(|s| s.as_object()) {
        for (key, label) in [
            ("premium_interactions", "Premium"),
            ("chat", "Chat"),
            ("completions", "Completions"),
        ] {
            let Some(snapshot) = snapshots.get(key) else {
                continue;
            };
            if snapshot.get("unlimited").and_then(|v| v.as_bool()) == Some(true) {
                continue;
            }
            let used = match snapshot.get("percent_remaining").and_then(json_f64) {
                Some(remaining) => (100.0 - remaining) / 100.0,
                None => {
                    let entitlement = snapshot.get("entitlement").and_then(json_f64)?;
                    let remaining = snapshot.get("remaining").and_then(json_f64)?;
                    if entitlement <= 0.0 {
                        continue;
                    }
                    1.0 - remaining / entitlement
                }
            };
            windows.push(AgentUsageWindow {
                label: label.to_string(),
                used_fraction: used.clamp(0.0, 1.0) as f32,
                resets_at,
            });
        }
    } else if let (Some(monthly), Some(left)) = (
        body.get("monthly_quotas").and_then(|m| m.as_object()),
        body.get("limited_user_quotas").and_then(|m| m.as_object()),
    ) {
        for (key, label) in [("chat", "Chat"), ("completions", "Completions")] {
            if let (Some(total), Some(remaining)) = (
                monthly.get(key).and_then(json_f64),
                left.get(key).and_then(json_f64),
            ) && total > 0.0
            {
                windows.push(AgentUsageWindow {
                    label: label.to_string(),
                    used_fraction: (1.0 - remaining / total).clamp(0.0, 1.0) as f32,
                    resets_at,
                });
            }
        }
    }
    let plan_label = str_field(body, "copilot_plan").map(|plan| {
        let name = match plan.as_str() {
            "individual" => "Pro".to_string(),
            "individual_pro" => "Pro+".to_string(),
            other => other
                .split('_')
                .map(|w| {
                    let mut c = w.chars();
                    c.next()
                        .map(|f| format!("{}{}", f.to_uppercase(), c.as_str()))
                        .unwrap_or_default()
                })
                .collect::<Vec<_>>()
                .join(" "),
        };
        format!("Copilot {name}")
    });
    (!windows.is_empty() || plan_label.is_some()).then_some(UsageSnapshot {
        windows,
        plan_label,
    })
}

/// The Nous portal's account: the subscription's monthly credits.
pub(super) fn nous_usage_snapshot(body: &serde_json::Value) -> Option<UsageSnapshot> {
    let subscription = body.get("subscription").filter(|s| s.is_object());
    let plan_label = subscription.and_then(|s| str_field(s, "plan")).map(|plan| {
        let mut chars = plan.chars();
        let plan = chars
            .next()
            .map(|f| format!("{}{}", f.to_uppercase(), chars.as_str()))
            .unwrap_or_default();
        format!("Nous {plan}")
    });
    let mut windows = Vec::new();
    if let Some(sub) = subscription
        && let Some(monthly) = sub.get("monthly_credits").and_then(json_f64)
        && let Some(remaining) = sub.get("credits_remaining").and_then(json_f64)
        && monthly > 0.0
    {
        windows.push(AgentUsageWindow {
            label: "Month".to_string(),
            used_fraction: (1.0 - remaining / monthly).clamp(0.0, 1.0) as f32,
            resets_at: parse_when(sub.get("current_period_end")),
        });
    }
    (body.get("user").is_some() || subscription.is_some()).then_some(UsageSnapshot {
        windows,
        plan_label,
    })
}

/// A number arriving as JSON number or numeric string (proto3 int64s).
fn json_f64(value: &serde_json::Value) -> Option<f64> {
    match value {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

/// Unix seconds as a number or proto3 int64 string.
fn json_secs(value: Option<&serde_json::Value>) -> Option<DateTime<Utc>> {
    DateTime::<Utc>::from_timestamp(json_f64(value?)? as i64, 0)
}

/// A bare `YYYY-MM-DD` as that day's UTC midnight.
fn day_start(value: Option<&serde_json::Value>) -> Option<DateTime<Utc>> {
    let day = chrono::NaiveDate::parse_from_str(value?.as_str()?, "%Y-%m-%d").ok()?;
    Some(day.and_hms_opt(0, 0, 0)?.and_utc())
}
