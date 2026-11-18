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

use super::super::signing::{self, SigningKey};

pub const SECRET_PREFIX: &str = "zpk_";
/// The environment variable `zeron plugin publish` reads the secret from.
pub const ENV: &str = "ZERON_DEPLOY_KEY";
/// What a deploy key's proof of possession signs, before `<plugin id>:<key>`.
pub const PROOF_PREFIX: &str = "zeron-deploy-key:v1:";
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
        // (Best effort: the pair inside ring keeps its own copy.)
        self.seed.iter_mut().for_each(|b| *b = 0);
    }
}

/// Why a secret can't be used, in words a CI log can carry (never quoting it).
pub const NOT_A_SECRET: &str = "That isn't a deploy key's secret (it starts zpk_): copy it again from where it was shown when the key was made";
pub const DAMAGED: &str = "That deploy key's secret is cut short or mistyped: copy it again from where it was shown when the key was made";

impl DeployKey {
    /// A new key, from the system's randomness.
    pub fn generate() -> Result<Self, String> {
        let mut seed = [0u8; SEED_LEN];
        ring::rand::SystemRandom::new()
            .fill(&mut seed)
            .map_err(|_| "Couldn't make a key".to_string())?;
        Self::from_seed(seed)
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
        let bytes = B64URL.decode(body).map_err(|_| DAMAGED)?;
        if bytes.len() != SEED_LEN + CHECK_LEN {
            return Err(DAMAGED.into());
        }
        let (seed, check) = bytes.split_at(SEED_LEN);
        if sha2::Sha256::digest(seed)[..CHECK_LEN] != *check {
            return Err(DAMAGED.into());
        }
        let mut owned = [0u8; SEED_LEN];
        owned.copy_from_slice(seed);
        Self::from_seed(owned)
    }

    /// The secret, to show once.
    pub fn secret(&self) -> String {
        let mut bytes = self.seed.to_vec();
        bytes.extend_from_slice(&sha2::Sha256::digest(self.seed)[..CHECK_LEN]);
        format!("{SECRET_PREFIX}{}", B64URL.encode(bytes))
    }

    /// The public half, base64url without padding.
    pub fn public(&self) -> String {
        self.key.public()
    }

    /// What it signs packages with.
    pub fn signing_key(&self) -> &SigningKey {
        &self.key
    }

    /// Its proof of possession, for registering it to `plugin`.
    pub fn proof(&self, plugin: &str) -> String {
        self.key
            .sign(proof_message(plugin, &self.public()).as_bytes())
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

/// What registering `key` to `plugin` proves possession with.
pub fn proof_message(plugin: &str, key: &str) -> String {
    format!("{PROOF_PREFIX}{plugin}:{key}")
}

/// What a signed request's signature covers.
pub fn request_message(method: &str, path: &str, body_sha256: &str, ts: i64) -> String {
    format!("{REQUEST_PREFIX}{method}\n{path}\n{body_sha256}\n{ts}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::{ED25519, UnparsedPublicKey};

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
        assert_eq!(proof_message("git", "KEY"), "zeron-deploy-key:v1:git:KEY");
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
        assert!(verify(
            &proof_message("focus", &key.public()),
            &key.proof("focus")
        ));
        assert!(
            !verify(&proof_message("other", &key.public()), &key.proof("focus")),
            "scoped to its plugin"
        );
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
