//! `zeron plugin check` and `zeron plugin publish` as a CI job runs them: the
//! real binary, no Zeron running, nobody signed in, a deploy token in
//! `ZERON_DEPLOY_TOKEN`, against a stand-in for the edge's store. Each failure
//! says one line and exits with its code (`store_cli`).

#![cfg(unix)]

#[path = "../../../crates/engine/tests/support/fake_store.rs"]
mod fake_store;

use std::path::Path;
use std::process::Output;

use fake_store::FakeStore;
use serde_json::json;
use zeron_engine::plugins::store::deploy_token::{DeployToken, Scope};
use zeron_engine::plugins::{install, package, signing::SigningKey};

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

/// Version `version` of focus-board as a device publishes it (signed, by
/// its owner): a plugin's first version always comes so.
fn from_a_device(store: &FakeStore, version: &str) {
    let files: std::collections::BTreeMap<String, Vec<u8>> = [("init.luau".to_string(), b"return {}".to_vec())].into();
    let manifest = json!({ "id": "focus-board", "name": "Focus Board", "version": version, "entry": "init.luau" });
    let (key, _) = SigningKey::generate().unwrap();
    let raw = package::pack(install::FORMAT, &key.sign_manifest(&manifest, &package::hashes(&files)), &files).unwrap();
    store.add("ada", &raw, "listed", "");
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
        self.run(store, secret, "plugin", args).await
    }

    async fn run(&self, store: &FakeStore, secret: Option<&str>, noun: &str, args: &[&str]) -> Output {
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_zeron"));
        command
            .args([noun])
            .args(args)
            .env_clear()
            .env("HOME", &self.home)
            .env("ZERON_DATA_DIR", self.home.join("data"))
            .env("ZERON_EDGE_URL", &store.url)
            // A port nothing listens on: no Zeron is running.
            .env("ZERON_IPC_PORT", "1")
            .env("PATH", std::env::var("PATH").unwrap_or_default());
        if let Some(secret) = secret {
            command.env("ZERON_DEPLOY_TOKEN", secret);
        }
        command.output().await.unwrap()
    }
}

fn text(output: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
}

#[tokio::test(flavor = "multi_thread")]
async fn ci_checks_and_publishes_with_a_deploy_token() {
    let store = FakeStore::start().await;
    let run = Run::new();
    let folder = run.home.join("focus-board");
    plugin(&folder, "1.0.0");
    let folder_arg = folder.display().to_string();
    store.claim("focus-board", "ada");
    let secret = store.add_deploy_token("ada", "GitHub Actions", Scope::plugins(&["focus-board".into()]).unwrap());
    let short = DeployToken::parse(&secret).unwrap().short().to_owned();

    // check: no token, no Zeron, no sign-in.
    let checked = run.zeron(&store, None, &["check", &folder_arg]).await;
    assert_eq!(checked.status.code(), Some(0), "{}", text(&checked));
    assert!(text(&checked).contains("focus-board 1.0.0 meets the store's rules"));

    // --dry-run: checked and packed, nothing sent; the token named by its
    // short id alone.
    let dry = run.zeron(&store, Some(&secret), &["publish", &folder_arg, "--dry-run"]).await;
    assert_eq!(dry.status.code(), Some(0), "{}", text(&dry));
    assert!(text(&dry).contains("Not sent") && text(&dry).contains(&format!("deploy token {short}")), "{}", text(&dry));
    assert!(store.state.lock().unwrap().submitted.is_empty());

    // A plugin's first version: never a token's (3), said plainly.
    let first = run.zeron(&store, Some(&secret), &["publish", &folder_arg]).await;
    assert_eq!(first.status.code(), Some(3), "{}", text(&first));
    assert!(text(&first).contains("token_not_allowed: This deploy token may not publish this plugin (a plugin's first version is published from your computer)."), "{}", text(&first));
    assert!(store.state.lock().unwrap().submitted.is_empty());
    from_a_device(&store, "0.9.0");

    // Published: unsigned (the store records it as the token's), the token
    // its bearer.
    let published = run.zeron(&store, Some(&secret), &["publish", &folder_arg]).await;
    assert_eq!(published.status.code(), Some(0), "{}", text(&published));
    assert!(text(&published).contains("Published focus-board 1.0.0: live now, not yet verified."), "{}", text(&published));
    assert!(text(&published).contains("/plugins/focus-board"), "its page: {}", text(&published));
    let (id, raw, via) = store.state.lock().unwrap().submitted[0].clone();
    assert_eq!((id.as_str(), via.as_str()), ("focus-board", "deploy_token"));
    assert_eq!(install::parse_from_store(raw.clone(), false).unwrap().key, None, "unsigned");
    assert_eq!(install::reason_of(&install::parse(raw).unwrap_err()), Some("unsigned"), "never a sideload");
    let auth = store.state.lock().unwrap().requests.iter().rev().find(|r| r.0 == "POST").and_then(|r| r.2.clone()).unwrap();
    assert_eq!(auth, format!("Bearer {secret}"));
    // The token is never said back (its short id is all that's named).
    for output in [&checked, &dry, &published] {
        assert!(!text(output).contains(&secret[12..]));
    }

    // Again: the store says so, and it's the plugin's to fix (1).
    let again = run.zeron(&store, Some(&secret), &["publish", &folder_arg]).await;
    assert_eq!(again.status.code(), Some(1), "{}", text(&again));
    assert!(text(&again).contains("error: version_exists"));

    // A deploy token publishes, nothing else: status, yank, unyank and
    // tokens are the signed-in person's (2, said plainly).
    let requests = store.state.lock().unwrap().requests.len();
    for args in [&["status"][..], &["yank", "focus-board", "1.0.0"], &["unyank", "focus-board", "1.0.0"], &["token", "list"]] {
        let out = run.zeron(&store, Some(&secret), args).await;
        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", text(&out));
        assert!(text(&out).contains("needs Zeron running here and signed in: a deploy token only publishes"), "{}", text(&out));
    }
    assert_eq!(store.state.lock().unwrap().requests.len(), requests, "nothing was sent");

    // 1.1.0 out before anyone verified 1.0.0: 1.0.0 takes no new installs,
    // and the publish says so.
    plugin(&folder, "1.1.0");
    let newer = run.zeron(&store, Some(&secret), &["publish", &folder_arg]).await;
    assert_eq!(newer.status.code(), Some(0), "{}", text(&newer));
    assert!(text(&newer).contains("1.0.0 no longer takes new installs: 1.1.0 replaced it before it was verified"), "{}", text(&newer));
    // An older version: refused before it's sent, by the store's public
    // list of the plugin's versions (the token reads nothing of yours).
    plugin(&folder, "1.0.5");
    let sent = store.state.lock().unwrap().submitted.len();
    let older = run.zeron(&store, Some(&secret), &["publish", &folder_arg]).await;
    assert_eq!(older.status.code(), Some(1), "{}", text(&older));
    assert!(text(&older).contains("error: not_newer (version): Version 1.0.5 isn't later than 1.1.0"), "{}", text(&older));
    assert_eq!(store.state.lock().unwrap().submitted.len(), sent, "nothing was sent");
    let asked = store.state.lock().unwrap().requests.iter().rev().find(|r| r.1.starts_with("/store/v1/plugins/focus-board/versions?")).cloned().unwrap();
    assert!(asked.2.is_none(), "the public list, asked with no token: {asked:?}");

    // "Zeron" in a listing is nobody's concern: published as any other.
    plugin(&folder, "1.2.0");
    let path = folder.join("zeron-plugin.json");
    let mut manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    manifest["name"] = json!("Zeron Git");
    manifest["description"] = json!("Boards, by Zeron.");
    std::fs::write(&path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
    let published = run.zeron(&store, Some(&secret), &["publish", &folder_arg]).await;
    let said = text(&published);
    assert_eq!(published.status.code(), Some(0), "{said}");
    assert!(!said.contains("warning: "), "{said}");

    // Claiming the mark: refused before it's sent, the word named, how to
    // reword said (the plugin's to fix: 1).
    plugin(&folder, "1.3.0");
    let mut manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    manifest["description"] = json!("The official board plugin.");
    std::fs::write(&path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
    let requests = store.state.lock().unwrap().submitted.len();
    let refused = run.zeron(&store, Some(&secret), &["publish", &folder_arg]).await;
    let said = text(&refused);
    assert_eq!(refused.status.code(), Some(1), "{said}");
    assert!(said.contains("\"official\"") && said.contains("Say what it does instead"), "{said}");
    assert_eq!(store.state.lock().unwrap().submitted.len(), requests, "nothing was sent");
}

/// `zeron template check` and `publish` as a CI job runs them: no Zeron, no
/// sign-in; unsigned, under the templates' prefix, the same words and codes.
#[tokio::test(flavor = "multi_thread")]
async fn ci_checks_and_publishes_a_template_with_a_deploy_token() {
    let store = FakeStore::start().await;
    let run = Run::new();
    let folder = run.home.join("midnight");
    let write = |rel: &str, bytes: &[u8]| {
        let path = folder.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    };
    let version = |version: &str| {
        let manifest = json!({ "id": "midnight", "name": "Midnight", "description": "A dark interface.", "author": "you", "version": version, "zeron_version": "0.3.16" });
        write("zeron-template.json", &serde_json::to_vec(&manifest).unwrap());
        write("CHANGELOG.md", format!("## {version}\n- Darker.\n").as_bytes());
    };
    version("1.0.0");
    write("app.luau", b"return {}");
    write("preview.png", b"\x89PNG\r\n\x1a\n");
    let arg = folder.display().to_string();
    let secret = store.add_deploy_token("ada", "CI", Scope::plugins(&["midnight".into(), "focus-board".into()]).unwrap());

    // What it lacks, all said, nothing sent (1).
    let checked = run.run(&store, None, "template", &["check", &arg]).await;
    assert_eq!(checked.status.code(), Some(1), "{}", text(&checked));
    assert!(text(&checked).contains("error: missing_readme"), "{}", text(&checked));
    let refused = run.run(&store, Some(&secret), "template", &["publish", &arg]).await;
    assert_eq!(refused.status.code(), Some(1), "{}", text(&refused));
    write("README.md", b"# Midnight");
    write("screenshots/x.luau", b"return {}");
    let stray = run.run(&store, None, "template", &["check", &arg]).await;
    assert_eq!(stray.status.code(), Some(1), "{}", text(&stray));
    assert!(text(&stray).contains("error: bad_listing (screenshots/x.luau)"), "{}", text(&stray));
    std::fs::remove_dir_all(folder.join("screenshots")).unwrap();
    assert!(store.state.lock().unwrap().requests.is_empty(), "nothing was sent");

    let checked = run.run(&store, None, "template", &["check", &arg]).await;
    assert_eq!(checked.status.code(), Some(0), "{}", text(&checked));
    assert!(text(&checked).contains("midnight 1.0.0 meets the store's rules"));
    let dry = run.run(&store, Some(&secret), "template", &["publish", &arg, "--dry-run"]).await;
    assert_eq!(dry.status.code(), Some(0), "{}", text(&dry));
    assert!(text(&dry).contains("Not sent") && store.state.lock().unwrap().submitted_templates.is_empty(), "{}", text(&dry));

    // Its first version is never a token's (3), said of a template.
    let first = run.run(&store, Some(&secret), "template", &["publish", &arg]).await;
    assert_eq!(first.status.code(), Some(3), "{}", text(&first));
    assert!(text(&first).contains("may not publish this template (a template's first version is published from your computer)"), "{}", text(&first));
    store.state.lock().unwrap().templates.insert("midnight".into(), ("ada".into(), vec!["0.9.0".into()]));

    let published = run.run(&store, Some(&secret), "template", &["publish", &arg]).await;
    assert_eq!(published.status.code(), Some(0), "{}", text(&published));
    assert!(text(&published).contains("Published midnight 1.0.0: live now, not yet verified.") && text(&published).contains("/templates/midnight"), "{}", text(&published));
    let (id, raw) = store.state.lock().unwrap().submitted_templates[0].clone();
    let package = zeron_engine::plugins::template_packages::parse_from_store(raw, false).unwrap();
    assert_eq!((id.as_str(), package.key.as_deref()), ("midnight", None), "unsigned");
    assert!(package.files.contains_key("README.md") && package.manifest.get("author").is_none());
    assert!(store.state.lock().unwrap().submitted.is_empty(), "never as a plugin");
    for output in [&dry, &first, &published] {
        assert!(!text(output).contains(&secret[12..]), "the token is never said back");
    }
    // An older version: refused before it's sent, by its public versions.
    version("0.9.5");
    let older = run.run(&store, Some(&secret), "template", &["publish", &arg]).await;
    assert_eq!(older.status.code(), Some(1), "{}", text(&older));
    assert!(text(&older).contains("error: not_newer (version)"), "{}", text(&older));
    assert_eq!(store.state.lock().unwrap().submitted_templates.len(), 1);
    // What it leaves out is said, and in no one's way.
    version("1.0.1");
    std::fs::create_dir_all(folder.join("account")).unwrap();
    write("account/page.luau", b"return {}");
    let said = run.run(&store, None, "template", &["check", &arg]).await;
    assert_eq!(said.status.code(), Some(0), "{}", text(&said));
    assert!(text(&said).contains("warning: left_out (account/page.luau)"), "{}", text(&said));
    std::fs::remove_dir_all(folder.join("account")).unwrap();
    // The rest is the running Zeron's (2, when none is), never a token's.
    for (token, args, words) in [
        (None, vec!["publish", "midnight"], "Zeron isn't running here"),
        (None, vec!["store"], "`zeron template store` needs Zeron running"),
        (None, vec!["yank", "midnight", "1.0.0"], "`zeron template yank` needs Zeron running"),
        (Some(secret.as_str()), vec!["unyank", "midnight", "1.0.0"], "a deploy token only publishes"),
    ] {
        let out = run.run(&store, token, "template", &args).await;
        assert!(out.status.code() == Some(2) && text(&out).contains(words), "{args:?}: {}", text(&out));
    }
    // No token, no such folder, a folder that's no template (2).
    for (token, args) in [(None, vec!["publish", &arg]), (Some(secret.as_str()), vec!["publish", "/no/such/folder"]), (None, vec!["check", run.home.to_str().unwrap()])] {
        let out = run.run(&store, token, "template", &args).await;
        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", text(&out));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn each_failure_has_one_line_and_its_exit_code() {
    let store = FakeStore::start().await;
    let run = Run::new();
    let folder = run.home.join("focus-board");
    plugin(&folder, "1.0");
    let folder_arg = folder.display().to_string();
    store.claim("focus-board", "ada");
    let secret = store.add_deploy_token("ada", "CI", Scope::plugins(&["focus-board".into()]).unwrap());

    // The store's rules, before anything is sent (1).
    let checked = run.zeron(&store, None, &["check", &folder_arg]).await;
    assert_eq!(checked.status.code(), Some(1));
    assert!(text(&checked).contains("error: not_semver (version)"), "{}", text(&checked));
    let refused = run.zeron(&store, Some(&secret), &["publish", &folder_arg]).await;
    assert_eq!(refused.status.code(), Some(1));
    assert!(store.state.lock().unwrap().requests.is_empty(), "nothing was sent");
    plugin(&folder, "1.0.0");

    // A mistyped token, or something that isn't one (2), never echoed, and
    // nothing sent.
    let last = if secret.ends_with('0') { '1' } else { '0' };
    let damaged = format!("{}{last}", &secret[..secret.len() - 1]);
    for (bad, word) in [(damaged.as_str(), "damaged"), ("ghp_notours", "not_a_token")] {
        let out = run.zeron(&store, Some(bad), &["publish", &folder_arg]).await;
        assert_eq!(out.status.code(), Some(2), "{}", text(&out));
        assert!(text(&out).contains("ZERON_DEPLOY_TOKEN") && text(&out).contains(word) && !text(&out).contains(&bad[4..]), "{}", text(&out));
    }
    assert!(store.state.lock().unwrap().requests.is_empty(), "nothing was sent");
    // No token and no Zeron running (2), saying how CI does it.
    let out = run.zeron(&store, None, &["publish", &folder_arg]).await;
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));
    assert!(text(&out).contains("ZERON_DEPLOY_TOKEN"));
    // No such folder (2).
    let out = run.zeron(&store, Some(&secret), &["publish", "/no/such/folder"]).await;
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));

    // A token the store doesn't know (or revoked), or for another plugin (3).
    let stranger = DeployToken::from_random(&[99; 32]);
    let out = run.zeron(&store, Some(stranger.as_str()), &["publish", &folder_arg]).await;
    assert_eq!(out.status.code(), Some(3), "{}", text(&out));
    assert!(text(&out).contains("invalid_token"), "{}", text(&out));
    let other = store.add_deploy_token("ada", "other", Scope::plugins(&["another-plugin".into()]).unwrap());
    let out = run.zeron(&store, Some(&other), &["publish", &folder_arg]).await;
    assert_eq!(out.status.code(), Some(3), "{}", text(&out));
    assert!(text(&out).contains("token_not_allowed") && text(&out).contains("may not publish this plugin"), "{}", text(&out));

    // The store unreachable (4): try again.
    store.offline.store(true, std::sync::atomic::Ordering::SeqCst);
    let out = run.zeron(&store, Some(&secret), &["publish", &folder_arg]).await;
    assert_eq!(out.status.code(), Some(4), "{}", text(&out));
    assert!(text(&out).contains("store_unreachable"));
    assert_eq!(text(&out).lines().filter(|l| l.starts_with("error:")).count(), 1, "one line: {}", text(&out));
}
