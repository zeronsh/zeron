//! Pins against real accounts on disk: what a pinned chat's run gets, what
//! stays as it was, and the hook that decides a chat's first run.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::super::muse::OWN_LOGIN;
use super::super::*;
use crate::registry::RunEnvironment;

const TOKEN_ENV: &str = "CLAUDE_CODE_OAUTH_TOKEN";

fn config(root: &Path) -> AgentAccountsConfig {
    AgentAccountsConfig {
        data_dir: root.join("data"),
        claude_config_dir: root.join("claude"),
        claude_config_file: root.join("claude.json"),
        codex_home: root.join("codex"),
        cursor_sdk_auth_file: root.join("cursor-sdk").join("auth.json"),
        claude_keychain_service: None,
        antigravity_home: Some(root.join("gemini")),
        antigravity_keychain: false,
        ..AgentAccountsConfig::isolated(root)
    }
}

fn write_live_claude(root: &Path, uuid: &str, email: &str) {
    let config = config(root);
    std::fs::create_dir_all(&config.claude_config_dir).unwrap();
    std::fs::write(
        &config.claude_config_file,
        serde_json::json!({ "oauthAccount": { "accountUuid": uuid, "emailAddress": email } }).to_string(),
    )
    .unwrap();
    std::fs::write(
        config.claude_creds_file(),
        serde_json::json!({ "claudeAiOauth": { "accessToken": "live-token", "refreshToken": "live-rt", "expiresAt": now_ms() + 3_600_000 } })
            .to_string(),
    )
    .unwrap();
}

fn claude_slot(uuid: &str, email: &str, access: &str, expires_at: i64) -> Slot {
    Slot {
        id: slot_id_for(HarnessId::ClaudeCode, uuid),
        harness: HarnessId::ClaudeCode,
        account_key: uuid.into(),
        profile: SlotProfile {
            email: email.into(),
            display_name: None,
            organization: None,
            plan: None,
            auth_kind: AgentAuthKind::Oauth,
        },
        credentials: serde_json::json!({ "claudeAiOauth": {
            "accessToken": access, "refreshToken": format!("rt-{access}"), "expiresAt": expires_at,
        }}),
        claude_config: Some(serde_json::json!({ "oauthAccount": { "accountUuid": uuid, "emailAddress": email } })),
        saved_at: now_ms(),
        created_at: None,
        store_key: None,
    }
}

fn request() -> zeron_proto::RunRequest {
    serde_json::from_str(
        r#"{"prompt":"p","model":null,"reasoning":null,"cwd":".","sandbox":"workspace-write","resume":null}"#,
    )
    .unwrap()
}

/// A device with the live login `acct-a` and a saved login `acct-b`.
struct Device {
    _tmp: tempfile::TempDir,
    accounts: AgentAccounts,
    live: String,
    saved: String,
}

fn device() -> Device {
    let tmp = tempfile::tempdir().unwrap();
    let accounts = AgentAccounts::new(config(tmp.path()));
    write_live_claude(tmp.path(), "acct-a", "a@example.com");
    let live = claude_slot("acct-a", "a@example.com", "live-token", now_ms() + 3_600_000);
    let saved = claude_slot("acct-b", "b@example.com", "at-b", now_ms() + 3_600_000);
    accounts.write_slot(&live).unwrap();
    accounts.write_slot(&saved).unwrap();
    Device { _tmp: tmp, accounts, live: live.id, saved: saved.id }
}

fn row(id: &str, kind: AgentAuthKind) -> AgentAccount {
    serde_json::from_value(serde_json::json!({
        "id": id, "harness": "claude-code", "email": null, "planLabel": null, "active": false,
        "authKind": match kind { AgentAuthKind::Oauth => "oauth", AgentAuthKind::ApiKey => "api-key" },
        "switchable": true,
    }))
    .unwrap()
}

/// Answers every request with `status`/`body`, counting them.
async fn fixed_server(status: &'static str, body: &'static str) -> (String, Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let mut request = vec![0u8; 16 * 1024];
            let _ = socket.read(&mut request).await;
            counter.fetch_add(1, Ordering::SeqCst);
            let reply = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(reply.as_bytes()).await;
        }
    });
    (url, hits)
}

async fn prepared(env: &PinRunEnvironment, chat: &str) -> Result<zeron_proto::RunRequest, String> {
    let mut request = request();
    env.prepare(HarnessId::ClaudeCode, Some(chat), &mut request).await?;
    Ok(request)
}

#[tokio::test]
async fn a_pinned_claude_chat_runs_on_its_saved_accounts_token_and_the_live_login_stays() {
    let d = device();
    let env = PinRunEnvironment(d.accounts.clone());
    d.accounts.set_pin(HarnessId::ClaudeCode, "c1", Some(&d.saved));
    let live_before = std::fs::read(config(d._tmp.path()).claude_creds_file()).unwrap();
    let run = prepared(&env, "c1").await.unwrap();
    assert_eq!(run.env.get(TOKEN_ENV).map(String::as_str), Some("at-b"));
    assert_eq!(std::fs::read(config(d._tmp.path()).claude_creds_file()).unwrap(), live_before, "the live login is never touched");
    // Another chat, and a run of no chat, are as they always were.
    assert!(prepared(&env, "c2").await.unwrap().env.is_empty());
    let mut titles = request();
    env.prepare(HarnessId::ClaudeCode, None, &mut titles).await.unwrap();
    assert!(titles.env.is_empty());
    // Never over what another provider set.
    let mut set = request();
    set.env.insert(TOKEN_ENV.into(), "broker".into());
    env.prepare(HarnessId::ClaudeCode, Some("c1"), &mut set).await.unwrap();
    assert_eq!(set.env[TOKEN_ENV], "broker");
}

#[tokio::test]
async fn a_chat_pinned_to_the_live_login_or_a_gone_account_runs_as_without_a_pin() {
    let d = device();
    let env = PinRunEnvironment(d.accounts.clone());
    d.accounts.set_pin(HarnessId::ClaudeCode, "live", Some(&d.live));
    d.accounts.set_pin(HarnessId::ClaudeCode, "gone", Some("no-such-account"));
    for chat in ["live", "gone"] {
        assert!(prepared(&env, chat).await.unwrap().env.is_empty(), "{chat}");
        assert_eq!(env.credential(HarnessId::ClaudeCode, Some(chat)).await.as_deref(), Some("live"), "{chat}");
    }
}

#[tokio::test]
async fn the_run_credential_follows_the_pin_so_a_parked_session_is_replaced() {
    let d = device();
    let env = PinRunEnvironment(d.accounts.clone());
    // Never pinned: unknown, so nothing is replaced.
    assert_eq!(env.credential(HarnessId::ClaudeCode, Some("c1")).await, None);
    d.accounts.set_pin(HarnessId::ClaudeCode, "c1", Some(&d.saved));
    let pinned = env.credential(HarnessId::ClaudeCode, Some("c1")).await.expect("a credential");
    assert!(pinned.starts_with("claude:"), "{pinned}");
    assert_eq!(env.credential(HarnessId::ClaudeCode, Some("c1")).await, Some(pinned.clone()), "stable");
    // A token Zeron has since refreshed is another credential.
    let mut slot = d.accounts.read_slot(HarnessId::ClaudeCode, &d.saved).unwrap();
    slot.credentials["claudeAiOauth"]["accessToken"] = "at-b2".into();
    d.accounts.write_slot(&slot).unwrap();
    assert_ne!(env.credential(HarnessId::ClaudeCode, Some("c1")).await, Some(pinned));
    // A pin taken back moves it to the live login.
    d.accounts.set_pin(HarnessId::ClaudeCode, "c1", None);
    assert_eq!(env.credential(HarnessId::ClaudeCode, Some("c1")).await.as_deref(), Some("live"));
    assert!(prepared(&env, "c1").await.unwrap().env.is_empty());
    assert!(d.accounts.pins().is_empty(), "a taken-back pin isn't listed");
    // A chat never pinned stays unknown after another's pin goes.
    assert_eq!(env.credential(HarnessId::ClaudeCode, Some("c9")).await, None);
}

#[tokio::test]
async fn a_token_about_to_expire_is_refreshed_first_and_kept_in_its_slot() {
    let tmp = tempfile::tempdir().unwrap();
    let (token_url, hits) =
        fixed_server("200 OK", r#"{"access_token":"fresh","refresh_token":"rt-new","expires_in":3600}"#).await;
    let endpoints = ProbeEndpoints { claude_token: token_url, ..Default::default() };
    let accounts = AgentAccounts::with_endpoints(config(tmp.path()), endpoints, Default::default());
    write_live_claude(tmp.path(), "acct-a", "a@example.com");
    let saved = claude_slot("acct-b", "b@example.com", "at-b", now_ms() + 60_000);
    accounts.write_slot(&saved).unwrap();
    accounts.set_pin(HarnessId::ClaudeCode, "c1", Some(&saved.id));
    let env = PinRunEnvironment(accounts.clone());
    let run = prepared(&env, "c1").await.unwrap();
    assert_eq!(run.env[TOKEN_ENV], "fresh");
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    let slot = accounts.read_slot(HarnessId::ClaudeCode, &saved.id).unwrap();
    assert_eq!(slot.credentials["claudeAiOauth"]["refreshToken"], "rt-new", "the slot holds the rotated pair");
    // A second run finds a token with time left: no second refresh.
    assert_eq!(prepared(&env, "c1").await.unwrap().env[TOKEN_ENV], "fresh");
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_spent_login_stops_the_run_and_says_where_to_sign_in() {
    let tmp = tempfile::tempdir().unwrap();
    let (token_url, _) = fixed_server("400 Bad Request", r#"{"error":"invalid_grant"}"#).await;
    let endpoints = ProbeEndpoints { claude_token: token_url, ..Default::default() };
    let accounts = AgentAccounts::with_endpoints(config(tmp.path()), endpoints, Default::default());
    write_live_claude(tmp.path(), "acct-a", "a@example.com");
    let spent = claude_slot("acct-b", "b@example.com", "at-b", now_ms() - 60_000);
    accounts.write_slot(&spent).unwrap();
    accounts.set_pin(HarnessId::ClaudeCode, "c1", Some(&spent.id));
    let err = prepared(&PinRunEnvironment(accounts), "c1").await.unwrap_err();
    assert!(err.contains("b@example.com") && err.contains("signing in again"), "{err}");
}

#[tokio::test]
async fn pins_survive_a_restart() {
    let d = device();
    d.accounts.set_pin(HarnessId::ClaudeCode, "c1", Some(&d.saved));
    d.accounts.set_pin(HarnessId::ClaudeCode, "c2", Some(&d.live));
    d.accounts.set_pin(HarnessId::ClaudeCode, "c2", None);
    let again = AgentAccounts::new(config(d._tmp.path()));
    let pins = again.pins();
    assert_eq!(pins.len(), 1, "{pins:?}");
    assert_eq!((pins[0].chat.as_str(), pins[0].account.as_str()), ("c1", d.saved.as_str()));
    assert_eq!(
        PinRunEnvironment(again).credential(HarnessId::ClaudeCode, Some("c2")).await.as_deref(),
        Some("live"),
        "a taken-back pin is remembered too"
    );
}

#[tokio::test]
async fn a_chats_first_run_asks_the_hook_once_and_a_pinned_chat_isnt_asked() {
    let d = device();
    let env = PinRunEnvironment(d.accounts.clone());
    let asked = Arc::new(AtomicUsize::new(0));
    let (counter, saved) = (asked.clone(), d.saved.clone());
    d.accounts.set_pin_hook(Arc::new(move |harness, chat| {
        counter.fetch_add(1, Ordering::SeqCst);
        let answer = (harness == HarnessId::ClaudeCode && chat == "c1").then(|| saved.clone());
        Box::pin(async move { answer })
    }));
    // The hook answers an account: the chat is pinned there (the hook pins
    // through the plugins' `pin`; here a stand-in that does not, so the run
    // itself still gets the account's token).
    d.accounts.set_pin(HarnessId::ClaudeCode, "c1", Some(&d.saved));
    assert_eq!(prepared(&env, "c1").await.unwrap().env[TOKEN_ENV], "at-b");
    assert_eq!(asked.load(Ordering::SeqCst), 0, "a pinned chat isn't asked");
    // A chat with no pin is asked; an answer of nothing leaves it on the live login.
    assert!(prepared(&env, "c2").await.unwrap().env.is_empty());
    assert_eq!(asked.load(Ordering::SeqCst), 1);
    // Another harness, or a run of no chat, is never asked.
    let mut other = request();
    env.prepare(HarnessId::Codex, Some("c2"), &mut other).await.unwrap();
    env.prepare(HarnessId::ClaudeCode, None, &mut other).await.unwrap();
    assert_eq!(asked.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn the_hooks_account_runs_the_chat_when_nothing_pinned_it_yet() {
    let d = device();
    let env = PinRunEnvironment(d.accounts.clone());
    let saved = d.saved.clone();
    let accounts = d.accounts.clone();
    d.accounts.set_pin_hook(Arc::new(move |harness, chat| {
        // As the plugins' `preparing` does: answer, and pin the chat there.
        accounts.set_pin(harness, &chat, Some(&saved));
        let saved = saved.clone();
        Box::pin(async move { Some(saved) })
    }));
    assert_eq!(prepared(&env, "c1").await.unwrap().env[TOKEN_ENV], "at-b");
    assert_eq!(d.accounts.pins()[0].chat, "c1");
}

#[tokio::test]
async fn only_oauth_logins_of_a_harness_with_a_mechanism_can_be_pinned() {
    let d = device();
    assert_eq!(d.accounts.pin_mechanism(&row(&d.saved, AgentAuthKind::Oauth)), Some("env"));
    assert_eq!(d.accounts.pin_mechanism(&row(&d.saved, AgentAuthKind::ApiKey)), None, "an API key bills per token");
    assert_eq!(d.accounts.pin_mechanism(&row("not-saved", AgentAuthKind::Oauth)), None);
    let mut codex = row(&d.saved, AgentAuthKind::Oauth);
    codex.harness = HarnessId::Codex;
    assert_eq!(d.accounts.pin_mechanism(&codex), None, "no per-chat mechanism yet");
}

#[cfg(unix)]
#[tokio::test]
async fn a_pinned_muse_chat_runs_on_its_accounts_own_config_root() {
    let tmp = tempfile::tempdir().unwrap();
    let config = config(tmp.path());
    std::fs::create_dir_all(config.muse_config_root.join("muse")).unwrap();
    let accounts = AgentAccounts::new(config);
    let slot = Slot {
        id: "m1".into(),
        harness: HarnessId::Muse,
        account_key: "m1".into(),
        profile: SlotProfile {
            email: "m@example.com".into(),
            display_name: None,
            organization: None,
            plan: None,
            auth_kind: AgentAuthKind::Oauth,
        },
        credentials: serde_json::json!({}),
        claude_config: None,
        saved_at: now_ms(),
        created_at: None,
        store_key: None,
    };
    accounts.write_slot(&slot).unwrap();
    let env = PinRunEnvironment(accounts.clone());
    let mut row = row("m1", AgentAuthKind::Oauth);
    row.harness = HarnessId::Muse;
    assert_eq!(accounts.pin_mechanism(&row), Some("root"));
    accounts.set_pin(HarnessId::Muse, "c1", Some("m1"));
    let mut run = request();
    env.prepare(HarnessId::Muse, Some("c1"), &mut run).await.unwrap();
    let root = run.env.get("XDG_CONFIG_HOME").expect("its own root");
    assert!(root.contains("m1"), "{root}");
    assert_eq!(env.credential(HarnessId::Muse, Some("c1")).await.as_deref(), Some("m1"));
    // A Muse chat that isn't pinned follows the selected account, as before.
    assert_eq!(env.credential(HarnessId::Muse, Some("c2")).await.as_deref(), Some(OWN_LOGIN));
}
