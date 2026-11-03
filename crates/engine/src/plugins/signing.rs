//! Canonical JSON, content hashes and ed25519 signatures (contract "Signing").

use std::collections::BTreeMap;
use std::path::Path;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use ring::signature::{ED25519, Ed25519KeyPair, KeyPair, UnparsedPublicKey};
use sha2::Digest as _;

use super::manifest::Manifest;

pub const PAYLOAD_PREFIX: &str = "zeron-plugin:v1\n";
/// A template package's (plugins/template_packages.rs): its own domain, so a
/// plugin's signature never verifies as a template's, or the reverse.
pub const TEMPLATE_PAYLOAD_PREFIX: &str = "zeron-template:v1\n";
pub const KEY_PROOF_PREFIX: &str = "zeron-plugin-key:v1:";
pub const KEY_FILE: &str = "plugin-signing-key";

pub type FileHashes = BTreeMap<String, String>;

/// Objects with keys sorted bytewise, no whitespace, arrays in order —
/// independent of whether serde_json preserves insertion order.
pub fn canonical_json(value: &serde_json::Value) -> Vec<u8> {
    let mut out = Vec::new();
    write_canonical(value, &mut out);
    out
}

fn write_canonical(value: &serde_json::Value, out: &mut Vec<u8>) {
    match value {
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
            out.push(b'{');
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                out.extend(serde_json::to_vec(key).unwrap_or_default());
                out.push(b':');
                write_canonical(&map[key], out);
            }
            out.push(b'}');
        }
        serde_json::Value::Array(items) => {
            out.push(b'[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write_canonical(item, out);
            }
            out.push(b']');
        }
        scalar => out.extend(serde_json::to_vec(scalar).unwrap_or_default()),
    }
}

pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&sha2::Sha256::digest(bytes))
}

fn files_value(files: &FileHashes) -> serde_json::Value {
    serde_json::Value::Object(
        files
            .iter()
            .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
            .collect(),
    )
}

/// hex sha256 of `canonical(manifest without signature) + "\n" + canonical(F)`.
pub fn content_hash(manifest: &Manifest, files: &FileHashes) -> String {
    let mut bytes = canonical_json(&manifest.unsigned_value());
    bytes.push(b'\n');
    bytes.extend(canonical_json(&files_value(files)));
    sha256_hex(&bytes)
}

/// The bytes a plugin's signature covers: the manifest with `signature =
/// {key, files}`.
pub fn payload(manifest: &serde_json::Value, key: &str, files: &FileHashes) -> Vec<u8> {
    payload_for(PAYLOAD_PREFIX, manifest, key, files)
}

/// The bytes a signature covers in `prefix`'s domain (a plugin's, a
/// template package's): the manifest with `signature = {key, files}`.
pub fn payload_for(prefix: &str, manifest: &serde_json::Value, key: &str, files: &FileHashes) -> Vec<u8> {
    let mut value = manifest.clone();
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "signature".into(),
            serde_json::json!({ "key": key, "files": files_value(files) }),
        );
    }
    let mut bytes = prefix.as_bytes().to_vec();
    bytes.extend(canonical_json(&value));
    bytes
}

/// What a manifest's signature says about the files on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SigCheck {
    Unsigned,
    /// `signature.files` differs from the files on disk.
    FilesDiffer,
    /// Malformed, or the ed25519 check failed.
    BadSignature,
    /// Checks out, by this public key (b64url).
    Valid(String),
}

pub fn check(manifest: &Manifest, files: &FileHashes) -> SigCheck {
    let Some(signature) = &manifest.signature else {
        return SigCheck::Unsigned;
    };
    let Ok(signature) = signature else {
        return SigCheck::BadSignature;
    };
    if &signature.files != files {
        return SigCheck::FilesDiffer;
    }
    verify(PAYLOAD_PREFIX, &manifest.raw, &signature.key, &signature.sig, files)
}

/// `check` in `prefix`'s domain, for a manifest as raw JSON (a template
/// package's, which isn't a plugin `Manifest`): its `signature = {key,
/// files, sig}` against `files`.
pub fn check_raw(prefix: &str, manifest: &serde_json::Value, files: &FileHashes) -> SigCheck {
    let Some(signature) = manifest.get("signature") else {
        return SigCheck::Unsigned;
    };
    let (Some(key), Some(sig), Some(signed)) = (
        signature.get("key").and_then(|k| k.as_str()),
        signature.get("sig").and_then(|s| s.as_str()),
        signature.get("files").and_then(|f| f.as_object()),
    ) else {
        return SigCheck::BadSignature;
    };
    let signed: Option<FileHashes> = signed
        .iter()
        .map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_owned())))
        .collect();
    let Some(signed) = signed else {
        return SigCheck::BadSignature;
    };
    if &signed != files {
        return SigCheck::FilesDiffer;
    }
    verify(prefix, manifest, key, sig, files)
}

/// The ed25519 check of `sig` by `key` over `prefix`'s payload.
fn verify(prefix: &str, manifest: &serde_json::Value, key: &str, sig: &str, files: &FileHashes) -> SigCheck {
    let (Ok(key_bytes), Ok(sig_bytes)) = (B64URL.decode(key), B64URL.decode(sig)) else {
        return SigCheck::BadSignature;
    };
    if key_bytes.len() != 32 || sig_bytes.len() != 64 {
        return SigCheck::BadSignature;
    }
    let message = payload_for(prefix, manifest, key, files);
    match UnparsedPublicKey::new(&ED25519, &key_bytes).verify(&message, &sig_bytes) {
        Ok(()) => SigCheck::Valid(key.to_owned()),
        Err(_) => SigCheck::BadSignature,
    }
}

/// This device's signing key.
pub struct SigningKey {
    pair: Ed25519KeyPair,
}

impl SigningKey {
    pub fn from_pkcs8(der: &[u8]) -> Option<Self> {
        Ed25519KeyPair::from_pkcs8(der)
            .ok()
            .map(|pair| Self { pair })
    }

    pub fn generate() -> Result<(Self, Vec<u8>), String> {
        let rng = ring::rand::SystemRandom::new();
        let der = Ed25519KeyPair::generate_pkcs8(&rng)
            .map_err(|_| "Couldn't make a signing key".to_string())?;
        let key = Self::from_pkcs8(der.as_ref()).ok_or("Couldn't make a signing key")?;
        Ok((key, der.as_ref().to_vec()))
    }

    /// The public key, b64url without padding.
    pub fn public(&self) -> String {
        B64URL.encode(self.pair.public_key().as_ref())
    }

    pub fn sign(&self, message: &[u8]) -> String {
        B64URL.encode(self.pair.sign(message).as_ref())
    }

    /// `b64url(sig("zeron-plugin-key:v1:" + key))` for `POST /plugin-keys`.
    pub fn proof(&self) -> String {
        self.sign(format!("{KEY_PROOF_PREFIX}{}", self.public()).as_bytes())
    }

    /// `manifest` with a fresh `signature` over `files` (key order kept: an
    /// existing `signature` is replaced where it stands).
    pub fn sign_manifest(
        &self,
        manifest: &serde_json::Value,
        files: &FileHashes,
    ) -> serde_json::Value {
        self.sign_manifest_for(PAYLOAD_PREFIX, manifest, files)
    }

    /// `sign_manifest` in `prefix`'s domain (a template package's).
    pub fn sign_manifest_for(
        &self,
        prefix: &str,
        manifest: &serde_json::Value,
        files: &FileHashes,
    ) -> serde_json::Value {
        let key = self.public();
        let sig = self.sign(&payload_for(prefix, manifest, &key, files));
        let mut value = manifest.clone();
        if let Some(object) = value.as_object_mut() {
            object.insert(
                "signature".into(),
                serde_json::json!({ "key": key, "files": files_value(files), "sig": sig }),
            );
        }
        value
    }
}

/// Read `<data>/plugin-signing-key`, if there is one.
pub fn load_key(data_dir: &Path) -> Option<SigningKey> {
    let der = std::fs::read(data_dir.join(KEY_FILE)).ok()?;
    SigningKey::from_pkcs8(&der)
}

/// This device's proof for a device-room dial to `target` on `conn_id` at
/// `ts` (epoch ms): `(public key, signature)` over
/// [`zeron_rpc::caller_proof_message`], both b64url. `None` without a key
/// (never made here: an unregistered key would prove nothing). The private
/// key never leaves this module.
pub fn device_link_proof(data_dir: &Path, target: &str, conn_id: &str, ts: i64) -> Option<(String, String)> {
    let key = load_key(data_dir)?;
    let message = zeron_rpc::caller_proof_message(target, conn_id, ts);
    Some((key.public(), key.sign(message.as_bytes())))
}

/// Held while the key file is read and made, so two first uses at once
/// (the service starting and a sign-in landing) agree on one key: without
/// it, each made its own, and the one registered could be the one whose
/// file lost.
static KEY_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Read the key, or make and store one (0600).
pub fn load_or_create_key(data_dir: &Path) -> Result<SigningKey, String> {
    let _held = KEY_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    load_or_create_locked(data_dir)
}

fn load_or_create_locked(data_dir: &Path) -> Result<SigningKey, String> {
    if let Some(key) = load_key(data_dir) {
        return Ok(key);
    }
    let (key, der) = SigningKey::generate()?;
    crate::runner::write_private_atomic(&data_dir.join(KEY_FILE), &der)
        .map_err(|_| "Couldn't save this device's signing key".to_string())?;
    Ok(key)
}

/// Delete the current key (and any set aside by older versions as
/// `plugin-signing-key.revoked-<unix>`) and make a fresh one. A retired
/// private key is never kept: whoever holds it can sign as it.
pub fn rotate_key(data_dir: &Path) -> Result<SigningKey, String> {
    let _held = KEY_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    delete_locked(data_dir)?;
    load_or_create_locked(data_dir)
}

/// Delete this device's key, and old ones set aside (the next use makes
/// a fresh one).
pub fn delete_key(data_dir: &Path) {
    let _held = KEY_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Err(error) = delete_locked(data_dir) {
        tracing::warn!(%error, "plugins: couldn't delete the old signing key");
    }
}

fn delete_locked(data_dir: &Path) -> Result<(), String> {
    let path = data_dir.join(KEY_FILE);
    if std::fs::symlink_metadata(&path).is_ok() {
        std::fs::remove_file(&path).map_err(|_| "Couldn't replace this device's signing key".to_string())?;
    }
    let aside = format!("{KEY_FILE}.revoked-");
    for entry in std::fs::read_dir(data_dir).into_iter().flatten().flatten() {
        if entry.file_name().to_string_lossy().starts_with(&aside) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    Ok(())
}

/// The manifest as Zeron writes it: 2-space indent, trailing newline.
pub fn pretty(value: &serde_json::Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec_pretty(value).unwrap_or_default();
    bytes.push(b'\n');
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugins_first_uses_at_once_agree_on_one_key() {
        let dir = std::env::temp_dir().join(format!("zeron-key-race-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let keys: Vec<String> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|_| scope.spawn(|| load_or_create_key(&dir).unwrap().public()))
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        assert!(keys.windows(2).all(|w| w[0] == w[1]), "{keys:?}");
        assert_eq!(load_key(&dir).unwrap().public(), keys[0]);
    }

    fn manifest(value: serde_json::Value) -> Manifest {
        Manifest::from_value(value).unwrap()
    }

    #[test]
    fn plugins_and_templates_never_verify_as_each_other() {
        let (key, _) = SigningKey::generate().unwrap();
        let raw = serde_json::json!({ "id": "x", "name": "X", "version": "1.0.0", "entry": "init.luau", "permissions": [] });
        let as_plugin = key.sign_manifest(&raw, &files());
        let as_template = key.sign_manifest_for(TEMPLATE_PAYLOAD_PREFIX, &raw, &files());
        assert!(matches!(check(&manifest(as_plugin.clone()), &files()), SigCheck::Valid(_)));
        assert!(matches!(check_raw(TEMPLATE_PAYLOAD_PREFIX, &as_template, &files()), SigCheck::Valid(_)));
        // A plugin's signature isn't a template's, and the reverse.
        assert_eq!(check_raw(TEMPLATE_PAYLOAD_PREFIX, &as_plugin, &files()), SigCheck::BadSignature);
        assert_eq!(check(&manifest(as_template.clone()), &files()), SigCheck::BadSignature);
        // Its files changed: told apart from a bad signature.
        let other: FileHashes = [("init.luau".to_string(), sha256_hex(b"print(2)"))].into_iter().collect();
        assert_eq!(check_raw(TEMPLATE_PAYLOAD_PREFIX, &as_template, &other), SigCheck::FilesDiffer);
        assert_eq!(check_raw(TEMPLATE_PAYLOAD_PREFIX, &raw, &files()), SigCheck::Unsigned);
    }

    #[test]
    fn plugins_payload_bytes_are_pinned() {
        // A fixed manifest's plugin payload, byte for byte: a refactor of the
        // prefixes can't move what existing signatures cover.
        let raw = serde_json::json!({ "name": "Pin", "id": "pin" });
        let bytes = payload(&raw, "KEY", &files());
        let expected = format!(
            "zeron-plugin:v1\n{{\"id\":\"pin\",\"name\":\"Pin\",\"signature\":{{\"files\":{{\"init.luau\":\"{}\"}},\"key\":\"KEY\"}}}}",
            sha256_hex(b"print(1)")
        );
        assert_eq!(String::from_utf8(bytes).unwrap(), expected);
    }

    fn files() -> FileHashes {
        [("init.luau".to_string(), sha256_hex(b"print(1)"))]
            .into_iter()
            .collect()
    }

    #[test]
    fn plugins_canonical_json_sorts_keys_bytewise() {
        let value = serde_json::json!({ "b": [2, { "z": 1, "a": "x y" }], "B": true, "a": null });
        assert_eq!(
            String::from_utf8(canonical_json(&value)).unwrap(),
            r#"{"B":true,"a":null,"b":[2,{"a":"x y","z":1}]}"#
        );
    }

    #[test]
    fn plugins_signature_round_trip() {
        let (key, der) = SigningKey::generate().unwrap();
        let again = SigningKey::from_pkcs8(&der).unwrap();
        assert_eq!(key.public(), again.public());
        let raw = serde_json::json!({ "id": "focus", "name": "Focus", "version": "1", "entry": "init.luau" });
        let signed = key.sign_manifest(&raw, &files());
        let parsed = manifest(signed.clone());
        assert_eq!(check(&parsed, &files()), SigCheck::Valid(key.public()));
        // Content hash ignores the signature.
        assert_eq!(
            content_hash(&parsed, &files()),
            content_hash(&manifest(raw.clone()), &files())
        );
        assert_eq!(check(&manifest(raw), &files()), SigCheck::Unsigned);

        // A changed manifest field breaks it.
        let mut edited = signed.clone();
        edited["version"] = serde_json::json!("2");
        assert_eq!(check(&manifest(edited), &files()), SigCheck::BadSignature);
        // A forged sig breaks it.
        let mut forged = signed.clone();
        forged["signature"]["sig"] = serde_json::json!(key.sign(b"something else"));
        assert_eq!(check(&manifest(forged), &files()), SigCheck::BadSignature);
        // Malformed.
        let mut junk = signed.clone();
        junk["signature"] = serde_json::json!("nope");
        assert_eq!(check(&manifest(junk), &files()), SigCheck::BadSignature);
    }

    #[test]
    fn plugins_tamper_detection() {
        let (key, _) = SigningKey::generate().unwrap();
        let raw = serde_json::json!({ "id": "focus", "name": "Focus", "version": "1", "entry": "init.luau" });
        let signed = manifest(key.sign_manifest(&raw, &files()));
        let mut extra = files();
        extra.insert("evil.luau".into(), sha256_hex(b"x"));
        assert_eq!(
            check(&signed, &extra),
            SigCheck::FilesDiffer,
            "an extra file"
        );
        let mut changed = files();
        changed.insert("init.luau".into(), sha256_hex(b"print(2)"));
        assert_eq!(
            check(&signed, &changed),
            SigCheck::FilesDiffer,
            "a changed file"
        );
        assert_eq!(
            check(&signed, &FileHashes::new()),
            SigCheck::FilesDiffer,
            "a missing file"
        );
    }

    #[test]
    fn plugins_proof_verifies() {
        let (key, _) = SigningKey::generate().unwrap();
        let public = B64URL.decode(key.public()).unwrap();
        let proof = B64URL.decode(key.proof()).unwrap();
        let message = format!("{KEY_PROOF_PREFIX}{}", key.public());
        UnparsedPublicKey::new(&ED25519, &public)
            .verify(message.as_bytes(), &proof)
            .unwrap();
        assert_eq!(public.len(), 32);
        assert_eq!(proof.len(), 64);
    }
}
