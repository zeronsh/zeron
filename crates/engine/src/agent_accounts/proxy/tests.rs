//! Rotation through the real account proxy, against a fake provider on
//! loopback (never the network, never a real login).

use std::collections::HashMap;

use base64::Engine as _;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::super::rotation::AccountsRunEnvironment;
use super::super::{
    AgentAccountsConfig, ProbeEndpoints, SlotProfile, parse_codex_auth, read_json, slot_id_for,
};
use super::*;
use crate::registry::RunEnvironment;

// ── fixtures ────────────────────────────────────────────────────────────────

fn accounts_at(root: &std::path::Path, endpoints: ProbeEndpoints) -> AgentAccounts {
    AgentAccounts::with_endpoints(
        AgentAccountsConfig::isolated(root),
        endpoints,
        Default::default(),
    )
}

/// Every provider endpoint on a closed loopback port, refreshes off.
fn read_only() -> ProbeEndpoints {
    let nowhere = "http://127.0.0.1:9".to_string();
    ProbeEndpoints {
        claude_usage: nowhere.clone(),
        claude_token: nowhere.clone(),
        claude_loopback_token: nowhere.clone(),
        claude_profile: nowhere.clone(),
        codex_usage: nowhere.clone(),
        grok_usage: nowhere.clone(),
        github_api: nowhere.clone(),
        github_login: nowhere.clone(),
        openai_auth: nowhere.clone(),
        nous_portal: nowhere,
        allow_slot_refresh: false,
        ..Default::default()
    }
}

fn jwt(claims: serde_json::Value) -> String {
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::to_vec(&claims).unwrap());
    format!("e30.{payload}.sig")
}

fn codex_auth(user: &str, access: &str, refresh: &str) -> serde_json::Value {
    serde_json::json!({
        "OPENAI_API_KEY": null,
        "tokens": {
            "id_token": jwt(serde_json::json!({
                "email": format!("{user}@example.com"),
                "https://api.openai.com/auth": {
                    "chatgpt_account_id": format!("ws-{user}"),
                    "chatgpt_user_id": user,
                },
            })),
            "access_token": access,
            "refresh_token": refresh,
            "account_id": format!("ws-{user}"),
        },
        "last_refresh": "2026-10-01T00:00:00Z",
    })
}

fn codex_slot(auth: serde_json::Value) -> Slot {
    let detected = parse_codex_auth(auth).unwrap();
    Slot {
        id: slot_id_for(HarnessId::Codex, &detected.account_key),
        harness: HarnessId::Codex,
        account_key: detected.account_key,
        profile: detected.profile,
        credentials: detected.credentials.unwrap(),
        claude_config: None,
        saved_at: now_ms(),
        created_at: None,
        store_key: None,
    }
}

/// The user's real Codex home signed in as `live`, plus a saved `other`.
fn two_codex_accounts(accounts: &AgentAccounts, other_access: &str) -> (Slot, Slot) {
    let live_auth = codex_auth("live", "live-access", "live-refresh");
    let file = accounts.inner.config.codex_auth_file();
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, live_auth.to_string()).unwrap();
    let live = codex_slot(live_auth);
    accounts.write_slot(&live).unwrap();
    let other = codex_slot(codex_auth("other", other_access, "other-refresh"));
    accounts.write_slot(&other).unwrap();
    (live, other)
}

#[derive(Debug, Clone)]
struct Seen {
    path: String,
    headers: HashMap<String, String>,
    body: String,
}

impl Seen {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }
}

type Reply = (u16, Vec<(&'static str, String)>, String);

/// A one-request-per-connection HTTP/1.1 server recording what it got.
async fn fake_provider(
    respond: impl Fn(&Seen) -> Reply + Send + Sync + 'static,
) -> (String, Arc<Mutex<Vec<Seen>>>) {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let respond = Arc::new(respond);
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let log = log.clone();
            let respond = respond.clone();
            tokio::spawn(async move {
                let mut raw = Vec::new();
                let mut buf = [0u8; 8192];
                let head_end = loop {
                    let n = stream.read(&mut buf).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    raw.extend_from_slice(&buf[..n]);
                    if let Some(at) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                        break at + 4;
                    }
                };
                let head = String::from_utf8_lossy(&raw[..head_end]).into_owned();
                let mut lines = head.split("\r\n");
                let path = lines
                    .next()
                    .and_then(|l| l.split(' ').nth(1))
                    .unwrap_or("")
                    .to_string();
                let headers: HashMap<String, String> = lines
                    .filter_map(|l| l.split_once(':'))
                    .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
                    .collect();
                let length: usize = headers
                    .get("content-length")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);
                let mut body = raw[head_end..].to_vec();
                while body.len() < length {
                    let n = stream.read(&mut buf).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    body.extend_from_slice(&buf[..n]);
                }
                let seen = Seen {
                    path,
                    headers,
                    body: String::from_utf8_lossy(&body).into_owned(),
                };
                // Stray local connections carry no credential: not ours.
                if seen.header("authorization").is_some() {
                    lock(&log).push(seen.clone());
                }
                let (status, extra, body) = respond(&seen);
                let mut reply = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\n\
                     content-length: {}\r\nconnection: close\r\n",
                    body.len()
                );
                for (name, value) in extra {
                    reply.push_str(&format!("{name}: {value}\r\n"));
                }
                reply.push_str("\r\n");
                reply.push_str(&body);
                let _ = stream.write_all(reply.as_bytes()).await;
                let _ = stream.shutdown().await;
            });
        }
    });
    (url, seen)
}

fn ok() -> Reply {
    (200, Vec::new(), "{\"ok\":true}".into())
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

/// What a Codex process of `chat` gets: its route and run credential.
async fn codex_route(env: &AccountsRunEnvironment, chat: &str) -> (Option<String>, Option<String>) {
    let mut request: zeron_proto::RunRequest = serde_json::from_str(
        r#"{"prompt":"p","model":null,"reasoning":null,"cwd":".","sandbox":"workspace-write","resume":null}"#,
    )
    .unwrap();
    env.prepare(HarnessId::Codex, Some(chat), &mut request)
        .await
        .unwrap();
    let credential = env.credential(HarnessId::Codex, Some(chat)).await;
    (request.env.get(env::CHATGPT).cloned(), credential)
}

/// One Codex request through `route`, sent the way the CLI sends it: with
/// its own (live) login.
async fn codex_request(route: &str) -> (u16, String) {
    let response = client()
        .post(format!("{route}/backend-api/codex/responses"))
        .header("authorization", "Bearer live-access")
        .header("chatgpt-account-id", "ws-live")
        .header("originator", "codex_cli_rs")
        .body(r#"{"model":"gpt"}"#)
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    (status, response.text().await.unwrap())
}

fn chat_rotation(accounts: &AgentAccounts, chat: &str) -> zeron_proto::AgentAccountChat {
    accounts
        .rotation_view(&[HarnessId::Codex])
        .remove(0)
        .chats
        .into_iter()
        .find(|c| c.chat_id == chat)
        .unwrap()
}

// ── tests ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn proxied_codex_chats_spread_across_accounts() {
    let tmp = tempfile::tempdir().unwrap();
    let accounts = accounts_at(tmp.path(), read_only());
    let (live, other) = two_codex_accounts(&accounts, "other-access");
    let (url, seen) = fake_provider(|seen| match seen.header("authorization") {
        Some("Bearer other-access") => (
            200,
            vec![
                ("x-codex-primary-used-percent", "42".into()),
                ("x-codex-primary-window-minutes", "300".into()),
            ],
            "{\"ok\":true}".into(),
        ),
        _ => ok(),
    })
    .await;
    accounts.set_proxy_upstream(ap::Provider::ChatGpt, url);
    let env = AccountsRunEnvironment(accounts.clone());

    // Off: no route, the CLI talks to ChatGPT directly.
    assert_eq!(codex_route(&env, "chat-0").await, (None, Some(LIVE.into())));

    accounts.set_rotation(HarnessId::Codex, true).await.unwrap();
    let (route_1, credential_1) = codex_route(&env, "chat-1").await;
    let (route_2, credential_2) = codex_route(&env, "chat-2").await;
    let (route_1, route_2) = (route_1.unwrap(), route_2.unwrap());
    assert_ne!(route_1, route_2);
    assert!(route_1.starts_with("http://127.0.0.1:"));
    // The route (and so the process) stays the chat's across restarts.
    assert_eq!(
        codex_route(&env, "chat-1").await,
        (Some(route_1.clone()), credential_1.clone())
    );
    assert!(credential_1.unwrap().starts_with("proxy:chat-1:"));
    assert_ne!(credential_2, Some(LIVE.into()));

    assert_eq!(codex_request(&route_1).await.0, 200);
    assert_eq!(codex_request(&route_2).await.0, 200);
    let seen = lock(&seen).clone();
    assert_eq!(seen.len(), 2);
    // chat-1 got the live account: the CLI's own login goes through.
    assert_eq!(seen[0].header("authorization"), Some("Bearer live-access"));
    assert_eq!(seen[0].header("chatgpt-account-id"), Some("ws-live"));
    // chat-2 got the other account: its credential is swapped in.
    assert_eq!(seen[1].header("authorization"), Some("Bearer other-access"));
    assert_eq!(seen[1].header("chatgpt-account-id"), Some("ws-other"));
    for request in &seen {
        assert_eq!(request.path, "/backend-api/codex/responses");
        assert_eq!(request.header("originator"), Some("codex_cli_rs"));
        assert_eq!(request.body, r#"{"model":"gpt"}"#);
    }
    assert_eq!(chat_rotation(&accounts, "chat-1").account_id, live.id);
    assert_eq!(chat_rotation(&accounts, "chat-2").account_id, other.id);

    // The response's rate-limit headers became the account's usage.
    let usage = lock(&accounts.inner.usage)
        .get(&usage_key(HarnessId::Codex, &other.account_key))
        .cloned()
        .unwrap_or_default();
    let windows = usage.usage.unwrap().windows;
    assert_eq!(windows.len(), 1);
    assert!((windows[0].used_fraction - 0.42).abs() < 1e-6);
    assert_eq!(
        windows[0].label,
        zeron_proto::plan_usage::codex_window_label(300 * 60)
    );
}

#[tokio::test]
async fn a_quota_error_fails_the_proxied_chat_over_and_moves_it() {
    let tmp = tempfile::tempdir().unwrap();
    let accounts = accounts_at(tmp.path(), read_only());
    let (live, other) = two_codex_accounts(&accounts, "other-access");
    let (url, seen) = fake_provider(|seen| {
        match seen.header("authorization") {
        Some("Bearer other-access") => (
            429,
            Vec::new(),
            r#"{"error":{"type":"usage_limit_reached","message":"limit","resets_in_seconds":7200}}"#
                .into(),
        ),
        _ => ok(),
    }
    })
    .await;
    accounts.set_proxy_upstream(ap::Provider::ChatGpt, url);
    accounts.set_rotation(HarnessId::Codex, true).await.unwrap();
    let env = AccountsRunEnvironment(accounts.clone());
    // chat-1 gets the live account at its run start, chat-2 the other.
    let _ = codex_route(&env, "chat-1").await;
    let (route, credential) = codex_route(&env, "chat-2").await;
    let route = route.unwrap();
    assert_eq!(chat_rotation(&accounts, "chat-2").account_id, other.id);

    // The other account is out: the same request goes out again on the
    // live login, and the CLI sees a normal answer.
    let (status, body) = codex_request(&route).await;
    assert_eq!((status, body.as_str()), (200, "{\"ok\":true}"));
    let auths: Vec<_> = lock(&seen)
        .iter()
        .map(|s| s.header("authorization").unwrap().to_string())
        .collect();
    assert_eq!(auths, ["Bearer other-access", "Bearer live-access"]);

    // The chat moved for good, and says why.
    let chat = chat_rotation(&accounts, "chat-2");
    assert_eq!(chat.account_id, live.id);
    assert_eq!(chat.previous_account_id.as_deref(), Some(other.id.as_str()));
    assert_eq!(
        chat.moved_reason.as_deref(),
        Some("Usage limit on other@example.com, moved to live@example.com")
    );
    let view = accounts.rotation_view(&[HarnessId::Codex]).remove(0);
    assert_eq!(view.skipped.len(), 1);
    assert_eq!(view.skipped[0].account_id, other.id);
    assert!(
        view.skipped[0]
            .until
            .is_some_and(|until| until > now_ms() + 3_600_000)
    );
    // Its next request goes straight to the new account, and its process
    // never needed replacing.
    codex_request(&route).await;
    assert_eq!(lock(&seen).len(), 3);
    assert_eq!(
        lock(&seen)[2].header("authorization"),
        Some("Bearer live-access")
    );
    assert_eq!(codex_route(&env, "chat-2").await.1, credential);
}

#[tokio::test]
async fn turning_rotation_off_passes_the_live_login_through() {
    let tmp = tempfile::tempdir().unwrap();
    let accounts = accounts_at(tmp.path(), read_only());
    let (_live, _other) = two_codex_accounts(&accounts, "other-access");
    let (url, seen) = fake_provider(|_| ok()).await;
    accounts.set_proxy_upstream(ap::Provider::ChatGpt, url);
    accounts.set_rotation(HarnessId::Codex, true).await.unwrap();
    let env = AccountsRunEnvironment(accounts.clone());
    let _ = codex_route(&env, "chat-1").await;
    let route = codex_route(&env, "chat-2").await.0.unwrap();

    accounts
        .set_rotation(HarnessId::Codex, false)
        .await
        .unwrap();
    // A process still holding its route sends the CLI's own login as is.
    assert_eq!(codex_request(&route).await.0, 200);
    let last = lock(&seen).last().cloned().unwrap();
    assert_eq!(last.header("authorization"), Some("Bearer live-access"));
    assert_eq!(last.header("chatgpt-account-id"), Some("ws-live"));
    // And the chat's next process runs without the proxy.
    assert_eq!(codex_route(&env, "chat-2").await, (None, Some(LIVE.into())));
    assert!(
        accounts.rotation_view(&[HarnessId::Codex])[0]
            .chats
            .is_empty()
    );

    // An archived chat's route is gone.
    accounts.set_rotation(HarnessId::Codex, true).await.unwrap();
    let route = codex_route(&env, "chat-3").await.0.unwrap();
    accounts.release_chat_routes("chat-3");
    assert_eq!(codex_request(&route).await.0, 404);
}

#[tokio::test]
async fn an_expiring_saved_chatgpt_login_is_refreshed_into_its_slot() {
    let tmp = tempfile::tempdir().unwrap();
    // The token endpoint: a fresh token set for the saved login.
    let new_access = jwt(serde_json::json!({"exp": now_ms() / 1000 + 86_400}));
    let reply_access = new_access.clone();
    let token_requests = Arc::new(Mutex::new(Vec::new()));
    let log = token_requests.clone();
    let (auth_url, _) = fake_provider(move |seen| {
        if seen.path == "/oauth/token" {
            lock(&log).push((seen.path.clone(), seen.body.clone()));
        }
        (
            200,
            Vec::new(),
            serde_json::json!({
                "access_token": reply_access,
                "refresh_token": "other-refresh-2",
                "id_token": jwt(serde_json::json!({
                    "email": "other@example.com",
                    "https://api.openai.com/auth": {"chatgpt_account_id": "ws-other"},
                })),
            })
            .to_string(),
        )
    })
    .await;
    // Refreshing is on; the live login is still never refreshed here.
    let accounts = accounts_at(
        tmp.path(),
        ProbeEndpoints {
            openai_auth: auth_url,
            allow_slot_refresh: true,
            ..read_only()
        },
    );
    let expiring = jwt(serde_json::json!({"exp": now_ms() / 1000 + 60}));
    let (_live, other) = two_codex_accounts(&accounts, &expiring);
    let (url, seen) = fake_provider(|_| ok()).await;
    accounts.set_proxy_upstream(ap::Provider::ChatGpt, url);
    accounts.set_rotation(HarnessId::Codex, true).await.unwrap();
    let env = AccountsRunEnvironment(accounts.clone());
    let _ = codex_route(&env, "chat-1").await;
    let route = codex_route(&env, "chat-2").await.0.unwrap();

    assert_eq!(codex_request(&route).await.0, 200);
    let calls = lock(&token_requests).clone();
    assert_eq!(calls.len(), 1, "one refresh");
    assert_eq!(calls[0].0, "/oauth/token");
    let refresh: serde_json::Value = serde_json::from_str(&calls[0].1).unwrap();
    assert_eq!(refresh["grant_type"], "refresh_token");
    assert_eq!(refresh["refresh_token"], "other-refresh");
    assert_eq!(refresh["client_id"], super::super::oauth::OPENAI_CLIENT_ID);
    let sent = lock(&seen).last().cloned().unwrap();
    assert_eq!(
        sent.header("authorization"),
        Some(format!("Bearer {new_access}").as_str())
    );
    let slot = accounts.read_slot(HarnessId::Codex, &other.id).unwrap();
    assert_eq!(slot.credentials["tokens"]["access_token"], new_access);
    assert_eq!(
        slot.credentials["tokens"]["refresh_token"],
        "other-refresh-2"
    );
    assert_ne!(slot.credentials["last_refresh"], "2026-10-01T00:00:00Z");
    // The live store is untouched.
    let live = read_json(&accounts.inner.config.codex_auth_file()).unwrap();
    assert_eq!(live["tokens"]["refresh_token"], "live-refresh");
}

fn pi_slot(store_key: &str, user: &str) -> Slot {
    let account_key = format!("{store_key}:{user}");
    Slot {
        id: slot_id_for(HarnessId::Pi, &account_key),
        harness: HarnessId::Pi,
        account_key,
        profile: SlotProfile {
            email: format!("{user}@example.com"),
            display_name: None,
            organization: None,
            plan: None,
            auth_kind: zeron_proto::AgentAuthKind::Oauth,
        },
        credentials: serde_json::json!({
            "type": "oauth",
            "access": format!("{store_key}-{user}"),
            "refresh": "r",
            "expires": now_ms() + 86_400_000,
            "accountId": format!("ws-{user}"),
        }),
        claude_config: None,
        saved_at: now_ms(),
        created_at: None,
        store_key: Some(store_key.into()),
    }
}

#[tokio::test]
async fn pi_rotates_each_provider_pool_on_its_own() {
    let tmp = tempfile::tempdir().unwrap();
    let accounts = accounts_at(tmp.path(), read_only());
    for (key, user) in [
        ("anthropic", "a1"),
        ("openai-codex", "o1"),
        ("anthropic", "a2"),
        ("openai-codex", "o2"),
        ("github-copilot", "g1"),
    ] {
        accounts.write_slot(&pi_slot(key, user)).unwrap();
    }
    accounts.set_rotation(HarnessId::Pi, true).await.unwrap();
    let lease = async |chat: &str, pool: &str| match accounts
        .proxy_lease(chat, HarnessId::Pi, Some(pool), &[], None)
        .await
    {
        ProxyLease::Account { headers, .. } => headers,
        other => panic!("{other:?}"),
    };
    // Each pool round-robins on its own cursor.
    let claude = lease("c1", "anthropic").await;
    assert_eq!(claude["authorization"], "Bearer anthropic-a1");
    assert!(!claude.contains_key("chatgpt-account-id"));
    let chatgpt = lease("c2", "openai-codex").await;
    assert_eq!(chatgpt["authorization"], "Bearer openai-codex-o1");
    assert_eq!(chatgpt["chatgpt-account-id"], "ws-o1");
    assert_eq!(
        lease("c3", "anthropic").await["authorization"],
        "Bearer anthropic-a2"
    );
    assert_eq!(
        lease("c4", "openai-codex").await["authorization"],
        "Bearer openai-codex-o2"
    );
    // The rotation lists the proxied pools' logins only (no Copilot).
    let view = accounts.rotation_view(&[HarnessId::Pi]).remove(0);
    assert!(view.supported && view.active);
    assert_eq!(view.accounts.len(), 4);
}

#[test]
fn rate_limit_headers_map_onto_the_probes_meters() {
    let limits = ap::RateLimitSnapshot {
        windows: vec![
            ap::RateLimitWindow {
                name: "five_hour".into(),
                used_percent: Some(25.0),
                ..Default::default()
            },
            ap::RateLimitWindow {
                name: "secondary".into(),
                used_percent: Some(80.0),
                window_minutes: Some(7 * 24 * 60),
                ..Default::default()
            },
            // No usage: no meter.
            ap::RateLimitWindow {
                name: "seven_day".into(),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let windows = usage_windows(&limits);
    assert_eq!(
        windows
            .iter()
            .map(|w| (w.label.as_str(), w.used_fraction))
            .collect::<Vec<_>>(),
        vec![
            ("Session", 0.25),
            (
                zeron_proto::plan_usage::codex_window_label(7 * 24 * 3600),
                0.8
            )
        ]
    );
}

// ── review fixes ────────────────────────────────────────────────────────────

/// A request as a CLI holding `(bearer, workspace)` sends it.
async fn request_as(route: &str, bearer: &str, workspace: &str, body: &str) -> (u16, String) {
    let response = client()
        .post(format!("{route}/backend-api/codex/responses"))
        .header("authorization", format!("Bearer {bearer}"))
        .header("chatgpt-account-id", workspace)
        .body(body.to_string())
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    (status, response.text().await.unwrap())
}

#[tokio::test]
async fn only_subscription_logins_without_an_endpoint_of_their_own_are_routed() {
    let tmp = tempfile::tempdir().unwrap();
    let accounts = accounts_at(tmp.path(), read_only());
    let (live, other) = two_codex_accounts(&accounts, "other-access");
    accounts.set_rotation(HarnessId::Codex, true).await.unwrap();
    let env = AccountsRunEnvironment(accounts.clone());
    let (route, routed) = codex_route(&env, "chat-1").await;
    assert!(route.is_some());

    // A gateway of the user's own: Zeron leaves Codex alone.
    let config = accounts.inner.config.codex_home.join("config.toml");
    std::fs::write(
        &config,
        "openai_base_url = \"https://gateway.example/v1\"\n",
    )
    .unwrap();
    assert_eq!(codex_route(&env, "chat-1").await, (None, Some(LIVE.into())));
    std::fs::remove_file(&config).unwrap();
    assert_eq!(
        codex_route(&env, "chat-1").await,
        (route.clone(), routed.clone())
    );

    // An API-key live login is no subscription to rotate around.
    let file = accounts.inner.config.codex_auth_file();
    let subscription = std::fs::read_to_string(&file).unwrap();
    std::fs::write(&file, r#"{"OPENAI_API_KEY":"sk-test","tokens":null}"#).unwrap();
    assert_eq!(codex_route(&env, "chat-1").await, (None, Some(LIVE.into())));
    std::fs::write(&file, &subscription).unwrap();

    // One usable account left: rotation is out of effect, and the process
    // is replaced (its credential flips).
    accounts.report_rejected(HarnessId::Codex, &other.id);
    assert_eq!(codex_route(&env, "chat-1").await, (None, Some(LIVE.into())));
    let mut relogged = accounts.read_slot(HarnessId::Codex, &other.id).unwrap();
    relogged.credentials["tokens"]["access_token"] = "other-access-2".into();
    accounts.write_slot(&relogged).unwrap();
    assert_eq!(codex_route(&env, "chat-1").await.1, routed);
    let _ = live;
}

#[tokio::test]
async fn a_live_switch_replaces_the_process_and_a_stale_cli_gets_the_chats_account() {
    let tmp = tempfile::tempdir().unwrap();
    let accounts = accounts_at(tmp.path(), read_only());
    let (_live, other) = two_codex_accounts(&accounts, "other-access");
    let (url, seen) = fake_provider(|_| ok()).await;
    accounts.set_proxy_upstream(ap::Provider::ChatGpt, url);
    accounts.set_rotation(HarnessId::Codex, true).await.unwrap();
    let env = AccountsRunEnvironment(accounts.clone());
    let _ = codex_route(&env, "chat-1").await;
    let (route, before) = codex_route(&env, "chat-2").await;
    let route = route.unwrap();
    assert_eq!(chat_rotation(&accounts, "chat-2").account_id, other.id);

    // The user makes `other` the live login.
    std::fs::write(
        accounts.inner.config.codex_auth_file(),
        codex_auth("other", "other-access", "other-refresh").to_string(),
    )
    .unwrap();
    let (_, after) = codex_route(&env, "chat-2").await;
    assert_ne!(before, after, "the parked process is replaced");

    // A process that still holds the old login: its request carries the
    // chat's account, not the CLI's stale one.
    assert_eq!(
        request_as(&route, "live-access", "ws-live", "{}").await.0,
        200
    );
    let last = lock(&seen).last().cloned().unwrap();
    assert_eq!(last.header("authorization"), Some("Bearer other-access"));
    assert_eq!(last.header("chatgpt-account-id"), Some("ws-other"));
    // The replaced process holds `other` itself: passed through as is.
    assert_eq!(request_as(&route, "cli-own", "ws-other", "{}").await.0, 200);
    let last = lock(&seen).last().cloned().unwrap();
    assert_eq!(last.header("authorization"), Some("Bearer cli-own"));
}

#[tokio::test]
async fn a_login_seen_live_is_never_refreshed_by_the_engine() {
    let tmp = tempfile::tempdir().unwrap();
    let token_requests = Arc::new(Mutex::new(0usize));
    let count = token_requests.clone();
    let (auth_url, _) = fake_provider(move |seen| {
        if seen.path == "/oauth/token" {
            *lock(&count) += 1;
        }
        (500, Vec::new(), "{}".into())
    })
    .await;
    let accounts = accounts_at(
        tmp.path(),
        ProbeEndpoints {
            openai_auth: auth_url,
            allow_slot_refresh: true,
            ..read_only()
        },
    );
    let expiring = jwt(serde_json::json!({"exp": now_ms() / 1000 + 60}));
    let (_live, other) = two_codex_accounts(&accounts, &expiring);
    let (url, seen) = fake_provider(|_| ok()).await;
    accounts.set_proxy_upstream(ap::Provider::ChatGpt, url);
    accounts.set_rotation(HarnessId::Codex, true).await.unwrap();
    let env = AccountsRunEnvironment(accounts.clone());
    // `other` was live for a while (a process may still hold its refresh
    // token), then the user switched back.
    let file = accounts.inner.config.codex_auth_file();
    let live_auth = std::fs::read_to_string(&file).unwrap();
    std::fs::write(&file, other.credentials.to_string()).unwrap();
    let _ = codex_route(&env, "chat-0").await;
    std::fs::write(&file, &live_auth).unwrap();
    // chat-0 took the first account; chat-2 gets `other`.
    let route = codex_route(&env, "chat-2").await.0.unwrap();
    assert_eq!(chat_rotation(&accounts, "chat-2").account_id, other.id);

    assert_eq!(codex_request(&route).await.0, 200);
    assert_eq!(*lock(&token_requests), 0, "never refreshed");
    let last = lock(&seen).last().cloned().unwrap();
    assert_eq!(
        last.header("authorization"),
        Some(format!("Bearer {expiring}").as_str())
    );
}

#[tokio::test]
async fn a_history_refusal_moves_the_chat_back_and_retries_there() {
    let tmp = tempfile::tempdir().unwrap();
    let accounts = accounts_at(tmp.path(), read_only());
    let (live, other) = two_codex_accounts(&accounts, "other-access");
    // Turn 1: `other` is out (for a second). Turn 2: the live account
    // refuses the reasoning `other` produced.
    let (url, seen) = fake_provider(|seen| {
        match (seen.header("authorization"), seen.body.as_str()) {
            (Some("Bearer other-access"), "turn-1") => (
                429,
                Vec::new(),
                r#"{"error":{"type":"usage_limit_reached","resets_in_seconds":1}}"#.into(),
            ),
            (Some("Bearer live-access"), "turn-2") => (
                400,
                Vec::new(),
                r#"{"error":{"type":"invalid_request_error","code":"invalid_encrypted_content","message":"The encrypted content could not be verified."}}"#.into(),
            ),
            _ => ok(),
        }
    })
    .await;
    accounts.set_proxy_upstream(ap::Provider::ChatGpt, url);
    accounts.set_rotation(HarnessId::Codex, true).await.unwrap();
    let env = AccountsRunEnvironment(accounts.clone());
    let _ = codex_route(&env, "chat-1").await;
    let route = codex_route(&env, "chat-2").await.0.unwrap();

    assert_eq!(
        request_as(&route, "live-access", "ws-live", "turn-1")
            .await
            .0,
        200
    );
    assert_eq!(chat_rotation(&accounts, "chat-2").account_id, live.id);
    tokio::time::sleep(Duration::from_millis(1200)).await;

    let (status, body) = request_as(&route, "live-access", "ws-live", "turn-2").await;
    assert_eq!((status, body.as_str()), (200, "{\"ok\":true}"));
    let auths: Vec<_> = lock(&seen)
        .iter()
        .map(|s| s.header("authorization").unwrap().to_string())
        .collect();
    assert_eq!(
        auths,
        [
            "Bearer other-access",
            "Bearer live-access",
            "Bearer live-access",
            "Bearer other-access"
        ]
    );
    let chat = chat_rotation(&accounts, "chat-2");
    assert_eq!(chat.account_id, other.id);
    assert_eq!(chat.previous_account_id.as_deref(), Some(live.id.as_str()));
    assert_eq!(
        chat.moved_reason.as_deref(),
        Some("Moved back to other@example.com: this chat's history is tied to it")
    );
    // It stays there from now on.
    assert_eq!(
        request_as(&route, "live-access", "ws-live", "turn-3")
            .await
            .0,
        200
    );
    assert_eq!(
        lock(&seen).last().unwrap().header("authorization"),
        Some("Bearer other-access")
    );
}

#[tokio::test]
async fn a_history_refusal_on_an_exhausted_owner_stops_like_a_usage_limit() {
    let tmp = tempfile::tempdir().unwrap();
    let accounts = accounts_at(tmp.path(), read_only());
    let (_live, other) = two_codex_accounts(&accounts, "other-access");
    let (url, seen) = fake_provider(|seen| match seen.header("authorization") {
        Some("Bearer other-access") => (
            429,
            Vec::new(),
            r#"{"error":{"type":"usage_limit_reached","resets_in_seconds":7200}}"#.into(),
        ),
        _ if seen.body == "turn-2" => (
            400,
            Vec::new(),
            r#"{"error":{"code":"invalid_encrypted_content","message":"encrypted content"}}"#
                .into(),
        ),
        _ => ok(),
    })
    .await;
    accounts.set_proxy_upstream(ap::Provider::ChatGpt, url);
    accounts.set_rotation(HarnessId::Codex, true).await.unwrap();
    let env = AccountsRunEnvironment(accounts.clone());
    let _ = codex_route(&env, "chat-1").await;
    let route = codex_route(&env, "chat-2").await.0.unwrap();
    assert_eq!(
        request_as(&route, "live-access", "ws-live", "turn-1")
            .await
            .0,
        200
    );

    let (status, body) = request_as(&route, "live-access", "ws-live", "turn-2").await;
    assert_eq!(status, 429);
    let body: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(body["error"]["type"], "usage_limit_reached");
    let resets_in = body["error"]["resets_in_seconds"].as_i64().unwrap();
    assert!((7000..=7200).contains(&resets_in), "{resets_in}");
    // Bound to its history's account: the next turn waits for it rather
    // than bouncing back.
    assert_eq!(chat_rotation(&accounts, "chat-2").account_id, other.id);
    let before = lock(&seen).len();
    let (status, _) = request_as(&route, "live-access", "ws-live", "turn-3").await;
    assert_eq!(status, 429);
    assert_eq!(lock(&seen).len(), before + 1, "one try, no loop");
}

#[tokio::test]
async fn pi_pools_keep_their_assignments_when_requests_alternate() {
    let tmp = tempfile::tempdir().unwrap();
    let accounts = accounts_at(tmp.path(), read_only());
    let slots: Vec<Slot> = [
        ("anthropic", "a1"),
        ("openai-codex", "o1"),
        ("anthropic", "a2"),
        ("openai-codex", "o2"),
    ]
    .into_iter()
    .map(|(key, user)| pi_slot(key, user))
    .collect();
    for slot in &slots {
        accounts.write_slot(slot).unwrap();
    }
    accounts.set_rotation(HarnessId::Pi, true).await.unwrap();
    let lease = async |pool: &str| match accounts
        .proxy_lease("c1", HarnessId::Pi, Some(pool), &[], None)
        .await
    {
        ProxyLease::Account { account_id, .. } => account_id,
        other => panic!("{other:?}"),
    };
    let claude = lease("anthropic").await;
    let chatgpt = lease("openai-codex").await;
    for _ in 0..3 {
        assert_eq!(lease("anthropic").await, claude);
        assert_eq!(lease("openai-codex").await, chatgpt);
    }
    // Claude rotation goes out of effect (one usable login left): only its
    // pool's assignment goes.
    accounts.report_rejected(HarnessId::Pi, &slots[2].id);
    assert_eq!(
        accounts
            .proxy_lease("c1", HarnessId::Pi, Some("anthropic"), &[], None)
            .await,
        ProxyLease::Live { account_id: None }
    );
    assert_eq!(lease("openai-codex").await, chatgpt);
    let view = accounts.rotation_view(&[HarnessId::Pi]).remove(0);
    let chat = view.chats.iter().find(|c| c.chat_id == "c1").unwrap();
    assert_eq!(chat.account_id, chatgpt);
}
