//! M5c integration: agent-account slot mechanics (claude-swap), uploads
//! chunk→commit→readback + path jail, chat auto-titling with the mock harness,
//! and the RPC dispatch for each new method over the memory transport.
//!
//! Account tests use explicit `AgentAccountsConfig` paths under a tempdir (never
//! the real `~/.claude` / `~/.codex`), so they are hermetic and parallel-safe.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as BASE64_URL;
use sha2::{Digest as _, Sha256};

use zeron_engine::{
    AgentAccounts, AgentAccountsConfig, EngineCore, HarnessRegistry, Repos, Uploads,
    worktree_branch_from_title,
};
use zeron_harness::mock::MockHarness;
use zeron_proto::{
    AgentAccountsSnapshot, AgentEvent, AgentLoginMode, AgentLoginStatus, DoneStatus, HarnessId,
    SandboxLevel,
};
use zeron_rpc::methods;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// AgentAccounts wired to temp claude/codex homes.
fn test_accounts(root: &Path) -> (AgentAccounts, AgentAccountsConfig) {
    let config = AgentAccountsConfig {
        data_dir: root.join("data"),
        claude_config_dir: root.join("claude"),
        claude_config_file: root.join("claude.json"),
        codex_home: root.join("codex"),
        cursor_sdk_auth_file: root.join("cursor-sdk").join("auth.json"),
        // File-only: a temp config must never reach the real Keychain login.
        claude_keychain_service: None,
        antigravity_home: Some(root.join("gemini")),
        antigravity_keychain: false,
        // Grok / Devin / OpenCode / Pi / Hermes: temp homes too.
        ..AgentAccountsConfig::isolated(root)
    };
    (AgentAccounts::new(config.clone()), config)
}

fn write_claude_login(config: &AgentAccountsConfig, email: &str, uuid: &str, token: &str) {
    std::fs::create_dir_all(&config.claude_config_dir).expect("claude dir");
    std::fs::write(
        &config.claude_config_file,
        serde_json::json!({
            "oauthAccount": {
                "accountUuid": uuid,
                "emailAddress": email,
                "displayName": "Test User",
                "organizationName": "Test Org",
                "organizationType": "claude_max",
                "organizationRateLimitTier": "default_claude_max_20x",
            },
            "userID": format!("user-{uuid}"),
            "projects": { "/keep/me": { "history": [] } },
        })
        .to_string(),
    )
    .expect("claude config");
    std::fs::write(
        config.claude_config_dir.join(".credentials.json"),
        serde_json::json!({
            "claudeAiOauth": {
                "accessToken": token,
                "refreshToken": format!("refresh-{token}"),
                // Far-future expiry: usage probes must never try to rotate it.
                "expiresAt": 4_102_444_800_000i64,
            }
        })
        .to_string(),
    )
    .expect("claude creds");
}

/// An unsigned JWT with the claims codex mines from `id_token`.
fn fake_id_token(email: &str, account_id: &str, plan: &str) -> String {
    let header = BASE64_URL.encode(br#"{"alg":"none"}"#);
    let payload = BASE64_URL.encode(
        serde_json::json!({
            "email": email,
            "name": "Codex User",
            "https://api.openai.com/auth": {
                "chatgpt_account_id": account_id,
                "chatgpt_plan_type": plan,
            },
        })
        .to_string(),
    );
    format!("{header}.{payload}.x")
}

fn fake_team_id_token(email: &str, user_id: &str, workspace_id: &str) -> String {
    let header = BASE64_URL.encode(br#"{"alg":"none"}"#);
    let payload = BASE64_URL.encode(
        serde_json::json!({
            "email": email,
            "name": "Codex Team User",
            "https://api.openai.com/auth": {
                "chatgpt_account_id": workspace_id,
                "chatgpt_user_id": user_id,
                "chatgpt_plan_type": "team",
            },
        })
        .to_string(),
    );
    format!("{header}.{payload}.x")
}

fn write_codex_login(config: &AgentAccountsConfig, email: &str, account_id: &str) {
    std::fs::create_dir_all(&config.codex_home).expect("codex home");
    std::fs::write(
        config.codex_home.join("auth.json"),
        serde_json::json!({
            "tokens": {
                "id_token": fake_id_token(email, account_id, "plus"),
                "access_token": format!("at-{account_id}"),
                "account_id": account_id,
            }
        })
        .to_string(),
    )
    .expect("codex auth");
}

fn write_codex_team_login(
    config: &AgentAccountsConfig,
    email: &str,
    user_id: &str,
    workspace_id: &str,
) {
    std::fs::create_dir_all(&config.codex_home).expect("codex home");
    std::fs::write(
        config.codex_home.join("auth.json"),
        serde_json::json!({
            "tokens": {
                "id_token": fake_team_id_token(email, user_id, workspace_id),
                "access_token": format!("at-{user_id}"),
                "account_id": workspace_id,
            }
        })
        .to_string(),
    )
    .expect("codex team auth");
}

fn account_emails(snapshot: &AgentAccountsSnapshot, harness: HarnessId) -> Vec<(String, bool)> {
    snapshot
        .accounts
        .iter()
        .filter(|a| a.harness == harness)
        .map(|a| (a.email.clone().unwrap_or_default(), a.active))
        .collect()
}

fn assemble_with_mock(dir: &Path, script: Vec<AgentEvent>) -> EngineCore {
    std::fs::create_dir_all(dir).expect("data dir");
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(MockHarness { script }));
    EngineCore::assemble(dir, Arc::new(registry), HarnessId::Mock, None).expect("engine assembles")
}

async fn git(cwd: &Path, args: &[&str]) {
    let output = tokio::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@test")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@test")
        .output()
        .await
        .expect("git spawns");
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}

async fn init_repo(dir: &Path) {
    std::fs::create_dir_all(dir).expect("repo dir");
    git(dir, &["init", "-b", "main"]).await;
    std::fs::write(dir.join("a.txt"), "one\n").expect("write a.txt");
    git(dir, &["add", "."]).await;
    git(dir, &["commit", "-m", "initial"]).await;
}

/// Poll until `probe` yields Some, or panic at the deadline.
async fn wait_for<T>(what: &str, mut probe: impl FnMut() -> Option<T>) -> T {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(value) = probe() {
            return value;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

// ---------------------------------------------------------------------------
// Agent accounts — claude slot swap round trip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn claude_slot_swap_round_trip() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (accounts, config) = test_accounts(tmp.path());

    // Live login = Alice. Listing detects + auto-snapshots her into a slot.
    write_claude_login(&config, "alice@example.com", "uuid-alice", "token-alice");
    let snapshot = accounts.list(false).await.expect("list");
    assert_eq!(
        account_emails(&snapshot, HarnessId::ClaudeCode),
        vec![("alice@example.com".to_string(), true)]
    );
    let alice = &snapshot.accounts[0];
    assert_eq!(
        alice.plan_label.as_deref(),
        Some("Max 20×"),
        "plan label parse"
    );
    assert_eq!(alice.display_name.as_deref(), Some("Test User"));
    assert_eq!(alice.organization.as_deref(), Some("Test Org"));
    assert!(alice.switchable);
    assert!(snapshot.warnings.is_empty());
    let alice_id = alice.id.clone();
    assert_eq!(alice_id.len(), 16, "slot id is 16 hex chars");

    // Bob logs in via the CLI (live files replaced) — next list snapshots Bob
    // and shows Alice as a saved, inactive slot.
    write_claude_login(&config, "bob@example.com", "uuid-bob", "token-bob");
    let snapshot = accounts.list(false).await.expect("list bob");
    let mut emails = account_emails(&snapshot, HarnessId::ClaudeCode);
    emails.sort();
    assert_eq!(
        emails,
        vec![
            ("alice@example.com".to_string(), false),
            ("bob@example.com".to_string(), true)
        ]
    );

    // Activate Alice: her slot's tokens land in the live files, Bob's live
    // session is auto-snapshotted first, identity merged into claude.json.
    let snapshot = accounts
        .activate(HarnessId::ClaudeCode, &alice_id)
        .await
        .expect("activate");
    let mut emails = account_emails(&snapshot, HarnessId::ClaudeCode);
    emails.sort();
    assert_eq!(
        emails,
        vec![
            ("alice@example.com".to_string(), true),
            ("bob@example.com".to_string(), false)
        ]
    );
    let creds: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(config.claude_config_dir.join(".credentials.json"))
            .expect("creds readable"),
    )
    .expect("creds json");
    assert_eq!(creds["claudeAiOauth"]["accessToken"], "token-alice");
    let cfg: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&config.claude_config_file).expect("cfg"))
            .expect("cfg json");
    assert_eq!(cfg["oauthAccount"]["emailAddress"], "alice@example.com");
    assert_eq!(cfg["userID"], "user-uuid-alice");
    // The rest of the config survived the merge (only identity fields swapped).
    assert!(
        cfg["projects"]["/keep/me"].is_object(),
        "unrelated config keys preserved"
    );

    // Slot files: exactly two, under data/agent-accounts/claude-code.
    let slots_dir = config.data_dir.join("agent-accounts").join("claude-code");
    let slot_count = std::fs::read_dir(&slots_dir)
        .expect("slots dir")
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .count();
    assert_eq!(slot_count, 2);

    // Corrupt claude.json → activate must refuse rather than wipe it.
    std::fs::write(&config.claude_config_file, "{ definitely not json").expect("corrupt");
    let bob_id = snapshot
        .accounts
        .iter()
        .find(|a| a.email.as_deref() == Some("bob@example.com"))
        .expect("bob listed")
        .id
        .clone();
    let refused = accounts.activate(HarnessId::ClaudeCode, &bob_id).await;
    assert!(refused.is_err(), "parse-failed config must block the swap");
    assert_eq!(
        std::fs::read_to_string(&config.claude_config_file).expect("still there"),
        "{ definitely not json",
        "the unparsable config was left untouched"
    );
}

#[tokio::test]
async fn claude_account_switch_keeps_live_mcp_oauth() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (accounts, config) = test_accounts(tmp.path());
    let creds_file = config.claude_config_dir.join(".credentials.json");

    write_claude_login(&config, "alice@example.com", "uuid-alice", "token-alice");
    // Alice's first snapshot includes a MCP token that will go stale.
    std::fs::write(
        &creds_file,
        serde_json::json!({
            "claudeAiOauth": {
                "accessToken": "token-alice",
                "refreshToken": "refresh-token-alice",
                "expiresAt": 4_102_444_800_000i64,
            },
            "mcpOAuth": { "github": { "accessToken": "stale-github" } },
            "pluginSecrets": { "old": true },
            "trustedDeviceToken": "alice-device",
        })
        .to_string(),
    )
    .expect("alice mcp creds");
    let snapshot = accounts.list(false).await.expect("list alice");
    let alice_id = snapshot
        .accounts
        .iter()
        .find(|a| a.email.as_deref() == Some("alice@example.com"))
        .expect("alice listed")
        .id
        .clone();

    // Bob becomes live; MCP tokens rotate while he is the active login.
    write_claude_login(&config, "bob@example.com", "uuid-bob", "token-bob");
    std::fs::write(
        &creds_file,
        serde_json::json!({
            "claudeAiOauth": {
                "accessToken": "token-bob",
                "refreshToken": "refresh-token-bob",
                "expiresAt": 4_102_444_800_000i64,
            },
            "mcpOAuth": { "github": { "accessToken": "live-github" } },
            "pluginSecrets": { "live": true },
            "trustedDeviceToken": "bob-device",
        })
        .to_string(),
    )
    .expect("bob mcp creds");
    accounts.list(false).await.expect("list bob");

    accounts
        .activate(HarnessId::ClaudeCode, &alice_id)
        .await
        .expect("activate alice");

    let creds: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&creds_file).expect("creds readable"))
            .expect("creds json");
    assert_eq!(creds["claudeAiOauth"]["accessToken"], "token-alice");
    assert_eq!(
        creds["trustedDeviceToken"], "alice-device",
        "account-bound device token stays with the slot"
    );
    assert_eq!(
        creds["mcpOAuth"]["github"]["accessToken"], "live-github",
        "live MCP OAuth must survive the switch, not Alice's stale snapshot"
    );
    assert_eq!(creds["pluginSecrets"]["live"], true);
    assert!(
        creds["pluginSecrets"].get("old").is_none(),
        "slot plugin secrets must not clobber the live generation"
    );
}

#[tokio::test]
async fn claude_account_switch_keeps_mcp_when_target_slot_has_none() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (accounts, config) = test_accounts(tmp.path());
    let creds_file = config.claude_config_dir.join(".credentials.json");

    // Alice saved via the oauth-only shape (new login / usage refresh).
    write_claude_login(&config, "alice@example.com", "uuid-alice", "token-alice");
    let snapshot = accounts.list(false).await.expect("list alice");
    let alice_id = snapshot.accounts[0].id.clone();

    write_claude_login(&config, "bob@example.com", "uuid-bob", "token-bob");
    std::fs::write(
        &creds_file,
        serde_json::json!({
            "claudeAiOauth": {
                "accessToken": "token-bob",
                "refreshToken": "refresh-token-bob",
                "expiresAt": 4_102_444_800_000i64,
            },
            "mcpOAuth": { "linear": { "accessToken": "live-linear" } },
        })
        .to_string(),
    )
    .expect("bob mcp creds");
    accounts.list(false).await.expect("list bob");

    accounts
        .activate(HarnessId::ClaudeCode, &alice_id)
        .await
        .expect("activate alice");

    let creds: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&creds_file).expect("creds readable"))
            .expect("creds json");
    assert_eq!(creds["claudeAiOauth"]["accessToken"], "token-alice");
    assert_eq!(creds["mcpOAuth"]["linear"]["accessToken"], "live-linear");
}

#[tokio::test]
async fn codex_slot_swap_and_api_key_detection() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (accounts, config) = test_accounts(tmp.path());

    write_codex_login(&config, "carol@example.com", "acct-carol");
    let snapshot = accounts.list(false).await.expect("list");
    let carol = snapshot
        .accounts
        .iter()
        .find(|a| a.harness == HarnessId::Codex)
        .expect("codex account");
    assert_eq!(carol.email.as_deref(), Some("carol@example.com"));
    assert_eq!(carol.plan_label.as_deref(), Some("ChatGPT Plus"));
    assert!(carol.active);
    let carol_id = carol.id.clone();

    // Second login (Dave) becomes live; swap back to Carol.
    write_codex_login(&config, "dave@example.com", "acct-dave");
    accounts.list(false).await.expect("list dave");
    let snapshot = accounts
        .activate(HarnessId::Codex, &carol_id)
        .await
        .expect("activate carol");
    let mut emails = account_emails(&snapshot, HarnessId::Codex);
    emails.sort();
    assert_eq!(
        emails,
        vec![
            ("carol@example.com".to_string(), true),
            ("dave@example.com".to_string(), false)
        ]
    );
    let auth: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(config.codex_home.join("auth.json")).expect("auth"),
    )
    .expect("auth json");
    assert_eq!(auth["tokens"]["account_id"], "acct-carol");

    // API-key mode: no tokens, just the key.
    std::fs::write(
        config.codex_home.join("auth.json"),
        serde_json::json!({ "OPENAI_API_KEY": "sk-test-12345678abcd" }).to_string(),
    )
    .expect("api key auth");
    let snapshot = accounts.list(false).await.expect("list api key");
    let key_account = snapshot
        .accounts
        .iter()
        .find(|a| a.harness == HarnessId::Codex && a.active)
        .expect("api key account");
    assert_eq!(key_account.plan_label.as_deref(), Some("API key"));
    assert_eq!(key_account.email.as_deref(), Some("API key ·…abcd"));
}

#[tokio::test]
async fn codex_team_seats_are_distinct_and_legacy_slots_are_migrated() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (accounts, config) = test_accounts(tmp.path());
    write_codex_team_login(&config, "erin@team.com", "user-erin", "ws-team");
    let snapshot = accounts.list(false).await.expect("list erin");
    let erin_id = snapshot
        .accounts
        .iter()
        .find(|a| a.email.as_deref() == Some("erin@team.com"))
        .expect("erin")
        .id
        .clone();

    // Simulate the workspace-only slot format written by older versions.
    let slots = config.data_dir.join("agent-accounts").join("codex");
    let mut legacy: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(slots.join(format!("{erin_id}.json"))).expect("erin slot"),
    )
    .expect("slot json");
    let digest = Sha256::digest(b"codex:ws-team");
    let legacy_id = digest[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    legacy["id"] = serde_json::json!(legacy_id);
    legacy["accountKey"] = serde_json::json!("ws-team");
    std::fs::write(slots.join(format!("{legacy_id}.json")), legacy.to_string())
        .expect("legacy slot");
    std::fs::remove_file(slots.join(format!("{erin_id}.json"))).expect("remove new slot");

    // Another teammate in the same workspace must get a separate slot, and
    // migration must preserve Erin's credentials under her stable new id.
    write_codex_team_login(&config, "finn@team.com", "user-finn", "ws-team");
    let snapshot = accounts.list(false).await.expect("list finn");
    let mut emails = account_emails(&snapshot, HarnessId::Codex);
    emails.sort();
    assert_eq!(
        emails,
        vec![
            ("erin@team.com".to_string(), false),
            ("finn@team.com".to_string(), true),
        ]
    );
    assert!(!slots.join(format!("{legacy_id}.json")).exists());
    assert!(slots.join(format!("{erin_id}.json")).exists());
}

#[tokio::test]
async fn forget_guards_and_removes_slots() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (accounts, config) = test_accounts(tmp.path());
    write_claude_login(&config, "alice@example.com", "uuid-alice", "token-alice");
    let snapshot = accounts.list(false).await.expect("list");
    let alice_id = snapshot.accounts[0].id.clone();

    // Path-shaped ids never reach the filesystem.
    assert!(
        accounts
            .forget(HarnessId::ClaudeCode, "../../evil")
            .await
            .is_err()
    );
    assert!(
        accounts
            .forget(HarnessId::ClaudeCode, "ABCDEF0123456789")
            .await
            .is_err()
    );
    // A non-active slot forgets cleanly.
    write_claude_login(&config, "bob@example.com", "uuid-bob", "token-bob");
    accounts.list(false).await.expect("list bob");
    let snapshot = accounts
        .forget(HarnessId::ClaudeCode, &alice_id)
        .await
        .expect("forget alice");
    assert_eq!(
        account_emails(&snapshot, HarnessId::ClaudeCode),
        vec![("bob@example.com".to_string(), true)]
    );

    // The live (and only) login forgets too — by signing the CLI out, so it
    // isn't re-detected; the rest of ~/.claude.json survives.
    let bob_id = snapshot.accounts[0].id.clone();
    let snapshot = accounts
        .forget(HarnessId::ClaudeCode, &bob_id)
        .await
        .expect("forget live bob");
    assert!(account_emails(&snapshot, HarnessId::ClaudeCode).is_empty());
    let cfg: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&config.claude_config_file).unwrap())
            .unwrap();
    assert!(cfg.get("oauthAccount").is_none());
    assert!(cfg["projects"].get("/keep/me").is_some());
    let creds: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(config.claude_config_dir.join(".credentials.json")).unwrap(),
    )
    .unwrap();
    assert!(creds.get("claudeAiOauth").is_none());
}

/// Grok, Devin, OpenCode and Pi through the public API: each live login is
/// snapshotted, a second login is kept beside it, and switching rewrites
/// exactly that agent's store (for OpenCode/Pi: exactly that provider's
/// entry) at 0600.
#[tokio::test]
async fn grok_devin_opencode_and_pi_logins_swap_round_trip() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (accounts, config) = test_accounts(tmp.path());
    let write = |path: &Path, contents: String| {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    };
    let grok = |user: &str| {
        serde_json::json!({ "https://auth.x.ai::c": {
            "key": format!("key-{user}"), "refresh_token": "r", "oidc_issuer": "https://auth.x.ai",
            "oidc_client_id": "c", "user_id": user, "email": format!("{user}@x.ai"),
        }})
        .to_string()
    };
    let devin = |key: &str| format!("windsurf_api_key = \"{key}\"\n");
    let chatgpt = |account: &str| {
        let claims = serde_json::json!({
            "https://api.openai.com/auth": { "chatgpt_account_id": account, "chatgpt_plan_type": "plus" },
            "https://api.openai.com/profile": { "email": format!("{account}@example.com") },
        });
        serde_json::json!({
            "type": "oauth",
            "access": format!("e30.{}.sig", BASE64_URL.encode(claims.to_string())),
            "refresh": format!("refresh-{account}"),
            "expires": 1,
            "accountId": account,
        })
    };
    let grok_file = config.grok_home.join("auth.json");
    let opencode_file = config.opencode_auth_file.clone();
    let pi_file = config.pi_agent_dir.join("auth.json");
    let opencode = |account: &str| {
        serde_json::json!({ "openai": chatgpt(account), "openrouter": { "type": "api", "key": "keep" } })
            .to_string()
    };
    let pi = |account: &str| serde_json::json!({ "openai-codex": chatgpt(account) }).to_string();

    write(&grok_file, grok("ann"));
    write(&config.devin_credentials_file, devin("devin-ann"));
    write(&opencode_file, opencode("ann"));
    write(&pi_file, pi("ann"));
    let first = accounts.list(false).await.expect("list");
    let id_of = |snapshot: &AgentAccountsSnapshot, harness| {
        snapshot
            .accounts
            .iter()
            .find(|a| a.harness == harness && a.active)
            .map(|a| a.id.clone())
            .expect("live login listed")
    };
    let ann: Vec<(HarnessId, String)> = [
        HarnessId::Grok,
        HarnessId::Devin,
        HarnessId::Opencode,
        HarnessId::Pi,
    ]
    .into_iter()
    .map(|h| (h, id_of(&first, h)))
    .collect();

    write(&grok_file, grok("bob"));
    write(&config.devin_credentials_file, devin("devin-bob"));
    write(&opencode_file, opencode("bob"));
    write(&pi_file, pi("bob"));
    let second = accounts.list(false).await.expect("list");
    for (harness, _) in &ann {
        let rows: Vec<_> = second
            .accounts
            .iter()
            .filter(|a| a.harness == *harness)
            .collect();
        assert_eq!(rows.len(), 2, "{harness:?} keeps both logins");
        assert_eq!(rows.iter().filter(|a| a.active).count(), 1);
        assert!(rows.iter().all(|a| a.switchable));
    }
    for (harness, id) in &ann {
        let snapshot = accounts.activate(*harness, id).await.expect("switch");
        assert!(snapshot.accounts.iter().any(|a| a.id == *id && a.active));
    }
    assert!(
        std::fs::read_to_string(&grok_file)
            .unwrap()
            .contains("key-ann")
    );
    assert!(
        std::fs::read_to_string(&config.devin_credentials_file)
            .unwrap()
            .contains("devin-ann")
    );
    let opencode_live: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&opencode_file).unwrap()).unwrap();
    assert_eq!(opencode_live["openai"]["accountId"], "ann");
    assert_eq!(opencode_live["openrouter"]["key"], "keep");
    let pi_live: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&pi_file).unwrap()).unwrap();
    assert_eq!(pi_live["openai-codex"]["accountId"], "ann");
    #[cfg(unix)]
    for file in [
        &grok_file,
        &config.devin_credentials_file,
        &opencode_file,
        &pi_file,
    ] {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "{}", file.display());
    }
    // OpenCode / Pi rows name the provider they belong to.
    let snapshot = accounts.list(false).await.expect("list");
    assert!(
        snapshot
            .accounts
            .iter()
            .filter(|a| matches!(a.harness, HarnessId::Opencode | HarnessId::Pi))
            .all(|a| a.provider.is_some())
    );
}

/// Hermes keeps every account in its own pool: listed as-is, the active
/// provider's first entry in use, never switched or forgotten from zeron.
#[tokio::test]
async fn hermes_credential_pool_is_listed_read_only() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (accounts, config) = test_accounts(tmp.path());
    let file = config.hermes_home.join("auth.json");
    std::fs::create_dir_all(&config.hermes_home).unwrap();
    let pool = serde_json::json!({
        "active_provider": "nous",
        "credential_pool": { "nous": [
            { "id": "b", "label": "second@nous.ai", "auth_type": "oauth", "priority": 1, "access_token": "t2" },
            { "id": "a", "label": "first@nous.ai", "auth_type": "oauth", "priority": 0, "access_token": "t1" },
        ]}
    })
    .to_string();
    std::fs::write(&file, &pool).unwrap();
    let snapshot = accounts.list(false).await.expect("list");
    let hermes: Vec<_> = snapshot
        .accounts
        .iter()
        .filter(|a| a.harness == HarnessId::Hermes)
        .collect();
    assert_eq!(hermes.len(), 2);
    assert_eq!(hermes[0].email.as_deref(), Some("first@nous.ai"));
    assert!(hermes[0].active && !hermes[1].active);
    assert!(hermes.iter().all(|a| !a.switchable));
    assert_eq!(hermes[0].provider.as_deref(), Some("nous"));
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
    assert_eq!(std::fs::read_to_string(&file).unwrap(), pool);
}

#[test]
fn snapshot_wire_shape() {
    let snapshot = AgentAccountsSnapshot::default();
    let value = serde_json::to_value(&snapshot).expect("serializes");
    assert_eq!(value, serde_json::json!({ "accounts": [], "warnings": [] }));
}

#[tokio::test]
async fn claude_login_flow_is_the_clis_loopback_pkce() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (accounts, _) = test_accounts(tmp.path());
    let start = accounts
        .start_login(HarnessId::ClaudeCode)
        .await
        .expect("start");
    // Claude Code's own automatic login: claude.ai authorize, PKCE, and a
    // redirect to the loopback port the start reply names.
    let port = start.callback_port.expect("a loopback callback port");
    assert!(
        start
            .url
            .starts_with("https://claude.com/cai/oauth/authorize?code=true")
    );
    assert!(start.url.contains("code_challenge_method=S256"));
    assert!(start.url.contains(&format!(
        "redirect_uri=http%3A%2F%2Flocalhost%3A{port}%2Fcallback"
    )));
    let mode = serde_json::to_value(start.mode).expect("mode");
    assert_eq!(mode, serde_json::json!("browser"));

    // Pending until the browser lands; cancel drops the flow (and closes its
    // listener) so the next poll reports it expired.
    let poll = accounts.poll_login(&start.login_id).await.expect("poll");
    assert_eq!(
        serde_json::to_value(poll.status).expect("status"),
        serde_json::json!("pending")
    );
    accounts.cancel_login(&start.login_id);
    assert!(
        accounts.poll_login(&start.login_id).await.is_err(),
        "cancelled flow is gone"
    );
    assert!(
        accounts
            .complete_login(&start.login_id, "code#state")
            .await
            .is_err()
    );
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(
        tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_err(),
        "a cancelled login stops listening"
    );
}

// ---------------------------------------------------------------------------
// Uploads
// ---------------------------------------------------------------------------

#[tokio::test]
async fn uploads_chunk_commit_readback_and_jail() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let uploads = Uploads::new(tmp.path());

    // 100KB of pseudo-random bytes, staged as three positional base64 chunks
    // (out of order, with one retried) — chunk boundaries are multiples of 3
    // bytes so independent base64 strings concatenate losslessly.
    let payload: Vec<u8> = (0..100_002u32)
        .map(|i| (i.wrapping_mul(31) % 251) as u8)
        .collect();
    let chunks: Vec<String> = payload.chunks(45_000).map(|c| BASE64.encode(c)).collect();
    assert_eq!(chunks.len(), 3);
    uploads
        .append("up-1", &chunks[2], Some(2))
        .expect("chunk 2");
    uploads
        .append("up-1", &chunks[0], Some(0))
        .expect("chunk 0");
    uploads
        .append("up-1", &chunks[0], Some(0))
        .expect("chunk 0 retry is idempotent");
    uploads
        .append("up-1", &chunks[1], Some(1))
        .expect("chunk 1");
    let path = uploads.commit("up-1", "photo.png").expect("commit");
    assert!(path.ends_with("up-1-photo.png"), "path: {path}");
    assert_eq!(std::fs::read(&path).expect("committed file"), payload);

    // Readback: chunked reassembly round-trips.
    let mut assembled = Vec::new();
    let mut offset = 0u64;
    loop {
        let chunk = uploads.read_chunk(&path, offset, &[]).expect("read chunk");
        assert_eq!(chunk.mime_type, "image/png");
        assert_eq!(chunk.name, "up-1-photo.png");
        assembled.extend(BASE64.decode(&chunk.data).expect("chunk base64"));
        offset = chunk.next_offset;
        if chunk.done {
            break;
        }
    }
    assert_eq!(assembled, payload);

    // Missing chunk → commit fails.
    uploads
        .append("up-2", &chunks[0], Some(0))
        .expect("chunk 0");
    uploads
        .append("up-2", &chunks[2], Some(2))
        .expect("chunk 2 (hole at 1)");
    assert!(
        uploads.commit("up-2", "holey.png").is_err(),
        "hole detected"
    );

    // Path jail: files outside the uploads dir (and outside any allowed cwd
    // root) are rejected, including traversal attempts and the dir itself.
    let outside = tmp.path().join("outside.png");
    std::fs::write(&outside, b"nope").expect("outside file");
    assert!(
        uploads
            .read_chunk(&outside.to_string_lossy(), 0, &[])
            .is_err()
    );
    assert!(uploads.read_chunk("/etc/passwd", 0, &[]).is_err());
    let sneaky = format!("{}/../outside.png", uploads.dir().display());
    assert!(
        uploads.read_chunk(&sneaky, 0, &[]).is_err(),
        "traversal rejected"
    );
    // …but a workspace-known cwd root admits its files.
    let ok = uploads
        .read_chunk(&outside.to_string_lossy(), 0, &[tmp.path().to_path_buf()])
        .expect("cwd-rooted read");
    assert_eq!(BASE64.decode(&ok.data).expect("data"), b"nope");
    // Non-image extensions are refused even inside the jail (zeron parity).
    let text = PathBuf::from(uploads.dir()).join("notes.txt");
    std::fs::create_dir_all(uploads.dir()).expect("uploads dir");
    std::fs::write(&text, b"text").expect("txt");
    assert!(uploads.read_chunk(&text.to_string_lossy(), 0, &[]).is_err());

    // Bogus upload ids never become paths.
    assert!(uploads.append("../evil", "aGk=", None).is_err());
    assert!(uploads.commit("unknown-upload", "x.png").is_err());
}

// ---------------------------------------------------------------------------
// Titling
// ---------------------------------------------------------------------------

#[tokio::test]
async fn titling_e2e_names_chat_and_renames_worktree_branch() {
    let tmp = tempfile::tempdir().expect("tempdir");
    // Worktree root must be inside the tempdir (EngineCore reads the env-less
    // default otherwise) — create the worktree with a dedicated Repos handle.
    let repo_dir = tmp.path().join("repo");
    init_repo(&repo_dir).await;
    let repos = Repos::with_worktrees_root(
        &tmp.path().join("data"),
        "device-test",
        tmp.path().join("worktrees"),
    );
    let worktree = repos
        .create_worktree(&repo_dir, "main")
        .await
        .expect("worktree");

    let core = assemble_with_mock(
        &tmp.path().join("data"),
        vec![
            AgentEvent::TextDelta {
                text: "Fix Login Flow".into(),
            },
            AgentEvent::Done {
                status: DoneStatus::Completed,
                result: None,
                error: None,
                session_id: None,
            },
        ],
    );
    let chat_id = "chat-title-1";
    core.workspace
        .create_space(
            "space-title",
            &core.device_id,
            &repo_dir.to_string_lossy(),
            None,
            true,
        )
        .expect("create space");
    core.workspace
        .create_chat(
            chat_id,
            Some("space-title"),
            None,
            None,
            Some(worktree.path.clone()),
        )
        .expect("create chat");
    core.workspace
        .set_chat_branch(chat_id, &worktree.branch)
        .expect("set branch");

    let request = zeron_proto::RunRequest {
        prompt: "please fix the login flow".into(),
        harness: None,
        model: None,
        reasoning: None,
        model_options: serde_json::Map::new(),
        cwd: worktree.path.clone(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: true,
        attachments: Vec::new(),
        worktree: None,
        resume: None,
    };
    core.sessions
        .dispatch(chat_id, HarnessId::Mock, request, None)
        .await
        .expect("dispatch");

    // The mock's scripted reply doubles as the titling model's output.
    let chat = wait_for("chat title", || {
        core.workspace
            .chat(chat_id)
            .ok()
            .flatten()
            .filter(|c| c.title.as_deref().is_some_and(|t| !t.is_empty()))
    })
    .await;
    assert_eq!(chat.title.as_deref(), Some("Fix Login Flow"));
    // Branch renamed from the title, chat row updated to match.
    assert_eq!(chat.branch.as_deref(), Some("zeron/fix-login-flow"));
    let head = tokio::process::Command::new("git")
        .args(["branch", "--show-current"])
        .current_dir(&worktree.path)
        .output()
        .await
        .expect("git");
    assert_eq!(
        String::from_utf8_lossy(&head.stdout).trim(),
        "zeron/fix-login-flow"
    );

    // A titled chat is never re-titled: rename, run again, title sticks.
    core.workspace
        .rename_chat(chat_id, "My Custom Name")
        .expect("rename");
    let request = zeron_proto::RunRequest {
        prompt: "another request".into(),
        harness: None,
        model: None,
        reasoning: None,
        model_options: serde_json::Map::new(),
        cwd: worktree.path.clone(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: true,
        attachments: Vec::new(),
        worktree: None,
        resume: None,
    };
    core.sessions
        .dispatch(chat_id, HarnessId::Mock, request, None)
        .await
        .expect("second dispatch");
    tokio::time::sleep(Duration::from_millis(400)).await;
    let chat = core.workspace.chat(chat_id).expect("chat").expect("row");
    assert_eq!(chat.title.as_deref(), Some("My Custom Name"));
    core.shutdown().await;
}

#[tokio::test]
async fn rename_worktree_branch_guards_and_collisions() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo_dir = tmp.path().join("repo");
    init_repo(&repo_dir).await;
    let repos = Repos::with_worktrees_root(
        &tmp.path().join("data"),
        "device-test",
        tmp.path().join("worktrees"),
    );
    let wt = repos
        .create_worktree(&repo_dir, "main")
        .await
        .expect("worktree");
    let wt_path = Path::new(&wt.path);

    // Guard: expected branch mismatch → no-op, returns the actual branch.
    let unchanged = repos
        .rename_worktree_branch(wt_path, "zeron/not-this-one", "Some Title")
        .await
        .expect("guarded");
    assert_eq!(unchanged, wt.branch);

    // Happy path: renamed to the title slug.
    let renamed = repos
        .rename_worktree_branch(wt_path, &wt.branch, "Add Dark Mode!")
        .await
        .expect("renamed");
    assert_eq!(renamed, "zeron/add-dark-mode");

    // Already renamed → the guard (branch no longer zeron/<folder>) makes any
    // further title rename a no-op.
    let again = repos
        .rename_worktree_branch(wt_path, "zeron/add-dark-mode", "Different Title")
        .await
        .expect("second rename");
    assert_eq!(again, "zeron/add-dark-mode");

    // Collision: a second worktree whose title slug already exists gets the
    // stable hash suffix.
    let wt2 = repos
        .create_worktree(&repo_dir, "main")
        .await
        .expect("worktree 2");
    let renamed2 = repos
        .rename_worktree_branch(Path::new(&wt2.path), &wt2.branch, "Add Dark Mode!")
        .await
        .expect("suffixed rename");
    assert!(
        renamed2.starts_with("zeron/add-dark-mode-")
            && renamed2.len() == "zeron/add-dark-mode-".len() + 6,
        "suffixed: {renamed2}"
    );

    // Slug edge cases.
    assert_eq!(
        worktree_branch_from_title("  Fix `Login` Flow!  "),
        "zeron/fix-login-flow"
    );
    assert_eq!(worktree_branch_from_title("***"), "zeron/update");
    assert_eq!(
        worktree_branch_from_title("Cafe's Dark Mode"),
        "zeron/cafes-dark-mode"
    );
}

// ---------------------------------------------------------------------------
// RPC dispatch
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rpc_dispatch_for_m5c_methods() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let core = assemble_with_mock(&tmp.path().join("data"), Vec::new());
    let client = zeron_rpc::memory_client(core.rpc_service());

    // Uploads: chunk → commit → readback over the wire.
    let payload = b"fake png bytes".to_vec();
    let ok = client
        .call(
            methods::UPLOAD_CHUNK,
            serde_json::json!({ "uploadId": "rpc-up", "data": BASE64.encode(&payload), "seq": 0 }),
        )
        .await
        .expect("UploadChunk");
    assert_eq!(ok["ok"], true);
    let committed = client
        .call(
            methods::UPLOAD_COMMIT,
            serde_json::json!({ "uploadId": "rpc-up", "fileName": "shot.png" }),
        )
        .await
        .expect("UploadCommit");
    let path = committed["path"].as_str().expect("path").to_string();
    assert!(path.ends_with("rpc-up-shot.png"));
    let chunk = client
        .call(
            methods::READ_ATTACHMENT_CHUNK,
            serde_json::json!({ "path": path, "offset": 0 }),
        )
        .await
        .expect("ReadAttachmentChunk");
    assert_eq!(chunk["mimeType"], "image/png");
    assert_eq!(chunk["done"], true);
    assert_eq!(
        BASE64
            .decode(chunk["data"].as_str().expect("data"))
            .expect("base64"),
        payload
    );
    // Jail holds over RPC too.
    assert!(
        client
            .call(
                methods::READ_ATTACHMENT_CHUNK,
                serde_json::json!({ "path": "/etc/passwd", "offset": 0 })
            )
            .await
            .is_err()
    );

    // Agent accounts: snapshot shape (this machine's real CLI state may or may
    // not include logins — assert the envelope, not the contents).
    let snapshot = client
        .call(methods::LIST_AGENT_ACCOUNTS, serde_json::json!({}))
        .await
        .expect("ListAgentAccounts");
    assert!(snapshot["accounts"].is_array());
    assert!(snapshot["warnings"].is_array());

    // The provider param reaches the engine: Pi's Claude login is refused
    // with its reason (no port bound, no network).
    let refused = client
        .call(
            methods::START_AGENT_LOGIN,
            serde_json::json!({ "harness": "pi", "provider": "anthropic" }),
        )
        .await
        .expect_err("pi claude login is not offered");
    assert!(refused.to_string().contains("/login"), "{refused}");

    // Login lifecycle: start (loopback browser flow, like the CLI's own
    // automatic login) → poll pending → cancel → gone.
    let start = client
        .call(
            methods::START_AGENT_LOGIN,
            serde_json::json!({ "harness": "claude-code" }),
        )
        .await
        .expect("StartAgentLogin");
    assert_eq!(start["mode"], "browser");
    assert!(start["callbackPort"].as_u64().is_some());
    assert!(
        start["url"]
            .as_str()
            .expect("url")
            .contains("claude.com/cai/oauth/authorize")
    );
    let login_id = start["loginId"].as_str().expect("loginId").to_string();
    let poll = client
        .call(
            methods::POLL_AGENT_LOGIN,
            serde_json::json!({ "loginId": login_id }),
        )
        .await
        .expect("PollAgentLogin");
    assert_eq!(poll["status"], "pending");
    let cancelled = client
        .call(
            methods::CANCEL_AGENT_LOGIN,
            serde_json::json!({ "loginId": login_id }),
        )
        .await
        .expect("CancelAgentLogin");
    assert_eq!(cancelled["ok"], true);
    assert!(
        client
            .call(
                methods::POLL_AGENT_LOGIN,
                serde_json::json!({ "loginId": login_id })
            )
            .await
            .is_err(),
        "cancelled login is expired"
    );

    // Error paths: junk account ids and dead logins fail cleanly.
    assert!(
        client
            .call(
                methods::FORGET_AGENT_ACCOUNT,
                serde_json::json!({ "harness": "claude-code", "accountId": "../nope" })
            )
            .await
            .is_err()
    );
    assert!(
        client
            .call(
                methods::ACTIVATE_AGENT_ACCOUNT,
                serde_json::json!({ "harness": "claude-code", "accountId": "0123456789abcdef" })
            )
            .await
            .is_err(),
        "unknown slot cannot be activated"
    );
    assert!(
        client
            .call(
                methods::COMPLETE_AGENT_LOGIN,
                serde_json::json!({ "loginId": "no-such-login", "code": "x#y" })
            )
            .await
            .is_err()
    );
    core.shutdown().await;
}

fn write_cursor_login(config: &AgentAccountsConfig, email: &str, expires_in_ms: i64) {
    let file = &config.cursor_sdk_auth_file;
    std::fs::create_dir_all(file.parent().unwrap()).expect("cursor sdk dir");
    std::fs::write(
        file,
        serde_json::json!({
            "version": 1,
            "backendUrl": "https://api2.cursor.sh",
            "apiKey": format!("key-{email}"),
            "apiKeyExpiresAtMs": now_ms_test() + expires_in_ms,
            "email": email,
            "createdAtMs": now_ms_test(),
        })
        .to_string(),
    )
    .expect("cursor auth");
}

fn now_ms_test() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

#[tokio::test]
async fn cursor_slot_swap_round_trip() {
    let dir = tempfile::tempdir().expect("tmp");
    let (accounts, config) = test_accounts(dir.path());

    // Live SDK login = Erin; listing detects + auto-snapshots her slot.
    write_cursor_login(&config, "erin@example.com", 86_400_000);
    let snapshot = accounts.list(false).await.expect("list");
    assert_eq!(
        account_emails(&snapshot, HarnessId::Cursor),
        vec![("erin@example.com".to_string(), true)]
    );
    assert!(snapshot.warnings.is_empty(), "{:?}", snapshot.warnings);
    let erin_id = snapshot.accounts[snapshot
        .accounts
        .iter()
        .position(|a| a.harness == HarnessId::Cursor)
        .unwrap()]
    .id
    .clone();

    // A second login (Frank) becomes live; both slots exist, Frank active.
    write_cursor_login(&config, "frank@example.com", 86_400_000);
    let snapshot = accounts.list(false).await.expect("list");
    assert_eq!(
        account_emails(&snapshot, HarnessId::Cursor),
        vec![
            ("erin@example.com".to_string(), false),
            ("frank@example.com".to_string(), true),
        ]
    );

    // Swap back to Erin: the SDK store file is rewritten from her slot.
    let snapshot = accounts
        .activate(HarnessId::Cursor, &erin_id)
        .await
        .expect("activate");
    assert_eq!(
        account_emails(&snapshot, HarnessId::Cursor),
        vec![
            ("erin@example.com".to_string(), true),
            ("frank@example.com".to_string(), false),
        ]
    );
    let live: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&config.cursor_sdk_auth_file).unwrap())
            .unwrap();
    assert_eq!(live["email"], "erin@example.com");

    // An expired live key detects (card + slot survive) but warns.
    write_cursor_login(&config, "erin@example.com", -1000);
    let snapshot = accounts.list(false).await.expect("list");
    assert!(
        snapshot
            .warnings
            .iter()
            .any(|w| w.harness == HarnessId::Cursor && w.message.contains("expired")),
        "{:?}",
        snapshot.warnings
    );
}

#[tokio::test]
async fn cursor_login_flow_spawns_shim_and_auto_activates() {
    let dir = tempfile::tempdir().expect("tmp");
    let (accounts, config) = test_accounts(dir.path());

    // Fake shim: in login mode, emit the auth-url frame, write the minted
    // store file where the engine pointed us, exit 0. Mirrors the real shim's
    // `node <shim> login <store-path>` argv contract.
    let shim = dir.path().join("fake-cursor-shim.sh");
    std::fs::write(
        &shim,
        r#"#!/bin/sh
[ "$1" = "login" ] || exit 1
printf '%s\n' '{"ev":"auth-url","url":"https://cursor.com/loginDeepControl?challenge=fake"}'
cat > "$2" <<JSON
{"version":1,"backendUrl":"https://api2.cursor.sh","apiKey":"key-minted","apiKeyExpiresAtMs":99999999999999,"email":"grace@example.com","createdAtMs":1}
JSON
printf '%s\n' '{"ev":"logged-in","email":"grace@example.com"}'
exit 0
"#,
    )
    .expect("fake shim");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    unsafe { std::env::set_var("CURSOR_SDK_SHIM_EXECUTABLE", &shim) };

    let start = accounts
        .start_login(HarnessId::Cursor)
        .await
        .expect("start");
    assert_eq!(start.mode, AgentLoginMode::Browser);
    assert_eq!(
        start.url,
        "https://cursor.com/loginDeepControl?challenge=fake"
    );

    poll_until_done(&accounts, &start.login_id).await;

    // First connect on a device with no live login: the minted key was
    // auto-activated, so runs work immediately.
    let live: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&config.cursor_sdk_auth_file).unwrap())
            .unwrap();
    assert_eq!(live["email"], "grace@example.com");
    let snapshot = accounts.list(false).await.expect("list");
    assert_eq!(
        account_emails(&snapshot, HarnessId::Cursor),
        vec![("grace@example.com".to_string(), true)]
    );

    // Re-connecting the LIVE account (still usable, older key) replaces the
    // live key — it is not re-snapshotted back over the fresh one.
    write_cursor_login(&config, "grace@example.com", 60_000);
    accounts.list(false).await.expect("list old key");
    login_until_done(&accounts, HarnessId::Cursor).await;
    assert_eq!(
        read_json_file(&config.cursor_sdk_auth_file)["apiKey"],
        "key-minted"
    );
    let snapshot = accounts.list(false).await.expect("list");
    let slot = snapshot
        .accounts
        .iter()
        .find(|a| a.harness == HarnessId::Cursor)
        .unwrap();
    assert!(slot.active);

    // Connecting while ANOTHER usable account is live leaves it live.
    write_cursor_login(&config, "hopper@example.com", 60_000);
    accounts.list(false).await.expect("list hopper");
    login_until_done(&accounts, HarnessId::Cursor).await;
    assert_eq!(
        read_json_file(&config.cursor_sdk_auth_file)["email"],
        "hopper@example.com"
    );

    // The live login is removable: the SDK store goes, so it isn't re-detected.
    let snapshot = accounts.list(false).await.expect("list");
    let hopper = snapshot
        .accounts
        .iter()
        .find(|a| a.email.as_deref() == Some("hopper@example.com"))
        .unwrap();
    let snapshot = accounts
        .forget(HarnessId::Cursor, &hopper.id)
        .await
        .expect("forget live hopper");
    assert!(!config.cursor_sdk_auth_file.exists());
    assert_eq!(
        account_emails(&snapshot, HarnessId::Cursor),
        vec![("grace@example.com".to_string(), false)]
    );
}

fn read_json_file(path: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

async fn poll_until_done(accounts: &AgentAccounts, login_id: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let poll = accounts.poll_login(login_id).await.expect("poll");
        match poll.status {
            AgentLoginStatus::Done => break,
            AgentLoginStatus::Pending => {}
            AgentLoginStatus::Error => panic!("login errored: {:?}", poll.message),
        }
        assert!(tokio::time::Instant::now() < deadline, "login never landed");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn login_until_done(accounts: &AgentAccounts, harness: HarnessId) {
    let start = accounts.start_login(harness).await.expect("start");
    poll_until_done(accounts, &start.login_id).await;
}

/// Codex's `codex login` against a fake CLI that writes whatever
/// `next-auth.json` holds into the throwaway `CODEX_HOME`.
#[tokio::test]
async fn codex_relogin_revives_the_live_account_and_live_is_removable() {
    let dir = tempfile::tempdir().expect("tmp");
    let (accounts, config) = test_accounts(dir.path());
    let codex = dir.path().join("fake-codex.sh");
    let next_auth = dir.path().join("next-auth.json");
    std::fs::write(
        &codex,
        format!(
            "#!/bin/sh\n[ \"$1\" = \"login\" ] || exit 1\ncp '{}' \"$CODEX_HOME/auth.json\"\nexit 0\n",
            next_auth.display()
        ),
    )
    .expect("fake codex");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&codex, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    unsafe { std::env::set_var("CODEX_EXECUTABLE", &codex) };
    let fresh_auth = |email: &str, account_id: &str| {
        serde_json::json!({
            "tokens": {
                "id_token": fake_id_token(email, account_id, "plus"),
                "access_token": format!("fresh-{account_id}"),
                "account_id": account_id,
            }
        })
        .to_string()
    };

    // Live ada with dead tokens; signing ada in again makes the fresh ones live.
    write_codex_login(&config, "ada@example.com", "acct-ada");
    accounts.list(false).await.expect("list ada");
    std::fs::write(&next_auth, fresh_auth("ada@example.com", "acct-ada")).unwrap();
    login_until_done(&accounts, HarnessId::Codex).await;
    let live_file = config.codex_home.join("auth.json");
    assert_eq!(
        read_json_file(&live_file)["tokens"]["access_token"],
        "fresh-acct-ada"
    );
    let snapshot = accounts.list(false).await.expect("list");
    assert_eq!(
        account_emails(&snapshot, HarnessId::Codex),
        vec![("ada@example.com".to_string(), true)]
    );

    // Another account signs in as a spare — ada stays live.
    std::fs::write(&next_auth, fresh_auth("bea@example.com", "acct-bea")).unwrap();
    login_until_done(&accounts, HarnessId::Codex).await;
    assert_eq!(
        read_json_file(&live_file)["tokens"]["access_token"],
        "fresh-acct-ada"
    );

    // Removing live ada signs codex out; bea remains as a saved login.
    let snapshot = accounts.list(false).await.expect("list");
    let ada = snapshot
        .accounts
        .iter()
        .find(|a| a.email.as_deref() == Some("ada@example.com"))
        .unwrap();
    let snapshot = accounts
        .forget(HarnessId::Codex, &ada.id)
        .await
        .expect("forget live ada");
    assert!(!live_file.exists());
    assert_eq!(
        account_emails(&snapshot, HarnessId::Codex),
        vec![("bea@example.com".to_string(), false)]
    );

    // With nothing live, a sign-in goes live at once.
    std::fs::write(&next_auth, fresh_auth("bea@example.com", "acct-bea")).unwrap();
    login_until_done(&accounts, HarnessId::Codex).await;
    let snapshot = accounts.list(false).await.expect("list");
    assert_eq!(
        account_emails(&snapshot, HarnessId::Codex),
        vec![("bea@example.com".to_string(), true)]
    );
}
