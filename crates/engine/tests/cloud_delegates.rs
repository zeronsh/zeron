//! A Cloud machine starts delegates of the chat it runs: `createChat { cloud }`
//! from the machine asks the edge for a child session (no repository of its
//! own — the edge uses the parent's) and waits for the edge-written row.
//! Its own test binary: the machine's session is process-global.

use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use zeron_engine::{EdgeConfig, EngineCore, HarnessId, default_registry};
use zeron_rpc::{memory_client, methods};

type Requests = Arc<Mutex<Vec<(String, String)>>>;

/// The edge's delegate route, enough for the engine.
async fn edge() -> (String, Requests) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let seen: Requests = Arc::default();
    let log = seen.clone();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let log = log.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = vec![0u8; 16 * 1024];
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
                let json: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
                let (status, reply) =
                    if request.starts_with("POST ") && request.ends_with("/sessions") {
                        let chat_id = json["chatId"].as_str().unwrap_or_default();
                        (
                            200,
                            serde_json::json!({
                                "chatId": chat_id, "deviceId": "cloud-child", "spaceId": "sp-app",
                                "repo": "acme/app", "path": "/home/user/app",
                                "state": "provisioning", "createdAt": 1
                            })
                            .to_string(),
                        )
                    } else {
                        (
                            404,
                            r#"{"error":"not_found","message":"no route"}"#.to_string(),
                        )
                    };
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

#[tokio::test]
async fn a_cloud_machine_starts_delegates_of_its_own_chat() {
    zeron_engine::runner::set_session(Some(zeron_engine::runner::CloudSession {
        account: "cloud-acct".into(),
        chat_id: "main".into(),
        repo: Some("acme/app".into()),
        path: Some("/home/user/app".into()),
        branch: None,
    }));
    let (edge_url, seen) = edge().await;
    let dir = tempfile::tempdir().unwrap();
    let core = EngineCore::assemble(
        dir.path(),
        Arc::new(default_registry()),
        HarnessId::Mock,
        Some(EdgeConfig::with_static_token(edge_url, "dev-user")),
    )
    .unwrap();
    let client = memory_client(core.rpc_service());

    // The edge writes the delegate's row; here it arrives as registry sync
    // would, a moment after the session starts.
    let workspace = core.workspace.clone();
    let landed = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        workspace
            .create_chat_hosted(
                "child",
                None,
                Some("cloud-child"),
                Some("cloud-child"),
                None,
                Some("/home/user/app".into()),
                Some("main".into()),
            )
            .unwrap();
    });
    let created: serde_json::Value = client
        .call_as(
            methods::MUTATE,
            serde_json::json!({
                "op": "createChat", "chatId": "child", "parentChatId": "main", "cloud": true,
                "config": { "harness": "codex", "sandbox": "workspace-write" },
            }),
        )
        .await
        .unwrap();
    landed.await.unwrap();
    assert_eq!(created["deviceId"], "cloud-child");
    assert_eq!(created["cwd"], "/home/user/app");
    let child = core.workspace.chat("child").unwrap().unwrap();
    assert_eq!(child.device_id, "cloud-child");
    assert_eq!(child.parent_chat_id.as_deref(), Some("main"));

    // The request named the parent and the agent — never a repository.
    let (request, body) = seen
        .lock()
        .unwrap()
        .iter()
        .find(|(r, _)| r.starts_with("POST ") && r.ends_with("/sessions"))
        .cloned()
        .unwrap();
    assert!(request.contains("/cloud/"), "{request}");
    let body: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(body["chatId"], "child");
    assert_eq!(body["parentChatId"], "main");
    assert_eq!(body["config"]["harness"], "codex");
    assert!(
        body.get("repo").is_none() && body.get("spaceId").is_none(),
        "{body}"
    );

    // Only for the chat this machine runs.
    let refused = client
        .call_as::<serde_json::Value>(
            methods::MUTATE,
            serde_json::json!({
                "op": "createChat", "chatId": "stray", "parentChatId": "someone-else", "cloud": true,
            }),
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(refused.contains("the chat it runs"), "{refused}");
    core.shutdown().await;
}
