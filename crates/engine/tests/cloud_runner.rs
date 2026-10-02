//! Cloud runner mode (docs/design/cloud-device.md) against a stub edge + stub
//! GitHub on plain tokio TcpListeners: enrollment, signed runner tokens, the
//! fork/template identity guard, heartbeats, and the credential broker.

#![cfg(unix)]

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use zeron_engine::credentials::{BrokerConfig, CredentialBroker, GITHUB_NOT_CONNECTED};
use zeron_engine::runner::{self, RunnerIdentity};
use zeron_engine::{Auth, AuthConfig, AuthState, Engine};
use zeron_proto::{HarnessId, RunRequest};
use zeron_rpc::{TokenError, TokenSource};

// ---------------------------------------------------------------------------
// Stub HTTP server
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Req {
    method: String,
    /// Path including the query string.
    path: String,
    bearer: Option<String>,
    body: Value,
}

type Handler = Box<dyn FnMut(&Req) -> (u16, Value) + Send>;

struct Stub {
    port: u16,
    requests: Arc<Mutex<Vec<Req>>>,
    handler: Arc<Mutex<Handler>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Stub {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Stub {
    async fn start(handler: impl FnMut(&Req) -> (u16, Value) + Send + 'static) -> Stub {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let handler: Arc<Mutex<Handler>> = Arc::new(Mutex::new(Box::new(handler)));
        let (reqs, h) = (requests.clone(), handler.clone());
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let (reqs, h) = (reqs.clone(), h.clone());
                tokio::spawn(async move {
                    let Some(req) = read_request(&mut stream).await else {
                        return;
                    };
                    reqs.lock().unwrap().push(req.clone());
                    let (status, body) = (h.lock().unwrap())(&req);
                    let body = body.to_string();
                    let response = format!(
                        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.shutdown().await;
                });
            }
        });
        Stub {
            port,
            requests,
            handler,
            task,
        }
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn set(&self, handler: impl FnMut(&Req) -> (u16, Value) + Send + 'static) {
        *self.handler.lock().unwrap() = Box::new(handler);
    }

    fn requests(&self, path_prefix: &str) -> Vec<Req> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.path.starts_with(path_prefix))
            .cloned()
            .collect()
    }
}

async fn read_request(stream: &mut tokio::net::TcpStream) -> Option<Req> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut lines = head.lines();
    let mut first = lines.next()?.split_whitespace();
    let method = first.next()?.to_string();
    let path = first.next()?.to_string();
    let mut length = 0usize;
    let mut bearer = None;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        match name.trim().to_ascii_lowercase().as_str() {
            "content-length" => length = value.trim().parse().unwrap_or(0),
            "authorization" => bearer = value.trim().strip_prefix("Bearer ").map(str::to_string),
            _ => {}
        }
    }
    let mut body = buf[header_end..].to_vec();
    while body.len() < length {
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    Some(Req {
        method,
        path,
        bearer,
        body: serde_json::from_slice(&body).unwrap_or(Value::Null),
    })
}

// ---------------------------------------------------------------------------
// Edge behavior
// ---------------------------------------------------------------------------

const ORG: &str = "org_1";
const USER: &str = "user_1";

fn b64url(bytes: &[u8]) -> String {
    B64URL.encode(bytes)
}

fn jwt(ttl_secs: i64) -> String {
    let now = chrono::Utc::now().timestamp();
    let claims = json!({"sub": USER, "org_id": ORG, "dev": "cloud-a", "kind": "runner",
        "iat": now, "exp": now + ttl_secs});
    format!(
        "{}.{}.sig",
        b64url(br#"{"alg":"ES256"}"#),
        b64url(claims.to_string().as_bytes())
    )
}

fn verify(public_key: &str, message: &str, sig: &str) -> bool {
    let key = ring::signature::UnparsedPublicKey::new(
        &ring::signature::ED25519,
        B64URL.decode(public_key).unwrap(),
    );
    key.verify(message.as_bytes(), &B64URL.decode(sig).unwrap())
        .is_ok()
}

/// A stateful fake edge: enrollment stores the public key; tokens and grants
/// must be signed by it with strictly increasing `ts` (one fence per route).
#[derive(Default)]
struct Edge {
    public_keys: HashMap<String, String>,
    last_token_ts: i64,
    last_grant_ts: i64,
    enroll_status: Vec<u16>,
    token_ttl: i64,
    token_reject: bool,
    /// provider → (status, body). Absent → 403 not_authorized.
    grants: HashMap<String, (u16, Value)>,
    stale_once: bool,
    /// Every `github` grant request's `repo`.
    github_repos: Vec<String>,
    /// Grant requests seen, per provider.
    grant_requests: HashMap<String, usize>,
}

fn edge_handler(state: Arc<Mutex<Edge>>) -> impl FnMut(&Req) -> (u16, Value) + Send + 'static {
    move |req: &Req| {
        let mut edge = state.lock().unwrap();
        let path = req.path.as_str();
        let b = &req.body;
        match (req.method.as_str(), path) {
            ("POST", "/runner/enroll") => {
                let status = if edge.enroll_status.is_empty() {
                    200
                } else {
                    edge.enroll_status.remove(0)
                };
                if status != 200 {
                    return (status, json!({"error": "bad_code", "message": "code used"}));
                }
                edge.public_keys.insert(
                    b["deviceId"].as_str().unwrap().into(),
                    b["publicKey"].as_str().unwrap().into(),
                );
                (200, json!({"ok": true}))
            }
            ("POST", "/runner/token") => {
                if edge.token_reject {
                    return (
                        403,
                        json!({"error": "forbidden", "message": "device deleted"}),
                    );
                }
                let device = b["deviceId"].as_str().unwrap();
                let ts = b["ts"].as_i64().unwrap();
                let message = format!(
                    "zeron-runner-token\n{}\n{}\n{device}\n{ts}",
                    b["orgId"].as_str().unwrap(),
                    b["userId"].as_str().unwrap()
                );
                let Some(key) = edge.public_keys.get(device).cloned() else {
                    return (403, json!({"error": "device_unknown"}));
                };
                if !verify(&key, &message, b["sig"].as_str().unwrap()) {
                    return (401, json!({"error": "bad_signature"}));
                }
                if ts <= edge.last_token_ts {
                    return (401, json!({"error": "stale"}));
                }
                edge.last_token_ts = ts;
                let ttl = if edge.token_ttl == 0 {
                    3600
                } else {
                    edge.token_ttl
                };
                (
                    200,
                    json!({"accessToken": jwt(ttl), "expiresAt": chrono::Utc::now().timestamp_millis() + ttl * 1000}),
                )
            }
            ("POST", "/runner/heartbeat") => {
                if req.bearer.is_none() {
                    return (401, json!({"error": "unauthorized"}));
                }
                (200, json!({"ok": true}))
            }
            ("POST", p) if p == format!("/vault/{ORG}/grant") => {
                if req.bearer.is_none() {
                    return (401, json!({"error": "unauthorized"}));
                }
                let device = b["deviceId"].as_str().unwrap();
                let provider = b["provider"].as_str().unwrap();
                let ts = b["ts"].as_i64().unwrap();
                let message = format!("zeron-vault-grant\n{USER}\n{device}\n{provider}\n{ts}");
                let key = edge.public_keys.get(device).cloned().unwrap_or_default();
                if !verify(&key, &message, b["sig"].as_str().unwrap()) {
                    return (401, json!({"error": "bad_signature"}));
                }
                if edge.stale_once {
                    edge.stale_once = false;
                    return (401, json!({"error": "stale", "message": "replayed"}));
                }
                if ts <= edge.last_grant_ts {
                    return (401, json!({"error": "stale"}));
                }
                edge.last_grant_ts = ts;
                *edge.grant_requests.entry(provider.into()).or_default() += 1;
                // Like the vault: a GitHub grant must name its repository.
                if provider == "github" {
                    let Some(repo) = b["repo"].as_str() else {
                        return (
                            400,
                            json!({"error": "bad_request", "message": "repo required"}),
                        );
                    };
                    edge.github_repos.push(repo.into());
                }
                edge.grants.get(provider).cloned().unwrap_or((
                    403,
                    json!({"error": "not_authorized", "message": "not connected"}),
                ))
            }
            _ => (404, json!({"error": "not_found"})),
        }
    }
}

struct Rig {
    _dir: tempfile::TempDir,
    data_dir: std::path::PathBuf,
    home: std::path::PathBuf,
    edge: Stub,
    state: Arc<Mutex<Edge>>,
}

impl Rig {
    async fn new() -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().join("data");
        let home = dir.path().join("home");
        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let state = Arc::new(Mutex::new(Edge::default()));
        let edge = Stub::start(edge_handler(state.clone())).await;
        Rig {
            _dir: dir,
            data_dir,
            home,
            edge,
            state,
        }
    }

    fn broker_config(&self, github_api: &str) -> BrokerConfig {
        BrokerConfig {
            data_dir: self.data_dir.clone(),
            home: self.home.clone(),
            github_api: github_api.into(),
            claude_config_dir: self.home.join(".claude"),
            claude_config_file: self.home.join(".claude.json"),
            claude_env_login: false,
            codex_env_login: false,
            github_repo: Some("acme/app".into()),
        }
    }

    async fn bootstrap(&self, enroll: Option<&str>) -> Result<Option<RunnerIdentity>, String> {
        runner::bootstrap_with(
            &self.data_dir,
            &self.edge.url(),
            enroll,
            &reqwest::Client::new(),
            &self.broker_config("http://127.0.0.1:9"),
        )
        .await
        .map_err(|e| e.to_string())
    }

    async fn enrolled(&self) -> Arc<RunnerIdentity> {
        Arc::new(
            self.bootstrap(Some(&format!("{ORG}.{USER}.cloud-a.secret-code")))
                .await
                .unwrap()
                .unwrap(),
        )
    }

    fn auth(&self, identity: Arc<RunnerIdentity>) -> Auth {
        Auth::new_runner(
            AuthConfig::new(identity.edge_url(), &self.data_dir),
            identity,
        )
    }

    fn grant(&self, provider: &str, status: u16, body: Value) {
        self.state
            .lock()
            .unwrap()
            .grants
            .insert(provider.into(), (status, body));
    }

    fn broker(&self, identity: Arc<RunnerIdentity>, github_api: &str) -> CredentialBroker {
        let auth = self.auth(identity.clone());
        CredentialBroker::new(self.broker_config(github_api), identity, Arc::new(auth))
    }
}

fn in_ms(secs: i64) -> i64 {
    chrono::Utc::now().timestamp_millis() + secs * 1000
}

fn run_request() -> RunRequest {
    serde_json::from_str(
        r#"{"prompt":"p","model":null,"reasoning":null,"cwd":".","sandbox":"workspace-write","resume":null}"#,
    )
    .unwrap()
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

// ---------------------------------------------------------------------------
// Enrollment + identity
// ---------------------------------------------------------------------------

#[tokio::test]
async fn enrollment_retries_network_failures_then_persists_identity() {
    let rig = Rig::new().await;
    rig.state.lock().unwrap().enroll_status = vec![503];
    let identity = rig.enrolled().await;

    let enrolls = rig.edge.requests("/runner/enroll");
    assert_eq!(enrolls.len(), 2, "one 503, then success");
    let body = &enrolls[1].body;
    assert_eq!(body["orgId"], ORG);
    assert_eq!(body["userId"], USER);
    assert_eq!(body["deviceId"], "cloud-a");
    assert_eq!(body["code"], "secret-code");
    assert_eq!(body["publicKey"], identity.public_key());
    assert_eq!(B64URL.decode(identity.public_key()).unwrap().len(), 32);

    let runner_json: Value =
        serde_json::from_slice(&std::fs::read(rig.data_dir.join("runner.json")).unwrap()).unwrap();
    assert_eq!(
        runner_json,
        json!({"orgId": ORG, "userId": USER, "deviceId": "cloud-a", "edgeUrl": rig.edge.url()})
    );
    assert_eq!(mode(&rig.data_dir.join("runner.json")), 0o600);
    assert_eq!(mode(&rig.data_dir.join("runner-key")), 0o600);
    assert_eq!(
        std::fs::read_to_string(rig.data_dir.join("device-id")).unwrap(),
        "cloud-a"
    );
}

#[tokio::test]
async fn a_rejected_enrollment_code_is_fatal() {
    let rig = Rig::new().await;
    rig.state.lock().unwrap().enroll_status = vec![400];
    let err = rig
        .bootstrap(Some(&format!("{ORG}.{USER}.cloud-a.used-code")))
        .await
        .unwrap_err();
    assert!(err.contains("rejected"), "{err}");
    assert!(!rig.data_dir.join("runner.json").exists());
    assert_eq!(
        rig.edge.requests("/runner/enroll").len(),
        1,
        "no retry on 4xx"
    );
}

#[tokio::test]
async fn no_runner_json_and_no_env_is_not_a_runner() {
    let rig = Rig::new().await;
    assert!(rig.bootstrap(None).await.unwrap().is_none());
}

#[tokio::test]
async fn runner_json_wins_when_the_env_names_the_same_device() {
    let rig = Rig::new().await;
    let first = rig.enrolled().await;
    // Restart with the provisioning env still set (same device): no re-enroll.
    let again = rig
        .bootstrap(Some(&format!("{ORG}.{USER}.cloud-a.secret-code")))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rig.edge.requests("/runner/enroll").len(), 1);
    assert_eq!(again.public_key(), first.public_key());
    assert_eq!(again.device_id(), "cloud-a");
    // And without the env at all.
    let plain = rig.bootstrap(None).await.unwrap().unwrap();
    assert_eq!(plain.public_key(), first.public_key());
}

#[tokio::test]
async fn copied_identity_with_a_new_enroll_code_reenrolls_as_the_new_device() {
    let rig = Rig::new().await;
    let old = rig.enrolled().await;
    // The forked disk also carries the old device's managed credentials.
    let codex_home = rig.data_dir.join("cloud/codex-home");
    std::fs::create_dir_all(&codex_home).unwrap();
    std::fs::write(codex_home.join("auth.json"), r#"{"tokens":{}}"#).unwrap();
    std::fs::write(
        rig.data_dir.join("cloud/git-credentials"),
        "https://x-access-token:old@github.com\n",
    )
    .unwrap();
    std::fs::create_dir_all(rig.home.join(".claude")).unwrap();
    std::fs::write(
        rig.home.join(".claude/.credentials.json"),
        json!({"claudeAiOauth": {"accessToken": "old-at", "refreshToken": "", "expiresAt": 1},
               "mcpOAuth": {"server": "keep"}})
        .to_string(),
    )
    .unwrap();
    let repo = rig.data_dir.join("repos/app/README.md");
    std::fs::create_dir_all(repo.parent().unwrap()).unwrap();
    std::fs::write(&repo, "user file").unwrap();

    let new = rig
        .bootstrap(Some(&format!("{ORG}.{USER}.cloud-b.fresh-code")))
        .await
        .unwrap()
        .unwrap();

    let enrolls = rig.edge.requests("/runner/enroll");
    assert_eq!(enrolls.len(), 2);
    assert_eq!(enrolls[1].body["deviceId"], "cloud-b");
    assert_ne!(
        new.public_key(),
        old.public_key(),
        "a NEW key per enrollment"
    );
    assert_eq!(new.device_id(), "cloud-b");
    assert_eq!(
        std::fs::read_to_string(rig.data_dir.join("device-id")).unwrap(),
        "cloud-b"
    );
    let runner_json: Value =
        serde_json::from_slice(&std::fs::read(rig.data_dir.join("runner.json")).unwrap()).unwrap();
    assert_eq!(runner_json["deviceId"], "cloud-b");
    assert!(!codex_home.exists(), "managed CODEX_HOME discarded");
    assert!(!rig.data_dir.join("cloud/git-credentials").exists());
    let claude: Value =
        serde_json::from_slice(&std::fs::read(rig.home.join(".claude/.credentials.json")).unwrap())
            .unwrap();
    assert!(
        claude.get("claudeAiOauth").is_none(),
        "managed Claude login dropped"
    );
    assert_eq!(claude["mcpOAuth"]["server"], "keep");
    assert_eq!(std::fs::read_to_string(&repo).unwrap(), "user file");
}

#[tokio::test]
async fn discarding_a_copied_identity_keeps_a_native_claude_login() {
    let rig = Rig::new().await;
    rig.enrolled().await;
    std::fs::create_dir_all(rig.home.join(".claude")).unwrap();
    let native = json!({"claudeAiOauth": {"accessToken": "at", "refreshToken": "rt-native", "expiresAt": 1}});
    std::fs::write(
        rig.home.join(".claude/.credentials.json"),
        native.to_string(),
    )
    .unwrap();
    rig.bootstrap(Some(&format!("{ORG}.{USER}.cloud-b.fresh-code")))
        .await
        .unwrap()
        .unwrap();
    let after: Value =
        serde_json::from_slice(&std::fs::read(rig.home.join(".claude/.credentials.json")).unwrap())
            .unwrap();
    assert_eq!(after, native);
}

// ---------------------------------------------------------------------------
// Runner auth
// ---------------------------------------------------------------------------

#[tokio::test]
async fn runner_auth_is_signed_in_and_mints_signed_monotonic_tokens() {
    let rig = Rig::new().await;
    let identity = rig.enrolled().await;
    let auth = rig.auth(identity.clone());

    match auth.state() {
        AuthState::SignedIn { user, org_id } => {
            assert_eq!(user.id, USER);
            assert_eq!(user.email, "");
            assert_eq!(org_id.as_deref(), Some(ORG));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(auth.user_id().as_deref(), Some(USER));
    assert!(auth.is_runner());
    assert!(!auth.workos_enabled());
    assert_eq!(
        Engine::initial_workspace_scope(&auth),
        zeron_proto::WorkspaceScope::Synced
    );
    // Sign-out is meaningless for a runner.
    auth.sign_out();
    assert!(auth.state().is_signed_in());
    assert!(!rig.data_dir.join("session.json").exists());

    // Single-flight: concurrent consumers share one signed request.
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..10 {
        let auth = auth.clone();
        tasks.spawn(async move { auth.access_token().await });
    }
    let mut tokens = Vec::new();
    while let Some(result) = tasks.join_next().await {
        tokens.push(result.unwrap().unwrap());
    }
    assert!(tokens.windows(2).all(|w| w[0] == w[1]));
    assert_eq!(rig.edge.requests("/runner/token").len(), 1);
    // Cached while fresh; TokenSource agrees.
    assert_eq!(TokenSource::token(&auth).await.unwrap(), tokens[0]);
    assert_eq!(rig.edge.requests("/runner/token").len(), 1);

    // Short-lived tokens refresh on every use, each with a larger ts (the
    // stub's fence would answer `stale` otherwise).
    rig.state.lock().unwrap().token_ttl = 10;
    let fresh = rig.auth(identity.clone());
    for _ in 0..3 {
        fresh.access_token().await.unwrap();
    }
    let ts: Vec<i64> = rig
        .edge
        .requests("/runner/token")
        .iter()
        .map(|r| r.body["ts"].as_i64().unwrap())
        .collect();
    assert_eq!(ts.len(), 4);
    assert!(ts.windows(2).all(|w| w[1] > w[0]), "{ts:?}");
    for req in rig.edge.requests("/runner/token") {
        let b = &req.body;
        let message = format!(
            "zeron-runner-token\n{ORG}\n{USER}\ncloud-a\n{}",
            b["ts"].as_i64().unwrap()
        );
        assert!(verify(
            &identity.public_key(),
            &message,
            b["sig"].as_str().unwrap()
        ));
    }
}

#[tokio::test]
async fn a_rejected_runner_reads_signed_out_but_stays_up() {
    let rig = Rig::new().await;
    let identity = rig.enrolled().await;
    rig.state.lock().unwrap().token_reject = true;
    let auth = rig.auth(identity);
    assert!(matches!(
        auth.access_token().await,
        Err(TokenError::SignedOut)
    ));
    // The identity never changes; the engine is not torn down.
    assert!(auth.state().is_signed_in());
}

#[tokio::test]
async fn heartbeat_reports_active_runs_with_the_runner_bearer() {
    let rig = Rig::new().await;
    let identity = rig.enrolled().await;
    let auth = rig.auth(identity.clone());
    // Relay traffic: two real requests and a stream open count; the dial
    // readiness probe and EngineReady do not.
    struct Echo;
    #[async_trait::async_trait]
    impl zeron_rpc::RpcService for Echo {
        async fn handle(
            &self,
            _method: &str,
            params: Value,
        ) -> Result<zeron_rpc::RpcReply, zeron_rpc::RpcError> {
            zeron_rpc::RpcReply::value(&params)
        }
    }
    let activity = runner::RelayActivity::default();
    let served = activity.wrap(Arc::new(Echo));
    use zeron_rpc::methods;
    for (method, params) in [
        (methods::LIST_HARNESSES, json!({ "readinessProbe": true })),
        (methods::ENGINE_READY, json!({})),
        (methods::LIST_HARNESSES, json!({})),
        (methods::LIST_REPOS, json!({})),
        (methods::WATCH_QUEUE, json!({"chatId": "c"})),
    ] {
        served.handle(method, params).await.unwrap();
    }
    let task = runner::spawn_heartbeat(
        identity.edge_url(),
        Arc::new(auth),
        Arc::new(|| 2),
        activity.clone(),
        Duration::from_millis(50),
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        while rig.edge.requests("/runner/heartbeat").len() < 2 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("heartbeats");
    task.abort();
    let beats = rig.edge.requests("/runner/heartbeat");
    assert_eq!(beats[0].body, json!({"activeRuns": 2, "clients": 3}));
    // Drained by the delivered beat: an idle open link reports nothing.
    assert_eq!(beats[1].body, json!({"activeRuns": 2, "clients": 0}));
    assert!(beats[0].bearer.as_deref().is_some_and(|b| b.contains('.')));
}

// ---------------------------------------------------------------------------
// Credential broker
// ---------------------------------------------------------------------------

#[tokio::test]
async fn codex_grant_becomes_a_managed_codex_home() {
    let rig = Rig::new().await;
    let identity = rig.enrolled().await;
    std::fs::create_dir_all(rig.home.join(".codex")).unwrap();
    std::fs::write(rig.home.join(".codex/config.toml"), "model = \"gpt-6\"\n").unwrap();
    rig.grant(
        "codex",
        200,
        json!({"provider": "codex", "accessToken": "chatgpt-at", "expiresAt": in_ms(86_400),
               "accountId": "acct_1", "idToken": "h.p.s", "generation": 1}),
    );
    let broker = rig.broker(identity, "http://127.0.0.1:9");
    let mut request = run_request();
    broker
        .prepare(HarnessId::Codex, &mut request)
        .await
        .unwrap();

    let home = rig.data_dir.join("cloud/codex-home");
    assert_eq!(
        request.env.get("CODEX_HOME").map(String::as_str),
        Some(home.to_str().unwrap())
    );
    assert!(
        !request.env.contains_key("GH_TOKEN"),
        "github not connected"
    );
    let auth: Value =
        serde_json::from_slice(&std::fs::read(home.join("auth.json")).unwrap()).unwrap();
    assert_eq!(auth["OPENAI_API_KEY"], Value::Null);
    assert_eq!(auth["tokens"]["access_token"], "chatgpt-at");
    assert_eq!(auth["tokens"]["id_token"], "h.p.s");
    assert_eq!(auth["tokens"]["account_id"], "acct_1");
    assert_eq!(auth["tokens"]["refresh_token"], "");
    assert!(auth["last_refresh"].is_string());
    assert_eq!(mode(&home.join("auth.json")), 0o600);
    let config = std::fs::read_to_string(home.join("config.toml")).unwrap();
    assert!(config.contains("cli_auth_credentials_store = \"file\""));
    assert!(config.contains("model = \"gpt-6\""));
    // The secret never serializes with the request.
    assert!(
        !serde_json::to_string(&request)
            .unwrap()
            .contains("chatgpt-at")
    );
}

#[tokio::test]
async fn openai_key_backs_codex_when_no_chatgpt_grant_and_revocation_removes_it() {
    let rig = Rig::new().await;
    let identity = rig.enrolled().await;
    rig.grant(
        "openai-key",
        200,
        json!({"provider": "openai-key", "accessToken": "sk-own", "expiresAt": in_ms(86_400 * 365), "generation": 1}),
    );
    let broker = rig.broker(identity.clone(), "http://127.0.0.1:9");
    let mut request = run_request();
    broker
        .prepare(HarnessId::Codex, &mut request)
        .await
        .unwrap();
    let auth_file = rig.data_dir.join("cloud/codex-home/auth.json");
    let auth: Value = serde_json::from_slice(&std::fs::read(&auth_file).unwrap()).unwrap();
    assert_eq!(auth, json!({"OPENAI_API_KEY": "sk-own"}));

    // Disconnected later (fresh broker: no cache): the stale file goes away,
    // and with no login of the sandbox's own the run is refused up front,
    // saying where to sign in.
    rig.state.lock().unwrap().grants.clear();
    let broker = rig.broker(identity, "http://127.0.0.1:9");
    let mut request = run_request();
    assert_eq!(
        broker
            .prepare(HarnessId::Codex, &mut request)
            .await
            .unwrap_err(),
        zeron_engine::credentials::CODEX_NOT_SIGNED_IN
    );
    assert!(!request.env.contains_key("CODEX_HOME"));
    assert!(!auth_file.exists());
    // A sandbox `codex login` is used as is.
    std::fs::create_dir_all(rig.home.join(".codex")).unwrap();
    std::fs::write(rig.home.join(".codex/auth.json"), "{}").unwrap();
    let mut request = run_request();
    broker
        .prepare(HarnessId::Codex, &mut request)
        .await
        .unwrap();
    assert!(!request.env.contains_key("CODEX_HOME"));
}

#[tokio::test]
async fn claude_grant_is_written_to_the_cli_credentials_without_a_refresh_token() {
    let rig = Rig::new().await;
    let identity = rig.enrolled().await;
    std::fs::create_dir_all(rig.home.join(".claude")).unwrap();
    std::fs::write(
        rig.home.join(".claude/.credentials.json"),
        json!({"mcpOAuth": {"srv": {"accessToken": "mcp"}}}).to_string(),
    )
    .unwrap();
    let expires = in_ms(4 * 3600);
    rig.grant(
        "claude",
        200,
        json!({"provider": "claude", "accessToken": "sk-ant-oat-x", "expiresAt": expires,
               "scopes": ["user:inference"], "subscriptionType": "max", "generation": 2}),
    );
    rig.grant(
        "anthropic-key",
        200,
        json!({"provider": "anthropic-key", "accessToken": "sk-ant-api", "expiresAt": in_ms(86_400), "generation": 1}),
    );
    let broker = rig.broker(identity, "http://127.0.0.1:9");
    let mut request = run_request();
    broker
        .prepare(HarnessId::ClaudeCode, &mut request)
        .await
        .unwrap();

    // The grant goes to the CLI's own credential file, never the env.
    assert!(!request.env.contains_key("ANTHROPIC_API_KEY"));
    assert!(!request.env.contains_key("CLAUDE_CODE_OAUTH_TOKEN"));
    let path = rig.home.join(".claude/.credentials.json");
    let creds: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        creds["claudeAiOauth"],
        json!({"accessToken": "sk-ant-oat-x", "refreshToken": "", "expiresAt": expires,
               "scopes": ["user:inference"], "subscriptionType": "max"})
    );
    assert_eq!(
        creds["mcpOAuth"]["srv"]["accessToken"], "mcp",
        "siblings kept"
    );
    assert_eq!(mode(&path), 0o600);
    let state: Value =
        serde_json::from_slice(&std::fs::read(rig.home.join(".claude.json")).unwrap()).unwrap();
    assert_eq!(state["hasCompletedOnboarding"], true);
    // A Codex run never receives the Claude credential (and has no login).
    let mut codex = run_request();
    assert!(broker.prepare(HarnessId::Codex, &mut codex).await.is_err());
    assert!(codex.env.values().all(|v| !v.contains("sk-ant")));
}

#[tokio::test]
async fn native_claude_login_is_left_alone_then_api_key_is_the_fallback() {
    let rig = Rig::new().await;
    let identity = rig.enrolled().await;
    rig.grant(
        "anthropic-key",
        200,
        json!({"provider": "anthropic-key", "accessToken": "sk-ant-api", "expiresAt": in_ms(86_400), "generation": 1}),
    );
    std::fs::create_dir_all(rig.home.join(".claude")).unwrap();
    let path = rig.home.join(".claude/.credentials.json");
    let native = json!({"claudeAiOauth": {"accessToken": "at", "refreshToken": "rt-native", "expiresAt": 1}});
    std::fs::write(&path, native.to_string()).unwrap();

    let broker = rig.broker(identity.clone(), "http://127.0.0.1:9");
    let mut request = run_request();
    broker
        .prepare(HarnessId::ClaudeCode, &mut request)
        .await
        .unwrap();
    assert!(request.env.is_empty(), "{:?}", request.env.keys());
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(&path).unwrap()).unwrap(),
        native
    );

    std::fs::remove_file(&path).unwrap();
    let broker = rig.broker(identity, "http://127.0.0.1:9");
    let mut request = run_request();
    broker
        .prepare(HarnessId::ClaudeCode, &mut request)
        .await
        .unwrap();
    assert_eq!(
        request.env.get("ANTHROPIC_API_KEY").map(String::as_str),
        Some("sk-ant-api")
    );
    assert!(!path.exists(), "no credential file is ever invented");
}

#[tokio::test]
async fn github_grant_sets_env_git_store_helper_and_gh_hosts() {
    let rig = Rig::new().await;
    let identity = rig.enrolled().await;
    rig.grant(
        "github",
        200,
        json!({"provider": "github", "accessToken": "ghu_token", "expiresAt": in_ms(8 * 3600),
               "account": "octo", "generation": 1}),
    );
    let broker = rig.broker(identity, "http://127.0.0.1:9");
    let mut request = run_request();
    // No Claude login here, so the run itself is refused; GitHub is wired
    // regardless (terminals use it too).
    assert_eq!(
        broker
            .prepare(HarnessId::ClaudeCode, &mut request)
            .await
            .unwrap_err(),
        zeron_engine::credentials::CLAUDE_NOT_SIGNED_IN
    );
    assert_eq!(
        request.env.get("GH_TOKEN").map(String::as_str),
        Some("ghu_token")
    );
    assert_eq!(
        request.env.get("GITHUB_TOKEN").map(String::as_str),
        Some("ghu_token")
    );
    let store = rig.data_dir.join("cloud/git-credentials");
    assert_eq!(
        std::fs::read_to_string(&store).unwrap(),
        "https://x-access-token:ghu_token@github.com\n"
    );
    assert_eq!(mode(&store), 0o600);
    let helper = std::process::Command::new("git")
        .args([
            "config",
            "--global",
            "--get-all",
            "credential.https://github.com.helper",
        ])
        .env("HOME", &rig.home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    // An empty helper first: it resets the list, so a system helper (Apple
    // git's osxkeychain) never also stores the App token.
    assert_eq!(
        String::from_utf8_lossy(&helper.stdout),
        format!("\nstore --file=\"{}\"\n", store.display())
    );
    let hosts = std::fs::read_to_string(rig.home.join(".config/gh/hosts.yml")).unwrap();
    assert!(hosts.contains("oauth_token: ghu_token"));
    assert!(hosts.contains("user: octo"));
}

#[tokio::test]
async fn grants_are_cached_fall_back_offline_and_drop_on_revocation() {
    let rig = Rig::new().await;
    let identity = rig.enrolled().await;
    // 4 minutes left: inside the 5-minute slack, so every use re-asks.
    rig.grant(
        "anthropic-key",
        200,
        json!({"provider": "anthropic-key", "accessToken": "k1", "expiresAt": in_ms(240), "generation": 1}),
    );
    let broker = rig.broker(identity.clone(), "http://127.0.0.1:9");
    assert_eq!(broker.prepare_claude().await.as_deref(), Some("k1"));

    // Vault unreachable (503): the cached, still-valid grant keeps working.
    rig.grant("anthropic-key", 503, json!({"error": "unavailable"}));
    assert_eq!(broker.prepare_claude().await.as_deref(), Some("k1"));

    // Revoked (403): the cached grant is dropped immediately.
    rig.grant("anthropic-key", 403, json!({"error": "device_revoked"}));
    assert_eq!(broker.prepare_claude().await, None);

    // A long-lived grant is served from cache without asking again.
    rig.grant(
        "anthropic-key",
        200,
        json!({"provider": "anthropic-key", "accessToken": "k2", "expiresAt": in_ms(86_400), "generation": 2}),
    );
    // (Same identity: one process has one strictly increasing ts counter.)
    let broker2 = rig.broker(identity, "http://127.0.0.1:9");
    assert_eq!(broker2.prepare_claude().await.as_deref(), Some("k2"));
    let asked = rig.edge.requests("/vault/").len();
    assert_eq!(broker2.prepare_claude().await.as_deref(), Some("k2"));
    // Both the key and the (briefly cached) "claude not connected" answer
    // come from cache: no new vault round trip.
    assert_eq!(rig.edge.requests("/vault/").len(), asked);
}

#[tokio::test]
async fn a_stale_grant_timestamp_is_retried_once_with_a_larger_ts() {
    let rig = Rig::new().await;
    let identity = rig.enrolled().await;
    rig.grant(
        "anthropic-key",
        200,
        json!({"provider": "anthropic-key", "accessToken": "k1", "expiresAt": in_ms(86_400), "generation": 1}),
    );
    rig.state.lock().unwrap().stale_once = true;
    let broker = rig.broker(identity, "http://127.0.0.1:9");
    assert_eq!(broker.prepare_claude().await.as_deref(), Some("k1"));
    let ts: Vec<i64> = rig
        .edge
        .requests("/vault/")
        .iter()
        .map(|r| r.body["ts"].as_i64().unwrap())
        .collect();
    assert!(ts.windows(2).all(|w| w[1] > w[0]), "{ts:?}");
}

// ---------------------------------------------------------------------------
// ListGithubRepos
// ---------------------------------------------------------------------------

fn gh_repo(name: &str, pushed: &str) -> Value {
    json!({"full_name": name, "clone_url": format!("https://github.com/{name}.git"),
           "default_branch": "main", "private": true, "description": "d", "pushed_at": pushed})
}

#[tokio::test]
async fn github_repos_are_the_installation_tokens_repositories_filtered_and_sorted() {
    let rig = Rig::new().await;
    let identity = rig.enrolled().await;
    rig.grant(
        "github",
        200,
        json!({"provider": "github", "accessToken": "ghs_installation", "expiresAt": in_ms(3600), "generation": 1}),
    );
    let github = Stub::start(|req: &Req| {
        assert_eq!(req.bearer.as_deref(), Some("ghs_installation"));
        let path = req.path.as_str();
        if path.starts_with("/installation/repositories?per_page=100&page=1") {
            let mut page: Vec<Value> = (0..100)
                .map(|i| gh_repo(&format!("acme/r{i:03}"), "2026-08-01T00:00:00Z"))
                .collect();
            page[0] = gh_repo("acme/api", "2026-09-01T00:00:00Z");
            (200, json!({"total_count": 102, "repositories": page}))
        } else if path.starts_with("/installation/repositories?per_page=100&page=2") {
            (
                200,
                json!({"total_count": 102, "repositories": [gh_repo("acme/web", "2026-09-20T00:00:00Z"),
                                                            gh_repo("acme/dots", "2026-09-10T00:00:00Z")]}),
            )
        } else {
            (404, json!({}))
        }
    })
    .await;
    let broker = rig.broker(identity, &github.url());
    let repos = broker.list_github_repos(None).await.unwrap();
    assert_eq!(repos.len(), 102);
    assert_eq!(
        repos[..3]
            .iter()
            .map(|r| r.full_name.as_str())
            .collect::<Vec<_>>(),
        ["acme/web", "acme/dots", "acme/api"]
    );
    assert!(repos[0].private);
    let filtered = broker.list_github_repos(Some("WEB")).await.unwrap();
    assert_eq!(
        filtered
            .iter()
            .map(|r| r.full_name.as_str())
            .collect::<Vec<_>>(),
        ["acme/web"]
    );
    // The grant named the session repository.
    assert_eq!(rig.state.lock().unwrap().github_repos, ["acme/app"]);
}

#[tokio::test]
async fn a_device_without_a_github_repository_never_asks_for_a_github_grant() {
    let rig = Rig::new().await;
    let identity = rig.enrolled().await;
    rig.grant(
        "github",
        200,
        json!({"provider": "github", "accessToken": "ghs_installation", "expiresAt": in_ms(3600), "generation": 1}),
    );
    let auth = rig.auth(identity.clone());
    let config = BrokerConfig {
        github_repo: None,
        ..rig.broker_config("http://127.0.0.1:9")
    };
    let broker = CredentialBroker::new(config, identity, Arc::new(auth));
    assert_eq!(broker.github_token().await, None);
    assert_eq!(
        broker.list_github_repos(None).await.unwrap_err(),
        GITHUB_NOT_CONNECTED
    );
    assert_eq!(rig.state.lock().unwrap().grant_requests.get("github"), None);
}

#[tokio::test]
async fn github_repos_without_a_grant_explain_how_to_connect() {
    let rig = Rig::new().await;
    let identity = rig.enrolled().await;
    let broker = rig.broker(identity, "http://127.0.0.1:9");
    assert_eq!(
        broker.list_github_repos(None).await.unwrap_err(),
        GITHUB_NOT_CONNECTED
    );
}

#[tokio::test]
async fn list_github_repos_on_a_local_engine_asks_for_a_signed_in_account() {
    let dir = tempfile::tempdir().unwrap();
    let core = zeron_engine::EngineCore::assemble(
        dir.path(),
        Arc::new(zeron_engine::default_registry()),
        HarnessId::Mock,
        None,
    )
    .unwrap();
    let rpc = core.rpc_service();
    let err = zeron_rpc::RpcService::handle(
        rpc.as_ref(),
        zeron_rpc::methods::LIST_GITHUB_REPOS,
        json!({"query": "x"}),
    )
    .await
    .err()
    .expect("a local-only engine can't reach the vault");
    // Laptops list through the edge's vault (they hold no GitHub grant); a
    // local profile has no edge, so it answers with the sign-in hint and
    // never pretends GitHub is merely disconnected.
    assert!(err.to_string().contains("signed-in Zeron account"), "{err}");
    assert!(!err.to_string().contains(GITHUB_NOT_CONNECTED), "{err}");
    core.shutdown().await;
}

// ---------------------------------------------------------------------------
// Background refresh + native-login safety
// ---------------------------------------------------------------------------

#[tokio::test]
async fn background_refresh_rewrites_claude_and_github_credentials_before_expiry() {
    let rig = Rig::new().await;
    let identity = rig.enrolled().await;
    // 8 minutes left: valid for a spawn (5-min slack), due for the refresher
    // (10-min lead).
    rig.grant(
        "claude",
        200,
        json!({"provider": "claude", "accessToken": "oat-v1", "expiresAt": in_ms(480), "generation": 1}),
    );
    rig.grant(
        "github",
        200,
        json!({"provider": "github", "accessToken": "ghu_v1", "expiresAt": in_ms(480), "generation": 1}),
    );
    let broker = rig.broker(identity.clone(), "http://127.0.0.1:9");
    let mut request = run_request();
    broker
        .prepare(HarnessId::ClaudeCode, &mut request)
        .await
        .unwrap();
    let creds = rig.home.join(".claude/.credentials.json");
    let read_token = || {
        serde_json::from_slice::<Value>(&std::fs::read(&creds).unwrap()).unwrap()["claudeAiOauth"]
            ["accessToken"]
            .clone()
    };
    assert_eq!(read_token(), "oat-v1");

    rig.grant(
        "claude",
        200,
        json!({"provider": "claude", "accessToken": "oat-v2", "expiresAt": in_ms(4 * 3600), "generation": 2}),
    );
    rig.grant(
        "github",
        200,
        json!({"provider": "github", "accessToken": "ghu_v2", "expiresAt": in_ms(8 * 3600), "generation": 2}),
    );
    broker.refresh_due().await;
    assert_eq!(read_token(), "oat-v2");
    let creds_json: Value = serde_json::from_slice(&std::fs::read(&creds).unwrap()).unwrap();
    assert_eq!(creds_json["claudeAiOauth"]["refreshToken"], "");
    assert_eq!(
        std::fs::read_to_string(rig.data_dir.join("cloud/git-credentials")).unwrap(),
        "https://x-access-token:ghu_v2@github.com\n"
    );

    // Disconnected: the refresher drops the managed login.
    rig.state.lock().unwrap().grants.clear();
    let broker = rig.broker(identity, "http://127.0.0.1:9");
    // Seed the cache with a nearly-expired grant first so the pass is due.
    rig.grant(
        "claude",
        200,
        json!({"provider": "claude", "accessToken": "oat-v3", "expiresAt": in_ms(480), "generation": 3}),
    );
    broker.prepare_claude().await;
    rig.grant("claude", 403, json!({"error": "device_revoked"}));
    broker.refresh_due().await;
    let after: Value = serde_json::from_slice(&std::fs::read(&creds).unwrap()).unwrap();
    assert!(after.get("claudeAiOauth").is_none(), "{after}");
}

#[tokio::test]
async fn a_claude_grant_displacing_a_native_login_backs_it_up_and_restores_it() {
    let rig = Rig::new().await;
    let identity = rig.enrolled().await;
    std::fs::create_dir_all(rig.home.join(".claude")).unwrap();
    let path = rig.home.join(".claude/.credentials.json");
    let native = json!({"accessToken": "native-at", "refreshToken": "rt-native", "expiresAt": 5});
    std::fs::write(
        &path,
        json!({"claudeAiOauth": native, "mcpOAuth": {}}).to_string(),
    )
    .unwrap();
    rig.grant(
        "claude",
        200,
        json!({"provider": "claude", "accessToken": "oat", "expiresAt": in_ms(3600), "generation": 1}),
    );
    let broker = rig.broker(identity.clone(), "http://127.0.0.1:9");
    assert_eq!(broker.prepare_claude().await, None);
    let creds: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(creds["claudeAiOauth"]["accessToken"], "oat");
    assert!(
        rig.data_dir
            .join("cloud/claude-native-credentials.json")
            .exists()
    );

    // Grant revoked: the native login comes back, untouched.
    rig.state.lock().unwrap().grants.clear();
    let broker = rig.broker(identity, "http://127.0.0.1:9");
    assert_eq!(broker.prepare_claude().await, None, "native login: no env");
    let creds: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(creds["claudeAiOauth"], native);
    assert!(creds.get("mcpOAuth").is_some());
    assert!(
        !rig.data_dir
            .join("cloud/claude-native-credentials.json")
            .exists()
    );
}
