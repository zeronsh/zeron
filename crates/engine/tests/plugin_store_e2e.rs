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
//!
//! The flow is amendments 44 and 45's: a version is live as soon as it's
//! published and verified later; devices mark what isn't, ask before taking
//! it, never take it in bulk, learn of take-downs from the feed; deploy keys
//! are the account's, for the plugins they name.

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
            // (The take-down feed and the publish notices every second, not
            // every 15 minutes.)
            .env("ZERON_TAKEDOWN_EVERY_SECS", "1")
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
    /// A fresh sign-in through the device's loopback, begun for `action`
    /// (`StepUp {for}`; `None`: for nothing in particular).
    async fn step_up(&self, device: &Device, user: &str, action: Option<&str>) {
        let params = action.map_or_else(|| json!({}), |a| json!({ "for": a }));
        let url = device.call(methods::STEP_UP, params).await.unwrap();
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

/// The plugin's offer in `device`'s update check, or null.
async fn offer(device: &Device, id: &str) -> Value {
    let checked = device.call(methods::CHECK_PLUGIN_UPDATES, json!({ "id": id })).await.unwrap();
    checked["updates"].as_array().and_then(|u| u.iter().find(|u| u["id"] == id)).cloned().unwrap_or(Value::Null)
}

/// The ids of `device`'s waiting publish notices.
async fn notices(device: &Device) -> Vec<String> {
    let list = device.call(methods::LIST_PLUGINS, json!({})).await.unwrap();
    list["store_notices"].as_array().into_iter().flatten().filter_map(|n| n["id"].as_str().map(str::to_owned)).collect()
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

/// A deploy key made on `device` for `scope`, through a step-up begun for
/// it: the secret, as shown once. Neither the device's sign-in nor a step-up
/// for nothing in particular makes one, and each key takes its own.
async fn deploy_key_on(e: &Env, device: &Device, user: &str, scope: Value, name: &str) -> String {
    let create = || device.call(methods::CREATE_DEPLOY_KEY, json!({ "name": name, "scope": scope }));
    let refused = create().await.unwrap();
    e.check("4", &format!("{user}'s sign-in alone doesn't make a deploy key"), refused["error"] == "step_up_required", &refused);
    e.step_up(device, user, None).await;
    let refused = create().await.unwrap();
    e.check("4", "nor a step-up for nothing in particular", refused["error"] == "step_up_required", &refused);
    e.step_up(device, user, Some("deploy_key")).await;
    let made = create().await.unwrap();
    // (The store keeps a scope's ids sorted.)
    let mut sorted = scope.clone();
    if let Some(ids) = sorted["plugins"].as_array_mut() {
        ids.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
    }
    e.must("4", &format!("{user} makes a deploy key for {scope} through a step-up for it"), made["ok"] == true && made["key"]["scope"] == sorted, &made["key"]);
    let again = create().await.unwrap();
    e.check("4", "and the next key takes a step-up of its own", again["error"] == "step_up_required", &again);
    made["secret"].as_str().unwrap().to_owned()
}

// ── The steps ───────────────────────────────────────────────────────────────

const OTHER: &str = "notes-board-e2e";
const THIRD: &str = "third-board-e2e";

#[tokio::test(flavor = "multi_thread")]
#[ignore = "run by scripts/plugin-store-e2e/run.sh, against a local edge"]
async fn the_plugin_store_end_to_end() {
    let Some(e) = env() else { panic!("E2E_EDGE_JSON, E2E_ZERON_BIN and E2E_WORK_DIR: run scripts/plugin-store-e2e/run.sh") };

    // ── 0: staff ──
    let staff = e.web_sign_in("user_admin").await;
    let grant = e.http.post(format!("{}/admin/roles/grant", e.edge)).bearer_auth(&e.admin_token).json(&json!({ "userId": "user_admin", "role": "admin", "by": "e2e" })).send().await.unwrap();
    e.must("0", "the operator grants admin", grant.status().is_success(), grant.status());
    let staff = if staff.csrf.is_empty() { e.web_sign_in("user_admin").await } else { staff };

    // ── 1: the author publishes from the engine: live at once, not verified ──
    let laptop = e.device("ada-laptop").await;
    e.sign_in(&laptop, "user_ada").await;
    e.claim_username(&laptop, "ada-e2e").await;
    let ada_dir = author_plugin(&laptop, ID, "1.0.0", &["ui.add"]).await;
    let published = publish(&laptop, ID).await;
    let ok = published["ok"] == true && published["status"] == "listed" && published["verified"] == false;
    e.must("1", "the author publishes from the engine: listed at once, not verified", ok, &published);
    e.check("1", "the answer gives its page on the store", published["url"].as_str().is_some_and(|u| u.ends_with(&format!("/plugins/{ID}"))), &published["url"]);
    let mine = laptop.call(methods::STORE_MY_PLUGINS, json!({})).await.unwrap();
    let ours = mine["plugins"].as_array().unwrap().iter().find(|p| p["id"] == ID).cloned().unwrap_or_default();
    let v = &ours["versions"][0];
    e.check("1", "my plugins says listed, not yet decided, published from a device", v["status"] == "listed" && v["review"]["decided"] == false && v["via"] == "device", &mine);

    // ── 2: anyone installs it at once, marked not verified ──
    let phone = e.device("bob-phone").await;
    e.sign_in(&phone, "user_bob").await;
    e.claim_username(&phone, "bob-e2e").await;
    let detail = phone.call(methods::STORE_PLUGIN, json!({ "id": ID })).await.unwrap();
    e.check("2", "its page says not verified, with no verified version", detail["verified"] == false && detail["verified_version"].is_null() && detail["version"] == "1.0.0", &detail);
    let (status, _) = e.get(&format!("/store/v1/plugins/{ID}/versions/1.0.0/package"), None).await;
    e.check("2", "anyone signed out can fetch its package", status == 200, status);
    let installed = phone.call(methods::INSTALL_PLUGIN, json!({ "store": ID })).await;
    let ok = installed.as_ref().is_ok_and(|r| r["plugin"]["author"]["username"] == "ada-e2e" && r["waiting"] == true && r["plugin"]["source"]["verified"] == false);
    e.must("2", "another account installs it by id, marked not verified; it waits for the person", ok, format!("{installed:?}"));
    let counted = wait_for("the install to be counted", async || installs(&phone).await == 1).await;
    e.check("2", "the install is counted", counted, installs(&phone).await);
    phone.call(methods::REMOVE_PLUGIN, json!({ "id": ID })).await.unwrap();
    phone.call(methods::INSTALL_PLUGIN, json!({ "store": ID })).await.unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;
    e.check("2", "the same account's second install isn't counted again", installs(&phone).await == 1, installs(&phone).await);
    e.check("2", "the person allows it", phone.allow(ID).await["status"] == "on", "");
    let icon = phone.call(methods::STORE_FILE, json!({ "id": ID, "version": "1.0.0", "path": "icon.svg" })).await;
    let ok = icon.as_ref().is_ok_and(|f| f["bytes"] == ICON.len() && f["file"].as_str().and_then(|p| std::fs::read(p).ok()).is_some_and(|b| b == ICON.as_bytes()));
    e.check("2", "the device fetches its icon from the package", ok, format!("{icon:?}"));

    // ── 3: verified: the mark, drawn from what each device installed from ──
    let (status, body) = e.admin(&staff, &format!("versions/{ID}/1.0.0/verify"), json!({})).await;
    e.must("3", "staff verify 1.0.0 on a web session", status == 200, &body);
    let detail = phone.call(methods::STORE_PLUGIN, json!({ "id": ID })).await.unwrap();
    e.check("3", "its page now says verified", detail["verified"] == true && detail["verified_version"] == "1.0.0", &detail);
    e.check("3", "the phone's copy was installed unverified, and says so still", detail["installed"]["verified"] == false && phone.item(ID).await["source"]["verified"] == false, &detail["installed"]);
    let only = phone.call(methods::STORE_SEARCH, json!({ "verified": true })).await.unwrap();
    e.check("3", "a search for verified plugins finds it", only["items"].as_array().is_some_and(|i| i.iter().any(|c| c["id"] == ID && c["verified"] == true)), &only);
    let desk = e.device("ada-desk").await;
    e.sign_in(&desk, "user_ada").await;
    let installed = desk.call(methods::INSTALL_PLUGIN, json!({ "store": ID })).await;
    let ok = installed.as_ref().is_ok_and(|r| r["plugin"]["source"]["verified"] == true);
    e.must("3", "the author's other device installs it now, marked verified by its record", ok, format!("{installed:?}"));
    e.check("3", "and allows it", desk.allow(ID).await["status"] == "on", "");
    // (Its first look at the account's publishes: a baseline, nothing noticed.)
    tokio::time::sleep(Duration::from_secs(3)).await;

    // ── 4: two more plugins; a deploy key for two of the three ──
    for id in [OTHER, THIRD] {
        author_plugin(&laptop, id, "1.0.0", &["ui.add"]).await;
        let published = publish(&laptop, id).await;
        e.must("4", &format!("the author publishes {id} 1.0.0 from the laptop"), published["ok"] == true, &published);
    }
    let seen = wait_for("the desk's notices", async || {
        let n = notices(&desk).await;
        n.contains(&format!("{OTHER}@1.0.0")) && n.contains(&format!("{THIRD}@1.0.0"))
    })
    .await;
    let list = desk.call(methods::LIST_PLUGINS, json!({})).await.unwrap();
    let from_laptop = list["store_notices"].as_array().into_iter().flatten().all(|n| n["via"] == "device");
    e.check("4", "the desk notices the laptop's publishes, as another device's", seen && from_laptop, &list["store_notices"]);
    e.check("4", "the laptop notices none of its own", notices(&laptop).await.is_empty(), notices(&laptop).await.join(", "));
    let refused = laptop.call(methods::CREATE_DEPLOY_KEY, json!({ "name": "CI", "scope": { "plugins": [ID, "nobody-has-this-e2e"] } })).await.unwrap();
    e.check("4", "a key for a plugin that isn't yours is refused, naming it", refused["error"] == "not_owner" && refused.to_string().contains("nobody-has-this-e2e"), &refused);
    let scope = json!({ "plugins": [OTHER, ID] });
    let secret = deploy_key_on(&e, &laptop, "user_ada", scope, "GitHub Actions").await;
    let ada_key = DeployKey::from_secret(&secret).unwrap();
    let keys = laptop.call(methods::LIST_DEPLOY_KEYS, json!({})).await.unwrap();
    e.check("4", "the key list shows it with its scope", keys["keys"].as_array().is_some_and(|k| k.iter().any(|k| k["key"] == ada_key.public() && k["scope"] == json!({ "plugins": [ID, OTHER] }))), &keys);
    let ci = e.work.join("ci-checkout").join(ID);
    checkout(&ada_dir, &ci);
    set_version(&ci, "1.1.0", &["ui.add", "links.open"], "- Opens links.");
    let ci_arg = ci.display().to_string();
    let out = e.cli("ci", &["publish", &ci_arg], &[("ZERON_DEPLOY_KEY", &secret)]);
    let said = text(&out);
    e.must("4", "the CLI publishes 1.1.0 with only ZERON_DEPLOY_KEY: live now, not yet verified", out.status.success() && said.contains("live now, not yet verified") && said.contains(&format!("/plugins/{ID}")), &said);
    e.check("4", "the CLI never says the secret", !said.contains(&secret[4..]), "");
    let other_ci = e.work.join("ci-checkout").join(OTHER);
    checkout(&laptop.item(OTHER).await["dir"].as_str().map(PathBuf::from).unwrap(), &other_ci);
    set_version(&other_ci, "1.1.0", &["ui.add"], "- Notes.");
    let out = e.cli("ci", &["publish", &other_ci.display().to_string()], &[("ZERON_DEPLOY_KEY", &secret)]);
    e.check("4", &format!("the same key publishes {OTHER} 1.1.0"), out.status.success(), text(&out));
    let third_ci = e.work.join("ci-checkout").join(THIRD);
    checkout(&laptop.item(THIRD).await["dir"].as_str().map(PathBuf::from).unwrap(), &third_ci);
    set_version(&third_ci, "1.1.0", &["ui.add"], "- Third.");
    let out = e.cli("ci", &["publish", &third_ci.display().to_string()], &[("ZERON_DEPLOY_KEY", &secret)]);
    e.check("4", &format!("and is refused on {THIRD}, yours but outside its scope (exit 3)"), out.status.code() == Some(3) && text(&out).contains("key_not_allowed"), text(&out));
    let noticed = wait_for("the laptop's notices of the key's publishes", async || {
        let n = notices(&laptop).await;
        n.contains(&format!("{ID}@1.1.0")) && n.contains(&format!("{OTHER}@1.1.0"))
    })
    .await;
    let list = laptop.call(methods::LIST_PLUGINS, json!({})).await.unwrap();
    let named = list["store_notices"].as_array().into_iter().flatten().all(|n| n["via"] == "deploy_key" && n["key_name"] == "GitHub Actions");
    e.check("4", "each version the key published is a notice on the author's devices, naming the key", noticed && named && !notices(&laptop).await.iter().any(|n| n.starts_with(THIRD)), &list["store_notices"]);
    laptop.call(methods::ACK_STORE_NOTICES, json!({ "ids": notices(&laptop).await })).await.unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;
    e.check("4", "acknowledged, they go and don't come back", notices(&laptop).await.is_empty(), notices(&laptop).await.join(", "));

    // ── 5: the unverified update: offered, marked, never in bulk, always asks ──
    let update = offer(&desk, ID).await;
    let ok = update["version"] == "1.1.0" && update["verified"] == false && update["installed_verified"] == true && update["added"] == json!(["links.open"]) && update["notes"] == "- Opens links.";
    e.check("5", "the desk is offered 1.1.0 at once: not verified, where what it has was", ok, &update);
    let (_, view) = e.admin_get(&staff, &format!("versions/{ID}/1.1.0")).await;
    let diff = &view["permissions_diff"];
    e.check("5", "the review's permission diff says what the offer says", diff["added"] == update["added"], format!("store {diff} / engine {update}"));
    let bulk = desk.call(methods::UPDATE_PLUGIN, json!({ "id": ID, "verified_only": true })).await;
    e.check("5", "an update-all leaves it for its own question (not_verified)", bulk.as_ref().is_err_and(|r| r.contains("not_verified")), format!("{bulk:?}"));
    let set = desk.call(methods::SET_PLUGIN_UPDATES, json!({ "id": ID, "prefer": "verified" })).await.unwrap();
    let held = offer(&desk, ID).await;
    e.check("5", "a device set to take verified updates holds it", set["prefer"] == "verified" && held["held"] == true, &held);
    desk.call(methods::SET_PLUGIN_UPDATES, json!({ "id": ID, "prefer": null })).await.unwrap();
    offer(&desk, ID).await;
    let updated = desk.call(methods::UPDATE_PLUGIN, json!({ "id": ID })).await;
    let ok = updated.as_ref().is_ok_and(|r| r["plugin"]["version"] == "1.1.0" && r["plugin"]["status"] == "pending" && r["plugin"]["source"]["verified"] == false);
    e.check("5", "the person takes it on its own: it asks, and is marked not verified", ok, format!("{updated:?}"));
    e.check("5", "the person allows it", desk.allow(ID).await["status"] == "on", "");

    // ── 6: decline, yank, and superseding ──
    let (status, body) = e.admin(&staff, &format!("versions/{OTHER}/1.1.0/decline"), json!({ "reason": "broken", "message": "It doesn't open." })).await;
    e.must("6", &format!("staff decline to verify {OTHER} 1.1.0"), status == 200, &body);
    let out = e.cli_on(&laptop, &["status", OTHER]);
    e.check("6", "the author's status says why", text(&out).contains("1.1.0  listed  not verified: declined: broken: It doesn't open."), text(&out));
    let (_, public) = e.get(&format!("/store/v1/plugins/{OTHER}/versions/1.1.0"), None).await;
    e.check("6", "the public sees it unverified, never declined", public["verified"] == false && !public.to_string().contains("declined") && !public.to_string().contains("open."), &public);
    let out = e.cli_on(&laptop, &["yank", ID, "1.1.0"]);
    e.check("6", "the author yanks 1.1.0 (the CLI, with the session)", out.status.success(), text(&out));
    let latest = phone.call(methods::STORE_PLUGIN, json!({ "id": ID })).await.unwrap();
    e.check("6", "a yanked version is no longer the one new installs get", latest["version"] == "1.0.0", &latest["version"]);
    let out = e.cli_on(&laptop, &["unyank", ID, "1.1.0"]);
    e.check("6", "the author unyanks it", out.status.success(), text(&out));
    set_version(&ci, "1.2.0", &["ui.add", "links.open"], "- Faster.");
    let out = e.cli("ci", &["publish", &ci_arg], &[("ZERON_DEPLOY_KEY", &secret)]);
    e.must("6", "CI publishes 1.2.0, and says it supersedes 1.1.0, never verified", out.status.success() && text(&out).contains("1.1.0 no longer takes new installs: 1.2.0 replaced it before it was verified"), text(&out));
    let out = e.cli_on(&laptop, &["status", ID]);
    e.check("6", "status shows 1.1.0 yanked, superseded by 1.2.0", text(&out).contains("1.1.0  yanked  not verified: superseded by 1.2.0"), text(&out));
    let out = e.cli_on(&laptop, &["unyank", ID, "1.1.0"]);
    e.check("6", "and the author can't unyank it", !out.status.success() && text(&out).contains("superseded"), text(&out));
    let refused = phone.call(methods::INSTALL_PLUGIN, json!({ "store": ID, "version": "1.1.0" })).await;
    e.check("6", "nobody installs 1.1.0 any more", refused.as_ref().is_err_and(|r| r.contains("not_in_store")), format!("{refused:?}"));
    e.check("6", "while the desk's copy of it keeps running", desk.item(ID).await["status"] == "on", "");
    // A first install of 1.2.0, not verified: its sheet offers 1.0.0, the
    // newest verified, and installs it by its own record.
    let pad = e.device("eve-pad").await;
    e.sign_in(&pad, "user_eve").await;
    let fresh = pad.call(methods::INSTALL_PLUGIN, json!({ "store": ID })).await.unwrap_or_default();
    let alternative = &fresh["plugin"]["source"]["verified_alternative"];
    let ok = fresh["plugin"]["version"] == "1.2.0" && fresh["plugin"]["source"]["verified"] == false && alternative["version"] == "1.0.0" && alternative["removed"] == json!(["links.open"]);
    e.check("6", "a first install of an unverified version names the verified one instead, and what it asks less", ok, &fresh);
    let instead = pad.call(methods::INSTALL_VERIFIED_PLUGIN, json!({ "id": ID, "hash": fresh["plugin"]["hash"] })).await;
    let ok = instead.as_ref().is_ok_and(|r| r["plugin"]["version"] == "1.0.0" && r["plugin"]["source"]["verified"] == true);
    e.check("6", "and installs it instead, marked verified by its record", ok, format!("{instead:?}"));
    drop(pad);
    let (status, body) = e.admin(&staff, &format!("versions/{ID}/1.2.0/verify"), json!({})).await;
    e.must("6", "staff verify 1.2.0", status == 200, &body);
    let update = offer(&phone, ID).await;
    e.check("6", "the phone is offered 1.2.0, verified", update["version"] == "1.2.0" && update["verified"] == true, &update);
    let updated = phone.call(methods::UPDATE_PLUGIN, json!({ "id": ID, "verified_only": true })).await;
    let ok = updated.as_ref().is_ok_and(|r| r["plugin"]["version"] == "1.2.0" && r["plugin"]["source"]["verified"] == true);
    e.check("6", "an update-all takes a verified one", ok, format!("{updated:?}"));
    e.check("6", "its new permission asks; the person allows it", phone.allow(ID).await["status"] == "on", "");

    // ── 7: a take-down reaches the device by the feed; a restore asks again ──
    let (status, feed) = e.get("/store/v1/takedowns", None).await;
    e.check("7", "the feed, asked from nowhere, answers a reset and its head", status == 200 && feed["reset"] == true, &feed);
    let (status, body) = e.admin(&staff, &format!("plugins/{ID}/versions/1.2.0/take-down"), json!({ "reason": "It sent your chats somewhere." })).await;
    e.must("7", "staff take 1.2.0 down", status == 200, &body);
    let off = wait_for("the phone to turn it off", async || phone.item(ID).await["status"] == "off").await;
    let item = phone.item(ID).await;
    e.check("7", "the phone turns it off from the feed, with the reason, without a check", off && item["blocked"]["reason"] == "It sent your chats somewhere.", &item);
    let (_, feed) = e.get(&format!("/store/v1/takedowns?after={}", feed["head"].as_u64().unwrap_or(0)), None).await;
    let event = feed["events"].as_array().and_then(|ev| ev.iter().find(|ev| ev["id"] == ID)).cloned().unwrap_or_default();
    e.check("7", "the feed carries the take-down", event["kind"] == "take_down" && event["version"] == "1.2.0", &feed);
    let (status, body) = e.admin(&staff, &format!("plugins/{ID}/versions/1.2.0/restore"), json!({ "reason": "A mistake." })).await;
    e.must("7", "staff restore it", status == 200, &body);
    let back = wait_for("the phone to lift the block", async || phone.item(ID).await.get("blocked").is_none()).await;
    let item = phone.item(ID).await;
    e.check("7", "the restore lifts the block; back unverified, it asks again", back && item["status"] == "pending" && item["source"]["verified"] == false, &item);
    let detail = phone.call(methods::STORE_PLUGIN, json!({ "id": ID })).await.unwrap();
    e.check("7", "and the store says it's unverified again", detail["verified"] == false && detail["verified_version"] == "1.0.0", &detail);
    e.check("7", "the person allows it", phone.allow(ID).await["status"] == "on", "");

    // ── 8: a transfer ──
    let bob_bearer = e.bearer(&phone).await;
    let lookup = |key: String| {
        let (e, bearer) = (&e, bob_bearer.clone());
        async move { e.get(&format!("/plugin-keys/{key}?plugin={ID}"), Some(&bearer)).await }
    };
    let (_, before) = lookup(ada_key.public()).await;
    e.check("8", "before: ada's deploy key reads as verified for its plugin", before["verified"] == true, &before);
    let (status, body) = e.admin(&staff, &format!("plugins/{ID}/transfer"), json!({ "to_username": "bob-e2e" })).await;
    e.must("8", "staff transfer it to @bob-e2e", status == 200, &body);
    set_version(&ci, "1.2.1", &["ui.add", "links.open"], "- After.");
    let out = e.cli("ci", &["publish", &ci_arg], &[("ZERON_DEPLOY_KEY", &secret)]);
    e.check("8", "the old owner's deploy key is refused on publish (exit 3)", out.status.code() == Some(3), text(&out));
    let (_, after) = lookup(ada_key.public()).await;
    e.check("8", "the old owner's deploy key reads unverified for the plugin", after["verified"] == false, &after);
    set_version(&other_ci, "1.2.0", &["ui.add"], "- Still mine.");
    let out = e.cli("ci", &["publish", &other_ci.display().to_string()], &[("ZERON_DEPLOY_KEY", &secret)]);
    e.check("8", &format!("while it still publishes {OTHER}"), out.status.success(), text(&out));
    let bob_laptop = e.device("bob-laptop").await;
    e.sign_in(&bob_laptop, "user_bob").await;
    author_plugin(&bob_laptop, ID, "1.2.2", &["ui.add", "links.open"]).await;
    let published = publish(&bob_laptop, ID).await;
    e.must("8", "the new owner publishes 1.2.2", published["ok"] == true, &published);
    let (status, body) = e.admin(&staff, &format!("versions/{ID}/1.2.2/verify"), json!({})).await;
    e.must("8", "staff verify 1.2.2", status == 200, &body);
    let update = offer(&desk, ID).await;
    e.check("8", "the update says it has a new owner", update["version"] == "1.2.2" && update["new_owner"] == true, &update);
    let updated = desk.call(methods::UPDATE_PLUGIN, json!({ "id": ID })).await;
    let ok = updated.as_ref().is_ok_and(|r| r["plugin"]["author"]["username"] == "bob-e2e" && r["plugin"]["status"] == "pending");
    e.check("8", "updated, it's by @bob-e2e and asks again", ok, format!("{updated:?}"));

    // ── 9: the refusals, each against the real edge ──
    let bob_secret = deploy_key_on(&e, &bob_laptop, "user_bob", json!({ "plugins": [ID] }), "CI").await;
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
    e.check("9", "a signed publish lands, listed", first == 201 && body["status"] == "listed", &body);
    let (status, body) = e.signed(&bob_key, "POST", &path, &pkg, ts, None).await;
    e.check("9", "the same signed request again is refused as a replay", status == 401 && body["error"] == "replayed_request", format!("{status} {body}"));
    let (status, body) = e.admin(&staff, &format!("versions/{ID}/1.3.0/verify"), json!({})).await;
    e.check("9", "staff verify 1.3.0", status == 200, &body);
    let other = store_package(signer, ID, "1.3.1", json!({}), &[]);
    let (status, body) = e.signed(&bob_key, "POST", &path, &other, now_ms(), Some(&pkg)).await;
    e.check("9", "a body that isn't the one its hash names is refused", status == 400 && body["error"] == "body_mismatch", format!("{status} {body}"));
    let elsewhere = store_package(signer, OTHER, "9.0.0", json!({}), &[]);
    let (status, body) = e.publish_signed(&bob_key, OTHER, &elsewhere).await;
    e.check("9", "a key for another plugin is refused", status == 403 && body["error"] == "key_not_allowed", format!("{status} {body}"));
    let (status, body) = e.publish_signed(&bob_key, ID, &store_package(signer, ID, "1.2.5", json!({}), &[])).await;
    e.check("9", "a version that isn't newer is refused", body["error"] == "not_newer" || body["error"] == "version_exists", format!("{status} {body}"));
    // (On ada's third plugin, from her laptop: should the edge take one,
    // nothing below depends on that plugin.)
    let ada_device_key = signing::load_key(&laptop.data).expect("ada's laptop's signing key");
    let ada_bearer = e.bearer(&laptop).await;
    let from_laptop = async |version: &str, extra: Value| {
        let res = e
            .http
            .post(format!("{}/store/v1/plugins/{THIRD}/versions", e.edge))
            .bearer_auth(&ada_bearer)
            .header("content-type", "application/json")
            .body(store_package(&ada_device_key, THIRD, version, extra, &[]))
            .send()
            .await
            .unwrap();
        (res.status().as_u16(), res.json::<Value>().await.unwrap_or_default())
    };
    let (status, body) = from_laptop("2.0.0", json!({ "name": "Verified Third Board" })).await;
    e.check("9", "a name claiming the mark is refused (bad_listing)", status == 400 && body["error"] == "bad_listing", format!("{status} {body}"));
    let (status, body) = from_laptop("2.0.1", json!({ "description": "The \u{43e}fficial board." })).await;
    e.check("9", "and a description claiming it in look-alike letters", status == 400 && body["error"] == "bad_listing", format!("{status} {body}"));
    let (status, body) = from_laptop("2.0.2", json!({ "description": "Boards, by Zeron." })).await;
    e.check("9", "a description saying it's Zeron's own is refused (claims_zeron)", status == 400 && body["error"] == "claims_zeron", format!("{status} {body}"));
    let big = store_package(signer, ID, "1.3.2", json!({}), &[("data.bin", vec![7u8; 16 * 1024 * 1024])]);
    let (status, body) = e.publish_signed(&bob_key, ID, &big).await;
    e.check("9", "a package over the store's limit is refused", status == 413 || body["error"] == "too_large", format!("{status} {body}"));
    let colliding = store_package(signer, ID, "1.3.3", json!({}), &[("Notes.txt", b"a".to_vec()), ("notes.txt", b"b".to_vec())]);
    let (status, body) = e.publish_signed(&bob_key, ID, &colliding).await;
    e.check("9", "a package whose paths collide on macOS and Windows is refused", status >= 400, format!("{status} {body}"));
    let deep_text = String::from_utf8(store_package(signer, ID, "1.3.4", json!({}), &[])).unwrap();
    let nest = format!("{}{}", "[".repeat(127), "]".repeat(127));
    let deep = format!("{{\"x\":{nest},{}", &deep_text[1..]).into_bytes();
    let (status, body) = e.publish_signed(&bob_key, ID, &deep).await;
    e.check("9", "a package nesting one level too deep is refused", status == 400, format!("{status} {body}"));
    let updated = desk.call(methods::UPDATE_PLUGIN, json!({ "id": ID, "verified_only": true })).await;
    let ok = updated.as_ref().is_ok_and(|r| r["plugin"]["version"] == "1.3.0") && desk.allow(ID).await["status"] == "on";
    e.check("9", "a device takes 1.3.0, with its tools", ok, format!("{updated:?}"));
    let retried = store_package(signer, ID, "1.3.5", tools_after, &[]);
    let (a, first_body) = e.publish_signed(&bob_key, ID, &retried).await;
    let (b, second_body) = e.publish_signed(&bob_key, ID, &retried).await;
    e.check("9", "a publish retried with the same bytes gets the same answer", a == b && first_body == second_body, format!("{a} {first_body} / {b} {second_body}"));
    let (status, _) = e.get(&format!("/store/v1/plugins/{ID}/versions/1.3.5/package"), None).await;
    e.check("9", "and its package is served", status == 200, status);
    let (_, view) = e.admin_get(&staff, &format!("versions/{ID}/1.3.5")).await;
    let offer_135 = offer(&desk, ID).await;
    let (store, engine) = (&view["permissions_diff"], &offer_135["digest_diff"]);
    let same = ["tools_added", "tools_changed", "tools_removed", "added_commands", "removed_commands", "ui_change_added", "ui_replace_added", "network_added"]
        .iter()
        .all(|k| store[k] == engine[k])
        && store["added"] == engine["permissions_added"]
        && store["added"] == offer_135["added"];
    let expected = engine["tools_changed"]
        == json!([
            { "name": "lookup", "description_before": "Looks a word up.", "description_after": "Looks a word up, and its plural.", "input_changed": false },
            { "name": "shape", "description_before": "Same words.", "description_after": "Same words.", "input_changed": true },
        ])
        && engine["tools_added"] == json!([{ "name": "new", "description": "Arrives." }])
        && engine["tools_removed"] == json!(["old"]);
    e.check("9", "the review's diff of 1.3.5 says what the device's offer says (tools added, changed, removed)", offer_135["version"] == "1.3.5" && same && expected, format!("store {store} / engine {offer_135}"));
    let incompatible = store_package(signer, ID, "1.4.0", json!({ "engines": { "zeron": ">=99.0.0" } }), &[]);
    let (status, body) = e.publish_signed(&bob_key, ID, &incompatible).await;
    e.check("9", "a version for another Zeron is taken by the store, superseding 1.3.5", status == 201 && body["superseded"] == json!(["1.3.5"]), format!("{status} {body}"));
    let (status, body) = e.admin(&staff, &format!("versions/{ID}/1.4.0/verify"), json!({})).await;
    e.check("9", "staff verify 1.4.0", status == 200, &body);
    let refused = phone.call(methods::INSTALL_PLUGIN, json!({ "store": ID, "version": "1.4.0" })).await;
    e.check("9", "a device refuses a version for another Zeron", refused.as_ref().is_err_and(|r| r.contains("incompatible")), format!("{refused:?}"));
    let offered = offer(&phone, ID).await;
    e.check("9", "the update check never offers it", offered["version"] != "1.4.0", &offered);
    let (status, body) = e.admin_as(&staff, &format!("versions/{ID}/1.3.5/decline"), json!({ "reason": "other", "message": "x" }), &e.site_origin.clone(), Some(&e.bearer(&laptop).await)).await;
    e.check("9", "a bearer on an admin route is refused, a valid session beside it", status == 403, format!("{status} {body}"));
    let (status, body) = e.admin_as(&staff, &format!("plugins/{ID}/feature"), json!({ "featured": true }), "https://evil.example", None).await;
    e.check("9", "a cross-site change with a valid session is refused", status == 403 && body["error"] == "bad_csrf", format!("{status} {body}"));
    set_version(&ci, "1.5.0", &["ui.add", "links.open"], "- Late.");
    let out = e.cli("ci", &["publish", &ci_arg], &[("ZERON_DEPLOY_KEY", &secret)]);
    e.check("9", "the former owner's deploy key is still refused on publish", out.status.code() == Some(3), text(&out));
    let (_, after) = lookup(ada_key.public()).await;
    e.check("9", "and still reads unverified for the plugin", after["verified"] == false, &after);
    let claiming = e.work.join("claims").join(ID);
    checkout(&ci, &claiming);
    let path_m = claiming.join("zeron-plugin.json");
    let mut manifest: Value = serde_json::from_slice(&std::fs::read(&path_m).unwrap()).unwrap();
    manifest["name"] = json!("Focus Board \u{2714}");
    std::fs::write(&path_m, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
    let requests_before = e.get(&format!("/store/v1/plugins/{ID}/versions?limit=50"), None).await.1["total"].clone();
    let out = e.cli("ci", &["publish", &claiming.display().to_string()], &[("ZERON_DEPLOY_KEY", &bob_secret)]);
    let after_total = e.get(&format!("/store/v1/plugins/{ID}/versions?limit=50"), None).await.1["total"].clone();
    e.check("9", "the CLI refuses a name with a check mark before sending it (exit 1, bad_listing)", out.status.code() == Some(1) && text(&out).contains("bad_listing") && requests_before == after_total, text(&out));

    // ── 10: reports ──
    let report = json!({ "id": ID, "reason": "broken", "text": "It doesn't open." });
    let first = desk.call(methods::STORE_REPORT, report.clone()).await.unwrap();
    e.must("10", "a person reports it from the engine", first["ok"] == true && first["status"] == "open", &first);
    let again = desk.call(methods::STORE_REPORT, report).await.unwrap();
    e.check("10", "reporting again while it's open is the same report", again["ok"] == true && again["id"] == first["id"], &again);
    let (status, listed) = e.admin_get(&staff, "reports?status=open").await;
    let seen = listed["reports"].as_array().and_then(|r| r.iter().find(|r| r["id"] == first["id"])).cloned().unwrap_or_default();
    let ok = status == 200 && seen["plugin"]["id"] == ID && seen["reason"] == "broken" && seen["text"] == "It doesn't open.";
    e.check("10", "staff see it, with its reason and the person's words", ok, format!("{status} {listed}"));
    let reports = format!("/store/v1/plugins/{ID}/reports");
    let body = serde_json::to_vec(&json!({ "reason": "other" })).unwrap();
    let (status, answer) = e.signed(&bob_key, "POST", &reports, &body, now_ms(), None).await;
    e.check("10", "a deploy key can't report", status == 403 && answer["error"] == "key_not_allowed", format!("{status} {answer}"));
    let installed = format!("/store/v1/plugins/{ID}/installed");
    let (status, answer) = e.signed(&bob_key, "POST", &installed, b"{}", now_ms(), None).await;
    e.check("10", "nor count an install", status == 403 && answer["error"] == "key_not_allowed", format!("{status} {answer}"));
    let cookie = staff.cookie.lock().unwrap().clone();
    let res = e.http.post(format!("{}{reports}", e.site)).header("cookie", cookie).json(&json!({ "reason": "other" })).send().await.unwrap();
    let status = res.status().as_u16();
    let answer: Value = res.json().await.unwrap_or_default();
    e.check("10", "a report on a web session without the site's Origin is refused", status == 403 && answer["error"] == "bad_csrf", format!("{status} {answer}"));
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
    e.check("10", "a report on a web session with a wrong CSRF token is refused", status == 403 && answer["error"] == "bad_csrf", format!("{status} {answer}"));
    let (status, carols) = web_report(&carol.csrf.clone()).await;
    e.check("10", "with the session's token, it lands", status == 201 && carols["status"] == "open" && carols["id"] != first["id"], format!("{status} {carols}"));
    let id = first["id"].as_i64().unwrap_or_default();
    let (status, body) = e.admin(&staff, &format!("reports/{id}/resolve"), json!({ "resolution": "dismissed", "note": "Works here." })).await;
    e.check("10", "staff resolve it", status == 200, &body);
    let (_, open) = e.admin_get(&staff, "reports?status=open").await;
    let gone = open["reports"].as_array().is_some_and(|r| r.iter().all(|r| r["id"] != first["id"]) && r.iter().any(|r| r["id"] == carols["id"]));
    e.check("10", "and it leaves the open reports (the other one stays)", gone, &open);
    // Signing out on the site: Origin and the CSRF token, as any write.
    let logout = async |origin: &str, csrf: &str| {
        let cookie = carol.cookie.lock().unwrap().clone();
        let res = e.http.post(format!("{}/auth/web/logout", e.site)).header("cookie", cookie).header("origin", origin).header("x-csrf-token", csrf).send().await.unwrap();
        (res.status().as_u16(), res.json::<Value>().await.unwrap_or_default())
    };
    let (status, answer) = logout(&e.site_origin.clone(), "not-the-session-token").await;
    e.check("10", "signing out with a wrong CSRF token is refused", status == 403 && answer["error"] == "bad_csrf", format!("{status} {answer}"));
    let (status, answer) = logout("https://evil.example", &carol.csrf.clone()).await;
    e.check("10", "and from another origin", status == 403 && answer["error"] == "bad_csrf", format!("{status} {answer}"));
    let (status, answer) = logout(&e.site_origin.clone(), &carol.csrf.clone()).await;
    e.check("10", "with both, it signs out", status == 200 && answer["ok"] == true, format!("{status} {answer}"));
    let (status, answer) = web_report(&carol.csrf.clone()).await;
    e.check("10", "and the ended session can't report", status == 401, format!("{status} {answer}"));

    // ── 11: featured needs a verified newest version ──
    let ids = |reply: &Value| reply["items"].as_array().into_iter().flatten().filter_map(|c| c["id"].as_str().map(str::to_owned)).collect::<Vec<_>>();
    let featured = async || {
        let engine = phone.call(methods::STORE_SEARCH, json!({ "featured": true })).await.unwrap();
        let (_, edge) = e.get("/store/v1/plugins?featured=true", None).await;
        (ids(&engine), ids(&edge), engine)
    };
    let (status, body) = e.admin(&staff, &format!("plugins/{ID}/feature"), json!({ "featured": true })).await;
    e.must("11", &format!("staff feature {ID}, its newest verified"), status == 200, &body);
    let (engine, edge, reply) = featured().await;
    let only = engine == [ID] && engine == edge && reply["items"][0]["featured"] == true;
    e.check("11", "a featured search lists only the featured one", only, format!("engine {engine:?} / edge {edge:?}"));
    let (status, body) = e.admin(&staff, &format!("plugins/{OTHER}/feature"), json!({ "featured": true })).await;
    e.check("11", &format!("{OTHER}, its newest not verified, can't be featured"), status == 409 && body["error"] == "not_verified", format!("{status} {body}"));
    let (status, body) = e.admin(&staff, &format!("versions/{OTHER}/1.2.0/verify"), json!({})).await;
    e.must("11", &format!("staff verify {OTHER} 1.2.0"), status == 200, &body);
    let (status, body) = e.admin(&staff, &format!("plugins/{OTHER}/feature"), json!({ "featured": true })).await;
    e.must("11", "then it can", status == 200, &body);
    let (engine, edge, _) = featured().await;
    let both = engine.len() == 2 && engine.contains(&ID.to_owned()) && engine.contains(&OTHER.to_owned()) && engine == edge;
    e.check("11", "a featured search lists both, in the edge's order", both, format!("engine {engine:?} / edge {edge:?}"));

    // ── 12: totals, counts, a publisher's plugins; the site's own ids ──
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
    e.check("12", "a search's total is what the edge lists across its pages, the same on each", ok, format!("totals {totals:?}, listed {seen:?}, edge {whole}"));
    let stats = phone.call(methods::STORE_STATS, json!({})).await.unwrap();
    let verified = whole["items"].as_array().map_or(0, |i| i.iter().filter(|c| c["verified"] == true).count());
    e.check("12", "the store's counts are what it lists", stats["plugins"] == whole["total"] && stats["verified"] == verified, format!("{stats} / {verified} verified of {}", whole["total"]));
    let ada = phone.call(methods::STORE_SEARCH, json!({ "author": "ada-e2e" })).await.unwrap();
    let mut by_ada = ids(&ada);
    by_ada.sort();
    e.check("12", "a publisher's plugins are theirs alone", by_ada == [OTHER, THIRD], &ada);
    let featured_total = phone.call(methods::STORE_SEARCH, json!({ "featured": true })).await.unwrap();
    e.check("12", "a featured search's total is the featured ones'", featured_total["total"] == 2, &featured_total["total"]);
    let none = phone.call(methods::STORE_SEARCH, json!({ "q": "nothing-is-called-this" })).await.unwrap();
    e.check("12", "a search that finds nothing is total 0", none["total"] == 0 && ids(&none).is_empty(), &none);
    author_plugin(&bob_laptop, "all", "1.0.0", &["ui.add"]).await;
    tokio::time::sleep(Duration::from_millis(800)).await;
    let out = e.cli_on(&bob_laptop, &["publish", "all"]);
    e.check("12", "the CLI refuses to publish id all before upload (exit 1, id_reserved)", out.status.code() == Some(1) && text(&out).contains("id_reserved"), text(&out));
    let (_, mine) = e.get("/store/v1/me/plugins", Some(&e.bearer(&bob_laptop).await)).await;
    let sent = mine["plugins"].as_array().is_some_and(|p| p.iter().any(|p| p["id"] == "all"));
    e.check("12", "and nothing reached the store", !sent, &mine);
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
    e.check("12", "forced past the CLI, the edge refuses it as id_reserved", status == 403 && answer["error"] == "id_reserved", format!("{status} {answer}"));
    let (status, answer) = e.admin(&staff, "reservations", json!({ "id": "all", "to_username": "bob-e2e" })).await;
    e.check("12", "staff can't reserve it for anyone (site_id)", status == 400 && answer["error"] == "site_id", format!("{status} {answer}"));

    drop((laptop, desk, phone, bob_laptop));
    let failures = e.failures.lock().unwrap().clone();
    assert!(failures.is_empty(), "{} check(s) failed:\n{}", failures.len(), failures.join("\n"));
}
