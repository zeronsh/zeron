//! The store's HTTP routes at the edge (`/store/v1/…`, the plan's
//! contract), as a device calls them: anonymously, with the signed-in
//! session's bearer, or signed with a deploy key (CI).
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

use super::deploy_key::DeployKey;
use crate::plugins::install::{self, failure};
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
    /// A deploy key's signed request (CI).
    DeployKey(Arc<DeployKey>),
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
    #[serde(default)]
    pub hash: String,
    /// The package file's sha256.
    #[serde(default)]
    pub sha256: String,
    #[serde(default)]
    pub bytes: u64,
    /// The key that signed it, when the store says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

/// Who published a plugin (`PluginCard.author`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Publisher {
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub verified: bool,
}

/// What the device needs of `GET /store/v1/plugins/:id` (the rest of the
/// detail is passed on as it came).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Detail {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub author: Publisher,
    #[serde(default)]
    pub package: PackageRecord,
}

/// One version, as `GET /store/v1/plugins/:id/versions` lists it.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct VersionRecord {
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub yanked: bool,
    #[serde(default)]
    pub hash: String,
    #[serde(default)]
    pub sha256: String,
    #[serde(default)]
    pub bytes: u64,
    #[serde(default)]
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
    #[serde(default)]
    pub sha256: String,
    #[serde(default)]
    pub bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// Who owns the id now (its username).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher: Option<String>,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub added: Vec<String>,
    #[serde(default)]
    pub removed: Vec<String>,
    /// The caller's own version, still in review (offered to its author's
    /// devices alone).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub in_review: bool,
}

/// A version taken down and blocked: devices turn it off.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Block {
    pub id: String,
    pub version: String,
    #[serde(default)]
    pub reason: String,
}

/// `POST /store/v1/updates`'s answer.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Updates {
    #[serde(default)]
    pub updates: Vec<Offer>,
    #[serde(default)]
    pub blocked: Vec<Block>,
}

impl StoreClient {
    /// The store at the edge `edge_url`.
    pub fn new(edge_url: &str) -> Self {
        Self {
            base: edge_url.trim_end_matches('/').to_owned(),
            http: reqwest::Client::builder()
                .timeout(TIMEOUT)
                .user_agent(concat!("Zeron/", env!("CARGO_PKG_VERSION")))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    /// `method path` (path from `/store/v1`, query included) with `body`,
    /// as `caller`, its reply read up to `limit` bytes: `(status, bytes)` for
    /// a 2xx, else the failure.
    async fn send(&self, method: reqwest::Method, path: &str, body: Option<(Vec<u8>, &str)>, caller: &Caller, limit: u64) -> Result<Vec<u8>, String> {
        let url = format!("{}{path}", self.base);
        let mut request = self.http.request(method.clone(), &url);
        match caller {
            Caller::Anonymous => {}
            Caller::Session(token) => {
                if let Ok(token) = token.token().await {
                    request = request.bearer_auth(token);
                }
            }
            Caller::DeployKey(key) => {
                // The path signed is the URL's path alone.
                let bare = path.split('?').next().unwrap_or(path);
                let empty = Vec::new();
                let signed = body.as_ref().map_or(&empty, |(bytes, _)| bytes);
                let ts = chrono::Utc::now().timestamp_millis();
                request = request.header("Authorization", key.authorization(method.as_str(), bare, signed, ts));
            }
        }
        if let Some((bytes, content_type)) = body {
            request = request.header("Content-Type", content_type).body(bytes);
        }
        let unreachable = |_| failure(reason::STORE_UNREACHABLE, "The store can't be reached right now");
        let mut response = request.send().await.map_err(unreachable)?;
        let status = response.status().as_u16();
        let cap = if (200..300).contains(&status) { limit } else { MAX_JSON_BYTES };
        if response.content_length().is_some_and(|n| n > cap) {
            return Err(failure(install::reason::TOO_BIG, "The store's answer is too big"));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(unreachable)? {
            bytes.extend_from_slice(&chunk);
            if bytes.len() as u64 > cap {
                return Err(failure(install::reason::TOO_BIG, "The store's answer is too big"));
            }
        }
        if (200..300).contains(&status) {
            return Ok(bytes);
        }
        Err(refusal(status, &bytes))
    }

    async fn json(&self, method: reqwest::Method, path: &str, body: Option<Value>, caller: &Caller) -> Result<Value, String> {
        let body = body.map(|b| (serde_json::to_vec(&b).unwrap_or_default(), "application/json"));
        let bytes = self.send(method, path, body, caller, MAX_JSON_BYTES).await?;
        if bytes.is_empty() {
            return Ok(Value::Object(Default::default()));
        }
        serde_json::from_slice(&bytes).map_err(|_| failure(reason::STORE_ERROR, "The store's answer couldn't be read"))
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

    pub async fn versions(&self, id: &str, caller: &Caller) -> Result<(Value, Vec<VersionRecord>), String> {
        let value = self.json(reqwest::Method::GET, &format!("/store/v1/plugins/{}/versions", segment(id)), None, caller).await?;
        let versions = Self::typed(value.get("versions").cloned().unwrap_or(Value::Array(Vec::new())))?;
        Ok((value, versions))
    }

    /// Version `version`'s package file, up to the largest a package may be.
    pub async fn package(&self, id: &str, version: &str, caller: &Caller) -> Result<Vec<u8>, String> {
        let path = format!("/store/v1/plugins/{}/versions/{}/package", segment(id), segment(version));
        self.send(reqwest::Method::GET, &path, None, caller, MAX_PACKAGE_BYTES).await
    }

    /// One file of version `version`'s package, at most `limit` bytes.
    pub async fn file(&self, id: &str, version: &str, file: &str, caller: &Caller, limit: u64) -> Result<Vec<u8>, String> {
        let path = format!("/store/v1/plugins/{}/files/{}/{}", segment(id), segment(version), segments(file));
        self.send(reqwest::Method::GET, &path, None, caller, limit).await
    }

    /// The batched update check for every store-installed plugin.
    pub async fn updates(&self, ask: &UpdatesAsk, caller: &Caller) -> Result<Updates, String> {
        let body = serde_json::to_value(ask).unwrap_or_default();
        Self::typed(self.json(reqwest::Method::POST, "/store/v1/updates", Some(body), caller).await?)
    }

    // ── Publisher ───────────────────────────────────────────────────────

    /// Submit `package` (a `.zeron-plugin`) as a new version of `id`.
    pub async fn publish(&self, id: &str, package: Vec<u8>, caller: &Caller) -> Result<Value, String> {
        let path = format!("/store/v1/plugins/{}/versions", segment(id));
        let bytes = self.send(reqwest::Method::POST, &path, Some((package, "application/json")), caller, MAX_JSON_BYTES).await?;
        serde_json::from_slice(&bytes).map_err(|_| failure(reason::STORE_ERROR, "The store's answer couldn't be read"))
    }

    pub async fn my_plugins(&self, caller: &Caller) -> Result<Value, String> {
        self.json(reqwest::Method::GET, "/store/v1/me/plugins", None, caller).await
    }

    /// `withdraw`, `yank` or `unyank` version `version` of `id`.
    pub async fn version_action(&self, id: &str, version: &str, action: &str, caller: &Caller) -> Result<Value, String> {
        let path = format!("/store/v1/plugins/{}/versions/{}/{}", segment(id), segment(version), segment(action));
        self.json(reqwest::Method::POST, &path, None, caller).await
    }

    pub async fn keys(&self, id: &str, caller: &Caller) -> Result<Value, String> {
        self.json(reqwest::Method::GET, &format!("/store/v1/plugins/{}/keys", segment(id)), None, caller).await
    }

    pub async fn add_key(&self, id: &str, body: Value, caller: &Caller) -> Result<Value, String> {
        self.json(reqwest::Method::POST, &format!("/store/v1/plugins/{}/keys", segment(id)), Some(body), caller).await
    }

    pub async fn revoke_key(&self, id: &str, key: &str, caller: &Caller) -> Result<Value, String> {
        let path = format!("/store/v1/plugins/{}/keys/{}", segment(id), segment(key));
        self.json(reqwest::Method::DELETE, &path, None, caller).await
    }

    /// Counted as installed (once per account, by the edge).
    pub async fn installed(&self, id: &str, caller: &Caller) -> Result<(), String> {
        self.json(reqwest::Method::POST, &format!("/store/v1/plugins/{}/installed", segment(id)), None, caller).await.map(drop)
    }
}

/// A non-2xx answer as a failure: the edge's own word when it gave one.
fn refusal(status: u16, body: &[u8]) -> String {
    let value: Value = serde_json::from_slice(body).unwrap_or_default();
    let word = value["error"].as_str().filter(|w| !w.is_empty() && w.len() <= 64 && w.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'));
    let message: String = value["message"].as_str().unwrap_or("").chars().filter(|c| !c.is_control()).take(500).collect();
    match (word, status) {
        (Some(word), _) => failure(word, if message.is_empty() { format!("The store refused it ({status})") } else { message }),
        (None, 404) => failure(reason::NOT_IN_STORE, "The store has no such plugin"),
        (None, 429) => failure(install::reason::RATE_LIMITED, "The store is turning away requests from here for now; try again later"),
        (None, 500..) => failure(reason::STORE_UNREACHABLE, format!("The store couldn't answer just now ({status})")),
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
    }

    #[test]
    fn refusals_carry_the_edge_s_word() {
        let said = |status, body: &str| refusal(status, body.as_bytes());
        assert_eq!(said(409, r#"{"error":"id_taken","message":"Someone has that id."}"#), "id_taken: Someone has that id.");
        assert_eq!(install::reason_of(&said(404, "")), Some(reason::NOT_IN_STORE));
        assert_eq!(install::reason_of(&said(503, "<html>")), Some(reason::STORE_UNREACHABLE));
        assert_eq!(install::reason_of(&said(429, "")), Some(install::reason::RATE_LIMITED));
        // A word the edge didn't really give (not a plain word) isn't taken as one.
        assert_eq!(install::reason_of(&said(400, r#"{"error":"Bad: thing"}"#)), Some(reason::STORE_ERROR));
        assert!(!said(400, "{\"error\":\"x\",\"message\":\"a\\u0007b\"}").contains('\u{7}'));
    }
}
