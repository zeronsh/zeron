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
//!                                  error?, failed_action?, awake, max_awake,
//!                                  machines, machine? }; `machines` is empty
//!                                from an edge that predates machine choice,
//!                                each { id, name, vcpu, memory_gb, disk_gb,
//!                                rate } (`rate`: credits relative to standard)
//!   cloud.set_machine({ machine })  the size new chats start on; the status
//!   cloud.sessions               the Cloud sessions, each { chat, device, state,
//!                                space, repo?, error?, failed_action?,
//!                                machine?, stage?, disk?, wake_blocked?,
//!                                expires? }; disk: { used, capacity, level };
//!                                wake_blocked: { reason, needed }; expires in
//!                                Unix seconds, as the scripts' times are. Nil
//!                                from an edge with no sessions route.
//!   cloud.wake({ chat, machine? })  wake a chat's machine (on that size): its
//!                                session. Rejects with `disk_too_small` for a
//!                                size too small, else a word as below or the
//!                                edge's own sentence.
//!   cloud.sleep({ chat })        put it to sleep (what a machine that's up has
//!                                to do to change size): its session.
//!   cloud.retry({ chat, machine? })  on a session whose files expired, a fresh
//!                                machine on its repository and branch: its
//!                                session. Rejects as `cloud.wake` does.
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
    host.async_action("cloud.sessions", |_| async {
        let listed = call("CloudSessions", json!({})).await?;
        Ok(sessions_value(&listed))
    });
    host.async_action("cloud.wake", |args| {
        let chat = args.field("chat").map(text).filter(|c| !c.is_empty());
        let machine = args.field("machine").map(text).filter(|m| !m.is_empty());
        async move {
            crate::plugins::preview::refuse("cloud.wake")?;
            on_machine("CloudSessionWake", chat, machine).await
        }
    });
    host.async_action("cloud.sleep", |args| {
        let chat = args.field("chat").map(text).filter(|c| !c.is_empty());
        async move {
            crate::plugins::preview::refuse("cloud.sleep")?;
            on_machine("CloudSessionSleep", chat, None).await
        }
    });
    host.async_action("cloud.retry", |args| {
        let chat = args.field("chat").map(text).filter(|c| !c.is_empty());
        let machine = args.field("machine").map(text).filter(|m| !m.is_empty());
        async move {
            crate::plugins::preview::refuse("cloud.retry")?;
            on_machine("CloudSessionRetry", chat, machine).await
        }
    });
    host.async_action("cloud.set_machine", |args| {
        let machine = args.field("machine").map(text).filter(|m| !m.is_empty());
        async move {
            crate::plugins::preview::refuse("cloud.set_machine")?;
            let status = call("CloudSetMachine", json!({ "machine": machine.ok_or("invalid")? })).await?;
            Ok(status_value(&status))
        }
    });
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

/// One action on a chat's machine: its session, or why not.
async fn on_machine(method: &str, chat: Option<String>, machine: Option<String>) -> Result<Value, String> {
    let mut params = json!({ "chatId": chat.ok_or("invalid")? });
    if let Some(machine) = machine {
        params["machine"] = Json::from(machine);
    }
    let session = engine::get().call(method, params).await.map_err(|error| wake_failure(&error))?;
    Ok(session_value(&session))
}

/// Why a machine's action didn't happen, for the screen: `disk_too_small` for the typed
/// refusal, a word for what has one, else the edge's own sentence.
fn wake_failure(error: &str) -> String {
    eprintln!("zeron-zgx: cloud: a machine's action failed: {error}");
    if error.contains("disk_too_small") {
        return "disk_too_small".into();
    }
    if error.to_ascii_lowercase().contains("unknown method") {
        return "outdated".into();
    }
    match engine::outcome(error) {
        "failed" => crate::sessions::cloud_words(error),
        word => word.to_string(),
    }
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
        ("machines", machines_value(&s["machines"])),
        ("machine", str_or_nil(&s["machine"])),
    ])
}

fn machines_value(list: &Json) -> Value {
    let machines: Vec<Value> = list
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| m["id"].is_string())
        .map(|m| {
            Value::record([
                ("id", Value::from(m["id"].as_str().unwrap_or_default())),
                ("name", str_or_nil(&m["name"])),
                ("vcpu", Value::from(m["vcpu"].as_f64().unwrap_or(0.0))),
                ("memory_gb", Value::from(m["memoryGB"].as_f64().unwrap_or(0.0))),
                ("disk_gb", Value::from(m["diskGiB"].as_f64().unwrap_or(0.0))),
                ("rate", Value::from(m["creditMultiplier"].as_f64().unwrap_or(1.0))),
            ])
        })
        .collect();
    Value::List(machines.into())
}

fn sessions_value(listed: &Json) -> Value {
    Value::List(listed["sessions"].as_array().into_iter().flatten().map(session_value).collect::<Vec<_>>().into())
}

/// A session's fields, as the edge left them: what an old edge doesn't send is
/// nil here, so the scripts read one shape from either.
fn session_value(s: &Json) -> Value {
    let disk = if s["disk"].is_object() {
        Value::record([
            ("used", Value::from(s["disk"]["usedGiB"].as_f64().unwrap_or(0.0))),
            ("capacity", Value::from(s["disk"]["capacityGiB"].as_f64().unwrap_or(0.0))),
            ("level", Value::from(s["disk"]["level"].as_str().unwrap_or("ok"))),
        ])
    } else {
        Value::Nil
    };
    let blocked = if s["wakeBlocked"].is_object() {
        Value::record([("reason", str_or_nil(&s["wakeBlocked"]["reason"])), ("needed", num(&s["wakeBlocked"]["neededGiB"]))])
    } else {
        Value::Nil
    };
    Value::record([
        ("chat", Value::from(s["chatId"].as_str().unwrap_or_default())),
        ("device", Value::from(s["deviceId"].as_str().unwrap_or_default())),
        ("state", Value::from(s["state"].as_str().unwrap_or("off"))),
        ("space", Value::from(s["spaceId"].as_str().unwrap_or_default())),
        ("repo", str_or_nil(&s["repo"])),
        ("error", str_or_nil(&s["error"])),
        ("failed_action", str_or_nil(&s["failedAction"])),
        ("machine", str_or_nil(&s["machine"])),
        ("stage", str_or_nil(&s["stage"])),
        ("disk", disk),
        ("wake_blocked", blocked),
        // (Seconds, as the scripts' times are.)
        ("expires", s["expiresAt"].as_f64().map_or(Value::Nil, |ms| Value::from(ms / 1000.0))),
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
    fn machines_and_a_session_read_as_the_scripts_expect() {
        let status = json!({ "state": "ready", "available": true, "deviceId": "cloud-1", "machine": "large",
            "machines": [{ "id": "xlarge", "name": "Extra large", "vcpu": 8, "memoryGB": 32, "diskGiB": 400, "creditMultiplier": 5.56 }] });
        let out: Json = serde_json::from_str(&status_value(&status).to_json()).unwrap();
        assert_eq!(out["machine"], "large");
        assert_eq!(out["machines"][0]["disk_gb"], 400.0);
        assert_eq!(out["machines"][0]["memory_gb"], 32.0);
        assert_eq!(out["machines"][0]["rate"], 5.56);

        let session = json!({ "chatId": "c1", "deviceId": "cloud-s1", "state": "sleeping", "repo": "acme/atlas", "machine": "default",
            "disk": { "usedGiB": 41.5, "capacityGiB": 50, "level": "warn" },
            "wakeBlocked": { "reason": "disk_too_small", "neededGiB": 150 },
            "expiresAt": 1_800_000_000_000_i64 });
        let out: Json = serde_json::from_str(&session_value(&session).to_json()).unwrap();
        assert_eq!(out["chat"], "c1");
        assert_eq!(out["disk"]["used"], 41.5);
        assert_eq!(out["disk"]["level"], "warn");
        assert_eq!(out["wake_blocked"]["needed"], 150.0);
        assert_eq!(out["expires"], 1_800_000_000.0);
    }

    #[test]
    fn an_old_edge_s_answers_read_with_nothing_of_the_new_fields() {
        let out: Json = serde_json::from_str(&status_value(&json!({ "state": "ready", "available": true })).to_json()).unwrap();
        assert_eq!(out["machines"].as_array().map(Vec::len), Some(0));
        assert!(out["machine"].is_null());
        let out: Json = serde_json::from_str(&session_value(&json!({ "chatId": "c", "deviceId": "d", "state": "ready" })).to_json()).unwrap();
        assert!(out["disk"].is_null());
        assert!(out["wake_blocked"].is_null());
        assert!(out["expires"].is_null());
        assert!(out["machine"].is_null());
    }

    #[test]
    fn a_refused_wake_says_why() {
        assert_eq!(wake_failure("disk_too_small: That machine holds 20 GB."), "disk_too_small");
        assert_eq!(wake_failure("unknown method CloudSessionWake"), "outdated");
        assert_eq!(wake_failure("All 5 of your Cloud sessions that may run at once are busy."), "All 5 of your Cloud sessions that may run at once are busy.");
    }

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
