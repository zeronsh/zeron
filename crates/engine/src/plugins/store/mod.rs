//! The plugin store, device side: deploy keys, the store's HTTP client,
//! publishing, and installing and updating plugins from the store.
//!
//! A plugin from the store is a package like any other (`install::parse`
//! verifies its signature over every file) that must also be exactly what
//! the store's record says: its file's sha256, its content hash, its id and
//! version, and the key that signed it when the record names one. Who made
//! it is who owns its id in the store (the record's publisher), so its
//! decision is bound to that owner (`StoreSource::binding`): an update
//! signed by another of the owner's keys (a deploy key, a new machine)
//! keeps what you allowed, and a new owner asks again. File, link,
//! release and git installs keep their own rules.

pub mod client;
pub mod deploy_key;
mod device;
pub mod publish;

use super::install::{self, Package, StoreSource, failure};
use super::signing;
use client::{PackageRecord, reason};

pub use device::handles;
pub(in crate::plugins) use device::StoreHandle;

/// Whether `name` reads as a username: 1 to 40 letters, digits, `_` and `-`.
pub fn username(name: &str) -> bool {
    (1..=40).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// The package `raw`, verified and checked against the store's record of
/// `id` at `version` (`record`), published by `publisher`: the package and
/// the store's record to keep beside it.
pub fn verify(raw: Vec<u8>, id: &str, version: &str, record: &PackageRecord, publisher: Option<&str>) -> Result<(Package, StoreSource), String> {
    let mismatch = |what: &str| failure(reason::STORE_MISMATCH, format!("What the store sent isn't what its record says ({what}), so it wasn't installed"));
    // A username: what "by @…" shows and consent is bound to, so nothing a
    // store could dress up (spaces, control or direction characters).
    let publisher = publisher.map(str::trim).filter(|p| username(p)).ok_or_else(|| mismatch("who published it"))?;
    if record.sha256.is_empty() || record.hash.is_empty() {
        return Err(mismatch("its record is incomplete"));
    }
    if signing::sha256_hex(&raw) != record.sha256.to_ascii_lowercase() {
        return Err(mismatch("the file"));
    }
    if record.bytes != 0 && record.bytes != raw.len() as u64 {
        return Err(mismatch("its size"));
    }
    let package = install::parse(raw)?;
    if package.id() != id {
        return Err(mismatch("its id"));
    }
    if package.manifest.version != version {
        return Err(mismatch("its version"));
    }
    if package.hash != record.hash.to_ascii_lowercase() {
        return Err(mismatch("its contents"));
    }
    if record.key.as_deref().is_some_and(|key| key != package.key) {
        return Err(mismatch("who signed it"));
    }
    let store = StoreSource { id: id.to_owned(), publisher: publisher.to_owned(), hash: package.hash.clone(), key: Some(package.key.clone()) };
    Ok((package, store))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::install::tests::signed_package;
    use crate::plugins::signing::SigningKey;

    fn record(raw: &[u8], package: &Package) -> PackageRecord {
        PackageRecord { hash: package.hash.clone(), sha256: signing::sha256_hex(raw), bytes: raw.len() as u64, key: Some(package.key.clone()) }
    }

    #[test]
    fn a_store_package_is_exactly_what_its_record_says() {
        let (key, _) = SigningKey::generate().unwrap();
        let raw = signed_package(&key, "focus", "1.2.0", &["ui.add"], "return 1");
        let package = install::parse(raw.clone()).unwrap();
        let good = record(&raw, &package);
        let (verified, store) = verify(raw.clone(), "focus", "1.2.0", &good, Some("ada")).unwrap();
        assert_eq!(verified.hash, package.hash);
        assert_eq!(store, StoreSource { id: "focus".into(), publisher: "ada".into(), hash: package.hash.clone(), key: Some(key.public()) });
        assert_eq!(store.binding(), "store:ada");

        let refused = |raw: Vec<u8>, id: &str, version: &str, record: &PackageRecord, publisher: Option<&str>| {
            let error = verify(raw, id, version, record, publisher).unwrap_err();
            install::reason_of(&error).map(str::to_owned).unwrap_or(error)
        };
        // Another plugin, version, file, content or key than the record's.
        assert_eq!(refused(raw.clone(), "other", "1.2.0", &good, Some("ada")), "store_mismatch");
        assert_eq!(refused(raw.clone(), "focus", "1.3.0", &good, Some("ada")), "store_mismatch");
        assert_eq!(refused(raw.clone(), "focus", "1.2.0", &PackageRecord { sha256: "00".repeat(32), ..good.clone() }, Some("ada")), "store_mismatch");
        assert_eq!(refused(raw.clone(), "focus", "1.2.0", &PackageRecord { bytes: 3, ..good.clone() }, Some("ada")), "store_mismatch");
        let (eve, _) = SigningKey::generate().unwrap();
        assert_eq!(refused(raw.clone(), "focus", "1.2.0", &PackageRecord { key: Some(eve.public()), ..good.clone() }, Some("ada")), "store_mismatch");
        // A record that names no publisher, or no hashes.
        assert_eq!(refused(raw.clone(), "focus", "1.2.0", &good, None), "store_mismatch");
        assert_eq!(refused(raw.clone(), "focus", "1.2.0", &good, Some(" ")), "store_mismatch");
        assert_eq!(refused(raw.clone(), "focus", "1.2.0", &good, Some("ada\u{202e}evil")), "store_mismatch");
        assert_eq!(refused(raw.clone(), "focus", "1.2.0", &good, Some("ada lovelace")), "store_mismatch");
        assert_eq!(refused(raw.clone(), "focus", "1.2.0", &PackageRecord { hash: String::new(), ..good.clone() }, Some("ada")), "store_mismatch");
        // A package whose content isn't the record's, though its file is (the
        // store's record names another build of the same version).
        let other = signed_package(&key, "focus", "1.2.0", &["ui.add"], "return 666");
        let theirs = PackageRecord { sha256: signing::sha256_hex(&other), bytes: other.len() as u64, ..good.clone() };
        assert_eq!(refused(other, "focus", "1.2.0", &theirs, Some("ada")), "store_mismatch");
        // A tampered package fails as any package does, record or not.
        let mut value: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        value["files"]["init.luau"] = serde_json::json!("cmV0dXJuIDI=");
        let tampered = serde_json::to_vec(&value).unwrap();
        let tampered_record = PackageRecord { sha256: signing::sha256_hex(&tampered), bytes: tampered.len() as u64, ..good.clone() };
        assert_eq!(refused(tampered, "focus", "1.2.0", &tampered_record, Some("ada")), "tampered");
        // The record's key is optional; its hashes compare in any case.
        let loose = PackageRecord { key: None, sha256: good.sha256.to_uppercase(), ..good.clone() };
        assert!(verify(raw, "focus", "1.2.0", &loose, Some("ada")).is_ok());
    }
}
