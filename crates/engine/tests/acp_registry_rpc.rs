//! The ACP registry through the engine's RPCs, on an isolated engine: a
//! registry served on the loopback lists a build of the fake ACP agent;
//! `ListAcpRegistry` lists it as this device sees it (its build, checksum,
//! icon, the agents Zeron drives itself marked built in); `AddAcpAgent`
//! installs it and it becomes a harness, which `ListHarnesses` shows only to
//! a client that asks for registry agents, with its icon, and whose models
//! come from its own handshake; `RemoveAcpAgent` takes it away again.

#![cfg(unix)]

use std::io::Write;
use std::path::PathBuf;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use zeron_engine::EngineCore;
use zeron_proto::HarnessId;
use zeron_rpc::methods;

fn fake_agent() -> Vec<u8> {
    let script = std::fs::read(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../harness/tests/fixtures/fake-acp.sh")).unwrap();
    let mut tar = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_gnu();
    header.set_size(script.len() as u64);
    header.set_mode(0o755);
    header.set_cksum();
    tar.append_data(&mut header, "agent", &script[..]).unwrap();
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gz.write_all(&tar.into_inner().unwrap()).unwrap();
    gz.finish().unwrap()
}

async fn serve(files: Vec<(String, Vec<u8>)>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    // (The registry's URLs name this port: written once it's known.)
    let files: Vec<(String, Vec<u8>)> = files
        .into_iter()
        .map(|(p, b)| (p, String::from_utf8(b.clone()).map(|t| t.replace("PORT", &port.to_string()).into_bytes()).unwrap_or(b)))
        .collect();
    unsafe {
        std::env::set_var("ZERON_ACP_REGISTRY_URL", format!("http://127.0.0.1:{port}/registry.json"));
    }
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else { return };
            let files = files.clone();
            tokio::spawn(async move {
                let mut buf = vec![0; 4096];
                let n = socket.read(&mut buf).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]);
                let path = request.split_whitespace().nth(1).unwrap_or("/").to_owned();
                let reply = match files.iter().find(|(p, _)| *p == path) {
                    Some((_, body)) => {
                        let mut r = format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n", body.len()).into_bytes();
                        r.extend_from_slice(body);
                        r
                    }
                    None => b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_vec(),
                };
                let _ = socket.write_all(&reply).await;
            });
        }
    });
}

fn find<'a>(list: &'a Value, key: &str, id: &str) -> Option<&'a Value> {
    list.as_array()?.iter().find(|e| e[key] == id)
}

#[tokio::test]
async fn registry_agents_are_listed_added_as_harnesses_and_removed() {
    let dir = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("ZERON_ACP_REGISTRY_LOOPBACK", "1");
        std::env::set_var("ZERON_AGENTS_DIR", dir.path().join("agents"));
    }
    let build = fake_agent();
    let sha = format!("{:x}", Sha256::digest(&build));
    let platform = zeron_harness::acp_registry::platform().unwrap();
    let index = json!({
        "version": "1.0.0",
        "agents": [
            { "id": "fake-agent", "name": "Fake Agent", "version": "2.0.0", "description": "Answers in scripts.",
              "authors": ["Zeron tests <tests@zeron.sh>"], "license": "MIT", "repository": "https://github.com/zeronsh/fake-agent",
              "icon": "http://127.0.0.1:PORT/fake-agent.svg",
              "distribution": { "binary": { platform: { "archive": "http://127.0.0.1:PORT/fake.tar.gz", "cmd": "./agent", "sha256": sha } } } },
            { "id": "claude-acp", "name": "Claude Agent", "version": "0.99.1", "description": "Built in.",
              "distribution": { "npx": { "package": "@agentclientprotocol/claude-agent-acp@0.99.1" } } }
        ]
    });
    // The registry an hour later: a newer build of the same agent.
    let mut newer = index.clone();
    newer["agents"][0]["version"] = json!("2.1.0");
    serve(vec![
        ("/registry.json".into(), index.to_string().into_bytes()),
        ("/registry-newer.json".into(), newer.to_string().into_bytes()),
        ("/fake.tar.gz".into(), build),
        ("/fake-agent.svg".into(), b"<svg viewBox=\"0 0 16 16\"><path fill=\"currentColor\" d=\"M0 0h16v16H0z\"/></svg>".to_vec()),
    ])
    .await;

    let core = EngineCore::assemble(dir.path(), zeron_engine::registry::default_registry().into(), HarnessId::Mock, None)
        .expect("an isolated engine");
    zeron_engine::acp_registry::start(dir.path(), &core.registry);
    let client = zeron_rpc::memory_client(core.rpc_service());

    let listed = client.call(methods::LIST_ACP_REGISTRY, json!({ "refresh": true })).await.unwrap();
    assert_eq!(listed["offline"], false, "{listed}");
    let fake = find(&listed["agents"], "id", "fake-agent").expect("listed");
    assert_eq!(fake["kind"], "binary");
    assert_eq!(fake["checksum"], true);
    assert_eq!(fake["readiness"], "ready");
    assert_eq!(fake["host"], "127.0.0.1");
    assert!(fake["icon"].as_str().is_some_and(|i| i.contains("<svg")), "{fake}");
    assert!(fake.get("added").is_none());
    assert_eq!(find(&listed["agents"], "id", "claude-acp").unwrap()["builtIn"], "claude-code");
    // Zeron's own can't be added again.
    assert!(client.call(methods::ADD_ACP_AGENT, json!({ "id": "claude-acp" })).await.is_err());

    let added = client.call(methods::ADD_ACP_AGENT, json!({ "id": "fake-agent" })).await.unwrap();
    assert_eq!(added["agent"]["added"], "2.0.0", "{added}");
    let harness = find(&added["harnesses"], "id", "acp.fake-agent").expect("a harness now");
    assert_eq!(harness["installed"], true);
    assert_eq!(harness["enabled"], true);
    assert!(harness["icon"].as_str().is_some());

    // Only a client that asks sees it among the harnesses.
    let old = client.call(methods::LIST_HARNESSES, json!({})).await.unwrap();
    assert!(find(&old, "id", "acp.fake-agent").is_none());
    let new = client.call(methods::LIST_HARNESSES, json!({ "includeRegistry": true })).await.unwrap();
    assert!(find(&new, "id", "acp.fake-agent").is_some());
    // Its models are its own, from its handshake.
    let models = client
        .call(methods::LIST_MODELS, json!({ "harness": "acp.fake-agent" }))
        .await
        .expect("models");
    let ids: Vec<&str> = models
        .as_array()
        .or_else(|| models["models"].as_array())
        .into_iter()
        .flatten()
        .filter_map(|m| m["id"].as_str())
        .collect();
    assert_eq!(ids, ["grok-4-fast", "grok-4.5"], "{models}");
    // Its icon stays with it once it has run (its slot resolved since).
    let listed = client.call(methods::LIST_HARNESSES, json!({ "includeRegistry": true })).await.unwrap();
    let after = find(&listed, "id", "acp.fake-agent").expect("still listed");
    assert!(after["icon"].as_str().is_some_and(|i| i.contains("<svg")), "the icon after a run: {after}");

    // Its updates: the update coordinator's, as any agent's, the registry's
    // version the latest.
    let statuses = client
        .call(methods::CHECK_HARNESS_UPDATES, json!({ "harness": "acp.fake-agent" }))
        .await
        .unwrap();
    let status = find(&statuses, "harness", "acp.fake-agent").expect("tracked");
    assert_eq!(status["installedVersion"], "2.0.0", "{status}");
    assert_eq!(status["latestVersion"], "2.0.0");
    assert_eq!(status["phase"], "current");
    assert_eq!(status["source"], "managed-by-zeron");
    assert_eq!(status["canApply"], true);
    let url = std::env::var("ZERON_ACP_REGISTRY_URL").unwrap();
    unsafe { std::env::set_var("ZERON_ACP_REGISTRY_URL", url.replace("registry.json", "registry-newer.json")) };
    client.call(methods::LIST_ACP_REGISTRY, json!({ "refresh": true })).await.unwrap();
    let statuses = client
        .call(methods::CHECK_HARNESS_UPDATES, json!({ "harness": "acp.fake-agent" }))
        .await
        .unwrap();
    let status = find(&statuses, "harness", "acp.fake-agent").unwrap();
    assert_eq!(status["latestVersion"], "2.1.0", "{status}");
    assert_eq!(status["phase"], "available");
    // Auto when idle, as any agent's policy.
    client
        .call(methods::SET_HARNESS_UPDATE_POLICY, json!({ "harness": "acp.fake-agent", "policy": "auto-when-idle" }))
        .await
        .expect("a policy for a registry agent");
    // …which takes it on its own, no chat running on it.
    let agent0 = zeron_proto::RegistryAgent::new("fake-agent").unwrap();
    let mut version = String::new();
    for _ in 0..100 {
        version = zeron_harness::acp_registry::record(agent0).unwrap().version;
        if version == "2.1.0" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(version, "2.1.0", "updated automatically");
    let statuses = client.call(methods::CHECK_HARNESS_UPDATES, json!({ "harness": "acp.fake-agent" })).await.unwrap();
    let status = find(&statuses, "harness", "acp.fake-agent").unwrap();
    assert_eq!(status["installedVersion"], "2.1.0", "{status}");
    assert_eq!(status["phase"], "current");
    assert!(dir.path().join("agents/fake-agent/2.1.0").is_dir());

    // Its environment: kept on this device, owner-only, handed to its launch.
    assert!(client
        .call(methods::SET_ACP_AGENT_ENV, json!({ "id": "fake-agent", "env": "not a pair" }))
        .await
        .is_err());
    let set = client
        .call(methods::SET_ACP_AGENT_ENV, json!({ "id": "fake-agent", "env": "FAKE_KEY=sk-1\n# note\nexport MODEL=\"m 1\"\n" }))
        .await
        .unwrap();
    assert_eq!(set["env"], "FAKE_KEY=sk-1\nMODEL=m 1\n");
    let got = client.call(methods::GET_ACP_AGENT_ENV, json!({ "id": "fake-agent" })).await.unwrap();
    assert_eq!(got["env"], set["env"]);
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.path().join("agents/fake-agent/env")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    let agent = zeron_proto::RegistryAgent::new("fake-agent").unwrap();
    let (_, _, env) = zeron_harness::acp_registry::launch(agent).unwrap();
    assert!(env.contains(&("FAKE_KEY".to_owned(), "sk-1".to_owned())), "{env:?}");

    // One of your own, through the same harness.
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../harness/tests/fixtures/fake-acp.sh");
    let own = client
        .call(
            methods::ADD_OWN_AGENT,
            json!({ "name": "Halcyon agent", "command": format!("'{}' --acp", fixture.display()), "env": "HALCYON=1" }),
        )
        .await
        .unwrap();
    assert_eq!(own["agent"]["own"], true, "{own}");
    assert!(own["agent"].get("delisted").is_none());
    assert!(find(&own["harnesses"], "id", "acp.own.halcyon-agent").is_some(), "{own}");
    let own_models = client
        .call(methods::LIST_MODELS, json!({ "harness": "acp.own.halcyon-agent" }))
        .await
        .expect("its models");
    assert!(own_models.to_string().contains("grok-4.5"), "{own_models}");
    assert!(client
        .call(methods::ADD_OWN_AGENT, json!({ "name": "Halcyon agent", "command": "true" }))
        .await
        .is_err(), "the same name twice");
    assert!(client
        .call(methods::ADD_OWN_AGENT, json!({ "name": "Nowhere", "command": "/no/such/agent" }))
        .await
        .is_err());
    client.call(methods::REMOVE_ACP_AGENT, json!({ "id": "own.halcyon-agent" })).await.unwrap();

    let removed = client.call(methods::REMOVE_ACP_AGENT, json!({ "id": "fake-agent" })).await.unwrap();
    let statuses = client.call(methods::CHECK_HARNESS_UPDATES, json!({})).await.unwrap();
    assert!(find(&statuses, "harness", "acp.fake-agent").is_none(), "untracked once removed");
    assert!(find(&removed["harnesses"], "id", "acp.fake-agent").is_none());
    let listed = client.call(methods::LIST_ACP_REGISTRY, json!({})).await.unwrap();
    assert!(find(&listed["agents"], "id", "fake-agent").unwrap().get("added").is_none());
    assert!(!dir.path().join("agents/fake-agent").exists());
}
