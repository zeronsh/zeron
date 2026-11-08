//! The laptop-side Cloud / vault RPCs through the engine's real dispatch
//! (docs/design/cloud-device.md, "Engine changes → Laptop side"): unavailable
//! without an edge (no network call), and acting as the user against the edge
//! when one is configured. Never relay-forwarded.

use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use zeron_engine::{EdgeConfig, EngineCore, HarnessId, default_registry};
use zeron_proto::{CloudState, CloudStatus, CloudUsage, VaultStatus};
use zeron_rpc::{memory_client, methods};

/// Request lines (`METHOD /path`) + their Authorization headers.
type Seen = Arc<Mutex<Vec<(String, Option<String>)>>>;

/// A tiny edge: `GET /cloud/{org}` and `GET /vault/{org}` answer; everything
/// else (the workspace's room dials included) is a 404.
async fn mock_edge() -> (String, Seen) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let seen: Seen = Arc::default();
    let log = seen.clone();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let log = log.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 16 * 1024];
                let Ok(n) = stream.read(&mut buf).await else {
                    return;
                };
                let head = String::from_utf8_lossy(&buf[..n]).to_string();
                let line = head.lines().next().unwrap_or("").to_string();
                let mut parts = line.split_whitespace();
                let request = format!(
                    "{} {}",
                    parts.next().unwrap_or(""),
                    parts.next().unwrap_or("")
                );
                let authorization = head.lines().find_map(|l| {
                    l.split_once(':')
                        .filter(|(name, _)| name.eq_ignore_ascii_case("authorization"))
                        .map(|(_, value)| value.trim().to_string())
                });
                let (status, body) = if request.starts_with("GET /cloud/") {
                    // The edge says whether the account is offered Cloud.
                    (
                        "200 OK",
                        r#"{"state":"ready","deviceId":"cloud-abc","available":true}"#,
                    )
                } else if request.starts_with("GET /vault/") {
                    (
                        "200 OK",
                        r#"{"connections":[],"devices":[],"available":true}"#,
                    )
                } else {
                    (
                        "404 Not Found",
                        r#"{"error":"not_found","message":"no route"}"#,
                    )
                };
                log.lock().unwrap().push((request, authorization));
                let response = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\n\
                     content-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.shutdown().await;
            });
        }
    });
    (url, seen)
}

#[tokio::test]
async fn cloud_rpcs_without_an_edge_are_unavailable_and_offline() {
    let dir = tempfile::tempdir().unwrap();
    let core = EngineCore::assemble(
        dir.path(),
        Arc::new(default_registry()),
        HarnessId::Mock,
        None,
    )
    .unwrap();
    let client = memory_client(core.rpc_service());

    let status: CloudStatus = client
        .call_as(methods::CLOUD_STATUS, serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(status.state, CloudState::Off);
    assert!(!status.available);
    // Lifecycle calls answer the same way instead of failing.
    let enabled: CloudStatus = client
        .call_as(methods::CLOUD_ENABLE, serde_json::json!({}))
        .await
        .unwrap();
    assert!(!enabled.available);
    let usage: CloudUsage = client
        .call_as(methods::CLOUD_USAGE, serde_json::Value::Null)
        .await
        .unwrap();
    assert!(!usage.available);
    let vault: VaultStatus = client
        .call_as(methods::VAULT_STATUS, serde_json::json!({}))
        .await
        .unwrap();
    assert!(!vault.available);

    // Credential uploads refuse before anything starts (no browser, no CLI).
    for method in [methods::VAULT_CONNECT_CODEX, methods::VAULT_CONNECT_CLAUDE] {
        let error = client
            .call(
                method,
                serde_json::json!({ "authorizedDevices": ["cloud-1"] }),
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("signed-in"), "{method}: {error}");
    }
    assert!(
        client
            .call(
                methods::VAULT_PUT_API_KEY,
                serde_json::json!({ "provider": "anthropic-key", "key": "sk-1" }),
            )
            .await
            .is_err()
    );
    // Bad params are rejected as such.
    assert!(
        client
            .call(
                methods::VAULT_PUT_API_KEY,
                serde_json::json!({ "provider": "nope", "key": "sk-1" }),
            )
            .await
            .is_err()
    );
    core.shutdown().await;
}

#[tokio::test]
async fn cloud_rpcs_reach_the_edge_as_the_user_and_are_never_forwarded() {
    let (edge_url, seen) = mock_edge().await;
    let dir = tempfile::tempdir().unwrap();
    let core = EngineCore::assemble(
        dir.path(),
        Arc::new(default_registry()),
        HarnessId::Mock,
        Some(EdgeConfig::with_static_token(edge_url, "dev-user")),
    )
    .unwrap();
    let client = memory_client(core.rpc_service());

    // A targetDeviceId is ignored: these act for THIS runtime's user only
    // (no peer links exist here, so a forward would have failed).
    let status: CloudStatus = client
        .call_as(
            methods::CLOUD_STATUS,
            serde_json::json!({ "targetDeviceId": "some-other-device" }),
        )
        .await
        .unwrap();
    assert_eq!(status.state, CloudState::Ready);
    assert_eq!(status.device_id.as_deref(), Some("cloud-abc"));
    assert!(status.available);
    let vault: VaultStatus = client
        .call_as(methods::VAULT_STATUS, serde_json::json!({}))
        .await
        .unwrap();
    assert!(vault.available);

    let org = core.workspace.org_id().to_string();
    let seen = seen.lock().unwrap().clone();
    for path in [format!("GET /cloud/{org}"), format!("GET /vault/{org}")] {
        let (_, authorization) = seen
            .iter()
            .find(|(request, _)| *request == path)
            .unwrap_or_else(|| panic!("{path} not called: {seen:?}"));
        assert_eq!(authorization.as_deref(), Some("Bearer dev-user"));
    }
    core.shutdown().await;
}

/// A scripted edge: `respond(request line, body)` → (status, JSON body).
/// Records every request line + body.
type Requests = Arc<Mutex<Vec<(String, String)>>>;

#[tokio::test]
async fn waking_on_a_size_and_the_default_size_reach_the_edge() {
    let (edge_url, seen) = scripted_edge(|request, body| {
        if request.contains("/machine") {
            return (
                200,
                r#"{"state":"ready","deviceId":"cloud-abc","available":true,"machine":"large"}"#
                    .to_string(),
            );
        }
        if request.contains("/retry") {
            return (
                200,
                r#"{"chatId":"chat-1","deviceId":"cloud-s1","spaceId":"sp","state":"provisioning","createdAt":1}"#
                    .to_string(),
            );
        }
        if request.contains("/wake") {
            if body.contains("\"small\"") {
                return (
                    409,
                    r#"{"error":"machine_too_small","message":"That machine holds 20 GB."}"#
                        .to_string(),
                );
            }
            return (
                200,
                r#"{"chatId":"chat-1","deviceId":"cloud-s1","spaceId":"sp","state":"starting","createdAt":1,"machine":"xlarge"}"#
                    .to_string(),
            );
        }
        if request.starts_with("GET /cloud/") {
            return (
                200,
                r#"{"state":"ready","deviceId":"cloud-abc","available":true,"machine":"large"}"#
                    .to_string(),
            );
        }
        (404, r#"{"error":"not_found"}"#.to_string())
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let core = EngineCore::assemble(
        dir.path(),
        Arc::new(default_registry()),
        HarnessId::Mock,
        Some(EdgeConfig::with_static_token(edge_url, "dev-user")),
    )
    .unwrap();
    let client = memory_client(core.rpc_service());
    let org = core.workspace.org_id().to_string();

    let woke: zeron_proto::CloudSession = client
        .call_as(
            methods::CLOUD_SESSION_WAKE,
            serde_json::json!({ "chatId": "chat-1", "machine": "xlarge" }),
        )
        .await
        .unwrap();
    assert_eq!(woke.state, CloudState::Starting);
    assert_eq!(woke.machine.as_deref(), Some("xlarge"));
    // A caller from before sizes (only a chat id) still wakes it as it is.
    let _: zeron_proto::CloudSession = client
        .call_as(
            methods::CLOUD_SESSION_WAKE,
            serde_json::json!({ "chatId": "chat-1" }),
        )
        .await
        .unwrap();
    let fresh: zeron_proto::CloudSession = client
        .call_as(
            methods::CLOUD_SESSION_RETRY,
            serde_json::json!({ "chatId": "chat-1", "machine": "large" }),
        )
        .await
        .unwrap();
    assert_eq!(fresh.state, CloudState::Provisioning);
    let refused = client
        .call_as::<zeron_proto::CloudSession>(
            methods::CLOUD_SESSION_WAKE,
            serde_json::json!({ "chatId": "chat-1", "machine": "small" }),
        )
        .await
        .unwrap_err();
    assert!(
        refused.to_string().contains("disk_too_small"),
        "the typed refusal survives the RPC: {refused}"
    );
    let status: CloudStatus = client
        .call_as(
            methods::CLOUD_SET_MACHINE,
            serde_json::json!({ "machine": "large" }),
        )
        .await
        .unwrap();
    assert_eq!(status.machine.as_deref(), Some("large"));

    let seen = seen.lock().unwrap().clone();
    let wake = format!("POST /cloud/{org}/sessions/chat-1/wake");
    let wakes: Vec<serde_json::Value> = seen
        .iter()
        .filter(|(request, _)| *request == wake)
        .map(|(_, body)| serde_json::from_str(body).unwrap())
        .collect();
    assert_eq!(
        wakes,
        vec![
            serde_json::json!({ "machine": "xlarge" }),
            serde_json::json!({}),
            serde_json::json!({ "machine": "small" }),
        ]
    );
    let retry = format!("POST /cloud/{org}/sessions/chat-1/retry");
    let (_, body) = seen
        .iter()
        .find(|(request, _)| *request == retry)
        .unwrap_or_else(|| panic!("{retry} not called: {seen:?}"));
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(body).unwrap(),
        serde_json::json!({ "machine": "large" })
    );
    let put = format!("PUT /cloud/{org}/machine");
    let (_, body) = seen
        .iter()
        .find(|(request, _)| *request == put)
        .unwrap_or_else(|| panic!("{put} not called: {seen:?}"));
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(body).unwrap(),
        serde_json::json!({ "machine": "large" })
    );
    core.shutdown().await;
}

async fn scripted_edge(
    respond: impl Fn(&str, &str) -> (u16, String) + Send + Sync + 'static,
) -> (String, Requests) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let seen: Requests = Arc::default();
    let log = seen.clone();
    let respond = Arc::new(respond);
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let log = log.clone();
            let respond = respond.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = vec![0u8; 16 * 1024];
                // Read the head, then exactly content-length body bytes.
                let (head, mut body) = loop {
                    let Ok(n) = stream.read(&mut chunk).await else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    if let Some(at) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        break (
                            String::from_utf8_lossy(&buf[..at]).to_string(),
                            buf[at + 4..].to_vec(),
                        );
                    }
                };
                let length = head
                    .lines()
                    .find_map(|l| {
                        l.split_once(':')
                            .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                            .and_then(|(_, v)| v.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                while body.len() < length {
                    let Ok(n) = stream.read(&mut chunk).await else {
                        return;
                    };
                    if n == 0 {
                        break;
                    }
                    body.extend_from_slice(&chunk[..n]);
                }
                let line = head.lines().next().unwrap_or("").to_string();
                let mut parts = line.split_whitespace();
                let request = format!(
                    "{} {}",
                    parts.next().unwrap_or(""),
                    parts.next().unwrap_or("")
                );
                let body = String::from_utf8_lossy(&body).to_string();
                let (status, reply) = respond(&request, &body);
                log.lock().unwrap().push((request, body));
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
    (url, seen)
}

/// The edge's side of Cloud sessions (and the vault's GitHub reads), enough
/// for the engine.
/// Whether the test edge's `GET /cloud/{org}/models` answers (a test turns it
/// off to see the last list kept stand in).
static MODELS_UP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

fn cloud_edge(request: &str, body: &str) -> (u16, String) {
    let json: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
    if request.starts_with("GET /cloud/") && request.ends_with("/models") {
        if !MODELS_UP.load(std::sync::atomic::Ordering::SeqCst) {
            return (503, r#"{"error":"unavailable"}"#.into());
        }
        return (
            200,
            r#"{"models":{"claude-code":[{"id":"cloud-opus","label":"Cloud Opus","reasoningLevels":["low","high","ultrathink"],"options":[]}],"codex":[{"id":"cloud-codex","label":"Cloud Codex","reasoningLevels":["low"],"options":[]}]}}"#
                .into(),
        );
    }
    if request.starts_with("GET ") && request.contains("/github/repos") {
        return (
            200,
            r#"{"repos":[{"fullName":"acme/app","cloneUrl":"https://github.com/acme/app.git","defaultBranch":"main","private":true,"pushedAt":1}]}"#
                .into(),
        );
    }
    if request.starts_with("GET ") && request.contains("/github/branches?repo=acme%2Fapp") {
        return (200, r#"{"branches":["dev","main","feature/x"]}"#.into());
    }
    if request.starts_with("POST ") && request.ends_with("/sessions") {
        let chat_id = json["chatId"].as_str().unwrap_or_default();
        if chat_id == "chat-fail" {
            return (
                503,
                r#"{"error":"unavailable","message":"Cloud is down"}"#.into(),
            );
        }
        let name = json["repo"]["fullName"]
            .as_str()
            .and_then(|full| full.split('/').nth(1))
            .unwrap_or_default();
        let session = serde_json::json!({
            "chatId": chat_id, "deviceId": format!("cloud-s-{chat_id}"),
            "spaceId": json["spaceId"], "repo": json["repo"]["fullName"],
            "path": format!("/home/user/{name}"), "state": "provisioning", "createdAt": 1
        });
        return (200, session.to_string());
    }
    if request.starts_with("GET /cloud/") {
        return (
            200,
            r#"{"state":"ready","deviceId":"cloud-acct","awakeSessions":0,"maxAwakeSessions":5,"available":true}"#
                .into(),
        );
    }
    (404, r#"{"error":"not_found","message":"no route"}"#.into())
}

/// `Mutate createChat` with `body`'s fields.
async fn create(
    client: &zeron_rpc::RpcClient,
    body: serde_json::Value,
) -> Result<serde_json::Value, zeron_rpc::RpcError> {
    let mut op = serde_json::json!({ "op": "createChat" });
    op.as_object_mut()
        .unwrap()
        .extend(body.as_object().unwrap().clone());
    client
        .call_as::<serde_json::Value>(methods::MUTATE, op)
        .await
}

/// A project on this computer (`git` stamped by its owner, as SpacesSync
/// would), optionally with a GitHub origin.
async fn local_project(
    core: &EngineCore,
    client: &zeron_rpc::RpcClient,
    dir: &std::path::Path,
    space_id: &str,
    github_repo: Option<&str>,
) {
    let path = dir.join(space_id);
    std::fs::create_dir_all(&path).unwrap();
    client
        .call_as::<serde_json::Value>(
            methods::MUTATE,
            serde_json::json!({ "op": "createSpace", "spaceId": space_id,
                "deviceId": core.workspace.device_id(), "path": path, "gitDetected": true }),
        )
        .await
        .unwrap();
    core.workspace
        .set_space_git(space_id, true, None, None, github_repo)
        .unwrap();
}

#[tokio::test]
async fn chats_run_on_cloud_get_their_own_session_machines() {
    let (edge_url, seen) = scripted_edge(cloud_edge).await;
    let dir = tempfile::tempdir().unwrap();
    let core = EngineCore::assemble(
        dir.path(),
        Arc::new(default_registry()),
        HarnessId::Mock,
        Some(EdgeConfig::with_static_token(edge_url, "dev-user")),
    )
    .unwrap();
    let client = memory_client(core.rpc_service());
    local_project(&core, &client, dir.path(), "sp-app", Some("acme/app")).await;
    local_project(&core, &client, dir.path(), "sp-plain", None).await;
    local_project(&core, &client, dir.path(), "sp-other", Some("someone/else")).await;

    // Cloud picked in the checkout menu: the chat gets its own session
    // machine, working in that machine's clone.
    let created = create(
        &client,
        serde_json::json!({
        "chatId": "chat-1", "spaceId": "sp-app", "cloud": true }),
    )
    .await
    .unwrap();
    assert_eq!(created["deviceId"], "cloud-s-chat-1");
    assert_eq!(created["cwd"], "/home/user/app");
    let chat = core.workspace.chat("chat-1").unwrap().unwrap();
    assert_eq!(chat.device_id, "cloud-s-chat-1");
    assert_eq!(chat.space_id.as_deref(), Some("sp-app"));
    assert_eq!(chat.cwd.as_deref(), Some("/home/user/app"));

    // A second one, from the branch picked in the composer: a second machine.
    create(
        &client,
        serde_json::json!({
        "chatId": "chat-2", "spaceId": "sp-app", "cloud": true, "branch": "dev" }),
    )
    .await
    .unwrap();
    assert_eq!(
        core.workspace.chat("chat-2").unwrap().unwrap().device_id,
        "cloud-s-chat-2"
    );

    // A side chat an agent spawns shares its parent's machine (and files).
    create(
        &client,
        serde_json::json!({
        "chatId": "side-1", "spaceId": "sp-app", "parentChatId": "chat-1" }),
    )
    .await
    .unwrap();
    let side = core.workspace.chat("side-1").unwrap().unwrap();
    assert_eq!(side.device_id, "cloud-s-chat-1");
    assert_eq!(side.cwd.as_deref(), Some("/home/user/app"));

    // Without Cloud picked, the same project runs here.
    create(
        &client,
        serde_json::json!({ "chatId": "chat-local", "spaceId": "sp-app" }),
    )
    .await
    .unwrap();
    assert_eq!(
        core.workspace
            .chat("chat-local")
            .unwrap()
            .unwrap()
            .device_id,
        core.workspace.device_id()
    );

    // No GitHub origin, a repository Cloud doesn't reach, the edge refusing:
    // no row, a clear error.
    for (chat_id, space_id, message) in [
        ("chat-plain", "sp-plain", "no GitHub repository"),
        ("chat-other", "sp-other", "Cloud can't reach someone/else"),
        (
            "chat-fail",
            "sp-app",
            "Couldn't start a Cloud session: Cloud is down",
        ),
    ] {
        let failed = create(
            &client,
            serde_json::json!({
            "chatId": chat_id, "spaceId": space_id, "cloud": true }),
        )
        .await
        .unwrap_err();
        assert!(failed.to_string().contains(message), "{failed}");
        assert!(core.workspace.chat(chat_id).unwrap().is_none());
    }

    let sessions: Vec<serde_json::Value> = seen
        .lock()
        .unwrap()
        .iter()
        .filter(|(request, _)| request.starts_with("POST ") && request.ends_with("/sessions"))
        .map(|(_, body)| serde_json::from_str(body).unwrap())
        .collect();
    let summary: Vec<(&str, &str, &str, Option<&str>)> = sessions
        .iter()
        .map(|body| {
            (
                body["chatId"].as_str().unwrap(),
                body["spaceId"].as_str().unwrap(),
                body["repo"]["cloneUrl"].as_str().unwrap(),
                body["branch"].as_str(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            ("chat-1", "sp-app", "https://github.com/acme/app.git", None),
            (
                "chat-2",
                "sp-app",
                "https://github.com/acme/app.git",
                Some("dev")
            ),
            (
                "chat-fail",
                "sp-app",
                "https://github.com/acme/app.git",
                None
            ),
        ]
    );
    core.shutdown().await;
}

#[tokio::test]
async fn cloud_branches_are_github_branches_default_first() {
    let (edge_url, seen) = scripted_edge(cloud_edge).await;
    let dir = tempfile::tempdir().unwrap();
    let core = EngineCore::assemble(
        dir.path(),
        Arc::new(default_registry()),
        HarnessId::Mock,
        Some(EdgeConfig::with_static_token(edge_url, "dev-user")),
    )
    .unwrap();
    let client = memory_client(core.rpc_service());
    let refs: Vec<zeron_proto::RepoRef> = client
        .call_as(
            methods::LIST_CLOUD_BRANCHES,
            serde_json::json!({ "repo": "acme/app", "defaultBranch": "main" }),
        )
        .await
        .unwrap();
    let names: Vec<(&str, bool)> = refs.iter().map(|r| (r.name.as_str(), r.current)).collect();
    assert_eq!(
        names,
        [("main", true), ("dev", false), ("feature/x", false)]
    );
    assert!(
        seen.lock()
            .unwrap()
            .iter()
            .all(|(r, _)| !r.contains("/device/"))
    );
    core.shutdown().await;
}

#[tokio::test]
async fn catalogs_for_cloud_devices_are_answered_here() {
    let (edge_url, seen) = scripted_edge(cloud_edge).await;
    let dir = tempfile::tempdir().unwrap();
    let core = EngineCore::assemble(
        dir.path(),
        Arc::new(default_registry()),
        HarnessId::Mock,
        Some(EdgeConfig::with_static_token(edge_url, "dev-user")),
    )
    .unwrap();
    core.workspace.upsert_device_row(&zeron_proto::Device {
        id: "cloud-acct".into(),
        name: "Cloud".into(),
        platform: "cloud".into(),
        last_seen_at: None,
        created_at: None,
        version: None,
        cursor_sdk_version: None,
        capabilities: vec![zeron_proto::CLOUD_ACCOUNT_CAPABILITY.into()],
    });
    let client = memory_client(core.rpc_service());

    // Harnesses for any Cloud device: Codex and Claude Code, both usable
    // there, without dialing anything (no peer links exist here).
    for target in ["cloud-acct", "cloud-s-chat-1"] {
        let harnesses: Vec<serde_json::Value> = client
            .call_as(
                methods::LIST_HARNESSES,
                serde_json::json!({ "targetDeviceId": target }),
            )
            .await
            .unwrap();
        let ids: Vec<&str> = harnesses.iter().filter_map(|h| h["id"].as_str()).collect();
        assert_eq!(ids.len(), 2, "{harnesses:?}");
        assert!(
            ids.contains(&"codex") && ids.contains(&"claude-code"),
            "{ids:?}"
        );
        assert!(harnesses.iter().all(|h| h["installed"] == true));
    }
    let refused = client
        .call_as::<serde_json::Value>(
            methods::LIST_MODELS,
            serde_json::json!({ "targetDeviceId": "cloud-acct", "harness": "mock" }),
        )
        .await
        .unwrap_err();
    assert!(
        refused.to_string().contains("Codex and Claude Code only"),
        "{refused}"
    );

    // Models for a chat on a session machine (asleep, or gone): Cloud's, as
    // the edge lists them, whatever this computer has installed or signed in.
    // Nothing dials the machine.
    for (target, harness, id) in [
        ("cloud-s-chat-1", "codex", "cloud-codex"),
        ("cloud-s-chat-1", "claude-code", "cloud-opus"),
        ("cloud-acct", "claude-code", "cloud-opus"),
    ] {
        let models: Vec<serde_json::Value> = client
            .call_as(
                methods::LIST_MODELS,
                serde_json::json!({ "targetDeviceId": target, "harness": harness }),
            )
            .await
            .unwrap();
        let ids: Vec<&str> = models.iter().filter_map(|m| m["id"].as_str()).collect();
        assert_eq!(ids, [id], "{target} {harness}");
    }
    // The edge out of reach: the last list read.
    MODELS_UP.store(false, std::sync::atomic::Ordering::SeqCst);
    let kept: Vec<serde_json::Value> = client
        .call_as(
            methods::LIST_MODELS,
            serde_json::json!({ "targetDeviceId": "cloud-s-chat-1", "harness": "codex" }),
        )
        .await
        .unwrap();
    MODELS_UP.store(true, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(kept[0]["id"], "cloud-codex");
    assert!(
        seen.lock()
            .unwrap()
            .iter()
            .all(|(r, _)| !r.contains("cloud-s-chat-1")),
        "no request names the machine"
    );

    // The logical device has no engine: anything else fails fast.
    let folders = client
        .call_as::<serde_json::Value>(
            methods::LIST_FOLDERS,
            serde_json::json!({ "targetDeviceId": "cloud-acct", "path": "/" }),
        )
        .await
        .unwrap_err();
    assert!(folders.to_string().contains("isn't a machine"), "{folders}");
    assert!(
        seen.lock()
            .unwrap()
            .iter()
            .all(|(r, _)| !r.contains("/device/"))
    );
    core.shutdown().await;
}

/// Deleting a Cloud chat deletes its machine; deleting any other chat
/// touches no Cloud session.
#[tokio::test]
async fn deleting_a_cloud_chat_deletes_its_machine() {
    let (edge_url, seen) = mock_edge().await;
    let dir = tempfile::tempdir().unwrap();
    let core = EngineCore::assemble(
        dir.path(),
        Arc::new(default_registry()),
        HarnessId::Mock,
        Some(EdgeConfig::with_static_token(edge_url, "dev-user")),
    )
    .unwrap();
    let client = memory_client(core.rpc_service());
    let cwd = dir.path().display().to_string();
    for (chat, device) in [
        ("cloud-chat", "cloud-session-1".to_string()),
        ("mac-chat", core.workspace.device_id().to_string()),
    ] {
        core.workspace
            .create_chat(chat, None, Some(&device), None, Some(cwd.clone()))
            .unwrap();
    }
    let org = core.workspace.org_id().to_string();
    let deletes = |seen: &Seen| {
        seen.lock()
            .unwrap()
            .iter()
            .filter(|(request, _)| request.starts_with("DELETE "))
            .map(|(request, _)| request.clone())
            .collect::<Vec<_>>()
    };

    for chat in ["mac-chat", "cloud-chat"] {
        client
            .call_as::<serde_json::Value>(
                methods::MUTATE,
                serde_json::json!({ "op": "deleteChat", "chatId": chat }),
            )
            .await
            .unwrap();
    }
    let expected = format!("DELETE /cloud/{org}/sessions/cloud-chat");
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !deletes(&seen).contains(&expected) {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{expected} not called: {:?}", deletes(&seen)));
    assert_eq!(
        deletes(&seen),
        vec![expected],
        "only the Cloud chat's machine"
    );
    core.shutdown().await;
}

/// A chat hosted by an engine that can't switch provider (a Cloud machine
/// on an older build) keeps its provider: the switch is refused here rather
/// than handing the new provider the old one's session id.
#[tokio::test]
async fn provider_switch_is_refused_for_a_host_that_cannot_switch() {
    let dir = tempfile::tempdir().unwrap();
    let core = EngineCore::assemble(
        dir.path(),
        Arc::new(default_registry()),
        HarnessId::Mock,
        None,
    )
    .unwrap();
    let client = memory_client(core.rpc_service());
    let config =
        |harness: &str| serde_json::json!({ "harness": harness, "sandbox": "workspace-write" });
    let cwd = dir.path().display().to_string();
    for (chat, device) in [
        ("old-host-chat", "cloud-old-machine".to_string()),
        ("own-chat", core.workspace.device_id().to_string()),
    ] {
        core.workspace
            .create_chat(
                chat,
                None,
                Some(&device),
                Some(serde_json::from_value(config("claude-code")).unwrap()),
                Some(cwd.clone()),
            )
            .unwrap();
    }
    let switch = |chat: &str, harness: &str| {
        client.call_as::<serde_json::Value>(
            methods::MUTATE,
            serde_json::json!({ "op": "setChatConfig", "chatId": chat, "config": config(harness) }),
        )
    };
    let refused = switch("old-host-chat", "codex")
        .await
        .unwrap_err()
        .to_string();
    assert!(refused.contains("newer Zeron"), "{refused}");
    // The same provider (a model or effort change) still goes through.
    switch("old-host-chat", "claude-code").await.unwrap();
    // This engine hosts its own chats and switches them.
    switch("own-chat", "codex").await.unwrap();
    core.shutdown().await;
}

/// GitHub's logins through the engine: listed, one disconnected, one made the
/// first new sessions try; a repository names who reaches it; and an edge from
/// before several logins answers shapes these calls still read.
#[tokio::test]
async fn github_logins_are_listed_and_changed_one_at_a_time() {
    fn login(slot: &str, account: &str, active: bool) -> serde_json::Value {
        serde_json::json!({
            "slot": slot, "status": "connected", "active": active, "kind": "github",
            "account": account, "avatarUrl": format!("https://github.com/{account}.png?size=64"),
            "authorizedDevices": ["cloud-acct"], "createdAt": 1, "updatedAt": 2
        })
    }
    let (edge_url, seen) = scripted_edge(|request, _body| {
        if request.starts_with("GET ") && request.ends_with("/accounts/github") {
            let accounts = [login("awork", "work-me", false), login("ahome", "home-me", true)];
            return (200, serde_json::json!({ "accounts": accounts }).to_string());
        }
        if request.starts_with("DELETE ") && request.ends_with("/accounts/github/awork") {
            let accounts = [login("ahome", "home-me", true)];
            return (200, serde_json::json!({ "accounts": accounts }).to_string());
        }
        if request.starts_with("POST ") && request.ends_with("/accounts/github/active") {
            let accounts = [login("awork", "work-me", true), login("ahome", "home-me", false)];
            return (200, serde_json::json!({ "accounts": accounts }).to_string());
        }
        if request.starts_with("GET ") && request.contains("/github/repos") {
            let reply = serde_json::json!({
                "repos": [{
                    "fullName": "work-org/api", "cloneUrl": "https://github.com/work-org/api.git",
                    "defaultBranch": "main", "private": true, "pushedAt": 1,
                    "installationId": 100, "account": "work-me", "accounts": ["work-me", "home-me"]
                }, {
                    // As an old vault answers.
                    "fullName": "old/shape", "cloneUrl": "https://github.com/old/shape.git",
                    "defaultBranch": "main", "private": false, "pushedAt": 0
                }],
                "installations": [{ "id": 100, "target": "work-org", "reachedBy": ["work-me", "home-me"] }]
            });
            return (200, reply.to_string());
        }
        if request.starts_with("POST ") && request.contains("/github/device/flow1") {
            return (
                200,
                r#"{"state":"connected","account":"work-me","alreadyConnected":true}"#.into(),
            );
        }
        if request.starts_with("GET /cloud/") {
            return (200, r#"{"state":"ready","deviceId":"cloud-acct","available":true}"#.into());
        }
        (404, r#"{"error":"not_found","message":"no route"}"#.into())
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let core = EngineCore::assemble(
        dir.path(),
        Arc::new(default_registry()),
        HarnessId::Mock,
        Some(EdgeConfig::with_static_token(edge_url, "dev-user")),
    )
    .unwrap();
    let client = memory_client(core.rpc_service());

    let listed: serde_json::Value = client
        .call_as(methods::GITHUB_ACCOUNTS, serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(listed["accounts"][0]["account"], "work-me");
    assert_eq!(
        listed["accounts"][0]["avatarUrl"],
        "https://github.com/work-me.png?size=64"
    );
    assert_eq!(listed["accounts"][1]["active"], true);

    let left: serde_json::Value = client
        .call_as(
            methods::GITHUB_ACCOUNT_FORGET,
            serde_json::json!({ "slot": "awork" }),
        )
        .await
        .unwrap();
    assert_eq!(left["accounts"].as_array().unwrap().len(), 1);
    assert_eq!(left["accounts"][0]["account"], "home-me");

    let picked: serde_json::Value = client
        .call_as(
            methods::GITHUB_ACCOUNT_ACTIVATE,
            serde_json::json!({ "slot": "awork" }),
        )
        .await
        .unwrap();
    assert_eq!(picked["accounts"][0]["active"], true);

    // A slot goes into a path: nothing but an id gets there.
    for method in [
        methods::GITHUB_ACCOUNT_FORGET,
        methods::GITHUB_ACCOUNT_ACTIVATE,
    ] {
        let refused = client
            .call(
                method,
                serde_json::json!({ "slot": "../credentials/github" }),
            )
            .await;
        assert!(refused.is_err(), "{method}");
    }

    let repos: serde_json::Value = client
        .call_as(methods::LIST_GITHUB_REPOS, serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(repos["repos"][0]["account"], "work-me");
    assert_eq!(
        repos["repos"][0]["accounts"],
        serde_json::json!(["work-me", "home-me"])
    );
    assert_eq!(repos["repos"][0]["installationId"], 100);
    assert_eq!(repos["repos"][1]["fullName"], "old/shape");
    assert!(repos["repos"][1].get("account").is_none());

    let polled: serde_json::Value = client
        .call_as(
            methods::GITHUB_CONNECT_POLL,
            serde_json::json!({ "flowId": "flow1" }),
        )
        .await
        .unwrap();
    assert_eq!(polled["alreadyConnected"], true);

    let seen = seen.lock().unwrap().clone();
    let org = core.workspace.org_id().to_string();
    for wanted in [
        format!("GET /vault/{org}/accounts/github"),
        format!("DELETE /vault/{org}/accounts/github/awork"),
        format!("POST /vault/{org}/accounts/github/active"),
    ] {
        assert!(
            seen.iter().any(|(request, _)| *request == wanted),
            "{wanted}: {seen:?}"
        );
    }
    assert!(
        !seen
            .iter()
            .any(|(request, _)| request.contains("credentials/github"))
    );
    core.shutdown().await;
}

/// A message that reached a Cloud chat's doc and then waited — its machine
/// couldn't be woken — for longer than its clock gives it still goes once
/// the person wakes the machine: it goes again, once, with its clock started
/// now. One still within its time is left to go as it is.
#[tokio::test]
async fn a_message_that_waited_days_for_its_machine_goes_once_when_woken() {
    use zeron_doc::{SessionCommandEntry, SessionCommandPayload, SessionCommandStatus};
    let (edge_url, seen) = scripted_edge(|request, _| {
        if request.contains("/wake") {
            return (
                200,
                r#"{"chatId":"cloud-chat","deviceId":"cloud-s1","spaceId":"sp","state":"starting","createdAt":1,"machine":"xlarge"}"#
                    .to_string(),
            );
        }
        (404, r#"{"error":"not_found"}"#.to_string())
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let core = EngineCore::assemble(
        dir.path(),
        Arc::new(default_registry()),
        HarnessId::Mock,
        Some(EdgeConfig::with_static_token(edge_url, "dev-user")),
    )
    .unwrap();
    let client = memory_client(core.rpc_service());
    let cwd = dir.path().display().to_string();
    core.workspace
        .create_chat("cloud-chat", None, Some("cloud-session-1"), None, Some(cwd))
        .unwrap();
    let handle = core.doc_host.open("cloud-chat").unwrap();
    const DAY: i64 = 24 * 60 * 60 * 1000;
    let now = chrono::Utc::now().timestamp_millis();
    let run = |prompt: &str, message: &str| SessionCommandPayload::Run {
        request: zeron_proto::RunRequest {
            mcp: None,
            prompt: prompt.into(),
            harness: None,
            model: None,
            reasoning: None,
            model_options: Default::default(),
            cwd: "/tmp".into(),
            sandbox: zeron_proto::SandboxLevel::WorkspaceWrite,
            auto_approve: true,
            access: Default::default(),
            plan: false,
            attachments: Vec::new(),
            worktree: None,
            resume: None,
            env: Default::default(),
            extra_mcp: Vec::new(),
        },
        message_id: message.into(),
    };
    for (id, message, issued_at) in [
        ("cmd-ten-days", "m-ten-days", now - 10 * DAY),
        // Its clock runs out while the machine boots.
        ("cmd-nearly", "m-nearly", now - DAY + 10 * 60 * 1000),
        ("cmd-an-hour", "m-an-hour", now - 60 * 60 * 1000),
    ] {
        handle
            .doc()
            .queue_command(&SessionCommandEntry {
                id: id.into(),
                payload: run("Fix the flaky test", message),
                issued_by: "phone".into(),
                issued_at,
                based_on: None,
                expires_at: Some(issued_at + DAY),
                status: SessionCommandStatus::Pending,
                resolution: None,
            })
            .unwrap();
    }
    let attempts = |message: &str| -> Vec<SessionCommandEntry> {
        handle
            .doc()
            .read_commands()
            .unwrap()
            .into_iter()
            .filter(|c| {
                matches!(&c.payload,
                    SessionCommandPayload::Run { message_id, .. } if message_id == message)
            })
            .collect()
    };
    let wake = || async {
        let _: zeron_proto::CloudSession = client
            .call_as(
                methods::CLOUD_SESSION_WAKE,
                serde_json::json!({ "chatId": "cloud-chat", "machine": "xlarge" }),
            )
            .await
            .unwrap();
    };

    wake().await;
    let old = attempts("m-ten-days");
    assert_eq!(old.len(), 2, "the original and one re-issue: {old:?}");
    assert_eq!(old[0].status, SessionCommandStatus::Rejected);
    let why = old[0].resolution.clone().unwrap_or_default();
    assert!(why.contains("ran out"), "{why}");
    let again = &old[1];
    assert_eq!(again.payload, old[0].payload, "the same message, as it was");
    assert!(again.issued_at >= now, "its clock starts at the wake");
    assert!(
        again.effective_expiry() > chrono::Utc::now().timestamp_millis() + DAY / 2,
        "a full clock from now: {again:?}"
    );
    assert_eq!(attempts("m-nearly").len(), 2, "one about to run out goes again too");
    assert_eq!(
        attempts("m-an-hour").len(),
        1,
        "one still within its time goes as it is"
    );

    // Woken again: nothing sends twice.
    wake().await;
    assert_eq!(attempts("m-ten-days").len(), 2);
    assert_eq!(attempts("m-nearly").len(), 2);
    assert_eq!(attempts("m-an-hour").len(), 1);
    let wakes = seen
        .lock()
        .unwrap()
        .iter()
        .filter(|(request, _)| request.ends_with("/sessions/cloud-chat/wake"))
        .count();
    assert_eq!(wakes, 2, "both wakes reached the edge");
    core.shutdown().await;
}
