//! Publishing a plugin: its folder read, checked against the store's rules
//! (`listing::check`, so every problem shows before anything is sent),
//! signed and packed. The same steps whether the person publishes from
//! Zeron's window or the CLI (signed with this device's key, uploaded with
//! their session) or a CI job does (unsigned, uploaded with a deploy token:
//! the store records the version as the token's).

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Value, json};

use crate::plugins::files;
use crate::plugins::install::{self, FORMAT, WORDS};
use crate::plugins::listing::{self, Listing, Problem};
use crate::plugins::manifest::{MANIFEST_FILE, Manifest};
use crate::plugins::package::{self, MAX_PACKAGE_BYTES};
use crate::plugins::signing::{self, SigningKey};
use crate::plugins::{template_packages, templates};

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
    let (mut problems, mut warnings) = listing::check(&manifest, &read);
    if let Some(measure) = SigningKey::from_seed(&[0; 32]) {
        problems.extend(sizes(&measure.sign_manifest(&manifest.unsigned_value(), &package::hashes(&read)), FORMAT, &read));
    }
    // What the stores show as typed (raw HTML, footnotes): advice, the
    // publish check's own, never a failure.
    for name in [listing::README, listing::CHANGELOG] {
        if let Some(bytes) = read.get(name) {
            warnings.extend(super::text_notes::notes(name, &String::from_utf8_lossy(bytes)));
        }
    }
    let notes = read
        .get(listing::CHANGELOG)
        .and_then(|bytes| listing::changelog_notes(&String::from_utf8_lossy(bytes), &manifest.version));
    Ok(Prepared { manifest, files: read, hash, problems, warnings, notes })
}

/// What the store bounds a package by as it's uploaded (amendment 34): the
/// file's size, and its manifest's (as written in it, signature included)
/// size and number of values. Measured on the package as it would be sent:
/// a signature is the same size whoever signs, so any key measures it.
/// (`signed`: the manifest so signed; `format`: the package's.)
fn sizes(signed: &Value, format: &str, files: &BTreeMap<String, Vec<u8>>) -> Vec<Problem> {
    let mut problems = Vec::new();
    let manifest_bytes = serde_json::to_vec(signed).map_or(0, |b| b.len());
    if manifest_bytes > listing::MAX_MANIFEST_JSON_BYTES || listing::count_values(signed) > listing::MAX_MANIFEST_VALUES {
        problems.push(Problem {
            rule: listing::rule::TOO_LARGE,
            field: Some("manifest"),
            path: None,
            line: None,
            cause: None,
            word: None,
            words: "Its manifest is too big for the store (1 MB and 100,000 values, its signature included)".into(),
        });
    }
    let total: usize = files.values().map(Vec::len).sum();
    // (Skipped past the engine's own limit: `listing::check` says so.)
    if total as u64 <= files::MAX_BYTES
        && package::pack(format, signed, files).is_ok_and(|raw| raw.len() > listing::MAX_PUBLISH_BYTES)
    {
        problems.push(Problem {
            rule: listing::rule::TOO_LARGE,
            field: Some("package"),
            path: None,
            line: None,
            cause: None,
            word: None,
            words: "It's too big for the store: its package is over 20 MB".into(),
        });
    }
    problems
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
        if verified.hash != self.hash || verified.key.as_deref() != Some(key.public().as_str()) {
            return Err("It changed while it was being packed: try again".into());
        }
        Ok(raw)
    }

    /// It packed unsigned, for a deploy token to publish (the store records
    /// it as the token's, its contents vouched for by the record's hash),
    /// then read back as a device will (`install::parse_from_store`).
    pub fn pack_unsigned(&self) -> Result<Vec<u8>, String> {
        let raw = package::pack(FORMAT, &self.manifest.unsigned_value(), &self.files)?;
        if raw.len() as u64 > MAX_PACKAGE_BYTES {
            return Err(WORDS.too_big.into());
        }
        let verified = install::parse_from_store(raw.clone(), false)?;
        if verified.hash != self.hash || verified.key.is_some() {
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

/// A template folder read for publishing (contract "Templates in the
/// store"): what its package carries, and what the store's rules say of it.
#[derive(Debug, Clone)]
pub struct PreparedTemplate {
    /// Its manifest, unsigned, as packed.
    pub manifest: Value,
    /// Every file its package carries.
    pub files: BTreeMap<String, Vec<u8>>,
    /// Its content hash: what's checked is what's sent.
    pub hash: String,
    pub problems: Vec<Problem>,
    pub warnings: Vec<Problem>,
    /// This version's notes, from its CHANGELOG.md.
    pub notes: Option<String>,
}

/// The template folder `dir`, read as an export reads it
/// (`template_packages::contents`) and checked. `Err` only when it can't be
/// read as a template at all.
pub fn read_template(dir: &Path) -> Result<PreparedTemplate, String> {
    let no_manifest = || install::failure(install::reason::NO_MANIFEST, "There's no zeron-template.json in that folder");
    let bytes = templates::read_regular(&dir.join(templates::MANIFEST)).ok_or_else(no_manifest)?;
    let own: Value = serde_json::from_slice(&bytes).map_err(|_| install::failure(listing::rule::BAD_PACKAGE, "Its zeron-template.json isn't JSON"))?;
    let id = own["id"].as_str().unwrap_or("").trim();
    let (manifest, files) = template_packages::contents(dir, id)?;
    let hashes = package::hashes(&files);
    let hash = signing::content_hash_of(&manifest, &hashes);
    let (mut problems, mut warnings) = listing::check_template(&manifest, &files);
    if let Some(measure) = SigningKey::from_seed(&[0; 32]) {
        problems.extend(sizes(&measure.sign_manifest_for(signing::TEMPLATE_PAYLOAD_PREFIX, &manifest, &hashes), template_packages::FORMAT, &files));
    }
    for name in [listing::README, listing::CHANGELOG] {
        if let Some(bytes) = files.get(name) {
            warnings.extend(super::text_notes::notes(name, &String::from_utf8_lossy(bytes)));
        }
    }
    let notes = files
        .get(listing::CHANGELOG)
        .and_then(|bytes| listing::changelog_notes(&String::from_utf8_lossy(bytes), manifest["version"].as_str().unwrap_or("")));
    Ok(PreparedTemplate { manifest, files, hash, problems, warnings, notes })
}

impl PreparedTemplate {
    pub fn id(&self) -> &str {
        self.manifest["id"].as_str().unwrap_or("")
    }

    pub fn version(&self) -> &str {
        self.manifest["version"].as_str().unwrap_or("")
    }

    /// Whether nothing stands in the way of publishing it.
    pub fn ok(&self) -> bool {
        self.problems.is_empty()
    }

    /// It packed, signed by `key` in the templates' domain or (none) unsigned
    /// for a deploy token to publish, then read back as a device will.
    pub fn pack(&self, key: Option<&SigningKey>) -> Result<Vec<u8>, String> {
        let raw = template_packages::pack(&self.manifest, &self.files, key)?;
        let read = template_packages::parse_from_store(raw.clone(), key.is_some())?;
        if read.content_hash != self.hash || read.key != key.map(SigningKey::public) {
            return Err("It changed while it was being packed: try again".into());
        }
        Ok(raw)
    }
}

/// Whether `version` is one the store will take beside the versions it
/// already has of the plugin (`versions`: `[{version, status}]`, the
/// owner's view or its public list), as amendment 21 says: equal precedence to any version
/// submitted is `version_exists`; a release must be above every release
/// that isn't rejected or withdrawn, a pre-release above every version that
/// isn't, else `not_newer`. (The store has the last word: this is what it
/// said a moment ago.)
pub fn newer_problem(version: &str, versions: &[Value]) -> Option<Problem> {
    use crate::plugins::version::Version;
    let mine = Version::parse(version).ok()?;
    let mut above: Option<Version> = None;
    for v in versions {
        let Some(theirs) = v["version"].as_str().and_then(|t| Version::parse(t).ok()) else { continue };
        if theirs == mine {
            return Some(Problem {
                rule: "version_exists",
                field: Some("version"),
                path: None,
                line: None,
                cause: None,
                word: None,
                words: format!("Version {version} was submitted already: a version is submitted once, so give this one a new number"),
            });
        }
        // (Every version the plugin has had counts, in any state: amendment 45.)
        if above.as_ref().is_none_or(|a| &theirs > a) {
            above = Some(theirs);
        }
    }
    let above = above.filter(|a| &mine < a)?;
    Some(Problem {
        rule: "not_newer",
        field: Some("version"),
        path: None,
        line: None,
        cause: None,
        word: None,
        words: format!("Version {version} isn't later than {above}, which the store has"),
    })
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

    /// A template folder is read as its package will carry it, checked, and
    /// packed signed or (a deploy token's) unsigned, the same contents.
    #[test]
    fn a_template_folder_is_read_checked_and_packed() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_template(dir.path()).unwrap_err().starts_with("no_manifest: "));
        let write = |rel: &str, bytes: &[u8]| {
            let path = dir.path().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        };
        write("zeron-template.json", br#"{"id":"midnight","name":"Midnight","description":"Dark.","author":"you","version":"1.2.0","zeron_version":"0.3.16"}"#);
        write("app.luau", b"return 1");
        write("preview.png", b"\x89PNG\r\n\x1a\n");
        write("CHANGELOG.md", b"## 1.2.0\n- Darker.");
        write(".base.json", b"{}");
        let missing = read_template(dir.path()).unwrap();
        assert_eq!(missing.problems.iter().map(|p| p.rule).collect::<Vec<_>>(), ["missing_readme"]);
        write("README.md", b"# Midnight");
        let ready = read_template(dir.path()).unwrap();
        assert!(ready.ok(), "{:?}", ready.problems);
        assert_eq!((ready.id(), ready.version(), ready.notes.as_deref(), ready.files.len()), ("midnight", "1.2.0", Some("- Darker."), 4));
        let (key, _) = SigningKey::generate().unwrap();
        let signed = template_packages::parse(ready.pack(Some(&key)).unwrap()).unwrap();
        let unsigned = template_packages::parse_from_store(ready.pack(None).unwrap(), false).unwrap();
        assert_eq!((signed.key, unsigned.key), (Some(key.public()), None));
        assert!(signed.content_hash == ready.hash && unsigned.content_hash == ready.hash);
    }

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
        let key = SigningKey::from_seed(&[9; 32]).unwrap();
        let raw = prepared.pack(&key).unwrap();
        let package = install::parse(raw).unwrap();
        assert_eq!((package.key, package.hash.clone()), (Some(key.public()), prepared.hash.clone()));
        // For a deploy token: unsigned, the same contents (the same hash),
        // as a device takes it from the store; never as a sideload.
        let unsigned = prepared.pack_unsigned().unwrap();
        let package = install::parse_from_store(unsigned.clone(), false).unwrap();
        assert_eq!((package.key, package.hash.clone()), (None, prepared.hash.clone()));
        assert_eq!(install::reason_of(&install::parse(unsigned).unwrap_err()), Some("unsigned"));
        // An old signature in the folder doesn't matter: it's signed afresh.
        let (other, _) = SigningKey::generate().unwrap();
        let signed = other.sign_manifest(&manifest, &package::hashes(&files));
        let dir = folder(&signed, &files);
        let again = read(dir.path()).unwrap();
        assert_eq!(again.hash, prepared.hash, "the content hash leaves the signature out");
        assert_eq!(install::parse(again.pack(&key).unwrap()).unwrap().key, Some(key.public()));
        assert_eq!(again.shown()["ok"], true);
    }

    /// Amendment 34: the store's own bounds, said before anything is sent.
    #[test]
    fn the_store_s_size_bounds_are_checked_here() {
        let (mut manifest, mut files) = listed();
        let dir = folder(&manifest, &files);
        assert!(read(dir.path()).unwrap().ok());
        // Over 20 MB as uploaded (16 MB of bytes is over 21 MB as base64).
        files.insert("data.bin".into(), vec![0; 16 * 1024 * 1024]);
        let dir = folder(&manifest, &files);
        let rules: Vec<_> = read(dir.path()).unwrap().problems.iter().map(|p| (p.rule, p.field)).collect();
        assert_eq!(rules, vec![("too_large", Some("package"))]);
        files.remove("data.bin");
        // Over 100,000 values in its manifest.
        manifest["x-table"] = json!(vec![0; listing::MAX_MANIFEST_VALUES]);
        let dir = folder(&manifest, &files);
        let rules: Vec<_> = read(dir.path()).unwrap().problems.iter().map(|p| (p.rule, p.field)).collect();
        assert_eq!(rules, vec![("too_large", Some("manifest"))]);
        assert_eq!(listing::count_values(&json!({ "a": [1, 2], "b": { "c": null } })), 6);
    }

    /// Amendment 21, as the store says it.
    #[test]
    fn a_version_is_newer_as_the_store_counts_it() {
        let have = |list: &[(&str, &str)]| -> Vec<Value> { list.iter().map(|(v, s)| json!({ "version": v, "status": s })).collect() };
        let rule = |version: &str, list: &[(&str, &str)]| newer_problem(version, &have(list)).map(|p| p.rule);
        // Every version the plugin has had counts, in any state (amendment 45).
        let store = [("1.0.0", "listed"), ("1.1.0", "taken_down"), ("1.2.0-beta.1", "yanked"), ("0.9.0", "removed")];
        assert_eq!(rule("1.2.0-beta.2", &store), None, "above every version, the pre-release included");
        assert_eq!(rule("1.1.5", &store), Some("not_newer"), "1.2.0-beta.1 is above it, yanked or not");
        assert_eq!(rule("1.0.1", &store), Some("not_newer"), "1.1.0 counts though it was taken down");
        assert_eq!(rule("1.1.0", &store), Some("version_exists"), "taken down, it's still had");
        assert_eq!(rule("0.9.0", &store), Some("version_exists"), "removed, it's still had");
        assert_eq!(rule("1.0.0+build.2", &store), Some("version_exists"), "build metadata isn't a new version");
        assert_eq!(rule("1.2.0", &store), None);
        assert_eq!(rule("1.0.0", &[]), None, "a first version");
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

    #[test]
    fn what_the_stores_show_as_typed_is_advice_never_a_failure() {
        let (manifest, mut files) = listed();
        let readme = "# Focus Board\n\n<p align=\"center\"><img src=\"shots/board.png\"></p>\n\nFast.[^1]\n\n[^1]: Mostly.\n";
        files.insert(listing::README.into(), readme.as_bytes().to_vec());
        let log = String::from_utf8(files[listing::CHANGELOG].clone()).unwrap();
        files.insert(listing::CHANGELOG.into(), format!("{log}\n<details>Older</details>\n").into_bytes());
        let prepared = read(folder(&manifest, &files).path()).unwrap();
        assert!(prepared.ok(), "{:?}", prepared.problems);
        let said: Vec<(&str, Option<&str>)> = prepared.warnings.iter().map(|w| (w.rule, w.path.as_deref())).collect();
        assert_eq!(said, [("raw_html", Some("README.md")), ("footnote", Some("README.md")), ("raw_html", Some("CHANGELOG.md"))]);
        // Each says its line, for a caller saying its own words.
        assert_eq!(prepared.warnings.iter().map(|w| w.line).collect::<Vec<_>>(), [Some(3), Some(5), Some(10)]);
        assert_eq!(prepared.shown()["warnings"][0]["line"], 3);
        assert_eq!(prepared.warnings[0].words, "README.md line 3 has HTML; the store shows it as text. Use Markdown for pictures and headings.");
        assert_eq!(lines(&prepared.warnings)[1], "footnote (README.md): README.md line 5 has a footnote; the store shows it as text. Write the note into the text instead.");
        // The listed plugin as it was: nothing to say.
        assert_eq!(read(folder(&listed().0, &listed().1).path()).unwrap().warnings.len(), 0);
    }
}
