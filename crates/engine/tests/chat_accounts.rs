//! Accounts per chat: a chat may name one of the saved Claude Code or Codex
//! accounts (`ChatConfig::account`) and run on it while other chats on the
//! same machine run on theirs, or on the machine's active login.
//!
//! Every path is under a tempdir (`AgentAccountsConfig::isolated`): never the
//! real `~/.claude`, `~/.codex` or a Keychain item.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as BASE64_URL;
use futures::{StreamExt, stream::BoxStream};
use tokio::sync::Notify;
use zeron_engine::{
    AccountsRunEnvironment, AgentAccounts, AgentAccountsConfig, EngineCore, HarnessRegistry,
};
use zeron_harness::{Harness, HarnessError, RunControls};
use zeron_proto::{
    AgentEvent, ChatConfig, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SandboxLevel,
    SessionStatus, SteeringMode,
};

const CLAUDE: HarnessId = HarnessId::ClaudeCode;
const CODEX: HarnessId = HarnessId::Codex;

// ── fixtures ───────────────────────────────────────────────────────────────

fn write_claude_login(config: &AgentAccountsConfig, email: &str, uuid: &str, token: &str) {
    std::fs::create_dir_all(&config.claude_config_dir).unwrap();
    let mut cfg = std::fs::read_to_string(&config.claude_config_file)
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    cfg["oauthAccount"] = serde_json::json!({
        "accountUuid": uuid,
        "emailAddress": email,
        "organizationType": "claude_max",
    });
    cfg["userID"] = serde_json::json!(format!("user-{uuid}"));
    std::fs::write(&config.claude_config_file, cfg.to_string()).unwrap();
    std::fs::write(
        config.claude_config_dir.join(".credentials.json"),
        claude_creds(token).to_string(),
    )
    .unwrap();
}

fn claude_creds(token: &str) -> serde_json::Value {
    serde_json::json!({
        "claudeAiOauth": {
            "accessToken": token,
            "refreshToken": format!("refresh-{token}"),
            "expiresAt": 4_102_444_800_000i64,
        },
        "mcpOAuth": { "github": { "accessToken": "machine-mcp" } },
    })
}

fn fake_id_token(email: &str, account_id: &str) -> String {
    let header = BASE64_URL.encode(br#"{"alg":"none"}"#);
    let payload = BASE64_URL.encode(
        serde_json::json!({
            "email": email,
            "https://api.openai.com/auth": {
                "chatgpt_account_id": account_id,
                "chatgpt_plan_type": "plus",
            },
        })
        .to_string(),
    );
    format!("{header}.{payload}.x")
}

fn write_codex_login(config: &AgentAccountsConfig, email: &str, account_id: &str) {
    std::fs::create_dir_all(&config.codex_home).unwrap();
    std::fs::write(
        config.codex_home.join("auth.json"),
        serde_json::json!({
            "tokens": {
                "id_token": fake_id_token(email, account_id),
                "access_token": format!("at-{account_id}"),
                "refresh_token": format!("rt-{account_id}"),
                "account_id": account_id,
            }
        })
        .to_string(),
    )
    .unwrap();
}

fn read_json(path: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// Saved accounts: Alice and Bob (Claude), Dana and Eve (Codex); the machine
/// is left signed in as Carol (Claude) and Finn (Codex).
struct Accounts {
    accounts: AgentAccounts,
    config: AgentAccountsConfig,
    ids: HashMap<&'static str, String>,
}

async fn accounts(root: &Path) -> Accounts {
    let config = AgentAccountsConfig::isolated(root);
    let accounts = AgentAccounts::new(config.clone());
    let mut ids = HashMap::new();
    let id_of = |snapshot: &zeron_proto::AgentAccountsSnapshot, email: &str| {
        snapshot
            .accounts
            .iter()
            .find(|a| a.email.as_deref() == Some(email))
            .unwrap()
            .id
            .clone()
    };
    for (name, uuid) in [
        ("alice", "uuid-alice"),
        ("bob", "uuid-bob"),
        ("carol", "uuid-carol"),
    ] {
        let email = format!("{name}@example.com");
        write_claude_login(&config, &email, uuid, &format!("token-{name}"));
        let snapshot = accounts.list(false).await.unwrap();
        ids.insert(name, id_of(&snapshot, &email));
    }
    for (name, account) in [
        ("dana", "acct-dana"),
        ("eve", "acct-eve"),
        ("finn", "acct-finn"),
    ] {
        let email = format!("{name}@example.com");
        write_codex_login(&config, &email, account);
        let snapshot = accounts.list(false).await.unwrap();
        ids.insert(name, id_of(&snapshot, &email));
    }
    let ids = ids
        .into_iter()
        .map(|(k, v)| (k, v))
        .collect::<HashMap<&'static str, String>>();
    Accounts {
        accounts,
        config,
        ids,
    }
}

impl Accounts {
    fn id(&self, name: &str) -> String {
        self.ids[name].clone()
    }
    fn home(&self, harness: HarnessId, name: &str) -> PathBuf {
        self.accounts.account_home(harness, &self.id(name)).unwrap()
    }
    fn live_claude_token(&self) -> serde_json::Value {
        read_json(&self.config.claude_config_dir.join(".credentials.json"))["claudeAiOauth"]
            ["accessToken"]
            .clone()
    }
}

// ── the engine rig (a fake provider that records what it was run with) ─────

type Seen = Arc<Mutex<Vec<(HarnessId, RunRequest)>>>;

struct Fake {
    id: HarnessId,
    seen: Seen,
    gate: Arc<Mutex<Option<Arc<Notify>>>>,
}

#[async_trait]
impl Harness for Fake {
    fn id(&self) -> HarnessId {
        self.id
    }
    fn display_name(&self) -> &str {
        "Fake"
    }
    fn supports_steering(&self) -> bool {
        false
    }
    fn steering_mode(&self) -> SteeringMode {
        SteeringMode::TurnBoundary
    }
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &[]
    }
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        Ok(vec![])
    }
    async fn run(
        &self,
        request: RunRequest,
        _: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        self.seen.lock().unwrap().push((self.id, request));
        let gate = self.gate.lock().unwrap().clone();
        let id = self.id;
        let session = format!("{}-session", id.as_str());
        Ok(futures::stream::once(async move {
            if let Some(gate) = gate {
                gate.notified().await;
            }
            vec![
                Ok(AgentEvent::SessionStarted {
                    harness: id,
                    model: "m".into(),
                    tools: vec![],
                    cwd: "/tmp".into(),
                    session_id: session.clone(),
                    assistant_message_id: format!("{session}-a"),
                }),
                Ok(AgentEvent::TextDelta {
                    text: "answer".into(),
                }),
                Ok(AgentEvent::Done {
                    status: DoneStatus::Completed,
                    result: None,
                    error: None,
                    session_id: Some(session),
                }),
            ]
        })
        .flat_map(futures::stream::iter)
        .boxed())
    }
}

struct Rig {
    core: EngineCore,
    seen: Seen,
    gate: Arc<Mutex<Option<Arc<Notify>>>>,
    accounts: Accounts,
    _dir: tempfile::TempDir,
}

async fn rig() -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let seen: Seen = Arc::default();
    let gate: Arc<Mutex<Option<Arc<Notify>>>> = Arc::default();
    let registry = Arc::new(HarnessRegistry::new());
    for id in [CLAUDE, CODEX] {
        registry.register(Arc::new(Fake {
            id,
            seen: seen.clone(),
            gate: gate.clone(),
        }));
    }
    let engine_dir = dir.path().join("engine");
    std::fs::create_dir_all(&engine_dir).unwrap();
    let core = EngineCore::assemble(&engine_dir, registry.clone(), CLAUDE, None).unwrap();
    // The isolated accounts stand in for the engine's own (which would look
    // at this machine's real logins).
    let accounts = accounts(&dir.path().join("accounts")).await;
    registry.set_account_environment(Arc::new(AccountsRunEnvironment(accounts.accounts.clone())));
    Rig {
        core,
        seen,
        gate,
        accounts,
        _dir: dir,
    }
}

fn config(harness: HarnessId, account: Option<String>) -> ChatConfig {
    ChatConfig {
        harness,
        model: None,
        reasoning: None,
        model_options: Default::default(),
        sandbox: SandboxLevel::WorkspaceWrite,
        access: Default::default(),
        plan: false,
        account,
    }
}

fn run_request(harness: HarnessId, prompt: &str) -> RunRequest {
    RunRequest {
        mcp: None,
        prompt: prompt.into(),
        harness: Some(harness),
        model: None,
        reasoning: None,
        model_options: Default::default(),
        cwd: "/tmp".into(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: true,
        access: Default::default(),
        plan: false,
        resume: None,
        attachments: vec![],
        worktree: None,
        env: Default::default(),
        extra_mcp: Vec::new(),
    }
}

impl Rig {
    fn chat(&self, id: &str, harness: HarnessId, account: Option<&str>) {
        self.core
            .workspace
            .create_chat(
                id,
                None,
                Some(&self.core.device_id),
                Some(config(harness, account.map(|a| self.accounts.id(a)))),
                Some("/tmp".into()),
            )
            .unwrap();
    }

    fn pin(&self, chat: &str, harness: HarnessId, account: Option<&str>) {
        assert!(
            self.core
                .workspace
                .set_chat_config(chat, &config(harness, account.map(|a| self.accounts.id(a))))
                .unwrap()
        );
    }

    async fn send(&self, chat: &str, harness: HarnessId, prompt: &str, message: &str) {
        self.core
            .sessions
            .dispatch(
                chat,
                harness,
                run_request(harness, prompt),
                Some(message.into()),
            )
            .await
            .unwrap();
    }

    async fn wait_runs(&self, count: usize) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while self.seen.lock().unwrap().len() < count {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("runs start");
    }

    async fn wait_idle(&self, chat: &str) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while self.core.sessions.session_status(chat).is_some_and(|s| {
                matches!(
                    s.status,
                    SessionStatus::Working | SessionStatus::AwaitingInput
                )
            }) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("turn settles");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    /// The request chat `chat` ran with (its latest).
    fn ran(&self, chat: &str) -> RunRequest {
        let origin = zeron_proto::chat_origin(chat);
        self.seen
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|(_, r)| r.env.get(zeron_proto::ORIGIN_ENV) == Some(&origin))
            .map(|(_, r)| r.clone())
            .unwrap_or_else(|| panic!("{chat} never ran"))
    }
}

// ── tests ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn two_chats_run_with_different_accounts_at_once() {
    let rig = rig().await;
    rig.chat("a", CLAUDE, Some("alice"));
    rig.chat("b", CLAUDE, Some("bob"));
    rig.chat("x", CODEX, Some("dana"));
    rig.chat("y", CODEX, Some("eve"));
    let gate = Arc::new(Notify::new());
    *rig.gate.lock().unwrap() = Some(gate.clone());

    rig.send("a", CLAUDE, "hi", "a1").await;
    rig.send("b", CLAUDE, "hi", "b1").await;
    rig.send("x", CODEX, "hi", "x1").await;
    rig.send("y", CODEX, "hi", "y1").await;
    // All four are running at the same time, each on its own account.
    rig.wait_runs(4).await;
    for chat in ["a", "b", "x", "y"] {
        assert_eq!(
            rig.core.sessions.session_status(chat).map(|s| s.status),
            Some(SessionStatus::Working),
            "{chat} is running"
        );
    }

    let dir = |chat: &str, var: &str| PathBuf::from(rig.ran(chat).env.get(var).cloned().unwrap());
    let a = dir("a", "CLAUDE_CONFIG_DIR");
    let b = dir("b", "CLAUDE_CONFIG_DIR");
    assert_eq!(a, rig.accounts.home(CLAUDE, "alice"));
    assert_eq!(b, rig.accounts.home(CLAUDE, "bob"));
    assert_ne!(a, b);
    let token = |dir: &Path| {
        read_json(&dir.join(".credentials.json"))["claudeAiOauth"]["accessToken"].clone()
    };
    assert_eq!(token(&a), "token-alice");
    assert_eq!(token(&b), "token-bob");
    // Each account dir names its own account to Claude Code.
    let identity =
        |dir: &Path| read_json(&dir.join(".claude.json"))["oauthAccount"]["emailAddress"].clone();
    assert_eq!(identity(&a), "alice@example.com");
    assert_eq!(identity(&b), "bob@example.com");
    assert!(rig.ran("a").env.get("CODEX_HOME").is_none());

    let x = dir("x", "CODEX_HOME");
    let y = dir("y", "CODEX_HOME");
    assert_eq!(x, rig.accounts.home(CODEX, "dana"));
    assert_eq!(y, rig.accounts.home(CODEX, "eve"));
    let account = |dir: &Path| read_json(&dir.join("auth.json"))["tokens"]["account_id"].clone();
    assert_eq!(account(&x), "acct-dana");
    assert_eq!(account(&y), "acct-eve");
    assert!(rig.ran("x").env.get("CLAUDE_CONFIG_DIR").is_none());

    // The machine's own logins never moved.
    assert_eq!(rig.accounts.live_claude_token(), "token-carol");
    assert_eq!(
        read_json(&rig.accounts.config.codex_home.join("auth.json"))["tokens"]["account_id"],
        "acct-finn"
    );
    gate.notify_waiters();
}

#[tokio::test]
async fn an_unpinned_chat_runs_as_before() {
    let rig = rig().await;
    rig.chat("plain", CLAUDE, None);
    rig.chat("plain-codex", CODEX, None);
    rig.send("plain", CLAUDE, "hi", "p1").await;
    rig.wait_idle("plain").await;
    rig.send("plain-codex", CODEX, "hi", "p2").await;
    rig.wait_idle("plain-codex").await;
    for chat in ["plain", "plain-codex"] {
        let env = rig.ran(chat).env;
        assert!(env.get("CLAUDE_CONFIG_DIR").is_none(), "{chat}: {env:?}");
        assert!(env.get("CODEX_HOME").is_none(), "{chat}: {env:?}");
    }
    // Nothing was made for it.
    assert!(
        !rig.accounts.config.data_dir.join("agent-homes").exists(),
        "no account dir for a chat that names none"
    );
    assert_eq!(rig.accounts.live_claude_token(), "token-carol");
}

#[tokio::test]
async fn a_pinned_account_has_one_live_copy_of_its_login() {
    let tmp = tempfile::tempdir().unwrap();
    let a = accounts(tmp.path()).await;
    let env = AccountsRunEnvironment(a.accounts.clone());
    let alice = a.id("alice");
    let home = a.home(CLAUDE, "alice");
    let prepare = |account: Option<String>| {
        let env = &env;
        async move {
            let mut request = run_request(CLAUDE, "hi");
            zeron_engine::registry::RunEnvironment::prepare_for(
                env,
                CLAUDE,
                account.as_deref(),
                &mut request,
            )
            .await
            .unwrap();
            request.env
        }
    };

    // Alice isn't the machine's login: her dir holds it.
    let run = prepare(Some(alice.clone())).await;
    assert_eq!(
        run.get("CLAUDE_CONFIG_DIR"),
        Some(&home.display().to_string())
    );
    let seeded = read_json(&home.join(".credentials.json"));
    assert_eq!(seeded["claudeAiOauth"]["accessToken"], "token-alice");
    // The machine's MCP sign-ins aren't copied (their tokens rotate too).
    assert!(seeded.get("mcpOAuth").is_none());

    // Claude Code refreshes Alice's login inside her dir.
    std::fs::write(
        home.join(".credentials.json"),
        serde_json::json!({ "claudeAiOauth": {
            "accessToken": "token-alice-2",
            "refreshToken": "refresh-token-alice-2",
            "expiresAt": 4_102_444_900_000i64,
        }})
        .to_string(),
    )
    .unwrap();
    // A later run never seeds over it.
    prepare(Some(alice.clone())).await;
    assert_eq!(
        read_json(&home.join(".credentials.json"))["claudeAiOauth"]["accessToken"],
        "token-alice-2"
    );
    // The machine's login is untouched; the saved copy follows the dir.
    a.accounts.list(false).await.unwrap();
    assert_eq!(a.live_claude_token(), "token-carol");
    let slot = read_json(
        &a.config
            .data_dir
            .join("agent-accounts")
            .join("claude-code")
            .join(format!("{alice}.json")),
    );
    assert_eq!(
        slot["credentials"]["claudeAiOauth"]["refreshToken"],
        "refresh-token-alice-2"
    );
    // The engine never refreshes a saved login while its dir holds it.
    assert!(a.accounts.holds_live_login(CLAUDE, &alice).await);

    // Alice made the machine's account: her login MOVES from her dir to the
    // machine (the newest pair), and her dir no longer holds it.
    a.accounts.activate(CLAUDE, &alice).await.unwrap();
    assert_eq!(a.live_claude_token(), "token-alice-2");
    assert!(!home.join(".credentials.json").exists());
    // Her chats now run on the machine's login: no second copy.
    let run = prepare(Some(alice.clone())).await;
    assert!(run.get("CLAUDE_CONFIG_DIR").is_none(), "{run:?}");
    assert!(!home.join(".credentials.json").exists());

    // Bob made the machine's account: Alice's chats get her dir back, seeded
    // with her newest pair (saved from the machine's login before the swap).
    a.accounts.activate(CLAUDE, &a.id("bob")).await.unwrap();
    assert_eq!(a.live_claude_token(), "token-bob");
    let run = prepare(Some(alice.clone())).await;
    assert_eq!(
        run.get("CLAUDE_CONFIG_DIR"),
        Some(&home.display().to_string())
    );
    assert_eq!(
        read_json(&home.join(".credentials.json"))["claudeAiOauth"]["refreshToken"],
        "refresh-token-alice-2"
    );

    // Forgetting Alice removes her dir; a chat still naming her says so.
    a.accounts.forget(CLAUDE, &alice).await.unwrap();
    assert!(!home.exists());
    let mut request = run_request(CLAUDE, "hi");
    let refused = zeron_engine::registry::RunEnvironment::prepare_for(
        &env,
        CLAUDE,
        Some(&alice),
        &mut request,
    )
    .await;
    assert!(refused.is_err());
}

#[tokio::test]
async fn codex_pinned_login_moves_rather_than_copies() {
    let tmp = tempfile::tempdir().unwrap();
    let a = accounts(tmp.path()).await;
    let env = AccountsRunEnvironment(a.accounts.clone());
    let dana = a.id("dana");
    let home = a.home(CODEX, "dana");
    let mut request = run_request(CODEX, "hi");
    zeron_engine::registry::RunEnvironment::prepare_for(&env, CODEX, Some(&dana), &mut request)
        .await
        .unwrap();
    assert_eq!(
        request.env.get("CODEX_HOME"),
        Some(&home.display().to_string())
    );
    // Codex refreshes in her home.
    let mut auth = read_json(&home.join("auth.json"));
    auth["tokens"]["refresh_token"] = serde_json::json!("rt-dana-2");
    std::fs::write(home.join("auth.json"), auth.to_string()).unwrap();
    a.accounts.activate(CODEX, &dana).await.unwrap();
    let live = read_json(&a.config.codex_home.join("auth.json"));
    assert_eq!(live["tokens"]["refresh_token"], "rt-dana-2");
    assert!(!home.join("auth.json").exists());
    let mut request = run_request(CODEX, "hi");
    zeron_engine::registry::RunEnvironment::prepare_for(&env, CODEX, Some(&dana), &mut request)
        .await
        .unwrap();
    assert!(request.env.get("CODEX_HOME").is_none());
}

#[tokio::test]
async fn shared_config_is_visible_in_an_account_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let a = accounts(tmp.path()).await;
    // The user's own Claude Code setup.
    let real = &a.config.claude_config_dir;
    std::fs::write(real.join("settings.json"), r#"{"theme":"dark"}"#).unwrap();
    std::fs::write(real.join("CLAUDE.md"), "be kind").unwrap();
    std::fs::create_dir_all(real.join("skills").join("mine")).unwrap();
    std::fs::create_dir_all(real.join("plugins")).unwrap();
    let mut cfg = read_json(&a.config.claude_config_file);
    cfg["mcpServers"] = serde_json::json!({ "docs": { "command": "docs-mcp" } });
    std::fs::write(&a.config.claude_config_file, cfg.to_string()).unwrap();
    // …and Codex's.
    std::fs::write(a.config.codex_home.join("config.toml"), "model = \"gpt\"").unwrap();
    std::fs::create_dir_all(a.config.codex_home.join("skills")).unwrap();

    let env = AccountsRunEnvironment(a.accounts.clone());
    for (harness, name) in [(CLAUDE, "alice"), (CODEX, "dana")] {
        let mut request = run_request(harness, "hi");
        zeron_engine::registry::RunEnvironment::prepare_for(
            &env,
            harness,
            Some(&a.id(name)),
            &mut request,
        )
        .await
        .unwrap();
    }

    let home = a.home(CLAUDE, "alice");
    assert_eq!(
        std::fs::read_to_string(home.join("settings.json")).unwrap(),
        r#"{"theme":"dark"}"#
    );
    assert_eq!(
        std::fs::read_to_string(home.join("CLAUDE.md")).unwrap(),
        "be kind"
    );
    assert!(home.join("skills").join("mine").is_dir());
    assert!(home.join("plugins").is_dir());
    // Session history is the user's own: written through the account dir,
    // it lands in the real one.
    assert!(home.join("projects").is_dir());
    std::fs::write(home.join("projects").join("s.jsonl"), "{}").unwrap();
    assert!(real.join("projects").join("s.jsonl").exists());
    // The login is the account's own, never a link.
    let creds = std::fs::symlink_metadata(home.join(".credentials.json")).unwrap();
    assert!(!creds.file_type().is_symlink());
    // `.claude.json`: the user's MCP servers, the account's identity.
    let own = read_json(&home.join(".claude.json"));
    assert_eq!(own["mcpServers"]["docs"]["command"], "docs-mcp");
    assert_eq!(own["oauthAccount"]["emailAddress"], "alice@example.com");
    assert_eq!(own["userID"], "user-uuid-alice");
    // The user's own file still names the machine's account.
    assert_eq!(
        read_json(&a.config.claude_config_file)["oauthAccount"]["emailAddress"],
        "carol@example.com"
    );

    let home = a.home(CODEX, "dana");
    assert_eq!(
        std::fs::read_to_string(home.join("config.toml")).unwrap(),
        "model = \"gpt\""
    );
    assert!(home.join("skills").is_dir());
    assert!(home.join("sessions").is_dir());
    std::fs::write(home.join("sessions").join("r.jsonl"), "{}").unwrap();
    assert!(
        a.config
            .codex_home
            .join("sessions")
            .join("r.jsonl")
            .exists()
    );
    let auth = std::fs::symlink_metadata(home.join("auth.json")).unwrap();
    assert!(!auth.file_type().is_symlink());
}

#[tokio::test]
async fn resume_works_after_switching_a_chats_account() {
    let rig = rig().await;
    rig.chat("main", CLAUDE, Some("alice"));
    rig.send("main", CLAUDE, "Remember PINEAPPLE", "u1").await;
    rig.wait_idle("main").await;
    let first = rig.ran("main");
    assert_eq!(first.resume, None);
    let alice_home = rig.accounts.home(CLAUDE, "alice");
    assert_eq!(
        first.env.get("CLAUDE_CONFIG_DIR"),
        Some(&alice_home.display().to_string())
    );
    // The session Claude Code wrote, through Alice's dir.
    std::fs::create_dir_all(alice_home.join("projects").join("-tmp")).unwrap();
    std::fs::write(
        alice_home
            .join("projects")
            .join("-tmp")
            .join("claude-code-session.jsonl"),
        "{}",
    )
    .unwrap();

    // The chat moves to Bob: the next turn runs on Bob, resuming the session.
    rig.pin("main", CLAUDE, Some("bob"));
    rig.send("main", CLAUDE, "What fruit?", "u2").await;
    rig.wait_idle("main").await;
    let second = rig.ran("main");
    let bob_home = rig.accounts.home(CLAUDE, "bob");
    assert_eq!(
        second.env.get("CLAUDE_CONFIG_DIR"),
        Some(&bob_home.display().to_string())
    );
    assert_eq!(second.resume.as_deref(), Some("claude-code-session"));
    assert!(
        bob_home
            .join("projects")
            .join("-tmp")
            .join("claude-code-session.jsonl")
            .exists(),
        "the session is there for Bob's dir too"
    );

    // Back to the machine's account: still the same session.
    rig.pin("main", CLAUDE, None);
    rig.send("main", CLAUDE, "And now?", "u3").await;
    rig.wait_idle("main").await;
    let third = rig.ran("main");
    assert!(third.env.get("CLAUDE_CONFIG_DIR").is_none());
    assert_eq!(third.resume.as_deref(), Some("claude-code-session"));
}
