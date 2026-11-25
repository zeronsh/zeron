//! Live end-to-end verification against the real `agy_acp_server` 1.5.0 binary
//! through Zeron's [`AcpHarness::antigravity`] and [`EngineCore`].
//!
//! Strictly isolates state in a temporary `GEMINI_HOME` (copying only
//! `acp_token.json` and `settings.json` from `~/.gemini/antigravity-acp/`).
//!
//! Run manually:
//! `cargo test -p zeron-engine --test antigravity_live -- --ignored --nocapture`

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use zeron_doc::{
    MessagePart, MessageRole, MessageStatus, SessionCommandPayload, SessionMessageEntry,
};
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::{AcpHarness, Harness};
use zeron_proto::{
    AccessMode, HarnessId, ReasoningLevel, RunRequest, SandboxLevel, SessionStatus,
};

const CHAT: &str = "chat-agy-live";
const DEVICE: &str = "dev-agy-live";

/// Restores an environment variable on drop (including panics) so live tests
/// run serially with `--test-threads=1` never leak temp dirs or touch `~/.gemini`.
struct EnvGuard {
    key: &'static str,
    previous: Option<std::ffi::OsString>,
}

impl EnvGuard {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let previous = std::env::var_os(key);
        // SAFETY: ignored live tests are run serially with `--test-threads=1`.
        unsafe {
            std::env::set_var(key, value);
        }
        Self { key, previous }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // SAFETY: ignored live tests are run serially with `--test-threads=1`.
        unsafe {
            match &self.previous {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

fn antigravity_binary() -> PathBuf {
    if let Ok(path) = std::env::var("AGY_ACP_BIN") {
        return PathBuf::from(path);
    }
    let home = std::env::var("HOME").expect("HOME set");
    let unified = PathBuf::from(&home).join(".zeron/adapters/antigravity-acp/1.5.15/antigravity");
    if unified.exists() {
        return unified;
    }
    PathBuf::from(home).join(".zeron/adapters/antigravity-acp/1.5.0/agy_acp_server.par")
}

fn prepare_isolated_gemini_home(tmp: &tempfile::TempDir) -> PathBuf {
    let home = std::env::var("HOME").expect("HOME set");
    let src_dir = PathBuf::from(home).join(".gemini/antigravity-acp");
    let dst_home = tmp.path().join("gemini-home");
    let dst_dir = dst_home.join("antigravity-acp");
    std::fs::create_dir_all(&dst_dir).expect("create isolated antigravity-acp dir");
    for file in ["antigravity-oauth-token", "acp_token.json", "settings.json"] {
        let src = src_dir.join(file);
        if src.exists() {
            std::fs::copy(&src, dst_dir.join(file))
                .unwrap_or_else(|e| panic!("copy {file} into scratch GEMINI_HOME: {e}"));
        }
    }
    assert!(
        dst_dir.join("antigravity-oauth-token").exists() || dst_dir.join("acp_token.json").exists(),
        "expected ~/.gemini/antigravity-acp/antigravity-oauth-token or acp_token.json for live test"
    );
    dst_home
}

fn assemble(data_dir: &Path, bin: &Path) -> EngineCore {
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(AcpHarness::antigravity().with_executable(bin)));
    EngineCore::assemble(data_dir, Arc::new(registry), HarnessId::Antigravity, None)
        .expect("engine core assembles")
}

fn pre_title(core: &EngineCore, cwd: &Path) {
    core.workspace
        .create_space(
            "space-agy-live",
            &core.device_id,
            &cwd.display().to_string(),
            None,
            false,
        )
        .expect("create space");
    core.workspace
        .create_chat(CHAT, Some("space-agy-live"), None, None, None)
        .expect("create chat");
    core.workspace
        .rename_chat(CHAT, "Antigravity Live E2E")
        .expect("rename chat");
}

fn make_request(
    cwd: &Path,
    gemini_home: &Path,
    model: &str,
    reasoning: ReasoningLevel,
    prompt: &str,
) -> RunRequest {
    let mut env = std::collections::BTreeMap::new();
    env.insert("GEMINI_HOME".into(), gemini_home.display().to_string());
    RunRequest {
        mcp: None,
        prompt: prompt.into(),
        harness: Some(HarnessId::Antigravity),
        model: Some(model.into()),
        reasoning: Some(reasoning),
        model_options: Default::default(),
        cwd: cwd.display().to_string(),
        sandbox: SandboxLevel::DangerFullAccess,
        auto_approve: true,
        access: AccessMode::FullAccess,
        plan: false,
        code_mode: false,
        zeron_mcp_only: false,
        attachments: Vec::new(),
        worktree: None,
        resume: None,
        env,
        extra_mcp: Vec::new(),
    }
}

fn entries_now(core: &EngineCore) -> Vec<SessionMessageEntry> {
    core.doc_host
        .open(CHAT)
        .ok()
        .and_then(|h| h.doc().read_entries().ok())
        .unwrap_or_default()
}

fn entry_text(entry: &SessionMessageEntry) -> String {
    entry
        .parts
        .iter()
        .filter_map(|p| match p {
            MessagePart::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn entry_errors(entry: &SessionMessageEntry) -> Vec<String> {
    entry
        .parts
        .iter()
        .filter_map(|p| match p {
            MessagePart::Error { message, .. } => Some(message.clone()),
            _ => None,
        })
        .collect()
}

fn complete_assistant_count(core: &EngineCore) -> usize {
    entries_now(core)
        .iter()
        .filter(|e| e.role == MessageRole::Assistant && e.status == Some(MessageStatus::Complete))
        .count()
}

async fn wait_for<F: FnMut() -> bool>(mut predicate: F, timeout_secs: u64, what: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);
    while !predicate() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out after {timeout_secs}s waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
#[ignore = "live end-to-end test against real agy_acp_server 1.5.0 with local OAuth token"]
async fn antigravity_1_5_live_e2e_discovery_steer_stop_and_corrupt_recovery() {
    let bin = antigravity_binary();
    assert!(
        bin.exists(),
        "real agy_acp_server 1.5.0 not found at {}",
        bin.display()
    );

    let tmp = tempfile::tempdir().unwrap();
    let gemini_home = prepare_isolated_gemini_home(&tmp);
    let cwd = tmp.path().join("workspace");
    std::fs::create_dir_all(&cwd).unwrap();
    let data_dir = tmp.path().join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
    std::fs::write(data_dir.join("device-id"), DEVICE).unwrap();

    let _gemini_guard = EnvGuard::set("GEMINI_HOME", &gemini_home);

    // 1. Live model discovery: verify Neon gets XHigh while Flash gets Low/Medium/High.
    eprintln!("[1/6] Testing live model discovery...");
    let harness = AcpHarness::antigravity().with_executable(&bin);
    let models = harness
        .model_catalog(true)
        .await
        .expect("live model discovery")
        .models;
    eprintln!(
        "Discovered {} models: {:?}",
        models.len(),
        models
            .iter()
            .map(|m| (&m.id, &m.reasoning_levels))
            .collect::<Vec<_>>()
    );
    let neon = models
        .iter()
        .find(|m| m.id == "gemini-4-neon")
        .expect("gemini-4-neon discovered");
    assert_eq!(
        neon.reasoning_levels,
        vec![
            ReasoningLevel::Low,
            ReasoningLevel::Medium,
            ReasoningLevel::High,
            ReasoningLevel::XHigh,
        ]
    );
    let flash = models
        .iter()
        .find(|m| m.id == "gemini-3.8-flash")
        .expect("gemini-3.8-flash discovered");
    assert_eq!(
        flash.reasoning_levels,
        vec![
            ReasoningLevel::Low,
            ReasoningLevel::Medium,
            ReasoningLevel::High,
        ]
    );

    // 2. Turn 1 on gemini-4-neon + XHigh via EngineCore: remember secret code word.
    eprintln!("[2/6] Testing Turn 1 on gemini-4-neon + XHigh...");
    let core = assemble(&data_dir, &bin);
    pre_title(&core, &cwd);
    core.doc_host
        .queue_command(
            CHAT,
            SessionCommandPayload::Run {
                request: make_request(
                    &cwd,
                    &gemini_home,
                    "gemini-4-neon",
                    ReasoningLevel::XHigh,
                    "Remember the exact secret code word OBSIDIAN-742. Do not call any tools. Reply with only the single word STORED.",
                ),
                message_id: "msg-1".into(),
                injected: None,
            },
        )
        .unwrap();
    wait_for(|| complete_assistant_count(&core) >= 1, 90, "turn 1 completion").await;
    let t1_entries = entries_now(&core);
    let t1_reply = t1_entries
        .iter()
        .rev()
        .find(|e| e.role == MessageRole::Assistant)
        .map(entry_text)
        .unwrap_or_default();
    eprintln!("Turn 1 reply: {t1_reply:?}");
    assert!(
        t1_reply.to_ascii_uppercase().contains("STORED"),
        "unexpected Turn 1 reply: {t1_reply:?}"
    );
    wait_for(
        || {
            core.workspace
                .chat(CHAT)
                .ok()
                .flatten()
                .and_then(|c| c.harness_session_id)
                .is_some()
        },
        15,
        "harness_session_id saved after turn 1",
    )
    .await;
    let session_id = core
        .workspace
        .chat(CHAT)
        .unwrap()
        .unwrap()
        .harness_session_id
        .expect("harness session id saved after turn 1");
    eprintln!("Saved Antigravity session id: {session_id}");

    // 3. Mid-turn Steer (using fast flash for quick turnaround): verify steer
    //    lands cleanly and zero synthetic cancel chunk leaks into transcript.
    eprintln!("[3/6] Testing mid-turn Steer...");
    let before_steer = complete_assistant_count(&core);
    core.doc_host
        .queue_command(
            CHAT,
            SessionCommandPayload::Run {
                request: make_request(
                    &cwd,
                    &gemini_home,
                    "gemini-3.8-flash",
                    ReasoningLevel::Low,
                    "Write a detailed 500-word essay about the history of the printing press. Do not use any tools.",
                ),
                message_id: "msg-2".into(),
                injected: None,
            },
        )
        .unwrap();
    // Wait until streaming output has actually started before steering.
    wait_for(
        || {
            entries_now(&core)
                .iter()
                .rev()
                .find(|e| e.role == MessageRole::Assistant)
                .is_some_and(|e| e.status != Some(MessageStatus::Complete) && !entry_text(e).is_empty())
        },
        60,
        "turn 2 streaming text before steer",
    )
    .await;
    core.sessions
        .steer(
            CHAT,
            "Stop writing the essay immediately. Do not call tools. Reply with only the exact token STEER-OK.".into(),
            Some("msg-2-steer".into()),
        )
        .await
        .expect("mid-turn steer accepted");
    wait_for(
        || {
            core.sessions
                .session_status(CHAT)
                .is_some_and(|s| s.status == SessionStatus::Idle)
                && complete_assistant_count(&core) > before_steer
        },
        90,
        "steered turn completion",
    )
    .await;
    let after_steer_entries = entries_now(&core);
    let all_text_so_far: String = after_steer_entries.iter().map(entry_text).collect();
    assert!(
        !all_text_so_far.contains("The request was cancelled by the client."),
        "synthetic cancel chunk leaked into transcript: {all_text_so_far:?}"
    );
    let last_reply = after_steer_entries
        .iter()
        .rev()
        .find(|e| e.role == MessageRole::Assistant)
        .map(entry_text)
        .unwrap_or_default();
    eprintln!("Steered reply: {last_reply:?}");
    assert!(
        last_reply.contains("STEER-OK"),
        "steer reply did not contain STEER-OK: {last_reply:?}"
    );

    // 4. Mid-turn Stop (interrupt) + clean follow-up.
    eprintln!("[4/6] Testing mid-turn Stop + follow-up...");
    core.doc_host
        .queue_command(
            CHAT,
            SessionCommandPayload::Run {
                request: make_request(
                    &cwd,
                    &gemini_home,
                    "gemini-3.8-flash",
                    ReasoningLevel::Low,
                    "Write a 500-word essay about ocean currents. Do not use any tools.",
                ),
                message_id: "msg-3".into(),
                injected: None,
            },
        )
        .unwrap();
    wait_for(
        || {
            entries_now(&core)
                .iter()
                .rev()
                .find(|e| e.role == MessageRole::Assistant)
                .is_some_and(|e| e.status != Some(MessageStatus::Complete) && !entry_text(e).is_empty())
        },
        60,
        "turn 3 streaming text before Stop",
    )
    .await;
    core.sessions.interrupt(CHAT).await.expect("interrupt succeeds");
    wait_for(
        || {
            core.sessions
                .session_status(CHAT)
                .is_some_and(|s| s.status == SessionStatus::Idle)
        },
        30,
        "session idle after Stop",
    )
    .await;
    let after_stop_text: String = entries_now(&core).iter().map(entry_text).collect();
    assert!(
        !after_stop_text.contains("The request was cancelled by the client."),
        "synthetic cancel chunk leaked on Stop: {after_stop_text:?}"
    );
    eprintln!("Stop settled cleanly without synthetic cancel noise.");

    // Shut down core cleanly so SQLite files are flushed & closed before we corrupt the DB.
    core.shutdown().await;
    drop(core);

    // 5. Corrupt the isolated session's SQLite DB and verify automatic recovery
    //    with prior conversation history!
    eprintln!("[5/6] Corrupting isolated trajectory DB and testing automatic recovery...");
    let conv_dir = gemini_home.join("antigravity-acp/conversations");
    let db_path = conv_dir.join(format!("{session_id}.db"));
    assert!(
        db_path.exists(),
        "expected isolated trajectory DB at {}",
        db_path.display()
    );
    for suffix in ["", "-wal", "-shm"] {
        let p = conv_dir.join(format!("{session_id}.db{suffix}"));
        if suffix.is_empty() {
            std::fs::write(&p, b"NOT A SQLITE DATABASE - CORRUPTED BY LIVE TEST").unwrap();
        } else if p.exists() {
            let _ = std::fs::remove_file(&p);
        }
    }

    let core = assemble(&data_dir, &bin);
    let before_recovery = complete_assistant_count(&core);
    core.doc_host
        .queue_command(
            CHAT,
            SessionCommandPayload::Run {
                request: make_request(
                    &cwd,
                    &gemini_home,
                    "gemini-3.8-flash",
                    ReasoningLevel::Low,
                    "What was the exact secret code word I asked you to remember in our very first message? Do not call tools. Reply with only that code word.",
                ),
                message_id: "msg-4".into(),
                injected: None,
            },
        )
        .unwrap();
    wait_for(
        || complete_assistant_count(&core) > before_recovery,
        90,
        "recovered turn completion",
    )
    .await;
    let recovered_entries = entries_now(&core);
    let recovered_assistant = recovered_entries
        .iter()
        .rev()
        .find(|e| e.role == MessageRole::Assistant)
        .expect("recovered assistant entry");
    let recovered_text = entry_text(recovered_assistant);
    let recovered_notes = entry_errors(recovered_assistant);
    eprintln!("Recovered notices: {recovered_notes:?}");
    eprintln!("Recovered reply: {recovered_text:?}");
    assert!(
        recovered_notes
            .iter()
            .any(|n| n.contains("prior conversation history")),
        "expected visible history-recovery notice: {recovered_notes:?}"
    );
    assert!(
        recovered_text.contains("OBSIDIAN-742"),
        "Antigravity failed to recall OBSIDIAN-742 after corrupt DB recovery: {recovered_text:?}"
    );
    wait_for(
        || {
            core.workspace
                .chat(CHAT)
                .ok()
                .flatten()
                .and_then(|c| c.harness_session_id)
                .is_some_and(|id| id != session_id)
        },
        15,
        "recovered harness_session_id saved",
    )
    .await;
    let new_session_id = core
        .workspace
        .chat(CHAT)
        .unwrap()
        .unwrap()
        .harness_session_id
        .expect("new session id saved");
    assert_ne!(
        new_session_id, session_id,
        "recovered turn must replace the corrupted session id"
    );

    // 6. Teardown & verify zero orphan processes for our isolated GEMINI_HOME.
    eprintln!("[6/6] Shutting down and checking for orphan processes...");
    core.shutdown().await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let marker = gemini_home.display().to_string();
    let ps_out = std::process::Command::new("ps")
        .args(["eww", "-o", "pid=,comm=,args="])
        .output()
        .expect("ps runs");
    let ps_str = String::from_utf8_lossy(&ps_out.stdout);
    let my_pid = std::process::id().to_string();
    let orphans: Vec<&str> = ps_str
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            let mut parts = trimmed.split_whitespace();
            let Some(pid) = parts.next() else {
                return false;
            };
            let Some(comm) = parts.next() else {
                return false;
            };
            pid != my_pid
                && comm != "ps"
                && line.contains(&marker)
                && (comm.contains("agy_acp")
                    || comm.contains("antigravity")
                    || comm.contains("localharness")
                    || comm.starts_with("python"))
        })
        .collect();
    assert!(
        orphans.is_empty(),
        "orphan Antigravity processes survived shutdown: {orphans:#?}"
    );
    eprintln!("ALL 6 LIVE CHECKS PASSED!");
}

#[tokio::test]
#[ignore = "hits live Antigravity CCPA backend using isolated GEMINI_HOME"]
async fn antigravity_live_neon_streams_reasoning_summaries() {
    let bin = antigravity_binary();
    assert!(bin.is_file(), "missing Antigravity binary at {}", bin.display());

    let tmp = tempfile::tempdir().expect("tempdir");
    let data_dir = tmp.path().join("zeron-data");
    let work_dir = tmp.path().join("work");
    std::fs::create_dir_all(&work_dir).expect("mkdir work");
    let gemini_home = prepare_isolated_gemini_home(&tmp);
    let _gemini_guard = EnvGuard::set("GEMINI_HOME", &gemini_home);

    let core = assemble(&data_dir, &bin);
    pre_title(&core, &work_dir);

    let req = make_request(
        &work_dir,
        &gemini_home,
        "gemini-4-neon",
        ReasoningLevel::High,
        "Compute 17 * 23 step by step, then answer with just the number.",
    );
    core.doc_host
        .queue_command(
            CHAT,
            SessionCommandPayload::Run {
                request: req,
                message_id: "neon-think-1".into(),
                injected: None,
            },
        )
        .expect("queue_command succeeds");
    wait_for(
        || complete_assistant_count(&core) >= 1,
        120,
        "Neon reasoning turn completes",
    )
    .await;

    let entries = entries_now(&core);
    let assistant = entries
        .iter()
        .rev()
        .find(|e| e.role == MessageRole::Assistant)
        .expect("assistant entry present");
    let reasoning: String = assistant
        .parts
        .iter()
        .filter_map(|p| match p {
            MessagePart::Reasoning { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    let text = entry_text(assistant);
    eprintln!("LIVE NEON REASONING ({} chars): {:?}", reasoning.len(), reasoning);
    eprintln!("LIVE NEON ANSWER: {:?}", text);

    core.shutdown().await;

    assert!(
        !reasoning.trim().is_empty(),
        "expected non-empty Reasoning parts from gemini-4-neon via CCPA thoughts proxy; parts={:?}",
        assistant.parts
    );
    assert!(
        text.contains("391"),
        "expected answer containing 391, got: {text:?}"
    );
}

#[tokio::test]
#[ignore = "live verification of unified Antigravity CLI manifest install, paste-login, and token refresh"]
async fn antigravity_live_unified_cli_install_paste_login_and_refresh() {
    // 1. Live release manifest fetch + verified archive install into isolated ZERON_ADAPTERS_DIR.
    let tmp = tempfile::tempdir().expect("tempdir");
    let adapters_dir = tmp.path().join("adapters");
    std::fs::create_dir_all(&adapters_dir).expect("mkdir adapters");
    let _adapters_guard = EnvGuard::set("ZERON_ADAPTERS_DIR", &adapters_dir);

    eprintln!("[1/4] Fetching live Antigravity manifest & installing verified release...");
    let client = reqwest::Client::new();
    let release = zeron_harness::acp::fetch_antigravity_release(&client)
        .await
        .expect("live Antigravity release manifest fetch");
    eprintln!("Fetched release {}", release.version);
    assert!(
        release.installable(),
        "expected release {} to be installable on this platform",
        release.version
    );
    let installed_bin = zeron_harness::acp::install_antigravity_release(&release)
        .await
        .expect("live Antigravity release install");
    assert!(
        installed_bin.is_file(),
        "installed binary missing at {}",
        installed_bin.display()
    );
    assert!(
        zeron_harness::acp::is_unified_antigravity_cli(&installed_bin),
        "installed binary {} should be recognized as unified CLI",
        installed_bin.display()
    );
    eprintln!(
        "Installed and verified unified CLI at {}",
        installed_bin.display()
    );

    // 2. Live paste-code login URL capture + clean cancel.
    eprintln!("[2/4] Testing live paste-code login start + cancel...");
    let login_home = tmp.path().join("paste-login-home");
    std::fs::create_dir_all(&login_home).expect("mkdir paste-login-home");
    let harness = AcpHarness::antigravity().with_executable(&installed_bin);
    assert!(harness.uses_unified_antigravity_cli());
    let paste = harness
        .start_antigravity_paste_login(&login_home)
        .await
        .expect("start_antigravity_paste_login");
    eprintln!("Captured paste-login URL: {}", paste.url());
    assert!(
        paste
            .url()
            .starts_with("https://accounts.google.com/o/oauth2/auth?"),
        "unexpected paste-login URL: {}",
        paste.url()
    );
    paste.cancel().await;
    eprintln!("Cancelled paste-login cleanly.");

    // 3. Live isolated token refresh from an expired migrated token.
    eprintln!("[3/4] Testing live isolated OAuth token refresh...");
    let isolated_home = prepare_isolated_gemini_home(&tmp);
    zeron_harness::acp::ensure_antigravity_oauth_token(&isolated_home);
    let migrated_path = zeron_harness::acp::antigravity_oauth_token_file(&isolated_home);
    let mut doc: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&migrated_path).expect("read migrated token"),
    )
    .expect("parse migrated token");
    doc["token"]["access_token"] = serde_json::json!("ya29.expired-for-test");
    doc["token"]["expiry"] = serde_json::json!("2020-01-01T00:00:00Z");

    let refreshed = harness
        .refresh_antigravity_oauth_token(&doc)
        .await
        .expect("live isolated token refresh");
    let new_access = refreshed["token"]["access_token"]
        .as_str()
        .expect("refreshed access_token present");
    assert_ne!(new_access, "ya29.expired-for-test");
    assert!(
        new_access.starts_with("ya29."),
        "expected Google OAuth access token, got prefix {:?}",
        &new_access[..new_access.len().min(8)]
    );
    eprintln!("Live isolated OAuth refresh succeeded cleanly!");

    // 4. Live AgentAccounts CCPA usage probe on a unified-only slot whose
    //    access_token is expired (proves real auto-refresh via ZERON_ADAPTERS_DIR
    //    + live loadCodeAssist + retrieveUserQuotaSummary).
    eprintln!("[4/4] Testing live AgentAccounts usage probe with unified CLI auto-refresh...");
    let accounts_root = tmp.path().join("accounts-root");
    let config = zeron_engine::AgentAccountsConfig::isolated(&accounts_root);
    let agy_home = config.antigravity_home.clone().expect("antigravity_home");
    let oauth_dest = zeron_harness::acp::antigravity_oauth_token_file(&agy_home);
    std::fs::create_dir_all(oauth_dest.parent().unwrap()).expect("mkdir agy_home");
    std::fs::write(&oauth_dest, serde_json::to_string_pretty(&doc).unwrap())
        .expect("write expired unified token");

    let accounts = zeron_engine::AgentAccounts::new(config);
    let snap = accounts.list(true).await.expect("live accounts.list(true)");
    let agy_rows: Vec<_> = snap
        .accounts
        .iter()
        .filter(|a| a.harness == HarnessId::Antigravity)
        .collect();
    eprintln!("Live Antigravity account rows: {agy_rows:#?}");
    assert_eq!(agy_rows.len(), 1, "expected 1 Antigravity account row");
    assert!(agy_rows[0].active);
    assert!(
        agy_rows[0].usage_error.is_none(),
        "unexpected live usage_error: {:?}",
        agy_rows[0].usage_error
    );
    assert!(
        !agy_rows[0].usage_windows.is_empty() || agy_rows[0].plan_label.is_some(),
        "expected live quota windows or plan label: {:?}",
        agy_rows[0]
    );
    eprintln!(
        "Live usage probe succeeded! email={:?} plan={:?} windows={}",
        agy_rows[0].email,
        agy_rows[0].plan_label,
        agy_rows[0].usage_windows.len()
    );
}

