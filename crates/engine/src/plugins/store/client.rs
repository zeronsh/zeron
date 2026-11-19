//! The store's HTTP routes at the edge (`/store/v1/…`, the plan's
//! contract), as a device calls them: anonymously, with the signed-in
//! session's bearer, or a deploy token's (CI).
//!
//! Replies the device acts on (a plugin's record, its package, the batched
//! update check) are read into types here, each field the contract names
//! and nothing assumed beyond it; the rest (cards, details, my plugins) is
//! handed on as the edge wrote it.
//!
//! A failure reads `"<word>: <words>"` like every plugin failure
//! (`install::failure`): the edge's error word when it gave one
//! (`id_taken`, `not_semver`, …), else `store_unreachable` (no answer),
//! `not_in_store` (404) or `store_error` (anything else).

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::deploy_token::DeployToken;
use crate::plugins::install::{self, failure};
use crate::plugins::listing;
use crate::plugins::package::MAX_PACKAGE_BYTES;

const TIMEOUT: Duration = Duration::from_secs(60);
/// The most a JSON reply may be.
const MAX_JSON_BYTES: u64 = 4 * 1024 * 1024;

/// The words a store call fails with, beyond the edge's own.
pub mod reason {
    /// The store didn't answer (offline, a timeout).
    pub const STORE_UNREACHABLE: &str = "store_unreachable";
    /// The store has no such plugin or version (to this caller).
    pub const NOT_IN_STORE: &str = "not_in_store";
    /// The store answered something that isn't a reply.
    pub const STORE_ERROR: &str = "store_error";
    /// What the store sent isn't what its record says (hash, version, key,
    /// publisher): never installed.
    pub const STORE_MISMATCH: &str = "store_mismatch";
}

/// How a request proves who sends it.
#[derive(Clone)]
pub enum Caller {
    /// Nobody: public reads.
    Anonymous,
    /// The signed-in session's bearer, when there is one (none: as
    /// anonymous).
    Session(Arc<dyn zeron_rpc::TokenSource>),
    /// A deploy token, as a bearer (CI).
    DeployToken(Arc<DeployToken>),
}

/// The edge's store routes.
#[derive(Clone)]
pub struct StoreClient {
    base: String,
    http: reqwest::Client,
}

/// `text` as one URL path segment.
fn segment(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Whether every segment of `path` (no query) is one the URL keeps as
/// written: never empty, `.` or `..` (spelled out or percent-encoded),
/// which a URL resolves away and so would send the request elsewhere.
fn plain_segments(path: &str) -> bool {
    path.split('?').next().unwrap_or(path).split('/').skip(1).all(|segment| {
        let lower = segment.to_ascii_lowercase().replace("%2e", ".");
        !segment.is_empty() && lower != "." && lower != ".."
    })
}

/// `url` as the edge the store CLI may sign requests for: `https://`, or
/// `http://` on this machine (loopback), with no path, query, fragment or
/// user. Its base, without a trailing slash.
pub fn edge_base(url: &str) -> Result<String, String> {
    let bad = |why: &str| format!("ZERON_EDGE_URL {why}");
    let parsed = url::Url::parse(url.trim()).map_err(|_| bad("isn't a URL"))?;
    let loopback = match parsed.host() {
        Some(url::Host::Domain(name)) => name.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => return Err(bad("has no host")),
    };
    match parsed.scheme() {
        "https" => {}
        "http" if loopback => {}
        _ => return Err(bad("is https (http only on this machine)")),
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(bad("can't carry a user or password"));
    }
    if parsed.path() != "/" || parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(bad("is the edge's address alone, with no path (like https://zeron.sh)"));
    }
    Ok(parsed.as_str().trim_end_matches('/').to_owned())
}

/// A `/`-separated path (a file in a package), each part a segment.
fn segments(path: &str) -> String {
    path.split('/').map(segment).collect::<Vec<_>>().join("/")
}

/// What a catalog listing asks (`GET /store/v1/plugins`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Search {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keyword: Option<String>,
    /// Only the plugins staff feature (`featured=true`); false or absent:
    /// every listed plugin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub featured: Option<bool>,
    /// Only plugins whose newest version is verified (`verified=1`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified: Option<bool>,
    /// One publisher's plugins, by username.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

impl Search {
    fn query(&self) -> String {
        let limit = self.limit.map(|l| l.clamp(1, 50).to_string());
        let pairs = [
            ("q", self.q.as_deref()),
            ("category", self.category.as_deref()),
            ("keyword", self.keyword.as_deref()),
            ("featured", (self.featured == Some(true)).then_some("true")),
            ("verified", (self.verified == Some(true)).then_some("1")),
            ("author", self.author.as_deref()),
            ("sort", self.sort.as_deref()),
            ("cursor", self.cursor.as_deref()),
            ("limit", limit.as_deref()),
        ];
        let query: Vec<String> = pairs
            .iter()
            .filter_map(|(k, v)| v.map(str::trim).filter(|v| !v.is_empty()).map(|v| format!("{k}={}", segment(v))))
            .collect();
        if query.is_empty() { String::new() } else { format!("?{}", query.join("&")) }
    }
}

/// The package a store record names (`PluginDetail.package`, a version's).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PackageRecord {
    /// The plugin's content hash (`signing::content_hash`).
    #[serde(default, deserialize_with = "nullable")]
    pub hash: String,
    /// The package file's sha256.
    #[serde(default, deserialize_with = "nullable")]
    pub sha256: String,
    #[serde(default, deserialize_with = "nullable")]
    pub bytes: u64,
    /// The key that signed it, when the store says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

/// Who published a plugin (`PluginCard.author`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Publisher {
    /// The owner's stable, opaque id (`pub_…`).
    #[serde(default, deserialize_with = "nullable")]
    pub id: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub username: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub verified: bool,
}

/// What the device needs of `GET /store/v1/plugins/:id` (the rest of the
/// detail is passed on as it came).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Detail {
    #[serde(default, deserialize_with = "nullable")]
    pub id: String,
    #[serde(default, deserialize_with = "nullable")]
    pub version: String,
    #[serde(default, deserialize_with = "nullable")]
    pub author: Publisher,
    #[serde(default, deserialize_with = "nullable")]
    pub package: PackageRecord,
    /// Its `version` is verified.
    #[serde(default, deserialize_with = "nullable")]
    pub verified: bool,
    /// The newest verified listed version, if any (installed through the
    /// one-version route).
    #[serde(default, deserialize_with = "nullable")]
    pub verified_version: Option<String>,
}

/// One version, as `GET /store/v1/plugins/:id/versions` lists it and
/// `…/versions/:version` answers it.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct VersionRecord {
    #[serde(default, deserialize_with = "nullable")]
    pub version: String,
    /// `listed` or `yanked` (what the public sees).
    #[serde(default, deserialize_with = "nullable")]
    pub status: String,
    #[serde(default, deserialize_with = "nullable")]
    pub yanked: bool,
    /// A reviewer verified it. Required: a version that doesn't say isn't
    /// read.
    pub verified: bool,
    #[serde(default, deserialize_with = "nullable")]
    pub verified_at: Option<i64>,
    #[serde(default, deserialize_with = "nullable")]
    pub hash: String,
    #[serde(default, deserialize_with = "nullable")]
    pub sha256: String,
    #[serde(default, deserialize_with = "nullable")]
    pub bytes: u64,
    #[serde(default, deserialize_with = "nullable")]
    pub key: Option<String>,
}

/// One installed plugin, as the batched update check names it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Installed {
    pub id: String,
    pub version: String,
    pub hash: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub prerelease: bool,
}

/// `POST /store/v1/updates`'s ask.
#[derive(Debug, Clone, Serialize)]
pub struct UpdatesAsk {
    pub zeron: String,
    pub plugins: Vec<Installed>,
}

/// A newer version the store offers.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Offer {
    pub id: String,
    pub version: String,
    /// Its content hash.
    pub hash: String,
    #[serde(default, deserialize_with = "nullable")]
    pub sha256: String,
    #[serde(default, deserialize_with = "nullable")]
    pub bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// Who owns the id now (its username).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher: Option<String>,
    /// And their stable id (`pub_…`): what a changed owner is told by.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher_id: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub notes: String,
    #[serde(default, deserialize_with = "nullable")]
    pub added: Vec<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub removed: Vec<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub prerelease: bool,
    /// A reviewer verified it. Required: an offer that doesn't say is
    /// unreadable.
    pub verified: bool,
    #[serde(default, deserialize_with = "nullable")]
    pub verified_at: Option<i64>,
    /// When this offer isn't verified: the newest verified version newer
    /// than the one installed, if there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified_update: Option<Box<Offer>>,
}

/// The take-down feed (`GET /store/v1/takedowns?after=<seq>`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Takedowns {
    #[serde(default, deserialize_with = "nullable")]
    pub events: Vec<TakedownEvent>,
    /// The last `seq` there is: where to continue from after a reset.
    #[serde(default, deserialize_with = "nullable")]
    pub head: u64,
    /// The device's place is gone (or it had none): run the full update
    /// check, then continue from `head`.
    #[serde(default, deserialize_with = "nullable")]
    pub reset: bool,
}

/// A version taken down or restored, in the order the store did it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct TakedownEvent {
    pub seq: u64,
    /// `take_down` or `restore`.
    pub kind: String,
    pub id: String,
    pub version: String,
    #[serde(default, deserialize_with = "nullable")]
    pub reason: String,
}

/// A version taken down and blocked: devices turn it off.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Block {
    pub id: String,
    pub version: String,
    #[serde(default, deserialize_with = "nullable")]
    pub reason: String,
}

/// `POST /store/v1/updates`'s answer: each offer and block read on its
/// own, so one the device can't read (a field missing, the wrong shape)
/// never hides the others, a block least of all.
#[derive(Debug, Clone, Default)]
pub struct Updates {
    pub updates: Vec<Offer>,
    pub blocked: Vec<Block>,
    /// Offers or blocks that couldn't be read, each as the store sent it
    /// (its `id`, when it had one).
    pub unreadable: Vec<Option<String>>,
}

impl Offer {
    /// Who the store says owns the plugin now.
    pub fn publisher(&self) -> Publisher {
        Publisher { id: self.publisher_id.clone(), username: self.publisher.clone(), verified: false }
    }
}

impl Updates {
    fn read(value: &Value) -> Self {
        let mut out = Updates::default();
        let items = |key: &str| value.get(key).and_then(Value::as_array).cloned().unwrap_or_default();
        for item in items("updates") {
            match serde_json::from_value::<Offer>(item.clone()) {
                Ok(offer) => out.updates.push(offer),
                Err(_) => out.unreadable.push(item["id"].as_str().map(str::to_owned)),
            }
        }
        for item in items("blocked") {
            match serde_json::from_value::<Block>(item.clone()) {
                Ok(block) => out.blocked.push(block),
                Err(_) => out.unreadable.push(item["id"].as_str().map(str::to_owned)),
            }
        }
        out
    }
}

/// A field the store may send as `null` for "none": read as its default.
fn nullable<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

impl StoreClient {
    /// The store at the edge `edge_url`.
    pub fn new(edge_url: &str) -> Self {
        Self {
            base: edge_url.trim_end_matches('/').to_owned(),
            http: reqwest::Client::builder()
                .timeout(TIMEOUT)
                .user_agent(concat!("Zeron/", env!("CARGO_PKG_VERSION")))
                // Nothing in the store's contract redirects: a misroute
                // fails here, loudly, rather than being followed elsewhere.
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    /// Plugin `id`'s page on the store's website: the site beside this edge
    /// (`edge.zeron.sh` → `zeron.sh`), at `/plugins/<id>`.
    pub fn page_url(&self, id: &str) -> String {
        let site = url::Url::parse(&self.base)
            .ok()
            .map(|mut url| {
                if let Some(host) = url.host_str().and_then(|h| h.strip_prefix("edge.")).map(str::to_owned) {
                    let _ = url.set_host(Some(&host));
                }
                url.as_str().trim_end_matches('/').to_owned()
            })
            .unwrap_or_else(|| self.base.clone());
        format!("{site}/plugins/{}", segment(id))
    }

    /// `method path` (path from `/store/v1`, query included) with `body`,
    /// as `caller`, its reply read up to `limit` bytes: `(status, bytes)` for
    /// a 2xx, else the failure.
    async fn send(&self, method: reqwest::Method, path: &str, body: Option<(Vec<u8>, &str)>, caller: &Caller, limit: u64) -> Result<Vec<u8>, String> {
        self.exchange(method, path, body, caller, limit).await.map_err(|(failure, _)| failure)
    }

    /// As [`Self::send`], a refusal with its listing fields ([`refused_fields`]).
    async fn exchange(&self, method: reqwest::Method, path: &str, body: Option<(Vec<u8>, &str)>, caller: &Caller, limit: u64) -> Result<Vec<u8>, (String, Value)> {
        let plain = |failure: String| (failure, Value::Null);
        if !plain_segments(path) {
            return Err(plain(failure(reason::NOT_IN_STORE, "That isn't a name the store has")));
        }
        let url = format!("{}{path}", self.base);
        let mut request = self.http.request(method.clone(), &url);
        match caller {
            Caller::Anonymous => {}
            Caller::Session(token) => {
                if let Ok(token) = token.token().await {
                    request = request.bearer_auth(token);
                }
            }
            Caller::DeployToken(token) => {
                // (Marked sensitive: never in a log or a debug print.)
                let Ok(mut value) = reqwest::header::HeaderValue::from_str(&token.authorization()) else {
                    return Err(plain(failure(super::deploy_token::reason::INVALID_TOKEN, super::deploy_token::DAMAGED)));
                };
                value.set_sensitive(true);
                request = request.header(reqwest::header::AUTHORIZATION, value);
            }
        }
        if let Some((bytes, content_type)) = body {
            request = request.header("Content-Type", content_type).body(bytes);
        }
        let unreachable = |_| failure(reason::STORE_UNREACHABLE, "The store can't be reached right now");
        let mut response = request.send().await.map_err(unreachable).map_err(plain)?;
        let status = response.status().as_u16();
        let cap = if (200..300).contains(&status) { limit } else { MAX_JSON_BYTES };
        if response.content_length().is_some_and(|n| n > cap) {
            return Err(plain(failure(install::reason::TOO_BIG, "The store's answer is too big")));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(unreachable).map_err(plain)? {
            bytes.extend_from_slice(&chunk);
            if bytes.len() as u64 > cap {
                return Err(plain(failure(install::reason::TOO_BIG, "The store's answer is too big")));
            }
        }
        if (200..300).contains(&status) {
            return Ok(bytes);
        }
        Err((refusal(status, &bytes), refused_fields(&bytes)))
    }

    async fn json(&self, method: reqwest::Method, path: &str, body: Option<Value>, caller: &Caller) -> Result<Value, String> {
        let body = body.map(|b| (serde_json::to_vec(&b).unwrap_or_default(), "application/json"));
        let bytes = self.send(method, path, body, caller, MAX_JSON_BYTES).await?;
        if bytes.is_empty() {
            return Ok(Value::Object(Default::default()));
        }
        object(&bytes)
    }

    fn typed<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, String> {
        serde_json::from_value(value).map_err(|_| failure(reason::STORE_ERROR, "The store's answer couldn't be read"))
    }

    // ── Public ──────────────────────────────────────────────────────────

    pub async fn search(&self, search: &Search, caller: &Caller) -> Result<Value, String> {
        self.json(reqwest::Method::GET, &format!("/store/v1/plugins{}", search.query()), None, caller).await
    }

    pub async fn categories(&self) -> Result<Value, String> {
        self.json(reqwest::Method::GET, "/store/v1/categories", None, &Caller::Anonymous).await
    }

    /// The plugin's detail, as the store wrote it, and what the device acts on.
    pub async fn plugin(&self, id: &str, caller: &Caller) -> Result<(Value, Detail), String> {
        let value = self.json(reqwest::Method::GET, &format!("/store/v1/plugins/{}", segment(id)), None, caller).await?;
        let detail = Self::typed(value.clone())?;
        Ok((value, detail))
    }

    /// One page of `id`'s listed and yanked versions, newest first:
    /// `{versions, total, next_cursor?, prev_cursor?}`, as the store wrote it.
    pub async fn versions(&self, id: &str, cursor: Option<&str>, limit: Option<u32>, caller: &Caller) -> Result<Value, String> {
        let mut query = Vec::new();
        if let Some(cursor) = cursor.map(str::trim).filter(|c| !c.is_empty()) {
            query.push(format!("cursor={}", segment(cursor)));
        }
        if let Some(limit) = limit {
            query.push(format!("limit={}", limit.clamp(1, 50)));
        }
        let query = if query.is_empty() { String::new() } else { format!("?{}", query.join("&")) };
        self.json(reqwest::Method::GET, &format!("/store/v1/plugins/{}/versions{query}", segment(id)), None, caller).await
    }

    /// Version `version` of `id`, as the store records it (listed or
    /// yanked): what a device installs a named version by.
    /// A record that can't be read (one without `verified`, say: readers
    /// require it) is `store_mismatch`: nothing is installed by it.
    pub async fn version(&self, id: &str, version: &str, caller: &Caller) -> Result<(Value, VersionRecord), String> {
        let path = format!("/store/v1/plugins/{}/versions/{}", segment(id), segment(version));
        let value = self.json(reqwest::Method::GET, &path, None, caller).await?;
        let record = serde_json::from_value(value.clone())
            .map_err(|_| failure(reason::STORE_MISMATCH, format!("The store's record of {id} {version} can't be read (it doesn't say all a record says), so it wasn't installed")))?;
        Ok((value, record))
    }

    /// The take-down feed after `after` (none: from the start, which the
    /// store answers with a reset).
    pub async fn takedowns(&self, after: Option<u64>) -> Result<Takedowns, String> {
        let path = after.map_or_else(|| "/store/v1/takedowns".to_owned(), |seq| format!("/store/v1/takedowns?after={seq}"));
        Self::typed(self.json(reqwest::Method::GET, &path, None, &Caller::Anonymous).await?)
    }

    /// The store's totals, `{plugins, verified}`.
    pub async fn stats(&self) -> Result<Value, String> {
        self.json(reqwest::Method::GET, "/store/v1/stats", None, &Caller::Anonymous).await
    }

    /// Version `version`'s package file, up to the largest a package may be.
    /// Read no further than `bytes`, the size the store's record says (a
    /// package can be): anything longer isn't it.
    pub async fn package(&self, id: &str, version: &str, caller: &Caller, bytes: u64) -> Result<Vec<u8>, String> {
        if bytes == 0 || bytes > MAX_PACKAGE_BYTES {
            return Err(failure(reason::STORE_MISMATCH, "The store's record doesn't give a size a package can be, so nothing was fetched"));
        }
        let path = format!("/store/v1/plugins/{}/versions/{}/package", segment(id), segment(version));
        self.send(reqwest::Method::GET, &path, None, caller, bytes).await.map_err(|error| {
            if install::reason_of(&error) == Some(install::reason::TOO_BIG) {
                failure(reason::STORE_MISMATCH, "What the store sent is bigger than its record says, so it wasn't installed")
            } else {
                error
            }
        })
    }

    /// One file of version `version`'s package, at most `limit` bytes.
    pub async fn file(&self, id: &str, version: &str, file: &str, caller: &Caller, limit: u64) -> Result<Vec<u8>, String> {
        let path = format!("/store/v1/plugins/{}/files/{}/{}", segment(id), segment(version), segments(file));
        self.send(reqwest::Method::GET, &path, None, caller, limit).await
    }

    /// The batched update check for every store-installed plugin.
    pub async fn updates(&self, ask: &UpdatesAsk, caller: &Caller) -> Result<Updates, String> {
        let body = serde_json::to_value(ask).unwrap_or_default();
        Ok(Updates::read(&self.json(reqwest::Method::POST, "/store/v1/updates", Some(body), caller).await?))
    }

    // ── Publisher ───────────────────────────────────────────────────────

    /// Submit `package` (a `.zeron-plugin`) as a new version of `id`. A
    /// refusal comes with its listing fields ([`refused_fields`]).
    pub async fn publish(&self, id: &str, package: Vec<u8>, caller: &Caller) -> Result<Value, (String, Value)> {
        let path = format!("/store/v1/plugins/{}/versions", segment(id));
        let bytes = self.exchange(reqwest::Method::POST, &path, Some((package, "application/json")), caller, MAX_JSON_BYTES).await?;
        object(&bytes).map_err(|failure| (failure, Value::Null))
    }

    pub async fn my_plugins(&self, caller: &Caller) -> Result<Value, String> {
        self.json(reqwest::Method::GET, "/store/v1/me/plugins", None, caller).await
    }

    /// `yank` or `unyank` version `version` of `id`.
    pub async fn version_action(&self, id: &str, version: &str, action: &str, caller: &Caller) -> Result<Value, String> {
        let path = format!("/store/v1/plugins/{}/versions/{}/{}", segment(id), segment(version), segment(action));
        self.json(reqwest::Method::POST, &path, None, caller).await
    }

    /// The account's deploy tokens (never their secrets).
    pub async fn tokens(&self, caller: &Caller) -> Result<Value, String> {
        self.json(reqwest::Method::GET, "/store/v1/me/tokens", None, caller).await
    }

    /// Make a deploy token: the answer carries it whole, this once.
    pub async fn create_token(&self, body: Value, caller: &Caller) -> Result<Value, String> {
        self.json(reqwest::Method::POST, "/store/v1/me/tokens", Some(body), caller).await
    }

    pub async fn revoke_token(&self, id: &str, caller: &Caller) -> Result<Value, String> {
        self.json(reqwest::Method::DELETE, &format!("/store/v1/me/tokens/{}", segment(id)), None, caller).await
    }

    /// Report a listed plugin (`reason`: malware, misleading, broken,
    /// copyright or other).
    pub async fn report(&self, id: &str, reason: &str, text: Option<&str>, caller: &Caller) -> Result<Value, String> {
        let mut body = serde_json::json!({ "reason": reason });
        if let Some(text) = text {
            body["text"] = serde_json::json!(text);
        }
        self.json(reqwest::Method::POST, &format!("/store/v1/plugins/{}/reports", segment(id)), Some(body), caller).await
    }

    /// Counted as installed (once per account, by the edge).
    pub async fn installed(&self, id: &str, caller: &Caller) -> Result<(), String> {
        self.json(reqwest::Method::POST, &format!("/store/v1/plugins/{}/installed", segment(id)), None, caller).await.map(drop)
    }
}

/// A reply that must be a JSON object (anything else isn't the store's).
fn object(bytes: &[u8]) -> Result<Value, String> {
    match serde_json::from_slice::<Value>(bytes) {
        Ok(value @ Value::Object(_)) => Ok(value),
        _ => Err(failure(reason::STORE_ERROR, "The store's answer couldn't be read")),
    }
}

/// A refusal's listing fields, those the edge gave and as a device would
/// give them: `field` (a manifest key), `cause` (one of
/// [`listing::cause::ALL`]) and `word` ([`listing::problem_word`]); an
/// object, empty when it gave none.
pub fn refused_fields(body: &[u8]) -> Value {
    let value: Value = serde_json::from_slice(body).unwrap_or_default();
    let mut fields = serde_json::Map::new();
    if let Some(field) = value["field"].as_str().filter(|f| !f.is_empty() && f.len() <= 32 && f.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')) {
        fields.insert("field".into(), serde_json::json!(field));
    }
    if let Some(cause) = value["cause"].as_str().filter(|c| listing::cause::ALL.contains(c)) {
        fields.insert("cause".into(), serde_json::json!(cause));
    }
    if let Some(word) = value["word"].as_str().map(listing::problem_word).filter(|w| !w.is_empty()) {
        fields.insert("word".into(), serde_json::json!(word));
    }
    Value::Object(fields)
}

/// A non-2xx answer as a failure: the edge's own word when it gave one.
fn refusal(status: u16, body: &[u8]) -> String {
    let value: Value = serde_json::from_slice(body).unwrap_or_default();
    let word = value["error"].as_str().filter(|w| !w.is_empty() && w.len() <= 64 && w.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'));
    let message: String = value["message"].as_str().unwrap_or("").chars().filter(|c| !listing::unseen_char(*c)).take(500).collect();
    match (word, status) {
        // The body changed on its way (a proxy, a broken connection): sending
        // it again is the fix.
        // (The edge's word for a plugin or version it doesn't show this caller.)
        (Some("not_found"), 404) => failure(reason::NOT_IN_STORE, if message.is_empty() { "The store has no such plugin".to_owned() } else { message }),
        (Some("body_mismatch"), _) => failure("body_mismatch", "What reached the store isn't what was signed (it changed on the way): send it again"),
        (Some(word), _) => failure(word, if message.is_empty() { format!("The store refused it ({status})") } else { message }),
        (None, 404) => failure(reason::NOT_IN_STORE, "The store has no such plugin"),
        (None, 429) => failure(install::reason::RATE_LIMITED, "The store is turning away requests from here for now; try again later"),
        (None, 500..) => failure(reason::STORE_UNREACHABLE, format!("The store couldn't answer just now ({status})")),
        // (No body to say more: who's asking wasn't accepted, or may not.)
        (None, 401) => failure("unauthenticated", "The store didn't accept who's asking (401)"),
        (None, 403) => failure("forbidden", "The store refused it (403)"),
        (None, 300..=399) => failure(reason::STORE_ERROR, format!("The store sent it elsewhere ({status}), which it never does: nothing was fetched")),
        (None, _) => failure(reason::STORE_ERROR, format!("The store refused it ({status})")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_and_queries_are_escaped() {
        assert_eq!(segment("1.0.0-rc.1+build.5"), "1.0.0-rc.1%2Bbuild.5");
        assert_eq!(segments("shots/a b.png"), "shots/a%20b.png");
        assert_eq!(segments("../x"), "../x", "(paths are checked before they get here)");
        let search = Search { q: Some("git & review".into()), limit: Some(500), sort: Some(" ".into()), ..Default::default() };
        assert_eq!(search.query(), "?q=git%20%26%20review&limit=50");
        assert_eq!(Search::default().query(), "");
        let featured = |featured| Search { featured, category: Some("git".into()), ..Default::default() }.query();
        assert_eq!(featured(Some(true)), "?category=git&featured=true");
        assert_eq!((featured(Some(false)), featured(None)), ("?category=git".into(), "?category=git".into()));
    }

    #[test]
    fn the_edge_the_cli_signs_for_is_its_address_alone() {
        assert_eq!(edge_base("https://zeron.sh").as_deref(), Ok("https://zeron.sh"));
        assert_eq!(edge_base("https://zeron.sh/").as_deref(), Ok("https://zeron.sh"));
        assert_eq!(edge_base("http://127.0.0.1:8787").as_deref(), Ok("http://127.0.0.1:8787"));
        assert_eq!(edge_base("http://localhost:8787/").as_deref(), Ok("http://localhost:8787"));
        assert_eq!(edge_base("http://[::1]:8787").as_deref(), Ok("http://[::1]:8787"));
        for refused in [
            "http://zeron.sh", "http://10.0.0.2:8787", "ftp://zeron.sh", "https://zeron.sh/api", "https://zeron.sh/?x=1",
            "https://zeron.sh/#x", "https://user:pw@zeron.sh", "zeron.sh", "",
        ] {
            assert!(edge_base(refused).is_err(), "{refused}");
        }
    }

    #[test]
    fn a_path_a_url_would_resolve_away_is_never_sent() {
        assert!(plain_segments("/store/v1/plugins/focus/keys/abc"));
        assert!(plain_segments("/store/v1/plugins?q=..%2e"), "(a query is the query's)");
        for path in ["/store/v1/plugins/focus/keys/..", "/store/v1/plugins/focus/keys/.", "/store/v1/plugins/focus/keys/",
                     "/store/v1/plugins/focus/keys/%2E%2e", "/store/v1/plugins/focus/keys/.%2E", "/store/v1/plugins//keys/x"] {
            assert!(!plain_segments(path), "{path}");
        }
        assert_eq!(segment(".."), "..", "(segment escapes; send() refuses what still resolves)");
    }

    #[tokio::test]
    async fn revoking_dot_dot_sends_nothing() {
        // (A closed port: had it been sent, the word would be store_unreachable.)
        let client = StoreClient::new("http://127.0.0.1:9");
        let error = client.revoke_token("..", &Caller::Anonymous).await.unwrap_err();
        assert_eq!(install::reason_of(&error), Some(reason::NOT_IN_STORE), "{error}");
    }

    #[test]
    fn refusals_carry_the_edge_s_word() {
        let said = |status, body: &str| refusal(status, body.as_bytes());
        assert_eq!(said(409, r#"{"error":"id_taken","message":"Someone has that id."}"#), "id_taken: Someone has that id.");
        assert_eq!(install::reason_of(&said(404, "")), Some(reason::NOT_IN_STORE));
        assert_eq!(install::reason_of(&said(503, "<html>")), Some(reason::STORE_UNREACHABLE));
        assert_eq!(install::reason_of(&said(429, "")), Some(install::reason::RATE_LIMITED));
        // The edge's id-claim limit (amendment 30): its word, its one line.
        let claims = said(429, r#"{"error":"rate_limited","message":"An account claims at most 5 new ids a week. Try again later, or publish a new version of one you have."}"#);
        assert_eq!(claims, "rate_limited: An account claims at most 5 new ids a week. Try again later, or publish a new version of one you have.");
        assert_eq!(said(404, r#"{"error":"not_found","message":"No such plugin."}"#), "not_in_store: No such plugin.");
        // A word the edge didn't really give (not a plain word) isn't taken as one.
        assert_eq!(install::reason_of(&said(400, r#"{"error":"Bad: thing"}"#)), Some(reason::STORE_ERROR));
        assert!(!said(400, "{\"error\":\"x\",\"message\":\"a\\u0007b\"}").contains('\u{7}'));
        // No body to say more: who's asking wasn't accepted (not "try again").
        assert_eq!(install::reason_of(&said(401, "")), Some("unauthenticated"));
        assert_eq!(install::reason_of(&said(403, "<html>")), Some("forbidden"));
        assert_eq!(install::reason_of(&said(302, "")), Some(reason::STORE_ERROR));
    }

    #[test]
    fn a_refusal_s_listing_fields_are_taken_clean() {
        let fields = |body: &str| refused_fields(body.as_bytes());
        let body = r#"{"error":"bad_listing","message":"Its name says \"verified\"","field":"name","cause":"mark_word","word":"Ver\u202eified"}"#;
        assert_eq!(fields(body), serde_json::json!({ "field": "name", "cause": "mark_word", "word": "Verified" }));
        // Only causes of the closed list, fields that are plain words, words of at most 64 characters.
        let odd = format!(r#"{{"field":"Name: x","cause":"made_up","word":"{}"}}"#, "w".repeat(100));
        assert_eq!(fields(&odd), serde_json::json!({ "word": "w".repeat(64) }));
        assert_eq!(fields("<html>"), serde_json::json!({}));
    }

    /// The store adds fields as it grows (amendments 39–41: `listed_at`,
    /// `icon`, `asks_changed`, more of `permissions_diff`): what the device
    /// reads takes what it names and lets the rest be, by design (no
    /// `deny_unknown_fields`), and passes cards through whole.
    #[test]
    fn fields_the_store_adds_later_are_let_be() {
        use serde_json::json;
        let author = json!({ "id": "pub_00000000000000000000000000000000", "username": "ada", "badges": [], "joined_at": 1 });
        let detail: Detail = StoreClient::typed(json!({
            "id": "focus", "version": "1.2.0", "author": author, "listed_at": 1_700_000_000_000_i64,
            "package": { "hash": "h", "sha256": "s", "bytes": 9, "url": "/x", "compressed": false },
            "permissions_diff": { "mcp_changed": null },
        }))
        .unwrap();
        assert_eq!((detail.id.as_str(), detail.author.username.as_deref(), detail.package.bytes), ("focus", Some("ada"), 9));
        let versions: Vec<VersionRecord> = StoreClient::typed(json!([{ "version": "1.2.0", "status": "listed", "hash": "h", "verified": true, "via": "device", "listed_at": 1, "asks_changed": true }])).unwrap();
        assert_eq!((versions[0].status.as_str(), versions[0].verified), ("listed", true));
        let updates = Updates::read(&json!({
            "updates": [{ "id": "focus", "version": "1.3.0", "hash": "h", "verified": false, "listed_at": 1, "icon": "icon.svg",
                          "verified_update": { "id": "focus", "version": "1.2.5", "hash": "g", "verified": true, "later": 1 } }],
            "blocked": [{ "id": "old", "version": "1.0.0", "reason": "r", "blocked_at": 1 }],
            "total": 2,
        }));
        assert_eq!((updates.updates.len(), updates.blocked.len(), updates.unreadable.len()), (1, 1, 0));
        assert_eq!(updates.updates[0].verified_update.as_ref().map(|v| (v.version.as_str(), v.verified)), Some(("1.2.5", true)));
        // `verified` is required: a version or an offer that doesn't say is
        // never read as either.
        assert!(StoreClient::typed::<VersionRecord>(json!({ "version": "1.2.0", "status": "listed", "hash": "h" })).is_err());
        let silent = Updates::read(&json!({ "updates": [{ "id": "focus", "version": "1.3.0", "hash": "h" }] }));
        assert_eq!((silent.updates.len(), silent.unreadable.clone()), (0, vec![Some("focus".to_owned())]));
    }
}
