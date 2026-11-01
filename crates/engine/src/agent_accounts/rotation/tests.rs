use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use base64::Engine as _;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::super::{
    AgentAccountsConfig, ProbeEndpoints, SlotProfile, UsageSnapshot, http_response, slot_id_for,
};
use super::*;
use crate::registry::{HarnessRegistry, RunEnvironment};

const HOUR: i64 = 3_600_000;
const REQUEST: &str = r#"{"prompt":"p","model":null,"reasoning":null,"cwd":".","sandbox":"workspace-write","resume":null}"#;

fn candidate(id: &str, usable: bool, used: Option<f32>) -> Candidate {
    Candidate {
        id: id.into(),
        usable,
        used,
    }
}

fn ids(candidates: &[Candidate], at: Option<usize>) -> Option<&str> {
    at.map(|at| candidates[at].id.as_str())
}

// ── selection rule ──────────────────────────────────────────────────────────

#[test]
fn picks_round_robin_after_the_cursor() {
    let all = [
        candidate("a", true, None),
        candidate("b", true, Some(0.2)),
        candidate("c", true, None),
    ];
    assert_eq!(ids(&all, pick(&all, None)), Some("a"));
    assert_eq!(ids(&all, pick(&all, Some("a"))), Some("b"));
    assert_eq!(ids(&all, pick(&all, Some("b"))), Some("c"));
    assert_eq!(ids(&all, pick(&all, Some("c"))), Some("a"));
    // A cursor naming a forgotten account starts over.
    assert_eq!(ids(&all, pick(&all, Some("gone"))), Some("a"));
}

#[test]
fn skips_signed_out_and_exhausted_accounts_while_another_has_room() {
    let all = [
        candidate("a", true, None),
        candidate("b", false, None),
        candidate("c", true, Some(0.97)),
        candidate("d", true, Some(0.5)),
    ];
    assert_eq!(ids(&all, pick(&all, Some("a"))), Some("d"));
    assert_eq!(ids(&all, pick(&all, Some("d"))), Some("a"));
}

#[test]
fn every_account_exhausted_picks_the_most_headroom() {
    let all = [
        candidate("a", true, Some(1.0)),
        candidate("b", true, Some(0.96)),
        candidate("c", false, Some(0.0)),
        candidate("d", true, Some(0.99)),
    ];
    assert_eq!(ids(&all, pick(&all, None)), Some("b"));
    assert_eq!(ids(&all, pick(&all, Some("b"))), Some("b"));
}

#[test]
fn nothing_usable_picks_nothing() {
    assert_eq!(pick(&[], None), None);
    let all = [candidate("a", false, None), candidate("b", false, None)];
    assert_eq!(pick(&all, None), None);
}

#[test]
fn a_window_that_already_reset_is_not_binding() {
    let now = chrono::Utc::now();
    let window = |used, resets_in: Option<i64>| AgentUsageWindow {
        label: "w".into(),
        used_fraction: used,
        resets_at: resets_in.map(|h| now + chrono::Duration::hours(h)),
    };
    let ms = now.timestamp_millis();
    assert_eq!(
        binding_usage(&[window(1.0, Some(-1)), window(0.3, Some(2))], ms),
        Some(0.3)
    );
    assert_eq!(
        binding_usage(&[window(0.4, None), window(0.9, Some(2))], ms),
        Some(0.9)
    );
    assert_eq!(binding_usage(&[window(1.0, Some(-1))], ms), None);
}

#[test]
fn state_round_trips_with_harness_keys() {
    let mut state = RotationState::default();
    state
        .harnesses
        .entry(HarnessId::ClaudeCode)
        .or_default()
        .enabled = true;
    state.chats.entry("chat".into()).or_default().insert(
        HarnessId::Codex,
        ChatAssignment {
            account_id: "a".into(),
            assigned_at: 1,
            used_at: 2,
            previous_account_id: None,
            moved_reason: None,
            history_bound: false,
            pools: Default::default(),
        },
    );
    let json = serde_json::to_string(&state).unwrap();
    assert!(json.contains("\"claude-code\""), "{json}");
    let back: RotationState = serde_json::from_str(&json).unwrap();
    assert_eq!(back, state);
}

// ── fixtures ────────────────────────────────────────────────────────────────

fn accounts_at(root: &Path) -> AgentAccounts {
    // Never a real login, never the network.
    AgentAccounts::with_endpoints(
        AgentAccountsConfig::isolated(root),
        ProbeEndpoints {
            allow_slot_refresh: false,
            ..Default::default()
        },
        Default::default(),
    )
}

fn request() -> zeron_proto::RunRequest {
    serde_json::from_str(REQUEST).unwrap()
}

fn write(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
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
            "accessToken": access,
            "refreshToken": format!("rt-{access}"),
            "expiresAt": expires_at,
        }}),
        claude_config: None,
        saved_at: now_ms(),
        created_at: None,
        store_key: None,
    }
}

/// `count` saved Claude logins, none of them live; their slot ids in
/// rotation order.
fn claude_accounts(accounts: &AgentAccounts, count: usize) -> Vec<String> {
    (0..count)
        .map(|i| {
            let slot = claude_slot(
                &format!("acct-{i}"),
                &format!("user{i}@example.com"),
                &format!("token-{i}"),
                now_ms() + 8 * HOUR,
            );
            accounts.write_slot(&slot).unwrap();
            slot.id
        })
        .collect()
}

/// Make `uuid` Claude Code's live login.
fn make_claude_live(root: &Path, uuid: &str, access: &str) {
    let config = AgentAccountsConfig::isolated(root);
    write(
        &config.claude_config_file,
        &serde_json::json!({ "oauthAccount": { "accountUuid": uuid } }).to_string(),
    );
    write(
        &config.claude_creds_file(),
        &serde_json::json!({ "claudeAiOauth": {
            "accessToken": access,
            "refreshToken": format!("rt-{access}"),
            "expiresAt": now_ms() + 8 * HOUR,
        }})
        .to_string(),
    );
}

fn jwt(claims: serde_json::Value) -> String {
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::to_vec(&claims).unwrap());
    format!("e30.{payload}.sig")
}

fn codex_auth(user: &str, access: &str, refresh: &str, last_refresh: &str) -> serde_json::Value {
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
        "last_refresh": last_refresh,
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

fn exhaust(accounts: &AgentAccounts, slot: &Slot) {
    lock(&accounts.inner.usage).insert(
        usage_key(slot.harness, &slot.account_key),
        UsageEntry {
            usage: Some(UsageSnapshot {
                windows: vec![AgentUsageWindow {
                    label: "5h".into(),
                    used_fraction: 1.0,
                    resets_at: Some(chrono::Utc::now() + chrono::Duration::hours(2)),
                }],
                plan_label: None,
            }),
            fetched_at: Some(now_ms()),
            ..Default::default()
        },
    );
}

async fn claude_token(env: &AccountsRunEnvironment, chat: &str) -> Option<String> {
    let mut request = request();
    env.prepare(HarnessId::ClaudeCode, Some(chat), &mut request)
        .await
        .unwrap();
    request.env.get("CLAUDE_CODE_OAUTH_TOKEN").cloned()
}

fn assigned(accounts: &AgentAccounts, chat: &str, harness: HarnessId) -> Option<String> {
    lock(&accounts.inner.rotation)
        .assignment(chat, harness, None)
        .map(|a| a.account_id.clone())
}

// ── assignment ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn rotation_is_off_by_default_and_a_single_account_never_rotates() {
    let tmp = tempfile::tempdir().unwrap();
    let accounts = accounts_at(tmp.path());
    let env = AccountsRunEnvironment(accounts.clone());
    claude_accounts(&accounts, 2);
    assert_eq!(claude_token(&env, "chat-1").await, None);
    assert_eq!(
        env.credential(HarnessId::ClaudeCode, Some("chat-1"))
            .await
            .as_deref(),
        Some(LIVE)
    );
    let snapshot = accounts.list(false).await.unwrap();
    let rotation = snapshot.rotation_for(HarnessId::ClaudeCode).unwrap();
    assert!(rotation.supported && !rotation.enabled && !rotation.active);

    let tmp = tempfile::tempdir().unwrap();
    let accounts = accounts_at(tmp.path());
    let env = AccountsRunEnvironment(accounts.clone());
    claude_accounts(&accounts, 1);
    let snapshot = accounts
        .set_rotation(HarnessId::ClaudeCode, true)
        .await
        .unwrap();
    let rotation = snapshot.rotation_for(HarnessId::ClaudeCode).unwrap();
    assert!(rotation.enabled && rotation.supported && !rotation.active);
    assert!(rotation.note.is_some());
    assert_eq!(claude_token(&env, "chat-1").await, None);
    assert_eq!(assigned(&accounts, "chat-1", HarnessId::ClaudeCode), None);
}

#[tokio::test]
async fn new_chats_spread_and_each_chat_keeps_its_account() {
    let tmp = tempfile::tempdir().unwrap();
    let accounts = accounts_at(tmp.path());
    let env = AccountsRunEnvironment(accounts.clone());
    let ids = claude_accounts(&accounts, 3);
    let snapshot = accounts
        .set_rotation(HarnessId::ClaudeCode, true)
        .await
        .unwrap();
    let rotation = snapshot.rotation_for(HarnessId::ClaudeCode).unwrap();
    assert!(rotation.active);
    assert_eq!(rotation.accounts, ids);
    assert_eq!(rotation.next_account_id.as_deref(), Some(ids[0].as_str()));

    for (chat, token) in [
        ("chat-1", "token-0"),
        ("chat-2", "token-1"),
        ("chat-3", "token-2"),
        ("chat-4", "token-0"),
        // Sticky: a chat's next run keeps its account.
        ("chat-2", "token-1"),
    ] {
        assert_eq!(
            claude_token(&env, chat).await.as_deref(),
            Some(token),
            "{chat}"
        );
    }

    let snapshot = accounts.list(false).await.unwrap();
    let rotation = snapshot.rotation_for(HarnessId::ClaudeCode).unwrap();
    assert_eq!(rotation.chats.len(), 4);
    assert_eq!(rotation.chat_count(&ids[0]), 2);
    assert_eq!(rotation.next_account_id.as_deref(), Some(ids[1].as_str()));
    assert_eq!(
        snapshot
            .chat_account("chat-3")
            .and_then(|a| a.email.as_deref()),
        Some("user2@example.com")
    );
}

#[tokio::test]
async fn the_cursor_and_assignments_survive_a_restart() {
    let tmp = tempfile::tempdir().unwrap();
    let ids = {
        let accounts = accounts_at(tmp.path());
        let ids = claude_accounts(&accounts, 3);
        accounts
            .set_rotation(HarnessId::ClaudeCode, true)
            .await
            .unwrap();
        let env = AccountsRunEnvironment(accounts.clone());
        claude_token(&env, "chat-1").await;
        claude_token(&env, "chat-2").await;
        ids
    };
    let accounts = accounts_at(tmp.path());
    let env = AccountsRunEnvironment(accounts.clone());
    assert_eq!(
        assigned(&accounts, "chat-1", HarnessId::ClaudeCode).as_deref(),
        Some(ids[0].as_str())
    );
    assert_eq!(
        claude_token(&env, "chat-3").await.as_deref(),
        Some("token-2")
    );
    assert_eq!(
        claude_token(&env, "chat-1").await.as_deref(),
        Some("token-0")
    );
}

#[tokio::test]
async fn new_chats_skip_an_exhausted_account() {
    let tmp = tempfile::tempdir().unwrap();
    let accounts = accounts_at(tmp.path());
    let env = AccountsRunEnvironment(accounts.clone());
    claude_accounts(&accounts, 3);
    let slots = accounts.read_slots(HarnessId::ClaudeCode);
    accounts
        .set_rotation(HarnessId::ClaudeCode, true)
        .await
        .unwrap();
    exhaust(&accounts, &slots[1]);
    assert_eq!(
        claude_token(&env, "chat-1").await.as_deref(),
        Some("token-0")
    );
    assert_eq!(
        claude_token(&env, "chat-2").await.as_deref(),
        Some("token-2")
    );
}

#[tokio::test]
async fn a_chat_on_a_forgotten_or_exhausted_account_moves_on() {
    let tmp = tempfile::tempdir().unwrap();
    let accounts = accounts_at(tmp.path());
    let env = AccountsRunEnvironment(accounts.clone());
    let ids = claude_accounts(&accounts, 3);
    accounts
        .set_rotation(HarnessId::ClaudeCode, true)
        .await
        .unwrap();
    assert_eq!(
        claude_token(&env, "chat-1").await.as_deref(),
        Some("token-0")
    );

    // Out of usage while others have room: moved at its next run start.
    let first = accounts.read_slot(HarnessId::ClaudeCode, &ids[0]).unwrap();
    exhaust(&accounts, &first);
    assert_eq!(
        claude_token(&env, "chat-1").await.as_deref(),
        Some("token-1")
    );
    let snapshot = accounts.list(false).await.unwrap();
    let chat = &snapshot.rotation_for(HarnessId::ClaudeCode).unwrap().chats[0];
    assert_eq!(chat.previous_account_id.as_deref(), Some(ids[0].as_str()));
    assert_eq!(
        chat.moved_reason.as_deref(),
        Some("Usage limit on user0@example.com, moved to user1@example.com")
    );

    // Its account removed: the chat is released and gets the next pick.
    accounts
        .forget(HarnessId::ClaudeCode, &ids[1])
        .await
        .unwrap();
    assert_eq!(assigned(&accounts, "chat-1", HarnessId::ClaudeCode), None);
    assert_eq!(
        claude_token(&env, "chat-1").await.as_deref(),
        Some("token-2")
    );
}

#[tokio::test]
async fn turning_rotation_off_returns_chats_to_the_live_login() {
    let tmp = tempfile::tempdir().unwrap();
    let accounts = accounts_at(tmp.path());
    let env = AccountsRunEnvironment(accounts.clone());
    claude_accounts(&accounts, 2);
    accounts
        .set_rotation(HarnessId::ClaudeCode, true)
        .await
        .unwrap();
    claude_token(&env, "chat-1").await;
    let rotated = env
        .credential(HarnessId::ClaudeCode, Some("chat-1"))
        .await
        .unwrap();
    assert!(rotated.starts_with("rotation:"), "{rotated}");

    let snapshot = accounts
        .set_rotation(HarnessId::ClaudeCode, false)
        .await
        .unwrap();
    let rotation = snapshot.rotation_for(HarnessId::ClaudeCode).unwrap();
    assert!(!rotation.enabled && !rotation.active && rotation.chats.is_empty());
    // A different credential: the parked session is replaced on the live login.
    assert_eq!(
        env.credential(HarnessId::ClaudeCode, Some("chat-1"))
            .await
            .as_deref(),
        Some(LIVE)
    );
    assert_eq!(claude_token(&env, "chat-1").await, None);
}

#[tokio::test]
async fn a_chat_on_the_live_account_runs_on_the_live_login() {
    let tmp = tempfile::tempdir().unwrap();
    let accounts = accounts_at(tmp.path());
    let env = AccountsRunEnvironment(accounts.clone());
    claude_accounts(&accounts, 2);
    make_claude_live(tmp.path(), "acct-0", "live-token");
    accounts
        .set_rotation(HarnessId::ClaudeCode, true)
        .await
        .unwrap();
    // chat-1 gets the live account: nothing injected, the live login stays.
    assert_eq!(claude_token(&env, "chat-1").await, None);
    let credential = env
        .credential(HarnessId::ClaudeCode, Some("chat-1"))
        .await
        .unwrap();
    assert!(credential.ends_with(":live"), "{credential}");
    assert_eq!(
        claude_token(&env, "chat-2").await.as_deref(),
        Some("token-1")
    );
    let live = read_json(&AgentAccountsConfig::isolated(tmp.path()).claude_creds_file()).unwrap();
    assert_eq!(live["claudeAiOauth"]["accessToken"], "live-token");
}

#[tokio::test]
async fn api_key_logins_never_rotate() {
    let tmp = tempfile::tempdir().unwrap();
    let accounts = accounts_at(tmp.path());
    let ids = claude_accounts(&accounts, 2);
    let mut keyed = accounts.read_slot(HarnessId::ClaudeCode, &ids[1]).unwrap();
    keyed.profile.auth_kind = AgentAuthKind::ApiKey;
    accounts.write_slot(&keyed).unwrap();
    let snapshot = accounts
        .set_rotation(HarnessId::ClaudeCode, true)
        .await
        .unwrap();
    let rotation = snapshot.rotation_for(HarnessId::ClaudeCode).unwrap();
    assert_eq!(rotation.accounts, vec![ids[0].clone()]);
    assert!(!rotation.active);
}

#[tokio::test]
async fn unsupported_harnesses_say_so() {
    let tmp = tempfile::tempdir().unwrap();
    let accounts = accounts_at(tmp.path());
    for harness in [HarnessId::Hermes, HarnessId::Antigravity, HarnessId::Muse] {
        assert!(
            accounts.set_rotation(harness, true).await.is_err(),
            "{harness:?}"
        );
    }
    let snapshot = accounts.list(false).await.unwrap();
    // Per-process isolation (Claude Code, Cursor, Devin) and the account
    // proxy (the rest) serve these.
    for harness in [
        HarnessId::ClaudeCode,
        HarnessId::Cursor,
        HarnessId::Devin,
        HarnessId::Codex,
        HarnessId::Grok,
        HarnessId::Opencode,
        HarnessId::Pi,
    ] {
        assert!(
            snapshot.rotation_for(harness).unwrap().supported,
            "{harness:?}"
        );
    }
    assert!(snapshot.rotation_for(HarnessId::Hermes).is_none());
    // The ones that can't say why.
    let view = accounts.rotation_view(&[HarnessId::Hermes, HarnessId::Antigravity]);
    assert!(view.iter().all(|r| !r.supported && !r.active));
    assert_eq!(
        view[0].note.as_deref(),
        Some("Hermes rotates its saved accounts itself.")
    );
    assert!(
        view[1]
            .note
            .as_deref()
            .is_some_and(|note| note.starts_with("Antigravity keeps one Google login")),
        "{:?}",
        view[1].note
    );
    // A mechanism installed later makes one supported.
    accounts.set_rotation_mechanism(HarnessId::Hermes, RotationMechanism::Env);
    let view = accounts.rotation_view(&[HarnessId::Hermes]);
    assert!(view[0].supported);
}

// ── Cursor and Devin keys ───────────────────────────────────────────────────

fn cursor_auth(email: &str, key: &str, expires_at: i64) -> serde_json::Value {
    serde_json::json!({
        "version": 1,
        "backendUrl": "https://api2.cursor.sh",
        "apiKey": key,
        "createdAtMs": now_ms() - HOUR,
        "apiKeyExpiresAtMs": expires_at,
        "email": email,
    })
}

fn slot_from(harness: HarnessId, detected: super::super::Detected) -> Slot {
    Slot {
        id: slot_id_for(harness, &detected.account_key),
        harness,
        account_key: detected.account_key,
        profile: detected.profile,
        credentials: detected.credentials.unwrap(),
        claude_config: None,
        saved_at: now_ms(),
        created_at: None,
        store_key: None,
    }
}

fn cursor_slot(auth: serde_json::Value) -> Slot {
    slot_from(
        HarnessId::Cursor,
        super::super::parse_cursor_auth(auth).unwrap(),
    )
}

async fn run_env(
    env: &AccountsRunEnvironment,
    harness: HarnessId,
    chat: &str,
) -> BTreeMap<String, String> {
    let mut request = request();
    env.prepare(harness, Some(chat), &mut request)
        .await
        .unwrap();
    request.env
}

#[tokio::test]
async fn cursor_chats_run_on_their_accounts_keys_and_skip_expired_ones() {
    let tmp = tempfile::tempdir().unwrap();
    let config = AgentAccountsConfig::isolated(tmp.path());
    let accounts = accounts_at(tmp.path());
    let env = AccountsRunEnvironment(accounts.clone());
    // The live login, saved by the next list like any other.
    let live = cursor_auth("live@example.com", "key-live", now_ms() + 30 * 24 * HOUR);
    write(&config.cursor_sdk_auth_file, &live.to_string());
    let other = cursor_slot(cursor_auth(
        "other@example.com",
        "key-other",
        now_ms() + 30 * 24 * HOUR,
    ));
    accounts.write_slot(&other).unwrap();
    let expired = cursor_slot(cursor_auth(
        "expired@example.com",
        "key-expired",
        now_ms() - HOUR,
    ));
    accounts.write_slot(&expired).unwrap();
    let snapshot = accounts
        .set_rotation(HarnessId::Cursor, true)
        .await
        .unwrap();
    let rotation = snapshot.rotation_for(HarnessId::Cursor).unwrap();
    assert!(rotation.supported && rotation.active, "{rotation:?}");
    assert_eq!(rotation.accounts.len(), 3);
    // An expired key sits out as signed out: nothing refreshes it.
    assert_eq!(
        rotation
            .skipped
            .iter()
            .map(|s| (s.account_id.as_str(), s.reason.as_str()))
            .collect::<Vec<_>>(),
        vec![(expired.id.as_str(), "Signed out")]
    );

    let mut seen = Vec::new();
    for chat in ["chat-1", "chat-2", "chat-3", "chat-4"] {
        let vars = run_env(&env, HarnessId::Cursor, chat).await;
        assert!(!vars.contains_key("CURSOR_BACKEND_URL"), "{vars:?}");
        seen.push(vars.get("CURSOR_API_KEY").cloned());
    }
    // The live account's chats run on the live login untouched; the other
    // account's get its key. The expired one never comes up.
    assert!(seen.contains(&None) && seen.contains(&Some("key-other".into())));
    assert!(!seen.contains(&Some("key-expired".into())));
    let other_chat = ["chat-1", "chat-2"]
        .into_iter()
        .find(|chat| assigned(&accounts, chat, HarnessId::Cursor).as_deref() == Some(&other.id))
        .unwrap();
    // The credential is stable across turns...
    let credential = env
        .credential(HarnessId::Cursor, Some(other_chat))
        .await
        .unwrap();
    assert!(credential.starts_with(&format!("rotation:{}:", other.id)));
    assert_eq!(
        env.credential(HarnessId::Cursor, Some(other_chat))
            .await
            .unwrap(),
        credential
    );
    // ...and changes when its account drops out: its key expired meanwhile,
    // leaving one usable account, so the chat is back on the live login.
    let mut lapsed = other.clone();
    lapsed.credentials["apiKeyExpiresAtMs"] = (now_ms() - 1).into();
    accounts.write_slot(&lapsed).unwrap();
    assert_eq!(
        env.credential(HarnessId::Cursor, Some(other_chat))
            .await
            .as_deref(),
        Some(LIVE)
    );
    assert!(
        !run_env(&env, HarnessId::Cursor, other_chat)
            .await
            .contains_key("CURSOR_API_KEY")
    );
    assert_eq!(read_json(&config.cursor_sdk_auth_file).unwrap(), live);
}

#[test]
fn a_cursor_key_from_another_backend_takes_it_along_only_to_cursor() {
    let mut auth = cursor_auth("a@example.com", "key-a", now_ms() + HOUR);
    auth["backendUrl"] = "https://api3.cursor.sh/".into();
    let (key, env) = key_run_env(&cursor_slot(auth.clone())).unwrap();
    assert_eq!(key, "key-a");
    assert_eq!(
        env.get("CURSOR_BACKEND_URL").map(String::as_str),
        Some("https://api3.cursor.sh")
    );
    auth["backendUrl"] = "https://cursor.sh.evil.example".into();
    let (_, env) = key_run_env(&cursor_slot(auth)).unwrap();
    assert_eq!(env.keys().collect::<Vec<_>>(), ["CURSOR_API_KEY"]);
    // Expired: no key to run on.
    let expired = cursor_auth("a@example.com", "key-a", now_ms() - 1);
    assert!(key_run_env(&cursor_slot(expired)).is_none());
}

fn devin_slot(key: &str, server: Option<&str>) -> Slot {
    let mut credentials = serde_json::json!({ "windsurf_api_key": key });
    if let Some(server) = server {
        credentials["api_server_url"] = server.into();
    }
    slot_from(
        HarnessId::Devin,
        super::super::stores::parse_devin_credentials(credentials).unwrap(),
    )
}

#[tokio::test]
async fn devin_chats_run_on_their_accounts_keys() {
    let tmp = tempfile::tempdir().unwrap();
    let config = AgentAccountsConfig::isolated(tmp.path());
    let accounts = accounts_at(tmp.path());
    let env = AccountsRunEnvironment(accounts.clone());
    let live_toml =
        "windsurf_api_key = \"devin-live\"\napi_server_url = \"https://server.codeium.com\"\n";
    write(&config.devin_credentials_file, live_toml);
    let enterprise = devin_slot("devin-enterprise", Some("https://acme.windsurf.com"));
    accounts.write_slot(&enterprise).unwrap();
    let rogue = devin_slot("devin-rogue", Some("https://collector.example"));
    accounts.write_slot(&rogue).unwrap();
    let snapshot = accounts.set_rotation(HarnessId::Devin, true).await.unwrap();
    let rotation = snapshot.rotation_for(HarnessId::Devin).unwrap();
    assert!(
        rotation.active && rotation.accounts.len() == 3,
        "{rotation:?}"
    );

    let mut by_key = BTreeMap::new();
    for chat in ["chat-1", "chat-2", "chat-3"] {
        let vars = run_env(&env, HarnessId::Devin, chat).await;
        by_key.insert(
            vars.get("WINDSURF_API_KEY").cloned().unwrap_or_default(),
            vars.get("WINDSURF_API_SERVER_URL").cloned(),
        );
    }
    // Live: nothing injected. A key bound to its own Devin server goes with
    // it; a server outside Devin's domains is never named.
    assert_eq!(
        by_key,
        BTreeMap::from([
            (String::new(), None),
            (
                "devin-enterprise".to_string(),
                Some("https://acme.windsurf.com".to_string())
            ),
            ("devin-rogue".to_string(), None),
        ])
    );
    assert_eq!(
        std::fs::read_to_string(&config.devin_credentials_file).unwrap(),
        live_toml
    );
}

// ── running chats ───────────────────────────────────────────────────────────

#[tokio::test]
async fn the_snapshot_says_which_rotated_chats_are_running() {
    let tmp = tempfile::tempdir().unwrap();
    let accounts = accounts_at(tmp.path());
    let env = AccountsRunEnvironment(accounts.clone());
    let ids = claude_accounts(&accounts, 2);
    accounts
        .set_rotation(HarnessId::ClaudeCode, true)
        .await
        .unwrap();
    for chat in ["chat-1", "chat-2", "chat-3"] {
        claude_token(&env, chat).await;
    }
    // Before the sessions engine is attached nothing reads as running.
    let snapshot = accounts.list(false).await.unwrap();
    let rotation = snapshot.rotation_for(HarnessId::ClaudeCode).unwrap();
    assert!(rotation.chats.iter().all(|c| !c.running));
    assert_eq!(rotation.chat_count(&ids[0]), 2);
    assert_eq!(rotation.running_count(&ids[0]), 0);

    accounts.set_running_chats(Arc::new(|| {
        std::collections::HashSet::from(["chat-3".to_string(), "unrotated".to_string()])
    }));
    let snapshot = accounts.list(false).await.unwrap();
    let rotation = snapshot.rotation_for(HarnessId::ClaudeCode).unwrap();
    let running: Vec<&str> = rotation
        .chats
        .iter()
        .filter(|c| c.running)
        .map(|c| c.chat_id.as_str())
        .collect();
    assert_eq!(running, ["chat-3"]);
    assert_eq!(rotation.running_count(&ids[0]), 1);
    assert_eq!(rotation.running_count(&ids[1]), 0);
    let json = serde_json::to_value(rotation).unwrap();
    let chats = json["chats"].as_array().unwrap();
    assert_eq!(
        chats.iter().filter(|c| c.get("running").is_some()).count(),
        1
    );
}

// ── refresh ─────────────────────────────────────────────────────────────────

async fn token_server(body: &'static str) -> (String, Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let mut request = vec![0u8; 16 * 1024];
            let _ = socket.read(&mut request).await;
            counter.fetch_add(1, Ordering::SeqCst);
            let _ = socket
                .write_all(
                    http_response("200 OK", &[("Content-Type", "application/json")], body)
                        .as_bytes(),
                )
                .await;
        }
    });
    (url, hits)
}

#[tokio::test]
async fn an_expiring_rotated_claude_login_is_refreshed_into_its_slot() {
    let tmp = tempfile::tempdir().unwrap();
    let (token_url, hits) = token_server(
        r#"{"access_token":"fresh-access","refresh_token":"fresh-refresh","expires_in":28800}"#,
    )
    .await;
    let accounts = AgentAccounts::with_endpoints(
        AgentAccountsConfig::isolated(tmp.path()),
        ProbeEndpoints {
            claude_token: token_url,
            ..Default::default()
        },
        Default::default(),
    );
    let env = AccountsRunEnvironment(accounts.clone());
    let stale = claude_slot("acct-a", "a@example.com", "old-access", now_ms() + 60_000);
    accounts.write_slot(&stale).unwrap();
    let other = claude_slot("acct-b", "b@example.com", "b-access", now_ms() + 8 * HOUR);
    accounts.write_slot(&other).unwrap();
    make_claude_live(tmp.path(), "acct-live", "live-token");
    accounts
        .set_rotation(HarnessId::ClaudeCode, true)
        .await
        .unwrap();

    let before = env
        .credential(HarnessId::ClaudeCode, Some("chat-1"))
        .await
        .unwrap();
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    assert_eq!(
        claude_token(&env, "chat-1").await.as_deref(),
        Some("fresh-access")
    );
    // Written back: the slot holds the rotated pair; no second refresh.
    let slot = accounts
        .read_slot(HarnessId::ClaudeCode, &stale.id)
        .unwrap();
    assert_eq!(
        slot.credentials["claudeAiOauth"]["refreshToken"],
        "fresh-refresh"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    assert_eq!(
        env.credential(HarnessId::ClaudeCode, Some("chat-1"))
            .await
            .unwrap(),
        before
    );
    // The live login is untouched.
    let live = read_json(&AgentAccountsConfig::isolated(tmp.path()).claude_creds_file()).unwrap();
    assert_eq!(live["claudeAiOauth"]["accessToken"], "live-token");
}

// ── retired Codex homes ─────────────────────────────────────────────────────

#[tokio::test]
async fn retired_codex_rotation_homes_save_their_refreshes_and_go() {
    let tmp = tempfile::tempdir().unwrap();
    let config = AgentAccountsConfig::isolated(tmp.path());
    let accounts = accounts_at(tmp.path());
    let live_auth = codex_auth(
        "live",
        "live-access",
        "live-refresh",
        "2026-10-01T00:00:00Z",
    );
    write(&config.codex_auth_file(), &live_auth.to_string());
    accounts.write_slot(&codex_slot(live_auth)).unwrap();
    let other = codex_slot(codex_auth(
        "other",
        "other-access",
        "other-refresh",
        "2026-10-01T00:00:00Z",
    ));
    accounts.write_slot(&other).unwrap();
    // An earlier build's home, where Codex refreshed the other login.
    let homes = config.root_dir().join(HOMES_DIR);
    let home = homes.join("codex").join(&other.id);
    let refreshed = codex_auth(
        "other",
        "other-access-2",
        "other-refresh-2",
        "2026-10-20T00:00:00Z",
    );
    write(&home.join("auth.json"), &refreshed.to_string());

    accounts.list(false).await.unwrap();
    let slot = accounts.read_slot(HarnessId::Codex, &other.id).unwrap();
    assert_eq!(
        slot.credentials["tokens"]["refresh_token"],
        "other-refresh-2"
    );
    assert!(!homes.exists());
    // Codex now rotates through the proxy: no per-process home.
    accounts.set_rotation(HarnessId::Codex, true).await.unwrap();
    let mut request = request();
    AccountsRunEnvironment(accounts.clone())
        .prepare(HarnessId::Codex, Some("chat-1"), &mut request)
        .await
        .unwrap();
    assert!(!request.env.contains_key("CODEX_HOME"));
}

#[tokio::test]
async fn a_retired_home_of_the_live_account_refreshes_the_live_store_only_when_newer() {
    let tmp = tempfile::tempdir().unwrap();
    let config = AgentAccountsConfig::isolated(tmp.path());
    let accounts = accounts_at(tmp.path());
    let live_auth = codex_auth("live", "a1", "r1", "2026-10-10T00:00:00Z");
    write(&config.codex_auth_file(), &live_auth.to_string());
    let live = codex_slot(live_auth);
    accounts.write_slot(&live).unwrap();
    let homes = config.root_dir().join(HOMES_DIR);
    let home = homes.join("codex").join(&live.id).join("auth.json");

    // Older than the live store: the live store stays.
    write(
        &home,
        &codex_auth("live", "a0", "r0", "2026-10-01T00:00:00Z").to_string(),
    );
    accounts.list(false).await.unwrap();
    assert_eq!(
        read_json(&config.codex_auth_file()).unwrap()["tokens"]["refresh_token"],
        "r1"
    );
    assert!(!homes.exists());

    // Newer (Codex rotated the refresh token there): it replaces the store.
    write(
        &home,
        &codex_auth("live", "a2", "r2", "2026-10-20T00:00:00Z").to_string(),
    );
    accounts.list(false).await.unwrap();
    assert_eq!(
        read_json(&config.codex_auth_file()).unwrap()["tokens"]["refresh_token"],
        "r2"
    );
    assert!(!homes.exists());
}

// ── spawn path ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn consecutive_chats_spawn_on_different_accounts_only_when_enabled() {
    let tmp = tempfile::tempdir().unwrap();
    let accounts = accounts_at(tmp.path());
    claude_accounts(&accounts, 2);
    let registry = HarnessRegistry::new();
    registry.set_account_environment(Arc::new(AccountsRunEnvironment(accounts.clone())));
    let spawn = async |chat: &str| {
        let mut request = request();
        let credential = registry
            .run_credential(HarnessId::ClaudeCode, Some(chat))
            .await;
        registry
            .prepare_run(HarnessId::ClaudeCode, Some(chat), &mut request)
            .await
            .unwrap();
        (
            credential,
            request.env.get("CLAUDE_CODE_OAUTH_TOKEN").cloned(),
        )
    };

    // Off: both chats on the live login, nothing injected.
    let (c1, t1) = spawn("chat-a").await;
    let (c2, t2) = spawn("chat-b").await;
    assert_eq!((t1, t2), (None, None));
    assert_eq!(c1, c2);

    accounts
        .set_rotation(HarnessId::ClaudeCode, true)
        .await
        .unwrap();
    let (c1, t1) = spawn("chat-c").await;
    let (c2, t2) = spawn("chat-d").await;
    assert_eq!(t1.as_deref(), Some("token-0"));
    assert_eq!(t2.as_deref(), Some("token-1"));
    assert_ne!(c1, c2);
    // Runs of no chat (title generation) never rotate.
    let mut request = request();
    registry
        .prepare_run(HarnessId::ClaudeCode, None, &mut request)
        .await
        .unwrap();
    assert!(!request.env.contains_key("CLAUDE_CODE_OAUTH_TOKEN"));
}

// ── proxy leases ────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_limit_fails_the_chat_over_and_it_stays_moved() {
    let tmp = tempfile::tempdir().unwrap();
    let accounts = accounts_at(tmp.path());
    let ids = claude_accounts(&accounts, 3);
    accounts
        .set_rotation(HarnessId::ClaudeCode, true)
        .await
        .unwrap();
    let lease = accounts
        .lease_for_chat("chat-1", HarnessId::ClaudeCode, &[])
        .await
        .unwrap();
    assert_eq!(lease.account_id, ids[0]);
    assert_eq!(lease.access_token, "token-0");
    assert_eq!(
        lease.headers.get("authorization").map(String::as_str),
        Some("Bearer token-0")
    );

    // The provider said account 0 is out: the request retries elsewhere.
    accounts.report_limit(HarnessId::ClaudeCode, &ids[0], None);
    let lease = accounts
        .lease_for_chat(
            "chat-1",
            HarnessId::ClaudeCode,
            std::slice::from_ref(&ids[0]),
        )
        .await
        .unwrap();
    assert_eq!(lease.account_id, ids[1]);
    // Sticky on the new account from now on.
    let lease = accounts
        .lease_for_chat("chat-1", HarnessId::ClaudeCode, &[])
        .await
        .unwrap();
    assert_eq!(lease.account_id, ids[1]);
    // A new chat skips the limited account too.
    let lease = accounts
        .lease_for_chat("chat-2", HarnessId::ClaudeCode, &[])
        .await
        .unwrap();
    assert_eq!(lease.account_id, ids[1]);

    let snapshot = accounts.list(false).await.unwrap();
    let rotation = snapshot.rotation_for(HarnessId::ClaudeCode).unwrap();
    assert_eq!(rotation.skipped.len(), 1);
    assert_eq!(rotation.skipped[0].account_id, ids[0]);
    assert_eq!(rotation.skipped[0].reason, "Usage limit");
    // No reset time reported: skipped for the cooldown.
    assert!(
        rotation.skipped[0]
            .until
            .is_some_and(|until| until > now_ms() + HOUR - 60_000)
    );
    let chat = rotation
        .chats
        .iter()
        .find(|c| c.chat_id == "chat-1")
        .unwrap();
    assert_eq!(chat.previous_account_id.as_deref(), Some(ids[0].as_str()));
    assert_eq!(
        chat.moved_reason.as_deref(),
        Some("Usage limit on user0@example.com, moved to user1@example.com")
    );
}

#[tokio::test]
async fn a_rejected_account_is_skipped_until_it_signs_in_again() {
    let tmp = tempfile::tempdir().unwrap();
    let accounts = accounts_at(tmp.path());
    let ids = claude_accounts(&accounts, 3);
    accounts
        .set_rotation(HarnessId::ClaudeCode, true)
        .await
        .unwrap();
    accounts.report_rejected(HarnessId::ClaudeCode, &ids[0]);
    let lease = accounts
        .lease_for_chat("chat-1", HarnessId::ClaudeCode, &[])
        .await
        .unwrap();
    assert_eq!(lease.account_id, ids[1]);
    let snapshot = accounts.list(false).await.unwrap();
    let skipped = &snapshot
        .rotation_for(HarnessId::ClaudeCode)
        .unwrap()
        .skipped;
    assert_eq!(
        skipped
            .iter()
            .map(|s| (s.account_id.as_str(), s.reason.as_str(), s.until))
            .collect::<Vec<_>>(),
        vec![(ids[0].as_str(), "Signed out", None)]
    );
    // A re-login (new credentials) makes it usable again.
    let mut slot = accounts.read_slot(HarnessId::ClaudeCode, &ids[0]).unwrap();
    slot.credentials["claudeAiOauth"]["accessToken"] = "relogged".into();
    accounts.write_slot(&slot).unwrap();
    let snapshot = accounts.list(false).await.unwrap();
    assert_eq!(
        snapshot
            .rotation_for(HarnessId::ClaudeCode)
            .unwrap()
            .next_account_id
            .as_deref(),
        Some(ids[2].as_str())
    );
    for (chat, id) in [("chat-2", &ids[2]), ("chat-3", &ids[0])] {
        let lease = accounts
            .lease_for_chat(chat, HarnessId::ClaudeCode, &[])
            .await
            .unwrap();
        assert_eq!(&lease.account_id, id, "{chat}");
    }
}
