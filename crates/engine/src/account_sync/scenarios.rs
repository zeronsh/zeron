//! Scenario tests for synced subscriptions (see the parent module): each one
//! drives [`AccountSync`] end to end against a mock vault, with every agent
//! login kept in a temp dir ([`AgentAccountsConfig::isolated`]).
//!
//! Claude's profile endpoint is the real one (no seam reaches it from here),
//! so every Claude grant installed below has its identity known up front —
//! from the uploaded login's `claude_config`, or seeded into
//! `state.identities` — and never calls out.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::*;
use crate::agent_accounts::AgentAccountsConfig;
use crate::registry::RunEnvironment as _;

// ── fixture ─────────────────────────────────────────────────────────────────

struct Device {
    _root: tempfile::TempDir,
    config: AgentAccountsConfig,
    accounts: AgentAccounts,
    sync: AccountSync,
}

fn signed_in(server: &mock::Server) -> ContextFn {
    let url = server.url.clone();
    Arc::new(move || {
        Some(SyncContext {
            cloud: CloudClient::new(
                url.clone(),
                "org1",
                Arc::new(mock::Token) as Arc<dyn zeron_rpc::TokenSource>,
            ),
            user_id: "user1".into(),
        })
    })
}

fn device(server: &mock::Server) -> Device {
    device_with(signed_in(server))
}

fn device_with(context: ContextFn) -> Device {
    let root = tempfile::tempdir().unwrap();
    let config = AgentAccountsConfig::isolated(root.path());
    let accounts = AgentAccounts::new(config.clone());
    let sync = AccountSync::new(
        &root.path().join("data"),
        "laptop-1",
        accounts.clone(),
        context,
    );
    Device {
        _root: root,
        config,
        accounts,
        sync,
    }
}

impl Device {
    fn codex_file(&self) -> PathBuf {
        self.config.codex_home.join("auth.json")
    }

    fn claude_creds_file(&self) -> PathBuf {
        self.config.claude_config_dir.join(".credentials.json")
    }

    /// A Claude Code login the user signed in to with the CLI.
    fn claude_cli_login(&self, email: &str, refresh: &str) {
        write(
            &self.claude_creds_file(),
            &json!({
                "claudeAiOauth": {
                    "accessToken": format!("at-{email}"),
                    "refreshToken": refresh,
                    "expiresAt": crate::now_ms() + 3_600_000,
                    "scopes": ["user:inference", "user:profile"],
                    "subscriptionType": "max",
                },
                "mcpOAuth": { "server": { "accessToken": "mcp-keep" } },
            }),
        );
        write(
            &self.config.claude_config_file,
            &json!({
                "theme": "dark",
                "oauthAccount": {
                    "accountUuid": format!("uuid-{email}"),
                    "emailAddress": email,
                    "organizationName": "Acme",
                    "displayName": "Me",
                },
                "userID": format!("uid-{email}"),
            }),
        );
    }

    /// Every saved slot of `slug` (`codex`, `claude-code`), raw.
    fn slots(&self, slug: &str) -> Vec<Value> {
        let dir = self.config.data_dir.join("agent-accounts").join(slug);
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .filter_map(|e| read(&e.path()))
            .collect()
    }

    fn state(&self) -> State {
        lock(&self.sync.inner.state).clone()
    }

    async fn credential(&self, harness: HarnessId) -> Option<String> {
        self.sync.credential(harness).await
    }
}

fn write(path: &Path, value: &Value) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, value.to_string()).unwrap();
}

fn read(path: &Path) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// A grant request was signed by the key this device enrolled.
fn assert_signed(server: &mock::Server, grant: &Value) {
    let enrolled = server.enrolled();
    let public = B64URL
        .decode(enrolled.last().unwrap()["publicKey"].as_str().unwrap())
        .unwrap();
    let message = crate::runner::RunnerIdentity::grant_message(
        "user1",
        "laptop-1",
        grant["provider"].as_str().unwrap(),
        grant["ts"].as_i64().unwrap(),
    );
    let sig = B64URL.decode(grant["sig"].as_str().unwrap()).unwrap();
    ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, public)
        .verify(message.as_bytes(), &sig)
        .expect("grant signed by the enrolled device key");
}

fn local_row(id: &str, harness: HarnessId, kind: AgentAuthKind, active: bool) -> AgentAccount {
    AgentAccount {
        id: id.into(),
        harness,
        email: Some(format!("{id}@x.com")),
        plan_label: None,
        active,
        usage_windows: Vec::new(),
        usage_fetched_at: None,
        usage_error: None,
        display_name: None,
        organization: None,
        auth_kind: Some(kind),
        switchable: true,
        saved_at: None,
        provider: None,
    }
}

fn ids(snapshot: &AgentAccountsSnapshot) -> Vec<&str> {
    snapshot.accounts.iter().map(|a| a.id.as_str()).collect()
}

fn active_of(snapshot: &AgentAccountsSnapshot, id: &str) -> bool {
    snapshot
        .accounts
        .iter()
        .find(|a| a.id == id)
        .unwrap_or_else(|| panic!("no row {id}: {snapshot:?}"))
        .active
}

// ── 1. Claude: enable ───────────────────────────────────────────────────────

#[tokio::test]
async fn claude_enable_uploads_the_live_login_and_installs_its_grant_beside_mcp_tokens() {
    let server = mock::start().await;
    let d = device(&server);
    d.claude_cli_login("me@x.com", "rt-me");

    let status = d.sync.set_enabled(true).await.unwrap();
    assert!(status.enabled && status.message.is_none(), "{status:?}");

    // Uploaded with its real refresh token and its identity, for every
    // device, without taking Cloud's active account.
    let uploads = server.uploads_for("claude");
    assert_eq!(uploads.len(), 1, "{uploads:?}");
    let body = &uploads[0];
    assert_eq!(body["material"]["claudeAiOauth"]["refreshToken"], "rt-me");
    assert_eq!(
        body["material"]["claudeAiOauth"]["accessToken"],
        "at-me@x.com"
    );
    assert!(body["material"].get("mcpOAuth").is_none(), "{body}");
    assert_eq!(body["profile"]["email"], "me@x.com");
    assert_eq!(body["profile"]["organization"], "Acme");
    assert_eq!(body["authorizedDevices"], json!(["*"]));
    assert_eq!(body["activate"], false);

    // Its slot is gone: the vault alone refreshes it now.
    let slots = d.slots("claude-code");
    assert!(
        slots.iter().all(|s| !s.to_string().contains("rt-me")),
        "{slots:?}"
    );

    // The grant was asked for by slot, and installed with an empty refresh
    // token next to the machine-shared MCP tokens.
    let grants = server.grants_for("claude");
    assert_eq!(grants.len(), 1);
    assert_eq!(grants[0]["slot"], "claude-1");
    assert_signed(&server, &grants[0]);
    let creds = read(&d.claude_creds_file()).unwrap();
    assert_eq!(creds["claudeAiOauth"]["accessToken"], "granted-claude-1");
    assert_eq!(creds["claudeAiOauth"]["refreshToken"], "");
    assert_eq!(creds["mcpOAuth"]["server"]["accessToken"], "mcp-keep");

    // The login's identity is Claude Code's identity.
    let cfg = read(&d.config.claude_config_file).unwrap();
    assert_eq!(cfg["theme"], "dark");
    assert_eq!(cfg["oauthAccount"]["emailAddress"], "me@x.com");
    assert_eq!(cfg["oauthAccount"]["accountUuid"], "uuid-me@x.com");
    assert_eq!(cfg["userID"], "uid-me@x.com");
    assert_eq!(
        d.state().identities["claude-1"]["oauthAccount"]["emailAddress"],
        "me@x.com"
    );
    assert_eq!(
        d.accounts.live_login(HarnessId::ClaudeCode).await,
        Some(("me@x.com".into(), Some("Acme".into()), false))
    );

    assert_eq!(
        d.sync.active_slot(HarnessId::ClaudeCode).as_deref(),
        Some("claude-1")
    );
    assert_eq!(
        d.credential(HarnessId::ClaudeCode).await,
        Some("synced:claude-1".into())
    );
    // Codex had nothing to sync.
    assert!(server.grants_for("codex").is_empty());
    assert!(!d.codex_file().exists());
}

// ── 2. Codex API key ────────────────────────────────────────────────────────

#[tokio::test]
async fn a_codex_api_key_is_never_replaced_by_a_synced_login() {
    let server = mock::start().await;
    server.add_account("codex", "codex-cloud", "cloud@x.com", true);
    let d = device(&server);
    write(&d.codex_file(), &json!({ "OPENAI_API_KEY": "sk-mine" }));
    let before = std::fs::read(d.codex_file()).unwrap();

    let status = d.sync.set_enabled(true).await.unwrap();
    assert!(status.enabled && status.message.is_none(), "{status:?}");

    assert_eq!(std::fs::read(d.codex_file()).unwrap(), before);
    assert_eq!(d.sync.active_slot(HarnessId::Codex), None);
    assert_eq!(d.credential(HarnessId::Codex).await, None);
    assert!(server.uploads_for("codex").is_empty());
    assert!(server.grants_for("codex").is_empty());
    // Cloud's login was still made every device's.
    assert!(server.authorized().contains(&"codex".to_string()));
}

// ── 3. Fresh device ─────────────────────────────────────────────────────────

#[tokio::test]
async fn a_fresh_device_installs_the_vaults_active_logins() {
    let server = mock::start().await;
    server.add_account("codex", "codex-a", "a@x.com", false);
    server.add_account("codex", "codex-b", "b@x.com", true);
    server.add_account("claude", "claude-a", "ca@x.com", false);
    server.add_account("claude", "claude-b", "cb@x.com", true);
    let d = device(&server);
    d.sync.update(|state| {
        for slot in ["claude-a", "claude-b"] {
            state.identities.insert(
                slot.into(),
                json!({ "oauthAccount": {
                    "accountUuid": format!("uuid-{slot}"),
                    "emailAddress": format!("{slot}@x.com"),
                } }),
            );
        }
    });

    let status = d.sync.set_enabled(true).await.unwrap();
    assert!(status.enabled && status.message.is_none(), "{status:?}");

    assert!(server.uploads().is_empty());
    let authorized = server.authorized();
    assert!(authorized.contains(&"codex".to_string()), "{authorized:?}");
    assert!(authorized.contains(&"claude".to_string()), "{authorized:?}");

    assert_eq!(
        d.sync.active_slot(HarnessId::Codex).as_deref(),
        Some("codex-b")
    );
    let live = read(&d.codex_file()).unwrap();
    assert_eq!(live["tokens"]["access_token"], "granted-codex-b");
    assert_eq!(live["tokens"]["refresh_token"], "");
    assert_eq!(
        d.accounts.live_login(HarnessId::Codex).await,
        Some(("b@x.com".into(), None, false))
    );

    assert_eq!(
        d.sync.active_slot(HarnessId::ClaudeCode).as_deref(),
        Some("claude-b")
    );
    let creds = read(&d.claude_creds_file()).unwrap();
    assert_eq!(creds["claudeAiOauth"]["accessToken"], "granted-claude-b");
    assert_eq!(creds["claudeAiOauth"]["refreshToken"], "");
    let cfg = read(&d.config.claude_config_file).unwrap();
    assert_eq!(cfg["oauthAccount"]["emailAddress"], "claude-b@x.com");
}

// ── 4. Background adoption ──────────────────────────────────────────────────

#[tokio::test]
async fn the_background_pass_adopts_a_login_signed_in_with_the_cli() {
    let server = mock::start().await;
    let d = device(&server);
    d.sync.set_enabled(true).await.unwrap();
    assert!(server.grants().is_empty());
    assert_eq!(d.sync.active_slot(HarnessId::Codex), None);

    // The user signs in with the CLIs themselves.
    write(&d.codex_file(), &mock::codex_auth("cli@x.com", "rt-cli"));
    d.claude_cli_login("cc@x.com", "rt-claude-cli");
    d.sync.tick().await;
    assert_eq!(d.sync.status().message, None);

    let codex = server.uploads_for("codex");
    assert_eq!(codex.len(), 1);
    assert_eq!(
        codex[0]["material"]["authJson"]["tokens"]["refresh_token"],
        "rt-cli"
    );
    let claude = server.uploads_for("claude");
    assert_eq!(claude.len(), 1);
    assert_eq!(
        claude[0]["material"]["claudeAiOauth"]["refreshToken"],
        "rt-claude-cli"
    );

    // Slots dropped, grants installed.
    assert!(
        d.slots("codex")
            .iter()
            .all(|s| !s.to_string().contains("rt-cli"))
    );
    assert!(
        d.slots("claude-code")
            .iter()
            .all(|s| !s.to_string().contains("rt-claude-cli"))
    );
    let live = read(&d.codex_file()).unwrap();
    assert_eq!(live["tokens"]["access_token"], "granted-codex-1");
    assert_eq!(live["tokens"]["refresh_token"], "");
    let creds = read(&d.claude_creds_file()).unwrap();
    assert_eq!(creds["claudeAiOauth"]["accessToken"], "granted-claude-1");
    assert_eq!(creds["claudeAiOauth"]["refreshToken"], "");
    assert_eq!(
        d.sync.active_slot(HarnessId::Codex).as_deref(),
        Some("codex-1")
    );
    assert_eq!(
        d.sync.active_slot(HarnessId::ClaudeCode).as_deref(),
        Some("claude-1")
    );

    // Nothing new on the next pass: no upload, fresh tokens left alone.
    let (uploads, grants) = (server.uploads().len(), server.grants().len());
    d.sync.tick().await;
    assert_eq!(server.uploads().len(), uploads);
    assert_eq!(server.grants().len(), grants);
}

// ── 5. Re-grant when due ────────────────────────────────────────────────────

#[tokio::test]
async fn the_background_pass_regrants_only_tokens_due_soon() {
    let server = mock::start().await;
    server.add_account("codex", "codex-1", "a@x.com", true);
    server.set_grant_ttl(5 * 60_000);
    let d = device(&server);
    d.sync.set_enabled(true).await.unwrap();
    assert_eq!(server.grants_for("codex").len(), 1);

    // Five minutes left < REFRESH_AHEAD: re-granted.
    d.sync.tick().await;
    assert_eq!(server.grants_for("codex").len(), 2);

    // Still due (the last grant was short); this one lasts an hour.
    server.set_grant_ttl(3_600_000);
    d.sync.tick().await;
    assert_eq!(server.grants_for("codex").len(), 3);
    let expires = lock(&d.sync.inner.installed)[&HarnessId::Codex].expires_at;
    assert!(expires - crate::now_ms() > REFRESH_AHEAD_MS, "{expires}");

    // Not due: no request.
    d.sync.tick().await;
    assert_eq!(server.grants_for("codex").len(), 3);
    assert_eq!(d.sync.status().message, None);
}

// ── 6. Grant replies ────────────────────────────────────────────────────────

#[tokio::test]
async fn a_stale_grant_timestamp_is_retried_with_a_newer_one() {
    let server = mock::start().await;
    server.add_account("codex", "codex-1", "a@x.com", true);
    server.script_grant(
        401,
        json!({ "error": "stale", "message": "timestamp not newer" }),
    );
    let d = device(&server);

    let status = d.sync.set_enabled(true).await.unwrap();
    assert!(status.message.is_none(), "{status:?}");

    let grants = server.grants_for("codex");
    assert_eq!(grants.len(), 2, "{grants:?}");
    assert!(
        grants[1]["ts"].as_i64() > grants[0]["ts"].as_i64(),
        "{grants:?}"
    );
    for grant in &grants {
        assert_signed(&server, grant);
    }
    assert_eq!(
        d.sync.active_slot(HarnessId::Codex).as_deref(),
        Some("codex-1")
    );
    assert_eq!(
        read(&d.codex_file()).unwrap()["tokens"]["access_token"],
        "granted-codex-1"
    );
}

#[tokio::test]
async fn an_unknown_device_enrolls_again_and_retries() {
    let server = mock::start().await;
    server.add_account("codex", "codex-1", "a@x.com", true);
    server.script_grant(403, json!({ "error": "device_unknown" }));
    let d = device(&server);

    let status = d.sync.set_enabled(true).await.unwrap();
    assert!(status.message.is_none(), "{status:?}");

    // Enrolled by the switch, then again after the refusal — same key.
    let enrolled = server.enrolled();
    assert_eq!(enrolled.len(), 2, "{enrolled:?}");
    for body in &enrolled {
        assert_eq!(body["deviceId"], "laptop-1");
        assert_eq!(body["kind"], "laptop");
    }
    assert_eq!(enrolled[0]["publicKey"], enrolled[1]["publicKey"]);
    assert_eq!(server.grants_for("codex").len(), 2);
    assert_eq!(
        read(&d.codex_file()).unwrap()["tokens"]["access_token"],
        "granted-codex-1"
    );
    assert_eq!(d.state().enrolled_for.as_deref(), Some("user1"));
}

#[tokio::test]
async fn a_grant_that_doesnt_name_its_slot_is_never_installed() {
    let server = mock::start().await;
    server.add_account("codex", "codex-1", "a@x.com", true);
    server.omit_grant_slot();
    let d = device(&server);

    let err = d.sync.set_enabled(true).await.unwrap_err();
    assert!(
        err.to_string().contains("don't support synced logins"),
        "{err}"
    );

    assert_eq!(server.grants_for("codex").len(), 1);
    assert!(!d.codex_file().exists());
    assert!(d.state().active.is_empty(), "{:?}", d.state());
    assert_eq!(d.credential(HarnessId::Codex).await, None);
    assert!(
        d.sync
            .status()
            .message
            .is_some_and(|m| m.contains("don't support synced logins"))
    );
}

#[tokio::test]
async fn a_login_removed_from_the_vault_is_signed_out_at_its_next_refresh() {
    let server = mock::start().await;
    server.add_account("codex", "codex-1", "a@x.com", true);
    server.set_grant_ttl(60_000);
    let d = device(&server);
    d.sync.set_enabled(true).await.unwrap();
    assert!(d.codex_file().exists());

    // Another device forgets it.
    server.remove_account("codex", "codex-1");
    d.sync.tick().await;

    assert_eq!(d.sync.active_slot(HarnessId::Codex), None);
    assert_eq!(d.credential(HarnessId::Codex).await, None);
    assert!(!d.codex_file().exists());
    assert!(
        lock(&d.sync.inner.installed)
            .get(&HarnessId::Codex)
            .is_none()
    );
    let message = d.sync.status().message.unwrap_or_default();
    assert!(message.contains(GONE), "{message}");

    // Nothing left to refresh.
    let grants = server.grants().len();
    d.sync.tick().await;
    assert_eq!(server.grants().len(), grants);
}

#[tokio::test]
async fn a_login_needing_reconnect_stays_in_use_and_says_so() {
    let server = mock::start().await;
    server.add_account("codex", "codex-1", "a@x.com", true);
    server.set_grant_ttl(60_000);
    let d = device(&server);
    d.sync.set_enabled(true).await.unwrap();

    server.script_grant(409, json!({ "error": "needs_reconnect" }));
    d.sync.tick().await;

    let message = d.sync.status().message.unwrap_or_default();
    assert!(message.contains("sign in to it again"), "{message}");
    assert_eq!(
        d.sync.active_slot(HarnessId::Codex).as_deref(),
        Some("codex-1")
    );
    assert_eq!(
        read(&d.codex_file()).unwrap()["tokens"]["access_token"],
        "granted-codex-1"
    );
}

// ── 7. Forget ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn forgetting_a_synced_login_signs_out_its_grant_only_when_in_use() {
    let server = mock::start().await;
    server.add_account("codex", "codex-a", "a@x.com", true);
    server.add_account("codex", "codex-b", "b@x.com", false);
    let d = device(&server);
    d.sync.set_enabled(true).await.unwrap();
    assert_eq!(
        d.sync.active_slot(HarnessId::Codex).as_deref(),
        Some("codex-a")
    );

    // Not this device's: removed from the vault, the live login stays.
    d.sync
        .forget(HarnessId::Codex, "codex:codex-b")
        .await
        .unwrap();
    assert_eq!(
        server.forgotten(),
        vec![("codex".to_string(), "codex-b".to_string())]
    );
    assert_eq!(
        d.sync.active_slot(HarnessId::Codex).as_deref(),
        Some("codex-a")
    );
    assert_eq!(
        read(&d.codex_file()).unwrap()["tokens"]["access_token"],
        "granted-codex-a"
    );

    // This device's: removed, and its grant signed out.
    d.sync
        .forget(HarnessId::Codex, "codex:codex-a")
        .await
        .unwrap();
    assert_eq!(server.forgotten().len(), 2);
    assert_eq!(server.slots("codex"), Vec::<String>::new());
    assert_eq!(d.sync.active_slot(HarnessId::Codex), None);
    assert!(!d.codex_file().exists());

    // In use, but the user has since signed in with the CLI: their own
    // login is never signed out.
    server.add_account("codex", "codex-c", "c@x.com", false);
    d.sync
        .activate(HarnessId::Codex, "codex:codex-c")
        .await
        .unwrap();
    assert_eq!(
        d.sync.active_slot(HarnessId::Codex).as_deref(),
        Some("codex-c")
    );
    let own = mock::codex_auth("own@x.com", "rt-own");
    write(&d.codex_file(), &own);
    d.sync
        .forget(HarnessId::Codex, "codex:codex-c")
        .await
        .unwrap();
    assert_eq!(d.sync.active_slot(HarnessId::Codex), None);
    assert_eq!(read(&d.codex_file()).unwrap(), own);
}

// ── 8. A local row ──────────────────────────────────────────────────────────

#[tokio::test]
async fn switching_to_a_local_row_stops_installing_synced_grants() {
    let server = mock::start().await;
    server.add_account("codex", "codex-1", "a@x.com", true);
    server.set_grant_ttl(60_000);
    let d = device(&server);
    d.sync.set_enabled(true).await.unwrap();
    assert_eq!(
        d.credential(HarnessId::Codex).await,
        Some("synced:codex-1".into())
    );
    let grants = server.grants().len();

    // The user picks their API key row (the accounts layer writes it).
    let key = json!({ "OPENAI_API_KEY": "sk-mine" });
    write(&d.codex_file(), &key);
    d.sync.release(HarnessId::Codex);

    assert_eq!(d.sync.active_slot(HarnessId::Codex), None);
    assert_eq!(d.credential(HarnessId::Codex).await, None);
    assert!(
        lock(&d.sync.inner.installed)
            .get(&HarnessId::Codex)
            .is_none()
    );

    // The (due) grant is not renewed over the key.
    d.sync.tick().await;
    assert_eq!(server.grants().len(), grants);
    assert_eq!(read(&d.codex_file()).unwrap(), key);
    assert!(d.sync.is_enabled());
}

// ── 9. Providers page ───────────────────────────────────────────────────────

#[tokio::test]
async fn the_page_lists_vault_logins_in_place_of_local_subscription_rows() {
    let server = mock::start().await;
    server.add_account("codex", "codex-1", "a@x.com", true);
    server.add_account("codex", "codex-2", "b@x.com", false);
    server.add_account("claude", "claude-1", "c@x.com", true);
    let d = device(&server);
    let local = AgentAccountsSnapshot {
        accounts: vec![
            local_row("cursor-1", HarnessId::Cursor, AgentAuthKind::Oauth, true),
            local_row("codex-oauth", HarnessId::Codex, AgentAuthKind::Oauth, false),
            local_row("codex-key", HarnessId::Codex, AgentAuthKind::ApiKey, true),
            local_row(
                "claude-oauth",
                HarnessId::ClaudeCode,
                AgentAuthKind::Oauth,
                true,
            ),
            local_row("grok-1", HarnessId::Grok, AgentAuthKind::Oauth, true),
        ],
        warnings: vec![AgentAccountWarning {
            harness: HarnessId::Grok,
            message: "local warning".into(),
        }],
    };

    // Off: the device's own view.
    assert_eq!(d.sync.list(local.clone(), false).await, local);

    d.sync.update(|state| {
        state.enabled = true;
        state.active.insert(HarnessId::Codex, "codex-1".into());
    });
    let listed = d.sync.list(local.clone(), false).await;
    assert_eq!(
        ids(&listed),
        vec![
            "cursor-1",
            "codex:codex-1",
            "codex:codex-2",
            "codex-key",
            "claude:claude-1",
            "grok-1"
        ]
    );
    assert!(active_of(&listed, "codex:codex-1"));
    assert!(!active_of(&listed, "codex:codex-2"));
    // A synced login in use outranks the key.
    assert!(!active_of(&listed, "codex-key"));
    // Active means THIS device's pick, not Cloud's.
    assert!(!active_of(&listed, "claude:claude-1"));
    assert!(active_of(&listed, "cursor-1"));
    assert!(active_of(&listed, "grok-1"));
    assert_eq!(listed.warnings, local.warnings);

    // The key in use again: it stays the active row.
    d.sync.release(HarnessId::Codex);
    let listed = d.sync.list(local.clone(), false).await;
    assert!(active_of(&listed, "codex-key"));
    assert!(!active_of(&listed, "codex:codex-1"));

    // Agents without local rows get theirs appended, in agent order.
    let bare = AgentAccountsSnapshot {
        accounts: vec![local_row(
            "cursor-1",
            HarnessId::Cursor,
            AgentAuthKind::Oauth,
            true,
        )],
        warnings: Vec::new(),
    };
    let listed = d.sync.list(bare, false).await;
    assert_eq!(
        ids(&listed),
        vec![
            "cursor-1",
            "claude:claude-1",
            "codex:codex-1",
            "codex:codex-2"
        ]
    );
}

// ── 10. Disable ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn disabling_keeps_the_vaults_logins_and_drops_only_grant_snapshots() {
    let server = mock::start().await;
    server.add_account("codex", "codex-1", "a@x.com", true);
    let d = device(&server);
    d.sync.set_enabled(true).await.unwrap();
    let grant = std::fs::read(d.codex_file()).unwrap();

    // A list snapshots the grant into a slot; a real login saved since
    // gets a slot of its own.
    d.accounts.list(false).await.unwrap();
    write(&d.codex_file(), &mock::codex_auth("b@x.com", "rt-b"));
    d.accounts.list(false).await.unwrap();
    std::fs::write(d.codex_file(), &grant).unwrap();
    assert_eq!(d.slots("codex").len(), 2, "{:?}", d.slots("codex"));

    let status = d.sync.set_enabled(false).await.unwrap();
    assert!(!status.enabled && status.message.is_none(), "{status:?}");

    let state = d.state();
    assert!(state.active.is_empty(), "{state:?}");
    assert!(lock(&d.sync.inner.installed).is_empty());
    assert_eq!(d.credential(HarnessId::Codex).await, None);
    // The vault keeps every login; this device keeps its token till expiry.
    assert!(server.forgotten().is_empty());
    assert_eq!(server.slots("codex"), vec!["codex-1".to_string()]);
    assert_eq!(std::fs::read(d.codex_file()).unwrap(), grant);
    // The grant's snapshot is gone, the real login's slot stays.
    let slots = d.slots("codex");
    assert_eq!(slots.len(), 1, "{slots:?}");
    assert_eq!(slots[0]["credentials"]["tokens"]["refresh_token"], "rt-b");
}

// ── 11. Signed out ──────────────────────────────────────────────────────────

#[tokio::test]
async fn sync_needs_a_signed_in_zeron_account() {
    let d = device_with(Arc::new(|| None::<SyncContext>));

    let err = d.sync.set_enabled(true).await.unwrap_err();
    assert!(err.to_string().contains(NOT_SIGNED_IN), "{err}");
    let status = d.sync.status();
    assert!(!status.available && !status.enabled, "{status:?}");
    assert_eq!(status.message.as_deref(), Some(NOT_SIGNED_IN));
    assert!(!d.sync.is_enabled());
    assert!(d.sync.inner.dir.join(KEY_FILE).metadata().is_err());

    // Left on from before the sign-out: the page says why, the background
    // pass notes it, nothing else happens.
    d.sync.update(|state| state.enabled = true);
    let listed = d.sync.list(AgentAccountsSnapshot::default(), false).await;
    assert!(
        listed.warnings.iter().any(|w| w.message == NOT_SIGNED_IN),
        "{listed:?}"
    );
    d.sync.tick().await;
    assert_eq!(d.state().message.as_deref(), Some(NOT_SIGNED_IN));
    let err = d
        .sync
        .activate(HarnessId::Codex, "codex:codex-1")
        .await
        .unwrap_err();
    assert!(err.to_string().contains(NOT_SIGNED_IN), "{err}");
}

// ── 12. Sign-in through Zeron ───────────────────────────────────────────────

#[tokio::test]
async fn a_codex_sign_in_through_zeron_goes_to_the_vault_and_is_used_here() {
    let server = mock::start().await;
    server.add_account("codex", "codex-old", "old@x.com", true);
    let d = device(&server);
    d.sync.set_enabled(true).await.unwrap();
    assert_eq!(
        d.sync.active_slot(HarnessId::Codex).as_deref(),
        Some("codex-old")
    );

    let captured = mock::codex_auth("new@x.com", "rt-new");
    let sink = d.sync.login_sink(HarnessId::Codex);
    sink(captured.clone()).await.unwrap();

    let uploads = server.uploads_for("codex");
    assert_eq!(uploads.len(), 1);
    assert_eq!(uploads[0]["material"]["authJson"], captured);
    assert_eq!(uploads[0]["activate"], false);
    assert_eq!(uploads[0]["authorizedDevices"], json!(["*"]));

    assert_eq!(
        d.sync.active_slot(HarnessId::Codex).as_deref(),
        Some("codex-2")
    );
    assert_eq!(
        server.grants_for("codex").last().unwrap()["slot"],
        "codex-2"
    );
    let live = read(&d.codex_file()).unwrap();
    assert_eq!(live["tokens"]["access_token"], "granted-codex-2");
    assert_eq!(live["tokens"]["refresh_token"], "");
    assert!(d.state().synced_at.is_some());
    // The captured tokens never land in a local slot.
    assert!(
        d.slots("codex")
            .iter()
            .all(|s| !s.to_string().contains("rt-new"))
    );
}

// ── review fixes ────────────────────────────────────────────────────────────

/// `codex login --with-api-key` in a terminal while a synced login is in
/// use: the next re-grant must not replace the user's key; this device lets
/// go of the synced login instead.
#[tokio::test]
async fn an_api_key_set_with_the_cli_is_not_overwritten_by_a_regrant() {
    let server = mock::start().await;
    server.add_account("codex", "codex-1", "a@x.com", true);
    server.set_grant_ttl(60_000);
    let d = device(&server);
    d.sync.set_enabled(true).await.unwrap();
    let grants = server.grants().len();

    let key = json!({ "OPENAI_API_KEY": "sk-cli" });
    write(&d.codex_file(), &key);
    d.sync.tick().await;

    assert_eq!(read(&d.codex_file()).unwrap(), key);
    assert_eq!(server.grants().len(), grants);
    assert_eq!(d.sync.active_slot(HarnessId::Codex), None);
    assert_eq!(d.credential(HarnessId::Codex).await, None);
    assert_eq!(d.sync.status().message, None);
}

/// The live login's refresh token is blanked before its upload; an upload
/// that fails gives it back, so the CLI is never left unable to refresh.
#[tokio::test]
async fn a_failed_upload_gives_the_cli_its_refresh_token_back() {
    let server = mock::start().await;
    server.script_put(400, json!({ "error": "invalid_material" }));
    let d = device(&server);
    let mine = mock::codex_auth("me@x.com", "rt-mine");
    write(&d.codex_file(), &mine);

    let err = d.sync.set_enabled(true).await.unwrap_err();
    assert!(err.to_string().contains("Codex"), "{err}");

    // Tried once, then restored: the CLI still owns (and refreshes) it.
    assert_eq!(server.uploads_for("codex").len(), 1);
    let live = read(&d.codex_file()).unwrap();
    assert_eq!(live["tokens"]["refresh_token"], "rt-mine");
    assert_eq!(
        live["tokens"]["access_token"],
        mine["tokens"]["access_token"]
    );
    assert!(server.grants().is_empty());
    assert!(d.state().active.is_empty(), "{:?}", d.state());
    // Its slot stays too, for the next attempt.
    assert!(
        d.slots("codex")
            .iter()
            .any(|s| s.to_string().contains("rt-mine"))
    );

    // The vault answers again: the next pass hands it over.
    d.sync.tick().await;
    assert_eq!(d.sync.status().message, None);
    assert_eq!(server.uploads_for("codex").len(), 2);
    let live = read(&d.codex_file()).unwrap();
    assert_eq!(live["tokens"]["access_token"], "granted-codex-1");
    assert_eq!(live["tokens"]["refresh_token"], "");
    assert_eq!(
        d.sync.active_slot(HarnessId::Codex).as_deref(),
        Some("codex-1")
    );
}

/// A saved slot of a login the vault already refreshes is an old copy (its
/// refresh token likely spent): dropped, never uploaded over the working
/// one. A saved login the vault doesn't have is uploaded.
#[tokio::test]
async fn saved_copies_of_synced_logins_are_dropped_not_uploaded() {
    let server = mock::start().await;
    server.add_account("codex", "codex-1", "a@x.com", true);
    let d = device(&server);
    // Two saved logins (an old copy of a@ and one the vault lacks), and an
    // API key live.
    write(&d.codex_file(), &mock::codex_auth("a@x.com", "rt-old"));
    d.accounts.list(false).await.unwrap();
    write(&d.codex_file(), &mock::codex_auth("c@x.com", "rt-c"));
    d.accounts.list(false).await.unwrap();
    let key = json!({ "OPENAI_API_KEY": "sk-mine" });
    write(&d.codex_file(), &key);

    let status = d.sync.set_enabled(true).await.unwrap();
    assert!(status.message.is_none(), "{status:?}");

    let uploads = server.uploads_for("codex");
    assert_eq!(uploads.len(), 1, "{uploads:?}");
    assert_eq!(
        uploads[0]["material"]["authJson"]["tokens"]["refresh_token"],
        "rt-c"
    );
    let slots = d.slots("codex");
    assert!(
        slots.iter().all(|s| {
            let s = s.to_string();
            !s.contains("rt-old") && !s.contains("rt-c")
        }),
        "{slots:?}"
    );
    // The key stays the live login.
    assert_eq!(read(&d.codex_file()).unwrap(), key);
    assert_eq!(d.sync.active_slot(HarnessId::Codex), None);
}

/// A terminal sign-in landing during a grant's round trip is the only copy
/// of that login: the install refuses to overwrite it, and the next pass
/// uploads it.
#[tokio::test]
async fn a_sign_in_during_a_grant_round_trip_is_never_overwritten() {
    let server = mock::start().await;
    server.add_account("codex", "codex-1", "a@x.com", true);
    server.set_grant_ttl(60_000);
    let d = device(&server);
    d.sync.set_enabled(true).await.unwrap();

    let file = d.codex_file();
    let own = mock::codex_auth("own@x.com", "rt-own");
    let landed = own.clone();
    server.during_next_grant(move || write(&file, &landed));
    d.sync.tick().await;

    assert_eq!(read(&d.codex_file()).unwrap(), own);
    let message = d.sync.status().message.unwrap_or_default();
    assert!(message.contains("new Codex login appeared"), "{message}");

    d.sync.tick().await;
    let uploads = server.uploads_for("codex");
    assert_eq!(uploads.len(), 1);
    assert_eq!(
        uploads[0]["material"]["authJson"]["tokens"]["refresh_token"],
        "rt-own"
    );
    let live = read(&d.codex_file()).unwrap();
    assert_eq!(live["tokens"]["refresh_token"], "");
    assert_eq!(live["tokens"]["access_token"], "granted-codex-2");
    assert_eq!(
        d.sync.active_slot(HarnessId::Codex).as_deref(),
        Some("codex-2")
    );
}

/// A saved slot that still refreshes (saved while the live login is a grant)
/// is picked up by the background pass; the login in use doesn't change.
#[tokio::test]
async fn the_background_pass_uploads_a_saved_login_that_still_refreshes() {
    let server = mock::start().await;
    server.add_account("codex", "codex-1", "a@x.com", true);
    let d = device(&server);
    d.sync.set_enabled(true).await.unwrap();
    let grant = std::fs::read(d.codex_file()).unwrap();

    write(&d.codex_file(), &mock::codex_auth("d@x.com", "rt-d"));
    d.accounts.list(false).await.unwrap();
    std::fs::write(d.codex_file(), &grant).unwrap();
    assert!(
        d.slots("codex")
            .iter()
            .any(|s| s.to_string().contains("rt-d"))
    );

    d.sync.tick().await;

    assert_eq!(d.sync.status().message, None);
    let uploads = server.uploads_for("codex");
    assert_eq!(uploads.len(), 1);
    assert_eq!(
        uploads[0]["material"]["authJson"]["tokens"]["refresh_token"],
        "rt-d"
    );
    assert!(
        d.slots("codex")
            .iter()
            .all(|s| !s.to_string().contains("rt-d"))
    );
    assert_eq!(
        d.sync.active_slot(HarnessId::Codex).as_deref(),
        Some("codex-1")
    );
    assert_eq!(
        read(&d.codex_file()).unwrap()["tokens"]["access_token"],
        "granted-codex-1"
    );
}

// ── mock vault ──────────────────────────────────────────────────────────────

/// A stand-in for the edge's `/vault/{org}/…` routes (raw HTTP/1.1, one
/// request per connection), scriptable per test.
mod mock {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use serde_json::{Value, json};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    pub struct Token;

    #[async_trait::async_trait]
    impl zeron_rpc::TokenSource for Token {
        async fn token(&self) -> Result<String, zeron_rpc::TokenError> {
            Ok("bearer".into())
        }
    }

    fn jwt(claims: &Value) -> String {
        use base64::Engine as _;
        let enc =
            |v: &Value| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string());
        format!("{}.{}.sig", enc(&json!({"alg": "none"})), enc(claims))
    }

    /// A ChatGPT `auth.json` as `codex login` writes it.
    pub fn codex_auth(email: &str, refresh: &str) -> Value {
        let claims = json!({
            "email": email,
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "ws-1",
                "chatgpt_user_id": format!("user-{email}"),
                "chatgpt_plan_type": "pro",
            },
        });
        json!({
            "OPENAI_API_KEY": null,
            "tokens": {
                "id_token": jwt(&claims),
                "access_token": format!("at-{email}"),
                "refresh_token": refresh,
                "account_id": "ws-1",
            },
            "last_refresh": "2026-11-01T00:00:00Z",
        })
    }

    struct Vault {
        /// `(provider, account)`.
        accounts: Vec<(String, Value)>,
        uploads: Vec<(String, Value)>,
        grants: Vec<Value>,
        enrolled: Vec<Value>,
        authorized: Vec<String>,
        forgotten: Vec<(String, String)>,
        /// Replies for the next grant requests, before any real one.
        grant_script: VecDeque<(u16, Value)>,
        grant_ttl_ms: i64,
        /// A vault from before per-device logins: grants don't name a slot.
        omit_slot: bool,
        /// Replies for the next uploads, before any real one.
        put_script: VecDeque<(u16, Value)>,
        /// Runs once, while the next grant request is being answered.
        on_grant: Option<Box<dyn FnOnce() + Send>>,
    }

    #[derive(Clone)]
    pub struct Server {
        pub url: String,
        vault: Arc<Mutex<Vault>>,
    }

    impl Server {
        fn with<T>(&self, f: impl FnOnce(&mut Vault) -> T) -> T {
            f(&mut self.vault.lock().unwrap())
        }
        pub fn uploads(&self) -> Vec<Value> {
            self.with(|v| v.uploads.iter().map(|(_, b)| b.clone()).collect())
        }
        pub fn uploads_for(&self, provider: &str) -> Vec<Value> {
            self.with(|v| {
                v.uploads
                    .iter()
                    .filter(|(p, _)| p == provider)
                    .map(|(_, b)| b.clone())
                    .collect()
            })
        }
        pub fn grants(&self) -> Vec<Value> {
            self.with(|v| v.grants.clone())
        }
        pub fn grants_for(&self, provider: &str) -> Vec<Value> {
            self.with(|v| {
                v.grants
                    .iter()
                    .filter(|g| g["provider"] == provider)
                    .cloned()
                    .collect()
            })
        }
        pub fn enrolled(&self) -> Vec<Value> {
            self.with(|v| v.enrolled.clone())
        }
        pub fn authorized(&self) -> Vec<String> {
            self.with(|v| v.authorized.clone())
        }
        pub fn forgotten(&self) -> Vec<(String, String)> {
            self.with(|v| v.forgotten.clone())
        }
        pub fn slots(&self, provider: &str) -> Vec<String> {
            self.with(|v| {
                v.accounts
                    .iter()
                    .filter(|(p, _)| p == provider)
                    .filter_map(|(_, a)| a["slot"].as_str().map(str::to_string))
                    .collect()
            })
        }
        /// A login stored earlier (Cloud's, another device's).
        pub fn add_account(&self, provider: &str, slot: &str, email: &str, active: bool) {
            self.with(|v| {
                v.accounts
                    .push((provider.into(), account(slot, email, active, 1)))
            });
        }
        /// Removed from another device.
        pub fn remove_account(&self, provider: &str, slot: &str) {
            self.with(|v| {
                v.accounts
                    .retain(|(p, a)| !(p == provider && a["slot"] == slot))
            });
        }
        pub fn script_grant(&self, status: u16, body: Value) {
            self.with(|v| v.grant_script.push_back((status, body)));
        }
        pub fn set_grant_ttl(&self, ms: i64) {
            self.with(|v| v.grant_ttl_ms = ms);
        }
        pub fn omit_grant_slot(&self) {
            self.with(|v| v.omit_slot = true);
        }
        pub fn script_put(&self, status: u16, body: Value) {
            self.with(|v| v.put_script.push_back((status, body)));
        }
        /// Something that happens on the device during the next grant's
        /// round trip.
        pub fn during_next_grant(&self, f: impl FnOnce() + Send + 'static) {
            self.with(|v| v.on_grant = Some(Box::new(f)));
        }
    }

    fn account(slot: &str, email: &str, active: bool, updated_at: i64) -> Value {
        json!({
            "slot": slot, "status": "connected", "active": active, "kind": "oauth",
            "account": email, "email": email, "authorizedDevices": ["*"],
            "createdAt": 1, "updatedAt": updated_at,
        })
    }

    fn status() -> Value {
        json!({ "connections": [], "devices": [], "available": true })
    }

    fn list(v: &Vault, provider: &str) -> Value {
        let accounts: Vec<Value> = v
            .accounts
            .iter()
            .filter(|(p, _)| p == provider)
            .map(|(_, a)| a.clone())
            .collect();
        json!({ "accounts": accounts })
    }

    fn route(vault: &Mutex<Vault>, method: &str, path: &str, body: Value) -> (u16, Value) {
        let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
        let mut v = vault.lock().unwrap();
        match (method, parts.as_slice()) {
            ("PUT", ["vault", _, "credentials", provider]) => {
                let provider = provider.to_string();
                if let Some(reply) = v.put_script.pop_front() {
                    v.uploads.push((provider, body));
                    return reply;
                }
                let mut n = v.accounts.iter().filter(|(p, _)| *p == provider).count() + 1;
                while v
                    .accounts
                    .iter()
                    .any(|(p, a)| *p == provider && a["slot"] == format!("{provider}-{n}"))
                {
                    n += 1;
                }
                let none_active = !v
                    .accounts
                    .iter()
                    .any(|(p, a)| *p == provider && a["active"] == true);
                let email = body["profile"]["email"].as_str().unwrap_or("x").to_string();
                let slot = format!("{provider}-{n}");
                let fresh = account(&slot, &email, none_active, crate::now_ms());
                v.accounts.push((provider.clone(), fresh));
                v.uploads.push((provider, body));
                let mut reply = status();
                reply["slot"] = json!(slot);
                (200, reply)
            }
            ("PATCH", ["vault", _, "credentials", provider]) => {
                v.authorized.push(provider.to_string());
                (200, status())
            }
            ("GET", ["vault", _, "accounts", provider]) => (200, list(&v, provider)),
            ("DELETE", ["vault", _, "accounts", provider, slot]) => {
                let (provider, slot) = (provider.to_string(), slot.to_string());
                v.accounts
                    .retain(|(p, a)| !(*p == provider && a["slot"] == slot.as_str()));
                v.forgotten.push((provider.clone(), slot));
                (200, list(&v, &provider))
            }
            ("POST", ["vault", _, "devices", "enroll"]) => {
                v.enrolled.push(body);
                (200, status())
            }
            ("POST", ["vault", _, "grant"]) => {
                v.grants.push(body.clone());
                if let Some(hook) = v.on_grant.take() {
                    hook();
                }
                if let Some(reply) = v.grant_script.pop_front() {
                    return reply;
                }
                let provider = body["provider"].as_str().unwrap_or_default().to_string();
                let slot = body["slot"].as_str().unwrap_or_default().to_string();
                let email = v
                    .accounts
                    .iter()
                    .find(|(p, a)| *p == provider && a["slot"] == slot.as_str())
                    .map(|(_, a)| a["email"].clone());
                let Some(email) = email else {
                    return (
                        404,
                        json!({ "error": "not_found", "message": format!("no such {provider} account") }),
                    );
                };
                let mut grant = json!({
                    "provider": provider,
                    "accessToken": format!("granted-{slot}"),
                    "expiresAt": crate::now_ms() + v.grant_ttl_ms,
                    "generation": 1,
                    "account": email,
                    "scopes": ["user:inference"],
                    "subscriptionType": "max",
                    "slot": slot,
                });
                if v.omit_slot {
                    grant.as_object_mut().unwrap().remove("slot");
                }
                (200, grant)
            }
            _ => (404, json!({ "error": "not_found" })),
        }
    }

    pub async fn start() -> Server {
        let vault = Arc::new(Mutex::new(Vault {
            accounts: Vec::new(),
            uploads: Vec::new(),
            grants: Vec::new(),
            enrolled: Vec::new(),
            authorized: Vec::new(),
            forgotten: Vec::new(),
            grant_script: VecDeque::new(),
            grant_ttl_ms: 3_600_000,
            omit_slot: false,
            put_script: VecDeque::new(),
            on_grant: None,
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let shared = vault.clone();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let vault = shared.clone();
                tokio::spawn(async move {
                    let mut raw = Vec::new();
                    let mut chunk = [0u8; 4096];
                    let header_end = loop {
                        let Ok(n) = stream.read(&mut chunk).await else {
                            return;
                        };
                        if n == 0 {
                            return;
                        }
                        raw.extend_from_slice(&chunk[..n]);
                        if let Some(at) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                            break at + 4;
                        }
                    };
                    let head = String::from_utf8_lossy(&raw[..header_end]).to_string();
                    let mut lines = head.lines();
                    let mut request_line = lines.next().unwrap_or("").split_whitespace();
                    let method = request_line.next().unwrap_or("").to_string();
                    let target = request_line.next().unwrap_or("").to_string();
                    let length = lines
                        .filter_map(|line| line.split_once(':'))
                        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    while raw.len() < header_end + length {
                        let Ok(n) = stream.read(&mut chunk).await else {
                            return;
                        };
                        if n == 0 {
                            break;
                        }
                        raw.extend_from_slice(&chunk[..n]);
                    }
                    let body = serde_json::from_slice(&raw[header_end..]).unwrap_or(Value::Null);
                    let path = target.split('?').next().unwrap_or("").to_string();
                    let (status, reply) = route(&vault, &method, &path, body);
                    let reply = reply.to_string();
                    let response = format!(
                        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\n\
                         content-length: {}\r\nconnection: close\r\n\r\n{reply}",
                        reply.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.shutdown().await;
                });
            }
        });
        Server { url, vault }
    }
}
