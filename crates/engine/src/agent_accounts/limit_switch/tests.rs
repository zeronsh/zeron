//! Account switches for limited chats, against accounts on disk under each
//! test's own root: which account takes over, what the chat's next run
//! gets, what stays as it was, and what survives a restart.

use super::super::limit_switch::fixtures::*;
use super::super::*;
use crate::registry::RunEnvironment;

const CHAT: &str = "chat-1";

fn row(id: &str, harness: HarnessId, windows: Vec<AgentUsageWindow>, fetched_ago_ms: i64) -> AgentAccount {
    AgentAccount {
        id: id.into(),
        harness,
        email: Some(format!("{id}@example.com")),
        plan_label: None,
        active: false,
        usage_windows: windows,
        usage_fetched_at: Some(now_ms() - fetched_ago_ms),
        usage_error: None,
        codex_reset_credits: None,
        display_name: None,
        organization: None,
        auth_kind: Some(AgentAuthKind::Oauth),
        switchable: true,
        saved_at: None,
        provider: None,
        invalid: false,
        invalid_reason: None,
    }
}

fn request() -> zeron_proto::RunRequest {
    serde_json::from_str(r#"{"prompt":"p","model":null,"reasoning":null,"cwd":".","sandbox":"workspace-write","resume":null}"#)
        .unwrap()
}

async fn prepared(accounts: &AgentAccounts, harness: HarnessId) -> zeron_proto::RunRequest {
    let mut request = request();
    run_environment(accounts).prepare(harness, Some(CHAT), &mut request).await.unwrap();
    request
}

async fn listed(accounts: &AgentAccounts, id: &str) -> AgentAccount {
    accounts.list(false).await.unwrap().accounts.into_iter().find(|a| a.id == id).unwrap()
}

#[test]
fn the_pick_takes_the_most_headroom_and_never_a_full_signed_out_stale_or_unswitchable_account() {
    let now = now_ms();
    let any = |_: &AgentAccount| true;
    let half = || vec![window(0.5, 3_600_000)];
    let mut unswitchable = row("unswitchable", HarnessId::ClaudeCode, vec![window(0.0, 3_600_000)], 0);
    unswitchable.switchable = false;
    let mut signed_out = row("signed-out", HarnessId::ClaudeCode, vec![window(0.0, 3_600_000)], 0);
    signed_out.usage_error = Some("Sign in again".into());
    let mut api_key = row("api-key", HarnessId::ClaudeCode, vec![window(0.0, 3_600_000)], 0);
    api_key.auth_kind = Some(AgentAuthKind::ApiKey);
    let accounts = vec![
        row("limited", HarnessId::ClaudeCode, vec![window(0.1, 3_600_000)], 0),
        row("full", HarnessId::ClaudeCode, vec![window(0.2, 3_600_000), window(1.0, 3_600_000)], 0),
        row("stale", HarnessId::ClaudeCode, vec![window(0.0, 3_600_000)], FRESH_USAGE_MS + 60_000),
        row("never-probed", HarnessId::ClaudeCode, vec![], 0),
        unswitchable,
        signed_out,
        api_key,
        row("codex", HarnessId::Codex, vec![window(0.0, 3_600_000)], 0),
        row("half", HarnessId::ClaudeCode, half(), 0),
        row("tenth", HarnessId::ClaudeCode, vec![window(0.1, 3_600_000)], 0),
        // Full by a probe whose reset has passed: empty again.
        row("reset", HarnessId::ClaudeCode, vec![window(1.0, -60_000), window(0.3, 3_600_000)], 0),
    ];
    let pick = |exclude: &[&str]| pick_switch_account(&accounts, HarnessId::ClaudeCode, exclude, now, any).map(|a| a.id.clone());
    assert_eq!(pick(&["limited"]).as_deref(), Some("tenth"));
    assert_eq!(pick(&["limited", "tenth"]).as_deref(), Some("reset"));
    assert_eq!(pick(&["limited", "tenth", "reset"]).as_deref(), Some("half"));
    assert_eq!(pick(&["limited", "tenth", "reset", "half"]), None, "nothing else has quota");
    // Only what a chat can run on.
    assert_eq!(pick_switch_account(&accounts, HarnessId::ClaudeCode, &[], now, |a| a.id == "half").map(|a| a.id.as_str()), Some("half"));
    assert_eq!(pick_switch_account(&accounts, HarnessId::Codex, &[], now, any).map(|a| a.id.as_str()), Some("codex"));
}

#[tokio::test]
async fn a_switched_claude_chat_runs_on_the_other_accounts_token_and_the_machine_login_stays() {
    let tmp = tempfile::tempdir().unwrap();
    let accounts = accounts(tmp.path());
    live_claude(tmp.path(), "acct-a", "a@example.com");
    let a = claude_slot(&accounts, "acct-a", "a@example.com", "live-token");
    let b = claude_slot(&accounts, "acct-b", "b@example.com", "at-b");
    let machine = std::fs::read(AgentAccountsConfig::isolated(tmp.path()).claude_creds_file()).unwrap();
    assert!(!prepared(&accounts, HarnessId::ClaudeCode).await.env.contains_key("CLAUDE_CODE_OAUTH_TOKEN"));

    let to = listed(&accounts, &b).await;
    let record = accounts.begin_limit_switch(CHAT, HarnessId::ClaudeCode, Some(&a), HarnessId::ClaudeCode, &to, None);
    assert_eq!((record.account.as_str(), record.label.as_str()), (b.as_str(), "b@example.com"));
    assert_eq!(record.from_account, None, "it ran on the machine's login");
    let run = prepared(&accounts, HarnessId::ClaudeCode).await;
    assert_eq!(run.env.get("CLAUDE_CODE_OAUTH_TOKEN").map(String::as_str), Some("at-b"));
    let env = run_environment(&accounts);
    let on_b = env.credential(HarnessId::ClaudeCode, Some(CHAT)).await;
    assert!(on_b.as_deref().is_some_and(|c| c.starts_with("claude:")), "{on_b:?}");
    assert_eq!(std::fs::read(AgentAccountsConfig::isolated(tmp.path()).claude_creds_file()).unwrap(), machine);

    let back = accounts.end_limit_switch(CHAT).unwrap();
    assert_eq!(back.account, b);
    assert!(!prepared(&accounts, HarnessId::ClaudeCode).await.env.contains_key("CLAUDE_CODE_OAUTH_TOKEN"));
    // A changed credential: the session parked on B is replaced.
    assert_eq!(env.credential(HarnessId::ClaudeCode, Some(CHAT)).await.as_deref(), Some("live"));
    assert!(accounts.limit_switch(CHAT).is_none());
    assert!(accounts.end_limit_switch(CHAT).is_none());
}

#[tokio::test]
async fn a_switched_codex_chat_gets_the_accounts_own_codex_home_beside_the_machines() {
    let tmp = tempfile::tempdir().unwrap();
    let accounts = accounts(tmp.path());
    let config = AgentAccountsConfig::isolated(tmp.path());
    live_codex(tmp.path(), "ws-a", "a@example.com");
    std::fs::write(config.codex_home.join("config.toml"), "model = \"x\"\n").unwrap();
    let a = codex_slot(&accounts, "ws-a", "a@example.com", "live-codex-token");
    let b = codex_slot(&accounts, "ws-b", "b@example.com", "codex-b");
    let machine = std::fs::read(config.codex_auth_file()).unwrap();

    let to = listed(&accounts, &b).await;
    assert_eq!(accounts.pin_mechanism(&to), Some("home"));
    accounts.begin_limit_switch(CHAT, HarnessId::Codex, Some(&a), HarnessId::Codex, &to, None);
    let run = prepared(&accounts, HarnessId::Codex).await;
    let home = std::path::PathBuf::from(run.env.get("CODEX_HOME").expect("its own CODEX_HOME"));
    assert_eq!(Some(home.clone()), accounts.codex_home(&b));
    let auth: serde_json::Value = serde_json::from_slice(&std::fs::read(home.join("auth.json")).unwrap()).unwrap();
    assert_eq!(auth["tokens"]["access_token"], "codex-b");
    // The user's own config and history, linked.
    assert_eq!(std::fs::read_link(home.join("config.toml")).unwrap(), config.codex_home.join("config.toml"));
    assert_eq!(std::fs::read_link(home.join("sessions")).unwrap(), config.codex_home.join("sessions"));
    assert!(std::fs::symlink_metadata(home.join("auth.json")).unwrap().file_type().is_file());
    assert_eq!(std::fs::read(config.codex_auth_file()).unwrap(), machine, "the machine's login stays");
    assert_eq!(
        run_environment(&accounts).credential(HarnessId::Codex, Some(CHAT)).await,
        Some(format!("codex-home:{b}"))
    );

    // Back on the machine's own account: no home of its own.
    accounts.end_limit_switch(CHAT);
    assert!(!prepared(&accounts, HarnessId::Codex).await.env.contains_key("CODEX_HOME"));
    // Pinned to the machine's account itself: as unpinned.
    accounts.set_pin(HarnessId::Codex, CHAT, Some(&a));
    assert!(!prepared(&accounts, HarnessId::Codex).await.env.contains_key("CODEX_HOME"));
    assert!(accounts.codex_home(&a).is_none_or(|home| !home.join("auth.json").exists()));
}

#[tokio::test]
async fn a_codex_homes_refreshed_login_flows_back_to_its_slot_and_moves_to_the_machine_on_a_switch() {
    let tmp = tempfile::tempdir().unwrap();
    let accounts = accounts(tmp.path());
    let config = AgentAccountsConfig::isolated(tmp.path());
    live_codex(tmp.path(), "ws-a", "a@example.com");
    codex_slot(&accounts, "ws-a", "a@example.com", "live-codex-token");
    let b = codex_slot(&accounts, "ws-b", "b@example.com", "codex-b");
    accounts.set_pin(HarnessId::Codex, CHAT, Some(&b));
    let run = prepared(&accounts, HarnessId::Codex).await;
    let home = std::path::PathBuf::from(&run.env["CODEX_HOME"]);
    // The CLI refreshed in its home.
    let refreshed = codex_auth("b@example.com", "ws-b", "codex-b-2", "2026-11-17T12:00:00Z");
    std::fs::write(home.join("auth.json"), refreshed.to_string()).unwrap();
    accounts.list(false).await.unwrap();
    assert_eq!(accounts.read_slot(HarnessId::Codex, &b).unwrap().credentials, refreshed);

    // B becomes the machine's login: the newest copy moves there, the home
    // lets go, and the chat runs on the machine's login.
    accounts.activate(HarnessId::Codex, &b).await.unwrap();
    let machine: serde_json::Value = serde_json::from_slice(&std::fs::read(config.codex_auth_file()).unwrap()).unwrap();
    assert_eq!(machine["tokens"]["access_token"], "codex-b-2");
    assert!(!home.join("auth.json").exists());
    assert!(!prepared(&accounts, HarnessId::Codex).await.env.contains_key("CODEX_HOME"));

    // Removed: its dir goes, the user's linked config stays.
    accounts.activate(HarnessId::Codex, &slot_id_for(HarnessId::Codex, "ws-a")).await.unwrap();
    accounts.set_pin(HarnessId::Codex, CHAT, Some(&b));
    prepared(&accounts, HarnessId::Codex).await;
    assert!(home.join("auth.json").exists());
    accounts.forget(HarnessId::Codex, &b).await.unwrap();
    assert!(!home.exists());
    assert!(config.codex_home.join("sessions").is_dir());
}

#[tokio::test]
async fn a_switch_survives_a_restart_and_a_second_switch_keeps_the_first_before() {
    let tmp = tempfile::tempdir().unwrap();
    live_claude(tmp.path(), "acct-a", "a@example.com");
    let (a, b, c) = {
        let accounts = accounts(tmp.path());
        let a = claude_slot(&accounts, "acct-a", "a@example.com", "live-token");
        let b = claude_slot(&accounts, "acct-b", "b@example.com", "at-b");
        let c = claude_slot(&accounts, "acct-c", "c@example.com", "at-c");
        let to = listed(&accounts, &b).await;
        accounts.begin_limit_switch(CHAT, HarnessId::ClaudeCode, Some(&a), HarnessId::ClaudeCode, &to, None);
        (a, b, c)
    };
    let accounts = accounts(tmp.path());
    let record = accounts.limit_switch(CHAT).expect("kept across a restart");
    assert_eq!(record.account, b);
    assert_eq!(accounts.pinned_account(HarnessId::ClaudeCode, CHAT), Some(b.clone()));
    assert_eq!(prepared(&accounts, HarnessId::ClaudeCode).await.env["CLAUDE_CODE_OAUTH_TOKEN"], "at-b");

    // B ran out too: on to C, still back to the machine's login (A) after.
    let to = listed(&accounts, &c).await;
    let again = accounts.begin_limit_switch(CHAT, HarnessId::ClaudeCode, Some(&b), HarnessId::ClaudeCode, &to, None);
    assert_eq!((again.account.as_str(), again.from_account.clone(), again.limited_account.as_deref()), (c.as_str(), None, Some(a.as_str())));
    assert_eq!(prepared(&accounts, HarnessId::ClaudeCode).await.env["CLAUDE_CODE_OAUTH_TOKEN"], "at-c");
    accounts.end_limit_switch(CHAT);
    assert_eq!(accounts.pinned_account(HarnessId::ClaudeCode, CHAT), None);
}

#[tokio::test]
async fn a_chat_pinned_before_its_limit_goes_back_to_its_pin() {
    let tmp = tempfile::tempdir().unwrap();
    let accounts = accounts(tmp.path());
    live_claude(tmp.path(), "acct-a", "a@example.com");
    claude_slot(&accounts, "acct-a", "a@example.com", "live-token");
    let b = claude_slot(&accounts, "acct-b", "b@example.com", "at-b");
    let c = claude_slot(&accounts, "acct-c", "c@example.com", "at-c");
    accounts.set_pin(HarnessId::ClaudeCode, CHAT, Some(&b));
    let to = listed(&accounts, &c).await;
    let record = accounts.begin_limit_switch(CHAT, HarnessId::ClaudeCode, Some(&b), HarnessId::ClaudeCode, &to, None);
    assert_eq!(record.from_account.as_deref(), Some(b.as_str()));
    accounts.end_limit_switch(CHAT);
    assert_eq!(accounts.pinned_account(HarnessId::ClaudeCode, CHAT), Some(b));
}


#[test]
fn an_account_its_provider_rejected_is_never_switched_to() {
    // (A Cursor key the provider rejected, or any login marked invalid.)
    let w = || vec![AgentUsageWindow { label: "5-hour".into(), used_fraction: 0.2, resets_at: None }];
    let mut bad = row("bad", HarnessId::ClaudeCode, w(), 0);
    bad.invalid = true;
    bad.invalid_reason = Some("Key rejected — sign in again".into());
    let good = row("good", HarnessId::ClaudeCode, w(), 0);
    let accounts = [bad, good];
    let pick = super::pick_switch_account(&accounts, HarnessId::ClaudeCode, &[], now_ms(), |_| true);
    assert_eq!(pick.map(|a| a.id.as_str()), Some("good"));
    let only_bad = [accounts[0].clone()];
    assert!(super::pick_switch_account(&only_bad, HarnessId::ClaudeCode, &[], now_ms(), |_| true).is_none());
}
