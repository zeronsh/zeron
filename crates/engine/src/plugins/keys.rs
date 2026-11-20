//! Who a signing key belongs to: the `/plugin-keys` edge routes, their cache
//! (`<data>/plugin-keys.json`), and this device's key registration
//! (`<data>/plugin-signing-key.json`).

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::status::Author;

pub const CACHE_FILE: &str = "plugin-keys.json";
pub const REGISTRATION_FILE: &str = "plugin-signing-key.json";
/// How long a lookup is trusted: a key revoked elsewhere stops counting
/// within this (refreshed in the background, `mod.rs`, so nothing waits).
pub const CACHE_TTL_SECS: i64 = 60 * 60;
/// Keys to revoke once the account they're registered to is signed in
/// again (a sign-out or rotation that couldn't reach the edge).
pub const REVOKE_FILE: &str = "plugin-keys-revoke.json";
/// When this device last looked for keys added on the account's other
/// devices (`{account: epoch ms}`).
pub const WATERMARK_FILE: &str = "plugin-keys-seen.json";

pub const KEY_NOT_KNOWN: &str = "Its author's key isn't known";
pub const COULD_NOT_CHECK: &str = "Couldn't check who made it";
pub const CHECKING: &str = "Checking who made it";
pub const NO_USERNAME: &str = "Its author hasn't claimed a username";

const HTTP_TIMEOUT: Duration = Duration::from_secs(10);

/// What the edge says about a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup {
    Found {
        username: Option<String>,
        mine: bool,
        revoked: bool,
        /// The edge's word: not revoked, and its owner holds a username.
        /// `None` from an edge before it said so.
        verified: Option<bool>,
    },
    /// 404: nobody registered it.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegisterError {
    /// 409 `key_revoked`: our own key, revoked — make a fresh one.
    Revoked,
    /// 409 `key_taken`: someone else's.
    Taken,
    /// 403 `step_up_required`: a new key needs a fresh sign-in. Not
    /// retried until the next one.
    StepUp,
    Other(String),
}

/// A key registered to the signed-in account (`GET /plugin-keys`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyRow {
    pub key: String,
    #[serde(default)]
    pub fingerprint: String,
    #[serde(default)]
    pub device_id: String,
    /// Epoch ms.
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub revoked_at: Option<i64>,
}

/// The `/plugin-keys` routes, behind a trait so tests can stand in for the edge.
#[async_trait]
pub trait KeyDirectory: Send + Sync + 'static {
    async fn lookup(&self, key: &str) -> Result<Lookup, String>;
    /// `step_up`: the ticket of the sign-in just made, which a new key needs.
    async fn register(&self, key: &str, device_id: &str, proof: &str, step_up: Option<&str>) -> Result<(), RegisterError>;
    /// Revoke one of the signed-in account's keys (done when it's revoked
    /// already, or isn't the account's).
    async fn revoke(&self, key: &str) -> Result<(), String>;
    /// The account's keys added or revoked after `since` (epoch ms), and
    /// the edge's now.
    async fn keys_since(&self, since: i64) -> Result<(Vec<KeyRow>, i64), String>;
}

/// The real edge, with the signed-in user's bearer.
pub struct EdgeDirectory {
    url: String,
    token: Arc<dyn zeron_rpc::TokenSource>,
    http: reqwest::Client,
}

impl EdgeDirectory {
    pub fn new(url: &str, token: Arc<dyn zeron_rpc::TokenSource>) -> Self {
        Self {
            url: url.trim_end_matches('/').to_owned(),
            token,
            http: reqwest::Client::builder()
                .timeout(HTTP_TIMEOUT)
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
    }
}

#[derive(Deserialize)]
struct LookupReply {
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    mine: bool,
    #[serde(default)]
    revoked: bool,
    #[serde(default)]
    verified: Option<bool>,
}

#[async_trait]
impl KeyDirectory for EdgeDirectory {
    async fn lookup(&self, key: &str) -> Result<Lookup, String> {
        let token = self.token.token().await.map_err(|e| e.to_string())?;
        let response = self
            .http
            .get(format!("{}/plugin-keys/{key}", self.url))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        match response.status().as_u16() {
            404 => Ok(Lookup::Unknown),
            200 => {
                let reply: LookupReply = response.json().await.map_err(|e| e.to_string())?;
                Ok(Lookup::Found {
                    username: reply.username,
                    mine: reply.mine,
                    revoked: reply.revoked,
                    verified: reply.verified,
                })
            }
            status => Err(format!("plugin-keys lookup: HTTP {status}")),
        }
    }

    async fn register(&self, key: &str, device_id: &str, proof: &str, step_up: Option<&str>) -> Result<(), RegisterError> {
        let token = self
            .token
            .token()
            .await
            .map_err(|e| RegisterError::Other(e.to_string()))?;
        let response = self
            .http
            .post(format!("{}/plugin-keys", self.url))
            .bearer_auth(token)
            .json(&match step_up {
                Some(ticket) => serde_json::json!({ "key": key, "deviceId": device_id, "proof": proof, "stepUp": ticket }),
                None => serde_json::json!({ "key": key, "deviceId": device_id, "proof": proof }),
            })
            .send()
            .await
            .map_err(|e| RegisterError::Other(e.to_string()))?;
        let status = response.status().as_u16();
        if (200..300).contains(&status) {
            return Ok(());
        }
        let body: serde_json::Value = response.json().await.unwrap_or_default();
        let code = body
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        Err(match (status, code) {
            (409, "key_revoked") => RegisterError::Revoked,
            (409, _) => RegisterError::Taken,
            (403, "step_up_required") => RegisterError::StepUp,
            _ => RegisterError::Other(format!("plugin-keys register: HTTP {status} {code}")),
        })
    }

    async fn revoke(&self, key: &str) -> Result<(), String> {
        let token = self.token.token().await.map_err(|e| e.to_string())?;
        let response = self
            .http
            .delete(format!("{}/plugin-keys/{key}", self.url))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        match response.status().as_u16() {
            // Revoked now or before; or not this account's to revoke.
            200..=299 | 404 => Ok(()),
            status => Err(format!("plugin-keys revoke: HTTP {status}")),
        }
    }

    async fn keys_since(&self, since: i64) -> Result<(Vec<KeyRow>, i64), String> {
        #[derive(Deserialize)]
        struct Reply {
            #[serde(default)]
            keys: Vec<KeyRow>,
            #[serde(default)]
            now: i64,
        }
        let token = self.token.token().await.map_err(|e| e.to_string())?;
        let response = self
            .http
            .get(format!("{}/plugin-keys?since={since}", self.url))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !response.status().is_success() {
            return Err(format!("plugin-keys list: HTTP {}", response.status().as_u16()));
        }
        let reply: Reply = response.json().await.map_err(|e| e.to_string())?;
        Ok((reply.keys, reply.now))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachedKey {
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub mine: bool,
    #[serde(default)]
    pub revoked: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified: Option<bool>,
    /// Unix seconds.
    pub checked_at: i64,
    /// The signed-in account the lookup was made as.
    pub account: String,
}

impl CachedKey {
    pub fn from_lookup(lookup: &Lookup, account: &str, now: i64) -> Self {
        let (username, mine, revoked, verified) = match lookup {
            Lookup::Found {
                username,
                mine,
                revoked,
                verified,
            } => (username.clone(), *mine, *revoked, *verified),
            // Unknown keys answer like revoked ones.
            Lookup::Unknown => (None, false, true, Some(false)),
        };
        Self {
            username,
            mine,
            revoked,
            verified,
            checked_at: now,
            account: account.to_owned(),
        }
    }

    pub fn fresh(&self, now: i64) -> bool {
        now - self.checked_at < CACHE_TTL_SECS
    }

    /// Who it is. Never `You`: that's this device's own key alone
    /// ([`resolve_cached`]); another of your devices' keys is verified, and
    /// asked about like anyone's. A key with no username isn't verified.
    pub fn author(&self) -> Author {
        let username = self.username.as_deref().map(str::trim).filter(|u| !u.is_empty());
        if self.revoked {
            Author::unknown(KEY_NOT_KNOWN)
        } else if self.verified == Some(false) || username.is_none() {
            Author::unknown(NO_USERNAME)
        } else {
            Author::Verified {
                username: username.map(str::to_owned),
                mine: self.mine,
            }
        }
    }
}

/// Whether a plugin installed from `old` may be updated by one signed with
/// `new`: the author's key on a new machine. `new` must read as verified
/// (live, its owner holding a username) and `old` (revoked or not: a lost
/// machine's key is revoked) belong to the same owner, by the username the
/// edge answers for each. Usernames are never handed to another account
/// (`edge/src/user-directory.ts`), so the same name is the same account. A
/// key whose owner was renamed between the two lookups doesn't match: the
/// update is refused, as it was before.
pub fn same_account(old: &CachedKey, new: &CachedKey) -> bool {
    let Author::Verified { username: Some(name), .. } = new.author() else {
        return false;
    };
    old.username
        .as_deref()
        .map(str::trim)
        .is_some_and(|was| !was.is_empty() && was.eq_ignore_ascii_case(&name))
}

/// `<data>/plugin-keys.json`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct KeyCache(pub BTreeMap<String, CachedKey>);

impl KeyCache {
    pub fn load(data_dir: &Path) -> Self {
        std::fs::read(data_dir.join(CACHE_FILE))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, data_dir: &Path) {
        let bytes = serde_json::to_vec_pretty(self).unwrap_or_default();
        if let Err(err) = crate::runner::write_private_atomic(&data_dir.join(CACHE_FILE), &bytes) {
            tracing::warn!(error = %err, "plugins: couldn't save the key cache");
        }
    }

    /// The entry for `key` made as `account` (fresh or not).
    pub fn get(&self, key: &str, account: &str) -> Option<&CachedKey> {
        self.0.get(key).filter(|entry| entry.account == account)
    }
}

/// Who made a plugin signed with `key`, without waiting on the network.
/// `Err(author)` = a lookup is owed (the author to show meanwhile: what a
/// stale entry said, which keeps a decision already bound to that key in
/// force; never one to grant by, see [`for_granting`]).
pub fn resolve_cached(
    cache: &KeyCache,
    key: &str,
    own_key: Option<&str>,
    account: Option<&str>,
    now: i64,
    lookup_failed: bool,
) -> Result<Author, Author> {
    if own_key == Some(key) {
        return Ok(Author::You);
    }
    let Some(account) = account else {
        return Ok(Author::unknown(COULD_NOT_CHECK));
    };
    match cache.get(key, account) {
        Some(entry) if entry.fresh(now) => Ok(entry.author()),
        // Stale: keep showing what it was while it's checked again.
        Some(entry) => Err(entry.author()),
        None if lookup_failed => Err(Author::unknown(COULD_NOT_CHECK)),
        None => Err(Author::unknown(CHECKING)),
    }
}

/// `resolve_cached`'s answer as a grant may be decided by: an author still
/// owed a lookup (stale, offline, never checked) is Unknown.
pub fn for_granting(resolved: Result<Author, Author>, gave_up: bool) -> Author {
    match resolved {
        Ok(author) => author,
        Err(_) if gave_up => Author::unknown(COULD_NOT_CHECK),
        Err(_) => Author::unknown(CHECKING),
    }
}

/// `<data>/plugin-signing-key.json`: `{ registered_for, key }`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Registration {
    #[serde(default)]
    pub registered_for: Option<String>,
    /// The key that registration was for (a rotated key isn't registered).
    #[serde(default)]
    pub key: Option<String>,
}

impl Registration {
    pub fn load(data_dir: &Path) -> Self {
        std::fs::read(data_dir.join(REGISTRATION_FILE))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, data_dir: &Path) {
        let bytes = serde_json::to_vec_pretty(self).unwrap_or_default();
        if let Err(err) =
            crate::runner::write_private_atomic(&data_dir.join(REGISTRATION_FILE), &bytes)
        {
            tracing::warn!(error = %err, "plugins: couldn't save the key registration");
        }
    }

    pub fn covers(&self, key: &str, account: &str) -> bool {
        self.registered_for.as_deref() == Some(account)
            && self.key.as_deref().is_none_or(|k| k == key)
    }
}

/// A key to revoke when `account` is signed in again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwedRevoke {
    pub key: String,
    pub account: String,
}

pub fn owed_revokes(data_dir: &Path) -> Vec<OwedRevoke> {
    std::fs::read(data_dir.join(REVOKE_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save_owed(data_dir: &Path, owed: &[OwedRevoke]) {
    let path = data_dir.join(REVOKE_FILE);
    if owed.is_empty() {
        let _ = std::fs::remove_file(path);
        return;
    }
    let bytes = serde_json::to_vec_pretty(owed).unwrap_or_default();
    if let Err(err) = crate::runner::write_private_atomic(&path, &bytes) {
        tracing::warn!(error = %err, "plugins: couldn't keep a key to revoke");
    }
}

pub fn owe_revoke(data_dir: &Path, key: &str, account: &str) {
    let mut owed = owed_revokes(data_dir);
    if !owed.iter().any(|o| o.key == key && o.account == account) {
        owed.push(OwedRevoke { key: key.to_owned(), account: account.to_owned() });
        save_owed(data_dir, &owed);
    }
}

/// Revoke the keys owed for `account` (signed in as it now).
pub async fn settle_revokes(data_dir: &Path, directory: &dyn KeyDirectory, account: &str) {
    let owed = owed_revokes(data_dir);
    let mut left = Vec::new();
    for o in owed {
        if o.account == account && directory.revoke(&o.key).await.is_ok() {
            tracing::info!("plugins: an old signing key was revoked");
            continue;
        }
        left.push(o);
    }
    save_owed(data_dir, &left);
}

/// This device's key retired: revoked on the edge for `account` (or owed,
/// when it can't be reached), its private half deleted, its registration
/// forgotten. The next use makes a fresh key, registered at the next
/// sign-in.
pub async fn retire_key(data_dir: &Path, directory: Option<&dyn KeyDirectory>, account: Option<&str>) {
    let Some(key) = super::signing::load_key(data_dir).map(|k| k.public()) else {
        return;
    };
    let registration = Registration::load(data_dir);
    let account = account.map(str::to_owned).or(registration.registered_for.clone());
    if let Some(account) = &account
        && registration.registered_for.as_deref() == Some(account.as_str())
    {
        let revoked = match directory {
            Some(directory) => tokio::time::timeout(Duration::from_secs(5), directory.revoke(&key))
                .await
                .map_err(|_| "timed out".to_string())
                .and_then(|r| r),
            None => Err("no edge".into()),
        };
        if let Err(error) = revoked {
            tracing::warn!(%error, "plugins: signing key not revoked yet; kept to revoke at the next sign-in");
            owe_revoke(data_dir, &key, account);
        }
    }
    super::signing::delete_key(data_dir);
    let _ = std::fs::remove_file(data_dir.join(REGISTRATION_FILE));
}

/// Register this device's key for `account` unless it already is (`step_up`:
/// the ticket of the sign-in just made, which a new key needs). When the
/// edge says it's revoked, or someone else's, a fresh key replaces it: the
/// old one revoked first where it's still live on another account we were
/// signed in as (owed until that account signs in). Returns whether the
/// key changed.
pub async fn ensure_registered(
    data_dir: &Path,
    directory: &dyn KeyDirectory,
    device_id: &str,
    account: &str,
    step_up: Option<&str>,
) -> Result<bool, String> {
    let mut key = super::signing::load_or_create_key(data_dir)?;
    let registration = Registration::load(data_dir);
    if registration.covers(&key.public(), account) {
        return Ok(false);
    }
    let mut rotated = false;
    for _ in 0..2 {
        match directory
            .register(&key.public(), device_id, &key.proof(), step_up)
            .await
        {
            Ok(()) => {
                Registration {
                    registered_for: Some(account.to_owned()),
                    key: Some(key.public()),
                }
                .save(data_dir);
                return Ok(rotated);
            }
            Err(error @ (RegisterError::Revoked | RegisterError::Taken)) if !rotated => {
                tracing::info!(
                    "plugins: this device's signing key can't be used for this account; making a new one"
                );
                // Live on the account it was registered for: revoked there
                // when that account is signed in again.
                if error == RegisterError::Taken
                    && let Some(other) = registration.registered_for.as_deref().filter(|a| *a != account)
                {
                    owe_revoke(data_dir, &key.public(), other);
                }
                key = super::signing::rotate_key(data_dir)?;
                rotated = true;
            }
            Err(err) => return Err(format!("{err:?}")),
        }
    }
    Err("the new signing key couldn't be registered either".into())
}

/// `<data>/plugin-keys-seen.json`: the watermark per account.
pub fn watermark(data_dir: &Path, account: &str) -> Option<i64> {
    let map: BTreeMap<String, i64> = std::fs::read(data_dir.join(WATERMARK_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())?;
    map.get(account).copied()
}

pub fn set_watermark(data_dir: &Path, account: &str, at: i64) {
    let mut map: BTreeMap<String, i64> = std::fs::read(data_dir.join(WATERMARK_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    map.insert(account.to_owned(), at);
    let bytes = serde_json::to_vec_pretty(&map).unwrap_or_default();
    let _ = crate::runner::write_private_atomic(&data_dir.join(WATERMARK_FILE), &bytes);
}

/// How far back the first look reaches (no watermark yet: a new engine, or
/// a new account here): keys added in the last day are still news.
pub const FIRST_LOOK_MS: i64 = 24 * 60 * 60 * 1000;

/// The keys added on the account's other devices since this device last
/// looked (the first time, in the last day).
pub async fn new_keys_elsewhere(
    data_dir: &Path,
    directory: &dyn KeyDirectory,
    device_id: &str,
    account: &str,
) -> Result<Vec<KeyRow>, String> {
    let since = watermark(data_dir, account);
    let (rows, now) = directory.keys_since(since.unwrap_or(0)).await?;
    let since = since.unwrap_or_else(|| now.max(chrono::Utc::now().timestamp_millis()) - FIRST_LOOK_MS);
    let fresh = rows
        .into_iter()
        .filter(|r| r.device_id != device_id && r.created_at > since && r.revoked_at.is_none())
        .collect();
    if now > 0 {
        set_watermark(data_dir, account, now);
    }
    Ok(fresh)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A stand-in edge: keys → lookups, and what registration answers.
    #[derive(Default)]
    pub struct FakeDirectory {
        pub keys: Mutex<BTreeMap<String, Lookup>>,
        pub unreachable: std::sync::atomic::AtomicBool,
        pub register_answers: Mutex<Vec<Result<(), RegisterError>>>,
        pub registered: Mutex<Vec<String>>,
        /// The step-up ticket each registration carried.
        pub step_ups: Mutex<Vec<Option<String>>>,
        pub revoked: Mutex<Vec<String>>,
        pub rows: Mutex<Vec<KeyRow>>,
        pub now: std::sync::atomic::AtomicI64,
    }

    impl FakeDirectory {
        pub fn found(username: Option<&str>, mine: bool) -> Lookup {
            Lookup::Found { username: username.map(str::to_owned), mine, revoked: false, verified: None }
        }
    }

    #[async_trait]
    impl KeyDirectory for FakeDirectory {
        async fn lookup(&self, key: &str) -> Result<Lookup, String> {
            if self.unreachable.load(std::sync::atomic::Ordering::SeqCst) {
                return Err("offline".into());
            }
            Ok(self
                .keys
                .lock()
                .unwrap()
                .get(key)
                .cloned()
                .unwrap_or(Lookup::Unknown))
        }
        async fn register(
            &self,
            key: &str,
            _device: &str,
            _proof: &str,
            step_up: Option<&str>,
        ) -> Result<(), RegisterError> {
            self.registered.lock().unwrap().push(key.to_owned());
            self.step_ups.lock().unwrap().push(step_up.map(str::to_owned));
            let mut answers = self.register_answers.lock().unwrap();
            if answers.is_empty() {
                Ok(())
            } else {
                answers.remove(0)
            }
        }
        async fn revoke(&self, key: &str) -> Result<(), String> {
            if self.unreachable.load(std::sync::atomic::Ordering::SeqCst) {
                return Err("offline".into());
            }
            self.revoked.lock().unwrap().push(key.to_owned());
            Ok(())
        }
        async fn keys_since(&self, since: i64) -> Result<(Vec<KeyRow>, i64), String> {
            let rows = self.rows.lock().unwrap().iter().filter(|r| r.created_at > since).cloned().collect();
            Ok((rows, self.now.load(std::sync::atomic::Ordering::SeqCst)))
        }
    }

    /// B4: an author's new key takes over a plugin only when it's live and
    /// verified, and the old key (revoked or not) is the same account's.
    #[test]
    fn plugins_key_same_account_is_a_live_key_of_the_same_username() {
        let key = |username: Option<&str>, revoked: bool, verified: Option<bool>| {
            CachedKey::from_lookup(&Lookup::Found { username: username.map(str::to_owned), mine: false, revoked, verified }, "u", 0)
        };
        let ada = key(Some("ada"), false, Some(true));
        assert!(same_account(&ada, &key(Some("ada"), false, Some(true))));
        assert!(same_account(&ada, &key(Some("Ada"), false, None)), "an older edge's answer, any case");
        assert!(same_account(&key(Some("ada"), true, Some(false)), &ada), "the old key revoked: a lost machine");
        assert!(!same_account(&ada, &key(Some("ada"), true, Some(false))), "the new key revoked");
        assert!(!same_account(&ada, &key(Some("eve"), false, Some(true))));
        assert!(!same_account(&ada, &key(Some("ada"), false, Some(false))));
        assert!(!same_account(&ada, &key(None, false, None)));
        assert!(!same_account(&key(None, true, None), &ada));
        let unknown = CachedKey::from_lookup(&Lookup::Unknown, "u", 0);
        assert!(!same_account(&unknown, &ada) && !same_account(&ada, &unknown));
    }

    #[test]
    fn plugins_key_cache_resolution() {
        let mut cache = KeyCache::default();
        let now = 1_000_000;
        // Own key, even signed out.
        assert_eq!(
            resolve_cached(&cache, "OWN", Some("OWN"), None, now, false),
            Ok(Author::You)
        );
        // Signed out, someone else's.
        assert_eq!(
            resolve_cached(&cache, "K", Some("OWN"), None, now, false),
            Ok(Author::unknown(COULD_NOT_CHECK))
        );
        // Not cached: a lookup is owed.
        assert_eq!(
            resolve_cached(&cache, "K", None, Some("u1"), now, false),
            Err(Author::unknown(CHECKING))
        );
        assert_eq!(
            resolve_cached(&cache, "K", None, Some("u1"), now, true),
            Err(Author::unknown(COULD_NOT_CHECK))
        );
        let found = FakeDirectory::found(Some("ada"), false);
        cache
            .0
            .insert("K".into(), CachedKey::from_lookup(&found, "u1", now));
        assert_eq!(
            resolve_cached(&cache, "K", None, Some("u1"), now + 10, false),
            Ok(Author::verified("ada"))
        );
        // Only valid for the account it was made as.
        assert!(resolve_cached(&cache, "K", None, Some("u2"), now, false).is_err());
        // Stale: still shown, but re-checked; never granted by.
        let stale = resolve_cached(&cache, "K", None, Some("u1"), now + CACHE_TTL_SECS + 1, false);
        assert_eq!(stale, Err(Author::verified("ada")));
        assert_eq!(for_granting(stale.clone(), false), Author::unknown(CHECKING));
        assert_eq!(for_granting(stale, true), Author::unknown(COULD_NOT_CHECK));
        let revoked = Lookup::Found { username: Some("x".into()), mine: false, revoked: true, verified: None };
        assert_eq!(
            CachedKey::from_lookup(&revoked, "u1", now).author(),
            Author::unknown(KEY_NOT_KNOWN)
        );
        assert_eq!(
            CachedKey::from_lookup(&Lookup::Unknown, "u1", now).author(),
            Author::unknown(KEY_NOT_KNOWN)
        );
    }

    /// R4: "you" was any key the edge called the account's, so a key
    /// registered from any account device (or with a leaked bearer) got
    /// every permission granted on install. Only this device's own key is
    /// "you" now; the account's others are verified, and asked about.
    #[test]
    fn plugins_key_mine_on_another_device_is_not_you() {
        let mine = FakeDirectory::found(Some("me"), true);
        assert_eq!(
            CachedKey::from_lookup(&mine, "u1", 0).author(),
            Author::Verified { username: Some("me".into()), mine: true }
        );
    }

    /// R17: a key with no username rendered "Verified · Made by @".
    #[test]
    fn plugins_key_without_a_username_is_unknown() {
        for lookup in [
            FakeDirectory::found(None, false),
            FakeDirectory::found(Some(" "), false),
            FakeDirectory::found(None, true),
            Lookup::Found { username: Some("ada".into()), mine: false, revoked: false, verified: Some(false) },
        ] {
            assert_eq!(CachedKey::from_lookup(&lookup, "u1", 0).author(), Author::unknown(NO_USERNAME), "{lookup:?}");
        }
        let edge_says = Lookup::Found { username: Some("ada".into()), mine: false, revoked: false, verified: Some(true) };
        assert_eq!(CachedKey::from_lookup(&edge_says, "u1", 0).author(), Author::verified("ada"));
    }

    #[tokio::test]
    async fn plugins_key_registration_once_per_account_and_rotates_on_revoked() {
        let dir = tempfile::tempdir().unwrap();
        let fake = FakeDirectory::default();
        assert!(
            !ensure_registered(dir.path(), &fake, "dev1", "u1", Some("ticket"))
                .await
                .unwrap()
        );
        let first = super::super::signing::load_key(dir.path())
            .unwrap()
            .public();
        assert_eq!(fake.registered.lock().unwrap().clone(), vec![first.clone()]);
        assert_eq!(fake.step_ups.lock().unwrap().clone(), vec![Some("ticket".to_string())]);
        // Again: nothing to do.
        ensure_registered(dir.path(), &fake, "dev1", "u1", None)
            .await
            .unwrap();
        assert_eq!(fake.registered.lock().unwrap().len(), 1);
        // Another account: registers again; this one says it's revoked.
        fake.register_answers
            .lock()
            .unwrap()
            .push(Err(RegisterError::Revoked));
        assert!(
            ensure_registered(dir.path(), &fake, "dev1", "u2", Some("t2"))
                .await
                .unwrap()
        );
        let second = super::super::signing::load_key(dir.path())
            .unwrap()
            .public();
        assert_ne!(first, second);
        assert_eq!(fake.registered.lock().unwrap().last().unwrap(), &second);
        assert!(Registration::load(dir.path()).covers(&second, "u2"));
        // R13: the retired private key isn't kept anywhere.
        let aside = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("plugin-signing-key.revoked-")
            });
        assert!(!aside);
        // A failure leaves it unregistered (retried next start).
        fake.register_answers
            .lock()
            .unwrap()
            .push(Err(RegisterError::Other("down".into())));
        assert!(
            ensure_registered(dir.path(), &fake, "dev1", "u3", None)
                .await
                .is_err()
        );
        assert!(!Registration::load(dir.path()).covers(&second, "u3"));
        // A new key without a fresh sign-in: refused, and nothing rotated.
        fake.register_answers.lock().unwrap().push(Err(RegisterError::StepUp));
        assert!(ensure_registered(dir.path(), &fake, "dev1", "u3", None).await.is_err());
        assert_eq!(super::super::signing::load_key(dir.path()).unwrap().public(), second);
    }

    /// R13: keys were never revoked: a rotation left the old key registered
    /// to the account it was taken from, and sign-out left it live.
    #[tokio::test]
    async fn plugins_key_retired_on_rotation_and_sign_out() {
        let dir = tempfile::tempdir().unwrap();
        let fake = FakeDirectory::default();
        ensure_registered(dir.path(), &fake, "dev1", "u1", Some("t")).await.unwrap();
        let first = super::super::signing::load_key(dir.path()).unwrap().public();
        // Signed in as another account, the key is u1's: rotated, and the
        // old one owed a revoke on u1.
        fake.register_answers.lock().unwrap().push(Err(RegisterError::Taken));
        ensure_registered(dir.path(), &fake, "dev1", "u2", Some("t")).await.unwrap();
        assert_eq!(owed_revokes(dir.path()), vec![OwedRevoke { key: first.clone(), account: "u1".into() }]);
        settle_revokes(dir.path(), &fake, "u2").await;
        assert!(fake.revoked.lock().unwrap().is_empty(), "only u1 can revoke it");
        settle_revokes(dir.path(), &fake, "u1").await;
        assert_eq!(fake.revoked.lock().unwrap().clone(), vec![first]);
        assert!(owed_revokes(dir.path()).is_empty());
        // Sign-out: revoked on the edge, private key deleted.
        let second = super::super::signing::load_key(dir.path()).unwrap().public();
        retire_key(dir.path(), Some(&fake), Some("u2")).await;
        assert_eq!(fake.revoked.lock().unwrap().last(), Some(&second));
        assert!(super::super::signing::load_key(dir.path()).is_none());
        assert_eq!(Registration::load(dir.path()), Registration::default());
        // Offline at sign-out: deleted all the same, revoked at the next sign-in.
        ensure_registered(dir.path(), &fake, "dev1", "u2", Some("t")).await.unwrap();
        let third = super::super::signing::load_key(dir.path()).unwrap().public();
        fake.unreachable.store(true, std::sync::atomic::Ordering::SeqCst);
        retire_key(dir.path(), Some(&fake), Some("u2")).await;
        assert!(super::super::signing::load_key(dir.path()).is_none());
        assert_eq!(owed_revokes(dir.path()), vec![OwedRevoke { key: third, account: "u2".into() }]);
    }

    #[tokio::test]
    async fn plugins_key_new_keys_on_other_devices_are_noticed_once() {
        let dir = tempfile::tempdir().unwrap();
        let fake = FakeDirectory::default();
        let row = |key: &str, device: &str, at: i64| KeyRow {
            key: key.into(), fingerprint: "abcd".into(), device_id: device.into(), created_at: at, revoked_at: None,
        };
        fake.rows.lock().unwrap().push(row("OLD", "dev2", 10));
        fake.now.store(100, std::sync::atomic::Ordering::SeqCst);
        // The first look only sets where to look from.
        assert!(new_keys_elsewhere(dir.path(), &fake, "dev1", "u1").await.unwrap().is_empty());
        fake.rows.lock().unwrap().extend([row("NEW", "dev2", 150), row("MINE", "dev1", 160)]);
        fake.now.store(200, std::sync::atomic::Ordering::SeqCst);
        let seen = new_keys_elsewhere(dir.path(), &fake, "dev1", "u1").await.unwrap();
        assert_eq!(seen.iter().map(|r| r.key.as_str()).collect::<Vec<_>>(), vec!["NEW"]);
        assert!(new_keys_elsewhere(dir.path(), &fake, "dev1", "u1").await.unwrap().is_empty());
    }
}
