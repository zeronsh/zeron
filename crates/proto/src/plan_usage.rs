//! A subscription's plan usage as meters: the replies of Claude's
//! `/api/oauth/usage` and Codex's `/wham/usage` read into windows
//! ("Session", "Week", "Month") and the live plan label. Shared by every
//! reader: the engine probing a CLI login, and Cloud's logins, whose usage
//! only the vault can fetch (the desktop and the phone read its reply). Pure.

use chrono::{DateTime, Utc};

use crate::{AgentUsageWindow, HarnessId};

/// `harness`'s usage reply as windows and the live plan label; `None` when
/// it carries no window.
pub fn from_reply(
    harness: HarnessId,
    body: &serde_json::Value,
) -> Option<(Vec<AgentUsageWindow>, Option<String>)> {
    match harness {
        HarnessId::ClaudeCode => claude(body).map(|windows| (windows, None)),
        HarnessId::Codex => codex(body),
        HarnessId::Antigravity => antigravity(body),
        _ => None,
    }
}

/// Antigravity's usage: Google's `retrieveUserQuotaSummary` (each group of
/// models with its own limits, each limit a `remainingFraction` with a
/// `resetTime`), with the `paidTier` and `currentTier` of `loadCodeAssist`
/// beside its `groups`. One window per limit, named by its group and span
/// ("Gemini Session", "Claude & GPT Week"); the plan is the paid tier's name
/// ("Google AI Ultra"). `None` with no `groups` at all; none in it is a tier
/// with no limits, and no windows.
pub fn antigravity(body: &serde_json::Value) -> Option<(Vec<AgentUsageWindow>, Option<String>)> {
    let groups = body.get("groups")?.as_array()?;
    let mut ranked = Vec::new();
    for group in groups {
        let name = group
            .get("displayName")
            .or_else(|| group.get("display_name"))
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let (group_rank, group_label) = antigravity_group(name);
        for bucket in group.get("buckets").and_then(|b| b.as_array()).into_iter().flatten() {
            if bucket.get("disabled").and_then(|d| d.as_bool()) == Some(true) {
                continue;
            }
            // (A count with no fraction can't be a meter.)
            let Some(remaining) = bucket
                .get("remainingFraction")
                .or_else(|| bucket.get("remaining_fraction"))
                .and_then(|v| v.as_f64())
            else {
                continue;
            };
            let (span_rank, span) = antigravity_span(bucket);
            ranked.push((
                (group_rank, group_label.clone(), span_rank),
                AgentUsageWindow {
                    label: format!("{group_label} {span}"),
                    used_fraction: (1.0 - remaining).clamp(0.0, 1.0) as f32,
                    resets_at: parse_when(bucket.get("resetTime").or_else(|| bucket.get("reset_time"))),
                },
            ));
        }
    }
    // Gemini's first, each group's shortest span first: the card's session
    // and week are its first window and its weekly one.
    ranked.sort_by(|a, b| a.0.cmp(&b.0));
    let plan = ["paidTier", "currentTier"]
        .iter()
        .filter_map(|tier| body.get(*tier))
        .find(|tier| tier.get("id").and_then(|id| id.as_str()).is_some_and(|id| id != "free-tier"))
        .and_then(|tier| tier.get("name")?.as_str())
        .filter(|name| !name.is_empty())
        .map(str::to_owned);
    Some((ranked.into_iter().map(|(_, window)| window).collect(), plan))
}

/// A quota group's rank and label: Gemini's, the Claude and GPT one, or
/// another by its own name ("Imagen models" → "Imagen").
fn antigravity_group(name: &str) -> (u8, String) {
    let lower = name.to_ascii_lowercase();
    if lower.contains("gemini") {
        (0, crate::ANTIGRAVITY_USAGE_GEMINI.to_owned())
    } else if lower.contains("claude") || lower.contains("gpt") {
        (1, crate::ANTIGRAVITY_USAGE_OTHER.to_owned())
    } else {
        let short = name.trim().trim_end_matches(" models").trim_end_matches(" Models").trim();
        (2, if short.is_empty() { "Models".to_owned() } else { short.to_owned() })
    }
}

/// A limit's span: its `window` ("5h", "weekly", …), else its name.
fn antigravity_span(bucket: &serde_json::Value) -> (u8, String) {
    let window = bucket.get("window").and_then(|w| w.as_str()).unwrap_or_default();
    match window.to_ascii_lowercase().as_str() {
        "5h" => (0, "Session".to_owned()),
        "daily" | "day" | "1d" => (1, "Day".to_owned()),
        "weekly" | "week" | "7d" => (2, "Week".to_owned()),
        "monthly" | "month" | "30d" => (3, "Month".to_owned()),
        _ => {
            let name = bucket
                .get("displayName")
                .or_else(|| bucket.get("display_name"))
                .and_then(|n| n.as_str())
                .map(|n| n.trim_end_matches(" Remaining").trim_end_matches(" Limit").trim())
                .filter(|n| !n.is_empty())
                .unwrap_or("Limit");
            (4, name.to_owned())
        }
    }
}

/// Claude's `/api/oauth/usage`: the 5-hour session and weekly buckets, each
/// a 0-100 `utilization` with an RFC3339 `resets_at`.
pub fn claude(body: &serde_json::Value) -> Option<Vec<AgentUsageWindow>> {
    let mut windows = Vec::new();
    for (key, label) in [("five_hour", "Session"), ("seven_day", "Week")] {
        if let Some(w) = body.get(key)
            && let Some(utilization) = w.get("utilization").and_then(|v| v.as_f64())
        {
            windows.push(AgentUsageWindow {
                label: label.to_string(),
                used_fraction: (utilization / 100.0) as f32,
                resets_at: parse_when(w.get("resets_at")),
            });
        }
    }
    (!windows.is_empty()).then_some(windows)
}

/// Codex `/wham/usage`: primary/secondary windows + the live plan ("free"/
/// "plus"/"pro"…), which beats the login-time JWT claim.
pub fn codex(body: &serde_json::Value) -> Option<(Vec<AgentUsageWindow>, Option<String>)> {
    let rl = body.get("rate_limit")?;
    let mut windows = Vec::new();
    for key in ["primary_window", "secondary_window"] {
        if let Some(w) = rl.get(key)
            && let Some(used) = w.get("used_percent").and_then(|v| v.as_f64())
        {
            let span = w
                .get("limit_window_seconds")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            windows.push(AgentUsageWindow {
                label: codex_window_label(span).to_string(),
                used_fraction: (used / 100.0) as f32,
                resets_at: parse_when(w.get("reset_at")),
            });
        }
    }
    if windows.is_empty() {
        return None;
    }
    let plan = body
        .get("plan_type")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());
    Some((windows, codex_plan(plan)))
}

/// "ChatGPT Plus" from Codex's `plan_type`.
pub fn codex_plan(plan: Option<&str>) -> Option<String> {
    let plan = plan?;
    let mut chars = plan.chars();
    let first = chars.next()?;
    Some(format!(
        "ChatGPT {}{}",
        first.to_uppercase(),
        chars.as_str()
    ))
}

/// Meter label for a Codex rate-limit window from its `limit_window_seconds`:
/// the free tier's window is a 30-day month (2_592_000s), Plus runs a 5-hour
/// primary (~18_000s) with a weekly secondary (604_800s). A bare "> 1 day =
/// week" rule mislabeled the monthly window "Week"; thresholds in seconds
/// leave the middle gaps to the nearest label rather than guessing a plan.
pub fn codex_window_label(span_seconds: i64) -> &'static str {
    const DAY: i64 = 86_400;
    if span_seconds >= 28 * DAY {
        "Month"
    } else if span_seconds >= 5 * DAY {
        "Week"
    } else {
        "Session"
    }
}

/// A reset time as Unix seconds or RFC 3339.
pub fn parse_when(value: Option<&serde_json::Value>) -> Option<DateTime<Utc>> {
    match value? {
        serde_json::Value::Number(n) => DateTime::<Utc>::from_timestamp(n.as_i64()?, 0),
        serde_json::Value::String(s) => DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|t| t.with_timezone(&Utc)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Google AI Ultra account's reply (live, `loadCodeAssist`'s tiers
    /// beside it): only the Claude and GPT group is limited, its 5-hour limit
    /// first; the paid tier names the plan.
    #[test]
    fn antigravity_limits_by_group_and_span() {
        let body = serde_json::json!({
            "groups": [{
                "displayName": "Claude and GPT models",
                "description": "Models within this group: Claude Opus, Claude Sonnet, GPT-OSS",
                "buckets": [
                    { "bucketId": "3p-weekly", "displayName": "Weekly Limit Remaining", "window": "weekly",
                      "resetTime": "2026-11-26T21:17:50Z", "remainingFraction": 0.75 },
                    { "bucketId": "3p-5h", "displayName": "Five Hour Limit Remaining", "window": "5h",
                      "resetTime": "2026-11-20T02:17:50Z", "remainingFraction": 1 }
                ]
            }],
            "currentTier": { "id": "free-tier", "name": "Antigravity" },
            "paidTier": { "id": "g1-ultra-tier", "name": "Google AI Ultra" }
        });
        let (windows, plan) = antigravity(&body).unwrap();
        let labels: Vec<&str> = windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, ["Claude & GPT Session", "Claude & GPT Week"]);
        assert_eq!(windows[0].used_fraction, 0.0);
        assert!((windows[1].used_fraction - 0.25).abs() < 1e-6);
        assert_eq!(windows[1].resets_at.unwrap().to_rfc3339(), "2026-11-26T21:17:50+00:00");
        assert_eq!(plan.as_deref(), Some("Google AI Ultra"));
        assert_eq!(from_reply(HarnessId::Antigravity, &body).unwrap().0.len(), 2);
    }

    /// Gemini's group goes first; a disabled limit, or one that's only a
    /// count, isn't a meter; a free account names no plan; no `groups` is no
    /// reply this reads, an empty one a tier with no limits.
    #[test]
    fn antigravity_orders_gemini_first_and_skips_what_isnt_a_meter() {
        let body = serde_json::json!({
            "groups": [
                { "displayName": "Claude and GPT models", "buckets": [{ "window": "weekly", "remainingFraction": 0.5 }] },
                { "displayName": "Gemini Models", "buckets": [
                    { "window": "weekly", "remainingFraction": 0.9 },
                    { "window": "5h", "remainingFraction": 0.2 },
                    { "window": "daily", "disabled": true, "remainingFraction": 0.0 },
                    { "displayName": "Requests", "remainingAmount": "100" }
                ] },
                { "displayName": "Imagen models", "buckets": [{ "displayName": "Daily Limit Remaining", "remainingFraction": 1.4 }] }
            ],
            "currentTier": { "id": "free-tier", "name": "Antigravity" }
        });
        let (windows, plan) = antigravity(&body).unwrap();
        let labels: Vec<&str> = windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, ["Gemini Session", "Gemini Week", "Claude & GPT Week", "Imagen Daily"]);
        assert!((windows[0].used_fraction - 0.8).abs() < 1e-6);
        assert_eq!(windows[3].used_fraction, 0.0, "clamped");
        assert_eq!(plan, None);
        assert_eq!(antigravity(&serde_json::json!({ "error": "x" })), None);
        assert_eq!(antigravity(&serde_json::json!({ "groups": [] })), Some((Vec::new(), None)));
    }
}
