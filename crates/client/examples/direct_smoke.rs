//! Direct-mode smoke test against a real machine:
//!
//! ```sh
//! cargo run -p zeron-client --example direct_smoke -- HOST PORT USER KEYFILE [ENGINE_PORT]
//! ```
//!
//! Probes (printing the host key for TOFU), connects with the key pinned,
//! lists chats, starts a project-less session on the engine's device, sends
//! a prompt and prints the reply as it streams.

use std::sync::Arc;
use std::time::{Duration, Instant};

use zeron_client::direct::{SshAuth, SshError, SshTarget};
use zeron_client::events::NullListener;
use zeron_client::{Client, ClientConfig, Credentials, NewSession, SendRequest, SessionTarget};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 4 {
        eprintln!("usage: direct_smoke HOST PORT USER KEYFILE [ENGINE_PORT]");
        std::process::exit(2);
    }
    let key = std::fs::read_to_string(&args[3]).expect("read key");
    let mut target = SshTarget {
        host: args[0].clone(),
        port: args[1].parse().unwrap(),
        user: args[2].clone(),
        auth: SshAuth::Key {
            private_key: key,
            passphrase: None,
        },
        engine_port: args
            .get(4)
            .map_or(zeron_client::direct::DEFAULT_ENGINE_PORT, |p| {
                p.parse().unwrap()
            }),
        host_key_fingerprint: None,
    };
    let rt = zeron_client::runtime::shared();
    // 1. TOFU: the first probe reports the unknown key.
    match rt.block_on(zeron_client::direct::probe(&target)) {
        Err(SshError::HostKeyUnknown {
            fingerprint,
            algorithm,
        }) => {
            println!("host key: {algorithm} {fingerprint} (trusting)");
            target.host_key_fingerprint = Some(fingerprint);
        }
        other => panic!("expected HostKeyUnknown, got {other:?}"),
    }
    let probe = rt
        .block_on(zeron_client::direct::probe(&target))
        .expect("probe");
    println!("probe ok: {probe:?}");
    // A wrong pin must be refused.
    let mut wrong = target.clone();
    wrong.host_key_fingerprint = Some("SHA256:AAAA".into());
    assert!(matches!(
        rt.block_on(zeron_client::direct::probe(&wrong)),
        Err(SshError::HostKeyMismatch { .. })
    ));
    println!("mismatch detection ok");

    let dir = std::env::temp_dir().join("zeron-direct-smoke");
    let _ = std::fs::remove_dir_all(&dir);
    let mut config = ClientConfig::new("https://edge.invalid", &dir);
    config.device_id = "android-smoke".into();
    config.platform = "android".into();
    let client =
        Client::new(config, Credentials::Direct(target), Arc::new(NullListener)).expect("client");
    wait("connected + synced", Duration::from_secs(30), || {
        client
            .workspace()
            .devices
            .iter()
            .any(|d| d.id == probe.engine_device_id && d.online)
    });
    let ws = client.workspace();
    println!(
        "devices={} projects={} recent={} connectivity={:?}",
        ws.devices.len(),
        ws.projects.len(),
        ws.front.recent.len(),
        client.connectivity().state
    );
    for row in ws.front.recent.iter().take(5) {
        println!("  chat {} {:?}", row.id, row.title);
    }
    let chat_id = client
        .create_session(NewSession {
            target: SessionTarget::Projectless {
                device_id: probe.engine_device_id.clone(),
            },
            config: None,
            branch: None,
            cwd: None,
            title: Some("direct smoke".into()),
        })
        .expect("create");
    println!("created {chat_id}");
    let session = client.open_session(&chat_id).expect("open");
    session.set_view_attached(true);
    std::thread::sleep(Duration::from_millis(800));
    let outcome = session
        .send(SendRequest::text("Hello over SSH"))
        .expect("send");
    println!("send: {outcome:?}");
    let started = Instant::now();
    let mut last = String::new();
    loop {
        let snap = session.snapshot();
        let text: String = snap
            .entries
            .iter()
            .map(|e| {
                format!(
                    "[{:?}{}] {}",
                    e.message.role,
                    if e.echo.is_some() { " echo" } else { "" },
                    e.message
                        .parts
                        .iter()
                        .filter_map(|p| match p {
                            zeron_client::MessagePart::Text { text, .. } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("")
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        if text != last {
            println!(
                "--- t+{}ms streaming={}\n{text}",
                started.elapsed().as_millis(),
                snap.streaming
            );
            last = text;
        }
        let done = snap.entries.len() >= 2
            && !snap.streaming
            && snap.pending.is_empty()
            && started.elapsed() > Duration::from_secs(2);
        if done || started.elapsed() > Duration::from_secs(60) {
            break;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    let row = client.workspace().session(&chat_id).cloned();
    println!(
        "row title={:?} indicator={:?}",
        row.as_ref().map(|r| r.title.clone()),
        row.map(|r| r.indicator)
    );
    client.shutdown();
}

fn wait(what: &str, timeout: Duration, mut f: impl FnMut() -> bool) {
    let start = Instant::now();
    while !f() {
        assert!(start.elapsed() < timeout, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
    println!("{what}: {}ms", start.elapsed().as_millis());
}
