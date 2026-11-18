//! `zeron plugin check` and `zeron plugin publish` as a CI job runs them: the
//! real binary, no Zeron running, nobody signed in, a deploy key's secret in
//! `ZERON_DEPLOY_KEY`, against a stand-in for the edge's store. Each failure
//! says one line and exits with its code (`store_cli`).

#![cfg(unix)]

#[path = "../../../crates/engine/tests/support/fake_store.rs"]
mod fake_store;

use std::path::Path;
use std::process::Output;

use fake_store::FakeStore;
use serde_json::json;
use zeron_engine::plugins::install;
use zeron_engine::plugins::store::deploy_key::{DeployKey, Scope};

const ICON: &str = "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 16 16\"><circle cx=\"8\" cy=\"8\" r=\"6\"/></svg>";

/// A plugin folder that meets the store's rules, at `version`.
fn plugin(dir: &Path, version: &str) {
    std::fs::create_dir_all(dir).unwrap();
    let manifest = json!({
        "id": "focus-board", "name": "Focus Board", "version": version, "entry": "init.luau",
        "description": "A board of what you're focused on.", "icon": "icon.svg", "categories": ["productivity"],
    });
    std::fs::write(dir.join("zeron-plugin.json"), serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
    std::fs::write(dir.join("init.luau"), "return {}").unwrap();
    std::fs::write(dir.join("icon.svg"), ICON).unwrap();
    std::fs::write(dir.join("README.md"), "# Focus Board\n\nWhat you're on.").unwrap();
    std::fs::write(dir.join("CHANGELOG.md"), format!("## {version}\n\n- Boards.\n")).unwrap();
}

struct Run {
    _home: tempfile::TempDir,
    home: std::path::PathBuf,
}

impl Run {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().to_path_buf();
        Run { _home: home, home: path }
    }

    async fn zeron(&self, store: &FakeStore, secret: Option<&str>, args: &[&str]) -> Output {
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_zeron"));
        command
            .args(["plugin"])
            .args(args)
            .env_clear()
            .env("HOME", &self.home)
            .env("ZERON_DATA_DIR", self.home.join("data"))
            .env("ZERON_EDGE_URL", &store.url)
            // A port nothing listens on: no Zeron is running.
            .env("ZERON_IPC_PORT", "1")
            .env("PATH", std::env::var("PATH").unwrap_or_default());
        if let Some(secret) = secret {
            command.env("ZERON_DEPLOY_KEY", secret);
        }
        command.output().await.unwrap()
    }
}

fn text(output: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
}

#[tokio::test(flavor = "multi_thread")]
async fn ci_checks_and_publishes_with_a_deploy_key() {
    let store = FakeStore::start().await;
    let run = Run::new();
    let folder = run.home.join("focus-board");
    plugin(&folder, "1.0.0");
    let folder_arg = folder.display().to_string();
    let key = DeployKey::from_seed([11; 32]).unwrap();
    let secret = key.secret();
    store.claim("focus-board", "ada");
    store.add_deploy_key(&key.public(), Scope::plugins(&["focus-board".into()]).unwrap(), "ada");

    // check: no key, no Zeron, no sign-in.
    let checked = run.zeron(&store, None, &["check", &folder_arg]).await;
    assert_eq!(checked.status.code(), Some(0), "{}", text(&checked));
    assert!(text(&checked).contains("focus-board 1.0.0 meets the store's rules"));

    // --dry-run: checked, signed and packed, nothing sent.
    let dry = run.zeron(&store, Some(&secret), &["publish", &folder_arg, "--dry-run"]).await;
    assert_eq!(dry.status.code(), Some(0), "{}", text(&dry));
    assert!(text(&dry).contains("Not sent"));
    assert!(store.state.lock().unwrap().submitted.is_empty());

    // Published: signed with the deploy key, the request signed with it too.
    let published = run.zeron(&store, Some(&secret), &["publish", &folder_arg]).await;
    assert_eq!(published.status.code(), Some(0), "{}", text(&published));
    assert!(text(&published).contains("Published focus-board 1.0.0: live now, not yet verified."), "{}", text(&published));
    assert!(text(&published).contains("/plugins/focus-board"), "its page: {}", text(&published));
    let (id, raw, via) = store.state.lock().unwrap().submitted[0].clone();
    assert_eq!((id.as_str(), via.as_str()), ("focus-board", "deploy_key"));
    let package = install::parse(raw).unwrap();
    assert_eq!(package.key, key.public(), "signed with the deploy key");
    let auth = store.state.lock().unwrap().requests.iter().rev().find(|r| r.0 == "POST").and_then(|r| r.2.clone()).unwrap();
    assert!(auth.starts_with(&format!("ZeronKey key={}, ts=", key.public())));
    let sent = store.state.lock().unwrap().content_sha256.last().cloned().flatten().unwrap();
    assert_eq!(sent, zeron_engine::plugins::signing::sha256_hex(&store.state.lock().unwrap().submitted[0].1), "X-Zeron-Content-SHA256 names the body");
    // The secret is never said back.
    for output in [&checked, &dry, &published] {
        assert!(!text(output).contains(&secret[4..]));
    }

    // Again: the store says so, and it's the plugin's to fix (1).
    let again = run.zeron(&store, Some(&secret), &["publish", &folder_arg]).await;
    assert_eq!(again.status.code(), Some(1), "{}", text(&again));
    assert!(text(&again).contains("error: version_exists"));

    // status and yank, as the deploy key.
    let status = run.zeron(&store, Some(&secret), &["status"]).await;
    assert_eq!(status.status.code(), Some(0), "{}", text(&status));
    assert!(text(&status).contains("focus-board\n  1.0.0  listed  not verified"), "{}", text(&status));
    // A deploy key publishes and reads status, nothing else: yank, unyank
    // and keys are the signed-in person's (2, said plainly).
    let requests = store.state.lock().unwrap().requests.len();
    for args in [&["yank", "focus-board", "1.0.0"][..], &["unyank", "focus-board", "1.0.0"], &["key", "list"]] {
        let out = run.zeron(&store, Some(&secret), args).await;
        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", text(&out));
        assert!(text(&out).contains("a deploy key only publishes and reads status"), "{}", text(&out));
    }
    assert_eq!(store.state.lock().unwrap().requests.len(), requests, "nothing was sent");

    // 1.1.0 out before anyone verified 1.0.0: 1.0.0 takes no new installs,
    // and both the publish and status say so.
    plugin(&folder, "1.1.0");
    let newer = run.zeron(&store, Some(&secret), &["publish", &folder_arg]).await;
    assert_eq!(newer.status.code(), Some(0), "{}", text(&newer));
    assert!(text(&newer).contains("1.0.0 no longer takes new installs: 1.1.0 replaced it before it was verified"), "{}", text(&newer));
    let status = run.zeron(&store, Some(&secret), &["status"]).await;
    assert!(text(&status).contains("  1.0.0  yanked  not verified: superseded by 1.1.0"), "{}", text(&status));
}

#[tokio::test(flavor = "multi_thread")]
async fn each_failure_has_one_line_and_its_exit_code() {
    let store = FakeStore::start().await;
    let run = Run::new();
    let folder = run.home.join("focus-board");
    plugin(&folder, "1.0");
    let folder_arg = folder.display().to_string();
    let key = DeployKey::from_seed([12; 32]).unwrap();
    store.claim("focus-board", "ada");
    store.add_deploy_key(&key.public(), Scope::plugins(&["focus-board".into()]).unwrap(), "ada");

    // The store's rules, before anything is sent (1).
    let checked = run.zeron(&store, None, &["check", &folder_arg]).await;
    assert_eq!(checked.status.code(), Some(1));
    assert!(text(&checked).contains("error: not_semver (version)"), "{}", text(&checked));
    let refused = run.zeron(&store, Some(&key.secret()), &["publish", &folder_arg]).await;
    assert_eq!(refused.status.code(), Some(1));
    assert!(store.state.lock().unwrap().requests.is_empty(), "nothing was sent");
    plugin(&folder, "1.0.0");

    // A damaged secret (2), never echoed.
    let damaged = format!("{}x", &key.secret()[..40]);
    let out = run.zeron(&store, Some(&damaged), &["publish", &folder_arg]).await;
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));
    assert!(text(&out).contains("ZERON_DEPLOY_KEY") && !text(&out).contains(&damaged[4..]));
    // No key and no Zeron running (2), saying how CI does it.
    let out = run.zeron(&store, None, &["publish", &folder_arg]).await;
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));
    assert!(text(&out).contains("ZERON_DEPLOY_KEY"));
    // No such folder (2).
    let out = run.zeron(&store, Some(&key.secret()), &["publish", "/no/such/folder"]).await;
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));

    // A key the store doesn't know, or for another plugin (3).
    let stranger = DeployKey::from_seed([13; 32]).unwrap();
    let out = run.zeron(&store, Some(&stranger.secret()), &["publish", &folder_arg]).await;
    assert_eq!(out.status.code(), Some(3), "{}", text(&out));
    assert!(text(&out).contains("key_not_allowed"));
    let other = DeployKey::from_seed([14; 32]).unwrap();
    store.add_deploy_key(&other.public(), Scope::plugins(&["another-plugin".into()]).unwrap(), "ada");
    let out = run.zeron(&store, Some(&other.secret()), &["publish", &folder_arg]).await;
    assert_eq!(out.status.code(), Some(3), "{}", text(&out));

    // The store unreachable (4): try again.
    store.offline.store(true, std::sync::atomic::Ordering::SeqCst);
    let out = run.zeron(&store, Some(&key.secret()), &["publish", &folder_arg]).await;
    assert_eq!(out.status.code(), Some(4), "{}", text(&out));
    assert!(text(&out).contains("store_unreachable"));
    assert_eq!(text(&out).lines().filter(|l| l.starts_with("error:")).count(), 1, "one line: {}", text(&out));
}
