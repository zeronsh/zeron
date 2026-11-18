//! Deploy keys: an ed25519 key pair made on the author's device, scoped to
//! one plugin and registered at the edge (with a fresh sign-in and a proof
//! of possession), whose secret goes in a CI's secrets. In CI, `zeron
//! plugin publish` signs the package with it and signs the upload request
//! with it: one secret does both.
//!
//! The secret is `zpk_` + base64url(seed ‖ sha256(seed)[..4]): the 32-byte
//! seed and a check, so a secret cut short or mistyped is told apart from a
//! wrong one. Zeron shows it once, when it's made, and never writes it
//! anywhere; the edge only ever sees the public half.
//!
//! A request is signed as the contract says: `Authorization: ZeronKey
//! key=<b64url>, ts=<ms>, sig=<b64url>`, where `sig` signs
//! `"zeron-plugin-publish:v1\n" + method + "\n" + path + "\n" +
//! sha256hex(body) + "\n" + ts`. `path` is the URL's path alone (no query).
//! The body's hash also goes as `X-Zeron-Content-SHA256` (for a request
//! without a body, the hash of nothing).

use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use ring::rand::SecureRandom as _;
use sha2::Digest as _;
use zeroize::{Zeroize as _, Zeroizing};

use super::super::signing::{self, SigningKey};

pub const SECRET_PREFIX: &str = "zpk_";
/// The environment variable `zeron plugin publish` reads the secret from.
pub const ENV: &str = "ZERON_DEPLOY_KEY";
/// What a deploy key's proof of possession signs, before
/// `<publisher id>:<expiry>:<scope text>:<key>` (v2: one account's key, one
/// lifetime, one scope, so a session can't widen or extend it; v1, which
/// named one plugin, is refused).
pub const PROOF_PREFIX: &str = "zeron-deploy-key:v2:";
/// The most plugins one key's scope names.
pub const MAX_SCOPE_PLUGINS: usize = 50;
/// What a signed request's signature covers, first.
pub const REQUEST_PREFIX: &str = "zeron-plugin-publish:v1\n";
pub const SCHEME: &str = "ZeronKey";
/// How far a signed request's time may be from the edge's.
pub const MAX_SKEW_MS: i64 = 5 * 60 * 1000;
/// The header a signed request carries its body's sha256 (hex) in, the
/// same hash its signature covers, so the edge can refuse a body before
/// reading it.
pub const CONTENT_SHA256_HEADER: &str = "X-Zeron-Content-SHA256";

const SEED_LEN: usize = 32;
const CHECK_LEN: usize = 4;

/// A deploy key: its seed (the secret) and the pair it makes. Never
/// printed: `Debug` shows only the public half.
pub struct DeployKey {
    seed: [u8; SEED_LEN],
    key: SigningKey,
}

impl fmt::Debug for DeployKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeployKey")
            .field("public", &self.public())
            .finish_non_exhaustive()
    }
}

impl Drop for DeployKey {
    fn drop(&mut self) {
        // (Zeroed so it can't be optimised away; the pair inside ring keeps
        // its own copy, which ring's own drop handles.)
        self.seed.zeroize();
    }
}

/// Whether `key` is a deploy key's public half as the store names it:
/// base64url of 32 bytes.
pub fn valid_public(key: &str) -> bool {
    key.len() == 43 && B64URL.decode(key).is_ok_and(|bytes| bytes.len() == SEED_LEN)
}

/// What a key may publish, fixed when it's made (amendment 44): every
/// plugin of its account's, those it publishes later included, or the
/// plugins named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    All,
    /// Sorted and unique.
    Plugins(Vec<String>),
}

impl Scope {
    /// The plugins named (`ids`, trimmed, sorted, unique), or `Err` when
    /// there are none or more than [`MAX_SCOPE_PLUGINS`], or one isn't an id.
    pub fn plugins(ids: &[String]) -> Result<Self, String> {
        let mut ids: Vec<String> = ids.iter().map(|id| id.trim().to_owned()).collect();
        ids.sort();
        ids.dedup();
        if ids.is_empty() || ids.len() > MAX_SCOPE_PLUGINS {
            return Err(format!("A key is for 1 to {MAX_SCOPE_PLUGINS} plugins, or for all of yours"));
        }
        if let Some(bad) = ids.iter().find(|id| super::super::manifest::check_id(id).is_err()) {
            return Err(format!("\"{bad}\" isn't a plugin's id"));
        }
        Ok(Self::Plugins(ids))
    }

    /// As the proof binds it: `*`, or the ids joined with `,`.
    pub fn text(&self) -> String {
        match self {
            Scope::All => "*".to_owned(),
            Scope::Plugins(ids) => ids.join(","),
        }
    }

    /// As the store takes and answers it: `{all: true}` or `{plugins: [...]}`.
    pub fn to_value(&self) -> serde_json::Value {
        match self {
            Scope::All => serde_json::json!({ "all": true }),
            Scope::Plugins(ids) => serde_json::json!({ "plugins": ids }),
        }
    }

    /// Whether it covers plugin `id`.
    pub fn covers(&self, id: &str) -> bool {
        match self {
            Scope::All => true,
            Scope::Plugins(ids) => ids.iter().any(|p| p == id),
        }
    }
}

/// Why a secret can't be used, in words a CI log can carry (never quoting it).
pub const NOT_A_SECRET: &str = "That isn't a deploy key's secret (it starts zpk_): copy it again from where it was shown when the key was made";
pub const DAMAGED: &str = "That deploy key's secret is cut short or mistyped: copy it again from where it was shown when the key was made";

impl DeployKey {
    /// A new key, from the system's randomness.
    pub fn generate() -> Result<Self, String> {
        let mut seed = Zeroizing::new([0u8; SEED_LEN]);
        ring::rand::SystemRandom::new()
            .fill(&mut seed[..])
            .map_err(|_| "Couldn't make a key".to_string())?;
        Self::from_seed(*seed)
    }

    pub fn from_seed(seed: [u8; SEED_LEN]) -> Result<Self, String> {
        let key = SigningKey::from_seed(&seed).ok_or("Couldn't make a key")?;
        Ok(Self { seed, key })
    }

    /// The key a secret (`zpk_…`, surrounding whitespace ignored) is.
    pub fn from_secret(secret: &str) -> Result<Self, String> {
        let body = secret
            .trim()
            .strip_prefix(SECRET_PREFIX)
            .ok_or(NOT_A_SECRET)?;
        let bytes = Zeroizing::new(B64URL.decode(body).map_err(|_| DAMAGED)?);
        if bytes.len() != SEED_LEN + CHECK_LEN {
            return Err(DAMAGED.into());
        }
        let (seed, check) = bytes.split_at(SEED_LEN);
        if sha2::Sha256::digest(seed)[..CHECK_LEN] != *check {
            return Err(DAMAGED.into());
        }
        let mut owned = Zeroizing::new([0u8; SEED_LEN]);
        owned.copy_from_slice(seed);
        Self::from_seed(*owned)
    }

    /// The secret, to show once.
    pub fn secret(&self) -> String {
        let mut bytes = Zeroizing::new(self.seed.to_vec());
        bytes.extend_from_slice(&sha2::Sha256::digest(self.seed)[..CHECK_LEN]);
        format!("{SECRET_PREFIX}{}", B64URL.encode(&*bytes))
    }

    /// The public half, base64url without padding.
    pub fn public(&self) -> String {
        self.key.public()
    }

    /// What it signs packages with.
    pub fn signing_key(&self) -> &SigningKey {
        &self.key
    }

    /// Its proof of possession, for registering it to the account
    /// `publisher_id` with `scope`, until `expires_at` (ms; `None`: never).
    pub fn proof(&self, publisher_id: &str, expires_at: Option<i64>, scope: &Scope) -> String {
        self.key
            .sign(proof_message(publisher_id, expires_at, scope, &self.public()).as_bytes())
    }

    /// The `Authorization` header for `method path` with `body`, at `ts`
    /// (epoch ms).
    pub fn authorization(&self, method: &str, path: &str, body: &[u8], ts: i64) -> String {
        let message = request_message(method, path, &signing::sha256_hex(body), ts);
        format!(
            "{SCHEME} key={}, ts={ts}, sig={}",
            self.public(),
            self.key.sign(message.as_bytes())
        )
    }
}

/// What registering `key` to the account `publisher_id` with `scope`, until
/// `expires_at` (ms; `None`: `never`), proves possession with.
pub fn proof_message(publisher_id: &str, expires_at: Option<i64>, scope: &Scope, key: &str) -> String {
    let expiry = expires_at.map_or_else(|| "never".to_owned(), |ms| ms.to_string());
    format!("{PROOF_PREFIX}{publisher_id}:{expiry}:{}:{key}", scope.text())
}

/// What a signed request's signature covers.
pub fn request_message(method: &str, path: &str, body_sha256: &str, ts: i64) -> String {
    format!("{REQUEST_PREFIX}{method}\n{path}\n{body_sha256}\n{ts}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::{ED25519, UnparsedPublicKey};

    #[test]
    fn a_public_key_is_32_bytes_of_base64url() {
        let key = DeployKey::from_seed([5; 32]).unwrap();
        assert!(valid_public(&key.public()));
        for not in ["", ".", "..", "abc", &format!("{}=", key.public()), &key.public()[1..], &key.secret()] {
            assert!(!valid_public(not), "{not}");
        }
    }

    /// The edge's own expectations (edge-identity `signing.test.ts`), byte
    /// for byte: the signing string, an empty body's hash, the proof.
    #[test]
    fn the_signing_string_is_the_edge_s() {
        let body = signing::sha256_hex(&[1, 2, 3]);
        assert_eq!(body, "039058c6f2c0cb492c533b0a4d14ef77cc0f78abccced5287d84a1a2011cfb81");
        assert_eq!(
            request_message("POST", "/store/v1/plugins/git/versions", &body, 1_700_000_000_000),
            format!("zeron-plugin-publish:v1\nPOST\n/store/v1/plugins/git/versions\n{body}\n1700000000000")
        );
        assert_eq!(signing::sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(proof_message("pub_1", None, &Scope::All, "KEY"), "zeron-deploy-key:v2:pub_1:never:*:KEY");
        let two = Scope::plugins(&["git".into(), " focus".into(), "git".into()]).unwrap();
        assert_eq!(proof_message("pub_1", Some(1_800_000_000_000), &two, "KEY"), "zeron-deploy-key:v2:pub_1:1800000000000:focus,git:KEY");
        assert_eq!(two.to_value(), serde_json::json!({ "plugins": ["focus", "git"] }));
        assert!(Scope::plugins(&[]).is_err() && Scope::plugins(&["Not An Id".into()]).is_err());
        assert!(Scope::plugins(&(0..51).map(|n| format!("p{n:02}")).collect::<Vec<_>>()).is_err());
        assert_eq!(CONTENT_SHA256_HEADER, "X-Zeron-Content-SHA256");
    }

    #[test]
    fn a_secret_round_trips_and_a_damaged_one_is_told_apart() {
        let key = DeployKey::generate().unwrap();
        let secret = key.secret();
        assert!(
            secret.starts_with("zpk_") && secret.len() == 4 + 48,
            "{secret}"
        );
        let again = DeployKey::from_secret(&format!("  {secret}\n")).unwrap();
        assert_eq!(again.public(), key.public());
        assert_eq!(DeployKey::from_secret("ghp_x").unwrap_err(), NOT_A_SECRET);
        assert_eq!(
            DeployKey::from_secret(&secret[..secret.len() - 1]).unwrap_err(),
            DAMAGED
        );
        // One character changed: the check catches it.
        let mut typo = secret.clone().into_bytes();
        let at = 10;
        typo[at] = if typo[at] == b'A' { b'B' } else { b'A' };
        assert_eq!(
            DeployKey::from_secret(&String::from_utf8(typo).unwrap()).unwrap_err(),
            DAMAGED
        );
        // Never printed.
        assert!(!format!("{key:?}").contains(&secret[4..]));
    }

    #[test]
    fn its_proof_and_requests_verify_with_its_public_half() {
        let key = DeployKey::from_seed([7; 32]).unwrap();
        let public = B64URL.decode(key.public()).unwrap();
        let verify = |message: &str, sig: &str| {
            UnparsedPublicKey::new(&ED25519, &public)
                .verify(message.as_bytes(), &B64URL.decode(sig).unwrap())
                .is_ok()
        };
        let focus = Scope::plugins(&["focus".into()]).unwrap();
        let proof = key.proof("pub_a", None, &focus);
        assert!(verify(&proof_message("pub_a", None, &focus, &key.public()), &proof));
        // One account's key, one lifetime, one scope: none can be widened.
        assert!(!verify(&proof_message("pub_a", None, &Scope::All, &key.public()), &proof), "another scope");
        assert!(!verify(&proof_message("pub_b", None, &focus, &key.public()), &proof), "another account");
        assert!(!verify(&proof_message("pub_a", Some(1), &focus, &key.public()), &proof), "another lifetime");
        let header = key.authorization(
            "POST",
            "/store/v1/plugins/focus/versions",
            b"{}",
            1_700_000_000_000,
        );
        let sig = header.rsplit("sig=").next().unwrap();
        assert!(header.starts_with(&format!(
            "ZeronKey key={}, ts=1700000000000, sig=",
            key.public()
        )));
        let message = format!(
            "zeron-plugin-publish:v1\nPOST\n/store/v1/plugins/focus/versions\n{}\n1700000000000",
            signing::sha256_hex(b"{}")
        );
        assert!(verify(&message, sig));
        // The package it signs verifies as a plugin's, by that key.
        let files = [("init.luau".to_string(), signing::sha256_hex(b"x"))]
            .into_iter()
            .collect();
        let manifest = key
            .signing_key()
            .sign_manifest(&serde_json::json!({ "id": "focus" }), &files);
        assert_eq!(
            signing::check_raw(signing::PAYLOAD_PREFIX, &manifest, &files),
            signing::SigCheck::Valid(key.public())
        );
    }
}
