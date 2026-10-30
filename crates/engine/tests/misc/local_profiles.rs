//! Integrated regressions for local-first profile privacy and lifecycle boundaries.

use std::path::Path;
use std::sync::{Arc, Barrier, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use zeron_engine::{
    AuthState, Engine, EngineConfig, EngineCore, EngineProfile, HarnessId, WorkspaceScope,
    default_registry,
};

fn config(
    data_dir: &Path,
    edge_url: String,
    workos_client_id: Option<&str>,
    edge_token: Option<&str>,
) -> EngineConfig {
    EngineConfig {
        data_dir: data_dir.to_path_buf(),
        edge_url,
        edge_token: edge_token.map(str::to_string),
        ipc_port: 0,
        default_harness: HarnessId::Mock,
        org_id: None,
        workos_client_id: workos_client_id.map(str::to_string),
    }
}

fn assemble(profile: EngineProfile) -> EngineCore {
    EngineCore::assemble_with_profile(profile, Arc::new(default_registry()), HarnessId::Mock, None)
        .expect("assemble profile")
}

async fn shutdown(core: EngineCore) {
    core.shutdown().await;
    drop(core);
}

fn concurrent_engine_info(
    config: Arc<EngineConfig>,
    workers: usize,
) -> std::collections::HashSet<String> {
    let barrier = Arc::new(Barrier::new(workers));
    (0..workers)
        .map(|_| {
            let config = config.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                Engine::engine_info(&config, WorkspaceScope::Local)
                    .expect("resolve concurrent engine info")
                    .device_id
            })
        })
        .collect::<Vec<_>>()
        .into_iter()
        .map(|call| call.join().expect("engine-info worker"))
        .collect()
}

#[tokio::test]
async fn concurrent_engine_info_and_runtime_share_one_device_identity() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = Arc::new(config(
        dir.path(),
        "http://127.0.0.1:1".into(),
        Some("client_test"),
        None,
    ));
    let announced = concurrent_engine_info(config, 32);

    assert_eq!(announced.len(), 1, "every viewport announces one identity");
    let announced = announced.into_iter().next().expect("announced id");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("device-id"))
            .expect("persisted device id")
            .trim(),
        announced
    );

    let core = assemble(EngineProfile::local(dir.path()).expect("local profile"));
    assert_eq!(
        core.device_id, announced,
        "the assembled runtime must use the identity already announced"
    );
    shutdown(core).await;
}

#[tokio::test]
async fn empty_legacy_device_identity_is_repaired_once_for_all_boots() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("device-id"), b"").expect("seed truncated identity");
    let config = Arc::new(config(
        dir.path(),
        "http://127.0.0.1:1".into(),
        Some("client_test"),
        None,
    ));

    let announced = concurrent_engine_info(config, 32);
    assert_eq!(
        announced.len(),
        1,
        "legacy repair must publish one identity"
    );
    let announced = announced.into_iter().next().expect("repaired id");
    assert!(!announced.trim().is_empty());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("device-id"))
            .expect("repaired device id")
            .trim(),
        announced
    );

    let core = assemble(EngineProfile::local(dir.path()).expect("local profile"));
    assert_eq!(core.device_id, announced);
    shutdown(core).await;
}

#[tokio::test]
async fn local_and_synced_profiles_remain_isolated_across_restarts() {
    let dir = tempfile::tempdir().expect("tempdir");
    let local_profile = EngineProfile::local(dir.path()).expect("local profile");
    let local_user_id = local_profile.user_id().to_string();
    let local_profile_file = dir.path().join("local-profile.json");
    let local_profile_bytes = std::fs::read(&local_profile_file).expect("local profile file");

    let local_upload = {
        let core = assemble(local_profile.clone());
        let device_id = core.device_id.clone();
        core.workspace
            .create_space(
                "local-space",
                &device_id,
                "/private/local-project",
                Some("Private project".into()),
                false,
            )
            .expect("create local space");
        core.workspace
            .create_chat("local-chat", Some("local-space"), None, None, None)
            .expect("create local chat");
        core.workspace
            .rename_chat("local-chat", "Private local session")
            .expect("name local chat");
        core.doc_host
            .open("local-chat")
            .expect("open local chat doc")
            .write_user_message("local-message", "Private local transcript", 1)
            .expect("write local transcript");
        core.uploads
            .append("local-upload", "cHJpdmF0ZQ==", Some(0))
            .expect("stage local upload");
        let upload = core
            .uploads
            .commit("local-upload", "private.png")
            .expect("commit local upload");
        assert!(Path::new(&upload).starts_with(local_profile.uploads_root()));
        shutdown(core).await;
        (device_id, upload)
    };

    let synced_profile = EngineProfile::synced(dir.path(), "cloud-org", "cloud-user");
    {
        let core = assemble(synced_profile.clone());
        assert_eq!(core.device_id, local_upload.0, "device identity is global");
        assert!(
            core.workspace
                .chat("local-chat")
                .expect("read synced chats")
                .is_none(),
            "the synced profile must not expose local chats"
        );
        assert_eq!(core.uploads.dir(), synced_profile.uploads_root());
        let error = core
            .uploads
            .read_chunk(&local_upload.1, 0, &[])
            .expect_err("synced upload jail must reject a local-profile path");
        assert!(
            error.to_string().contains("outside the upload cache"),
            "unexpected jail error: {error}"
        );

        core.workspace
            .create_space(
                "synced-space",
                &core.device_id,
                "/shared/cloud-project",
                None,
                false,
            )
            .expect("create synced space");
        core.workspace
            .create_chat("synced-chat", Some("synced-space"), None, None, None)
            .expect("create synced chat");
        shutdown(core).await;
    }

    let reopened_profile = EngineProfile::local(dir.path()).expect("reopen local profile");
    assert_eq!(reopened_profile.user_id(), local_user_id);
    assert_eq!(
        std::fs::read(&local_profile_file).expect("re-read local profile"),
        local_profile_bytes,
        "reopening local must not rotate or rewrite its identity"
    );
    {
        let core = assemble(reopened_profile);
        assert_eq!(core.device_id, local_upload.0);
        let chat = core
            .workspace
            .chat("local-chat")
            .expect("read local chat")
            .expect("local chat survived restart");
        assert_eq!(chat.title.as_deref(), Some("Private local session"));
        assert_eq!(chat.cwd.as_deref(), Some("/private/local-project"));
        let transcript = core
            .doc_host
            .open("local-chat")
            .expect("reopen local chat doc")
            .doc()
            .read_entries()
            .expect("read local transcript");
        assert_eq!(transcript.len(), 1);
        assert_eq!(transcript[0].id, "local-message");
        assert!(
            core.workspace
                .chat("synced-chat")
                .expect("read local chats")
                .is_none(),
            "the local profile must not expose synced chats"
        );
        assert_eq!(
            core.uploads
                .read_chunk(&local_upload.1, 0, &[])
                .expect("local profile can read its upload")
                .data,
            "cHJpdmF0ZQ=="
        );
        shutdown(core).await;
    }
}

#[tokio::test]
async fn synced_accounts_isolate_uploads_and_assign_the_legacy_cache_once() {
    let dir = tempfile::tempdir().expect("tempdir");
    let legacy_root = dir.path().join("uploads");
    std::fs::create_dir(&legacy_root).expect("legacy uploads root");
    let legacy_upload = legacy_root.join("legacy.png");
    std::fs::write(&legacy_upload, b"legacy").expect("legacy attachment");

    let first_profile = EngineProfile::synced(dir.path(), "org-a", "user-a");
    let first_upload = {
        let core = assemble(first_profile.clone());
        assert_eq!(core.uploads.dir(), first_profile.uploads_root());
        assert_eq!(
            core.uploads
                .read_chunk(legacy_upload.to_str().unwrap(), 0, &[])
                .expect("legacy owner can read the compatibility cache")
                .data,
            "bGVnYWN5"
        );
        core.uploads
            .append("shared-upload", "YWNjb3VudC1h", Some(0))
            .expect("stage first account upload");
        let upload = core
            .uploads
            .commit("shared-upload", "image.png")
            .expect("commit first account upload");
        assert!(Path::new(&upload).starts_with(first_profile.uploads_root()));
        shutdown(core).await;
        upload
    };

    let second_profile = EngineProfile::synced(dir.path(), "org-b", "user-b");
    let second_upload = {
        let core = assemble(second_profile.clone());
        assert_eq!(core.uploads.dir(), second_profile.uploads_root());
        assert_ne!(core.uploads.dir(), first_profile.uploads_root());
        for path in [legacy_upload.to_str().unwrap(), &first_upload] {
            let error = core
                .uploads
                .read_chunk(path, 0, &[])
                .expect_err("another account must not read the attachment");
            assert!(error.to_string().contains("outside the upload cache"));
        }
        core.uploads
            .append("shared-upload", "YWNjb3VudC1i", Some(0))
            .expect("stage second account upload with the same id");
        let upload = core
            .uploads
            .commit("shared-upload", "image.png")
            .expect("commit second account upload with the same name");
        assert!(Path::new(&upload).starts_with(second_profile.uploads_root()));
        assert_ne!(upload, first_upload);
        shutdown(core).await;
        upload
    };

    let core = assemble(first_profile);
    assert_eq!(
        core.uploads
            .read_chunk(&first_upload, 0, &[])
            .expect("first account can reopen its scoped upload")
            .data,
        "YWNjb3VudC1h"
    );
    assert_eq!(
        core.uploads
            .read_chunk(legacy_upload.to_str().unwrap(), 0, &[])
            .expect("legacy ownership survives account switches")
            .data,
        "bGVnYWN5"
    );
    let error = core
        .uploads
        .read_chunk(&second_upload, 0, &[])
        .expect_err("first account must not read the second account upload");
    assert!(error.to_string().contains("outside the upload cache"));
    shutdown(core).await;
}

#[tokio::test]
async fn existing_session_opens_the_historical_cloud_layout_in_place() {
    let dir = tempfile::tempdir().expect("tempdir");
    let historical = EngineProfile::synced(dir.path(), "legacy-org", "legacy-user");
    {
        let core = assemble(historical.clone());
        core.workspace
            .create_space(
                "legacy-space",
                &core.device_id,
                "/shared/legacy-project",
                None,
                false,
            )
            .expect("create historical space");
        core.workspace
            .create_chat("legacy-chat", Some("legacy-space"), None, None, None)
            .expect("create historical chat");
        shutdown(core).await;
    }
    std::fs::write(
        dir.path().join("session.json"),
        r#"{"refreshToken":"refresh-1","user":{"id":"legacy-user","email":"legacy@example.com"},"orgId":"legacy-org"}"#,
    )
    .expect("seed existing session");

    let config = config(
        dir.path(),
        "http://127.0.0.1:1".into(),
        Some("client_test"),
        None,
    );
    let auth = Engine::build_auth(&config).await;
    let scope = Engine::initial_workspace_scope(&auth);
    let resolved = Engine::resolve_profile(&config, &auth, scope)
        .expect("resolve existing session")
        .expect("existing org is ready");

    assert_eq!(scope, WorkspaceScope::Synced);
    assert_eq!(resolved, historical);
    assert_eq!(
        resolved.store_root(),
        dir.path().join("orgs/legacy-org/legacy-user")
    );
    assert!(!dir.path().join("profiles/synced").exists());
    {
        let core = assemble(resolved);
        assert!(
            core.workspace
                .chat("legacy-chat")
                .expect("read historical layout")
                .is_some(),
            "the existing cloud store must open without migration"
        );
        shutdown(core).await;
    }
    assert!(!dir.path().join("profiles/synced").exists());
}

struct RecordingEdge {
    url: String,
    requests: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for RecordingEdge {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl RecordingEdge {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind recording edge");
        let url = format!("http://{}", listener.local_addr().expect("edge addr"));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = requests.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let seen = seen.clone();
                tokio::spawn(async move { handle_edge_request(stream, seen).await });
            }
        });
        Self {
            url,
            requests,
            task,
        }
    }
}

async fn handle_edge_request(mut stream: tokio::net::TcpStream, requests: Arc<Mutex<Vec<String>>>) {
    let Some((target, _body)) = read_request(&mut stream).await else {
        return;
    };
    requests.lock().expect("requests lock").push(target.clone());
    let path = target.split('?').next().unwrap_or("");
    let (status, body) = if path == "/auth/exchange" {
        (
            "200 OK",
            serde_json::json!({
                "user": { "id": "cloud-user", "email": "cloud@example.com" },
                "accessToken": fake_jwt("cloud-org"),
                "refreshToken": "refresh-1",
            })
            .to_string(),
        )
    } else {
        ("404 Not Found", r#"{"error":"unexpected_request"}"#.into())
    };
    let response = format!(
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

async fn read_request(stream: &mut tokio::net::TcpStream) -> Option<(String, String)> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 1024];
    let header_end = loop {
        if let Some(position) = buffer.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
            break position + 4;
        }
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
    };
    let headers = String::from_utf8_lossy(&buffer[..header_end]);
    let mut lines = headers.lines();
    let target = lines.next()?.split_whitespace().nth(1)?.to_string();
    let content_length = lines
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = buffer[header_end..].to_vec();
    while body.len() < content_length {
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..read]);
    }
    Some((target, String::from_utf8_lossy(&body).into_owned()))
}

fn fake_jwt(org_id: &str) -> String {
    let claims = serde_json::json!({
        "iat": 1_000,
        "exp": 4_600,
        "org_id": org_id,
    });
    format!("e30.{}.sig", base64url(claims.to_string().as_bytes()))
}

fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut output = String::new();
    for chunk in bytes.chunks(3) {
        let bytes = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let value = (u32::from(bytes[0]) << 16) | (u32::from(bytes[1]) << 8) | u32::from(bytes[2]);
        output.push(ALPHABET[(value >> 18) as usize & 63] as char);
        output.push(ALPHABET[(value >> 12) as usize & 63] as char);
        if chunk.len() > 1 {
            output.push(ALPHABET[(value >> 6) as usize & 63] as char);
        }
        if chunk.len() > 2 {
            output.push(ALPHABET[value as usize & 63] as char);
        }
    }
    output
}

fn query_param(url: &str, key: &str) -> Option<String> {
    url.split_once('?')?
        .1
        .split('&')
        .filter_map(|part| part.split_once('='))
        .find(|(name, _)| *name == key)
        .map(|(_, value)| value.to_string())
}

#[tokio::test]
async fn signing_in_does_not_activate_sync_for_the_running_local_profile() {
    let edge = RecordingEdge::start().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let config = config(dir.path(), edge.url.clone(), Some("client_test"), None);
    let auth = Engine::build_auth(&config).await;
    let scope = Engine::initial_workspace_scope(&auth);
    let profile = Engine::resolve_profile(&config, &auth, scope)
        .expect("resolve local profile")
        .expect("local profile is ready");
    let runtime = Engine::assemble_runtime(&config, auth.clone(), profile)
        .await
        .expect("assemble local runtime");

    assert_eq!(scope, WorkspaceScope::Local);
    assert!(runtime.core().links().is_none());

    let sign_in_url = auth.start_headless_sign_in();
    let state = query_param(&sign_in_url, "state").expect("sign-in state");
    auth.complete_sign_in(&format!("{state}.authorization-code"))
        .await
        .expect("complete sign in");
    assert!(
        matches!(auth.state(), AuthState::SignedIn { org_id: Some(org), .. } if org == "cloud-org")
    );

    runtime
        .core()
        .workspace
        .create_space(
            "still-local-space",
            &runtime.core().device_id,
            "/private/still-local",
            None,
            false,
        )
        .expect("create space after sign in");
    runtime
        .core()
        .workspace
        .create_chat(
            "still-local-chat",
            Some("still-local-space"),
            None,
            None,
            None,
        )
        .expect("create chat after sign in");
    runtime
        .core()
        .doc_host
        .open("still-local-chat")
        .expect("open local chat doc")
        .write_user_message("message-1", "This stays local", 1)
        .expect("write local message");
    runtime
        .core()
        .uploads
        .append("still-local-upload", "cHJpdmF0ZQ==", Some(0))
        .expect("stage local upload after sign in");
    let upload = runtime
        .core()
        .uploads
        .commit("still-local-upload", "private.png")
        .expect("commit local upload after sign in");

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(runtime.workspace_scope(), WorkspaceScope::Local);
    assert!(runtime.core().links().is_none());
    assert!(Path::new(&upload).starts_with(dir.path().join("profiles/local/uploads")));
    let requests = edge.requests.lock().expect("requests lock").clone();
    assert_eq!(
        requests
            .iter()
            .filter(|target| target.starts_with("/auth/exchange"))
            .count(),
        1,
        "the sign-in exchange itself is expected: {requests:?}"
    );
    assert!(
        requests.iter().all(|target| {
            !["/registry/", "/session/", "/device/", "/attachments/"]
                .iter()
                .any(|prefix| target.starts_with(prefix))
        }),
        "the captured local runtime attempted an online write: {requests:?}"
    );

    runtime.shutdown().await;
}
