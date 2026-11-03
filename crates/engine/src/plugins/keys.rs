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
pub const CACHE_TTL_SECS: i64 = 24 * 60 * 60;

pub const KEY_NOT_KNOWN: &str = "Its author's key isn't known";
pub const COULD_NOT_CHECK: &str = "Couldn't check who made it";
pub const CHECKING: &str = "Checking who made it";

const HTTP_TIMEOUT: Duration = Duration::from_secs(10);

/// What the edge says about a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup {
    Found {
        username: Option<String>,
        mine: bool,
        revoked: bool,
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
    Other(String),
}

/// The `/plugin-keys` routes, behind a trait so tests can stand in for the edge.
#[async_trait]
pub trait KeyDirectory: Send + Sync + 'static {
    async fn lookup(&self, key: &str) -> Result<Lookup, String>;
    async fn register(&self, key: &str, device_id: &str, proof: &str) -> Result<(), RegisterError>;
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
                })
            }
            status => Err(format!("plugin-keys lookup: HTTP {status}")),
        }
    }

    async fn register(&self, key: &str, device_id: &str, proof: &str) -> Result<(), RegisterError> {
        let token = self
            .token
            .token()
            .await
            .map_err(|e| RegisterError::Other(e.to_string()))?;
        let response = self
            .http
            .post(format!("{}/plugin-keys", self.url))
            .bearer_auth(token)
            .json(&serde_json::json!({ "key": key, "deviceId": device_id, "proof": proof }))
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
            _ => RegisterError::Other(format!("plugin-keys register: HTTP {status} {code}")),
        })
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
    /// Unix seconds.
    pub checked_at: i64,
    /// The signed-in account the lookup was made as.
    pub account: String,
}

impl CachedKey {
    pub fn from_lookup(lookup: &Lookup, account: &str, now: i64) -> Self {
        let (username, mine, revoked) = match lookup {
            Lookup::Found {
                username,
                mine,
                revoked,
            } => (username.clone(), *mine, *revoked),
            // Unknown keys answer like revoked ones.
            Lookup::Unknown => (None, false, true),
        };
        Self {
            username,
            mine,
            revoked,
            checked_at: now,
            account: account.to_owned(),
        }
    }

    pub fn fresh(&self, now: i64) -> bool {
        now - self.checked_at < CACHE_TTL_SECS
    }

    pub fn author(&self) -> Author {
        if self.revoked {
            Author::unknown(KEY_NOT_KNOWN)
        } else if self.mine {
            Author::You
        } else {
            Author::Verified {
                username: self.username.clone(),
            }
        }
    }
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
/// `Err(author)` = a lookup is owed (the author to show meanwhile).
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

/// Register this device's key for `account` unless it already is. Rotates
/// the key when the edge says it was revoked (or belongs to another
/// account) and registers the new one. Returns whether the key changed.
pub async fn ensure_registered(
    data_dir: &Path,
    directory: &dyn KeyDirectory,
    device_id: &str,
    account: &str,
) -> Result<bool, String> {
    let mut key = super::signing::load_or_create_key(data_dir)?;
    if Registration::load(data_dir).covers(&key.public(), account) {
        return Ok(false);
    }
    let mut rotated = false;
    for _ in 0..2 {
        match directory
            .register(&key.public(), device_id, &key.proof())
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
            Err(RegisterError::Revoked | RegisterError::Taken) if !rotated => {
                tracing::info!(
                    "plugins: this device's signing key can't be used for this account; making a new one"
                );
                key = super::signing::rotate_key(data_dir)?;
                rotated = true;
            }
            Err(err) => return Err(format!("{err:?}")),
        }
    }
    Err("the new signing key couldn't be registered either".into())
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
        ) -> Result<(), RegisterError> {
            self.registered.lock().unwrap().push(key.to_owned());
            let mut answers = self.register_answers.lock().unwrap();
            if answers.is_empty() {
                Ok(())
            } else {
                answers.remove(0)
            }
        }
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
        let found = Lookup::Found {
            username: Some("ada".into()),
            mine: false,
            revoked: false,
        };
        cache
            .0
            .insert("K".into(), CachedKey::from_lookup(&found, "u1", now));
        assert_eq!(
            resolve_cached(&cache, "K", None, Some("u1"), now + 10, false),
            Ok(Author::Verified {
                username: Some("ada".into())
            })
        );
        // Only valid for the account it was made as.
        assert!(resolve_cached(&cache, "K", None, Some("u2"), now, false).is_err());
        // Stale: still shown, but re-checked.
        assert_eq!(
            resolve_cached(
                &cache,
                "K",
                None,
                Some("u1"),
                now + CACHE_TTL_SECS + 1,
                false
            ),
            Err(Author::Verified {
                username: Some("ada".into())
            })
        );
        let mine = Lookup::Found {
            username: Some("me".into()),
            mine: true,
            revoked: false,
        };
        assert_eq!(
            CachedKey::from_lookup(&mine, "u1", now).author(),
            Author::You
        );
        let revoked = Lookup::Found {
            username: Some("x".into()),
            mine: false,
            revoked: true,
        };
        assert_eq!(
            CachedKey::from_lookup(&revoked, "u1", now).author(),
            Author::unknown(KEY_NOT_KNOWN)
        );
        assert_eq!(
            CachedKey::from_lookup(&Lookup::Unknown, "u1", now).author(),
            Author::unknown(KEY_NOT_KNOWN)
        );
    }

    #[tokio::test]
    async fn plugins_key_registration_once_per_account_and_rotates_on_revoked() {
        let dir = tempfile::tempdir().unwrap();
        let fake = FakeDirectory::default();
        assert!(
            !ensure_registered(dir.path(), &fake, "dev1", "u1")
                .await
                .unwrap()
        );
        let first = super::super::signing::load_key(dir.path())
            .unwrap()
            .public();
        assert_eq!(fake.registered.lock().unwrap().clone(), vec![first.clone()]);
        // Again: nothing to do.
        ensure_registered(dir.path(), &fake, "dev1", "u1")
            .await
            .unwrap();
        assert_eq!(fake.registered.lock().unwrap().len(), 1);
        // Another account: registers again; this one says it's revoked.
        fake.register_answers
            .lock()
            .unwrap()
            .push(Err(RegisterError::Revoked));
        assert!(
            ensure_registered(dir.path(), &fake, "dev1", "u2")
                .await
                .unwrap()
        );
        let second = super::super::signing::load_key(dir.path())
            .unwrap()
            .public();
        assert_ne!(first, second);
        assert_eq!(fake.registered.lock().unwrap().last().unwrap(), &second);
        assert!(Registration::load(dir.path()).covers(&second, "u2"));
        let aside = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("plugin-signing-key.revoked-")
            });
        assert!(aside);
        // A failure leaves it unregistered (retried next start).
        fake.register_answers
            .lock()
            .unwrap()
            .push(Err(RegisterError::Other("down".into())));
        assert!(
            ensure_registered(dir.path(), &fake, "dev1", "u3")
                .await
                .is_err()
        );
        assert!(!Registration::load(dir.path()).covers(&second, "u3"));
    }
}
