//! Publishing a plugin: its folder read, checked against the store's rules
//! (`listing::check`, so every problem shows before anything is sent),
//! signed and packed. The same steps whether the person publishes from
//! Zeron's window or the CLI (signed with this device's key, uploaded with
//! their session) or a CI job does (signed with a deploy key, the upload
//! signed with it too).

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Value, json};

use crate::plugins::files;
use crate::plugins::install::{self, FORMAT, WORDS};
use crate::plugins::listing::{self, Listing, Problem};
use crate::plugins::manifest::{MANIFEST_FILE, Manifest};
use crate::plugins::package::{self, MAX_PACKAGE_BYTES};
use crate::plugins::signing::{self, SigningKey};

/// A plugin folder read for publishing: what it is, and what the store's
/// rules say of it.
#[derive(Debug, Clone)]
pub struct Prepared {
    pub manifest: Manifest,
    /// Every file it's made of (the manifest aside), as read.
    pub files: BTreeMap<String, Vec<u8>>,
    /// Its content hash (`signing::content_hash`): what's checked is what's
    /// sent.
    pub hash: String,
    pub problems: Vec<Problem>,
    pub warnings: Vec<Problem>,
    /// This version's notes, from its CHANGELOG.md.
    pub notes: Option<String>,
}

/// The plugin folder `dir`, read and checked. `Err` only when it can't be
/// read as a plugin at all (no manifest, a link in it, too many files).
pub fn read(dir: &Path) -> Result<Prepared, String> {
    let manifest_bytes = std::fs::read(dir.join(MANIFEST_FILE)).map_err(|_| install::failure(install::reason::NO_MANIFEST, "There's no zeron-plugin.json in that folder"))?;
    let manifest = Manifest::parse(&manifest_bytes)?;
    let hashes = files::hash_files(dir)?;
    let mut read = BTreeMap::new();
    for rel in hashes.keys() {
        let path = zeron_plugin_guard::path::join(dir, rel)?;
        let bytes = std::fs::read(&path).map_err(|_| format!("Couldn't read {rel}"))?;
        // What's packed is what was hashed: a file changing under the read
        // is caught when the package is verified.
        read.insert(rel.clone(), bytes);
    }
    let hash = signing::content_hash(&manifest, &package::hashes(&read));
    let (problems, warnings) = listing::check(&manifest, &read);
    let notes = read
        .get(listing::CHANGELOG)
        .and_then(|bytes| listing::changelog_notes(&String::from_utf8_lossy(bytes), &manifest.version));
    Ok(Prepared { manifest, files: read, hash, problems, warnings, notes })
}

impl Prepared {
    /// Whether nothing stands in the way of publishing it.
    pub fn ok(&self) -> bool {
        self.problems.is_empty()
    }

    /// It signed with `key` and packed, then read back and verified as any
    /// device will (`install::parse`): the package to send.
    pub fn pack(&self, key: &SigningKey) -> Result<Vec<u8>, String> {
        let hashes = package::hashes(&self.files);
        let manifest = key.sign_manifest(&self.manifest.unsigned_value(), &hashes);
        let raw = package::pack(FORMAT, &manifest, &self.files)?;
        if raw.len() as u64 > MAX_PACKAGE_BYTES {
            return Err(WORDS.too_big.into());
        }
        let verified = install::parse(raw.clone())?;
        if verified.hash != self.hash || verified.key != key.public() {
            return Err("It changed while it was being packed: try again".into());
        }
        Ok(raw)
    }

    /// What the person (or an agent) is shown of it.
    pub fn shown(&self) -> Value {
        let bytes: u64 = self.files.values().map(|b| b.len() as u64).sum();
        json!({
            "id": self.manifest.id,
            "name": self.manifest.name,
            "version": self.manifest.version,
            "description": self.manifest.description,
            "hash": self.hash,
            "ok": self.ok(),
            "problems": self.problems,
            "warnings": self.warnings,
            "listing": Listing::of(&self.manifest.raw),
            "notes": self.notes,
            "package": { "files": self.files.len(), "bytes": bytes },
        })
    }
}

/// Each problem as one line: `rule (field): words`.
pub fn lines(problems: &[Problem]) -> Vec<String> {
    problems
        .iter()
        .map(|p| {
            let at = p.path.as_deref().or(p.field).map(|a| format!(" ({a})")).unwrap_or_default();
            format!("{}{at}: {}", p.rule, p.words)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::listing::tests::listed;
    use crate::plugins::store::deploy_key::DeployKey;

    fn folder(manifest: &Value, files: &BTreeMap<String, Vec<u8>>) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (rel, bytes) in files {
            let path = dir.path().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
        std::fs::write(dir.path().join(MANIFEST_FILE), signing::pretty(manifest)).unwrap();
        dir
    }

    #[test]
    fn a_folder_is_checked_signed_and_packed_as_devices_will_read_it() {
        let (manifest, files) = listed();
        let dir = folder(&manifest, &files);
        // Dot entries (a repository's .git, a CI's .github) aren't part of it.
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".git/config"), b"x").unwrap();
        let prepared = read(dir.path()).unwrap();
        assert!(prepared.ok(), "{:?}", prepared.problems);
        assert_eq!(prepared.files.len(), files.len());
        assert_eq!(prepared.notes.as_deref(), Some("- Boards."));
        let key = DeployKey::from_seed([9; 32]).unwrap();
        let raw = prepared.pack(key.signing_key()).unwrap();
        let package = install::parse(raw).unwrap();
        assert_eq!((package.key, package.hash.clone()), (key.public(), prepared.hash.clone()));
        // An old signature in the folder doesn't matter: it's signed afresh.
        let (other, _) = SigningKey::generate().unwrap();
        let signed = other.sign_manifest(&manifest, &package::hashes(&files));
        let dir = folder(&signed, &files);
        let again = read(dir.path()).unwrap();
        assert_eq!(again.hash, prepared.hash, "the content hash leaves the signature out");
        assert_eq!(install::parse(again.pack(key.signing_key()).unwrap()).unwrap().key, key.public());
        assert_eq!(again.shown()["ok"], true);
    }

    #[test]
    fn every_problem_is_said_before_anything_is_sent() {
        let (mut manifest, mut files) = listed();
        manifest["version"] = json!("1.2");
        manifest["categories"] = json!(["games"]);
        files.remove(listing::README);
        let dir = folder(&manifest, &files);
        let prepared = read(dir.path()).unwrap();
        assert!(!prepared.ok());
        assert_eq!(
            lines(&prepared.problems),
            vec![
                "not_semver (version): \"1.2\" isn't a version (MAJOR.MINOR.PATCH): it needs three numbers, like 1.0.0".to_string(),
                "missing_readme: It needs a README.md: its page in the store".to_string(),
                "bad_listing (categories): \"games\" isn't one of the store's categories (agents, appearance, chat, developer-tools, git, integrations, productivity, utilities)".to_string(),
            ]
        );
        let shown = prepared.shown();
        assert_eq!(shown["problems"][0]["rule"], "not_semver");
        // A folder that isn't a plugin, or holds a link: refused outright.
        assert!(read(tempfile::tempdir().unwrap().path()).unwrap_err().starts_with("no_manifest"));
        #[cfg(unix)]
        {
            let dir = folder(&listed().0, &listed().1);
            std::os::unix::fs::symlink("/etc/passwd", dir.path().join("leak.txt")).unwrap();
            assert_eq!(read(dir.path()).unwrap_err(), files::LINK_ERROR);
        }
    }
}
