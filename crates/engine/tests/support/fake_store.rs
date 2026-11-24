//! A stand-in for the edge's plugin store (`/store/v1`), over real HTTP on
//! a random local port: enough of the contract (amendments 44 and 45) for a
//! device to install, update, be told of take-downs, and publish. Shared by
//! the engine's store tests and the CLI's (`apps/zeron/tests`).
//!
//! Sessions are bearer tokens naming a username (`session:<username>`); a
//! deploy token (`Bearer zdt_…`) is checked as the contract says (its form
//! and checksum, then its hash: live, unexpired, in scope for a plugin its
//! account owns). It can be made hostile on purpose (`serve_instead`,
//! `offer_anyway`), or go offline.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use serde_json::{Value, json};
use sha2::Digest as _;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use zeron_engine::plugins::install;
use zeron_engine::plugins::store::client::Kind;
use zeron_engine::plugins::store::deploy_token::{DeployToken, Scope};
use zeron_engine::plugins::version::{Range, Version};

#[derive(Clone, Debug)]
pub struct StoredVersion {
    pub version: String,
    /// `listed`, `yanked`, `taken_down` or `removed`.
    pub status: String,
    /// A reviewer verified it.
    pub verified: bool,
    pub raw: Vec<u8>,
    pub hash: String,
    /// Who signed it; none for a deploy token's (unsigned).
    pub key: Option<String>,
    pub notes: String,
    pub engines: Option<String>,
    /// Taken down, with this reason.
    pub blocked: Option<String>,
    /// `device` or `deploy_token`.
    pub via: String,
    /// The deploy token that sent it: `(id, short, name)`.
    pub deploy_token: Option<(String, String, String)>,
    /// When the store took it (ms).
    pub published_at: i64,
    /// Yanked because a newer version was listed while this one wasn't
    /// verified: the newer version (the author can't unyank it).
    pub superseded_by: Option<String>,
    /// The account that published it (fixed at publish: a transfer leaves
    /// it the former owner's).
    pub publisher_account: String,
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

/// A deploy token, as the store keeps it: never the token, its hash.
#[derive(Clone, Debug)]
pub struct StoredToken {
    pub id: String,
    pub short: String,
    pub owner: String,
    pub scope: Scope,
    pub name: String,
    pub expires_at: Option<i64>,
    pub revoked: bool,
}

#[derive(Default)]
pub struct State {
    pub plugins: BTreeMap<String, StoredPlugin>,
    /// Deploy tokens: the token's SHA-256 (hex) → the token.
    pub deploy_tokens: BTreeMap<String, StoredToken>,
    /// Tokens made so far (each one's randomness).
    pub tokens_made: u8,
    /// Each request: `(method, path, authorization)`.
    pub requests: Vec<(String, String, Option<String>)>,
    /// What the next package download of `(id, version)` serves instead.
    pub serve_instead: BTreeMap<(String, String), Vec<u8>>,
    /// Offers made whatever the device asked (a hostile store).
    pub offer_anyway: Vec<Value>,
    /// Packages submitted: `(id, raw, via)`.
    pub submitted: Vec<(String, Vec<u8>, String)>,
    /// Templates: each one's owner and versions, as published.
    pub templates: BTreeMap<String, (String, Vec<String>)>,
    /// A store from before templates: their routes aren't there.
    pub without_templates: bool,
    /// Templates submitted: `(id, package)`.
    pub submitted_templates: Vec<(String, Vec<u8>)>,
    /// Updates asks, as sent.
    pub asks: Vec<Value>,
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
    /// The take-down feed: `(seq, kind, id, version, reason)`, in order.
    pub events: Vec<(u64, String, String, String, String)>,
    /// Those events that are templates' (`item_kind: "template"`), by seq;
    /// the rest are plugins'.
    pub template_events: std::collections::BTreeSet<u64>,
    /// Events at or below this `seq` are forgotten (older than the feed
    /// keeps): asking after one answers a reset.
    pub feed_floor: u64,
    /// The `head` the feed answers instead of its last event's (a broken
    /// or hostile store: events past it).
    pub feed_head: Option<u64>,
    /// The last time the store's clock gave (`State::tick`).
    pub clock: i64,
    /// Version records answered without `verified` (a broken or hostile
    /// store: readers require it).
    pub records_without_verified: bool,
    /// Update offers answered without `verified`.
    pub offers_without_verified: bool,
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

/// Who sent a request: a username, and a deploy token's scope and hash.
type Who = (String, Option<(Scope, String)>);

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

    /// Listed versions, newest first by precedence.
    fn listed(&self) -> Vec<&StoredVersion> {
        let mut listed: Vec<&StoredVersion> = self.versions.iter().filter(|v| v.status == "listed").collect();
        listed.sort_by(|a, b| Version::parse(&b.version).unwrap().cmp(&Version::parse(&a.version).unwrap()));
        listed
    }

    /// The card's `version`: the newest listed release, else pre-release.
    fn newest(&self) -> Option<&StoredVersion> {
        let listed = self.listed();
        listed.iter().find(|v| !Version::parse(&v.version).unwrap().is_prerelease()).or(listed.first()).copied()
    }

    /// The newest verified listed version.
    fn newest_verified(&self) -> Option<&StoredVersion> {
        self.listed().into_iter().find(|v| v.verified)
    }

    /// As the store when version `newer` is listed: every older listed
    /// version not verified is yanked, superseded by it (a release
    /// supersedes older releases and pre-releases; a pre-release older
    /// pre-releases only).
    fn supersede(&mut self, newer: &str) {
        let newer_v = Version::parse(newer).unwrap();
        for v in &mut self.versions {
            let older = Version::parse(&v.version).unwrap();
            let on_its_line = !newer_v.is_prerelease() || older.is_prerelease();
            if v.status == "listed" && !v.verified && older < newer_v && on_its_line {
                v.status = "yanked".into();
                v.superseded_by = Some(newer.to_owned());
            }
        }
    }

    /// Version `version` superseded by one listed now, if any is: the
    /// newest such.
    fn superseder_of(&self, version: &str) -> Option<String> {
        let older = Version::parse(version).unwrap();
        self.listed()
            .into_iter()
            .find(|v| {
                let newer = Version::parse(&v.version).unwrap();
                newer > older && (!newer.is_prerelease() || older.is_prerelease())
            })
            .map(|v| v.version.clone())
    }
}

impl State {
    /// The store's clock: now, and never the same twice.
    fn tick(&mut self) -> i64 {
        self.clock = (self.clock + 1).max(chrono_now());
        self.clock
    }
}

/// A version as the public sees it (`VersionInfo`).
fn version_info(state: &State, v: &StoredVersion) -> Value {
    let mut info = json!({
        "version": v.version, "status": v.status, "yanked": v.status == "yanked",
        "prerelease": Version::parse(&v.version).is_ok_and(|x| x.is_prerelease()), "notes": v.notes,
        "engines": { "zeron": v.engines }, "key": v.key, "hash": v.hash, "sha256": sha256_hex(&v.raw),
        "bytes": bytes_of(state, v), "verified": v.verified, "verified_at": v.verified.then_some(1), "via": v.via,
        "published_at": v.published_at,
        "yanked_by": match (v.status.as_str(), &v.superseded_by) {
            ("yanked", Some(_)) => json!("superseded"),
            ("yanked", None) => json!("author"),
            _ => Value::Null,
        },
    });
    if let (true, Some(by)) = (v.status == "yanked", &v.superseded_by) {
        info["superseded_by"] = json!(by);
    }
    info
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

    /// Add version `raw` of its plugin, owned by `owner`: `verified` (listed
    /// and verified), `listed` (not verified) or `yanked`. Listed, it
    /// supersedes older unverified versions, as a publish does. Unsigned,
    /// it's a deploy token's (`via: "deploy_token"`, its record naming no
    /// key).
    pub fn add(&self, owner: &str, raw: &[u8], state: &str, notes: &str) {
        let package = install::parse(raw.to_vec()).or_else(|_| install::parse_from_store(raw.to_vec(), false)).expect("a valid package");
        let by_token = package.key.is_none();
        let engines = package.manifest.raw["engines"]["zeron"].as_str().map(str::to_owned);
        let mut all = self.state.lock().unwrap();
        let published_at = all.tick();
        let plugin = all.plugins.entry(package.id().to_owned()).or_default();
        if plugin.owner.is_empty() {
            plugin.owner = owner.to_owned();
        }
        plugin.versions.push(StoredVersion {
            version: package.manifest.version.clone(),
            status: if state == "verified" { "listed".into() } else { state.to_owned() },
            verified: state == "verified",
            raw: raw.to_vec(),
            hash: package.hash.clone(),
            key: package.key.clone(),
            notes: notes.to_owned(),
            engines,
            blocked: None,
            via: if by_token { "deploy_token" } else { "device" }.into(),
            deploy_token: by_token.then(|| ("tok_0000000000000000".to_owned(), "AAAAAAAA".to_owned(), "CI".to_owned())),
            published_at,
            superseded_by: None,
            publisher_account: String::new(),
        });
        let account = plugin.account().to_owned();
        plugin.versions.last_mut().unwrap().publisher_account = account;
        if state != "yanked" {
            plugin.supersede(&package.manifest.version);
        }
    }

    fn version_mut<'a>(state: &'a mut State, id: &str, version: &str) -> &'a mut StoredVersion {
        state.plugins.get_mut(id).unwrap().versions.iter_mut().find(|v| v.version == version).unwrap()
    }

    pub fn set_status(&self, id: &str, version: &str, status: &str) {
        Self::version_mut(&mut self.state.lock().unwrap(), id, version).status = status.to_owned();
    }

    /// A reviewer verifies it: a superseded yank lifts (every verified
    /// version is installable); an author's own stays.
    pub fn verify(&self, id: &str, version: &str) {
        let mut state = self.state.lock().unwrap();
        let v = Self::version_mut(&mut state, id, version);
        v.verified = true;
        if v.superseded_by.take().is_some() {
            v.status = "listed".into();
        }
    }

    /// Staff take it down: blocked on devices, and an event in the feed.
    pub fn block(&self, id: &str, version: &str, reason: &str) {
        let mut state = self.state.lock().unwrap();
        let v = Self::version_mut(&mut state, id, version);
        v.status = "taken_down".into();
        v.verified = false;
        v.blocked = Some(reason.to_owned());
        let seq = state.events.last().map_or(1, |e| e.0 + 1);
        state.events.push((seq, "take_down".into(), id.into(), version.into(), reason.into()));
    }

    /// Staff restore a version taken down: listed again, unverified.
    pub fn restore(&self, id: &str, version: &str) {
        let mut state = self.state.lock().unwrap();
        let superseder = state.plugins[id].superseder_of(version);
        let v = Self::version_mut(&mut state, id, version);
        v.blocked = None;
        // Back unverified, and superseded at once when anything newer on its
        // line is listed.
        v.status = if superseder.is_some() { "yanked" } else { "listed" }.into();
        v.superseded_by = superseder;
        let seq = state.events.last().map_or(1, |e| e.0 + 1);
        state.events.push((seq, "restore".into(), id.into(), version.into(), String::new()));
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

    /// `owner` has id `id` in the store, with no version at all (an id
    /// held, nothing published yet): a device may publish its first
    /// version, a deploy token may not.
    pub fn claim(&self, id: &str, owner: &str) {
        self.state.lock().unwrap().plugins.entry(id.to_owned()).or_insert_with(|| StoredPlugin { owner: owner.to_owned(), ..Default::default() });
    }

    /// Staff feature it.
    pub fn feature(&self, id: &str) {
        self.state.lock().unwrap().plugins.get_mut(id).unwrap().featured = true;
    }

    /// A deploy token of `owner`'s, named `name`, for `scope`, as the edge
    /// makes one: the token, said this once.
    pub fn add_deploy_token(&self, owner: &str, name: &str, scope: Scope) -> String {
        let mut state = self.state.lock().unwrap();
        state.tokens_made += 1;
        let token = DeployToken::from_random(&[state.tokens_made; 32]);
        let stored = StoredToken {
            id: format!("tok_{:016}", state.tokens_made),
            short: token.short().to_owned(),
            owner: owner.to_owned(),
            scope,
            name: name.to_owned(),
            expires_at: None,
            revoked: false,
        };
        state.deploy_tokens.insert(token.hash(), stored);
        token.as_str().to_owned()
    }

    pub fn installs(&self, id: &str) -> u32 {
        self.state.lock().unwrap().plugins.get(id).map_or(0, |p| p.installs)
    }

    /// Who a request is: a session's username, or a deploy token's owner
    /// (with its scope), as the edge reads one: its form and checksum, then
    /// its hash: unknown, revoked or expired answer 401 `invalid_token`; a
    /// token only publishes (else 401 `session_required`), and only plugins
    /// in its scope (else 403 `token_not_allowed`).
    fn who(&self, method: &str, path: &str, headers: &BTreeMap<String, String>) -> Result<Option<Who>, (u16, Value)> {
        let Some(auth) = headers.get("authorization") else { return Ok(None) };
        let Some(bearer) = auth.strip_prefix("Bearer ") else {
            return Err((401, json!({ "error": "unauthorized", "message": "Who's asking?" })));
        };
        if let Some(user) = bearer.strip_prefix("session:") {
            return Ok(Some((user.to_owned(), None)));
        }
        let bad = || Err((401, json!({ "error": "invalid_token", "message": "This deploy token is wrong, revoked or expired." })));
        let Ok(token) = DeployToken::parse(bearer) else { return bad() };
        let state = self.state.lock().unwrap();
        let Some(stored) = state.deploy_tokens.get(&token.hash()).filter(|t| !t.revoked && t.expires_at.is_none_or(|at| at > chrono_now())).cloned() else {
            return bad();
        };
        drop(state);
        let bare = path.split('?').next().unwrap_or(path);
        let bare_parts: Vec<&str> = bare.trim_start_matches('/').split('/').collect();
        let allowed = matches!((method, bare_parts.as_slice()), ("POST", ["store", "v1", "plugins" | "templates", _, "versions"]));
        if !allowed {
            return Err((401, json!({ "error": "session_required", "message": "A deploy token only publishes." })));
        }
        Ok(Some((stored.owner, Some((stored.scope, token.hash())))))
    }

    fn answer(&self, method: &str, path: &str, headers: &BTreeMap<String, String>, body: &[u8]) -> (u16, &'static str, Vec<u8>) {
        self.state.lock().unwrap().requests.push((method.to_owned(), path.to_owned(), headers.get("authorization").cloned()));
        let json_reply = |status: u16, value: Value| (status, "application/json", serde_json::to_vec(&value).unwrap());
        let who = match self.who(method, path, headers) {
            Ok(who) => who,
            Err((status, value)) => return json_reply(status, value),
        };
        let caller = who.as_ref().map(|(u, _)| u.clone());
        let parts: Vec<&str> = path.split('?').next().unwrap().trim_start_matches('/').split('/').collect();
        let query: BTreeMap<String, String> = path
            .split_once('?')
            .map_or("", |(_, q)| q)
            .split('&')
            .filter_map(|pair| pair.split_once('='))
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect();
        let not_found = || json_reply(404, json!({ "error": "not_found", "message": "No such plugin." }));
        let unauthorized = || json_reply(401, json!({ "error": "unauthorized", "message": "Sign in." }));
        match (method, parts.as_slice()) {
            ("GET", ["store", "v1", "categories"]) => json_reply(200, json!({ "categories": [{ "id": "productivity", "name": "Productivity", "count": 1 }] })),
            ("GET", ["store", "v1", "stats"]) => {
                let state = self.state.lock().unwrap();
                let plugins = state.plugins.values().filter(|p| p.newest().is_some()).count();
                let verified = state.plugins.values().filter(|p| p.newest().is_some_and(|v| v.verified)).count();
                json_reply(200, json!({ "plugins": plugins, "verified": verified }))
            }
            ("GET", ["store", "v1", "plugins"]) => {
                let state = self.state.lock().unwrap();
                let featured_only = query.get("featured").is_some_and(|v| v == "true");
                let verified_only = query.get("verified").is_some_and(|v| v == "1");
                // (As the edge: filters; the order is the listing's.)
                let items: Vec<Value> = state
                    .plugins
                    .iter()
                    .filter(|(_, p)| !featured_only || p.featured)
                    .filter_map(|(id, p)| {
                        let v = p.newest()?;
                        (!verified_only || v.verified).then(|| {
                            carded(&state, json!({
                                "id": id, "name": id, "version": v.version, "author": p.author(), "featured": p.featured,
                                "verified": v.verified, "verified_version": p.newest_verified().map(|v| v.version.clone()),
                            }))
                        })
                    })
                    .collect();
                // (One page: the total is what it lists.)
                json_reply(200, json!({ "total": items.len(), "items": items }))
            }
            ("GET", ["store", "v1", "plugins", id]) => {
                let state = self.state.lock().unwrap();
                let Some(p) = state.plugins.get(*id) else { return not_found() };
                let Some(v) = p.newest() else { return not_found() };
                let versions: Vec<Value> = p.listed().iter().take(5).map(|v| version_info(&state, v)).collect();
                json_reply(200, carded(&state, json!({
                    "id": id, "name": id, "version": v.version,
                    "author": p.author(),
                    "engines": { "zeron": v.engines },
                    "verified": v.verified,
                    "verified_version": p.newest_verified().map(|v| v.version.clone()),
                    "versions": versions,
                    "package": { "hash": v.hash, "sha256": sha256_hex(&v.raw), "bytes": bytes_of(&state, v), "key": v.key },
                })))
            }
            ("GET", ["store", "v1", "plugins", id, "versions"]) => {
                let state = self.state.lock().unwrap();
                let Some(p) = state.plugins.get(*id) else { return not_found() };
                let mut public: Vec<&StoredVersion> = p.versions.iter().filter(|v| matches!(v.status.as_str(), "listed" | "yanked")).collect();
                public.sort_by(|a, b| Version::parse(&b.version).unwrap().cmp(&Version::parse(&a.version).unwrap()));
                let from: usize = query.get("cursor").and_then(|c| c.parse().ok()).unwrap_or(0);
                let limit: usize = query.get("limit").and_then(|l| l.parse().ok()).unwrap_or(20);
                let page: Vec<Value> = public.iter().skip(from).take(limit).map(|v| version_info(&state, v)).collect();
                let mut reply = json!({ "versions": page, "total": public.len() });
                if from + limit < public.len() {
                    reply["next_cursor"] = json!((from + limit).to_string());
                }
                json_reply(200, reply)
            }
            ("GET", ["store", "v1", "plugins", id, "versions", version]) => {
                let state = self.state.lock().unwrap();
                let Some(v) = state.plugins.get(*id).and_then(|p| p.versions.iter().find(|v| v.version == *version)) else { return not_found() };
                if !matches!(v.status.as_str(), "listed" | "yanked") {
                    return not_found();
                }
                let mut info = version_info(&state, v);
                if state.records_without_verified {
                    info.as_object_mut().unwrap().remove("verified");
                }
                json_reply(200, info)
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
                match p.versions.iter().find(|v| v.version == *version) {
                    Some(v) if matches!(v.status.as_str(), "listed" | "yanked") => (200, "application/octet-stream", v.raw.clone()),
                    _ => not_found(),
                }
            }
            ("GET", ["store", "v1", "takedowns"]) => {
                let state = self.state.lock().unwrap();
                let head = state.feed_head.unwrap_or_else(|| state.events.last().map_or(0, |e| e.0));
                let after: Option<u64> = query.get("after").and_then(|a| a.parse().ok());
                match after {
                    Some(after) if after >= state.feed_floor => {
                        let events: Vec<Value> = state
                            .events
                            .iter()
                            .filter(|e| e.0 > after)
                            .take(500)
                            .map(|(seq, kind, id, version, reason)| {
                                let item_kind = if state.template_events.contains(seq) { "template" } else { "plugin" };
                                json!({ "seq": seq, "kind": kind, "item_kind": item_kind, "id": id, "version": version, "reason": reason })
                            })
                            .collect();
                        json_reply(200, json!({ "events": events, "head": head }))
                    }
                    _ => json_reply(200, json!({ "events": [], "head": head, "reset": true })),
                }
            }
            ("POST", ["store", "v1", "plugins", id, "reports"]) => {
                if caller.is_none() {
                    return unauthorized();
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
                    return unauthorized();
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
                    let fits = |v: &StoredVersion| {
                        v.engines.as_deref().is_none_or(|r| zeron.as_ref().is_some_and(|z| Range::parse(r).is_ok_and(|r| r.matches(z))))
                    };
                    let newer: Vec<&StoredVersion> = p
                        .listed()
                        .into_iter()
                        .filter(|v| Version::parse(&v.version).is_ok_and(|x| x > installed_v && (!x.is_prerelease() || asked["prerelease"] == true)))
                        .filter(|v| fits(v))
                        .collect();
                    let offer = |v: &StoredVersion| {
                        json!({
                            "id": id, "version": v.version, "hash": v.hash, "sha256": sha256_hex(&v.raw), "bytes": bytes_of(&state, v),
                            "key": v.key, "publisher": p.owner, "publisher_id": publisher_id(p.account()), "notes": v.notes, "added": [], "removed": [],
                            "prerelease": Version::parse(&v.version).is_ok_and(|x| x.is_prerelease()),
                            "verified": v.verified, "verified_at": v.verified.then_some(1),
                        })
                    };
                    let best = if asked["verified_only"] == true { newer.iter().find(|v| v.verified) } else { newer.first() };
                    if let Some(best) = best {
                        let mut item = offer(best);
                        if state.offers_without_verified {
                            item.as_object_mut().unwrap().remove("verified");
                        }
                        if !best.verified
                            && let Some(verified) = newer.iter().find(|v| v.verified)
                        {
                            item["verified_update"] = offer(verified);
                        }
                        updates.push(item);
                    }
                }
                updates.append(&mut state.offer_anyway);
                blocked.extend(state.blocked_anyway.iter().cloned());
                json_reply(200, json!({ "updates": updates, "blocked": blocked }))
            }
            ("POST", ["store", "v1", "plugins", id, "versions"]) => {
                let Some((owner, scope)) = who else { return unauthorized() };
                // A token's is unsigned (a device's, signed): read as such.
                let package = match install::parse_from_store(body.to_vec(), scope.is_none()) {
                    Ok(p) => p,
                    Err(e) => return json_reply(400, json!({ "error": "bad_package", "message": e })),
                };
                if package.id() != *id {
                    return json_reply(400, json!({ "error": "bad_package", "message": "Its id isn't this plugin's." }));
                }
                if Version::parse(&package.manifest.version).is_err() {
                    return json_reply(400, json!({ "error": "not_semver", "message": "Not a version." }));
                }
                // (As the edge: a listing's mark claim, with the fields the
                // device's check gives.)
                let (problems, _) = zeron_engine::plugins::listing::check(&package.manifest, &package.files);
                if let Some(p) = problems.iter().find(|p| p.rule == "bad_listing" && p.cause.is_some()) {
                    return json_reply(400, json!({ "error": "bad_listing", "message": p.words, "field": p.field, "cause": p.cause, "word": p.word }));
                }
                let mut state = self.state.lock().unwrap();
                let published_at = state.tick();
                let deploy_token = scope.as_ref().and_then(|(_, hash)| {
                    let token = state.deploy_tokens.get(hash)?;
                    Some((token.id.clone(), token.short.clone(), token.name.clone()))
                });
                // A token publishes plugins its account owns, in its scope,
                // that have a version already: never a new id, never a
                // plugin's first version (that comes from a device).
                if let Some((scope, _)) = &scope {
                    let owned = state.plugins.get(*id).is_some_and(|p| p.owner == owner && !p.versions.is_empty());
                    if !owned || !scope.covers(id, Kind::Plugin) {
                        return json_reply(403, json!({ "error": "token_not_allowed", "message": "This deploy token may not publish this plugin." }));
                    }
                }
                let plugin = state.plugins.entry((*id).to_owned()).or_insert_with(|| StoredPlugin { owner: owner.clone(), ..Default::default() });
                if plugin.owner != owner {
                    return json_reply(409, json!({ "error": "id_taken", "message": "Someone else has that id." }));
                }
                if plugin.versions.iter().any(|v| v.version == package.manifest.version) {
                    return json_reply(409, json!({ "error": "version_exists", "message": "That version was published already." }));
                }
                let via = if scope.is_some() { "deploy_token" } else { "device" };
                plugin.versions.push(StoredVersion {
                    version: package.manifest.version.clone(),
                    status: "listed".into(),
                    verified: false,
                    raw: body.to_vec(),
                    hash: package.hash.clone(),
                    key: package.key.clone(),
                    notes: String::new(),
                    engines: None,
                    blocked: None,
                    via: via.into(),
                    deploy_token,
                    published_at,
                    superseded_by: None,
                    publisher_account: String::new(),
                });
                let account = plugin.account().to_owned();
                plugin.versions.last_mut().unwrap().publisher_account = account;
                let before: Vec<String> = plugin.versions.iter().filter(|v| v.superseded_by.is_some()).map(|v| v.version.clone()).collect();
                plugin.supersede(&package.manifest.version);
                let superseded: Vec<String> = plugin.versions.iter().filter(|v| v.superseded_by.is_some() && !before.contains(&v.version)).map(|v| v.version.clone()).collect();
                state.submitted.push(((*id).to_owned(), body.to_vec(), via.to_owned()));
                json_reply(201, json!({ "id": id, "version": package.manifest.version, "status": "listed", "verified": false, "checks": [], "superseded": superseded }))
            }
            ("GET", ["store", "v1", "me", "templates"]) => {
                let Some((owner, None)) = who else { return unauthorized() };
                let state = self.state.lock().unwrap();
                if state.without_templates {
                    return not_found();
                }
                let templates: Vec<Value> = state.templates.iter().filter(|(_, (o, _))| *o == owner).map(|(id, _)| json!({ "id": id, "kind": "template" })).collect();
                json_reply(200, json!({ "publisher_id": publisher_id(&owner), "templates": templates }))
            }
            // Templates, under their own prefix: what a deploy token does.
            ("GET", ["store", "v1", "templates", id, "versions"]) => {
                let state = self.state.lock().unwrap();
                let Some((_, versions)) = state.templates.get(*id) else { return not_found() };
                let versions: Vec<Value> = versions.iter().rev().map(|v| json!({ "version": v, "status": "listed", "verified": false })).collect();
                json_reply(200, json!({ "total": versions.len(), "versions": versions }))
            }
            ("POST", ["store", "v1", "templates", id, "versions"]) => {
                let Some((owner, scope)) = who else { return unauthorized() };
                let mut state = self.state.lock().unwrap();
                let refused = |status: u16, word: &str, words: &str| json_reply(status, json!({ "error": word, "message": words }));
                if let Some((scope, _)) = &scope
                    && (!scope.covers(id, Kind::Template) || state.templates.get(*id).is_none_or(|(o, versions)| *o != owner || versions.is_empty()))
                {
                    return refused(403, "token_not_allowed", "This deploy token may not publish this template.");
                }
                let package = match zeron_engine::plugins::template_packages::parse_from_store(body.to_vec(), scope.is_none()) {
                    Ok(p) if p.id() == *id => p,
                    Ok(_) => return refused(400, "bad_package", "Its id isn't this template's."),
                    Err(e) => return refused(400, "bad_package", &e),
                };
                let version = package.version();
                let stored = state.templates.entry((*id).to_owned()).or_insert_with(|| (owner.clone(), Vec::new()));
                if stored.1.contains(&version) {
                    return refused(409, "version_exists", "That version was published already.");
                }
                stored.1.push(version.clone());
                state.submitted_templates.push(((*id).to_owned(), body.to_vec()));
                json_reply(201, json!({ "id": id, "version": version, "kind": "template", "status": "listed", "verified": false, "checks": [], "superseded": [] }))
            }
            ("GET", ["store", "v1", "me", "plugins"]) => {
                let Some((owner, scope)) = who else { return unauthorized() };
                let state = self.state.lock().unwrap();
                let plugins: Vec<Value> = state
                    .plugins
                    .iter()
                    .filter(|(id, p)| p.owner == owner && scope.as_ref().is_none_or(|(s, _)| s.covers(id, Kind::Plugin)))
                    .map(|(id, p)| carded(&state, json!({
                        "id": id, "author": p.author(),
                        "versions": p.versions.iter().map(|v| {
                            let mut info = version_info(&state, v);
                            // (Who and which key sent it: the publish notice's.)
                            info["publisher_id"] = json!(publisher_id(&v.publisher_account));
                            info["deploy_token"] = v.deploy_token.as_ref().map_or(Value::Null, |(id, short, name)| json!({ "id": id, "short": short, "name": name }));
                            info["yanked_by"] = match (v.status.as_str(), &v.superseded_by) {
                                ("yanked", Some(_)) => json!("superseded"),
                                ("yanked", None) => json!("author"),
                                _ => Value::Null,
                            };
                            info["superseded"] = v.superseded_by.as_ref().map_or(Value::Null, |by| json!({ "at": 1, "by": by }));
                            info["review"] = json!({ "decided": v.verified, "decision": if v.verified { json!("verified") } else { Value::Null } });
                            info
                        }).collect::<Vec<_>>(),
                    })))
                    .collect();
                // (The account's publisher id, plugins or none, as the edge's
                // ff4af27bc.)
                let account = state.plugins.values().find(|p| p.owner == owner).map_or(owner.as_str(), |p| p.account());
                let reply = json!({ "plugins": plugins, "publisher_id": publisher_id(account) });
                json_reply(200, reply)
            }
            ("GET", ["store", "v1", "me", "tokens"]) => {
                let Some((owner, None)) = who else { return unauthorized() };
                let state = self.state.lock().unwrap();
                let tokens: Vec<Value> = state
                    .deploy_tokens
                    .values()
                    .filter(|t| t.owner == owner)
                    .map(|t| json!({ "id": t.id, "short": t.short, "name": t.name, "scope": t.scope.to_value(), "expires_at": t.expires_at, "created_at": 1, "last_used_at": null, "revoked_at": if t.revoked { json!(2) } else { Value::Null } }))
                    .collect();
                json_reply(200, json!({ "tokens": tokens }))
            }
            ("POST", ["store", "v1", "me", "tokens"]) => {
                let Some((owner, None)) = who else { return unauthorized() };
                let request: Value = serde_json::from_slice(body).unwrap_or_default();
                if request["stepUp"].as_str().is_none_or(str::is_empty) {
                    return json_reply(403, json!({ "error": "step_up_required", "message": "Sign in again." }));
                }
                let scope = if request["scope"]["all"] == true {
                    let kinds: Option<Vec<String>> = serde_json::from_value(request["scope"]["all_kinds"].clone()).unwrap_or_default();
                    match Scope::all(kinds.as_deref()) {
                        Ok(scope) => scope,
                        Err(e) => return json_reply(400, json!({ "error": "invalid_scope", "message": e })),
                    }
                } else {
                    let ids: Vec<String> = request["scope"]["plugins"].as_array().into_iter().flatten().filter_map(|i| i.as_str().map(str::to_owned)).collect();
                    match Scope::plugins(&ids) {
                        Ok(scope) => scope,
                        Err(e) => return json_reply(400, json!({ "error": "invalid_scope", "message": e })),
                    }
                };
                {
                    let state = self.state.lock().unwrap();
                    if let Scope::Plugins(ids) = &scope {
                        let missing: Vec<&String> = ids.iter().filter(|id| state.plugins.get(*id).is_none_or(|p| p.owner != owner) && state.templates.get(*id).is_none_or(|(o, _)| *o != owner)).collect();
                        if !missing.is_empty() {
                            return json_reply(403, json!({ "error": "not_owner", "ids": missing, "message": "Not yours." }));
                        }
                    }
                }
                let name = request["name"].as_str().unwrap_or("").to_owned();
                let token = self.add_deploy_token(&owner, &name, scope.clone());
                let mut state = self.state.lock().unwrap();
                let hash = DeployToken::parse(&token).unwrap().hash();
                let stored = state.deploy_tokens.get_mut(&hash).unwrap();
                stored.expires_at = request["expires"].as_i64();
                json_reply(201, json!({ "token": token, "id": stored.id, "short": stored.short, "name": name, "scope": scope.to_value(), "expires_at": stored.expires_at, "created_at": 1 }))
            }
            ("DELETE", ["store", "v1", "me", "tokens", id]) => {
                let Some((owner, None)) = who else { return unauthorized() };
                let mut state = self.state.lock().unwrap();
                let Some(token) = state.deploy_tokens.values_mut().find(|t| t.id == *id && t.owner == owner) else {
                    return json_reply(404, json!({ "error": "unknown_token", "message": "No such deploy token." }));
                };
                token.revoked = true;
                json_reply(200, json!({ "revoked": true }))
            }
            ("POST", ["store", "v1", "plugins", id, "versions", version, action]) => {
                let Some((owner, scope)) = who else { return unauthorized() };
                let mut state = self.state.lock().unwrap();
                let Some(p) = state.plugins.get_mut(*id) else { return not_found() };
                if p.owner != owner || scope.is_some() {
                    return json_reply(403, json!({ "error": "forbidden", "message": "Not yours." }));
                }
                let Some(v) = p.versions.iter_mut().find(|v| v.version == *version) else { return not_found() };
                if *action == "unyank" && v.superseded_by.is_some() {
                    return json_reply(409, json!({ "error": "invalid_state", "message": "A newer version superseded this one before anyone verified it: it stays yanked." }));
                }
                v.status = match *action {
                    "yank" => "yanked",
                    "unyank" => "listed",
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
