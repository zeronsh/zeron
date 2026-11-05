//! Cloud and Billing for the scripts, as the shipping app's Settings →
//! Cloud, Settings → Billing and new-chat picker read them (crates/ui/src/
//! cloud.rs, settings/cloud.rs, settings/billing.rs, pickers.rs). Every call
//! is the engine's: it talks to the edge with the account's bearer, so no
//! token reaches here, let alone the scripts.
//!
//! Each is a promise; a failure rejects with an outcome word as its
//! `message` (offline, signed_out, outdated, failed).
//!
//!   cloud.status                 the account's Cloud, set up first if it
//!                                never was (as the shipping app does: no
//!                                machine starts, nothing is billed, until a
//!                                chat runs there):
//!                                { state, available, device?, credits?,
//!                                  error?, failed_action?, awake, max_awake }
//!   cloud.usage({ from, to })    credits used per UTC day, `YYYY-MM-DD` each:
//!                                { days = { { day, credits } }, credits,
//!                                  balance, available }
//!   cloud.repos                  the repositories Cloud can open, newest push
//!                                first: { { name, private, branch, clone_url,
//!                                pushed } }; `github_not_connected` without
//!                                a GitHub connection for Cloud
//!   cloud.branches({ repo, default? })
//!                                the branches a Cloud chat of `repo` can start
//!                                from, the default first, as `chat.refs`
//!                                lists a folder's: { { name, current } }
//!   billing.status               { available, plan?, credits, plans, pack,
//!                                  has_customer } (snake_case, as below)
//!   billing.checkout({ kind = "subscription", plan } | { kind = "credits",
//!                    packs })    Stripe's page for it, opened in the browser
//!   billing.portal               Stripe's page for the plan, opened

use serde_json::{Value as Json, json};
use zgx_desktop::WindowContext;
use zgx_script::{ScriptHost, Value};

use crate::auth::text;
use crate::engine;

/// Most packs one purchase buys (the edge's cap).
const MAX_PACKS: u64 = 20;

pub fn install(_cx: &mut WindowContext, host: &ScriptHost) {
    host.async_action("cloud.status", |_| async { status(true).await });
    host.async_action("cloud.usage", |args| {
        let day = |key: &str| args.field(key).map(text).filter(|d| is_day(d));
        let range = day("from").zip(day("to"));
        async move {
            let (from, to) = range.ok_or("invalid")?;
            let usage = call("CloudUsage", json!({ "from": from, "to": to })).await?;
            Ok(usage_value(&usage))
        }
    });
    host.async_action("cloud.repos", |_| async {
        let listed = call("ListGithubRepos", json!({})).await?;
        Ok(repos_value(&listed))
    });
    host.async_action("cloud.branches", |args| {
        let repo = args.field("repo").map(text).filter(|r| r.contains('/'));
        let default = args.field("default").map(text).filter(|b| !b.is_empty());
        async move {
            let repo = repo.ok_or("invalid")?;
            let mut params = json!({ "repo": repo });
            if let Some(default) = &default {
                params["defaultBranch"] = Json::from(default.as_str());
            }
            let found = call("ListCloudBranches", params).await?;
            Ok(branches_value(&found, default.as_deref()))
        }
    });
    host.async_action("billing.status", |_| async {
        let status = call("BillingStatus", json!({})).await?;
        Ok(billing_value(&status))
    });
    host.async_action("billing.checkout", |args| {
        let kind = args.field("kind").map(text).unwrap_or_default();
        let params = match kind.as_str() {
            "subscription" => args
                .field("plan")
                .map(text)
                .filter(|p| !p.is_empty())
                .map(|plan| json!({ "kind": "subscription", "plan": plan })),
            "credits" => {
                let packs = args.field("packs").and_then(Value::as_f64).unwrap_or(1.0);
                let packs = (packs.max(1.0) as u64).min(MAX_PACKS);
                Some(json!({ "kind": "credits", "packs": packs }))
            }
            _ => None,
        };
        async move { crate::plugins::preview::refuse("billing.checkout")?; open_link("BillingCheckout", params.ok_or("invalid")?).await }
    });
    host.async_action("billing.portal", |_| async { open_link("BillingPortal", json!({})).await });
}

/// An engine call, its failure said as an outcome word.
async fn call(method: &str, params: Json) -> Result<Json, String> {
    engine::get().call(method, params).await.map_err(|error| {
        eprintln!("zeron-zgx: cloud: {method} failed: {error}");
        if error.to_ascii_lowercase().contains("unknown method") {
            "outdated".to_string()
        } else if error.contains("github_not_connected") {
            // (`ListGithubRepos` without a GitHub connection for Cloud.)
            "github_not_connected".to_string()
        } else {
            engine::outcome(&error).to_string()
        }
    })
}

/// Cloud's status; one never set up is set up first (`set_up`), as every
/// signed-in account has Cloud.
async fn status(set_up: bool) -> Result<Value, String> {
    let mut status = call("CloudStatus", json!({})).await?;
    if set_up && status["available"].as_bool() == Some(true) && status["state"].as_str() == Some("off") {
        eprintln!("zeron-zgx: cloud: setting up Cloud for the account");
        if let Ok(on) = call("CloudEnable", json!({})).await {
            status = on;
        }
    }
    Ok(status_value(&status))
}

/// A Stripe page from the engine, opened in the browser.
async fn open_link(method: &str, params: Json) -> Result<Value, String> {
    let link = call(method, params).await?;
    let url = link["url"].as_str().ok_or("failed")?;
    if !url.starts_with("https://") {
        return Err("failed".into());
    }
    crate::github::open_page(url);
    Ok(Value::Bool(true))
}

fn is_day(day: &str) -> bool {
    let b = day.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter().enumerate().all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

fn num(v: &Json) -> Value {
    v.as_f64().map_or(Value::Nil, Value::from)
}

fn str_or_nil(v: &Json) -> Value {
    v.as_str().filter(|s| !s.is_empty()).map_or(Value::Nil, Value::from)
}

fn status_value(s: &Json) -> Value {
    Value::record([
        ("state", Value::from(s["state"].as_str().unwrap_or("off"))),
        ("available", Value::Bool(s["available"].as_bool() == Some(true))),
        ("device", str_or_nil(&s["deviceId"])),
        ("credits", num(&s["credits"])),
        ("error", str_or_nil(&s["error"])),
        ("failed_action", str_or_nil(&s["failedAction"])),
        ("awake", num(&s["awakeSessions"])),
        ("max_awake", num(&s["maxAwakeSessions"])),
    ])
}

fn usage_value(u: &Json) -> Value {
    let days: Vec<Value> = u["days"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|d| {
            Some(Value::record([
                ("day", Value::from(d["day"].as_str()?)),
                ("credits", Value::from(d["credits"].as_f64().unwrap_or(0.0))),
            ]))
        })
        .collect();
    Value::record([
        ("days", Value::List(days.into())),
        ("credits", Value::from(u["credits"].as_f64().unwrap_or(0.0))),
        ("balance", Value::from(u["balance"].as_f64().unwrap_or(0.0))),
        ("available", Value::Bool(u["available"].as_bool() == Some(true))),
    ])
}

fn repos_value(listed: &Json) -> Value {
    let mut repos: Vec<&Json> = listed["repos"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| r["fullName"].is_string())
        .collect();
    repos.sort_by(|a, b| {
        let pushed = |r: &Json| r["pushedAt"].as_i64().unwrap_or(0);
        let name = |r: &Json| r["fullName"].as_str().unwrap_or_default().to_ascii_lowercase();
        pushed(b).cmp(&pushed(a)).then_with(|| name(a).cmp(&name(b)))
    });
    Value::List(
        repos
            .into_iter()
            .map(|r| {
                Value::record([
                    ("name", Value::from(r["fullName"].as_str().unwrap_or_default())),
                    ("private", Value::Bool(r["private"].as_bool() == Some(true))),
                    ("branch", str_or_nil(&r["defaultBranch"])),
                    ("clone_url", str_or_nil(&r["cloneUrl"])),
                    // (Seconds, as the scripts' times are.)
                    ("pushed", Value::from(r["pushedAt"].as_f64().unwrap_or(0.0) / 1000.0)),
                ])
            })
            .collect::<Vec<_>>()
            .into(),
    )
}

/// A Cloud repository's branches: the default first (as `current`, the
/// one a chat starts from unless another is chosen), then as GitHub lists
/// them.
fn branches_value(found: &Json, default: Option<&str>) -> Value {
    let mut names: Vec<&str> = found.as_array().into_iter().flatten().filter_map(|r| r["name"].as_str()).collect();
    if let Some(default) = default
        && let Some(at) = names.iter().position(|n| *n == default)
    {
        let d = names.remove(at);
        names.insert(0, d);
    }
    let first = names.first().copied();
    Value::List(
        names
            .into_iter()
            .map(|name| {
                let current = default.map_or(Some(name) == first, |d| d == name);
                Value::record([("name", Value::from(name)), ("current", Value::Bool(current))])
            })
            .collect::<Vec<_>>()
            .into(),
    )
}

fn plan_value(p: &Json) -> Value {
    Value::record([
        ("id", Value::from(p["id"].as_str().unwrap_or_default())),
        ("name", Value::from(p["name"].as_str().unwrap_or_default())),
        ("price_cents", Value::from(p["priceCents"].as_f64().unwrap_or(0.0))),
        ("monthly_credits", Value::from(p["monthlyCredits"].as_f64().unwrap_or(0.0))),
    ])
}

fn billing_value(b: &Json) -> Value {
    let credits = &b["credits"];
    // (Unix seconds, as the scripts' times are.)
    let secs = |v: &Json| v.as_f64().map_or(Value::Nil, |ms| Value::from(ms / 1000.0));
    let plan = if b["plan"].is_object() {
        let p = &b["plan"];
        Value::record([
            ("id", Value::from(p["id"].as_str().unwrap_or_default())),
            ("name", Value::from(p["name"].as_str().unwrap_or_default())),
            ("price_cents", Value::from(p["priceCents"].as_f64().unwrap_or(0.0))),
            ("monthly_credits", Value::from(p["monthlyCredits"].as_f64().unwrap_or(0.0))),
            ("status", Value::from(p["status"].as_str().unwrap_or("active"))),
            ("period_end", secs(&p["periodEnd"])),
            ("cancel_at_period_end", Value::Bool(p["cancelAtPeriodEnd"].as_bool() == Some(true))),
        ])
    } else {
        Value::Nil
    };
    let plans: Vec<Value> = b["plans"].as_array().into_iter().flatten().map(plan_value).collect();
    Value::record([
        ("available", Value::Bool(b["available"].as_bool() == Some(true))),
        ("plan", plan),
        (
            "credits",
            Value::record([
                ("monthly", Value::from(credits["monthly"].as_f64().unwrap_or(0.0))),
                ("monthly_total", Value::from(credits["monthlyTotal"].as_f64().unwrap_or(0.0))),
                ("monthly_ends", secs(&credits["monthlyEndsAt"])),
                ("extra", Value::from(credits["extra"].as_f64().unwrap_or(0.0))),
                ("total", Value::from(credits["total"].as_f64().unwrap_or(0.0))),
            ]),
        ),
        ("plans", Value::List(plans.into())),
        (
            "pack",
            Value::record([
                ("price_cents", Value::from(b["pack"]["priceCents"].as_f64().unwrap_or(0.0))),
                ("credits", Value::from(b["pack"]["credits"].as_f64().unwrap_or(0.0))),
            ]),
        ),
        ("has_customer", Value::Bool(b["hasCustomer"].as_bool() == Some(true))),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_are_checked() {
        assert!(is_day("2026-11-01"));
        assert!(!is_day("2026-11-1"));
        assert!(!is_day("2026/11/01"));
        assert!(!is_day("x026-11-01"));
    }

    #[test]
    fn branches_put_the_default_first() {
        let found = json!([{ "name": "dev" }, { "name": "main" }, { "name": "fix" }]);
        let listed = branches_value(&found, Some("main")).to_json();
        let listed: Json = serde_json::from_str(&listed).unwrap();
        assert_eq!(listed[0]["name"], "main");
        assert_eq!(listed[0]["current"], true);
        assert_eq!(listed[1]["current"], false);
        let none = branches_value(&json!([{ "name": "a" }, { "name": "b" }]), None).to_json();
        let none: Json = serde_json::from_str(&none).unwrap();
        assert_eq!(none[0]["current"], true);
    }

    #[test]
    fn repos_sort_newest_first() {
        let listed = json!({ "repos": [
            { "fullName": "a/old", "pushedAt": 1000, "private": false },
            { "fullName": "a/new", "pushedAt": 5000, "private": true, "defaultBranch": "main" },
        ] });
        let out: Json = serde_json::from_str(&repos_value(&listed).to_json()).unwrap();
        assert_eq!(out[0]["name"], "a/new");
        assert_eq!(out[0]["private"], true);
        assert_eq!(out[0]["branch"], "main");
        assert_eq!(out[0]["pushed"], 5.0);
    }
}
