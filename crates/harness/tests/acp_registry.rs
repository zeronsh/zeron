//! An agent from the ACP registry, end to end on this machine: a registry
//! served on the loopback lists a build of the fake ACP agent
//! (`fixtures/fake-acp.sh`, packed as a .tar.gz with its sha256), it
//! installs into an agents folder of its own, launches through the generic
//! registry harness with the registry's arguments and environment, shows
//! its models from its own handshake, and is removed again. A build whose
//! download doesn't match its checksum never lands.

#![cfg(unix)]

use std::io::Write;
use std::path::PathBuf;

use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use zeron_harness::{AcpHarness, Harness, acp_registry as reg};
use zeron_proto::{HarnessId, RegistryAgent};

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake-acp.sh")
}

/// The fake agent as a registry build: `fake/agent` in a .tar.gz.
fn archive() -> Vec<u8> {
    let script = std::fs::read(fixture()).unwrap();
    let mut tar = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_gnu();
    header.set_size(script.len() as u64);
    header.set_mode(0o755);
    header.set_cksum();
    tar.append_data(&mut header, "fake/agent", &script[..]).unwrap();
    let tar = tar.into_inner().unwrap();
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gz.write_all(&tar).unwrap();
    gz.finish().unwrap()
}

/// Serves `files` by path on the loopback, forever (the test's lifetime).
async fn serve(files: Vec<(&'static str, Vec<u8>)>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
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
    base
}

#[tokio::test]
async fn a_registry_agent_installs_launches_and_is_removed() {
    let root = tempfile::tempdir().unwrap();
    // SAFETY: this file's only test; nothing else reads the environment.
    unsafe {
        std::env::set_var("ZERON_AGENTS_DIR", root.path());
        std::env::set_var("ZERON_ACP_REGISTRY_LOOPBACK", "1");
    }
    let built = archive();
    let sha = format!("{:x}", Sha256::digest(&built));
    let tampered = {
        let mut t = built.clone();
        let last = t.len() - 1;
        t[last] ^= 1;
        t
    };
    let base = serve(vec![
        ("/fake.tar.gz", built),
        ("/tampered.tar.gz", tampered),
        ("/icon.svg", b"<svg viewBox=\"0 0 16 16\"><path fill=\"currentColor\" d=\"M0 0h16v16H0z\"/></svg>".to_vec()),
    ])
    .await;
    let platform = reg::platform().expect("a registry platform");
    let index = serde_json::json!({
        "version": "1.0.0",
        "agents": [
            {
                "id": "fake-agent", "name": "Fake Agent", "version": "1.2.3",
                "description": "Answers in scripts.", "authors": ["Zeron tests"], "license": "MIT",
                "icon": format!("{base}/icon.svg"),
                "distribution": { "binary": { platform: {
                    "archive": format!("{base}/fake.tar.gz"), "cmd": "./fake/agent",
                    "args": [], "env": { "FAKE_REGISTRY_ENV": "1" }, "sha256": sha
                } } }
            },
            {
                "id": "tampered", "name": "Tampered", "version": "1.0.0", "description": "",
                "distribution": { "binary": { platform: {
                    "archive": format!("{base}/tampered.tar.gz"), "cmd": "./fake/agent", "sha256": sha
                } } }
            }
        ]
    });
    let entries = reg::parse_index(&serde_json::to_vec(&index).unwrap()).unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(reg::checksum(&entries[0]), Some(true));
    assert_eq!(reg::readiness(&entries[0]), reg::Readiness::Ready);

    // A download that doesn't match its checksum never lands.
    let refused = reg::install(&entries[1], None).await.expect_err("checksum mismatch");
    assert!(refused.to_string().contains("checksum"), "{refused}");
    let tampered = RegistryAgent::new("tampered").unwrap();
    assert!(!reg::installed(tampered));

    let icon = reg::fetch_icon(&format!("{base}/icon.svg")).await;
    assert!(icon.is_some());
    let record = reg::install(&entries[0], icon).await.expect("install");
    assert!(record.verified);
    assert_eq!(record.version, "1.2.3");
    assert!(record.program.starts_with(root.path().join("fake-agent").join("1.2.3")));
    let agent = RegistryAgent::new("fake-agent").unwrap();
    assert!(reg::installed(agent));
    assert_eq!(reg::records().len(), 1);
    let (_, _, env) = reg::launch(agent).unwrap();
    assert_eq!(env, vec![("FAKE_REGISTRY_ENV".to_owned(), "1".to_owned())]);

    // The generic harness: the agent's own models, from its handshake.
    let harness = AcpHarness::registry(agent, "Fake Agent");
    assert_eq!(harness.id(), HarnessId::Registry(agent));
    assert_eq!(harness.id().as_str(), "acp.fake-agent");
    assert!(harness.installed());
    let models = harness.models().await.expect("models from the agent");
    let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["grok-4-fast", "grok-4.5"]);

    reg::remove(agent).unwrap();
    assert!(!reg::installed(agent));
    assert!(!harness.installed());
    assert!(!root.path().join("fake-agent").exists());
}
