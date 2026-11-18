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
pub mod notices;
pub mod publish;
pub mod text_notes;

use super::install::{self, Package, StoreSource, failure};
use super::signing;
use client::{PackageRecord, Publisher, reason};

pub use device::{MAX_UPDATE_PLUGINS, REPORT_REASONS, handles};
pub(in crate::plugins) use device::{StoreHandle, publishes_idle};

/// Whether `name` reads as a username: 1 to 40 letters, digits, `_` and `-`.
pub fn username(name: &str) -> bool {
    (1..=40).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Whether `id` reads as a publisher's id: `pub_` and 32 lowercase hex
/// characters (amendment 33).
pub fn publisher_id(id: &str) -> bool {
    id.strip_prefix("pub_").is_some_and(|hex| hex.len() == 32 && hex.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
}

/// Who the store says published a plugin, `(username, id)`, when both read
/// as they must: else what the store sent isn't a record to install by.
pub fn vouched(publisher: &Publisher) -> Result<(String, String), String> {
    let mismatch = || failure(reason::STORE_MISMATCH, "What the store sent isn't what its record says (who published it), so it wasn't installed");
    // A username: what "by @…" shows, so nothing a store could dress up
    // (spaces, control or direction characters).
    let name = publisher.username.as_deref().map(str::trim).filter(|p| username(p)).ok_or_else(mismatch)?;
    let id = publisher.id.as_deref().map(str::trim).filter(|id| publisher_id(id)).ok_or_else(mismatch)?;
    Ok((name.to_owned(), id.to_owned()))
}

/// The package `raw`, verified and checked against the store's record of
/// `id` at `version` (`record`), published by `publisher`: the package and
/// the store's record to keep beside it.
/// `verified`: the version's record says Zeron verified it (fetched with
/// `record`, at install time).
pub fn verify(raw: Vec<u8>, id: &str, version: &str, record: &PackageRecord, publisher: &Publisher, verified: bool) -> Result<(Package, StoreSource), String> {
    let mismatch = |what: &str| failure(reason::STORE_MISMATCH, format!("What the store sent isn't what its record says ({what}), so it wasn't installed"));
    let (publisher, publisher_id) = vouched(publisher)?;
    if record.sha256.is_empty() || record.hash.is_empty() || record.bytes == 0 {
        return Err(mismatch("its record is incomplete"));
    }
    if signing::sha256_hex(&raw) != record.sha256.to_ascii_lowercase() {
        return Err(mismatch("the file"));
    }
    if record.bytes != raw.len() as u64 {
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
    let store = StoreSource { id: id.to_owned(), publisher, publisher_id, hash: package.hash.clone(), key: Some(package.key.clone()), verified, verified_at: None, verified_alternative: None, instead_of_unverified: false, replaced: None };
    Ok((package, store))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::install::tests::signed_package;
    use crate::plugins::signing::SigningKey;

    const ADA: &str = "pub_0123456789abcdef0123456789abcdef";

    fn ada() -> Publisher {
        Publisher { id: Some(ADA.into()), username: Some("ada".into()), verified: true }
    }

    fn record(raw: &[u8], package: &Package) -> PackageRecord {
        PackageRecord { hash: package.hash.clone(), sha256: signing::sha256_hex(raw), bytes: raw.len() as u64, key: Some(package.key.clone()) }
    }

    #[test]
    fn a_store_package_is_exactly_what_its_record_says() {
        let (key, _) = SigningKey::generate().unwrap();
        let raw = signed_package(&key, "focus", "1.2.0", &["ui.add"], "return 1");
        let package = install::parse(raw.clone()).unwrap();
        let good = record(&raw, &package);
        let (verified, store) = verify(raw.clone(), "focus", "1.2.0", &good, &ada(), true).unwrap();
        assert_eq!(verified.hash, package.hash);
        assert_eq!(store, StoreSource { id: "focus".into(), publisher: "ada".into(), publisher_id: ADA.into(), hash: package.hash.clone(), key: Some(key.public()), verified: true, verified_at: None, verified_alternative: None, instead_of_unverified: false, replaced: None });
        assert_eq!(store.binding(), format!("store:{ADA}"), "bound to the owner's id, never the username");

        let refused = |raw: Vec<u8>, id: &str, version: &str, record: &PackageRecord, publisher: Option<&str>| {
            let publisher = Publisher { id: Some(ADA.into()), username: publisher.map(str::to_owned), verified: true };
            let error = verify(raw, id, version, record, &publisher, true).unwrap_err();
            install::reason_of(&error).map(str::to_owned).unwrap_or(error)
        };
        // Another plugin, version, file, content or key than the record's.
        assert_eq!(refused(raw.clone(), "other", "1.2.0", &good, Some("ada")), "store_mismatch");
        assert_eq!(refused(raw.clone(), "focus", "1.3.0", &good, Some("ada")), "store_mismatch");
        assert_eq!(refused(raw.clone(), "focus", "1.2.0", &PackageRecord { sha256: "00".repeat(32), ..good.clone() }, Some("ada")), "store_mismatch");
        assert_eq!(refused(raw.clone(), "focus", "1.2.0", &PackageRecord { bytes: 3, ..good.clone() }, Some("ada")), "store_mismatch");
        assert_eq!(refused(raw.clone(), "focus", "1.2.0", &PackageRecord { bytes: 0, ..good.clone() }, Some("ada")), "store_mismatch", "a record without its size");
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
        assert!(verify(raw.clone(), "focus", "1.2.0", &loose, &ada(), true).is_ok());
        // A record without the owner's id, or one that isn't one.
        for id in [None, Some("ada"), Some("pub_XYZ"), Some("pub_0123456789abcdef0123456789ABCDEF")] {
            let publisher = Publisher { id: id.map(str::to_owned), username: Some("ada".into()), verified: true };
            assert_eq!(install::reason_of(&verify(raw.clone(), "focus", "1.2.0", &good, &publisher, true).unwrap_err()), Some("store_mismatch"), "{id:?}");
        }
    }
}
