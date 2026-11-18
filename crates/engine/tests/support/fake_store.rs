//! A stand-in for the edge's plugin store (`/store/v1`), over real HTTP on
//! a random local port: enough of the contract for a device to install,
//! update, be blocked, and publish. Shared by the engine's store tests and
//! the CLI's (`apps/zeron/tests`).
//!
//! Sessions are bearer tokens naming a username (`session:<username>`);
//! a deploy key's signed request is checked as the contract says (key
//! registered for the plugin, signature, five minutes of skew). It can be
//! made hostile on purpose (`serve_instead`, `offer_anyway`), or go offline.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use ring::signature::{ED25519, UnparsedPublicKey};
use serde_json::{Value, json};
use sha2::Digest as _;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use zeron_engine::plugins::install;
use zeron_engine::plugins::version::{Range, Version};

#[derive(Clone, Debug)]
pub struct StoredVersion {
    pub version: String,
    /// `approved`, `pending`, `rejected`, `withdrawn`, `yanked`, `removed`.
    pub status: String,
    pub raw: Vec<u8>,
    pub hash: String,
    pub key: String,
    pub notes: String,
    pub engines: Option<String>,
    /// Taken down and blocked, with this reason.
    pub blocked: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct StoredPlugin {
    /// The owner's username.
    pub owner: String,
    /// The owner's account (its user id: what their publisher id is made
    /// from). Empty: the account named as the username.
    pub owner_account: String,
    pub versions: Vec<StoredVersion>,
    pub installs: u32,
    /// Featured by staff (the edge's `featured_at`).
    pub featured: bool,
}

#[derive(Default)]
pub struct State {
    pub plugins: BTreeMap<String, StoredPlugin>,
    /// Deploy keys: public key → (plugin id, owner).
    pub deploy_keys: BTreeMap<String, (String, String)>,
    /// Each request: `(method, path, authorization)`.
    pub requests: Vec<(String, String, Option<String>)>,
    /// What the next package download of `(id, version)` serves instead.
    pub serve_instead: BTreeMap<(String, String), Vec<u8>>,
    /// Offers made whatever the device asked (a hostile store).
    pub offer_anyway: Vec<Value>,
    /// Packages submitted: `(id, raw, via)`.
    pub submitted: Vec<(String, Vec<u8>, String)>,
    /// Updates asks, as sent.
    pub asks: Vec<Value>,
    /// Each signed request's `X-Zeron-Content-SHA256`.
    pub content_sha256: Vec<Option<String>>,
    /// Reports: `(id, body)`.
    pub reports: Vec<(String, Value)>,
    /// Blocks sent whatever the device asked (a hostile store).
    pub blocked_anyway: Vec<Value>,
    /// Records sent without their package's size (`bytes: null`).
    pub without_bytes: bool,
    /// Answer the next package download with a 302 to this URL.
    pub redirect_package: Option<String>,
    /// Fields laid over every card, page and plugin of yours it answers (a
    /// hostile store's words).
    pub card_extra: Value,
}

/// `card` with the store's `card_extra` laid over it.
fn carded(state: &State, mut card: Value) -> Value {
    if let (Some(card), Some(extra)) = (card.as_object_mut(), state.card_extra.as_object()) {
        for (k, v) in extra {
            card.insert(k.clone(), v.clone());
        }
    }
    card
}

/// A version's package size as the store's records give it.
fn bytes_of(state: &State, v: &StoredVersion) -> Value {
    if state.without_bytes { Value::Null } else { json!(v.raw.len()) }
}

/// Who sent a request: a username, and the plugin a deploy key is for.
type Who = (String, Option<String>);

#[derive(Clone)]
pub struct FakeStore {
    pub url: String,
    pub state: Arc<Mutex<State>>,
    pub offline: Arc<AtomicBool>,
}

fn sha256_hex(bytes: &[u8]) -> String {
    sha2::Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// An account's publisher id, as the edge makes it (amendment 33).
pub fn publisher_id(account: &str) -> String {
    format!("pub_{}", &sha256_hex(format!("zeron-publisher:v1:{account}").as_bytes())[..32])
}

impl StoredPlugin {
    fn account(&self) -> &str {
        if self.owner_account.is_empty() { &self.owner } else { &self.owner_account }
    }

    fn author(&self) -> Value {
        json!({ "id": publisher_id(self.account()), "username": self.owner, "badges": [] })
    }
}

impl FakeStore {
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let store = FakeStore { url, state: Arc::default(), offline: Arc::default() };
        let serving = store.clone();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let store = serving.clone();
                tokio::spawn(async move {
                    let Some((method, path, headers, body)) = read_request(&mut socket).await else { return };
                    if store.offline.load(Ordering::SeqCst) {
                        // A dropped connection: no answer at all.
                        return;
                    }
                    let (status, content_type, reply) = store.answer(&method, &path, &headers, &body);
                    let head = if content_type == "redirect" {
                        let location = String::from_utf8_lossy(&reply);
                        format!("HTTP/1.1 {status} X\r\nlocation: {location}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
                    } else {
                        format!("HTTP/1.1 {status} X\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n", reply.len())
                    };
                    let reply = if content_type == "redirect" { Vec::new() } else { reply };
                    let _ = socket.write_all(head.as_bytes()).await;
                    let _ = socket.write_all(&reply).await;
                });
            }
        });
        store
    }

    /// Add version `raw` of its plugin, owned by `owner`, as `status`.
    pub fn add(&self, owner: &str, raw: &[u8], status: &str, notes: &str) {
        let package = install::parse(raw.to_vec()).expect("a valid package");
        let engines = package.manifest.raw["engines"]["zeron"].as_str().map(str::to_owned);
        let mut state = self.state.lock().unwrap();
        let plugin = state.plugins.entry(package.id().to_owned()).or_default();
        if plugin.owner.is_empty() {
            plugin.owner = owner.to_owned();
        }
        plugin.versions.push(StoredVersion {
            version: package.manifest.version.clone(),
            status: status.to_owned(),
            raw: raw.to_vec(),
            hash: package.hash.clone(),
            key: package.key.clone(),
            notes: notes.to_owned(),
            engines,
            blocked: None,
        });
    }

    pub fn set_status(&self, id: &str, version: &str, status: &str) {
        let mut state = self.state.lock().unwrap();
        let v = state.plugins.get_mut(id).unwrap().versions.iter_mut().find(|v| v.version == version).unwrap();
        v.status = status.to_owned();
    }

    pub fn block(&self, id: &str, version: &str, reason: &str) {
        let mut state = self.state.lock().unwrap();
        let v = state.plugins.get_mut(id).unwrap().versions.iter_mut().find(|v| v.version == version).unwrap();
        v.status = "removed".into();
        v.blocked = Some(reason.to_owned());
    }

    pub fn transfer(&self, id: &str, to: &str) {
        let mut state = self.state.lock().unwrap();
        let plugin = state.plugins.get_mut(id).unwrap();
        plugin.owner = to.to_owned();
        plugin.owner_account = String::new();
    }

    /// An operator gives the owner's username to another account, which
    /// then owns the plugin: the same name, another publisher.
    pub fn move_username(&self, id: &str, account: &str) {
        self.state.lock().unwrap().plugins.get_mut(id).unwrap().owner_account = account.to_owned();
    }

    /// Staff feature it.
    pub fn feature(&self, id: &str) {
        self.state.lock().unwrap().plugins.get_mut(id).unwrap().featured = true;
    }

    pub fn add_deploy_key(&self, public: &str, id: &str, owner: &str) {
        self.state.lock().unwrap().deploy_keys.insert(public.to_owned(), (id.to_owned(), owner.to_owned()));
    }

    pub fn installs(&self, id: &str) -> u32 {
        self.state.lock().unwrap().plugins.get(id).map_or(0, |p| p.installs)
    }

    /// Who a request is: a session's username, or a deploy key's owner (with
    /// the plugin it's scoped to).
    fn who(&self, method: &str, path: &str, headers: &BTreeMap<String, String>, body: &[u8]) -> Result<Option<Who>, (u16, Value)> {
        let Some(auth) = headers.get("authorization") else { return Ok(None) };
        if let Some(token) = auth.strip_prefix("Bearer ") {
            return Ok(token.strip_prefix("session:").map(|u| (u.to_owned(), None)));
        }
        let Some(rest) = auth.strip_prefix("ZeronKey ") else {
            return Err((401, json!({ "error": "unauthorized", "message": "Who's asking?" })));
        };
        let field = |name: &str| rest.split(", ").find_map(|kv| kv.strip_prefix(&format!("{name}="))).unwrap_or("").to_owned();
        let (key, ts, sig) = (field("key"), field("ts"), field("sig"));
        let bad = |why: &str| Err((401, json!({ "error": "bad_signature", "message": why })));
        let claimed = headers.get("x-zeron-content-sha256").cloned();
        self.state.lock().unwrap().content_sha256.push(claimed.clone());
        let Some(claimed) = claimed else { return bad("no X-Zeron-Content-SHA256") };
        let Some((scope, owner)) = self.state.lock().unwrap().deploy_keys.get(&key).cloned() else {
            return Err((403, json!({ "error": "key_not_allowed", "message": "That key isn't registered." })));
        };
        let Ok(ts_ms) = ts.parse::<i64>() else { return bad("no time") };
        if (chrono_now() - ts_ms).abs() > 5 * 60 * 1000 {
            return bad("too old");
        }
        let bare = path.split('?').next().unwrap_or(path);
        // As the edge: the signature over the claimed hash first, then the
        // body against it.
        let message = format!("zeron-plugin-publish:v1\n{method}\n{bare}\n{claimed}\n{ts}");
        let (Ok(key_bytes), Ok(sig_bytes)) = (B64URL.decode(&key), B64URL.decode(&sig)) else { return bad("not base64url") };
        if UnparsedPublicKey::new(&ED25519, &key_bytes).verify(message.as_bytes(), &sig_bytes).is_err() {
            return bad("the signature doesn't check out");
        }
        if sha256_hex(body) != claimed {
            return Err((400, json!({ "error": "body_mismatch", "message": "The body isn't what X-Zeron-Content-SHA256 says." })));
        }
        // Amendment 6: a deploy key publishes its plugin and reads its state.
        let bare_parts: Vec<&str> = bare.trim_start_matches('/').split('/').collect();
        let allowed = matches!((method, bare_parts.as_slice()), ("POST", ["store", "v1", "plugins", _, "versions"]) | ("GET", ["store", "v1", "me", "plugins"]));
        if !allowed {
            return Err((403, json!({ "error": "key_not_allowed", "message": "A deploy key publishes and reads status, nothing else." })));
        }
        Ok(Some((owner, Some(scope))))
    }

    fn answer(&self, method: &str, path: &str, headers: &BTreeMap<String, String>, body: &[u8]) -> (u16, &'static str, Vec<u8>) {
        self.state.lock().unwrap().requests.push((method.to_owned(), path.to_owned(), headers.get("authorization").cloned()));
        let json_reply = |status: u16, value: Value| (status, "application/json", serde_json::to_vec(&value).unwrap());
        let who = match self.who(method, path, headers, body) {
            Ok(who) => who,
            Err((status, value)) => return json_reply(status, value),
        };
        let caller = who.as_ref().map(|(u, _)| u.clone());
        let parts: Vec<&str> = path.split('?').next().unwrap().trim_start_matches('/').split('/').collect();
        let not_found = || json_reply(404, json!({ "error": "not_found", "message": "No such plugin." }));
        match (method, parts.as_slice()) {
            ("GET", ["store", "v1", "categories"]) => json_reply(200, json!({ "categories": [{ "id": "productivity", "name": "Productivity" }] })),
            ("GET", ["store", "v1", "plugins"]) => {
                let state = self.state.lock().unwrap();
                let query = path.split_once('?').map_or("", |(_, q)| q);
                let featured_only = query.split('&').any(|pair| pair == "featured=true");
                // (As the edge: `featured=true` filters; the order is the listing's.)
                let items: Vec<Value> = state
                    .plugins
                    .iter()
                    .filter(|(_, p)| !featured_only || p.featured)
                    .filter_map(|(id, p)| {
                        let v = p.versions.iter().rev().find(|v| v.status == "approved")?;
                        Some(carded(&state, json!({ "id": id, "name": id, "version": v.version, "author": p.author(), "featured": p.featured })))
                    })
                    .collect();
                // (One page: the total is what it lists.)
                json_reply(200, json!({ "total": items.len(), "items": items }))
            }
            ("GET", ["store", "v1", "plugins", id]) => {
                let state = self.state.lock().unwrap();
                let Some(p) = state.plugins.get(*id) else { return not_found() };
                let Some(v) = p.versions.iter().rev().find(|v| v.status == "approved") else { return not_found() };
                json_reply(200, carded(&state, json!({
                    "id": id, "name": id, "version": v.version,
                    "author": p.author(),
                    "engines": { "zeron": v.engines },
                    "package": { "hash": v.hash, "sha256": sha256_hex(&v.raw), "bytes": bytes_of(&state, v), "key": v.key },
                })))
            }
            ("GET", ["store", "v1", "plugins", id, "versions"]) => {
                let state = self.state.lock().unwrap();
                let Some(p) = state.plugins.get(*id) else { return not_found() };
                let mine = caller.as_deref() == Some(p.owner.as_str());
                let versions: Vec<Value> = p
                    .versions
                    .iter()
                    .filter(|v| mine || matches!(v.status.as_str(), "approved" | "yanked"))
                    .map(|v| json!({
                        "version": v.version, "status": v.status, "yanked": v.status == "yanked", "notes": v.notes,
                        "hash": v.hash, "sha256": sha256_hex(&v.raw), "bytes": bytes_of(&state, v), "key": v.key,
                    }))
                    .collect();
                json_reply(200, json!({ "versions": versions }))
            }
            ("GET", ["store", "v1", "plugins", id, "versions", version, "package"]) => {
                let mut state = self.state.lock().unwrap();
                if let Some(raw) = state.serve_instead.remove(&((*id).to_owned(), (*version).to_owned())) {
                    return (200, "application/octet-stream", raw);
                }
                if let Some(location) = state.redirect_package.take() {
                    return (302, "redirect", location.into_bytes());
                }
                let Some(p) = state.plugins.get(*id) else { return not_found() };
                let mine = caller.as_deref() == Some(p.owner.as_str());
                match p.versions.iter().find(|v| v.version == *version) {
                    Some(v) if v.status == "approved" || v.status == "yanked" || (mine && v.status == "pending") => (200, "application/octet-stream", v.raw.clone()),
                    _ => not_found(),
                }
            }
            ("POST", ["store", "v1", "plugins", id, "reports"]) => {
                if caller.is_none() {
                    return json_reply(401, json!({ "error": "unauthorized", "message": "Sign in." }));
                }
                let body: Value = serde_json::from_slice(body).unwrap_or_default();
                self.state.lock().unwrap().reports.push(((*id).to_owned(), body));
                json_reply(201, json!({ "reported": true }))
            }
            ("GET", ["store", "v1", "plugins", id, "files", version, rest @ ..]) => {
                let state = self.state.lock().unwrap();
                let Some(v) = state.plugins.get(*id).and_then(|p| p.versions.iter().find(|v| v.version == *version)) else { return not_found() };
                let package: Value = serde_json::from_slice(&v.raw).unwrap();
                let path = rest.join("/");
                match package["files"][path.as_str()].as_str() {
                    Some(b64) => (200, "application/octet-stream", base64::engine::general_purpose::STANDARD.decode(b64).unwrap()),
                    None => not_found(),
                }
            }
            ("POST", ["store", "v1", "plugins", id, "installed"]) => {
                if caller.is_none() {
                    return json_reply(401, json!({ "error": "unauthorized", "message": "Sign in." }));
                }
                let mut state = self.state.lock().unwrap();
                let Some(p) = state.plugins.get_mut(*id) else { return not_found() };
                p.installs += 1;
                json_reply(200, json!({}))
            }
            ("POST", ["store", "v1", "updates"]) => {
                let ask: Value = serde_json::from_slice(body).unwrap_or_default();
                let mut state = self.state.lock().unwrap();
                state.asks.push(ask.clone());
                let zeron = Version::parse(ask["zeron"].as_str().unwrap_or("")).ok();
                let mut updates = Vec::new();
                let mut blocked = Vec::new();
                for asked in ask["plugins"].as_array().cloned().unwrap_or_default() {
                    let (id, installed) = (asked["id"].as_str().unwrap_or(""), asked["version"].as_str().unwrap_or(""));
                    let Some(p) = state.plugins.get(id) else { continue };
                    let Ok(installed_v) = Version::parse(installed) else { continue };
                    if let Some(v) = p.versions.iter().find(|v| v.version == installed && v.blocked.is_some()) {
                        blocked.push(json!({ "id": id, "version": v.version, "reason": v.blocked }));
                    }
                    let mine = caller.as_deref() == Some(p.owner.as_str());
                    let fits = |v: &StoredVersion| {
                        v.engines.as_deref().is_none_or(|r| zeron.as_ref().is_some_and(|z| Range::parse(r).is_ok_and(|r| r.matches(z))))
                    };
                    let best = p
                        .versions
                        .iter()
                        .filter(|v| v.status == "approved" || (mine && v.status == "pending"))
                        .filter(|v| Version::parse(&v.version).is_ok_and(|x| x > installed_v && (!x.is_prerelease() || asked["prerelease"] == true)))
                        .filter(|v| fits(v))
                        .max_by(|a, b| Version::parse(&a.version).unwrap().cmp(&Version::parse(&b.version).unwrap()));
                    if let Some(v) = best {
                        updates.push(json!({
                            "id": id, "version": v.version, "hash": v.hash, "sha256": sha256_hex(&v.raw), "bytes": bytes_of(&state, v),
                            "key": v.key, "publisher": p.owner, "publisher_id": publisher_id(p.account()), "notes": v.notes, "added": [], "removed": [],
                            "in_review": v.status == "pending",
                        }));
                    }
                }
                updates.append(&mut state.offer_anyway);
                blocked.extend(state.blocked_anyway.iter().cloned());
                json_reply(200, json!({ "updates": updates, "blocked": blocked }))
            }
            ("POST", ["store", "v1", "plugins", id, "versions"]) => {
                let Some((owner, scope)) = who else {
                    return json_reply(401, json!({ "error": "unauthorized", "message": "Sign in." }));
                };
                if scope.as_deref().is_some_and(|s| s != *id) {
                    return json_reply(403, json!({ "error": "key_not_allowed", "message": "That key is for another plugin." }));
                }
                let package = match install::parse(body.to_vec()) {
                    Ok(p) => p,
                    Err(e) => return json_reply(400, json!({ "error": "bad_package", "message": e })),
                };
                if package.id() != *id {
                    return json_reply(400, json!({ "error": "bad_package", "message": "Its id isn't this plugin's." }));
                }
                if Version::parse(&package.manifest.version).is_err() {
                    return json_reply(400, json!({ "error": "not_semver", "message": "Not a version." }));
                }
                let mut state = self.state.lock().unwrap();
                let plugin = state.plugins.entry((*id).to_owned()).or_insert_with(|| StoredPlugin { owner: owner.clone(), ..Default::default() });
                if plugin.owner != owner {
                    return json_reply(409, json!({ "error": "id_taken", "message": "Someone else has that id." }));
                }
                if plugin.versions.iter().any(|v| v.version == package.manifest.version) {
                    return json_reply(409, json!({ "error": "version_exists", "message": "That version was submitted already." }));
                }
                plugin.versions.push(StoredVersion {
                    version: package.manifest.version.clone(),
                    status: "pending".into(),
                    raw: body.to_vec(),
                    hash: package.hash.clone(),
                    key: package.key.clone(),
                    notes: String::new(),
                    engines: None,
                    blocked: None,
                });
                let via = if scope.is_some() { "deploy_key" } else { "session" };
                state.submitted.push(((*id).to_owned(), body.to_vec(), via.to_owned()));
                json_reply(201, json!({ "id": id, "version": package.manifest.version, "status": "pending", "checks": [] }))
            }
            ("GET", ["store", "v1", "me", "plugins"]) => {
                let Some((owner, scope)) = who else {
                    return json_reply(401, json!({ "error": "unauthorized", "message": "Sign in." }));
                };
                let state = self.state.lock().unwrap();
                let plugins: Vec<Value> = state
                    .plugins
                    .iter()
                    .filter(|(id, p)| p.owner == owner && scope.as_deref().is_none_or(|s| s == id.as_str()))
                    .map(|(id, p)| carded(&state, json!({
                        "id": id, "author": p.author(),
                        "versions": p.versions.iter().map(|v| json!({
                            "version": v.version, "status": v.status, "yanked": v.status == "yanked",
                            "hash": v.hash, "sha256": sha256_hex(&v.raw), "bytes": bytes_of(&state, v), "key": v.key,
                        })).collect::<Vec<_>>(),
                    })))
                    .collect();
                json_reply(200, json!({ "plugins": plugins }))
            }
            ("POST", ["store", "v1", "plugins", id, "versions", version, action]) => {
                let Some((owner, scope)) = who else {
                    return json_reply(401, json!({ "error": "unauthorized", "message": "Sign in." }));
                };
                let mut state = self.state.lock().unwrap();
                let Some(p) = state.plugins.get_mut(*id) else { return not_found() };
                if p.owner != owner || scope.as_deref().is_some_and(|s| s != *id) {
                    return json_reply(403, json!({ "error": "forbidden", "message": "Not yours." }));
                }
                let Some(v) = p.versions.iter_mut().find(|v| v.version == *version) else { return not_found() };
                v.status = match *action {
                    "withdraw" => "withdrawn",
                    "yank" => "yanked",
                    "unyank" => "approved",
                    _ => return not_found(),
                }
                .into();
                json_reply(200, json!({ "id": id, "version": version, "status": v.status }))
            }
            _ => not_found(),
        }
    }
}

fn chrono_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64
}

async fn read_request(socket: &mut tokio::net::TcpStream) -> Option<(String, String, BTreeMap<String, String>, Vec<u8>)> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let header_end = loop {
        let n = socket.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(at) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break at + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let (method, path) = (first.next()?.to_owned(), first.next()?.to_owned());
    let headers: BTreeMap<String, String> = lines
        .filter_map(|l| l.split_once(": "))
        .map(|(k, v)| (k.to_ascii_lowercase(), v.to_owned()))
        .collect();
    let length: usize = headers.get("content-length").and_then(|l| l.parse().ok()).unwrap_or(0);
    let mut body = buf[header_end..].to_vec();
    while body.len() < length {
        let n = socket.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    Some((method, path, headers, body))
}
