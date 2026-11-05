//! Signed packages: one JSON file carrying a signed manifest and the files
//! it covers, `{format, manifest, files: {rel: base64}}`. Plugins
//! (`.zeron-plugin`, the plugin domain) and templates (`.zeron-template`,
//! the template domain) share it. Each kind brings its format, its signing
//! prefix, which paths it keeps, and its words for a refusal.
//!
//! Reading a package checks everything before anything is written: the
//! format, every path, the sizes, and the signature over every file's
//! sha256. What's done after (setting files aside, installing) is the
//! kind's own, on what was verified.

use std::collections::BTreeMap;
use std::path::Path;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use serde_json::{Value, json};

use super::files::{MAX_BYTES, MAX_FILES};
use super::signing::{self, FileHashes, SigCheck};

/// A package's largest size on disk: its files' limit, as base64, and room.
pub const MAX_PACKAGE_BYTES: u64 = MAX_BYTES * 4 / 3 + 1024 * 1024;

/// What a kind says when it refuses a package.
pub struct Words {
    pub not_a_package: &'static str,
    pub unsigned: &'static str,
    pub tampered: &'static str,
    pub bad_signature: &'static str,
    pub too_big: &'static str,
    /// Of a file it won't write (`{rel}` in it is the file's name).
    pub bad_file: &'static str,
}

/// A package, read and verified: its signed manifest, the key that signed
/// it, every file it carries, and the package as read with its sha256 (what
/// you're shown is what's installed).
#[derive(Debug, Clone)]
pub struct Verified {
    pub manifest: Value,
    pub key: String,
    pub files: BTreeMap<String, Vec<u8>>,
    pub raw: Vec<u8>,
    pub hash: String,
}

/// `manifest` (signed over `files`' hashes) and `files` as a package.
pub fn pack(format: &str, manifest: &Value, files: &BTreeMap<String, Vec<u8>>) -> Result<Vec<u8>, String> {
    let package = json!({
        "format": format,
        "manifest": manifest,
        "files": files.iter().map(|(rel, bytes)| (rel.clone(), json!(B64.encode(bytes)))).collect::<serde_json::Map<_, _>>(),
    });
    serde_json::to_vec(&package).map_err(|e| format!("Couldn't write it: {e}"))
}

/// The bytes of the package file at `path`, if it's one and not too big.
pub fn read(path: &Path, words: &Words) -> Result<Vec<u8>, String> {
    let meta = std::fs::metadata(path).map_err(|_| "That file can't be found.".to_string())?;
    if !meta.is_file() {
        return Err("That isn't a file.".into());
    }
    if meta.len() > MAX_PACKAGE_BYTES {
        return Err(words.too_big.into());
    }
    std::fs::read(path).map_err(|e| format!("Couldn't read it: {e}"))
}

/// The package `raw`, of `format`, signed in `prefix`'s domain, its paths
/// each kept by `keep_path`: verified, or why not.
pub fn parse(raw: Vec<u8>, format: &str, prefix: &str, keep_path: impl Fn(&str) -> bool, words: &Words) -> Result<Verified, String> {
    if raw.len() as u64 > MAX_PACKAGE_BYTES {
        return Err(words.too_big.into());
    }
    let value: Value = serde_json::from_slice(&raw).map_err(|_| words.not_a_package.to_string())?;
    if value["format"].as_str() != Some(format) {
        return Err(words.not_a_package.into());
    }
    let manifest = value["manifest"].clone();
    let files = value["files"].as_object().ok_or(words.not_a_package)?;
    if !manifest.is_object() {
        return Err(words.not_a_package.into());
    }
    if files.len() > MAX_FILES {
        return Err(words.too_big.into());
    }
    let mut decoded = BTreeMap::new();
    let mut total = 0u64;
    for (rel, data) in files {
        if !keep_path(rel) {
            return Err(words.bad_file.replace("{rel}", &format!("{rel:?}")));
        }
        let bytes = data
            .as_str()
            .and_then(|s| B64.decode(s).ok())
            .ok_or_else(|| words.bad_file.replace("{rel}", &format!("{rel:?}")))?;
        total += bytes.len() as u64;
        decoded.insert(rel.clone(), bytes);
    }
    if total > MAX_BYTES {
        return Err(words.too_big.into());
    }
    let hashes = hashes(&decoded);
    let key = match signing::check_raw(prefix, &manifest, &hashes) {
        SigCheck::Valid(key) => key,
        SigCheck::Unsigned => return Err(words.unsigned.into()),
        SigCheck::FilesDiffer => return Err(words.tampered.into()),
        SigCheck::BadSignature => return Err(words.bad_signature.into()),
    };
    let hash = signing::sha256_hex(&raw);
    Ok(Verified { manifest, key, files: decoded, raw, hash })
}

/// Each file's sha256, as a signature covers them.
pub fn hashes(files: &BTreeMap<String, Vec<u8>>) -> FileHashes {
    files.iter().map(|(rel, bytes)| (rel.clone(), signing::sha256_hex(bytes))).collect()
}

/// A plain relative path: `/`-separated parts, none empty, `.` or `..`, and
/// none starting with a dot (dot entries are never part of what's signed);
/// and one that stays in its folder on every system (no `C:`, device
/// names or trailing dots: crates/plugin-guard's `path`).
pub fn plain_path(rel: &str) -> bool {
    zeron_plugin_guard::path::is_safe(rel) && rel.split('/').all(|part| !part.starts_with('.'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::signing::{PAYLOAD_PREFIX, SigningKey};

    const WORDS: Words = Words {
        not_a_package: "not a package",
        unsigned: "unsigned",
        tampered: "tampered",
        bad_signature: "bad signature",
        too_big: "too big",
        bad_file: "won't write {rel}",
    };

    fn signed() -> (Vec<u8>, String) {
        let (key, _) = SigningKey::generate().unwrap();
        let mut files = BTreeMap::new();
        files.insert("init.luau".to_string(), b"return 1".to_vec());
        let manifest = key.sign_manifest(&json!({ "id": "p", "name": "P" }), &hashes(&files));
        (pack("zeron-plugin:v1", &manifest, &files).unwrap(), key.public())
    }

    #[test]
    fn a_package_verifies_over_every_file_before_anything_is_kept() {
        let (raw, key) = signed();
        let v = parse(raw.clone(), "zeron-plugin:v1", PAYLOAD_PREFIX, plain_path, &WORDS).unwrap();
        assert_eq!((v.key, v.files.len(), v.hash), (key, 1, signing::sha256_hex(&raw)));
        // Another kind's format, a file changed, a path outside, unsigned.
        assert_eq!(parse(raw.clone(), "zeron-template:v1", PAYLOAD_PREFIX, plain_path, &WORDS).unwrap_err(), "not a package");
        let mut value: Value = serde_json::from_slice(&raw).unwrap();
        value["files"]["init.luau"] = json!(B64.encode(b"return 2"));
        assert_eq!(parse(serde_json::to_vec(&value).unwrap(), "zeron-plugin:v1", PAYLOAD_PREFIX, plain_path, &WORDS).unwrap_err(), "tampered");
        let mut value: Value = serde_json::from_slice(&raw).unwrap();
        value["files"]["../evil"] = json!(B64.encode(b"x"));
        assert!(parse(serde_json::to_vec(&value).unwrap(), "zeron-plugin:v1", PAYLOAD_PREFIX, plain_path, &WORDS).unwrap_err().starts_with("won't write"));
        let mut value: Value = serde_json::from_slice(&raw).unwrap();
        value["manifest"].as_object_mut().unwrap().remove("signature");
        assert_eq!(parse(serde_json::to_vec(&value).unwrap(), "zeron-plugin:v1", PAYLOAD_PREFIX, plain_path, &WORDS).unwrap_err(), "unsigned");
    }

    /// R9: a signed package naming a drive-relative path (`D:x.cmd`, or
    /// `C:../…/Startup/x.cmd`), a device name or a trailing dot is refused
    /// before anything is written: on Windows each would land outside the
    /// staging folder.
    #[test]
    fn a_package_path_that_escapes_on_windows_is_refused() {
        let (key, _) = SigningKey::generate().unwrap();
        for bad in ["D:x.cmd", "C:../Roaming/Microsoft/Windows/Start Menu/Programs/Startup/x.cmd", "ui/NUL.luau", "init.luau.", "a:stream"] {
            let mut files = BTreeMap::new();
            files.insert("init.luau".to_string(), b"return 1".to_vec());
            files.insert(bad.to_string(), b"evil".to_vec());
            let manifest = key.sign_manifest(&json!({ "id": "p", "name": "P" }), &hashes(&files));
            let raw = pack("zeron-plugin:v1", &manifest, &files).unwrap();
            let refused = parse(raw, "zeron-plugin:v1", PAYLOAD_PREFIX, plain_path, &WORDS);
            assert!(refused.is_err_and(|e| e.starts_with("won't write")), "{bad}");
        }
    }

    #[test]
    fn plain_paths_only() {
        assert!(plain_path("init.luau") && plain_path("ui/badge.luau"));
        for bad in ["", "/etc/x", "../x", "a/../b", ".git/config", "a//b", "a\\b", "D:x.cmd", "CON", "com1.luau", "a. ", "x."] {
            assert!(!plain_path(bad), "{bad}");
        }
    }
}
