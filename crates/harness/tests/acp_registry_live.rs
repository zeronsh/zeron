//! The real ACP registry, live (ignored: it downloads agents): its index
//! fetched and read, and each agent named in `ZERON_LIVE_ACP_AGENTS`
//! (comma separated; a checksummed binary and an npm package by default)
//! installed into a scratch agents folder, launched as Zeron launches it,
//! answering ACP's `initialize`, and removed.
//!
//!   cargo test -p zeron-harness --test acp_registry_live -- --ignored --nocapture

use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use zeron_harness::acp_registry as reg;
use zeron_proto::RegistryAgent;

#[tokio::test]
#[ignore = "downloads agents from the live ACP registry"]
async fn live_registry_agents_install_and_answer_initialize() {
    let root = tempfile::tempdir().unwrap();
    // SAFETY: this file's only test; nothing else reads it.
    unsafe { std::env::set_var("ZERON_AGENTS_DIR", root.path()) };
    let (bytes, etag) = reg::fetch_index(reg::INDEX_URL, None).await.expect("the index");
    let entries = reg::parse_index(&bytes.expect("a body")).expect("a usable index");
    eprintln!("{} agents listed (etag {etag:?})", entries.len());
    assert!(entries.len() >= 20, "{}", entries.len());
    // Unchanged since: nothing to read again.
    if let Some(etag) = &etag {
        let (again, _) = reg::fetch_index(reg::INDEX_URL, Some(etag)).await.unwrap();
        assert!(again.is_none(), "a 304 for an unchanged index");
    }
    let wanted = std::env::var("ZERON_LIVE_ACP_AGENTS").unwrap_or_else(|_| "poolside,glm-acp-agent".into());
    for id in wanted.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let entry = entries.iter().find(|e| e.id == id).unwrap_or_else(|| panic!("{id} isn't listed"));
        eprintln!("{id}: {:?}, {:?}", reg::choose(entry).map(|c| c.kind()), reg::readiness(entry));
        if reg::readiness(entry) != reg::Readiness::Ready {
            eprintln!("{id}: skipped, this machine can't run it");
            continue;
        }
        let icon = match &entry.icon {
            Some(url) => reg::fetch_icon(url).await,
            None => None,
        };
        let started = std::time::Instant::now();
        let record = reg::install(entry, icon.clone()).await.unwrap_or_else(|e| panic!("{id}: {e}"));
        eprintln!(
            "{id}: installed {} ({:?}, checked {}) in {:.1}s → {}",
            record.version,
            record.kind,
            record.verified,
            started.elapsed().as_secs_f32(),
            record.program.display()
        );
        assert_eq!(record.icon_svg.is_some(), icon.is_some());
        let agent = RegistryAgent::new(id).unwrap();
        assert!(reg::installed(agent));

        // Launched as Zeron launches it: it speaks ACP.
        let (program, args, env) = reg::launch(agent).unwrap();
        let mut child = tokio::process::Command::new(&program)
            .args(&args)
            .envs(env)
            .current_dir(root.path())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap_or_else(|e| panic!("{id}: {e}"));
        let mut stdin = child.stdin.take().unwrap();
        let request = json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": 1,
            "clientInfo": { "name": "zeron", "version": "test" },
            "clientCapabilities": { "fs": { "readTextFile": false, "writeTextFile": false }, "terminal": false, "auth": { "terminal": true } }
        }});
        stdin.write_all(format!("{request}\n").as_bytes()).await.unwrap();
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let answer = tokio::time::timeout(Duration::from_secs(90), async {
            while let Ok(Some(line)) = lines.next_line().await {
                if let Ok(v) = serde_json::from_str::<Value>(&line)
                    && v["id"] == json!(1)
                {
                    return Some(v);
                }
            }
            None
        })
        .await
        .unwrap_or_else(|_| panic!("{id}: no answer to initialize"))
        .unwrap_or_else(|| panic!("{id}: closed without answering"));
        let methods: Vec<String> = answer["result"]["authMethods"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|m| format!("{}:{}", m["id"].as_str().unwrap_or("?"), m["type"].as_str().unwrap_or("agent")))
            .collect();
        eprintln!("{id}: protocol {} · sign-in {methods:?}", answer["result"]["protocolVersion"]);
        assert!(answer["result"]["protocolVersion"].is_number(), "{id}: {answer}");
        drop(stdin);
        let _ = child.kill().await;

        reg::remove(agent).unwrap();
        assert!(!reg::installed(agent));
    }
}

/// Each agent named in `ZERON_LIVE_ACP_SIGNIN` signing in as Zeron runs it,
/// in a scratch HOME (nothing of yours touched), for long enough to show
/// what it asks for: its page, a code, its output. Never finishes a
/// sign-in (no account), and says how far it got.
#[tokio::test]
#[ignore = "downloads agents and starts their sign-ins"]
async fn live_registry_sign_ins_report_their_page() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let browser = root.path().join("browser");
    let opened = root.path().join("opened");
    std::fs::write(&browser, format!("#!/bin/sh\nprintf '%s\\n' \"$1\" >> '{}'\n", opened.display())).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&browser, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    unsafe {
        std::env::set_var("ZERON_AGENTS_DIR", root.path().join("agents"));
    }
    let (bytes, _) = reg::fetch_index(reg::INDEX_URL, None).await.unwrap();
    let entries = reg::parse_index(&bytes.unwrap()).unwrap();
    let wanted = std::env::var("ZERON_LIVE_ACP_SIGNIN").unwrap_or_else(|_| "factory-droid".into());
    for id in wanted.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let entry = entries.iter().find(|e| e.id == id).unwrap();
        reg::install(entry, None).await.unwrap();
        let agent = RegistryAgent::new(id).unwrap();
        let _ = std::fs::remove_file(&opened);
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let log = seen.clone();
        let harness = zeron_harness::AcpHarness::registry(agent, &entry.name);
        let options = zeron_harness::acp::SignInOptions {
            browser: Some(browser.clone()),
            method: None,
            env: vec![("HOME".into(), home.clone().into()), ("ZERON_LOGIN_URL_FILE".into(), opened.clone().into())],
            url_filter: None,
        };
        let outcome = tokio::time::timeout(
            Duration::from_secs(25),
            harness.sign_in_registry(options, move |p| log.lock().unwrap().push(format!("{p:?}"))),
        )
        .await;
        eprintln!("== {id}: {}", match &outcome {
            Ok(Ok(())) => "signed in (?)".to_string(),
            Ok(Err(e)) => format!("ended: {e}"),
            Err(_) => "still waiting after 25s".to_string(),
        });
        for line in seen.lock().unwrap().iter().take(12) {
            eprintln!("   {}", line.chars().take(220).collect::<String>());
        }
        eprintln!("   browser asked for: {:?}", std::fs::read_to_string(&opened).ok());
    }
}
