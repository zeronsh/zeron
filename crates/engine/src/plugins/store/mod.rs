//! The plugin store, device side: deploy tokens, the store's HTTP client,
//! publishing, and installing and updating plugins from the store.
//!
//! A plugin from the store is a package like any other (`install::parse`
//! verifies its signature over every file) that must also be exactly what
//! the store's record says: its file's sha256, its content hash, its id and
//! version, and the key that signed it when the record names one. A
//! version published with a deploy token is unsigned and its record names
//! no key: taken by its contents, which the record's hash vouches for, now
//! and on every scan (`install::parse_from_store`, `scan::store_record`).
//! Who made it is who owns its id in the store (the record's publisher),
//! so its decision is bound to that owner (`StoreSource::binding`): an
//! update signed by another of the owner's keys (a new machine), or none
//! (a deploy token's), keeps what you allowed, and a new owner asks again.
//! File, link, release and git installs keep their own rules: signed.

pub mod client;
pub mod deploy_token;
mod device;
pub mod notices;
pub mod publish;
pub mod text_notes;

use super::install::{self, Package, StoreSource, failure};
use super::{signing, template_packages};
use client::{PackageRecord, Publisher, reason};

pub use device::{MAX_UPDATE_ITEMS, REPORT_REASONS, handles};
pub(in crate::plugins) use device::{ShownRecord, StoreHandle, record_state};
#[cfg(test)]
pub(in crate::plugins) use device::publishes_idle;

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
/// the store's record to keep beside it. Signed by the key the record
/// names; a record that names none (a deploy token's version) takes it
/// unsigned, its contents vouched for by the record's hash alone.
/// `verified`: the version's record says Zeron verified it (fetched with
/// `record`, at install time).
pub fn verify(raw: Vec<u8>, id: &str, version: &str, record: &PackageRecord, publisher: &Publisher, verified: bool) -> Result<(Package, StoreSource), String> {
    let (publisher, publisher_id) = vouched(publisher)?;
    sent(&raw, record)?;
    let package = install::parse_from_store(raw, record.key.is_some())?;
    recorded(id, version, record, &package.id(), &package.manifest.version, &package.hash, package.key.as_deref())?;
    Ok((package, store_source(id, publisher, publisher_id, record, verified)))
}

/// [`verify`] for a template (contract "Templates in the store"): its
/// package, signed in the templates' domain by the key the record names, or
/// unsigned under a record naming none, its contents vouched for by the
/// record's hash.
pub fn verify_template(raw: Vec<u8>, id: &str, version: &str, record: &PackageRecord, publisher: &Publisher, verified: bool) -> Result<(template_packages::Package, StoreSource), String> {
    let (publisher, publisher_id) = vouched(publisher)?;
    sent(&raw, record)?;
    let package = template_packages::parse_from_store(raw, record.key.is_some())
        .map_err(|error| failure(template_packages::refusal(&error).unwrap_or(reason::STORE_ERROR), error))?;
    recorded(id, version, record, &package.id(), &package.version(), &package.content_hash, package.key.as_deref())?;
    Ok((package, store_source(id, publisher, publisher_id, record, verified)))
}

fn mismatch(what: &str) -> String {
    failure(reason::STORE_MISMATCH, format!("What the store sent isn't what its record says ({what}), so it wasn't installed"))
}

/// The file `raw` is the one `record` names: its sha256 and its size.
fn sent(raw: &[u8], record: &PackageRecord) -> Result<(), String> {
    if record.sha256.is_empty() || record.hash.is_empty() || record.bytes == 0 {
        return Err(mismatch("its record is incomplete"));
    }
    if signing::sha256_hex(raw) != record.sha256.to_ascii_lowercase() {
        return Err(mismatch("the file"));
    }
    if record.bytes != raw.len() as u64 {
        return Err(mismatch("its size"));
    }
    Ok(())
}

/// A package (its own id, version, content hash and signer) is what the
/// store's record of `id` at `version` says.
fn recorded(id: &str, version: &str, record: &PackageRecord, package_id: &str, package_version: &str, hash: &str, key: Option<&str>) -> Result<(), String> {
    if package_id != id {
        return Err(mismatch("its id"));
    }
    if package_version != version {
        return Err(mismatch("its version"));
    }
    if hash != record.hash.to_ascii_lowercase() {
        return Err(mismatch("its contents"));
    }
    if record.key.as_deref().is_some_and(|named| Some(named) != key) {
        return Err(mismatch("who signed it"));
    }
    Ok(())
}

/// The store's record to keep beside an install of it.
fn store_source(id: &str, publisher: String, publisher_id: String, record: &PackageRecord, verified: bool) -> StoreSource {
    StoreSource {
        id: id.to_owned(),
        publisher,
        publisher_id,
        hash: record.hash.to_ascii_lowercase(),
        key: record.key.clone(),
        last_key: None,
        verified,
        verified_at: None,
        verified_alternative: None,
        instead_of_unverified: false,
        replaced: None,
    }
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
        PackageRecord { hash: package.hash.clone(), sha256: signing::sha256_hex(raw), bytes: raw.len() as u64, key: package.key.clone() }
    }

    /// A template from the store is exactly what its record says, signed by
    /// the key it names or unsigned under one that names none; a plugin's
    /// package never passes for one, nor one signed in the plugins' domain.
    #[test]
    fn a_store_template_is_exactly_what_its_record_says() {
        let (key, _) = SigningKey::generate().unwrap();
        let manifest = serde_json::json!({ "id": "midnight", "name": "Midnight", "description": "Dark.", "version": "1.2.0", "zeron_version": "0.3.16" });
        let files: std::collections::BTreeMap<String, Vec<u8>> = [("app.luau".to_owned(), b"return 1".to_vec()), ("preview.png".to_owned(), b"png".to_vec())].into();
        let signed = template_packages::pack(&manifest, &files, Some(&key)).unwrap();
        let unsigned = template_packages::pack(&manifest, &files, None).unwrap();
        let record = |raw: &[u8], key: Option<String>| PackageRecord {
            hash: template_packages::parse_from_store(raw.to_vec(), key.is_some()).unwrap().content_hash,
            sha256: signing::sha256_hex(raw),
            bytes: raw.len() as u64,
            key,
        };
        let good = record(&signed, Some(key.public()));
        let (package, store) = verify_template(signed.clone(), "midnight", "1.2.0", &good, &ada(), true).unwrap();
        assert_eq!((package.id(), store.key.clone(), store.verified), ("midnight".into(), Some(key.public()), true));
        assert_eq!((store.hash.as_str(), store.publisher.as_str(), store.publisher_id.as_str()), (package.content_hash.as_str(), "ada", ADA));
        let unkeyed = record(&unsigned, None);
        assert_eq!(unkeyed.hash, good.hash, "the signature is no part of the contents");
        let (package, store) = verify_template(unsigned.clone(), "midnight", "1.2.0", &unkeyed, &ada(), false).unwrap();
        assert_eq!((package.key, store.key, store.verified), (None, None, false));

        let refused = |raw: &[u8], id: &str, version: &str, record: &PackageRecord| {
            let error = verify_template(raw.to_vec(), id, version, record, &ada(), true).unwrap_err();
            install::reason_of(&error).map(str::to_owned).unwrap_or(error)
        };
        for (record, what) in [
            (PackageRecord { sha256: "00".repeat(32), ..good.clone() }, "the file"),
            (PackageRecord { bytes: 3, ..good.clone() }, "its size"),
            (PackageRecord { hash: "00".repeat(32), ..good.clone() }, "its contents"),
            (PackageRecord { hash: String::new(), ..good.clone() }, "no hash"),
            (PackageRecord { key: Some(SigningKey::generate().unwrap().0.public()), ..good.clone() }, "its signer"),
            (PackageRecord { key: None, ..good.clone() }, "signed, under a record naming no signer"),
        ] {
            assert_eq!(refused(&signed, "midnight", "1.2.0", &record), "store_mismatch", "{what}");
        }
        assert_eq!(refused(&signed, "other", "1.2.0", &good), "store_mismatch");
        assert_eq!(refused(&signed, "midnight", "1.3.0", &good), "store_mismatch");
        // Unsigned where the record names a signer.
        let named = PackageRecord { key: Some(key.public()), ..unkeyed.clone() };
        assert_eq!(refused(&unsigned, "midnight", "1.2.0", &named), "unsigned");
        // Another kind's package, or domain.
        let plugin = signed_package(&key, "midnight", "1.2.0", &["ui.add"], "return 1");
        let as_sent = |raw: &[u8]| PackageRecord { sha256: signing::sha256_hex(raw), bytes: raw.len() as u64, ..good.clone() };
        assert_eq!(refused(&plugin, "midnight", "1.2.0", &as_sent(&plugin)), "not_a_package");
        let hashes = crate::plugins::package::hashes(&files);
        let wrong_domain = crate::plugins::package::pack(template_packages::FORMAT, &key.sign_manifest(&manifest, &hashes), &files).unwrap();
        assert_eq!(refused(&wrong_domain, "midnight", "1.2.0", &as_sent(&wrong_domain)), "bad_signature");
        // No publisher to name.
        let nobody = Publisher { id: None, username: Some("ada".into()), verified: true };
        assert!(verify_template(signed, "midnight", "1.2.0", &good, &nobody, true).is_err());
    }

    #[test]
    fn a_store_package_is_exactly_what_its_record_says() {
        let (key, _) = SigningKey::generate().unwrap();
        let raw = signed_package(&key, "focus", "1.2.0", &["ui.add"], "return 1");
        let package = install::parse(raw.clone()).unwrap();
        let good = record(&raw, &package);
        let (verified, store) = verify(raw.clone(), "focus", "1.2.0", &good, &ada(), true).unwrap();
        assert_eq!(verified.hash, package.hash);
        assert_eq!(store, StoreSource { id: "focus".into(), publisher: "ada".into(), publisher_id: ADA.into(), hash: package.hash.clone(), key: Some(key.public()), last_key: None, verified: true, verified_at: None, verified_alternative: None, instead_of_unverified: false, replaced: None });
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
        // Its hashes compare in any case.
        let loud = PackageRecord { sha256: good.sha256.to_uppercase(), ..good.clone() };
        assert!(verify(raw.clone(), "focus", "1.2.0", &loud, &ada(), true).is_ok());
        // A record naming no signer takes unsigned, and nothing else (a
        // deploy token's version): this one, signed, is refused.
        let unsigned_only = PackageRecord { key: None, ..good.clone() };
        assert_eq!(refused(raw.clone(), "focus", "1.2.0", &unsigned_only, Some("ada")), "store_mismatch");
        // A record without the owner's id, or one that isn't one.
        for id in [None, Some("ada"), Some("pub_XYZ"), Some("pub_0123456789abcdef0123456789ABCDEF")] {
            let publisher = Publisher { id: id.map(str::to_owned), username: Some("ada".into()), verified: true };
            assert_eq!(install::reason_of(&verify(raw.clone(), "focus", "1.2.0", &good, &publisher, true).unwrap_err()), Some("store_mismatch"), "{id:?}");
        }
    }
}
