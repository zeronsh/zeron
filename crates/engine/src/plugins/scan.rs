//! One plugin folder → what it is (manifest, files, hash, signature) and the
//! list item the app sees. No network: authors come in resolved.

use std::path::{Path, PathBuf};

use serde::Serialize;

use super::capability;
use super::files::hash_files;
use super::manifest::{MANIFEST_FILE, Manifest, McpSpec, ToolSpec, UiSpec, check_id};
use super::signing::{FileHashes, SigCheck, check, content_hash};
use super::status::{Author, Decision, Facts, State, Status, Verdict, decide};

pub const UNSIGNED: &str = "It isn't signed";
pub const FILES_DIFFER: &str = "Its files don't match its signature";
pub const BAD_SIGNATURE: &str = "Its signature doesn't check out";

/// A plugin folder, read and hashed.
#[derive(Debug, Clone)]
pub struct Inspected {
    pub dir: PathBuf,
    /// The folder's name (the id for a folder under `<data>/plugins/`).
    pub folder: String,
    pub manifest: Option<Manifest>,
    pub manifest_bytes: Vec<u8>,
    pub files: FileHashes,
    /// Empty when it can't be computed.
    pub hash: String,
    pub sig: SigCheck,
    pub error: Option<String>,
}

impl Inspected {
    /// The signing key, when the signature checks out.
    pub fn key(&self) -> Option<&str> {
        match &self.sig {
            SigCheck::Valid(key) => Some(key),
            _ => None,
        }
    }

    pub fn id(&self) -> &str {
        self.manifest
            .as_ref()
            .map(|m| m.id.as_str())
            .filter(|id| !id.is_empty())
            .unwrap_or(&self.folder)
    }

    /// Who made it, as far as the signature alone tells.
    /// `Err(key)` = it checks out and `key` needs looking up.
    pub fn signature_author(&self) -> Result<Author, String> {
        match &self.sig {
            SigCheck::Unsigned => Ok(Author::unknown(UNSIGNED)),
            SigCheck::FilesDiffer => Ok(Author::unknown(FILES_DIFFER)),
            SigCheck::BadSignature => Ok(Author::unknown(BAD_SIGNATURE)),
            SigCheck::Valid(key) => Err(key.clone()),
        }
    }
}

/// Read `dir`. `plugin_id`: the folder sits under `<data>/plugins/` and its
/// name must be a valid id that matches the manifest's.
pub fn inspect(dir: &Path, under_plugins: bool) -> Inspected {
    let folder = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut inspected = Inspected {
        dir: dir.to_path_buf(),
        folder: folder.clone(),
        manifest: None,
        manifest_bytes: Vec::new(),
        files: FileHashes::new(),
        hash: String::new(),
        sig: SigCheck::Unsigned,
        error: None,
    };
    let fail = |mut inspected: Inspected, error: String| {
        inspected.error = Some(error);
        inspected
    };
    if under_plugins && let Err(error) = check_id(&folder) {
        return fail(inspected, error);
    }
    let manifest_path = dir.join(MANIFEST_FILE);
    match std::fs::symlink_metadata(&manifest_path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return fail(inspected, super::files::LINK_ERROR.into());
        }
        Ok(meta) if meta.is_file() => {}
        _ => return fail(inspected, "It has no zeron-plugin.json".into()),
    }
    let Ok(bytes) = std::fs::read(&manifest_path) else {
        return fail(inspected, "Its zeron-plugin.json can't be read".into());
    };
    inspected.manifest_bytes = bytes;
    let manifest = match Manifest::parse(&inspected.manifest_bytes) {
        Ok(manifest) => manifest,
        Err(error) => return fail(inspected, error),
    };
    let problem = manifest.problem().map(str::to_owned);
    let id_mismatch = under_plugins && !manifest.id.is_empty() && manifest.id != folder;
    let reserved = check_id(&manifest.id)
        .err()
        .filter(|_| !manifest.id.is_empty());
    inspected.manifest = Some(manifest);
    if let Some(problem) = problem {
        return fail(inspected, problem);
    }
    if let Some(error) = reserved {
        return fail(inspected, error);
    }
    if id_mismatch {
        return fail(
            inspected,
            format!("Its id doesn't match its folder's name ({folder})"),
        );
    }
    match hash_files(dir) {
        Ok(files) => inspected.files = files,
        Err(error) => return fail(inspected, error),
    }
    let manifest = inspected.manifest.as_ref().expect("parsed above");
    inspected.hash = content_hash(manifest, &inspected.files);
    inspected.sig = check(manifest, &inspected.files);
    if !inspected.files.contains_key(&manifest.entry) {
        let entry = manifest.entry.clone();
        return fail(inspected, format!("Its entry file {entry} is missing"));
    }
    inspected
}

/// What lists carry (contract "PluginItem").
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PluginItem {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub dir: String,
    pub entry: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<String>,
    pub hash: String,
    pub author: Author,
    pub own: bool,
    pub status: Status,
    pub permissions: Vec<String>,
    pub allowed: Vec<String>,
    pub ask: Vec<String>,
    pub ui: UiSpec,
    pub tools: Vec<ToolSpec>,
    /// The programs it runs (`commands`), named on its consent and checked
    /// by the app on each run.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<String>,
    /// The hosts its requests may reach (`network.hosts`), named on its
    /// consent; a private one (`localhost:8080`) said as such.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub network_hosts: Vec<String>,
    /// Its service's file (`service.luau`), run headless by the engine
    /// (plugins/service) while it's on and allowed `service`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    pub mcp: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// One of Zeron's own (shipped inside the app): allowed what it asks as
    /// it ships, on unless turned off, never removed or edited.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub builtin: bool,
    /// Where someone else's came from (`{kind, url?, ref?}`, install.rs).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<serde_json::Value>,
    /// A new version found at its source (`{version, added, removed,
    /// refused?}`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update: Option<serde_json::Value>,
    /// Its icon (the manifest's `icon`, an SVG in its folder), as text: the
    /// listing carries it so another device's window can draw it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_svg: Option<String>,
    /// The digest of its capability record (`capability.rs`): what a
    /// decision binds.
    pub digest: String,
    /// What grew since you allowed it, when that's why it waits
    /// (`capability::Diff`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub digest_diff: Option<serde_json::Value>,
}

/// The SVG a manifest names as its icon (`"icon": "icon.svg"`), read
/// whole: none if it names none, or one outside the folder, over 32 KB, or
/// not an SVG.
fn icon_svg(dir: &Path, manifest: &serde_json::Value) -> Option<String> {
    const MAX: u64 = 32 * 1024;
    let rel = manifest.get("icon")?.as_str()?;
    if !rel.ends_with(".svg") {
        return None;
    }
    let path = zeron_plugin_guard::path::join(dir, rel).ok()?;
    let meta = std::fs::symlink_metadata(&path).ok()?;
    if !meta.is_file() || meta.len() > MAX {
        return None;
    }
    let text = std::fs::read_to_string(&path).ok()?;
    text.trim_start().starts_with("<svg").then_some(text)
}

/// A scanned plugin: the item plus what the engine keeps to itself.
#[derive(Debug, Clone)]
pub struct Entry {
    pub item: PluginItem,
    pub inspected: Inspected,
    pub verdict: Verdict,
    /// Its capability record.
    pub record: serde_json::Value,
}

impl Entry {
    pub fn key(&self) -> Option<&str> {
        self.inspected.key()
    }

    pub fn mcp(&self) -> Option<&McpSpec> {
        self.inspected
            .manifest
            .as_ref()
            .and_then(|m| m.mcp.as_ref())
    }

    pub fn allows(&self, permission: &str) -> bool {
        self.item.status == Status::On && self.item.allowed.iter().any(|p| p == permission)
    }

    pub fn facts(&self) -> Facts<'_> {
        Facts {
            broken: self.item.status == Status::Broken,
            asked: &self.item.permissions,
            author: &self.item.author,
            key: self.key(),
            hash: &self.item.hash,
            record: &self.record,
        }
    }
}

/// Build the entry for an inspected folder whose author is resolved.
pub fn entry(inspected: Inspected, author: Author, state: &State) -> Entry {
    // Keyed by folder: a manifest whose id differs is broken anyway.
    let id = inspected.folder.clone();
    let own = state.own.contains(&id);
    let decision: Option<&Decision> = state.decisions.get(&id);
    let manifest = inspected.manifest.as_ref();
    let permissions: Vec<String> = manifest.map(|m| m.permissions.clone()).unwrap_or_default();
    let record = manifest.map_or_else(capability::empty, capability::record);
    let verdict = decide(
        &Facts {
            broken: inspected.error.is_some(),
            asked: &permissions,
            author: &author,
            key: inspected.key(),
            hash: &inspected.hash,
            record: &record,
        },
        decision,
    );
    let item = PluginItem {
        id: id.clone(),
        name: manifest
            .map(|m| m.name.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| id.clone()),
        version: manifest.map(|m| m.version.clone()).unwrap_or_default(),
        description: manifest.map(|m| m.description.clone()).unwrap_or_default(),
        dir: inspected.dir.display().to_string(),
        entry: manifest.map(|m| m.entry.clone()).unwrap_or_default(),
        snapshot: None,
        hash: inspected.hash.clone(),
        author,
        own,
        status: verdict.status,
        permissions,
        allowed: verdict.allowed.clone(),
        ask: verdict.ask.clone(),
        ui: manifest.map(|m| m.ui.clone()).unwrap_or_default(),
        tools: manifest.map(|m| m.tools.clone()).unwrap_or_default(),
        commands: manifest.map(|m| m.commands.clone()).unwrap_or_default(),
        network_hosts: manifest.map(|m| m.network_hosts.clone()).unwrap_or_default(),
        service: manifest.and_then(|m| m.service.clone()),
        mcp: manifest.is_some_and(|m| m.mcp.is_some()),
        error: inspected.error.clone(),
        builtin: false,
        source: None,
        update: None,
        icon_svg: manifest.and_then(|m| icon_svg(&inspected.dir, &m.raw)),
        digest: capability::digest(&record),
        digest_diff: verdict.diff.clone(),
    };
    Entry {
        item,
        inspected,
        verdict,
        record,
    }
}

/// One of Zeron's own plugins, from the app's folder of them: allowed
/// exactly what its manifest asks (a new version's asks come with it), on
/// unless you turned it off (`off`), its author Zeron.
pub fn builtin_entry(inspected: Inspected, off: bool) -> Entry {
    let mut entry = entry(inspected, Author::Zeron, &State::default());
    entry.item.builtin = true;
    entry.item.own = false;
    if entry.item.status != Status::Broken {
        entry.item.status = if off { Status::Off } else { Status::On };
        entry.item.allowed = entry.item.permissions.clone();
        entry.item.allowed.sort();
        entry.item.ask = Vec::new();
        entry.item.digest_diff = None;
    }
    entry
}

/// The plugin folders under `<data>/plugins/` (dot entries are not plugins).
pub fn plugin_dirs(plugins_dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(plugins_dir) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .filter(|e| {
            // A folder, or a link (reported broken), not a stray file.
            std::fs::symlink_metadata(e.path())
                .map(|m| m.is_dir() || m.file_type().is_symlink())
                .unwrap_or(false)
        })
        .map(|e| e.path())
        .collect();
    dirs.sort();
    dirs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::signing::SigningKey;

    fn write_plugin(root: &Path, id: &str, manifest: serde_json::Value) -> PathBuf {
        let dir = root.join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("init.luau"), "return 1").unwrap();
        std::fs::write(
            dir.join(MANIFEST_FILE),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        dir
    }

    fn manifest(id: &str) -> serde_json::Value {
        serde_json::json!({ "id": id, "name": "Focus", "version": "1.0.0", "entry": "init.luau" })
    }

    #[test]
    fn plugins_inspect_reports_broken_folders() {
        let root = tempfile::tempdir().unwrap();
        let ok = inspect(&write_plugin(root.path(), "focus", manifest("focus")), true);
        assert_eq!(ok.error, None);
        assert_eq!(ok.sig, SigCheck::Unsigned);
        assert_eq!(ok.hash.len(), 64);

        let mismatch = inspect(&write_plugin(root.path(), "other", manifest("focus")), true);
        assert!(mismatch.error.unwrap().contains("doesn't match"));

        let reserved = inspect(&write_plugin(root.path(), "zeron", manifest("zeron")), true);
        assert_eq!(reserved.error.as_deref(), Some("Zeron already uses that name"));

        let mut no_entry = manifest("noentry");
        no_entry["entry"] = serde_json::json!("main.luau");
        let missing = inspect(&write_plugin(root.path(), "noentry", no_entry), true);
        assert!(missing.error.unwrap().contains("main.luau"));

        std::fs::create_dir_all(root.path().join("empty")).unwrap();
        assert!(inspect(&root.path().join("empty"), true).error.is_some());

        std::fs::create_dir_all(root.path().join(".loaded")).unwrap();
        std::fs::write(root.path().join("stray.txt"), "x").unwrap();
        let names: Vec<String> = plugin_dirs(root.path())
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["empty", "focus", "noentry", "other", "zeron"]);
    }

    #[test]
    fn plugins_entry_for_signed_and_tampered() {
        let root = tempfile::tempdir().unwrap();
        let dir = write_plugin(root.path(), "focus", manifest("focus"));
        let (key, _) = SigningKey::generate().unwrap();
        let first = inspect(&dir, true);
        let signed = key.sign_manifest(&first.manifest.as_ref().unwrap().raw, &first.files);
        std::fs::write(
            dir.join(MANIFEST_FILE),
            serde_json::to_vec(&signed).unwrap(),
        )
        .unwrap();
        let again = inspect(&dir, true);
        assert_eq!(again.sig, SigCheck::Valid(key.public()));
        assert_eq!(
            again.hash, first.hash,
            "signing doesn't change the content hash"
        );
        assert_eq!(again.signature_author(), Err(key.public()));

        std::fs::write(dir.join("extra.luau"), "x").unwrap();
        let tampered = inspect(&dir, true);
        assert_eq!(tampered.sig, SigCheck::FilesDiffer);
        assert_eq!(
            tampered.signature_author(),
            Ok(Author::unknown(FILES_DIFFER))
        );
        let entry = entry(tampered, Author::unknown(FILES_DIFFER), &State::default());
        assert_eq!(entry.item.status, Status::Pending);
        assert_eq!(entry.item.name, "Focus");
    }
}
