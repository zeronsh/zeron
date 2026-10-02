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
                    ("200 OK", r#"{"state":"ready","deviceId":"cloud-abc"}"#)
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
fn cloud_edge(request: &str, body: &str) -> (u16, String) {
    let json: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
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
            r#"{"state":"ready","deviceId":"cloud-acct","awakeSessions":0,"maxAwakeSessions":5}"#
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
