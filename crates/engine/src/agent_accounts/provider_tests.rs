//! Grok / Devin / OpenCode / Pi / Hermes accounts against fake credential
//! stores in temp dirs, fake CLIs and a local mock of every provider
//! endpoint — never a real login, token or network call.

use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::stores::*;
use super::usage::*;
use super::*;

fn write(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// A JWT whose payload is `claims` (unsigned — only claims are mined).
fn jwt(claims: serde_json::Value) -> String {
    format!(
        "e30.{}.sig",
        BASE64_URL.encode(serde_json::to_vec(&claims).unwrap())
    )
}

fn chatgpt_access(email: &str, account_id: &str, plan: &str) -> String {
    jwt(serde_json::json!({
        "https://api.openai.com/auth": {
            "chatgpt_account_id": account_id,
            "chatgpt_plan_type": plan,
        },
        "https://api.openai.com/profile": { "email": email },
    }))
}

type Handler = dyn Fn(&str, &str, &str) -> (u16, String) + Send + Sync;

/// A local stand-in for provider endpoints: `handler(method, path+query,
/// body)` answers every request; `hits` records `"METHOD path"`.
struct MockServer {
    base: String,
    hits: Arc<Mutex<Vec<String>>>,
    _task: tokio::task::JoinHandle<()>,
}

impl MockServer {
    async fn start(
        handler: impl Fn(&str, &str, &str) -> (u16, String) + Send + Sync + 'static,
    ) -> Self {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let hits: Arc<Mutex<Vec<String>>> = Arc::default();
        let handler: Arc<Handler> = Arc::new(handler);
        let task_hits = hits.clone();
        let task = tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let handler = handler.clone();
                let hits = task_hits.clone();
                tokio::spawn(async move {
                    let mut raw = Vec::new();
                    let mut chunk = [0u8; 8192];
                    let (head_end, length) = loop {
                        let n = socket.read(&mut chunk).await.unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        raw.extend_from_slice(&chunk[..n]);
                        if let Some(end) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                            let head = String::from_utf8_lossy(&raw[..end]).to_ascii_lowercase();
                            let length = head
                                .lines()
                                .find_map(|l| l.strip_prefix("content-length:"))
                                .and_then(|v| v.trim().parse::<usize>().ok())
                                .unwrap_or(0);
                            break (end + 4, length);
                        }
                    };
                    while raw.len() < head_end + length {
                        let n = socket.read(&mut chunk).await.unwrap_or(0);
                        if n == 0 {
                            break;
                        }
                        raw.extend_from_slice(&chunk[..n]);
                    }
                    let head = String::from_utf8_lossy(&raw[..head_end]).to_string();
                    let body = String::from_utf8_lossy(&raw[head_end..]).to_string();
                    let mut first = head.lines().next().unwrap_or("").split_whitespace();
                    let method = first.next().unwrap_or("").to_string();
                    let path = first.next().unwrap_or("").to_string();
                    lock(&hits).push(format!("{method} {path}"));
                    let (status, reply) = handler(&method, &path, &body);
                    let response = http_response(
                        &format!("{status} X"),
                        &[("Content-Type", "application/json")],
                        &reply,
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        Self {
            base,
            hits,
            _task: task,
        }
    }

    fn hits(&self, prefix: &str) -> usize {
        lock(&self.hits)
            .iter()
            .filter(|h| h.starts_with(prefix))
            .count()
    }
}

fn accounts_with(root: &Path, endpoints: ProbeEndpoints) -> (AgentAccounts, AgentAccountsConfig) {
    let config = AgentAccountsConfig::isolated(root);
    (
        AgentAccounts::with_endpoints(config.clone(), endpoints, Default::default()),
        config,
    )
}

/// Endpoints that all point at `base` (a mock), with slot refresh allowed.
fn mocked(base: &str) -> ProbeEndpoints {
    ProbeEndpoints {
        claude_usage: format!("{base}/api/oauth/usage"),
        claude_token: format!("{base}/v1/oauth/token"),
        claude_loopback_token: format!("{base}/v1/oauth/token"),
        claude_profile: format!("{base}/api/oauth/profile"),
        codex_usage: format!("{base}/backend-api/wham/usage"),
        grok_usage: format!("{base}/v1/billing?format=credits"),
        github_api: base.to_string(),
        github_login: base.to_string(),
        openai_auth: base.to_string(),
        openai_port: 0,
        nous_portal: base.to_string(),
        allow_slot_refresh: true,
        allow_loopback_http: true,
    }
}

fn rows(snapshot: &AgentAccountsSnapshot, harness: HarnessId) -> Vec<AgentAccount> {
    snapshot
        .accounts
        .iter()
        .filter(|a| a.harness == harness)
        .cloned()
        .collect()
}

async fn settle(accounts: &AgentAccounts, login_id: &str) -> Vec<AgentLoginPoll> {
    let mut seen = Vec::new();
    for _ in 0..300 {
        let poll = accounts.poll_login(login_id).await.unwrap();
        let done = poll.status != AgentLoginStatus::Pending;
        seen.push(poll);
        if done {
            return seen;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("login never settled: {seen:?}");
}

async fn browser_get(port: u16, target: &str) -> String {
    let mut socket = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    socket
        .write_all(format!("GET {target} HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut response = String::new();
    let _ = socket.read_to_string(&mut response).await;
    response
}

fn query_param(url: &str, name: &str) -> String {
    reqwest::Url::parse(url)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == name)
        .unwrap()
        .1
        .into_owned()
}

/// A fake CLI under `dir/bin` (the isolated config uses `dir/<agent>` as
/// each agent's home).
#[cfg(unix)]
fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir.join("bin")).unwrap();
    let path = dir.join("bin").join(name);
    std::fs::write(&path, body).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[cfg(unix)]
fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

// ── Grok ────────────────────────────────────────────────────────────────────

fn grok_auth(user: &str, email: &str, key: &str) -> serde_json::Value {
    serde_json::json!({
        "https://auth.x.ai::client-1": {
            "key": key,
            "auth_mode": "oidc",
            "refresh_token": format!("refresh-{user}"),
            "expires_at": "2030-01-01T00:00:00Z",
            "oidc_issuer": "https://auth.x.ai",
            "oidc_client_id": "client-1",
            "user_id": user,
            "email": email,
            "first_name": "Ada",
            "last_name": "L",
            "subscription_tier": "SUBSCRIPTION_TIER_SUPER_GROK_HEAVY",
        }
    })
}

#[test]
fn grok_auth_prefers_auth_x_ai_and_keys_by_identity() {
    let mut auth = grok_auth("user-1", "ada@x.ai", "k1");
    auth.as_object_mut().unwrap().insert(
        "https://accounts.x.ai/sign-in::legacy".into(),
        serde_json::json!({ "key": "legacy", "oidc_issuer": "https://accounts.x.ai" }),
    );
    let detected = parse_grok_auth(auth).unwrap();
    assert_eq!(detected.account_key, "user-1");
    // The slot holds only the selected issuer's token set.
    assert_eq!(
        detected.store_key.as_deref(),
        Some("https://auth.x.ai::client-1")
    );
    let entry = detected.credentials.as_ref().unwrap();
    assert_eq!(entry["key"], "k1");
    assert!(entry.get("https://accounts.x.ai/sign-in::legacy").is_none());
    assert_eq!(detected.profile.email, "ada@x.ai");
    assert_eq!(detected.profile.display_name.as_deref(), Some("Ada L"));
    assert_eq!(detected.profile.plan.as_deref(), Some("SuperGrok Heavy"));
    assert!(detected.identity_known);

    // No identity at all: keyed by the stable issuer::client map key, never
    // the rotating access token.
    let bare = serde_json::json!({ "https://auth.x.ai::c": { "key": "rotating" } });
    let detected = parse_grok_auth(bare.clone()).unwrap();
    assert_eq!(detected.account_key, "oidc:https://auth.x.ai::c");
    let mut rotated = bare;
    rotated["https://auth.x.ai::c"]["key"] = "rotated".into();
    assert_eq!(
        parse_grok_auth(rotated).unwrap().account_key,
        detected.account_key
    );
    assert!(parse_grok_auth(serde_json::json!({ "x": { "refresh_token": "r" } })).is_none());
    assert_eq!(grok_plan(Some("SUBSCRIPTION_TIER_FREE")), None);
    assert_eq!(grok_plan(None), None);
}

#[test]
fn grok_usage_uses_the_grok_build_share_of_the_period() {
    let body = serde_json::json!({
        "config": {
            "creditUsagePercent": 12.0,
            "productUsage": [
                { "product": "Chat", "usagePercent": 80.0 },
                { "product": "GrokBuild", "usagePercent": 45.0 },
            ],
            "currentPeriod": {
                "type": "USAGE_PERIOD_TYPE_WEEKLY",
                "end": "2030-01-08T00:00:00Z",
            },
        }
    });
    let snapshot = grok_usage_snapshot(&body).unwrap();
    assert_eq!(snapshot.windows.len(), 1);
    assert_eq!(snapshot.windows[0].label, "Week");
    assert!((snapshot.windows[0].used_fraction - 0.45).abs() < 1e-6);
    assert!(snapshot.windows[0].resets_at.is_some());
    // Blended percent when the product breakdown is missing.
    let blended = serde_json::json!({ "config": { "creditUsagePercent": 12.0 } });
    let snapshot = grok_usage_snapshot(&blended).unwrap();
    assert_eq!(snapshot.windows[0].label, "Period");
    assert!(grok_usage_snapshot(&serde_json::json!({ "config": {} })).is_none());
}

#[tokio::test]
async fn grok_logins_snapshot_swap_and_forget() {
    let tmp = tempfile::tempdir().unwrap();
    let (accounts, config) = accounts_with(tmp.path(), ProbeEndpoints::default());
    let live = config.grok_home.join("auth.json");
    write(&live, &grok_auth("user-a", "a@x.ai", "key-a").to_string());
    let snapshot = accounts.list(false).await.unwrap();
    let grok = rows(&snapshot, HarnessId::Grok);
    assert_eq!(grok.len(), 1);
    assert!(grok[0].active && grok[0].switchable);
    assert_eq!(grok[0].plan_label.as_deref(), Some("SuperGrok Heavy"));
    let a_id = grok[0].id.clone();

    // The CLI signs in as someone else: both are kept, B is live.
    write(&live, &grok_auth("user-b", "b@x.ai", "key-b").to_string());
    let grok = rows(&accounts.list(false).await.unwrap(), HarnessId::Grok);
    assert_eq!(grok.len(), 2);
    assert_eq!(grok.iter().filter(|a| a.active).count(), 1);
    assert!(
        grok.iter()
            .any(|a| a.email.as_deref() == Some("b@x.ai") && a.active)
    );

    // Switching back rewrites the live store, 0600, and never leaves the
    // lock file held.
    let snapshot = accounts.activate(HarnessId::Grok, &a_id).await.unwrap();
    assert!(
        rows(&snapshot, HarnessId::Grok)
            .iter()
            .any(|a| a.id == a_id && a.active)
    );
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&live).unwrap()).unwrap();
    assert_eq!(written["https://auth.x.ai::client-1"]["key"], "key-a");
    #[cfg(unix)]
    assert_eq!(mode(&live), 0o600);

    // A saved login is just forgotten.
    let b_id = grok
        .iter()
        .find(|a| a.email.as_deref() == Some("b@x.ai"))
        .unwrap()
        .id
        .clone();
    let grok = rows(
        &accounts.forget(HarnessId::Grok, &b_id).await.unwrap(),
        HarnessId::Grok,
    );
    assert_eq!(grok.len(), 1);
    // The live (and only) one signs grok out of that issuer, so it isn't
    // re-detected — the only account stays removable (#546 parity).
    let grok = rows(
        &accounts.forget(HarnessId::Grok, &a_id).await.unwrap(),
        HarnessId::Grok,
    );
    assert!(grok.is_empty(), "{grok:?}");
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&live).unwrap()).unwrap();
    assert!(written.get("https://auth.x.ai::client-1").is_none());
}

/// Removing a per-provider agent's live login drops that store entry only:
/// other issuers (Grok), other providers' logins and API keys (OpenCode)
/// stay; Devin's key file goes.
#[tokio::test]
async fn forgetting_a_live_login_signs_out_only_that_login() {
    let tmp = tempfile::tempdir().unwrap();
    let (accounts, config) = accounts_with(tmp.path(), ProbeEndpoints::default());
    let grok_file = config.grok_home.join("auth.json");
    let mut grok = grok_auth("user-a", "a@x.ai", "key-a");
    grok.as_object_mut().unwrap().insert(
        "https://sso.corp.example::cli".into(),
        serde_json::json!({ "refresh_token": "keep-me" }),
    );
    write(&grok_file, &grok.to_string());
    write(
        &config.opencode_auth_file,
        &serde_json::json!({
            "openai": openai_entry("a@example.com", "acct-a"),
            "anthropic": { "type": "api", "key": "sk-ant-keep" },
        })
        .to_string(),
    );
    write(
        &config.devin_credentials_file,
        "windsurf_api_key = \"devin-key-1234\"\n",
    );
    let snapshot = accounts.list(false).await.unwrap();
    for harness in [HarnessId::Grok, HarnessId::Opencode, HarnessId::Devin] {
        let live = rows(&snapshot, harness);
        assert_eq!(live.len(), 1, "{harness:?}");
        assert!(live[0].active);
        let left = rows(
            &accounts.forget(harness, &live[0].id).await.unwrap(),
            harness,
        );
        assert!(left.is_empty(), "{harness:?}: {left:?}");
        assert!(accounts.read_slots(harness).is_empty(), "{harness:?}");
    }
    let grok: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&grok_file).unwrap()).unwrap();
    assert!(grok.get("https://auth.x.ai::client-1").is_none());
    assert_eq!(
        grok["https://sso.corp.example::cli"]["refresh_token"],
        "keep-me"
    );
    let opencode: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&config.opencode_auth_file).unwrap())
            .unwrap();
    assert!(opencode.get("openai").is_none());
    assert_eq!(opencode["anthropic"]["key"], "sk-ant-keep");
    assert!(!config.devin_credentials_file.exists());
    // Nothing comes back on the next list.
    let snapshot = accounts.list(false).await.unwrap();
    for harness in [HarnessId::Grok, HarnessId::Opencode, HarnessId::Devin] {
        assert!(rows(&snapshot, harness).is_empty(), "{harness:?}");
    }
}

#[tokio::test]
async fn a_rejected_saved_grok_token_refreshes_once_against_its_issuer() {
    let issuer_hits = Arc::new(AtomicUsize::new(0));
    let hits = issuer_hits.clone();
    let server = MockServer::start(move |method, path, body| match (method, path) {
        ("GET", p) if p.starts_with("/v1/billing") => {
            // Only the refreshed token is accepted.
            (200, r#"{"config":{"creditUsagePercent":30}}"#.to_string())
        }
        ("POST", "/oauth2/token") => {
            hits.fetch_add(1, Ordering::SeqCst);
            assert!(body.contains("grant_type=refresh_token"), "{body}");
            (
                200,
                r#"{"access_token":"fresh","refresh_token":"rotated","expires_in":60}"#.into(),
            )
        }
        _ => (404, String::new()),
    })
    .await;
    // Billing rejects every token but "fresh".
    let billing = MockServer::start(|_, _, _| (401, String::new())).await;
    let tmp = tempfile::tempdir().unwrap();
    let mut endpoints = mocked(&server.base);
    endpoints.grok_usage = format!("{}/v1/billing?format=credits", billing.base);
    let (accounts, _) = accounts_with(tmp.path(), endpoints);
    let mut auth = grok_auth("user-a", "a@x.ai", "stale");
    auth["https://auth.x.ai::client-1"]["oidc_issuer"] = server.base.clone().into();
    let detected = parse_grok_auth(auth).unwrap();
    let slot = Slot {
        id: slot_id_for(HarnessId::Grok, "user-a"),
        harness: HarnessId::Grok,
        account_key: "user-a".into(),
        profile: detected.profile,
        credentials: detected.credentials.unwrap(),
        claude_config: None,
        saved_at: 1,
        created_at: None,
        store_key: detected.store_key,
    };
    accounts.write_slot(&slot).unwrap();
    // The LIVE login is never refreshed by zeron.
    let live = accounts.grok_usage(&slot, true).await;
    assert!(matches!(live, Err(ProbeError::Unauthorized { .. })));
    assert_eq!(issuer_hits.load(Ordering::SeqCst), 0);
    // A saved one is — once — and the rotated pair lands in its slot.
    let saved = accounts.grok_usage(&slot, false).await;
    assert_eq!(issuer_hits.load(Ordering::SeqCst), 1);
    assert!(
        matches!(saved, Err(ProbeError::Unauthorized { .. })),
        "billing mock rejects all"
    );
    let stored = accounts.read_slot(HarnessId::Grok, &slot.id).unwrap();
    assert_eq!(stored.credentials["key"], "fresh");
    assert_eq!(stored.credentials["refresh_token"], "rotated");

    // An issuer outside xAI never receives the refresh token.
    let mut evil = slot.clone();
    evil.credentials["oidc_issuer"] = "https://auth.x.ai.evil.example".into();
    let before = issuer_hits.load(Ordering::SeqCst);
    let refused = accounts.grok_usage(&evil, false).await;
    assert_eq!(refused, Err(ProbeError::UntrustedEndpoint));
    assert_eq!(issuer_hits.load(Ordering::SeqCst), before);
}

#[cfg(unix)]
#[tokio::test]
async fn grok_add_account_runs_the_device_login_in_a_throwaway_home() {
    let tmp = tempfile::tempdir().unwrap();
    let (accounts, config) = accounts_with(tmp.path(), ProbeEndpoints::default());
    let cli = script(
        tmp.path(),
        "grok",
        r#"#!/bin/sh
case "$*" in *"login --device-auth"*) ;; *) echo "unexpected: $*" >&2; exit 3 ;; esac
case "$GROK_HOME" in *".login-"*) ;; *) echo "not isolated" >&2; exit 4 ;; esac
printf 'Open this URL in your browser to approve:\n  \033[1mhttps://accounts.x.ai/device?user_code=WXYZ-9876\033[0m\nCode: WXYZ-9876\nWaiting for approval\n'
sleep 1
printf '{"https://auth.x.ai::c":{"key":"k","refresh_token":"r","oidc_issuer":"https://auth.x.ai","oidc_client_id":"c","user_id":"u-new","email":"new@x.ai"}}' > "$GROK_HOME/auth.json"
sleep 30
"#,
    );
    accounts.override_cli(HarnessId::Grok, cli);
    let start = accounts.start_login(HarnessId::Grok).await.unwrap();
    assert_eq!(
        start.url,
        "https://accounts.x.ai/device?user_code=WXYZ-9876"
    );
    assert_eq!(start.callback_port, None, "a device code needs no tunnel");
    let polls = settle(&accounts, &start.login_id).await;
    let last = polls.last().unwrap();
    assert_eq!(last.status, AgentLoginStatus::Done, "{polls:?}");
    assert!(
        polls
            .iter()
            .any(|p| p.message.as_deref() == Some("Enter the code WXYZ-9876 when asked."))
    );
    // No live login before: the new one is connected, and it's a slot.
    let grok = rows(&accounts.list(false).await.unwrap(), HarnessId::Grok);
    assert_eq!(grok.len(), 1);
    assert!(grok[0].active && grok[0].email.as_deref() == Some("new@x.ai"));
    assert!(config.grok_home.join("auth.json").exists());
    // The throwaway home is reclaimed.
    let leftovers = std::fs::read_dir(config.data_dir.join("agent-accounts"))
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(".login-"))
        .count();
    assert_eq!(leftovers, 0);
}

// ── Devin ───────────────────────────────────────────────────────────────────

#[test]
fn devin_credentials_round_trip_and_key_by_digest() {
    let text = "windsurf_api_key = \"sk-devin-\\\"quoted\\\"-abcd\"\napi_server_url = \"https://server.codeium.com\"\ndangerously_skip_plugin_authentication = true\n";
    let creds = parse_devin_toml(text).unwrap();
    assert_eq!(creds["windsurf_api_key"], "sk-devin-\"quoted\"-abcd");
    assert_eq!(creds["dangerously_skip_plugin_authentication"], true);
    let back = parse_devin_toml(&devin_toml(&creds).unwrap()).unwrap();
    assert_eq!(back, creds, "unknown keys and escapes survive a swap");
    let detected = parse_devin_credentials(creds).unwrap();
    assert!(detected.account_key.starts_with("api-key:"));
    assert!(
        !detected.account_key.contains("abcd"),
        "never the key itself"
    );
    assert!(!detected.identity_known);
    assert_eq!(detected.profile.email, "Devin account ·…abcd");
    assert!(parse_devin_toml("").is_none());
    assert!(parse_devin_credentials(serde_json::json!({ "api_server_url": "x" })).is_none());
}

#[test]
fn devin_usage_inverts_remaining_quota_and_names_the_account() {
    let body = serde_json::json!({
        "userStatus": {
            "email": "dev@example.com",
            "name": "Dev",
            "planStatus": {
                "planInfo": { "teamsTier": "TEAMS_TIER_DEVIN_PRO", "planName": "Pro" },
                "dailyQuotaRemainingPercent": 75,
                "dailyQuotaResetAtUnix": "1893456000",
                "weeklyQuotaRemainingPercent": 10.0,
                "weeklyQuotaResetAtUnix": 1893888000,
            }
        }
    });
    let (snapshot, identity) = devin_usage_snapshot(&body).unwrap();
    assert_eq!(identity.email.as_deref(), Some("dev@example.com"));
    assert_eq!(snapshot.plan_label.as_deref(), Some("Devin Pro"));
    let labels: Vec<_> = snapshot.windows.iter().map(|w| w.label.as_str()).collect();
    assert_eq!(labels, ["Day", "Week"]);
    assert!((snapshot.windows[0].used_fraction - 0.25).abs() < 1e-6);
    assert!((snapshot.windows[1].used_fraction - 0.90).abs() < 1e-6);
    assert!(snapshot.windows.iter().all(|w| w.resets_at.is_some()));
    assert_eq!(
        devin_plan_label(None, Some("Teams")).as_deref(),
        Some("Devin Teams")
    );
    assert!(devin_usage_snapshot(&serde_json::json!({ "userStatus": {} })).is_none());
}

#[tokio::test]
async fn devin_swaps_and_learns_its_identity_from_the_usage_probe() {
    let server = MockServer::start(|method, path, body| {
        assert_eq!(method, "POST");
        assert!(
            path.ends_with("SeatManagementService/GetUserStatus"),
            "{path}"
        );
        let who = if body.contains("key-a") { "a" } else { "b" };
        (
            200,
            serde_json::json!({ "userStatus": {
                "email": format!("{who}@devin.ai"),
                "planStatus": { "dailyQuotaRemainingPercent": 50 },
            }})
            .to_string(),
        )
    })
    .await;
    let tmp = tempfile::tempdir().unwrap();
    let (accounts, config) = accounts_with(tmp.path(), mocked(&server.base));
    let live = &config.devin_credentials_file;
    let creds = |key: &str| {
        format!(
            "windsurf_api_key = \"{key}\"\napi_server_url = \"{}\"\n",
            server.base
        )
    };
    write(live, &creds("key-a"));
    let devin = rows(&accounts.list(true).await.unwrap(), HarnessId::Devin);
    assert_eq!(devin.len(), 1);
    assert_eq!(
        devin[0].email.as_deref(),
        Some("a@devin.ai"),
        "identity written back"
    );
    assert!((devin[0].usage_windows[0].used_fraction - 0.5).abs() < 1e-6);
    let a_id = devin[0].id.clone();
    // A later detection of the same key keeps the learned identity.
    let devin = rows(&accounts.list(false).await.unwrap(), HarnessId::Devin);
    assert_eq!(devin[0].email.as_deref(), Some("a@devin.ai"));

    write(live, &creds("key-b"));
    assert_eq!(
        rows(&accounts.list(false).await.unwrap(), HarnessId::Devin).len(),
        2
    );
    accounts.activate(HarnessId::Devin, &a_id).await.unwrap();
    let text = std::fs::read_to_string(live).unwrap();
    assert!(text.contains("key-a") && !text.contains("key-b"));
    #[cfg(unix)]
    assert_eq!(mode(live), 0o600);
}

#[cfg(unix)]
#[tokio::test]
async fn devin_add_account_authenticates_over_acp_in_a_throwaway_data_home() {
    let tmp = tempfile::tempdir().unwrap();
    let (accounts, config) = accounts_with(tmp.path(), ProbeEndpoints::default());
    let agent = script(
        tmp.path(),
        "devin",
        r#"#!/bin/sh
while read -r line; do
  id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9]*\).*/\1/')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1,"agentInfo":{"name":"devin","website":"https://devin.ai/"},"authMethods":[{"id":"devin-browser","name":"Log in with browser"}]}}\n' "$id" ;;
    *'"method":"authenticate"'*)
      case "$line" in *'"methodId":"devin-browser"'*) ;; *) exit 5 ;; esac
      case "$XDG_DATA_HOME" in *".login-"*) ;; *) exit 6 ;; esac
      printf 'Opening https://app.devin.ai/auth/cli/continue?redirect_uri=http%%3A%%2F%%2F127.0.0.1%%3A45678%%2Fcallback&state=s\n' >&2
      sleep 1
      mkdir -p "$XDG_DATA_HOME/devin"
      printf 'windsurf_api_key = "fresh-devin-key-9999"\n' > "$XDG_DATA_HOME/devin/credentials.toml"
      printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id" ;;
    *'"id":'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id" ;;
  esac
done
"#,
    );
    accounts.override_cli(HarnessId::Devin, agent);
    let routes = accounts.inner.callback_routes.clone();
    let start = accounts
        .start_login_for(HarnessId::Devin, Some("device-b"))
        .await
        .unwrap();
    let polls = settle(&accounts, &start.login_id).await;
    assert_eq!(
        polls.last().unwrap().status,
        AgentLoginStatus::Done,
        "{polls:?}"
    );
    // The sign-in page (not the url in the handshake) reached the app, with
    // its loopback port for the remote requester's tunnel.
    let page = polls
        .iter()
        .find_map(|p| p.url.clone())
        .expect("url reported");
    assert!(page.contains("/auth/cli/continue"), "{page}");
    assert!(polls.iter().any(|p| p.callback_port == Some(45678)));
    assert!(
        !routes.is_registered(&start.login_id),
        "route dropped once done"
    );
    // No live login before → the fresh key is connected.
    let text = std::fs::read_to_string(&config.devin_credentials_file).unwrap();
    assert!(text.contains("fresh-devin-key-9999"));
    let devin = rows(&accounts.list(false).await.unwrap(), HarnessId::Devin);
    assert_eq!(devin.len(), 1);
    assert!(devin[0].active);
}

// ── OpenCode / Pi ───────────────────────────────────────────────────────────

fn openai_entry(email: &str, account: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "oauth",
        "access": chatgpt_access(email, account, "plus"),
        "refresh": format!("refresh-{account}"),
        "expires": 1,
        "accountId": account,
    })
}

#[tokio::test]
async fn opencode_swaps_one_provider_entry_and_leaves_the_rest() {
    let tmp = tempfile::tempdir().unwrap();
    let (accounts, config) = accounts_with(tmp.path(), ProbeEndpoints::default());
    let file = &config.opencode_auth_file;
    let store = |openai: serde_json::Value| {
        serde_json::json!({
            "openai": openai,
            "anthropic": { "type": "api", "key": "sk-ant-keep" },
            "zen": { "type": "wellknown", "key": "k", "token": "t" },
        })
        .to_string()
    };
    write(file, &store(openai_entry("a@example.com", "acct-a")));
    let rows_a = rows(&accounts.list(false).await.unwrap(), HarnessId::Opencode);
    assert_eq!(rows_a.len(), 1, "API-key entries aren't accounts");
    assert_eq!(rows_a[0].provider.as_deref(), Some("openai"));
    assert_eq!(rows_a[0].plan_label.as_deref(), Some("ChatGPT Plus"));
    assert_eq!(rows_a[0].email.as_deref(), Some("a@example.com"));
    let a_id = rows_a[0].id.clone();

    write(file, &store(openai_entry("b@example.com", "acct-b")));
    assert_eq!(
        rows(&accounts.list(false).await.unwrap(), HarnessId::Opencode).len(),
        2
    );
    accounts.activate(HarnessId::Opencode, &a_id).await.unwrap();
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
    assert_eq!(written["openai"]["accountId"], "acct-a");
    assert_eq!(
        written["anthropic"]["key"], "sk-ant-keep",
        "other providers untouched"
    );
    assert_eq!(written["zen"]["token"], "t");
    #[cfg(unix)]
    assert_eq!(mode(file), 0o600);

    // A store that no longer parses is never overwritten.
    write(file, "{ not json");
    let err = accounts
        .write_keyed_entry(
            HarnessId::Opencode,
            "openai",
            Some(&openai_entry("a@example.com", "acct-a")),
        )
        .unwrap_err();
    assert!(err.to_string().contains("could not be parsed"), "{err}");
    assert_eq!(std::fs::read_to_string(file).unwrap(), "{ not json");
}

#[tokio::test]
async fn pi_swaps_under_its_lockfile_and_groups_rows_per_provider() {
    let tmp = tempfile::tempdir().unwrap();
    let (accounts, config) = accounts_with(tmp.path(), ProbeEndpoints::default());
    let file = config.pi_agent_dir.join("auth.json");
    write(
        &file,
        &serde_json::json!({ "openai-codex": openai_entry("a@example.com", "acct-a") }).to_string(),
    );
    let a_id = rows(&accounts.list(false).await.unwrap(), HarnessId::Pi)[0]
        .id
        .clone();
    write(
        &file,
        &serde_json::json!({ "openai-codex": openai_entry("b@example.com", "acct-b") }).to_string(),
    );
    accounts.list(false).await.unwrap();
    accounts.activate(HarnessId::Pi, &a_id).await.unwrap();
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(written["openai-codex"]["accountId"], "acct-a");
    assert!(
        !config.pi_agent_dir.join("auth.json.lock").exists(),
        "lock released"
    );

    // A lock held by pi makes the swap wait, then give up — never write
    // underneath it.
    std::fs::create_dir(config.pi_agent_dir.join("auth.json.lock")).unwrap();
    let blocked = accounts.write_keyed_entry(
        HarnessId::Pi,
        "openai-codex",
        Some(&openai_entry("b@example.com", "acct-b")),
    );
    assert!(blocked.unwrap_err().to_string().contains("locked"));
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(written["openai-codex"]["accountId"], "acct-a");
}

#[tokio::test]
async fn an_opaque_pi_claude_token_is_identified_once_then_matched_by_token() {
    let server = MockServer::start(|_, path, _| match path {
        "/api/oauth/profile" => (
            200,
            serde_json::json!({
                "account": { "uuid": "acct-uuid", "email_address": "claude@example.com" },
                "organization": { "name": "Org", "organization_type": "claude_max",
                                  "rate_limit_tier": "default_claude_max_20x" },
            })
            .to_string(),
        ),
        _ => (404, String::new()),
    })
    .await;
    let tmp = tempfile::tempdir().unwrap();
    let (accounts, config) = accounts_with(tmp.path(), mocked(&server.base));
    let file = config.pi_agent_dir.join("auth.json");
    let entry = |refresh: &str| {
        serde_json::json!({ "anthropic": {
            "type": "oauth", "access": "opaque-access", "refresh": refresh, "expires": 1,
        }})
        .to_string()
    };
    write(&file, &entry("r1"));
    let pi = rows(&accounts.list(false).await.unwrap(), HarnessId::Pi);
    assert_eq!(pi.len(), 1);
    assert_eq!(pi[0].email.as_deref(), Some("claude@example.com"));
    assert_eq!(pi[0].plan_label.as_deref(), Some("Claude Max 20×"));
    assert!(pi[0].active && pi[0].switchable);
    assert_eq!(server.hits("GET /api/oauth/profile"), 1);
    // The same token again: matched to its slot, no network.
    accounts.list(false).await.unwrap();
    assert_eq!(server.hits("GET /api/oauth/profile"), 1);
    // Pi refreshed (new pair): one more lookup, SAME account.
    write(&file, &entry("r2"));
    let pi = rows(&accounts.list(false).await.unwrap(), HarnessId::Pi);
    assert_eq!(pi.len(), 1);
    assert_eq!(server.hits("GET /api/oauth/profile"), 2);
}

#[tokio::test]
async fn an_unidentifiable_live_token_is_listed_but_not_switchable() {
    let server = MockServer::start(|_, _, _| (401, String::new())).await;
    let tmp = tempfile::tempdir().unwrap();
    let (accounts, config) = accounts_with(tmp.path(), mocked(&server.base));
    write(
        &config.opencode_auth_file,
        &serde_json::json!({ "github-copilot": {
            "type": "oauth", "access": "gho_x", "refresh": "gho_x", "expires": 0,
        }})
        .to_string(),
    );
    let rows = rows(&accounts.list(false).await.unwrap(), HarnessId::Opencode);
    assert_eq!(rows.len(), 1);
    assert!(rows[0].active && !rows[0].switchable);
    assert_eq!(rows[0].email.as_deref(), Some("GitHub account"));
    assert_eq!(rows[0].provider.as_deref(), Some("github-copilot"));
    // Nothing was snapshotted without an identity.
    assert!(accounts.read_slots(HarnessId::Opencode).is_empty());
    // A failed lookup isn't retried on every list.
    accounts.list(false).await.unwrap();
    assert_eq!(server.hits("GET /user"), 1);
}

#[tokio::test]
async fn chatgpt_sign_in_for_pi_lands_on_the_loopback_and_connects_the_first_login() {
    let server = MockServer::start(|method, path, body| match (method, path) {
        ("POST", "/oauth/token") => {
            assert!(body.contains("grant_type=authorization_code"), "{body}");
            assert!(body.contains("code=good-code"), "{body}");
            assert!(body.contains("code_verifier="), "{body}");
            let who = if body.contains("second") { "b" } else { "a" };
            (
                200,
                serde_json::json!({
                    "access_token": chatgpt_access(&format!("{who}@example.com"), &format!("acct-{who}"), "pro"),
                    "refresh_token": format!("refresh-{who}"),
                    "id_token": jwt(serde_json::json!({ "email": format!("{who}@example.com") })),
                    "expires_in": 3600,
                })
                .to_string(),
            )
        }
        _ => (404, String::new()),
    })
    .await;
    let tmp = tempfile::tempdir().unwrap();
    let (accounts, config) = accounts_with(tmp.path(), mocked(&server.base));
    let sign_in = |code: &'static str| {
        let accounts = accounts.clone();
        async move {
            let start = accounts.start_login(HarnessId::Pi).await.unwrap();
            assert_eq!(start.mode, AgentLoginMode::Browser);
            let port = start.callback_port.expect("loopback port reported");
            assert_eq!(loopback_port(&start.url), Some(port));
            assert_eq!(query_param(&start.url, "originator"), "pi");
            let state = query_param(&start.url, "state");
            // A stray without our state neither finishes nor kills it.
            assert!(
                browser_get(port, "/auth/callback?code=x&state=nope")
                    .await
                    .starts_with("HTTP/1.1 400")
            );
            let reply =
                browser_get(port, &format!("/auth/callback?code={code}&state={state}")).await;
            assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
            let polls = settle(&accounts, &start.login_id).await;
            assert_eq!(
                polls.last().unwrap().status,
                AgentLoginStatus::Done,
                "{polls:?}"
            );
        }
    };
    sign_in("good-code").await;
    let file = config.pi_agent_dir.join("auth.json");
    let live: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(
        live["openai-codex"]["accountId"], "acct-a",
        "first login connected"
    );
    assert_eq!(live["openai-codex"]["type"], "oauth");
    // A second account is saved next to it — the live one stays.
    sign_in("good-code-second").await;
    let live: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(live["openai-codex"]["accountId"], "acct-a");
    let pi = rows(&accounts.list(false).await.unwrap(), HarnessId::Pi);
    assert_eq!(pi.len(), 2);
    assert!(
        pi.iter()
            .any(|a| a.email.as_deref() == Some("b@example.com") && !a.active)
    );
    assert!(
        pi.iter()
            .all(|a| a.plan_label.as_deref() == Some("ChatGPT Pro"))
    );
    // Claude logins for Pi stay with pi.
    let refused = accounts
        .start_login_with(HarnessId::Pi, Some("anthropic"), None)
        .await
        .unwrap_err();
    assert!(refused.to_string().contains("/login"), "{refused}");
}

/// Signing in again as the live account (its tokens dead or revoked) makes
/// the fresh login live — otherwise the next list would snapshot the dead
/// live tokens straight back over the fresh slot (#546, for per-provider
/// stores too).
#[tokio::test]
async fn re_signing_in_the_live_chatgpt_account_replaces_its_dead_tokens() {
    let server = MockServer::start(|method, path, _| match (method, path) {
        ("POST", "/oauth/token") => (
            200,
            serde_json::json!({
                "access_token": chatgpt_access("a@example.com", "acct-a", "pro"),
                "refresh_token": "fresh-refresh",
                "expires_in": 3600,
            })
            .to_string(),
        ),
        _ => (404, String::new()),
    })
    .await;
    let tmp = tempfile::tempdir().unwrap();
    let (accounts, config) = accounts_with(tmp.path(), mocked(&server.base));
    let file = config.pi_agent_dir.join("auth.json");
    let mut dead = openai_entry("a@example.com", "acct-a");
    dead["refresh"] = "dead-refresh".into();
    write(
        &file,
        &serde_json::json!({
            "openai-codex": dead,
            "anthropic": { "type": "api", "key": "sk-ant-keep" },
        })
        .to_string(),
    );
    accounts.list(false).await.unwrap();

    let start = accounts.start_login(HarnessId::Pi).await.unwrap();
    let port = start.callback_port.unwrap();
    let state = query_param(&start.url, "state");
    let reply = browser_get(port, &format!("/auth/callback?code=c&state={state}")).await;
    assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
    let polls = settle(&accounts, &start.login_id).await;
    assert_eq!(polls.last().unwrap().status, AgentLoginStatus::Done);

    let live: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(live["openai-codex"]["refresh"], "fresh-refresh");
    assert_eq!(live["anthropic"]["key"], "sk-ant-keep");
    // A list afterwards keeps the fresh tokens in the slot.
    let pi = rows(&accounts.list(false).await.unwrap(), HarnessId::Pi);
    assert_eq!(pi.len(), 1);
    assert!(pi[0].active);
    let slot = accounts.read_slots(HarnessId::Pi).pop().unwrap();
    assert_eq!(slot.credentials["refresh"], "fresh-refresh");
}

/// A live login zeron can't identify (an opaque token whose profile call
/// failed) is never replaced by a new sign-in — it has no slot, so it would
/// be lost.
#[tokio::test]
async fn a_new_login_never_replaces_an_unidentified_live_one() {
    let user_calls = Arc::new(AtomicUsize::new(0));
    let calls = user_calls.clone();
    let server = MockServer::start(move |method, path, _| match (method, path) {
        ("POST", "/login/device/code") => (
            200,
            r#"{"device_code":"dc","user_code":"ABCD-1234","verification_uri":"https://github.com/login/device","interval":1}"#.into(),
        ),
        ("POST", "/login/oauth/access_token") => {
            (200, r#"{"access_token":"gho_new","token_type":"bearer"}"#.into())
        }
        // The live token's lookup fails; the new login's succeeds.
        ("GET", "/user") if calls.fetch_add(1, Ordering::SeqCst) == 0 => (401, String::new()),
        ("GET", "/user") => (200, r#"{"id":42,"login":"octo"}"#.into()),
        _ => (404, String::new()),
    })
    .await;
    let tmp = tempfile::tempdir().unwrap();
    let (accounts, config) = accounts_with(tmp.path(), mocked(&server.base));
    write(
        &config.opencode_auth_file,
        &serde_json::json!({ "github-copilot": {
            "type": "oauth", "access": "gho_old", "refresh": "gho_old", "expires": 0,
        }})
        .to_string(),
    );
    let before = rows(&accounts.list(false).await.unwrap(), HarnessId::Opencode);
    assert!(before[0].active && !before[0].switchable);

    let start = accounts
        .start_login_with(HarnessId::Opencode, Some("github-copilot"), None)
        .await
        .unwrap();
    let polls = settle(&accounts, &start.login_id).await;
    assert_eq!(
        polls.last().unwrap().status,
        AgentLoginStatus::Done,
        "{polls:?}"
    );
    let live: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&config.opencode_auth_file).unwrap())
            .unwrap();
    assert_eq!(
        live["github-copilot"]["refresh"], "gho_old",
        "live untouched"
    );
    let rows = rows(&accounts.list(false).await.unwrap(), HarnessId::Opencode);
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert!(
        rows.iter()
            .any(|a| a.email.as_deref() == Some("octo") && !a.active && a.switchable)
    );
}

#[tokio::test]
async fn a_new_chatgpt_sign_in_supersedes_one_holding_the_same_port() {
    let tmp = tempfile::tempdir().unwrap();
    let (accounts, _) = accounts_with(tmp.path(), mocked("http://127.0.0.1:9"));
    let first = accounts.start_login(HarnessId::Opencode).await.unwrap();
    let second = accounts.start_login(HarnessId::Pi).await.unwrap();
    assert!(
        accounts.poll_login(&first.login_id).await.is_err(),
        "reaped"
    );
    assert_eq!(
        accounts.poll_login(&second.login_id).await.unwrap().status,
        AgentLoginStatus::Pending
    );
    accounts.cancel_login(&second.login_id);
}

#[tokio::test]
async fn copilot_device_sign_in_for_opencode_shows_the_code_and_connects() {
    let polls = Arc::new(AtomicUsize::new(0));
    let polled = polls.clone();
    let server = MockServer::start(move |method, path, body| match (method, path) {
        ("POST", "/login/device/code") => {
            assert!(body.contains("client_id=Ov23li8tweQw6odWQebz"), "{body}");
            (
                200,
                r#"{"device_code":"dc","user_code":"ABCD-1234","verification_uri":"https://github.com/login/device","interval":1,"expires_in":900}"#.into(),
            )
        }
        ("POST", "/login/oauth/access_token") => {
            if polled.fetch_add(1, Ordering::SeqCst) == 0 {
                (200, r#"{"error":"authorization_pending"}"#.into())
            } else {
                (200, r#"{"access_token":"gho_new","token_type":"bearer"}"#.into())
            }
        }
        ("GET", "/user") => (200, r#"{"id":42,"login":"octo","name":"Octo Cat"}"#.into()),
        _ => (404, String::new()),
    })
    .await;
    let tmp = tempfile::tempdir().unwrap();
    let (accounts, config) = accounts_with(tmp.path(), mocked(&server.base));
    let start = accounts
        .start_login_with(HarnessId::Opencode, Some("github-copilot"), None)
        .await
        .unwrap();
    assert_eq!(start.url, "https://github.com/login/device");
    assert_eq!(start.callback_port, None);
    let first = accounts.poll_login(&start.login_id).await.unwrap();
    assert_eq!(
        first.message.as_deref(),
        Some("Enter the code ABCD-1234 on GitHub.")
    );
    let polls = settle(&accounts, &start.login_id).await;
    assert_eq!(
        polls.last().unwrap().status,
        AgentLoginStatus::Done,
        "{polls:?}"
    );
    let live: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&config.opencode_auth_file).unwrap())
            .unwrap();
    assert_eq!(live["github-copilot"]["refresh"], "gho_new");
    assert_eq!(live["github-copilot"]["access"], "gho_new");
    let rows = rows(&accounts.list(false).await.unwrap(), HarnessId::Opencode);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].email.as_deref(), Some("octo"));
    assert!(rows[0].active && rows[0].switchable);
}

#[test]
fn copilot_usage_reads_metered_quotas_and_the_plan() {
    let paid = serde_json::json!({
        "login": "octo",
        "copilot_plan": "individual",
        "quota_reset_date_utc": "2030-02-01T00:00:00.000Z",
        "quota_snapshots": {
            "chat": { "unlimited": true, "percent_remaining": 100.0 },
            "completions": { "unlimited": true },
            "premium_interactions": { "entitlement": 300, "remaining": 75,
                                      "percent_remaining": 25.0, "unlimited": false },
        }
    });
    let snapshot = copilot_usage_snapshot(&paid).unwrap();
    assert_eq!(snapshot.plan_label.as_deref(), Some("Copilot Pro"));
    assert_eq!(snapshot.windows.len(), 1);
    assert_eq!(snapshot.windows[0].label, "Premium");
    assert!((snapshot.windows[0].used_fraction - 0.75).abs() < 1e-6);
    assert!(snapshot.windows[0].resets_at.is_some());

    let free = serde_json::json!({
        "copilot_plan": "free",
        "limited_user_reset_date": "2030-02-01",
        "monthly_quotas": { "chat": 50, "completions": 2000 },
        "limited_user_quotas": { "chat": 40, "completions": 500 },
    });
    let snapshot = copilot_usage_snapshot(&free).unwrap();
    let labels: Vec<_> = snapshot.windows.iter().map(|w| w.label.as_str()).collect();
    assert_eq!(labels, ["Chat", "Completions"]);
    assert!((snapshot.windows[0].used_fraction - 0.2).abs() < 1e-6);
    assert!((snapshot.windows[1].used_fraction - 0.75).abs() < 1e-6);
    assert!(copilot_usage_snapshot(&serde_json::json!({})).is_none());
}

#[tokio::test]
async fn keyed_logins_probe_the_vendor_behind_them() {
    let server = MockServer::start(|method, path, _| match (method, path) {
        ("GET", "/backend-api/wham/usage") => (
            200,
            r#"{"plan_type":"plus","rate_limit":{"primary_window":{"used_percent":40,"limit_window_seconds":18000}}}"#.into(),
        ),
        ("GET", "/copilot_internal/user") => (
            200,
            r#"{"copilot_plan":"business","quota_snapshots":{"premium_interactions":{"percent_remaining":90,"unlimited":false}}}"#.into(),
        ),
        _ => (404, String::new()),
    })
    .await;
    let tmp = tempfile::tempdir().unwrap();
    let (accounts, config) = accounts_with(tmp.path(), mocked(&server.base));
    write(
        &config.opencode_auth_file,
        &serde_json::json!({ "openai": openai_entry("a@example.com", "acct-a") }).to_string(),
    );
    let slot = accounts.read_slots(HarnessId::Opencode);
    assert!(slot.is_empty());
    let rows = rows(&accounts.list(true).await.unwrap(), HarnessId::Opencode);
    assert!((rows[0].usage_windows[0].used_fraction - 0.4).abs() < 1e-6);
    assert_eq!(rows[0].plan_label.as_deref(), Some("ChatGPT Plus"));
    let copilot = Slot {
        id: "0123456789abcdef".into(),
        harness: HarnessId::Pi,
        account_key: "github-copilot:1".into(),
        profile: SlotProfile {
            email: "octo".into(),
            display_name: None,
            organization: None,
            plan: None,
            auth_kind: AgentAuthKind::Oauth,
        },
        credentials: serde_json::json!({ "type": "oauth", "access": "copilot-session", "refresh": "gho_x" }),
        claude_config: None,
        saved_at: 1,
        created_at: None,
        store_key: Some("github-copilot".into()),
    };
    let usage = accounts.keyed_usage(HarnessId::Pi, &copilot).await.unwrap();
    assert_eq!(usage.plan_label.as_deref(), Some("Copilot Business"));
    assert!((usage.windows[0].used_fraction - 0.1).abs() < 1e-6);
}

// ── Hermes ──────────────────────────────────────────────────────────────────

fn hermes_auth() -> serde_json::Value {
    serde_json::json!({
        "version": 1,
        "active_provider": "openai-codex",
        "providers": {},
        "credential_pool": {
            "nous": [
                { "id": "n1", "label": "nous@example.com", "auth_type": "oauth", "priority": 0,
                  "source": "device_code", "access_token": "nous-access",
                  "portal_base_url": "PORTAL" },
            ],
            "openai-codex": [
                { "id": "c2", "label": "second@example.com", "auth_type": "oauth", "priority": 1,
                  "access_token": chatgpt_access("second@example.com", "acct-2", "plus") },
                { "id": "c1", "label": "first@example.com", "auth_type": "oauth", "priority": 0,
                  "access_token": chatgpt_access("first@example.com", "acct-1", "pro") },
            ],
            "openrouter": [
                { "id": "k1", "label": "OPENROUTER_API_KEY", "auth_type": "api_key", "priority": 0,
                  "access_token": "sk-or" },
            ],
        }
    })
}

#[tokio::test]
async fn hermes_lists_its_own_pool_read_only() {
    let server = MockServer::start(|_, path, _| match path {
        "/api/oauth/account" => (
            200,
            r#"{"user":{"email":"nous@example.com"},"subscription":{"plan":"plus","monthly_credits":100,"credits_remaining":25,"current_period_end":"2030-02-01T00:00:00Z"}}"#.into(),
        ),
        "/backend-api/wham/usage" => (
            200,
            r#"{"rate_limit":{"primary_window":{"used_percent":10,"limit_window_seconds":18000}}}"#.into(),
        ),
        _ => (404, String::new()),
    })
    .await;
    let tmp = tempfile::tempdir().unwrap();
    let (accounts, config) = accounts_with(tmp.path(), mocked(&server.base));
    let auth = hermes_auth().to_string().replace("PORTAL", &server.base);
    let file = config.hermes_home.join("auth.json");
    write(&file, &auth);
    let hermes = rows(&accounts.list(true).await.unwrap(), HarnessId::Hermes);
    let order: Vec<_> = hermes.iter().map(|a| a.email.clone().unwrap()).collect();
    // The provider in use first, by priority; then the rest.
    assert_eq!(
        order,
        [
            "first@example.com",
            "second@example.com",
            "nous@example.com",
            "OPENROUTER_API_KEY"
        ]
    );
    assert!(hermes[0].active && !hermes[1].active && !hermes[2].active);
    assert!(hermes.iter().all(|a| !a.switchable && a.saved_at.is_none()));
    assert_eq!(hermes[0].plan_label.as_deref(), Some("ChatGPT Pro"));
    assert!((hermes[0].usage_windows[0].used_fraction - 0.1).abs() < 1e-6);
    let nous = &hermes[2];
    assert_eq!(nous.plan_label.as_deref(), Some("Nous Plus"));
    assert!((nous.usage_windows[0].used_fraction - 0.75).abs() < 1e-6);
    assert_eq!(
        hermes[3].usage_error.as_deref(),
        Some("API keys have no plan usage")
    );
    // zeron never writes Hermes' pool: no switch, no forget, no slot files.
    assert!(
        accounts
            .activate(HarnessId::Hermes, &hermes[1].id)
            .await
            .is_err()
    );
    assert!(
        accounts
            .forget(HarnessId::Hermes, &hermes[1].id)
            .await
            .is_err()
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), auth);
    assert!(accounts.read_slots(HarnessId::Hermes).is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn hermes_add_account_runs_hermes_auth_add_and_relays_the_device_code() {
    let tmp = tempfile::tempdir().unwrap();
    let (accounts, config) = accounts_with(tmp.path(), ProbeEndpoints::default());
    let cli = script(
        tmp.path(),
        "hermes",
        r#"#!/bin/sh
[ "$*" = "auth add openai-codex --type oauth --no-browser" ] || { echo "bad args: $*" >&2; exit 3; }
case "$HERMES_SHARED_AUTH_DIR" in *".login-"*) ;; *) exit 4 ;; esac
printf 'To continue, follow these steps:\n\n  1. Open this URL in your browser:\n     \033[94mhttps://auth.openai.com/codex/device\033[0m\n\n  2. Enter this code:\n     \033[94mQRST-5678\033[0m\n\nWaiting for sign-in...\n'
sleep 1
mkdir -p "$HERMES_HOME"
printf '{"active_provider":"openai-codex","credential_pool":{"openai-codex":[{"id":"abc123","label":"new@example.com","auth_type":"oauth","priority":0,"access_token":"x"}]}}' > "$HERMES_HOME/auth.json"
echo 'Added openai-codex OAuth credential #1: "new@example.com"'
"#,
    );
    accounts.override_cli(HarnessId::Hermes, cli);
    let start = accounts.start_login(HarnessId::Hermes).await.unwrap();
    assert_eq!(start.url, "https://auth.openai.com/codex/device");
    let polls = settle(&accounts, &start.login_id).await;
    assert_eq!(
        polls.last().unwrap().status,
        AgentLoginStatus::Done,
        "{polls:?}"
    );
    assert!(
        polls
            .iter()
            .any(|p| p.message.as_deref() == Some("Enter the code QRST-5678 when asked."))
    );
    let hermes = rows(&accounts.list(false).await.unwrap(), HarnessId::Hermes);
    assert_eq!(hermes.len(), 1);
    assert_eq!(hermes[0].email.as_deref(), Some("new@example.com"));
    assert!(config.hermes_home.join("auth.json").exists());
    // Only the device-code providers are offered.
    assert!(
        accounts
            .start_login_with(HarnessId::Hermes, Some("anthropic"), None)
            .await
            .is_err()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_failed_cli_sign_in_reports_the_clis_last_words() {
    let tmp = tempfile::tempdir().unwrap();
    let (accounts, _) = accounts_with(tmp.path(), ProbeEndpoints::default());
    let cli = script(
        tmp.path(),
        "hermes",
        "#!/bin/sh\nprintf '\\033[31mDevice code request failed: 503\\033[0m\\n' >&2\nexit 1\n",
    );
    accounts.override_cli(HarnessId::Hermes, cli);
    let start = accounts.start_login(HarnessId::Hermes).await.unwrap();
    let polls = settle(&accounts, &start.login_id).await;
    let last = polls.last().unwrap();
    assert_eq!(last.status, AgentLoginStatus::Error);
    assert_eq!(
        last.message.as_deref(),
        Some("Device code request failed: 503")
    );
}

// ── output scanning + reasons ───────────────────────────────────────────────

#[test]
fn device_codes_and_urls_are_found_in_coloured_cli_output() {
    let hermes = "  1. Open: \u{1b}[94mhttps://portal.nousresearch.com/device?code=AB12-CD34\u{1b}[0m\n  2. If prompted, enter code: AB12-CD34\n";
    assert_eq!(scan_device_code(hermes).as_deref(), Some("AB12-CD34"));
    assert_eq!(
        scan_https_url(hermes, &["nousresearch.com"]).as_deref(),
        Some("https://portal.nousresearch.com/device?code=AB12-CD34")
    );
    let codex_style = "2. Enter this code:\n     \u{1b}[94mQRST-5678\u{1b}[0m\n";
    assert_eq!(scan_device_code(codex_style).as_deref(), Some("QRST-5678"));
    // A loopback login's output has no code; foreign hosts don't match.
    assert_eq!(
        scan_device_code("open https://auth.openai.com/oauth?x in your browser"),
        None
    );
    assert_eq!(
        scan_device_code("error code: 503 Service Unavailable"),
        None
    );
    assert_eq!(
        scan_https_url("see https://evil.example/x.ai", &["x.ai"]),
        None
    );
    // Grok and Hermes pages: vendor host on a label boundary, https, no
    // userinfo and no explicit port; a later good url is still found.
    for output in [
        "see https://evilx.ai/device",
        "see https://x.ai.evil.example/device",
        "see https://auth.x.ai@evil.example/device",
        "see https://user@auth.x.ai/device",
        "see https://auth.x.ai:8443/device",
        "see http://auth.x.ai/device",
    ] {
        assert_eq!(scan_grok_url(output), None, "{output}");
    }
    assert_eq!(
        scan_grok_url("see https://evil.example/ then https://accounts.x.ai/device?c=1.")
            .as_deref(),
        Some("https://accounts.x.ai/device?c=1")
    );
    assert_eq!(
        scan_hermes_url("go to https://nousresearch.com.evil.example/device"),
        None
    );
    assert_eq!(
        scan_hermes_url("go to https://portal.nousresearch.com/device").as_deref(),
        Some("https://portal.nousresearch.com/device")
    );
    assert_eq!(
        strip_ansi("\u{1b}]8;;https://a\u{7}link\u{1b}]8;;\u{7} \u{1b}[1mbold\u{1b}[0m"),
        "link bold"
    );
}

#[test]
fn usage_reasons_name_each_providers_vendor_and_cli() {
    let entry = UsageEntry::default();
    let reason = |harness, key: Option<&str>, active, error: ProbeError| {
        usage_error_message(harness, key, active, &error, &entry, 0).unwrap()
    };
    let limited = ProbeError::RateLimited {
        retry_after_secs: None,
    };
    let rejected = ProbeError::Unauthorized { status: 401 };
    assert_eq!(
        reason(HarnessId::Grok, None, true, limited.clone()),
        "Rate limited by xAI"
    );
    assert_eq!(
        reason(HarnessId::Opencode, Some("github-copilot"), false, limited),
        "Rate limited by GitHub"
    );
    assert_eq!(
        reason(HarnessId::Grok, None, true, rejected.clone()),
        "Session expired — it refreshes the next time grok runs"
    );
    assert_eq!(
        reason(HarnessId::Pi, Some("openai-codex"), false, rejected.clone()),
        "Session expired — switch to it to refresh"
    );
    assert_eq!(
        reason(HarnessId::Devin, None, true, rejected.clone()),
        "Signed out — sign in again"
    );
    assert_eq!(
        reason(HarnessId::Grok, None, false, rejected),
        "Signed out — sign in again"
    );
}

// ── review hardening: grok entries, endpoints, the secret writer ────────────

/// A multi-issuer `auth.json`: a switch merges only the slot's issuer entry
/// back, leaving every other issuer's entry — including tokens refreshed
/// after the snapshot — exactly as the CLI left them.
#[tokio::test]
async fn a_grok_switch_leaves_other_issuers_entries_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let (accounts, config) = accounts_with(tmp.path(), ProbeEndpoints::default());
    let live = config.grok_home.join("auth.json");
    let with_sso = |auth: serde_json::Value, sso_key: &str| {
        let mut auth = auth;
        auth.as_object_mut().unwrap().insert(
            "https://sso.corp.example::cli".into(),
            serde_json::json!({ "key": sso_key, "oidc_issuer": "https://sso.corp.example" }),
        );
        auth.to_string()
    };
    write(
        &live,
        &with_sso(grok_auth("user-a", "a@x.ai", "key-a"), "sso-1"),
    );
    let a_id = rows(&accounts.list(false).await.unwrap(), HarnessId::Grok)[0]
        .id
        .clone();
    write(
        &live,
        &with_sso(grok_auth("user-b", "b@x.ai", "key-b"), "sso-1"),
    );
    accounts.list(false).await.unwrap();
    // The CLI refreshes the other issuer after the snapshots were taken.
    write(
        &live,
        &with_sso(grok_auth("user-b", "b@x.ai", "key-b"), "sso-2-refreshed"),
    );
    accounts.activate(HarnessId::Grok, &a_id).await.unwrap();
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&live).unwrap()).unwrap();
    assert_eq!(written["https://auth.x.ai::client-1"]["key"], "key-a");
    assert_eq!(
        written["https://sso.corp.example::cli"]["key"],
        "sso-2-refreshed"
    );
    assert_eq!(written.as_object().unwrap().len(), 2);
    // Rows don't form per-issuer groups: one Grok login is live at a time.
    let grok = rows(&accounts.list(false).await.unwrap(), HarnessId::Grok);
    assert!(grok.iter().all(|a| a.provider.is_none()));
}

#[test]
fn credential_defined_endpoints_must_be_the_vendors_own_https_hosts() {
    let ok = |raw: &str, policy| trusted_base(raw, policy, false).map(|u| u.to_string());
    assert_eq!(
        ok("https://auth.x.ai", GROK_ISSUERS).as_deref(),
        Some("https://auth.x.ai/")
    );
    assert!(ok("https://auth.x.ai/", GROK_ISSUERS).is_some());
    assert!(ok("https://server.codeium.com", DEVIN_SERVERS).is_some());
    assert!(ok("https://inference.codeium.com", DEVIN_SERVERS).is_some());
    assert!(ok("https://portal.nousresearch.com", NOUS_PORTALS).is_some());
    for raw in [
        "http://auth.x.ai",               // not https
        "https://auth.x.ai.evil.example", // look-alike suffix
        "https://evilx.ai",               // not a subdomain
        "https://user:pw@auth.x.ai",      // userinfo
        "https://auth.x.ai?next=evil",    // query
        "https://auth.x.ai#frag",         // fragment
        "https://auth.x.ai/oauth2/token", // a path, not a base
        "https://auth.x.ai:8443",         // explicit port
        "https://1.2.3.4",                // IP literal
        "auth.x.ai",                      // no scheme for a vendor url
        "file:///etc/passwd",
        "http://127.0.0.1:9", // loopback only in tests
    ] {
        assert_eq!(ok(raw, GROK_ISSUERS), None, "{raw}");
    }
    assert!(trusted_base("http://127.0.0.1:9", GROK_ISSUERS, true).is_some());
    // GitHub Enterprise Cloud: `<tenant>.ghe.com` only, https only.
    let ghe = |raw: &str| {
        copilot_api_base(
            &serde_json::json!({ "enterpriseUrl": raw }),
            "https://api.github.com",
            false,
        )
    };
    assert_eq!(
        ghe("company.ghe.com").as_deref(),
        Some("https://api.company.ghe.com")
    );
    assert_eq!(
        ghe("https://company.ghe.com/").as_deref(),
        Some("https://api.company.ghe.com")
    );
    for raw in [
        "company.ghe.com/x",
        "http://company.ghe.com",
        "evil@company.ghe.com",
        "localhost",
        "company.ghe.com?x=1",
        "10.0.0.1",
        // Any plain host outside ghe.com — the review's case: the file
        // alone must never pick where the GitHub token goes.
        "evil.example",
        "https://evil.example",
        // Self-hosted GHES: skipped (its REST root is /api/v3, and the
        // host isn't one zeron can vouch for).
        "github.company.com",
        // Look-alikes and nesting.
        "ghe.com",
        "company.ghe.com.evil.example",
        "a.b.ghe.com",
    ] {
        assert_eq!(ghe(raw), None, "{raw}");
    }
    assert_eq!(
        copilot_api_base(&serde_json::json!({}), "https://api.github.com", false).as_deref(),
        Some("https://api.github.com")
    );
}

/// Devin's sign-in page is opened on the requesting device: only a
/// Devin / Windsurf / Codeium https page of the expected shape qualifies.
#[test]
fn devin_sign_in_urls_must_be_devins_own_pages() {
    for url in [
        "https://app.devin.ai/auth/cli/continue?redirect_uri=http%3A%2F%2F127.0.0.1%3A45678%2Fcallback&state=s",
        "https://app.devin.ai/auth/cli/continue?state=s",
        "https://windsurf.com/editor/signin?response_type=token&redirect_uri=http://localhost:1455/callback",
        "https://www.codeium.com/editor/signin",
        "https://devin.ai/auth/cli/?redirect_uri=http://[::1]:1455/cb",
    ] {
        assert!(devin_login_url(url), "{url}");
    }
    for url in [
        // The review's case: any https host used to pass.
        "https://evil.example/auth/cli/?redirect_uri=http://localhost:1455/callback",
        "https://evildevin.ai/auth/cli/continue",
        "https://app.devin.ai.evil.example/auth/cli/continue",
        "https://app.devin.ai@evil.example/auth/cli/continue",
        "https://user:pw@app.devin.ai/auth/cli/continue",
        "https://app.devin.ai:8443/auth/cli/continue",
        "http://app.devin.ai/auth/cli/continue",
        "https://1.2.3.4/auth/cli/continue",
        // Wrong page shapes.
        "https://app.devin.ai/?redirect_uri=http://localhost:1455/callback",
        "https://app.devin.ai/settings",
        "https://app.devin.ai/editor/signinx",
        "https://app.devin.ai/x/auth/cli/continue",
        // A redirect that isn't a loopback http callback.
        "https://app.devin.ai/auth/cli/continue?redirect_uri=https://evil.example/cb",
        "https://app.devin.ai/auth/cli/continue?redirect_uri=http://evil.example:1455/cb",
        "https://app.devin.ai/auth/cli/continue?redirect_uri=http://localhost/cb",
        "https://app.devin.ai/auth/cli/continue?redirect_uri=http://localhost:1455/cb&redirect_uri=https://evil.example/",
        "not a url",
    ] {
        assert!(!devin_login_url(url), "{url}");
    }
    // The log fallback applies the same rule.
    let tmp = tempfile::tempdir().unwrap();
    let logs = tmp.path().join("logs");
    write(
        &logs.join("devin.log"),
        "open https://evil.example/auth/cli/?redirect_uri=http://localhost:1455/callback\n\
         open https://app.devin.ai/auth/cli/continue?redirect_uri=http://127.0.0.1:1455/cb\n",
    );
    assert_eq!(
        recorded_devin_url(&tmp.path().join("browser-url"), &logs).as_deref(),
        Some("https://app.devin.ai/auth/cli/continue?redirect_uri=http://127.0.0.1:1455/cb")
    );
}

#[tokio::test]
async fn untrusted_endpoints_in_credentials_are_never_sent_a_secret() {
    let server = MockServer::start(|_, _, _| (200, "{}".into())).await;
    let tmp = tempfile::tempdir().unwrap();
    // Production endpoints: a loopback url in a credential is NOT trusted.
    let (accounts, config) = accounts_with(tmp.path(), ProbeEndpoints::default());
    write(
        &config.devin_credentials_file,
        &format!(
            "windsurf_api_key = \"k\"\napi_server_url = \"{}\"\n",
            server.base
        ),
    );
    write(
        &config.opencode_auth_file,
        &serde_json::json!({ "github-copilot": {
            "type": "oauth", "access": "gho_x", "refresh": "gho_x", "expires": 0,
            "enterpriseUrl": "evil.example/steal?x=1",
        }})
        .to_string(),
    );
    let hermes = serde_json::json!({ "active_provider": "nous", "credential_pool": { "nous": [
        { "id": "n1", "label": "n@example.com", "auth_type": "oauth", "priority": 0,
          "access_token": "t", "portal_base_url": "https://portal.nousresearch.com.evil.example" },
    ]}});
    write(&config.hermes_home.join("auth.json"), &hermes.to_string());
    let snapshot = accounts.list(true).await.unwrap();
    let devin = rows(&snapshot, HarnessId::Devin);
    assert_eq!(
        devin[0].usage_error.as_deref(),
        Some("Usage skipped — this login names a server zeron doesn't recognize")
    );
    let hermes = rows(&snapshot, HarnessId::Hermes);
    assert_eq!(hermes[0].usage_error, devin[0].usage_error);
    // The Copilot login is identified locally (no request to the host its
    // file names): switchable, labelled by that host, usage skipped.
    let copilot = rows(&snapshot, HarnessId::Opencode);
    assert!(copilot[0].switchable && copilot[0].active);
    assert_eq!(
        copilot[0].email.as_deref(),
        Some("GitHub Enterprise account · evil.example")
    );
    assert_eq!(copilot[0].usage_error, devin[0].usage_error);
    assert_eq!(
        lock(&server.hits).len(),
        0,
        "no request reached the planted server"
    );
}

/// Throwaway sign-in homes hold whatever a CLI writes (fresh tokens, in
/// modes the CLI picks): the accounts root and every login home are 0700
/// before a CLI starts — an older, looser root included — and Devin's
/// recording browser's url file is 0600.
#[cfg(unix)]
#[test]
fn the_accounts_root_and_sign_in_homes_are_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let loosen = |path: &Path| {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o775)).unwrap()
    };
    let tmp = tempfile::tempdir().unwrap();
    let config = AgentAccountsConfig::isolated(tmp.path());
    let root = config.root_dir();
    std::fs::create_dir_all(&root).unwrap();
    loosen(&root);
    let (accounts, _) = accounts_with(tmp.path(), ProbeEndpoints::default());
    assert_eq!(
        mode(&root),
        0o700,
        "an existing root is tightened at startup"
    );

    loosen(&root);
    let home = accounts.login_home("0123456789abcdef").unwrap();
    assert_eq!(mode(&root), 0o700, "tightened again before a sign-in");
    assert_eq!(mode(&home), 0o700);

    let devin = accounts.devin_login_dirs("fedcba9876543210").unwrap();
    for dir in [&devin.home, &devin.data, &devin.config, &devin.cache] {
        assert_eq!(mode(dir), 0o700, "{}", dir.display());
    }
    assert_eq!(mode(&devin.url_file), 0o600);
    assert_eq!(std::fs::read_to_string(&devin.url_file).unwrap(), "");

    // The recording browser appends to that file without loosening it, and
    // creates a missing one owner-only too.
    let browser = ensure_recording_browser(&root).unwrap();
    let missing = devin.home.join("fresh-url");
    for file in [&devin.url_file, &missing] {
        let status = std::process::Command::new(&browser)
            .arg("https://app.devin.ai/auth/cli/continue")
            .env("ZERON_LOGIN_URL_FILE", file)
            .status()
            .unwrap();
        assert!(status.success());
        assert_eq!(mode(file), 0o600, "{}", file.display());
        assert_eq!(
            std::fs::read_to_string(file).unwrap(),
            "https://app.devin.ai/auth/cli/continue\n"
        );
    }

    // Slot dirs are owner-only too.
    let slots = accounts.slots_dir(HarnessId::Grok).unwrap();
    assert_eq!(mode(&slots), 0o700);
}

/// A self-hosted GHES Copilot login (a host zeron never sends the token to)
/// still switches: keyed by a fingerprint of its token, snapshotted without
/// any network call, and restored byte-for-byte after switching away.
#[tokio::test]
async fn a_self_hosted_ghes_login_switches_away_and_back_without_the_network() {
    let server = MockServer::start(|_, path, _| match path {
        "/user" => (
            200,
            r#"{"id":42,"login":"octo","email":"octo@example.com"}"#.into(),
        ),
        _ => (404, String::new()),
    })
    .await;
    let tmp = tempfile::tempdir().unwrap();
    let (accounts, config) = accounts_with(tmp.path(), mocked(&server.base));
    let file = &config.opencode_auth_file;
    let ghes = serde_json::json!({
        "type": "oauth", "access": "ghes-session", "refresh": "ghes-github-token",
        "expires": 0, "enterpriseUrl": "github.company.com",
    });
    let dotcom = serde_json::json!({
        "type": "oauth", "access": "dotcom-session", "refresh": "gho_dotcom", "expires": 0,
    });
    let store = |copilot: &serde_json::Value| {
        serde_json::json!({ "github-copilot": copilot, "zen": { "type": "api", "key": "k" } })
            .to_string()
    };
    // github.com login first (identified over the mocked API), then GHES.
    write(file, &store(&dotcom));
    let dotcom_id = rows(&accounts.list(false).await.unwrap(), HarnessId::Opencode)[0]
        .id
        .clone();
    write(file, &store(&ghes));
    let listed = rows(&accounts.list(true).await.unwrap(), HarnessId::Opencode);
    let ghes_row = listed
        .iter()
        .find(|a| a.email.as_deref() == Some("GitHub Enterprise account · github.company.com"))
        .expect("GHES login listed");
    assert!(ghes_row.active && ghes_row.switchable);
    assert_eq!(
        ghes_row.usage_error.as_deref(),
        Some("Usage skipped — this login names a server zeron doesn't recognize")
    );
    let ghes_id = ghes_row.id.clone();
    // Keyed by a fingerprint — the token itself is never the account key.
    let slot = accounts.read_slot(HarnessId::Opencode, &ghes_id).unwrap();
    assert!(
        slot.account_key.starts_with("github-copilot:enterprise:"),
        "{}",
        slot.account_key
    );
    assert!(!slot.account_key.contains("ghes-github-token"));

    // Switch away to the github.com login…
    accounts
        .activate(HarnessId::Opencode, &dotcom_id)
        .await
        .unwrap();
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
    assert_eq!(written["github-copilot"]["refresh"], "gho_dotcom");
    assert_eq!(written["zen"]["key"], "k");
    // …and back: the GHES entry is restored exactly.
    accounts
        .activate(HarnessId::Opencode, &ghes_id)
        .await
        .unwrap();
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
    assert_eq!(written["github-copilot"], ghes);
    let listed = rows(&accounts.list(false).await.unwrap(), HarnessId::Opencode);
    assert_eq!(listed.len(), 2);
    assert!(listed.iter().any(|a| a.id == ghes_id && a.active));
    // Only the github.com login was ever looked up; no request carried the
    // GHES token anywhere (its usage was skipped before any request).
    assert_eq!(server.hits("GET /user"), 1);
    let hits = lock(&server.hits).clone();
    assert!(
        hits.iter()
            .all(|h| h == "GET /user" || h.starts_with("GET /copilot_internal")),
        "{hits:?}"
    );
}

/// The secret writer: random exclusive temp file, 0600 before any byte,
/// renamed over the target (replacing a symlink rather than writing through
/// it), and nothing left behind.
#[cfg(unix)]
#[test]
fn secret_writes_are_exclusive_owner_only_and_never_follow_symlinks() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let target = dir.join("auth.json");
    std::fs::write(&target, "old").unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).unwrap();
    // The old predictable temp name, pre-planted by someone else.
    let planted = dir.join(format!("auth.tmp-{}", std::process::id()));
    std::fs::write(&planted, "planted").unwrap();
    std::fs::set_permissions(&planted, std::fs::Permissions::from_mode(0o666)).unwrap();
    write_file_atomic(&target, b"secret", true).unwrap();
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "secret");
    assert_eq!(
        mode(&target),
        0o600,
        "narrowed even though the target was 0644"
    );
    assert_eq!(std::fs::read_to_string(&planted).unwrap(), "planted");
    // A symlink at the target is replaced, never written through.
    let victim = dir.join("victim");
    std::fs::write(&victim, "keep").unwrap();
    let link = dir.join("link.json");
    std::os::unix::fs::symlink(&victim, &link).unwrap();
    write_file_atomic(&link, b"secret", true).unwrap();
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep");
    assert!(
        !std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(mode(&link), 0o600);
    // No temp files survive a write.
    let names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(names.iter().all(|n| !n.ends_with(".tmp")), "{names:?}");
    // Non-secret writes keep the target's mode.
    let config = dir.join("config.json");
    std::fs::write(&config, "{}").unwrap();
    std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o640)).unwrap();
    write_file_atomic(&config, b"{\"a\":1}", false).unwrap();
    assert_eq!(mode(&config), 0o640);
}

/// OpenCode takes no lock of its own: zeron serialises its writers on a
/// sidecar lock (released after the write) and never leaves it held.
#[test]
fn opencode_writes_hold_a_zeron_side_lock_only_while_writing() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("auth.json");
    let lock = tmp.path().join("auth.json.zeron-lock");
    std::fs::write(&file, r#"{"other":{"type":"api","key":"k"}}"#).unwrap();
    merge_json_entry(
        &file,
        "openai",
        Some(&serde_json::json!({ "type": "oauth", "access": "a" })),
        StoreLock::File(lock.clone()),
    )
    .unwrap();
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(written["other"]["key"], "k");
    assert_eq!(written["openai"]["access"], "a");
    // Released: another writer can take it at once.
    let held = FileLock::acquire(&lock).unwrap();
    drop(held);
}

/// A failing CLI's last words reach the dialog without the authorize url's
/// parameters, the device code or anything token-shaped.
#[cfg(unix)]
#[tokio::test]
async fn a_failed_sign_ins_output_is_redacted_before_the_ui_sees_it() {
    let tmp = tempfile::tempdir().unwrap();
    let (accounts, _) = accounts_with(tmp.path(), ProbeEndpoints::default());
    let cli = script(
        tmp.path(),
        "grok",
        "#!/bin/sh\necho 'failed at https://auth.x.ai/device?user_code=WXYZ-9876&state=s3cr3t with code WXYZ-9876 token=eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.sig' >&2\nexit 2\n",
    );
    accounts.override_cli(HarnessId::Grok, cli);
    let start = accounts.start_login(HarnessId::Grok).await.unwrap();
    let polls = settle(&accounts, &start.login_id).await;
    let message = polls.last().unwrap().message.clone().unwrap();
    assert_eq!(
        message,
        "failed at https://auth.x.ai/device?… with code [code] token=[redacted]"
    );
}
