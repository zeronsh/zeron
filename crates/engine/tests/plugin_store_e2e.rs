//! The plugin store end to end, against the real edge: real `zeron headless`
//! engines (one per device) and the real CLI, driven over their IPC, talking
//! to the edge worker run locally by scripts/plugin-store-e2e/edge.mjs.
//! Run by scripts/plugin-store-e2e/run.sh, which builds both, starts the
//! edge, runs this, and stops everything; ignored otherwise (it needs
//! `E2E_EDGE_JSON`, `E2E_ZERON_BIN` and `E2E_WORK_DIR`).
//!
//! Each check prints `PASS <step> <what>` or `FAIL <step> <what>: why`; the
//! run fails if any check did. Accounts are test sign-ins: `user_ada` (the
//! author, @ada-e2e), `user_bob` (@bob-e2e: someone else, then the new
//! owner), `user_admin` (granted `admin` with the operator token, signed in
//! on the web as staff are).

#![cfg(unix)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use zeron_engine::plugins::signing::{self, SigningKey};
use zeron_engine::plugins::store::deploy_key::{self, DeployKey};
use zeron_engine::plugins::{install, package};
use zeron_rpc::{RpcClient, methods};

// ── The run ─────────────────────────────────────────────────────────────────

struct Env {
    /// The edge, as devices and the CLI reach it.
    edge: String,
    /// The web site, as a browser reaches it, and its origin.
    site: String,
    site_origin: String,
    /// The WorkOS fake's AuthKit page.
    workos: String,
    admin_token: String,
    zeron: PathBuf,
    work: PathBuf,
    http: reqwest::Client,
    failures: Mutex<Vec<String>>,
}

fn env() -> Option<Env> {
    let get = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
    let edge: Value = serde_json::from_slice(&std::fs::read(get("E2E_EDGE_JSON")?).ok()?).ok()?;
    let text = |key: &str| edge[key].as_str().map(str::to_owned);
    Some(Env {
        edge: text("url")?,
        site: text("site")?,
        site_origin: text("site_origin")?,
        workos: text("workos")?,
        admin_token: text("admin_token")?,
        zeron: PathBuf::from(get("E2E_ZERON_BIN")?),
        work: PathBuf::from(get("E2E_WORK_DIR")?),
        // (A fresh connection each time: the edge may answer a refusal before
        // reading its body and close the connection a pool would reuse.)
        http: reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).pool_max_idle_per_host(0).build().unwrap(),
        failures: Mutex::new(Vec::new()),
    })
}

impl Env {
    /// One check: `PASS` or `FAIL`, the run going on either way.
    fn check(&self, step: &str, what: &str, ok: bool, detail: impl std::fmt::Display) {
        if ok {
            // (What the edge said, when it's short: the words matter too.)
            let said = detail.to_string();
            if !said.is_empty() && said.len() <= 160 && !said.contains('\n') {
                println!("PASS {step} {what} [{said}]");
            } else {
                println!("PASS {step} {what}");
            }
        } else {
            let line = format!("FAIL {step} {what}: {detail}");
            println!("{line}");
            self.failures.lock().unwrap().push(line);
        }
    }

    /// A check the run can't go on without.
    fn must(&self, step: &str, what: &str, ok: bool, detail: impl std::fmt::Display) {
        self.check(step, what, ok, &detail);
        assert!(ok, "FAIL {step} {what}: {detail}");
    }
}

/// A port nothing listens on yet.
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

// ── Devices ─────────────────────────────────────────────────────────────────

/// One device: a `zeron headless` on its own scratch home and data folder.
struct Device {
    name: String,
    home: PathBuf,
    data: PathBuf,
    port: u16,
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
            // (Two engines on one machine: each its own sign-in callback.)
            .env("ZERON_CALLBACK_PORT", free_port().to_string())
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
        Device { name: name.into(), home, data, port, child, client }
    }

    /// `user` signs in on `device` (a test code), and its signing key is
    /// registered with the ticket that sign-in earns.
    async fn sign_in(&self, device: &Device, user: &str) {
        let url = device.call(methods::SIGN_IN_HEADLESS, json!({})).await.unwrap();
        let state = query(url["url"].as_str().unwrap(), "state").unwrap();
        device
            .call(methods::COMPLETE_SIGN_IN, json!({ "code": format!("{state}.code:{user}") }))
            .await
            .unwrap_or_else(|e| panic!("FAIL {} couldn't sign in as {user}: {e}", device.name));
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

    /// A fresh sign-in on `device` as `user` (`StepUp`): the browser it opens,
    /// played here, lands on the engine's own loopback callback.
    async fn step_up(&self, device: &Device, user: &str) {
        let url = device.call(methods::STEP_UP, json!({})).await.unwrap();
        let url = url["url"].as_str().unwrap();
        let redirect = query(url, "redirect_uri").unwrap();
        let state = query(url, "state").unwrap();
        let res = self.http.get(format!("{redirect}?code=code:{user}&state={state}")).send().await.unwrap();
        assert!(res.status().is_success() || res.status().is_redirection(), "FAIL {}'s step-up callback: {}", device.name, res.status());
    }

    async fn bearer(&self, device: &Device) -> String {
        device.call(methods::ACCESS_TOKEN, json!({})).await.unwrap()["token"].as_str().unwrap().to_owned()
    }

    async fn claim_username(&self, device: &Device, username: &str) {
        let token = self.bearer(device).await;
        let res = self.http.post(format!("{}/username", self.edge)).bearer_auth(token).json(&json!({ "username": username })).send().await.unwrap();
        let status = res.status();
        assert!(status.is_success(), "FAIL claiming {username}: {status} {}", res.text().await.unwrap_or_default());
    }

    async fn get(&self, path: &str, bearer: Option<&str>) -> (u16, Value) {
        let mut req = self.http.get(format!("{}{path}", self.edge));
        if let Some(token) = bearer {
            req = req.bearer_auth(token);
        }
        let res = req.send().await.unwrap();
        (res.status().as_u16(), res.json().await.unwrap_or_default())
    }

    /// Run the CLI with only what a CI job has: a scratch home, the edge,
    /// and `extra`.
    fn cli(&self, name: &str, args: &[&str], extra: &[(&str, &str)]) -> Output {
        let home = self.work.join(name);
        std::fs::create_dir_all(&home).unwrap();
        let mut command = self.command(&home, &home.join("data"));
        command.env("ZERON_IPC_PORT", free_port().to_string());
        for (k, v) in extra {
            command.env(k, v);
        }
        command.arg("plugin").args(args).output().unwrap()
    }

    /// The CLI through `device`'s running Zeron (its session).
    fn cli_on(&self, device: &Device, args: &[&str]) -> Output {
        let mut command = self.command(&device.home, &device.data);
        command.env("ZERON_IPC_PORT", device.port.to_string()).arg("plugin").args(args).output().unwrap()
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

    /// Allowed as Zeron's consent window does: a review for it as shown.
    async fn allow(&self, id: &str) -> Value {
        let hash = self.item(id).await["hash"].clone();
        let review = self.call(methods::REVIEW_PLUGIN, json!({ "id": id, "hash": hash })).await.unwrap()["review"].clone();
        self.call(methods::DECIDE_PLUGIN, json!({ "id": id, "allow": true, "hash": hash, "review": review })).await.unwrap()
    }
}

fn query(url: &str, name: &str) -> Option<String> {
    url::Url::parse(url).ok()?.query_pairs().find(|(k, _)| k == name).map(|(_, v)| v.into_owned())
}

fn text(out: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
}

// ── Staff on the web ────────────────────────────────────────────────────────

/// A web session: staff sign in on the site, never with a bearer.
struct Staff {
    cookie: Mutex<String>,
    csrf: String,
}

/// The `name=value` of `name` among a response's cookies.
fn cookie_of(res: &reqwest::Response, name: &str) -> Option<String> {
    res.headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .filter_map(|v| v.split(';').next())
        .find(|pair| pair.starts_with(&format!("{name}=")))
        .map(str::to_owned)
}

const SESSION: &str = "__Host-zeron_session";

impl Env {
    /// `user` signs in on the site, as a browser does: login, AuthKit's page
    /// (the fake's, at the same path and query, the PKCE challenge kept), the
    /// callback with the login cookie; then the session and its CSRF token.
    async fn web_sign_in(&self, user: &str) -> Staff {
        let login = self.http.get(format!("{}/auth/web/login?return_to=/admin", self.site)).send().await.unwrap();
        assert_eq!(login.status().as_u16(), 302, "FAIL web login: {}", login.text().await.unwrap_or_default());
        let login_cookie = cookie_of(&login, "__Host-zeron_web_login").expect("a login cookie");
        let authorize = url::Url::parse(login.headers()["location"].to_str().unwrap()).unwrap();
        assert_eq!(authorize.host_str(), Some("api.workos.com"), "FAIL web login went to {authorize}");
        let page = format!("{}{}?{}&e2e_user={user}", self.workos, authorize.path(), authorize.query().unwrap());
        let back = self.http.get(page).send().await.unwrap();
        let callback = back.headers()["location"].to_str().unwrap().replace(&self.site_origin, &self.site);
        let signed_in = self.http.get(callback).header("cookie", &login_cookie).send().await.unwrap();
        let status = signed_in.status().as_u16();
        let Some(session) = cookie_of(&signed_in, SESSION) else {
            panic!("FAIL web callback ({status}): {}", signed_in.text().await.unwrap_or_default());
        };
        let res = self.http.get(format!("{}/auth/web/session", self.site)).header("cookie", &session).send().await.unwrap();
        let cookie = cookie_of(&res, SESSION).unwrap_or(session);
        let body: Value = res.json().await.unwrap();
        Staff { cookie: Mutex::new(cookie), csrf: body["csrf"].as_str().unwrap_or_default().to_owned() }
    }

    /// An admin's change, as the site sends it (through the site port, as
    /// the site's service binding forwards a browser's request): the
    /// session's cookie (the newest: staff sessions rotate), Origin and the
    /// CSRF token.
    async fn admin_as(&self, staff: &Staff, path: &str, body: Value, origin: &str, bearer: Option<&str>) -> (u16, Value) {
        let cookie = staff.cookie.lock().unwrap().clone();
        let mut req = self
            .http
            .post(format!("{}/store/v1/admin/{path}", self.site))
            .header("cookie", cookie)
            .header("origin", origin)
            .header("x-csrf-token", &staff.csrf)
            .json(&body);
        if let Some(token) = bearer {
            req = req.bearer_auth(token);
        }
        let res = req.send().await.unwrap();
        if let Some(newer) = cookie_of(&res, SESSION) {
            *staff.cookie.lock().unwrap() = newer;
        }
        (res.status().as_u16(), res.json().await.unwrap_or_default())
    }

    async fn admin(&self, staff: &Staff, path: &str, body: Value) -> (u16, Value) {
        self.admin_as(staff, path, body, &self.site_origin.clone(), None).await
    }

    /// An admin's read, as the site sends it: the session's cookie.
    async fn admin_get(&self, staff: &Staff, path: &str) -> (u16, Value) {
        let cookie = staff.cookie.lock().unwrap().clone();
        let res = self.http.get(format!("{}/store/v1/admin/{path}", self.site)).header("cookie", cookie).send().await.unwrap();
        if let Some(newer) = cookie_of(&res, SESSION) {
            *staff.cookie.lock().unwrap() = newer;
        }
        (res.status().as_u16(), res.json().await.unwrap_or_default())
    }
}

// ── Plugins ─────────────────────────────────────────────────────────────────

const ID: &str = "focus-board-e2e";
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

/// A copy of a plugin folder, as a CI job's checkout has it (no dot files).
fn checkout(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        if entry.file_type().unwrap().is_dir() {
            checkout(&entry.path(), &to.join(&name));
        } else {
            std::fs::copy(entry.path(), to.join(&name)).unwrap();
        }
    }
}

/// Publish `id` from `device` as the person does: checked, then submitted
/// with the ticket the check gave.
async fn publish(device: &Device, id: &str) -> Value {
    // (The watcher re-signs an own plugin as it's saved: let it settle.)
    tokio::time::sleep(Duration::from_millis(800)).await;
    let prepared = device.call(methods::PREPARE_PLUGIN_PUBLISH, json!({ "id": id })).await.unwrap();
    assert_eq!(prepared["ok"], true, "FAIL {id} doesn't meet the store's rules: {prepared}");
    device.call(methods::PUBLISH_PLUGIN, json!({ "id": id, "hash": prepared["hash"], "ticket": prepared["ticket"] })).await.unwrap()
}

/// A package of `id` at `version` meeting the store's rules, signed by `key`:
/// `manifest_extra` merged into its manifest, `files_extra` added.
fn store_package(key: &SigningKey, id: &str, version: &str, manifest_extra: Value, files_extra: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    files.insert("init.luau".into(), b"return {}".to_vec());
    files.insert("icon.svg".into(), ICON.as_bytes().to_vec());
    files.insert("README.md".into(), b"# Focus Board\n\nWhat you're on.\n".to_vec());
    files.insert("CHANGELOG.md".into(), format!("## {version}\n\n- Something.\n").into_bytes());
    for (rel, bytes) in files_extra {
        files.insert((*rel).into(), bytes.clone());
    }
    let mut manifest = json!({
        "id": id, "name": "Focus Board", "version": version, "entry": "init.luau", "permissions": ["ui.add"],
        "description": "A board of what you're focused on.", "icon": "icon.svg", "categories": ["productivity"],
    });
    for (k, v) in manifest_extra.as_object().unwrap() {
        manifest[k] = v.clone();
    }
    let manifest = key.sign_manifest(&manifest, &package::hashes(&files));
    package::pack(install::FORMAT, &manifest, &files).unwrap()
}

impl Env {
    /// `body` sent to `path` signed with deploy key `key` at `ts`; the body's
    /// hash claimed as `claimed` (default: its own).
    async fn signed(&self, key: &DeployKey, method: &str, path: &str, body: &[u8], ts: i64, claimed: Option<&[u8]>) -> (u16, Value) {
        let claimed = claimed.unwrap_or(body);
        let hash = signing::sha256_hex(claimed);
        let message = deploy_key::request_message(method, path, &hash, ts);
        let sig = key.signing_key().sign(message.as_bytes());
        let res = self
            .http
            .request(method.parse().unwrap(), format!("{}{path}", self.edge))
            .header("authorization", format!("ZeronKey key={}, ts={ts}, sig={sig}", key.public()))
            .header(deploy_key::CONTENT_SHA256_HEADER, hash)
            .header("content-type", "application/json")
            .body(body.to_vec())
            .send()
            .await
            .unwrap();
        (res.status().as_u16(), res.json().await.unwrap_or_default())
    }

    async fn publish_signed(&self, key: &DeployKey, id: &str, body: &[u8]) -> (u16, Value) {
        self.signed(key, "POST", &format!("/store/v1/plugins/{id}/versions"), body, now_ms(), None).await
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64
}

/// How many installs the store counts for the plugin.
async fn installs(device: &Device) -> u64 {
    let detail = device.call(methods::STORE_PLUGIN, json!({ "id": ID })).await.unwrap();
    detail["installs"].as_u64().unwrap_or(0)
}

async fn wait_for(what: &str, mut ready: impl AsyncFnMut() -> bool) -> bool {
    let started = Instant::now();
    while !ready().await {
        if started.elapsed() > Duration::from_secs(20) {
            println!("(waited 20 s for {what})");
            return false;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    true
}

/// A deploy key made on `device` for `id`, through its real step-up: the
/// secret, as shown once.
async fn deploy_key_on(e: &Env, device: &Device, user: &str, id: &str, name: &str) -> String {
    let mut made = device.call(methods::CREATE_DEPLOY_KEY, json!({ "id": id, "name": name })).await.unwrap();
    if made["error"] == "step_up_required" {
        e.step_up(device, user).await;
        made = device.call(methods::CREATE_DEPLOY_KEY, json!({ "id": id, "name": name })).await.unwrap();
    }
    e.must("4", &format!("{user} makes a deploy key for {id} through a fresh sign-in"), made["ok"] == true, &made);
    made["secret"].as_str().unwrap().to_owned()
}

// ── The steps ───────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
#[ignore = "run by scripts/plugin-store-e2e/run.sh, against a local edge"]
async fn the_plugin_store_end_to_end() {
    let Some(e) = env() else { panic!("E2E_EDGE_JSON, E2E_ZERON_BIN and E2E_WORK_DIR: run scripts/plugin-store-e2e/run.sh") };

    // ── 0: staff ──
    let staff = e.web_sign_in("user_admin").await;
    let grant = e.http.post(format!("{}/admin/roles/grant", e.edge)).bearer_auth(&e.admin_token).json(&json!({ "userId": "user_admin", "role": "admin", "by": "e2e" })).send().await.unwrap();
    e.must("0", "the operator grants admin", grant.status().is_success(), grant.status());
    let staff = if staff.csrf.is_empty() { e.web_sign_in("user_admin").await } else { staff };

    // ── 1: the author publishes from the engine: pending ──
    let laptop = e.device("ada-laptop").await;
    e.sign_in(&laptop, "user_ada").await;
    e.claim_username(&laptop, "ada-e2e").await;
    let ada_dir = author_plugin(&laptop, ID, "1.0.0", &["ui.add"]).await;
    let submitted = publish(&laptop, ID).await;
    e.must("1", "the author publishes from the engine; it's pending", submitted["ok"] == true && submitted["status"] == "pending", &submitted);
    let mine = laptop.call(methods::STORE_MY_PLUGINS, json!({})).await.unwrap();
    let ours = mine["plugins"].as_array().unwrap().iter().find(|p| p["id"] == ID).cloned().unwrap_or_default();
    e.check("1", "my plugins says pending", ours["versions"][0]["status"] == "pending", &mine);

    // ── 2: the author's other device installs it; nobody else can ──
    let desk = e.device("ada-desk").await;
    e.sign_in(&desk, "user_ada").await;
    let installed = desk.call(methods::INSTALL_PLUGIN, json!({ "store": ID })).await;
    let ok = installed.as_ref().is_ok_and(|r| r["plugin"]["version"] == "1.0.0" && r["plugin"]["author"]["username"] == "ada-e2e");
    e.must("2", "the author's other device installs the version in review", ok, format!("{installed:?}"));
    let phone = e.device("bob-phone").await;
    e.sign_in(&phone, "user_bob").await;
    e.claim_username(&phone, "bob-e2e").await;
    let refused = phone.call(methods::INSTALL_PLUGIN, json!({ "store": ID })).await;
    e.check("2", "another account can't install a version in review", refused.as_ref().is_err_and(|r| r.contains("not_in_store")), format!("{refused:?}"));
    let bob = e.bearer(&phone).await;
    let (status, _) = e.get(&format!("/store/v1/plugins/{ID}/versions/1.0.0/package"), Some(&bob)).await;
    e.check("2", "another account can't fetch its package", status == 404, status);
    let (status, _) = e.get(&format!("/store/v1/plugins/{ID}/versions/1.0.0/package"), None).await;
    e.check("2", "nobody signed out can fetch its package", status == 404, status);

    // ── 3: approved, listed, found, installed by id, counted once ──
    let (status, body) = e.admin(&staff, &format!("submissions/{ID}/1.0.0/approve"), json!({})).await;
    e.must("3", "staff approve it on a web session", status == 200, &body);
    let found = phone.call(methods::STORE_SEARCH, json!({ "q": "focus board" })).await.unwrap();
    e.check("3", "search finds it", found["items"].as_array().unwrap().iter().any(|c| c["id"] == ID), &found);
    let detail = phone.call(methods::STORE_PLUGIN, json!({ "id": ID })).await.unwrap();
    e.check("3", "its card names the icon as a path in its package", detail["icon"] == "icon.svg" && detail["version"] == "1.0.0", &detail);
    let icon = phone.call(methods::STORE_FILE, json!({ "id": ID, "version": "1.0.0", "path": "icon.svg" })).await;
    let ok = icon.as_ref().is_ok_and(|f| f["bytes"] == ICON.len() && f["file"].as_str().and_then(|p| std::fs::read(p).ok()).is_some_and(|b| b == ICON.as_bytes()));
    e.check("3", "and the device fetches that file from the package", ok, format!("{icon:?}"));
    let installed = phone.call(methods::INSTALL_PLUGIN, json!({ "store": ID })).await;
    let ok = installed.as_ref().is_ok_and(|r| r["plugin"]["author"]["username"] == "ada-e2e" && r["waiting"] == true);
    e.must("3", "another account installs it by id; it waits for consent", ok, format!("{installed:?}"));
    let counted = wait_for("the install to be counted", async || installs(&phone).await == 1).await;
    e.check("3", "the install is counted", counted, installs(&phone).await);
    phone.call(methods::REMOVE_PLUGIN, json!({ "id": ID })).await.unwrap();
    phone.call(methods::INSTALL_PLUGIN, json!({ "store": ID })).await.unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;
    e.check("3", "the same account's second install isn't counted again", installs(&phone).await == 1, installs(&phone).await);
    e.check("3", "the person allows it", phone.allow(ID).await["status"] == "on", "");
    e.check("3", "so does the author's other device", desk.allow(ID).await["status"] == "on", "");

    // ── 4: a deploy key; CI publishes 1.1.0 with a new permission ──
    let secret = deploy_key_on(&e, &laptop, "user_ada", ID, "GitHub Actions").await;
    let ada_key = DeployKey::from_secret(&secret).unwrap();
    let ci = e.work.join("ci-checkout").join(ID);
    checkout(&ada_dir, &ci);
    set_version(&ci, "1.1.0", &["ui.add", "links.open"], "- Opens links.");
    let ci_arg = ci.display().to_string();
    let out = e.cli("ci", &["publish", &ci_arg], &[("ZERON_DEPLOY_KEY", &secret)]);
    e.must("4", "the CLI publishes 1.1.0 with only ZERON_DEPLOY_KEY", out.status.success() && text(&out).contains("Submitted"), text(&out));
    e.check("4", "the CLI never says the secret", !text(&out).contains(&secret[4..]), "");
    let (status, body) = e.admin(&staff, &format!("submissions/{ID}/1.1.0/approve"), json!({})).await;
    e.must("4", "staff approve 1.1.0", status == 200, &body);
    for device in [&desk, &phone] {
        let checked = device.call(methods::CHECK_PLUGIN_UPDATES, json!({ "id": ID })).await.unwrap();
        let update = checked["updates"].as_array().and_then(|u| u.iter().find(|u| u["id"] == ID)).cloned().unwrap_or_default();
        let ok = update["version"] == "1.1.0" && update["notes"] == "- Opens links." && update["added"] == json!(["links.open"]);
        e.check("4", &format!("{} is offered 1.1.0 with its notes and the permission it adds", device.name), ok, &checked);
        let (_, review) = e.admin_get(&staff, &format!("submissions/{ID}/1.1.0")).await;
        let diff = &review["permissions_diff"];
        let ok = diff["added"] == update["added"] && diff["removed"] == update["removed"].as_array().map_or(json!([]), |r| json!(r));
        e.check("4", &format!("the review's permission diff says what {}'s offer says", device.name), ok, format!("store {diff} / engine {update}"));
        let updated = device.call(methods::UPDATE_PLUGIN, json!({ "id": ID })).await;
        let ok = updated.as_ref().is_ok_and(|r| r["plugin"]["version"] == "1.1.0" && r["plugin"]["status"] == "pending");
        e.check("4", &format!("{} updates; the new permission asks again", device.name), ok, format!("{updated:?}"));
        e.check("4", &format!("{} allows it", device.name), device.allow(ID).await["status"] == "on", "");
    }

    // ── 5: yank, unyank; a takedown with a block ──
    let out = e.cli_on(&laptop, &["yank", ID, "1.1.0"]);
    e.check("5", "the author yanks 1.1.0 (the CLI, with the session)", out.status.success(), text(&out));
    let latest = phone.call(methods::STORE_PLUGIN, json!({ "id": ID })).await.unwrap();
    e.check("5", "a yanked version is no longer the one new installs get", latest["version"] == "1.0.0", &latest["version"]);
    let out = e.cli_on(&laptop, &["unyank", ID, "1.1.0"]);
    e.check("5", "the author unyanks it", out.status.success(), text(&out));
    let latest = phone.call(methods::STORE_PLUGIN, json!({ "id": ID })).await.unwrap();
    e.check("5", "unyanked, it's the latest again", latest["version"] == "1.1.0", &latest["version"]);
    let (status, body) = e.admin(&staff, &format!("plugins/{ID}/versions/1.1.0/remove"), json!({ "block": true, "reason": "It sent your chats somewhere." })).await;
    e.must("5", "staff take 1.1.0 down with a block", status == 200, &body);
    phone.call(methods::CHECK_PLUGIN_UPDATES, json!({ "id": ID })).await.unwrap();
    let item = phone.item(ID).await;
    e.check("5", "the device turns it off with the reason", item["status"] == "off" && item["blocked"]["reason"] == "It sent your chats somewhere.", &item);
    let on = phone.call(methods::SET_PLUGIN_OFF, json!({ "id": ID, "off": false })).await.unwrap();
    e.check("5", "the person turns it back on", on["status"] == "on" && on["blocked"]["overridden"] == true, &on);
    phone.call(methods::CHECK_PLUGIN_UPDATES, json!({ "id": ID })).await.unwrap();
    e.check("5", "the next check leaves it on", phone.item(ID).await["status"] == "on", "");

    // ── 6: a transfer ──
    let bob_bearer = e.bearer(&phone).await;
    let lookup = |key: String| {
        let (e, bearer) = (&e, bob_bearer.clone());
        async move { e.get(&format!("/plugin-keys/{key}?plugin={ID}"), Some(&bearer)).await }
    };
    let (_, before) = lookup(ada_key.public()).await;
    e.check("6", "before: ada's deploy key reads as verified for its plugin", before["verified"] == true, &before);
    let (status, body) = e.admin(&staff, &format!("plugins/{ID}/transfer"), json!({ "to_username": "bob-e2e" })).await;
    e.must("6", "staff transfer it to @bob-e2e", status == 200, &body);
    set_version(&ci, "1.2.0", &["ui.add", "links.open"], "- After.");
    let out = e.cli("ci", &["publish", &ci_arg], &[("ZERON_DEPLOY_KEY", &secret)]);
    e.check("6", "the old owner's deploy key is refused on publish (exit 3)", out.status.code() == Some(3), text(&out));
    let (_, after) = lookup(ada_key.public()).await;
    e.check("6", "the old owner's deploy key reads unverified for the plugin", after["verified"] == false, &after);
    let bob_laptop = e.device("bob-laptop").await;
    e.sign_in(&bob_laptop, "user_bob").await;
    author_plugin(&bob_laptop, ID, "1.2.0", &["ui.add", "links.open"]).await;
    let submitted = publish(&bob_laptop, ID).await;
    e.must("6", "the new owner publishes 1.2.0", submitted["ok"] == true, &submitted);
    let (status, body) = e.admin(&staff, &format!("submissions/{ID}/1.2.0/approve"), json!({})).await;
    e.must("6", "staff approve 1.2.0", status == 200, &body);
    let checked = desk.call(methods::CHECK_PLUGIN_UPDATES, json!({ "id": ID })).await.unwrap();
    let update = checked["updates"].as_array().and_then(|u| u.iter().find(|u| u["id"] == ID)).cloned().unwrap_or_default();
    e.check("6", "the update says it has a new owner", update["version"] == "1.2.0" && update["new_owner"] == true, &checked);
    let updated = desk.call(methods::UPDATE_PLUGIN, json!({ "id": ID })).await;
    let ok = updated.as_ref().is_ok_and(|r| r["plugin"]["author"]["username"] == "bob-e2e" && r["plugin"]["status"] == "pending");
    e.check("6", "updated, it's by @bob-e2e and asks again", ok, format!("{updated:?}"));

    // ── 7: the refusals, each against the real edge ──
    let bob_secret = deploy_key_on(&e, &bob_laptop, "user_bob", ID, "CI").await;
    let bob_key = DeployKey::from_secret(&bob_secret).unwrap();
    let signer = bob_key.signing_key();
    let path = format!("/store/v1/plugins/{ID}/versions");
    // (1.3.0 and 1.3.5 carry tools, for the permission diff below.)
    let tools_before = json!({ "permissions": ["ui.add", "tools"], "tools": [
        { "name": "lookup", "description": "Looks a word up." },
        { "name": "old", "description": "Goes away." },
        { "name": "shape", "description": "Same words.", "input": { "type": "object", "properties": { "a": { "type": "string" } } } },
    ] });
    let tools_after = json!({ "permissions": ["ui.add", "tools"], "tools": [
        { "name": "lookup", "description": "Looks a word up, and its plural.", "input": { "type": "object" } },
        { "name": "new", "description": "Arrives." },
        { "name": "shape", "description": "Same words.", "input": { "type": "object", "properties": { "b": { "type": "string" } } } },
    ] });
    let pkg = store_package(signer, ID, "1.3.0", tools_before, &[]);
    let ts = now_ms();
    let (first, body) = e.signed(&bob_key, "POST", &path, &pkg, ts, None).await;
    e.check("7", "a signed publish lands", first == 201, &body);
    let (status, body) = e.signed(&bob_key, "POST", &path, &pkg, ts, None).await;
    e.check("7", "the same signed request again is refused as a replay", status == 401 && body["error"] == "replayed_request", format!("{status} {body}"));
    let other = store_package(signer, ID, "1.3.1", json!({}), &[]);
    let (status, body) = e.signed(&bob_key, "POST", &path, &other, now_ms(), Some(&pkg)).await;
    e.check("7", "a body that isn't the one its hash names is refused", status == 400 && body["error"] == "body_mismatch", format!("{status} {body}"));
    let elsewhere = store_package(signer, "other-e2e", "1.0.0", json!({}), &[]);
    let (status, body) = e.publish_signed(&bob_key, "other-e2e", &elsewhere).await;
    e.check("7", "a key for another plugin is refused", status == 403 && body["error"] == "key_not_allowed", format!("{status} {body}"));
    let (status, body) = e.publish_signed(&bob_key, ID, &store_package(signer, ID, "1.2.5", json!({}), &[])).await;
    e.check("7", "a version that isn't newer is refused", body["error"] == "not_newer" || body["error"] == "version_exists", format!("{status} {body}"));
    let big = store_package(signer, ID, "1.3.2", json!({}), &[("data.bin", vec![7u8; 16 * 1024 * 1024])]);
    let (status, body) = e.publish_signed(&bob_key, ID, &big).await;
    e.check("7", "a package over the store's limit is refused", status == 413 || body["error"] == "too_large", format!("{status} {body}"));
    let colliding = store_package(signer, ID, "1.3.3", json!({}), &[("Notes.txt", b"a".to_vec()), ("notes.txt", b"b".to_vec())]);
    let (status, body) = e.publish_signed(&bob_key, ID, &colliding).await;
    e.check("7", "a package whose paths collide on macOS and Windows is refused", status >= 400, format!("{status} {body}"));
    let deep_text = String::from_utf8(store_package(signer, ID, "1.3.4", json!({}), &[])).unwrap();
    let nest = format!("{}{}", "[".repeat(127), "]".repeat(127));
    let deep = format!("{{\"x\":{nest},{}", &deep_text[1..]).into_bytes();
    let (status, body) = e.publish_signed(&bob_key, ID, &deep).await;
    e.check("7", "a package nesting one level too deep is refused", status == 400, format!("{status} {body}"));
    let retried = store_package(signer, ID, "1.3.5", tools_after, &[]);
    let (a, first_body) = e.publish_signed(&bob_key, ID, &retried).await;
    let (b, second_body) = e.publish_signed(&bob_key, ID, &retried).await;
    e.check("7", "a publish retried with the same bytes gets the same answer", a == b && first_body == second_body, format!("{a} {first_body} / {b} {second_body}"));
    let (status, _) = e.get(&format!("/store/v1/plugins/{ID}/versions/1.3.5/package"), Some(&e.bearer(&bob_laptop).await)).await;
    e.check("7", "and its package is kept", status == 200, status);
    let incompatible = store_package(signer, ID, "1.4.0", json!({ "engines": { "zeron": ">=99.0.0" } }), &[]);
    let (status, body) = e.publish_signed(&bob_key, ID, &incompatible).await;
    e.check("7", "a version for another Zeron is taken by the store", status == 201, format!("{status} {body}"));
    let (status, body) = e.admin(&staff, &format!("submissions/{ID}/1.3.0/approve"), json!({})).await;
    e.check("7", "staff approve 1.3.0", status == 200, &body);
    desk.call(methods::CHECK_PLUGIN_UPDATES, json!({ "id": ID })).await.unwrap();
    let updated = desk.call(methods::UPDATE_PLUGIN, json!({ "id": ID })).await;
    let ok = updated.as_ref().is_ok_and(|r| r["plugin"]["version"] == "1.3.0") && desk.allow(ID).await["status"] == "on";
    e.check("7", "a device takes 1.3.0, with its tools", ok, format!("{updated:?}"));
    // (Before 1.4.0 is approved: the review's base is then 1.3.0, what the device allowed.)
    let (_, review) = e.admin_get(&staff, &format!("submissions/{ID}/1.3.5")).await;
    let (status, body) = e.admin(&staff, &format!("submissions/{ID}/1.3.5/approve"), json!({})).await;
    e.check("7", "staff approve 1.3.5", status == 200, &body);
    let checked = desk.call(methods::CHECK_PLUGIN_UPDATES, json!({ "id": ID })).await.unwrap();
    let offer = checked["updates"].as_array().and_then(|u| u.iter().find(|u| u["id"] == ID)).cloned().unwrap_or_default();
    let (store, engine) = (&review["permissions_diff"], &offer["digest_diff"]);
    let same = ["tools_added", "tools_changed", "tools_removed", "added_commands", "removed_commands", "ui_change_added", "ui_replace_added", "network_added"]
        .iter()
        .all(|k| store[k] == engine[k])
        && store["added"] == engine["permissions_added"]
        && store["added"] == offer["added"];
    let expected = engine["tools_changed"]
        == json!([
            { "name": "lookup", "description_before": "Looks a word up.", "description_after": "Looks a word up, and its plural.", "input_changed": false },
            { "name": "shape", "description_before": "Same words.", "description_after": "Same words.", "input_changed": true },
        ])
        && engine["tools_added"] == json!([{ "name": "new", "description": "Arrives." }])
        && engine["tools_removed"] == json!(["old"]);
    e.check("7", "the review's diff of 1.3.5 says what the device's offer says (tools added, changed, removed)", offer["version"] == "1.3.5" && same && expected, format!("store {store} / engine {offer}"));
    let (status, body) = e.admin(&staff, &format!("submissions/{ID}/1.4.0/approve"), json!({})).await;
    e.check("7", "staff approve 1.4.0", status == 200, &body);
    let refused = phone.call(methods::INSTALL_PLUGIN, json!({ "store": ID, "version": "1.4.0" })).await;
    e.check("7", "a device refuses a version for another Zeron", refused.as_ref().is_err_and(|r| r.contains("incompatible")), format!("{refused:?}"));
    let checked = phone.call(methods::CHECK_PLUGIN_UPDATES, json!({ "id": ID })).await.unwrap();
    let offered: Vec<&str> = checked["updates"].as_array().into_iter().flatten().filter_map(|u| u["version"].as_str()).collect();
    e.check("7", "the update check never offers it", !offered.contains(&"1.4.0"), &checked);
    let (status, body) = e.admin_as(&staff, &format!("submissions/{ID}/1.3.0/reject"), json!({ "reason": "other", "message": "x" }), &e.site_origin.clone(), Some(&e.bearer(&laptop).await)).await;
    e.check("7", "a bearer on an admin route is refused, a valid session beside it", status == 403, format!("{status} {body}"));
    let (status, body) = e.admin_as(&staff, &format!("plugins/{ID}/feature"), json!({ "featured": true }), "https://evil.example", None).await;
    e.check("7", "a cross-site change with a valid session is refused", status == 403 && body["error"] == "bad_csrf", format!("{status} {body}"));
    set_version(&ci, "1.5.0", &["ui.add", "links.open"], "- Late.");
    let out = e.cli("ci", &["publish", &ci_arg], &[("ZERON_DEPLOY_KEY", &secret)]);
    e.check("7", "the former owner's deploy key is still refused on publish", out.status.code() == Some(3), text(&out));
    let (_, after) = lookup(ada_key.public()).await;
    e.check("7", "and still reads unverified for the plugin", after["verified"] == false, &after);

    // ── 8: reports, and the counted install, as writes ──
    let report = json!({ "id": ID, "reason": "broken", "text": "It doesn't open." });
    let first = desk.call(methods::STORE_REPORT, report.clone()).await.unwrap();
    e.must("8", "a person reports it from the engine", first["ok"] == true && first["status"] == "open", &first);
    let again = desk.call(methods::STORE_REPORT, report).await.unwrap();
    e.check("8", "reporting again while it's open is the same report", again["ok"] == true && again["id"] == first["id"], &again);
    let (status, listed) = e.admin_get(&staff, "reports?status=open").await;
    let seen = listed["reports"].as_array().and_then(|r| r.iter().find(|r| r["id"] == first["id"])).cloned().unwrap_or_default();
    let ok = status == 200 && seen["plugin"]["id"] == ID && seen["reason"] == "broken" && seen["text"] == "It doesn't open.";
    e.check("8", "staff see it, with its reason and the person's words", ok, format!("{status} {listed}"));
    let reports = format!("/store/v1/plugins/{ID}/reports");
    let body = serde_json::to_vec(&json!({ "reason": "other" })).unwrap();
    let (status, answer) = e.signed(&bob_key, "POST", &reports, &body, now_ms(), None).await;
    e.check("8", "a deploy key can't report", status == 403 && answer["error"] == "key_not_allowed", format!("{status} {answer}"));
    let installed = format!("/store/v1/plugins/{ID}/installed");
    let (status, answer) = e.signed(&bob_key, "POST", &installed, b"{}", now_ms(), None).await;
    e.check("8", "nor count an install", status == 403 && answer["error"] == "key_not_allowed", format!("{status} {answer}"));
    let cookie = staff.cookie.lock().unwrap().clone();
    let res = e.http.post(format!("{}{reports}", e.site)).header("cookie", cookie).json(&json!({ "reason": "other" })).send().await.unwrap();
    let status = res.status().as_u16();
    let answer: Value = res.json().await.unwrap_or_default();
    e.check("8", "a report on a web session without the site's Origin is refused", status == 403 && answer["error"] == "bad_csrf", format!("{status} {answer}"));
    // (Someone who isn't staff, on the site: the route's own session check.)
    let carol = e.web_sign_in("user_carol").await;
    let web_report = async |csrf: &str| {
        let cookie = carol.cookie.lock().unwrap().clone();
        let res = e
            .http
            .post(format!("{}{reports}", e.site))
            .header("cookie", cookie)
            .header("origin", &e.site_origin)
            .header("x-csrf-token", csrf)
            .json(&json!({ "reason": "misleading", "text": "Not what it says." }))
            .send()
            .await
            .unwrap();
        if let Some(newer) = cookie_of(&res, SESSION) {
            *carol.cookie.lock().unwrap() = newer;
        }
        (res.status().as_u16(), res.json::<Value>().await.unwrap_or_default())
    };
    let (status, answer) = web_report("not-the-session-token").await;
    e.check("8", "a report on a web session with a wrong CSRF token is refused", status == 403 && answer["error"] == "bad_csrf", format!("{status} {answer}"));
    let (status, carols) = web_report(&carol.csrf.clone()).await;
    e.check("8", "with the session's token, it lands", status == 201 && carols["status"] == "open" && carols["id"] != first["id"], format!("{status} {carols}"));
    let id = first["id"].as_i64().unwrap_or_default();
    let (status, body) = e.admin(&staff, &format!("reports/{id}/resolve"), json!({ "resolution": "dismissed", "note": "Works here." })).await;
    e.check("8", "staff resolve it", status == 200, &body);
    let (_, open) = e.admin_get(&staff, "reports?status=open").await;
    let gone = open["reports"].as_array().is_some_and(|r| r.iter().all(|r| r["id"] != first["id"]) && r.iter().any(|r| r["id"] == carols["id"]));
    e.check("8", "and it leaves the open reports (the other one stays)", gone, &open);
    // Signing out on the site: Origin and the CSRF token, as any write.
    let logout = async |origin: &str, csrf: &str| {
        let cookie = carol.cookie.lock().unwrap().clone();
        let res = e.http.post(format!("{}/auth/web/logout", e.site)).header("cookie", cookie).header("origin", origin).header("x-csrf-token", csrf).send().await.unwrap();
        (res.status().as_u16(), res.json::<Value>().await.unwrap_or_default())
    };
    let (status, answer) = logout(&e.site_origin.clone(), "not-the-session-token").await;
    e.check("8", "signing out with a wrong CSRF token is refused", status == 403 && answer["error"] == "bad_csrf", format!("{status} {answer}"));
    let (status, answer) = logout("https://evil.example", &carol.csrf.clone()).await;
    e.check("8", "and from another origin", status == 403 && answer["error"] == "bad_csrf", format!("{status} {answer}"));
    let (status, answer) = logout(&e.site_origin.clone(), &carol.csrf.clone()).await;
    e.check("8", "with both, it signs out", status == 200 && answer["ok"] == true, format!("{status} {answer}"));
    let (status, answer) = web_report(&carol.csrf.clone()).await;
    e.check("8", "and the ended session can't report", status == 401, format!("{status} {answer}"));

    // ── 9: featured ──
    const OTHER: &str = "notes-board-e2e";
    let notes = author_plugin(&bob_laptop, OTHER, "1.0.0", &["ui.add"]).await;
    let manifest_path = notes.join("zeron-plugin.json");
    let mut manifest: Value = serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    manifest["name"] = json!("Notes Board");
    std::fs::write(&manifest_path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
    let submitted = publish(&bob_laptop, OTHER).await;
    e.must("9", "a second plugin is published", submitted["ok"] == true, &submitted);
    let (status, body) = e.admin(&staff, &format!("submissions/{OTHER}/1.0.0/approve"), json!({})).await;
    e.must("9", "and approved", status == 200, &body);
    let ids = |reply: &Value| reply["items"].as_array().into_iter().flatten().filter_map(|c| c["id"].as_str().map(str::to_owned)).collect::<Vec<_>>();
    let featured = async || {
        let engine = phone.call(methods::STORE_SEARCH, json!({ "featured": true })).await.unwrap();
        let (_, edge) = e.get("/store/v1/plugins?featured=true", None).await;
        (ids(&engine), ids(&edge), engine)
    };
    let (status, body) = e.admin(&staff, &format!("plugins/{ID}/feature"), json!({ "featured": true })).await;
    e.must("9", &format!("staff feature {ID}"), status == 200, &body);
    let (engine, edge, reply) = featured().await;
    let only = engine == [ID] && engine == edge && reply["items"][0]["featured"] == true;
    e.check("9", "a featured search lists only the featured one", only, format!("engine {engine:?} / edge {edge:?}"));
    let (_, every) = e.get("/store/v1/plugins", None).await;
    e.check("9", "while the full list has both", ids(&every).contains(&OTHER.to_owned()) && ids(&every).contains(&ID.to_owned()), &every);
    let (status, body) = e.admin(&staff, &format!("plugins/{OTHER}/feature"), json!({ "featured": true })).await;
    e.must("9", &format!("staff feature {OTHER} too"), status == 200, &body);
    let (engine, edge, _) = featured().await;
    let both = engine.len() == 2 && engine.contains(&ID.to_owned()) && engine.contains(&OTHER.to_owned()) && engine == edge;
    e.check("9", "a featured search lists both, in the edge's order", both, format!("engine {engine:?} / edge {edge:?}"));

    // ── 10: a search's total; the site's own ids ──
    let mut seen: Vec<String> = Vec::new();
    let mut totals = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let page = phone.call(methods::STORE_SEARCH, json!({ "limit": 1, "cursor": cursor })).await.unwrap();
        totals.push(page["total"].clone());
        seen.extend(ids(&page));
        match page["next_cursor"].as_str() {
            Some(next) if seen.len() < 20 => cursor = Some(next.to_owned()),
            _ => break,
        }
    }
    let (_, whole) = e.get("/store/v1/plugins?limit=50", None).await;
    let total = totals[0].as_u64().unwrap_or(u64::MAX);
    let ok = total == seen.len() as u64 && totals.iter().all(|t| t == &totals[0]) && whole["total"] == totals[0] && ids(&whole) == seen;
    e.check("10", "a search's total is what the edge lists across its pages, the same on each", ok, format!("totals {totals:?}, listed {seen:?}, edge {whole}"));
    let featured_total = phone.call(methods::STORE_SEARCH, json!({ "featured": true })).await.unwrap();
    e.check("10", "and a featured search's is the featured ones'", featured_total["total"] == 2, &featured_total["total"]);
    let none = phone.call(methods::STORE_SEARCH, json!({ "q": "nothing-is-called-this" })).await.unwrap();
    e.check("10", "a search that finds nothing is total 0", none["total"] == 0 && ids(&none).is_empty(), &none);
    author_plugin(&bob_laptop, "all", "1.0.0", &["ui.add"]).await;
    tokio::time::sleep(Duration::from_millis(800)).await;
    let out = e.cli_on(&bob_laptop, &["publish", "all"]);
    e.check("10", "the CLI refuses to publish id all before upload (exit 1, id_reserved)", out.status.code() == Some(1) && text(&out).contains("id_reserved"), text(&out));
    let (_, mine) = e.get("/store/v1/me/plugins", Some(&e.bearer(&bob_laptop).await)).await;
    let sent = mine["plugins"].as_array().is_some_and(|p| p.iter().any(|p| p["id"] == "all"));
    e.check("10", "and nothing reached the store", !sent, &mine);
    let device_key = signing::load_key(&bob_laptop.data).expect("bob's laptop's signing key");
    let forced = store_package(&device_key, "all", "1.0.0", json!({}), &[]);
    let res = e
        .http
        .post(format!("{}/store/v1/plugins/all/versions", e.edge))
        .bearer_auth(e.bearer(&bob_laptop).await)
        .header("content-type", "application/json")
        .body(forced)
        .send()
        .await
        .unwrap();
    let status = res.status().as_u16();
    let answer: Value = res.json().await.unwrap_or_default();
    e.check("10", "forced past the CLI, the edge refuses it as id_reserved", status == 403 && answer["error"] == "id_reserved", format!("{status} {answer}"));
    let (status, answer) = e.admin(&staff, "reservations", json!({ "id": "all", "to_username": "bob-e2e" })).await;
    e.check("10", "staff can't reserve it for anyone (site_id)", status == 400 && answer["error"] == "site_id", format!("{status} {answer}"));

    drop((laptop, desk, phone, bob_laptop));
    let failures = e.failures.lock().unwrap().clone();
    assert!(failures.is_empty(), "{} check(s) failed:\n{}", failures.len(), failures.join("\n"));
}
