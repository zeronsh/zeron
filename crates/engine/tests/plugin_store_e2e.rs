//! The plugin store end to end, against the real edge: real `zeron headless`
//! engines (one per device) and the real CLI, driven over their IPC, talking
//! to the edge worker run locally by scripts/plugin-store-e2e/edge.mjs.
//! Run by scripts/plugin-store-e2e/run.sh, which builds both, starts the
//! edge, runs this, and stops everything; ignored otherwise (it needs
//! `E2E_EDGE_URL`, `E2E_ZERON_BIN` and `E2E_WORK_DIR`).
//!
//! Each step prints `PASS <n> <what>` or panics with `FAIL <n> <what>: why`.
//! Accounts are test sign-ins (`code:<user id>`): `user_ada` (the author,
//! @ada-e2e), `user_bob` (someone else, @bob-e2e), `user_admin` (a store
//! admin, by the edge-store lane's stand-in session).

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use zeron_rpc::{RpcClient, methods};

struct Env {
    edge: String,
    zeron: PathBuf,
    work: PathBuf,
    http: reqwest::Client,
}

fn env() -> Option<Env> {
    let get = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
    Some(Env {
        edge: get("E2E_EDGE_URL")?,
        zeron: PathBuf::from(get("E2E_ZERON_BIN")?),
        work: PathBuf::from(get("E2E_WORK_DIR")?),
        http: reqwest::Client::new(),
    })
}

fn pass(step: &str, what: &str) {
    println!("PASS {step} {what}");
}

/// A port nothing listens on yet.
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

/// One device: a `zeron headless` on its own scratch home and data folder.
struct Device {
    name: String,
    data: PathBuf,
    child: Child,
    client: RpcClient,
}

impl Drop for Device {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Env {
    /// The environment every run of Zeron here gets: this run's alone.
    fn command(&self, home: &Path, data: &Path) -> Command {
        let mut command = Command::new(&self.zeron);
        command
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", home)
            .env("ZERON_DATA_DIR", data)
            .env("ZERON_EDGE_URL", &self.edge)
            .env("ZERON_WORKOS_CLIENT_ID", "client_e2e")
            .env("RUST_LOG", "warn,zeron_engine::plugins=info");
        command
    }

    async fn device(&self, name: &str) -> Device {
        let home = self.work.join(name).join("home");
        let data = self.work.join(name).join("data");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&data).unwrap();
        let port = free_port();
        let log = std::fs::File::create(self.work.join(name).join("engine.log")).unwrap();
        let child = self
            .command(&home, &data)
            .env("ZERON_IPC_PORT", port.to_string())
            .arg("headless")
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .expect("zeron headless starts");
        let started = Instant::now();
        let client = loop {
            match zeron_rpc::connect_ipc(port, Some(&data)).await {
                Ok(client) => break client,
                Err(_) if started.elapsed() < Duration::from_secs(60) => tokio::time::sleep(Duration::from_millis(250)).await,
                Err(error) => panic!("FAIL {name}'s engine never answered: {error}"),
            }
        };
        Device { name: name.into(), data, child, client }
    }

    /// `user` signs in on `device` (a test code), and its signing key is
    /// registered with the ticket that sign-in earns.
    async fn sign_in(&self, device: &Device, user: &str) {
        let url = device.call(methods::SIGN_IN_HEADLESS, json!({})).await.unwrap();
        let url = url["url"].as_str().unwrap();
        let state = url.split("state=").nth(1).unwrap().split('&').next().unwrap();
        device.call(methods::COMPLETE_SIGN_IN, json!({ "code": format!("{state}.code:{user}") })).await.unwrap_or_else(|e| panic!("FAIL {} couldn't sign in as {user}: {e}", device.name));
        let started = Instant::now();
        loop {
            let key = device.call(methods::PLUGIN_SIGNING_KEY, json!({})).await.unwrap();
            if key["registered"] == true {
                break;
            }
            assert!(started.elapsed() < Duration::from_secs(30), "FAIL {}'s signing key was never registered: {key}", device.name);
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    /// `user`'s bearer, from `device` signed in as them.
    async fn bearer(&self, device: &Device) -> String {
        device.call(methods::ACCESS_TOKEN, json!({})).await.unwrap()["token"].as_str().unwrap().to_owned()
    }

    async fn claim_username(&self, device: &Device, username: &str) {
        let token = self.bearer(device).await;
        let res = self.http.post(format!("{}/username", self.edge)).bearer_auth(token).json(&json!({ "username": username })).send().await.unwrap();
        let status = res.status();
        assert!(status.is_success(), "FAIL claiming {username}: {status} {}", res.text().await.unwrap_or_default());
    }

    /// An admin's call (the edge-store lane's stand-in web session).
    async fn admin(&self, path: &str, body: Value) -> (u16, Value) {
        let res = self
            .http
            .post(format!("{}/store/v1/admin/{path}", self.edge))
            .header("cookie", "__Host-zeron_session=store-test.user_admin")
            .header("origin", &self.edge)
            .json(&body)
            .send()
            .await
            .unwrap();
        (res.status().as_u16(), res.json().await.unwrap_or_default())
    }

    async fn get(&self, path: &str, bearer: Option<&str>) -> (u16, Value) {
        let mut req = self.http.get(format!("{}{path}", self.edge));
        if let Some(token) = bearer {
            req = req.bearer_auth(token);
        }
        let res = req.send().await.unwrap();
        (res.status().as_u16(), res.json().await.unwrap_or_default())
    }
}

impl Device {
    async fn call(&self, method: &'static str, params: Value) -> Result<Value, String> {
        self.client.call(method, params).await.map_err(|e| e.to_string())
    }

    async fn item(&self, id: &str) -> Value {
        let list = self.call(methods::LIST_PLUGINS, json!({})).await.unwrap();
        list["plugins"].as_array().unwrap().iter().find(|p| p["id"] == id).cloned().unwrap_or(Value::Null)
    }
}

const ICON: &str = "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 16 16\"><circle cx=\"8\" cy=\"8\" r=\"6\" fill=\"currentColor\"/></svg>";

/// A plugin of `device`'s own, made to meet the store's rules at `version`.
async fn author_plugin(device: &Device, id: &str, version: &str, permissions: &[&str]) -> PathBuf {
    let made = device.call(methods::NEW_PLUGIN, json!({ "id": id, "name": "Focus Board", "permissions": permissions })).await.unwrap();
    let dir = PathBuf::from(made["dir"].as_str().unwrap());
    set_version(&dir, version, permissions, "- First.");
    std::fs::write(dir.join("icon.svg"), ICON).unwrap();
    std::fs::write(dir.join("README.md"), "# Focus Board\n\nWhat you're on, in one place.\n").unwrap();
    dir
}

/// The plugin at `dir` at `version`, asking `permissions`, with `notes`.
fn set_version(dir: &Path, version: &str, permissions: &[&str], notes: &str) {
    let path = dir.join("zeron-plugin.json");
    let mut manifest: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    manifest.as_object_mut().unwrap().remove("signature");
    manifest["version"] = json!(version);
    manifest["permissions"] = json!(permissions);
    manifest["description"] = json!("A board of what you're focused on.");
    manifest["icon"] = json!("icon.svg");
    manifest["categories"] = json!(["productivity"]);
    std::fs::write(&path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
    let log = std::fs::read_to_string(dir.join("CHANGELOG.md")).unwrap_or_else(|_| "# Changes\n".into());
    std::fs::write(dir.join("CHANGELOG.md"), format!("{log}\n## {version}\n\n{notes}\n")).unwrap();
}

/// Publish `id` from `device` as the person does: checked, then submitted
/// with the ticket the check gave.
async fn publish(device: &Device, id: &str) -> Value {
    // (The watcher re-signs an own plugin as it's saved: let it settle.)
    tokio::time::sleep(Duration::from_millis(800)).await;
    let prepared = device.call(methods::PREPARE_PLUGIN_PUBLISH, json!({ "id": id })).await.unwrap();
    assert_eq!(prepared["ok"], true, "FAIL {id} doesn't meet the store's rules: {prepared}");
    device
        .call(methods::PUBLISH_PLUGIN, json!({ "id": id, "hash": prepared["hash"], "ticket": prepared["ticket"] }))
        .await
        .unwrap()
}

async fn wait_for(what: &str, mut ready: impl AsyncFnMut() -> bool) {
    let started = Instant::now();
    while !ready().await {
        assert!(started.elapsed() < Duration::from_secs(20), "FAIL {what}");
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

const ID: &str = "focus-board-e2e";

/// How many installs the store counts for the plugin.
async fn installs(device: &Device) -> u64 {
    let detail = device.call(methods::STORE_PLUGIN, json!({ "id": ID })).await.unwrap();
    detail["installs"].as_u64().unwrap_or(0)
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "run by scripts/plugin-store-e2e/run.sh, against a local edge"]
async fn the_plugin_store_end_to_end() {
    let Some(e) = env() else { panic!("E2E_EDGE_URL, E2E_ZERON_BIN and E2E_WORK_DIR: run scripts/plugin-store-e2e/run.sh") };

    // ── Step 1: the author publishes from the engine: pending ───────────
    let laptop = e.device("ada-laptop").await;
    e.sign_in(&laptop, "user_ada").await;
    e.claim_username(&laptop, "ada-e2e").await;
    author_plugin(&laptop, ID, "1.0.0", &["ui.add"]).await;
    let submitted = publish(&laptop, ID).await;
    assert_eq!((submitted["ok"].as_bool(), submitted["status"].as_str()), (Some(true), Some("pending")), "FAIL 1 publish: {submitted}");
    let mine = laptop.call(methods::STORE_MY_PLUGINS, json!({})).await.unwrap();
    let ours = mine["plugins"].as_array().unwrap().iter().find(|p| p["id"] == ID).cloned().unwrap_or_default();
    assert_eq!(ours["versions"][0]["status"], "pending", "FAIL 1 my plugins: {mine}");
    pass("1", "the author publishes from the engine; the version is pending");

    // ── Step 2: the author's other device installs it; nobody else can ──
    let desk = e.device("ada-desk").await;
    e.sign_in(&desk, "user_ada").await;
    let installed = desk.call(methods::INSTALL_PLUGIN, json!({ "store": ID })).await.unwrap_or_else(|e| panic!("FAIL 2 the author's second device: {e}"));
    let item = &installed["plugin"];
    assert_eq!((item["version"].as_str(), item["author"]["username"].as_str()), (Some("1.0.0"), Some("ada-e2e")), "FAIL 2: {installed}");
    assert_eq!(item["source"]["kind"], "store", "FAIL 2: {installed}");
    let phone = e.device("bob-phone").await;
    e.sign_in(&phone, "user_bob").await;
    e.claim_username(&phone, "bob-e2e").await;
    let refused = phone.call(methods::INSTALL_PLUGIN, json!({ "store": ID })).await.unwrap_err();
    assert!(refused.contains("not_in_store"), "FAIL 2 someone else installed a pending version: {refused}");
    let bob = e.bearer(&phone).await;
    let (status, _) = e.get(&format!("/store/v1/plugins/{ID}/versions/1.0.0/package"), Some(&bob)).await;
    assert_eq!(status, 404, "FAIL 2 someone else fetched a pending package");
    let (status, _) = e.get(&format!("/store/v1/plugins/{ID}/versions/1.0.0/package"), None).await;
    assert_eq!(status, 404, "FAIL 2 anyone fetched a pending package");
    pass("2", "the author's other device installs the version in review; another account can't see or fetch it");

    // ── Step 3: approved, listed, found, installed by id, counted once ──
    let (status, body) = e.admin(&format!("submissions/{ID}/1.0.0/approve"), json!({})).await;
    assert_eq!(status, 200, "FAIL 3 approve: {body}");
    let found = phone.call(methods::STORE_SEARCH, json!({ "q": "focus board" })).await.unwrap();
    assert!(found["items"].as_array().unwrap().iter().any(|c| c["id"] == ID), "FAIL 3 search: {found}");
    let installed = phone.call(methods::INSTALL_PLUGIN, json!({ "store": ID })).await.unwrap_or_else(|e| panic!("FAIL 3 install by id: {e}"));
    assert_eq!(installed["plugin"]["author"]["username"], "ada-e2e", "FAIL 3: {installed}");
    assert_eq!(installed["waiting"], true, "FAIL 3 someone else's plugin waits for the person: {installed}");
    wait_for("3 the install was never counted", async || installs(&phone).await == 1).await;
    // Removed and installed again by the same account: still one.
    phone.call(methods::REMOVE_PLUGIN, json!({ "id": ID })).await.unwrap();
    phone.call(methods::INSTALL_PLUGIN, json!({ "store": ID })).await.unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(installs(&phone).await, 1, "FAIL 3 an account's second install was counted again");
    pass("3", "an admin approves; it's listed and found; another account installs it by id; counted once");

    drop((laptop, desk, phone));
}
