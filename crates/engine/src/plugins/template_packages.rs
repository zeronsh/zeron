//! Sharing templates: a template as one signed file (`<id>.zeron-template`),
//! and that file installed on another device.
//!
//! A package is JSON: `{format, manifest, files: {rel: base64}}`. Its
//! manifest (`id, name, description, version, zeron_version`) is signed as a
//! plugin's is (`signing`), with this device's key, in the template domain
//! (`zeron-template:v1\n`: a plugin's signature never verifies as a
//! template's, nor the reverse), over each file's sha256.
//!
//! Importing reads the whole package and checks its signature over every
//! file in it before anything is written: unsigned, tampered or malformed,
//! it's refused. The files no template may replace (`PROTECTED`: the plugins
//! and templates pages, your account, the app's `.luaurc`) are then left out
//! and named. Yours (signed by this device's key) installs at once; anyone
//! else's waits in `.pending/` until you allow it in Zeron's own consent
//! window (`decide`), which checks it again. A template runs as the app's own
//! code, with all its powers: that's what the window asks you to allow.
//!
//! An installed one keeps its signed manifest and `.imported.json`
//! (`{key, author, at, ignored, hash, version, source?}`): who signed it, as
//! Zeron knew them then, the package it was, and where it came from.
//!
//! A package comes from a file, or a link (`origin`): one to a
//! `.zeron-template`, or a GitHub repository's release carrying one (its
//! latest, or a tag). Where it came from is checked for a new version as a
//! plugin's is (`check`, by the engine every few hours and as Settings →
//! Templates opens): a newer package signed by the same key is kept in
//! `.updates/` until it's installed in place (`update`), with no asking
//! again; one signed by anyone else is named, refused, and never kept.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::files::{MAX_BYTES, MAX_FILES};
use super::install::{self, Found, Origin, Source};
use super::listing;
use super::package::{self, MAX_PACKAGE_BYTES};
use super::signing::{self, FileHashes, SigningKey, TEMPLATE_PAYLOAD_PREFIX};
use super::status::Author;
use super::templates::{self, MANIFEST, PREVIEW, ZERON, inside, valid_id};
use super::version;

pub const FORMAT: &str = "zeron-template:v1";
pub const EXTENSION: &str = "zeron-template";
pub const PENDING: &str = ".pending";
pub const IMPORTED: &str = ".imported.json";
/// Updates found, waiting to be installed (`<id>.json`, `<id>.zeron-template`).
pub const UPDATES: &str = ".updates";
/// The app's mark of one of Zeron's built-in themes (apps/zgx
/// theme_templates `MARK`): never written from a package.
const BUILTIN_THEME: &str = "builtin_theme";
/// And of one of its official templates (apps/zgx `official::MARK`).
const BUILTIN_TEMPLATE: &str = "builtin_template";

/// What no template replaces: the app always loads Zeron's own. One answer
/// with the app's.
pub use zeron_plugin_guard::template::{never_templated, protected, short_name, unseen};

const UNSIGNED: &str = "This template isn't signed, so Zeron can't tell who made it or whether it was changed. Ask its author to export it again from Zeron.";
const TAMPERED: &str = "This template was changed after it was signed, so Zeron won't install it. Ask its author for a fresh copy.";
const BAD_SIGNATURE: &str = "This template's signature doesn't check out, so Zeron won't install it. Ask its author for a fresh copy.";
const NOT_A_PACKAGE: &str = "This isn't a Zeron template. Templates are shared as .zeron-template files, exported from Zeron.";
const TOO_BIG: &str = "This template is too big to install.";
const BAD_FILE_START: &str = "This template has a file Zeron won't write";
const STORE_MISMATCH: &str = "This template is signed, but the store's record names no signer, so it wasn't installed.";
const BAD_ID: &str = "This template's id can't be used";
const CLASH: &str = "This template can't be installed";

/// A template package's words for a refusal (`package::parse`).
const WORDS: package::Words = package::Words {
    not_a_package: NOT_A_PACKAGE,
    unsigned: UNSIGNED,
    tampered: TAMPERED,
    bad_signature: BAD_SIGNATURE,
    too_big: TOO_BIG,
    bad_file: "This template has a file Zeron won't write: {rel}.",
};

/// A package, read and verified.
#[derive(Debug)]
pub struct Package {
    /// Its manifest, signed.
    pub manifest: Value,
    /// The key that signed it (b64url); none for one the store served
    /// unsigned ([`parse_from_store`]).
    pub key: Option<String>,
    /// The files it installs: its app files and its preview.
    pub files: BTreeMap<String, Vec<u8>>,
    /// Files it carried that no template may replace, left out.
    pub ignored: Vec<String>,
    /// Their sha256s (its content hash is over them too).
    left_out: FileHashes,
    /// sha256 of the package file as read: what you're shown is what's
    /// installed.
    pub hash: String,
    /// Its content hash (`signing::content_hash_of` over its manifest
    /// without the signature and every file it carries): what a store
    /// record's `hash` names.
    pub content_hash: String,
    raw: Vec<u8>,
}

impl Package {
    fn text(&self, key: &str) -> String {
        self.manifest[key].as_str().unwrap_or("").trim().to_owned()
    }

    pub fn id(&self) -> String {
        self.text("id")
    }

    pub fn version(&self) -> String {
        self.text("version")
    }

    pub fn name(&self) -> String {
        let name = self.text("name");
        if name.is_empty() { self.id() } else { name }
    }

    /// Who made it, as `author_of` knows its signer. One the store served
    /// unsigned has no signer to know (the store's record names its
    /// publisher).
    fn author(&self, author_of: impl Fn(&str) -> Author) -> Author {
        match &self.key {
            Some(key) => author_of(key),
            None => Author::unknown("Not signed"),
        }
    }

    /// Whether a template from the store is for this Zeron, by its signed
    /// `zeron_version` (the Zeron it was made from): that or later, the
    /// `engines.zeron` the store gives it. `Err` is the `incompatible`
    /// failure saying why not; one that doesn't say is for none.
    pub fn compatible(&self) -> Result<(), String> {
        let range = listing::Listing::of_template(&self.manifest)
            .engines_zeron
            .ok_or_else(|| install::failure(install::reason::INCOMPATIBLE, "It doesn't say which Zeron it was made from"))?;
        super::version::accepts(Some(&range), &super::version::zeron()).map_err(|e| install::failure(install::reason::INCOMPATIBLE, e))
    }

    /// What the consent window and the tools show of it: its words bounded
    /// as the store bounds them, wherever it came from.
    fn about(&self) -> Value {
        json!({
            "id": self.id(),
            "name": listing::shown(&self.name(), listing::MAX_NAME_LEN),
            "description": listing::shown(&self.text("description"), listing::MAX_DESCRIPTION_LEN),
            "version": listing::shown(&self.text("version"), version::MAX_LEN),
            "zeron_version": listing::shown(&self.text("zeron_version"), version::MAX_LEN),
            "key": self.key,
            "files": self.files.len(),
            "ignored": self.ignored.iter().map(|rel| listing::shown(rel, MAX_SHOWN_PATH)).collect::<Vec<_>>(),
            "hash": self.hash,
        })
    }

    /// What the app keeps it by once you allow it, over the files it
    /// installs (`zeron_plugin_guard::template::digest`).
    fn digest(&self) -> String {
        zeron_plugin_guard::template::digest(&self.files)
    }
}

/// Held while a package is put where a template is, from reading what's
/// there: one allowed as another's updated never lands over what it wasn't
/// allowed over. (File work alone: nothing waits on the network under it.)
pub(in crate::plugins) fn changing() -> std::sync::MutexGuard<'static, ()> {
    static CHANGING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    CHANGING.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `ExportTemplate`: template `id` as a signed package at `out` (a
/// `.zeron-template` file outside the templates folder; replaced if there).
/// `{id, path, files, key}`.
pub fn export(dir: &Path, id: &str, out: &Path, key: &SigningKey) -> Result<Value, String> {
    if id == ZERON {
        return Err("Everyone already has the default interface. Export one of your own templates.".into());
    }
    let folder = templates::template_dir(dir, id)
        .ok_or_else(|| format!("There's no template {id}. list_templates names them."))?;
    if !out.is_absolute() || out.extension().and_then(|e| e.to_str()) != Some(EXTENSION) {
        return Err(format!("Say where to save it: a full path ending in .{EXTENSION}."));
    }
    let parent = out.parent().filter(|p| p.is_dir()).ok_or("That folder can't be found.")?;
    if let (Ok(parent), Ok(root)) = (std::fs::canonicalize(parent), std::fs::canonicalize(dir))
        && parent.starts_with(&root)
    {
        return Err("Save it outside Zeron's templates folder.".into());
    }
    if std::fs::symlink_metadata(out).is_ok_and(|m| !m.is_file()) {
        return Err("There's something at that path that isn't a file.".into());
    }

    super::template_blocks::refuse(dir, &folder, "exported")?;
    let (manifest, files) = contents(&folder, id)?;
    let bytes = pack(&manifest, &files, Some(key))?;
    let file = out.file_name().and_then(|n| n.to_str()).ok_or("Say where to save it.")?;
    templates::write_atomic(parent, file, &bytes)?;
    Ok(json!({
        "id": id,
        "path": out.display().to_string(),
        "files": files.len(),
        "key": key.public(),
    }))
}

/// What template folder `folder` is packed as, as `id`: its manifest
/// (unsigned) and every file its package carries. Its interface (the files
/// no template replaces, and those whose names could be another's, left
/// out), and its listing files: its preview, its
/// `README.md` (never the one Zeron wrote, which names a folder of its
/// author's), its `CHANGELOG.md` and its `screenshots/`.
pub fn contents(folder: &Path, id: &str) -> Result<(Value, BTreeMap<String, Vec<u8>>), String> {
    let mut files = BTreeMap::new();
    let mut total = 0u64;
    let mut rels = templates::own_files(folder);
    rels.retain(|rel| !protected(rel));
    rels.extend(templates::listing_files(folder));
    rels.retain(|rel| !short_name(rel) && !unseen(rel));
    for rel in rels {
        let path = inside(folder, &rel).ok_or_else(|| format!("{rel} isn't an app file"))?;
        let bytes = templates::read_regular(&path).ok_or_else(|| format!("Couldn't read {rel}"))?;
        if rel == listing::README && std::str::from_utf8(&bytes).is_ok_and(|t| t.contains(listing::TEMPLATE_README_STUB)) {
            continue;
        }
        total += bytes.len() as u64;
        files.insert(rel, bytes);
    }
    if files.len() > MAX_FILES || total > MAX_BYTES {
        return Err(format!(
            "This template is too big to share: at most {MAX_FILES} files and {} MB.",
            MAX_BYTES / 1024 / 1024
        ));
    }
    // Its description, fresh: never a signature or an author it carried
    // (who made it is whoever's key signs it, or the store's publisher).
    let own: Value = templates::read_regular(&folder.join(MANIFEST))
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let field = |k: &str| own[k].as_str().unwrap_or("").trim().to_owned();
    let name = if field("name").is_empty() { id.to_owned() } else { field("name") };
    let mut manifest = json!({
        "id": id,
        "name": name,
        "description": field("description"),
        "version": field("version"),
        "zeron_version": field("zeron_version"),
    });
    for key in listing::TEMPLATE_OPTIONAL {
        if let Some(value) = own.get(*key).filter(|v| !v.is_null()) {
            manifest[*key] = value.clone();
        }
    }
    Ok((manifest, files))
}

/// What a template's folder has and its package doesn't carry.
pub struct LeftOut {
    /// Each file, and why.
    pub files: Vec<(String, &'static str)>,
    /// How many of Zeron's own files there are as the template was made with
    /// them, which no template replaces (and no one it's sent to lacks).
    pub zeron_s: usize,
    /// Each member of its manifest that isn't its signature or its author.
    pub members: Vec<String>,
}

/// What template folder `folder` has that its package (`files`) doesn't.
pub fn left_out(folder: &Path, files: &BTreeMap<String, Vec<u8>>) -> LeftOut {
    const MOST: usize = 200;
    let base: BTreeMap<String, String> = templates::read_regular(&folder.join(templates::BASE)).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let as_made = |rel: &String| protected(rel) && base.get(rel).is_some_and(|hash| templates::read_regular(&folder.join(rel)).is_some_and(|b| signing::sha256_hex(&b) == *hash));
    let (zeron_s, theirs): (Vec<String>, Vec<String>) = templates::app_files(folder, &[MANIFEST, templates::BASE]).into_iter().filter(|rel| !files.contains_key(rel)).partition(as_made);
    let mut left: Vec<(String, &'static str)> = theirs
        .into_iter()
        .map(|rel| {
            let why = if protected(&rel) {
                "it's one of Zeron's own files no template replaces"
            } else if rel == listing::README {
                "it's the README Zeron wrote: write your own"
            } else {
                "its name could be another file's on some disk"
            };
            (listing::shown(&rel, MAX_SHOWN_PATH), why)
        })
        .collect();
    left.extend(templates::others(folder, MOST).into_iter().map(|rel| (listing::shown(&rel, MAX_SHOWN_PATH), "its name, or what it is, isn't one Zeron writes")));
    left.truncate(MOST);
    let own: Value = templates::read_regular(&folder.join(MANIFEST)).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let carried = [listing::TEMPLATE_REQUIRED, listing::TEMPLATE_OPTIONAL, &["author", "signature"]];
    let members = own.as_object().into_iter().flatten().map(|(key, _)| key.clone()).filter(|key| !carried.iter().any(|list| list.contains(&key.as_str()))).take(MOST).collect();
    LeftOut { files: left, zeron_s: zeron_s.len(), members }
}

/// `manifest` and `files` as a package's bytes: signed by `key` in the
/// template domain, or unsigned (a deploy token's).
pub fn pack(manifest: &Value, files: &BTreeMap<String, Vec<u8>>, key: Option<&SigningKey>) -> Result<Vec<u8>, String> {
    let signed = match key {
        Some(key) => key.sign_manifest_for(TEMPLATE_PAYLOAD_PREFIX, manifest, &package::hashes(files)),
        None => manifest.clone(),
    };
    package::pack(FORMAT, &signed, files)
}

/// Where Zeron saves an export when it isn't told where:
/// `<data>/template-exports/<id>.zeron-template`.
pub fn default_export_path(data_dir: &Path, id: &str) -> PathBuf {
    data_dir.join("template-exports").join(format!("{id}.{EXTENSION}"))
}

/// A path someone named for an export (R29): never inside Zeron's own data,
/// never in a hidden folder of your home (`~/.ssh`, `~/.config`), and never
/// over a file that isn't a template package (an earlier export is
/// replaced; nothing else is).
pub fn check_export_path(out: &Path, data_dir: &Path, home: Option<&Path>) -> Result<(), String> {
    let refuse = || Err("A template can only be saved in a folder of yours: not a hidden folder, not one of Zeron's, and not over a different kind of file.".to_string());
    let parent = out.parent().ok_or("Say where to save it.")?;
    let canonical = std::fs::canonicalize(parent).map_err(|_| "That folder can't be found.".to_string())?;
    let data = std::fs::canonicalize(data_dir).unwrap_or_else(|_| data_dir.to_path_buf());
    if canonical.starts_with(&data) {
        return refuse();
    }
    if let Some(home) = home {
        let home = std::fs::canonicalize(home).unwrap_or_else(|_| home.to_path_buf());
        if let Ok(rest) = canonical.strip_prefix(&home)
            && rest.components().any(|c| c.as_os_str().to_string_lossy().starts_with('.'))
        {
            return refuse();
        }
    }
    match std::fs::symlink_metadata(out) {
        Err(_) => Ok(()),
        Ok(meta) if meta.is_file() && meta.len() <= MAX_PACKAGE_BYTES => {
            let bytes = std::fs::read(out).map_err(|_| "That file can't be read.".to_string())?;
            let was = serde_json::from_slice::<Value>(&bytes).ok();
            if was.as_ref().and_then(|v| v["format"].as_str()) == Some(FORMAT) { Ok(()) } else { refuse() }
        }
        Ok(_) => refuse(),
    }
}

/// Reads and verifies the package at `path`: its signature over every file
/// in it, before anything is kept.
pub fn read(path: &Path) -> Result<Package, String> {
    let meta = std::fs::metadata(path).map_err(|_| "That file can't be found.".to_string())?;
    if !meta.is_file() {
        return Err("That isn't a file.".into());
    }
    if meta.len() > MAX_PACKAGE_BYTES {
        return Err("This template is too big to install.".into());
    }
    let raw = std::fs::read(path).map_err(|e| format!("Couldn't read it: {e}"))?;
    parse(raw)
}

pub fn parse(raw: Vec<u8>) -> Result<Package, String> {
    parse_package(raw, true)
}

/// [`parse`] for a package the store serves: `signed` is whether its
/// record names a signer. A version published with a deploy token is
/// unsigned, its record naming none: taken by its contents, which the
/// record's content hash then vouches for. Under a record naming no signer,
/// a signed package is refused: unsigned is taken there, and nothing else.
pub fn parse_from_store(raw: Vec<u8>, signed: bool) -> Result<Package, String> {
    let package = parse_package(raw, signed)?;
    if !signed && package.key.is_some() {
        return Err(STORE_MISMATCH.into());
    }
    Ok(package)
}

fn parse_package(raw: Vec<u8>, signed: bool) -> Result<Package, String> {
    // Every name a plain path inside the template (`templates::inside`: no
    // `..`, no absolute part, no dotfile but `.luaurc`), with no character
    // drawn as nothing (some disks look names up without them).
    let keep = |rel: &str| inside(Path::new("t"), rel).is_some() && !unseen(rel);
    let package::Verified { manifest, key, files: mut decoded, raw, hash } =
        package::parse(raw, FORMAT, TEMPLATE_PAYLOAD_PREFIX, keep, &WORDS, signed)?;
    // Over every file it carries, as its signature is.
    let mut unsigned = manifest.clone();
    if let Some(m) = unsigned.as_object_mut() {
        m.remove("signature");
    }
    let hashes = package::hashes(&decoded);
    let content_hash = signing::content_hash_of(&unsigned, &hashes);
    let id = manifest["id"].as_str().unwrap_or("").trim();
    if !valid_id(id) || id == ZERON {
        return Err(format!("{BAD_ID} ({id:?})."));
    }
    // Checked as signed; then what no template replaces is set aside (a
    // Windows short name may be one of those), and what isn't an app file (a
    // manifest of its own) too.
    let mut ignored = Vec::new();
    decoded.retain(|rel, _| {
        let keep = !protected(rel) && !short_name(rel) && rel != MANIFEST && !listing::template_stray_screenshot(rel);
        if !keep {
            ignored.push(rel.clone());
        }
        keep
    });
    // Of what's written, two paths that are one file on a disk folding
    // names (or a file where another needs a folder of that name; its
    // manifest and `.imported.json` among them): refused before anything is
    // written, on every disk alike, as a plugin's are.
    if let Some(clash) = listing::clashes(decoded.keys().map(String::as_str), &[MANIFEST, IMPORTED]).first() {
        return Err(format!("{CLASH}: {}.", clash.words()));
    }
    let left_out = hashes.into_iter().filter(|(rel, _)| ignored.contains(rel)).collect();
    Ok(Package {
        manifest,
        key,
        files: decoded,
        ignored,
        left_out,
        hash,
        content_hash,
        raw,
    })
}

/// The word for why a package was refused ([`parse`], [`parse_from_store`]):
/// one of `install::reason`'s, as a plugin package's are (`not_a_package`,
/// `too_big`, `bad_file`, `unsigned`, `tampered`, `bad_signature`,
/// `store_mismatch`, `paths_collide`) or `bad_id`.
pub fn refusal(error: &str) -> Option<&'static str> {
    use install::reason;
    let words: [(&str, &'static str); 7] = [
        (NOT_A_PACKAGE, reason::NOT_A_PACKAGE),
        (TOO_BIG, reason::TOO_BIG),
        (UNSIGNED, reason::UNSIGNED),
        (TAMPERED, reason::TAMPERED),
        (BAD_SIGNATURE, reason::BAD_SIGNATURE),
        (STORE_MISMATCH, reason::STORE_MISMATCH),
        (BAD_FILE_START, reason::BAD_FILE),
    ];
    if let Some((_, word)) = words.iter().find(|(words, _)| error.starts_with(words.trim_end_matches('.'))) {
        return Some(word);
    }
    if error.starts_with(BAD_ID) {
        return Some("bad_id");
    }
    error.starts_with(CLASH).then_some(reason::PATHS_COLLIDE)
}

/// A free id for `want` in `dir`: itself, else `want-2`, `want-3`…
fn free_id(dir: &Path, want: &str) -> Result<String, String> {
    let taken = |id: &str| std::fs::symlink_metadata(dir.join(id)).is_ok();
    if !taken(want) {
        return Ok(want.to_owned());
    }
    for n in 2..1000 {
        let suffix = format!("-{n}");
        let stem: String = want.chars().take(40 - suffix.len()).collect();
        let id = format!("{}{suffix}", stem.trim_end_matches('-'));
        if valid_id(&id) && !taken(&id) {
            return Ok(id);
        }
    }
    Err("There are too many templates with this id. Delete some first.".into())
}

/// Writes the package's files as a new template; its id (`want`'s, or a
/// free one beside it).
fn install(dir: &Path, package: &Package, author: &Author, source: Option<&Source>) -> Result<String, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("Couldn't make the templates folder: {e}"))?;
    if source.is_some_and(|s| s.store().is_some()) {
        super::template_blocks::begin(dir).map_err(|e| format!("Couldn't keep what's been taken down from the store: {e}"))?;
    }
    let id = free_id(dir, &package.id())?;
    let tmp = write_folder(dir, &id, package, author, source)?;
    if let Err(error) = std::fs::rename(&tmp, dir.join(&id)) {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(format!("Couldn't put the template in place: {error}"));
    }
    Ok(id)
}

/// The package written as template `id`'s folder, beside the templates
/// (a dot folder, never listed): the caller puts it in place, whole, by a
/// rename, so no one sees it half written. Its files, then its manifest and
/// its record: a file a package carries named `ZERON-~1.JSO` or
/// `IMPORT~1.JSO` is made before either of those, so on a disk with short
/// names it isn't one of them (`zeron_plugin_guard::path`).
fn write_folder(dir: &Path, id: &str, package: &Package, author: &Author, source: Option<&Source>) -> Result<PathBuf, String> {
    let tmp = dir.join(format!(".tmp-import-{id}-{}-{}", std::process::id(), super::now_secs()));
    let _ = std::fs::remove_dir_all(&tmp);
    let written = (|| -> Result<(), String> {
        std::fs::create_dir_all(&tmp).map_err(|e| format!("Couldn't make the template's folder: {e}"))?;
        for (rel, bytes) in &package.files {
            let to = inside(&tmp, rel).ok_or_else(|| format!("{rel} isn't an app file"))?;
            if let Some(parent) = to.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("Couldn't make a folder for {rel}: {e}"))?;
            }
            std::fs::write(&to, bytes).map_err(|e| format!("Couldn't write {rel}: {e}"))?;
        }
        // What marks Zeron's own built-in themes (`builtin_theme`, the
        // app's theme_templates) is the app's to write, never a package's:
        // left out, so an imported template never shows as Zeron's own.
        let mut manifest = package.manifest.clone();
        if let Some(m) = manifest.as_object_mut() {
            m.remove(BUILTIN_THEME);
            m.remove(BUILTIN_TEMPLATE);
        }
        std::fs::write(tmp.join(MANIFEST), signing::pretty(&manifest))
            .map_err(|e| format!("Couldn't write {MANIFEST}: {e}"))?;
        let mut imported = json!({
            "key": package.key,
            "author": author,
            "at": super::now_secs(),
            "ignored": package.ignored,
            "hash": package.hash,
            "version": package.text("version"),
        });
        if let Some(source) = source {
            imported["source"] = json!(source);
            // (What its content hash is over besides what's here: `edited`.)
            if source.store().is_some() && !package.left_out.is_empty() {
                imported["left_out"] = json!(package.left_out);
            }
        }
        std::fs::write(tmp.join(IMPORTED), signing::pretty(&imported))
            .map_err(|e| format!("Couldn't write {IMPORTED}: {e}"))
    })();
    if let Err(error) = written {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(error);
    }
    Ok(tmp)
}

/// `ImportTemplate`: the package at `path`, verified. Yours installs now
/// (`{installed, name, author, ignored}`); anyone else's waits for you in
/// Zeron (`{pending, …about, author, needs_consent: true}`).
pub fn import(dir: &Path, path: &Path, author_of: impl Fn(&str) -> Author) -> Result<Value, String> {
    let package = read(path)?;
    let shown = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let source = Source {
        kind: "file".into(),
        url: None,
        path: Some(shown.display().to_string()),
        git_ref: None,
        commit: None,
        version: package.text("version"),
        installed_at: super::now_secs(),
        store: None,
    };
    let from = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    import_package(dir, package, &from, source, author_of)
}

/// A package read and verified (from a file, or a link: `source`), as
/// `import` does with it: yours installs, anyone else's waits.
pub fn import_package(dir: &Path, package: Package, from: &str, source: Source, author_of: impl Fn(&str) -> Author) -> Result<Value, String> {
    let _held = changing();
    super::template_blocks::refuse_package(dir, &package.content_hash, None)?;
    let author = package.author(&author_of);
    if author == Author::You {
        let id = install(dir, &package, &author, Some(&source))?;
        return Ok(json!({
            "installed": id,
            "name": package.name(),
            "author": author,
            "ignored": package.ignored,
        }));
    }
    let pending = stage(dir, &package, from, &source)?;
    let mut out = package.about();
    out["pending"] = json!(pending);
    out["author"] = json!(author);
    out["needs_consent"] = json!(true);
    Ok(out)
}

/// A waiting package's name: `<id>-<unix>[-n]`.
fn valid_pending(name: &str) -> bool {
    (3..=80).contains(&name.len())
        && name.as_bytes()[0].is_ascii_alphanumeric()
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Keeps the package (as read: it's checked again when allowed) and what
/// to show of it, under `.pending/`.
fn stage(dir: &Path, package: &Package, from: &str, source: &Source) -> Result<String, String> {
    stage_as(dir, package, from, source, None, false)
}

/// `stage`, as an update of installed template `update_of` when it is one
/// (allowed, it replaces that template in place: `decide`).
/// (`instead`: though it's older: `stage_from_store`).
fn stage_as(dir: &Path, package: &Package, from: &str, source: &Source, update_of: Option<&str>, instead: bool) -> Result<String, String> {
    let pending_dir = dir.join(PENDING);
    std::fs::create_dir_all(&pending_dir).map_err(|e| format!("Couldn't keep it for you to allow: {e}"))?;
    let stamp = super::now_secs();
    let mut name = format!("{}-{stamp}", package.id());
    let mut n = 1;
    while std::fs::symlink_metadata(pending_dir.join(format!("{name}.json"))).is_ok() {
        name = format!("{}-{stamp}-{n}", package.id());
        n += 1;
    }
    templates::write_atomic(&pending_dir, &format!("{name}.{EXTENSION}"), &package.raw)?;
    let mut about = package.about();
    about["pending"] = json!(name);
    about["at"] = json!(stamp);
    about["source"] = json!(from);
    // (Where it came from, whole: carried to what's installed. Listed as
    // `from`, never with a local path.)
    about["origin"] = json!(source);
    if let Some(id) = update_of {
        about["update_of"] = json!(id);
    }
    if instead {
        about["instead"] = json!(true);
    }
    templates::write_atomic(&pending_dir, &format!("{name}.json"), &signing::pretty(&about))?;
    Ok(name)
}

/// The packages waiting for you, oldest first: each `{pending, id, name,
/// description, version, zeron_version, key, files, ignored, hash, digest,
/// at, source, from}`, read from the package itself, as it's read again when
/// it's allowed (one that can't be isn't listed: it can't be allowed); never
/// what the file beside it says of it. An update says whose (`update_of`, the
/// folder's id). One from the store says its record's (`store`, `author`);
/// for an update, that it has a new publisher (`new_owner`) or is over edits
/// of yours (`replaces_edits`); else, which here have its id from elsewhere
/// (`also_installed`). The caller adds who signed any other.
pub fn pending(dir: &Path) -> Vec<Value> {
    let pending_dir = dir.join(PENDING);
    let Ok(entries) = std::fs::read_dir(&pending_dir) else {
        return Vec::new();
    };
    let mut out: Vec<Value> = entries
        .flatten()
        .filter_map(|e| {
            let file = e.file_name().into_string().ok()?;
            let name = file.strip_suffix(".json")?;
            if !valid_pending(name) || !templates::is_regular_file(&pending_dir.join(format!("{name}.{EXTENSION}"))) {
                return None;
            }
            let about: Value = serde_json::from_slice(&templates::read_regular(&e.path())?).ok()?;
            (about["pending"].as_str() == Some(name)).then(|| waiting(dir, name, &about)).flatten()
        })
        .collect();
    out.sort_by_key(|v| (v["at"].as_i64().unwrap_or(0), v["pending"].as_str().unwrap_or("").to_owned()));
    out
}

/// `value` as where a package came from, with no store's record but one
/// from the store.
fn came_from(value: &Value) -> Option<Source> {
    let mut source: Source = serde_json::from_value(value.clone()).ok()?;
    if source.kind != install::STORE {
        source.store = None;
    }
    Some(source)
}

/// `source` with its words bounded, to be shown.
fn bounded(source: &Source) -> Source {
    let mut source = source.clone();
    source.kind = listing::shown(&source.kind, 16);
    for (text, max) in [(&mut source.url, MAX_SHOWN_PATH), (&mut source.git_ref, MAX_SHOWN_PATH)] {
        if let Some(text) = text {
            *text = listing::shown(text, max);
        }
    }
    if let Some(store) = &mut source.store {
        store.publisher = listing::shown(&store.publisher, MAX_SHOWN_PUBLISHER);
        store.id = listing::shown(&store.id, 40);
        store.publisher_id = listing::shown(&store.publisher_id, MAX_SHOWN_PUBLISHER);
        if let Some(other) = &mut store.verified_alternative {
            other.version = listing::shown(&other.version, version::MAX_LEN);
            other.notes = other.notes.chars().take(listing::MAX_NOTES_BYTES).collect();
            // (A template asks for nothing.)
            (other.added, other.removed) = (Vec::new(), Vec::new());
        }
        if let Some(replaced) = &mut store.replaced {
            replaced.version = listing::shown(&replaced.version, version::MAX_LEN);
        }
    }
    source
}

/// One waiting package, as [`pending`] lists it.
fn waiting(dir: &Path, name: &str, about: &Value) -> Option<Value> {
    waiting_of(dir, name, about, package::read(&dir.join(PENDING).join(format!("{name}.{EXTENSION}")), &WORDS).ok()?)
}

/// [`waiting`], its package's bytes read.
fn waiting_of(dir: &Path, name: &str, about: &Value, raw: Vec<u8>) -> Option<Value> {
    let source = came_from(&about["origin"]);
    let store = source.as_ref().and_then(Source::store);
    let package = read_waiting(raw, about, store).ok().filter(|p| about["key"] == json!(p.key))?;
    let mut out = package.about();
    out["pending"] = json!(name);
    out["digest"] = json!(package.digest());
    out["at"] = json!(about["at"].as_i64().unwrap_or(0));
    out["source"] = json!(listing::shown(about["source"].as_str().unwrap_or(""), MAX_SHOWN_PATH));
    let shown_source = source.as_ref().map(bounded);
    if let Some(source) = &shown_source {
        out["from"] = source.shown();
    }
    let update_of = match &about["update_of"] {
        Value::Null => None,
        id => Some(id.as_str().filter(|id| valid_id(id))?),
    };
    if let Some(id) = update_of {
        out["update_of"] = json!(id);
    }
    let (Some(store), Some(drawn)) = (store, shown_source.as_ref().and_then(Source::store)) else { return Some(out) };
    out["store"] = json!({
        "id": drawn.id,
        "version": out["version"],
        "publisher": drawn.publisher,
        "publisher_id": drawn.publisher_id,
        "verified": store.verified,
        "verified_alternative": drawn.verified_alternative.as_ref().filter(|_| !store.verified),
    });
    if store.verified && let Some(at) = store.verified_at {
        out["store"]["verified_at"] = json!(at);
    }
    out["author"] = json!(super::scan::store_author(drawn));
    if let Some(of) = update_of {
        // (One whose template is gone can't be allowed: only declined.)
        let Some(folder) = templates::template_dir(dir, of) else { return Some(out) };
        if self::source(&folder).and_then(|s| s.store().map(|was| was.publisher_id != store.publisher_id)) == Some(true) {
            out["new_owner"] = json!(true);
        }
        if edited(&folder) == Some(true) {
            out["replaces_edits"] = json!(true);
        }
        // (An older, verified one over what's here, by the person's choice.)
        if about["instead"] == true {
            out["instead"] = json!(true);
        }
    } else {
        // (Of its id, from anywhere: it comes in beside them.)
        let others: Vec<Value> = templates::installed(dir)
            .into_iter()
            .filter_map(|id| {
                let folder = dir.join(&id);
                let imported = imported(&folder)?;
                let manifest: Value = serde_json::from_slice(&templates::read_regular(&folder.join(MANIFEST))?).ok()?;
                (manifest["id"].as_str().map(str::trim) == Some(store.id.as_str())).then(|| {
                    let from = came_from(&imported["source"]).map_or_else(|| json!({ "kind": "file" }), |s| bounded(&s).shown());
                    json!({ "id": id, "from": from })
                })
            })
            .collect();
        if !others.is_empty() {
            out["also_installed"] = json!(others);
        }
    }
    Some(out)
}

/// A record's publisher, as it's shown.
pub fn shown_publisher(store: &install::StoreSource) -> String {
    listing::shown(&store.publisher, MAX_SHOWN_PUBLISHER)
}

/// Each template here from the store: its folder, and its record.
pub fn from_store(dir: &Path) -> Vec<(String, install::StoreSource)> {
    templates::installed(dir)
        .into_iter()
        .filter_map(|id| {
            let store = came_from(&imported(&dir.join(&id))?["source"])?.store?;
            Some((id, store))
        })
        .collect()
}

/// The most of a path or a link, and of a publisher's name, that's shown.
const MAX_SHOWN_PATH: usize = 1024;
const MAX_SHOWN_PUBLISHER: usize = 64;

/// Whether the store template in `folder` isn't the store's version any more:
/// what's there (its manifest, its files, and those its package carried that
/// were left out) isn't what its record's content hash is of, or can't be
/// read. `None` for one that isn't from the store. Read and hashed whole.
pub fn edited(folder: &Path) -> Option<bool> {
    edited_by(folder, &mut |path, _| templates::read_regular(path).map(|bytes| (bytes.len() as u64, signing::sha256_hex(&bytes))))
}

/// [`edited`] for a row, asked for at every look: a file that hasn't moved
/// since it was hashed isn't read again ([`Hashed`]). Never what a consent
/// window says.
fn edited_for_row(folder: &Path) -> Option<bool> {
    let mut known = {
        let mut all = hashed();
        all.remove(folder).unwrap_or_default()
    };
    let mut now = BTreeMap::new();
    let edited = edited_by(folder, &mut |path, rel| {
        let at = Stamp::of(path)?;
        let sha256 = match known.remove(rel) {
            Some(was) if was.0 == at => was.1,
            _ => signing::sha256_hex(&templates::read_regular(path)?),
        };
        let len = at.len;
        // (One written just now could be written again and look the same.)
        if at.settled() {
            now.insert(rel.to_owned(), (at, sha256.clone()));
        }
        Some((len, sha256))
    });
    let mut all = hashed();
    if all.len() >= MAX_HASHED {
        all.clear();
    }
    all.insert(folder.to_owned(), now);
    edited
}

/// Each store template's folder's files, as they were hashed for a row.
type Hashed = BTreeMap<PathBuf, BTreeMap<String, (Stamp, String)>>;

/// The most folders kept of them.
const MAX_HASHED: usize = 64;

fn hashed() -> std::sync::MutexGuard<'static, Hashed> {
    static HASHED: std::sync::Mutex<Hashed> = std::sync::Mutex::new(BTreeMap::new());
    HASHED.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// What moves when a file does: which it is, its size, when it was written
/// and (where the system says) when anything of it last changed, which no
/// program sets.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Stamp {
    len: u64,
    written: std::time::SystemTime,
    changed: Option<(i64, i64)>,
    file: (u64, u64),
}

impl Stamp {
    fn of(path: &Path) -> Option<Self> {
        let meta = std::fs::symlink_metadata(path).ok().filter(std::fs::Metadata::is_file)?;
        #[cfg(unix)]
        let (changed, file) = {
            use std::os::unix::fs::MetadataExt as _;
            (Some((meta.ctime(), meta.ctime_nsec())), (meta.dev(), meta.ino()))
        };
        #[cfg(not(unix))]
        let (changed, file) = (None, (0, 0));
        Some(Self { len: meta.len(), written: meta.modified().ok()?, changed, file })
    }

    /// Whether it was last written or changed a while ago: later than any
    /// clock's tick.
    fn settled(&self) -> bool {
        let now = std::time::SystemTime::now();
        let old = |at: std::time::SystemTime| now.duration_since(at).is_ok_and(|d| d > std::time::Duration::from_secs(2));
        let changed = self.changed.is_none_or(|(secs, _)| u64::try_from(secs).is_ok_and(|s| old(std::time::UNIX_EPOCH + std::time::Duration::from_secs(s))));
        old(self.written) && changed
    }
}

/// [`edited`], each file's size and sha256 as `file_hash` (its path, and its
/// path in the template) says.
fn edited_by(folder: &Path, file_hash: &mut dyn FnMut(&Path, &str) -> Option<(u64, String)>) -> Option<bool> {
    let record = imported(folder)?;
    let store = came_from(&record["source"])?.store?;
    let mut hash = || -> Option<String> {
        if templates::has_others(folder) {
            return None;
        }
        let mut manifest: Value = serde_json::from_slice(&templates::read_regular(&folder.join(MANIFEST))?).ok()?;
        manifest.as_object_mut()?.remove("signature");
        let mut hashes: FileHashes = match &record["left_out"] {
            Value::Null => FileHashes::new(),
            left_out => serde_json::from_value(left_out.clone()).ok()?,
        };
        let rels = templates::app_files(folder, &[MANIFEST, templates::BASE]);
        if rels.len() + hashes.len() > MAX_FILES {
            return None;
        }
        let mut total = 0;
        for rel in rels {
            let (len, sha256) = file_hash(&inside(folder, &rel)?, &rel)?;
            total += len;
            if total > MAX_BYTES || hashes.insert(rel, sha256).is_some() {
                return None;
            }
        }
        Some(signing::content_hash_of(&manifest, &hashes))
    };
    Some(hash().as_deref() != Some(store.hash.as_str()))
}

/// What a row says of the record (`.imported.json`) of the template in
/// `folder`: `{key, author, at, hash, version, ignored, source?}` and whether
/// it's edited since (the store's alone). Its source as it's shown, the
/// store's mark only while it's still the store's version; its author, for
/// one from the store, the record's publisher (the caller says who signed any
/// other).
pub fn shown_record(folder: &Path) -> Option<(Value, bool)> {
    let record = imported(folder)?;
    let text = |key: &str, max| listing::shown(record[key].as_str().unwrap_or(""), max);
    let mut out = json!({
        "key": record["key"].as_str(),
        "author": Author::unknown("Not signed"),
        "at": record["at"].as_i64().unwrap_or(0),
        "hash": text("hash", 64),
        "version": text("version", version::MAX_LEN),
        "ignored": record["ignored"].as_array().into_iter().flatten().filter_map(Value::as_str).map(|rel| listing::shown(rel, MAX_SHOWN_PATH)).collect::<Vec<_>>(),
    });
    let mut edited = false;
    if let Some(mut source) = came_from(&record["source"]).as_ref().map(bounded) {
        if let Some(store) = &mut source.store {
            edited = edited_for_row(folder) == Some(true);
            if edited {
                store.verified = false;
            }
            out["author"] = json!(super::scan::store_author(store));
        }
        out["source"] = source.shown();
    }
    Some((out, edited))
}

fn pending_paths(dir: &Path, pending: &str) -> Result<(PathBuf, PathBuf), String> {
    if !valid_pending(pending) {
        return Err("There's no template waiting by that name.".into());
    }
    let at = dir.join(PENDING);
    let (about, package) = (at.join(format!("{pending}.json")), at.join(format!("{pending}.{EXTENSION}")));
    if !templates::is_regular_file(&about) || !templates::is_regular_file(&package) {
        return Err("There's no template waiting by that name. It may have been answered already.".into());
    }
    Ok((about, package))
}

/// A waiting package from the store: the record it was fetched by, and what
/// it is.
pub struct FromStore {
    /// The folder it's an update of, if it's one.
    pub update_of: Option<String>,
    pub store: install::StoreSource,
    pub version: String,
    pub hash: String,
    pub content_hash: String,
}

/// The waiting package `pending`, if it's one from the store (and is still
/// the one its record says; `Err` when it isn't). `None` for any other, or
/// when there's none.
pub fn waiting_from_store(dir: &Path, pending: &str) -> Result<Option<FromStore>, String> {
    let Ok((about_path, package_path)) = pending_paths(dir, pending) else { return Ok(None) };
    let about: Value = templates::read_regular(&about_path).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let Some(store) = came_from(&about["origin"]).and_then(|s| s.store) else { return Ok(None) };
    let package = read_waiting(package::read(&package_path, &WORDS)?, &about, Some(&store))?;
    let update_of = about["update_of"].as_str().map(str::to_owned);
    Ok(Some(FromStore { update_of, version: package.version(), hash: package.hash, content_hash: package.content_hash, store }))
}

/// A package the store served (`source`: the record it was fetched by), to
/// wait for you: never installed here, whosever it is. The one that's waiting
/// already, when it's that very package of that record's. Over an earlier
/// version of it that's here (from the store, under whatever folder's name),
/// as an update of that one; beside anything else of its id. As [`pending`]
/// lists it. `at`: which one, when it's for one of them (an update's
/// alternative).
/// `instead`: the verified version the person chose over an unverified one
/// (its sheet's "instead"), which may be older than that one.
pub fn stage_from_store(dir: &Path, package: &Package, source: Source, at: Option<&str>, instead: bool) -> Result<Value, String> {
    let _held = changing();
    stage_held(dir, package, source, at, instead)
}

/// [`stage_from_store`], [`changing`] held.
fn stage_held(dir: &Path, package: &Package, mut source: Source, at: Option<&str>, instead: bool) -> Result<Value, String> {
    let store = source.store().cloned().ok_or(NOT_CHECKED)?;
    let identity = super::template_blocks::Identity { id: store.id.clone(), version: package.version(), hash: store.hash.clone() };
    super::template_blocks::refuse_package(dir, &package.content_hash, Some(&identity))?;
    package.compatible()?;
    let place = place_of(dir, &store.id, at);
    let version = package.version();
    let mut older = false;
    if let Some(place) = &place {
        if !install::newer_by_precedence(&version, &place.version) {
            older = instead && store.verified && place.takes_instead(&version);
            if !older {
                return Err(install::not_newer(&version, &place.version));
            }
        }
        if let Some(store) = source.store.as_mut() {
            store.replaced = Some(install::ReplacedVersion { version: place.version.clone(), verified: place.verified });
        }
    }
    let update_of = place.as_ref().map(|place| place.folder.clone());
    // (The very same: of one record, over one version of one folder, chosen so.)
    let waiting_already = pending(dir).into_iter().find(|item| {
        item["hash"] == package.hash.as_str()
            && item["store"]["id"] == store.id.as_str()
            && item["update_of"].as_str() == update_of.as_deref()
            && item["from"]["replaced"]["version"].as_str() == place.as_ref().map(|place| place.version.as_str())
            && (item["instead"] == true) == older
    });
    let name = match waiting_already.as_ref().and_then(|item| item["pending"].as_str()) {
        Some(name) => {
            keep_record_held(dir, name, source.store().ok_or(NOT_CHECKED)?)?;
            name.to_owned()
        }
        None => stage_as(dir, package, install::STORE, &source, update_of.as_deref(), older)?,
    };
    let mut out = pending(dir).into_iter().find(|item| item["pending"] == name.as_str()).ok_or("Couldn't keep it for you to allow")?;
    out["needs_consent"] = json!(true);
    Ok(out)
}

/// Where a store template goes over an earlier version of it here.
pub struct Place {
    pub folder: String,
    pub version: String,
    pub verified: bool,
}

impl Place {
    /// Whether a verified `version` may go over what's here though it isn't
    /// newer, by the person's own choice: over an unverified one, another.
    pub fn takes_instead(&self, version: &str) -> bool {
        !self.verified && install::newer_by_precedence(&self.version, version)
    }
}

/// The folder store template `id` is in here (`at`: that one, if it's it):
/// the one of its id before a copy of it.
pub fn place_of(dir: &Path, id: &str, at: Option<&str>) -> Option<Place> {
    let here = installed_from_store(dir, at).into_iter().filter(|t| t.store.id == id);
    here.min_by_key(|t| t.id != id).map(|t| Place { version: t.version, verified: t.store.verified, folder: t.id })
}

/// The waiting package `pending`, dropped.
pub fn drop_waiting(dir: &Path, pending: &str) {
    if let Ok((about, package)) = pending_paths(dir, pending) {
        let _ = std::fs::remove_file(package);
        let _ = std::fs::remove_file(about);
    }
}

/// What waits and can't be allowed (its package, its record or its file
/// beside it isn't what it was), read and found so, dropped: it isn't listed,
/// so nothing else would.
pub fn drop_unusable(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir.join(PENDING)) else { return };
    for name in entries.flatten().filter_map(|e| e.file_name().into_string().ok()?.strip_suffix(".json").map(str::to_owned)) {
        let Ok((about, package)) = pending_paths(dir, &name) else { continue };
        // (Couldn't be read just now: kept, for the next time.)
        let (Some(about), Ok(raw)) = (templates::read_regular(&about), package::read(&package, &WORDS)) else { continue };
        let about: Value = serde_json::from_slice(&about).unwrap_or_default();
        if about["pending"].as_str() != Some(name.as_str()) || waiting_of(dir, &name, &about, raw).is_none() {
            tracing::info!(pending = %name, "templates: what waited can't be allowed any more; dropped");
            drop_waiting(dir, &name);
        }
    }
}

/// The waiting store package `pending`'s record as the store has it now
/// (`now`): its mark and its publisher, for the list to say.
pub fn keep_record(dir: &Path, pending: &str, now: &install::StoreSource) -> Result<(), String> {
    let _changing = changing();
    keep_record_held(dir, pending, now)
}

/// [`keep_record`], under [`changing`].
fn keep_record_held(dir: &Path, pending: &str, now: &install::StoreSource) -> Result<(), String> {
    let (about_path, _) = pending_paths(dir, pending)?;
    let mut about: Value = templates::read_regular(&about_path).and_then(|b| serde_json::from_slice(&b).ok()).ok_or("What waits can't be read")?;
    let mut source = came_from(&about["origin"]).ok_or("What waits can't be read")?;
    let Some(store) = source.store.as_mut().filter(|s| same_item(s, now)) else { return Ok(()) };
    let was = store.clone();
    take_record(store, now);
    if *store == was {
        return Ok(());
    }
    about["origin"] = json!(source);
    templates::write_atomic(&dir.join(PENDING), &format!("{pending}.json"), &signing::pretty(&about))
}

/// Whether two records are of one version: its id, its contents and its
/// signer, which the store never changes.
fn same_item(a: &install::StoreSource, b: &install::StoreSource) -> bool {
    a.id == b.id && a.hash == b.hash && a.key == b.key
}

/// What a store's record says now that it may have said otherwise before.
fn take_record(store: &mut install::StoreSource, now: &install::StoreSource) {
    store.publisher.clone_from(&now.publisher);
    store.publisher_id.clone_from(&now.publisher_id);
    store.verified = now.verified;
    store.verified_at = now.verified_at;
}

/// What a store template that couldn't be checked is answered.
pub const NOT_CHECKED: &str = "Zeron couldn't check this with the store. Try again.";

/// `DecideTemplate`: a waiting package allowed (checked again, then
/// installed: `{installed, name, author, digest}`, `digest` what the app
/// keeps it by) or not (dropped: `{declined}`).
/// `hash` is the package's as you were shown it: a different one isn't
/// installed. `now`: for one from the store, its record as the store has it
/// now; none, or another's, isn't installed.
pub fn decide(dir: &Path, pending: &str, allow: bool, hash: &str, now: Option<&install::StoreSource>, author_of: impl Fn(&str) -> Author) -> Result<Value, String> {
    let _held = changing();
    let (about_path, package_path) = pending_paths(dir, pending)?;
    let forget = || {
        let _ = std::fs::remove_file(&package_path);
        let _ = std::fs::remove_file(&about_path);
    };
    if !allow {
        forget();
        return Ok(json!({ "declined": pending }));
    }
    let about: Value = templates::read_regular(&about_path)
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let mut source = came_from(&about["origin"]);
    match (source.as_mut().and_then(|s| s.store.as_mut()), now) {
        (None, None) => {}
        (Some(store), Some(now)) if same_item(store, now) => take_record(store, now),
        // (Not the store's as it was checked, or not checked as the store's.)
        _ => return Err(NOT_CHECKED.into()),
    }
    let store = source.as_ref().and_then(Source::store).cloned();
    let package = match package::read(&package_path, &WORDS).and_then(|raw| read_waiting(raw, &about, store.as_ref())) {
        Ok(package) => package,
        Err(error) => {
            forget();
            return Err(error);
        }
    };
    if package.hash != hash || about["key"] != json!(package.key) {
        return Err("This template changed while it waited for you. Import it again to see what it is now.".into());
    }
    // Taken down while it waited.
    let identity = store.as_ref().map(|s| super::template_blocks::Identity { id: s.id.clone(), version: package.version(), hash: s.hash.clone() });
    super::template_blocks::refuse_package(dir, &package.content_hash, identity.as_ref())?;
    // (The store's record says whose one from it is.)
    let author = match &store {
        Some(store) => super::scan::store_author(store),
        None => package.author(&author_of),
    };
    // An update that changes its code, allowed: in place of the template it
    // updates, if that's still the one it was for.
    if let Some(of) = about["update_of"].as_str() {
        let folder = templates::template_dir(dir, of).ok_or_else(|| format!("There's no template {of} to update any more"))?;
        let installed = imported(&folder).ok_or("It wasn't installed from a package")?;
        // One from the store is known by its record's id there; any other by
        // the key that signed it.
        let was_from = serde_json::from_value::<Source>(installed["source"].clone()).ok();
        let same = match (was_from.as_ref().and_then(Source::store), &store) {
            (Some(was), Some(now)) => was.id == now.id,
            (None, None) => installed["key"] == json!(package.key),
            _ => return Err("The new version comes from somewhere else, so it won't be installed".into()),
        };
        if !same {
            return Err("The new version is signed by someone else, so it won't be installed".into());
        }
        let was = installed["version"].as_str().unwrap_or("");
        let version = package.version();
        let fits = match (was_from.as_ref().and_then(Source::store), &store) {
            // (The verified version the person chose instead, over the very
            // one its window says it replaces.)
            (Some(before), Some(now)) => {
                let place = Place { folder: of.to_owned(), version: was.to_owned(), verified: before.verified };
                let instead = about["instead"] == true && now.verified && now.replaced.as_ref().is_some_and(|r| r.version == was);
                install::newer_by_precedence(&version, was) || instead && place.takes_instead(&version)
            }
            _ => install::newer(&version, was),
        };
        if !fits {
            return Err(install::not_newer(&version, was));
        }
        if let Some(source) = &mut source {
            source.version = version;
            source.installed_at = super::now_secs();
        }
        replace(dir, of, &folder, &package, &author, source.as_ref())?;
        forget();
        forget_found(dir, of);
        return Ok(json!({ "installed": of, "updated": of, "name": package.about()["name"], "author": author, "digest": package.digest() }));
    }
    // (Asked for twice before either was allowed: the other's here by now,
    // and this one's an update of it, or older.)
    if let Some(store) = &store
        && place_of(dir, &store.id, None).is_some()
    {
        return Err("It's been installed from the store since this one waited. Ask for it again to update it.".into());
    }
    let id = install(dir, &package, &author, source.as_ref())?;
    forget();
    Ok(json!({ "installed": id, "name": package.about()["name"], "author": author, "digest": package.digest() }))
}

/// A waiting package, read and verified again: one the store served
/// (`store`: the record it was fetched by) as its record says, unsigned only
/// when that names no signer, and the very contents it vouched for.
fn read_waiting(raw: Vec<u8>, about: &Value, store: Option<&install::StoreSource>) -> Result<Package, String> {
    let Some(store) = store else { return parse(raw) };
    let package = parse_from_store(raw, store.key.is_some())?;
    if package.content_hash != store.hash || package.key != store.key || about["id"].as_str() != Some(store.id.as_str()) || package.id() != store.id {
        return Err("What waited isn't what the store's record says, so it wasn't installed. Install it from the store again.".into());
    }
    Ok(package)
}

/// An installed template's `.imported.json`, if it came from a package: the
/// record says so, never its `key` (null for one a deploy token published).
pub fn imported(folder: &Path) -> Option<Value> {
    let value: Value = serde_json::from_slice(&templates::read_regular(&folder.join(IMPORTED))?).ok()?;
    value.is_object().then_some(value)
}

/// Whether `folder` has an `.imported.json` at all, one that can be read or
/// not: it isn't Zeron's, nor made here.
pub fn has_record(folder: &Path) -> bool {
    std::fs::symlink_metadata(folder.join(IMPORTED)).is_ok()
}

/// Where an installed template came from, as its record says.
pub fn source(folder: &Path) -> Option<Source> {
    serde_json::from_value(imported(folder)?["source"].clone()).ok()
}

// ── From a link ─────────────────────────────────────────────────────────────

/// What `link` names, if a template can come from it: a link to a
/// `.zeron-template`, or a GitHub repository (its latest release's, or its
/// page) or release (`…/releases/latest`, `…/releases/tag/<tag>`) carrying
/// one. A repository itself isn't a template: templates are shared signed.
pub fn origin(link: &str) -> Result<Origin, String> {
    let not_one = || {
        "That link isn't a template: give a link to a .zeron-template file, or a GitHub repository or release with one".to_string()
    };
    let link = link.trim();
    if link.is_empty() || link.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(not_one());
    }
    let Some(link) = install::https_link(link) else {
        return Err("Templates come from https links".into());
    };
    let bare = link.split(['?', '#']).next().unwrap_or(&link).trim_end_matches('/');
    if bare.ends_with(&format!(".{EXTENSION}")) {
        return Ok(Origin::Link(link));
    }
    let Some(rest) = bare.strip_prefix("https://github.com/") else {
        return Err(not_one());
    };
    let parts: Vec<&str> = rest.split('/').collect();
    if parts.len() < 2 || parts[0].is_empty() || parts[1].is_empty() {
        return Err(not_one());
    }
    let repo = format!("{}/{}", parts[0], parts[1].trim_end_matches(".git"));
    match &parts[2..] {
        [] | ["releases"] | ["releases", "latest"] => Ok(Origin::Release { repo, tag: None }),
        ["releases", "tag", tag] => Ok(Origin::Release { repo, tag: Some((*tag).to_owned()) }),
        _ => Err(not_one()),
    }
}

/// The package `origin` names, read and verified, and where it came from
/// (its `Source`, without `version`/`installed_at`, which installing fills).
pub async fn fetch(origin: &Origin) -> Result<(Package, Source), String> {
    let (raw, source) = match origin {
        Origin::Link(url) => (
            install::download(url).await?,
            Source { kind: "link".into(), url: Some(url.clone()), path: None, git_ref: None, commit: None, version: String::new(), installed_at: 0, store: None },
        ),
        Origin::Release { repo, tag } => {
            let asset = install::release_asset_of(repo, tag.as_deref(), EXTENSION).await.map_err(|error| {
                // (A repository with no release yet, or none with a template.)
                // (Anything else, GitHub turning requests away among them,
                // says what it is.)
                let missing = matches!(install::reason_of(&error), Some(install::reason::RELEASE_NOT_FOUND | install::reason::NO_PACKAGE));
                if tag.is_none() && missing {
                    install::failure(install::reason::NO_PACKAGE, "That repository has no release with a template in it")
                } else {
                    error
                }
            })?;
            (
                install::download(&asset).await?,
                Source {
                    kind: "release".into(),
                    url: Some(format!("https://github.com/{repo}")),
                    path: None,
                    git_ref: tag.clone(),
                    commit: None,
                    version: String::new(),
                    installed_at: 0,
                    store: None,
                },
            )
        }
        Origin::Git { .. } => return Err("A template comes from a .zeron-template file or a release, not a repository's files".into()),
    };
    let package = tokio::task::spawn_blocking(move || parse(raw))
        .await
        .map_err(|_| "Couldn't read it".to_string())??;
    Ok((package, source))
}

/// Where `source` is now, if it may have changed: `None` when it surely
/// hasn't (a package file gone).
pub async fn fetch_again(source: &Source) -> Result<Option<(Package, Source)>, String> {
    match source.kind.as_str() {
        "file" => {
            let Some(path) = source.path.clone().map(PathBuf::from) else { return Ok(None) };
            if !path.is_file() {
                return Ok(None);
            }
            let package = tokio::task::spawn_blocking(move || read(&path))
                .await
                .map_err(|_| "Couldn't read it".to_string())??;
            Ok(Some((package, source.clone())))
        }
        "link" => {
            let url = source.url.clone().ok_or("It has no link")?;
            fetch(&Origin::Link(url)).await.map(Some)
        }
        "release" => {
            let url = source.url.as_deref().ok_or("It has no repository")?;
            let repo = url.trim_start_matches("https://github.com/").to_owned();
            fetch(&Origin::Release { repo, tag: source.git_ref.clone() }).await.map(Some)
        }
        _ => Ok(None),
    }
}

// ── Updates ─────────────────────────────────────────────────────────────────

fn found_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(UPDATES).join(format!("{id}.json"))
}

fn found_package_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(UPDATES).join(format!("{id}.{EXTENSION}"))
}

/// The update found for template `id`, if it's still for the template as
/// it is (installed from package `hash`).
pub fn found(dir: &Path, id: &str, hash: &str) -> Option<Found> {
    let found: Found = serde_json::from_slice(&templates::read_regular(&found_path(dir, id))?).ok()?;
    (found.from == hash).then_some(found)
}

pub fn forget_found(dir: &Path, id: &str) {
    let _ = std::fs::remove_file(found_path(dir, id));
    let _ = std::fs::remove_file(found_package_path(dir, id));
}

/// Keep `package` as `id`'s update (none: refused, only what was found).
pub fn keep_found(dir: &Path, id: &str, found: &Found, package: Option<&Package>) -> Result<(), String> {
    let updates = dir.join(UPDATES);
    std::fs::create_dir_all(&updates).map_err(|_| "Couldn't keep the update".to_string())?;
    match package {
        Some(package) => templates::write_atomic(&updates, &format!("{id}.{EXTENSION}"), &package.raw)?,
        None => {
            let _ = std::fs::remove_file(found_package_path(dir, id));
        }
    }
    templates::write_atomic(&updates, &format!("{id}.json"), &signing::pretty(&json!(found)))
}

/// What a check of template `id` in `dir` (installed from package `hash`,
/// signed by `key`) found in `fetched`: nothing new, or an update (refused
/// when it's another template, someone else signed it, or it isn't a later
/// version). Another template is told by the package's own id, as its
/// installed manifest has it: one installed beside another of that id
/// (`<id>-2`) is still that package.
pub fn compare(dir: &Path, id: &str, key: &str, hash: &str, fetched: &Package, source: Source) -> Option<Found> {
    if fetched.hash == hash {
        return None;
    }
    let folder = templates::template_dir(dir, id);
    let manifest: Value = folder
        .as_ref()
        .and_then(|f| templates::read_regular(&f.join(MANIFEST)))
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let package_id = manifest["id"].as_str().map(str::trim).filter(|p| !p.is_empty()).unwrap_or(id).to_owned();
    let was = folder.as_deref().and_then(imported).and_then(|i| i["version"].as_str().map(str::to_owned)).unwrap_or_default();
    let refused = if fetched.id() != package_id {
        Some(format!("The new version is a different template ({})", fetched.id()))
    } else if fetched.key.as_deref() != Some(key) {
        Some("The new version is signed by someone else, so it won't be installed".to_string())
    } else if !install::newer(&fetched.version(), &was) {
        Some(install::not_newer(&fetched.version(), &was))
    } else {
        None
    };
    Some(Found {
        from: hash.to_owned(),
        version: fetched.text("version"),
        hash: fetched.hash.clone(),
        added: Vec::new(),
        removed: Vec::new(),
        refused,
        source: Some(source),
        digest_diff: None,
        ..Found::default()
    })
}

/// The templates installed from a package that may have a newer version
/// where they came from: each one's id, name, key, package hash and source.
pub fn checkable(dir: &Path) -> Vec<(String, String, String, String, Source)> {
    // (Their records alone: no list, whose rows are hashed for their mark.
    // In its order.)
    let mut out: Vec<(String, String, String, String, Source)> = templates::installed(dir)
        .into_iter()
        .filter_map(|id| {
            let folder = templates::template_dir(dir, &id)?;
            let imported = imported(&folder)?;
            let source: Source = serde_json::from_value(imported["source"].clone()).ok()?;
            // (The store's are asked about there alone: `Plugins::check_store_templates`.)
            if source.kind == install::STORE {
                return None;
            }
            let name = templates::name(&folder, &id);
            Some((
                id,
                name,
                imported["key"].as_str()?.to_owned(),
                imported["hash"].as_str()?.to_owned(),
                source,
            ))
        })
        .collect();
    out.sort_by_key(|(id, name, ..)| (name.to_lowercase(), id.clone()));
    out
}

/// `UpdateTemplate`: the update found for `id` installed in its place (its
/// id, its use and its look kept), checked again first: the same key as
/// the one installed, the same package as was found. `{updated, name,
/// version, from}`.
pub fn update(dir: &Path, id: &str, author_of: impl Fn(&str) -> Author) -> Result<Value, String> {
    let _held = changing();
    let folder = templates::template_dir(dir, id).ok_or_else(|| format!("There's no template {id}"))?;
    let imported = imported(&folder).ok_or("It wasn't installed from a package, so it has nothing to update from")?;
    // (The store's is, by its record as it is now: `update_from_store`.)
    if imported["source"]["kind"] == install::STORE {
        return Err(NOT_CHECKED.into());
    }
    let hash = imported["hash"].as_str().unwrap_or("");
    let found = found(dir, id, hash).ok_or("There's no update for it right now; check again")?;
    if let Some(refused) = found.refused {
        return Err(refused);
    }
    let package = read(&found_package_path(dir, id))?;
    if package.hash != found.hash {
        return Err("The update changed since it was found; check again".into());
    }
    if imported["key"] != json!(package.key) {
        return Err("The new version is signed by someone else, so it won't be installed".into());
    }
    let was = imported["version"].as_str().unwrap_or("");
    if !install::newer(&package.version(), was) {
        return Err(install::not_newer(&package.version(), was));
    }
    super::template_blocks::refuse_package(dir, &package.content_hash, None)?;
    let mut source: Option<Source> = found.source.clone().or_else(|| serde_json::from_value(imported["source"].clone()).ok());
    // A new version whose code differs from what's installed (its manifest, or anything but
    // its listing files) is a new template to allow: it waits for you in
    // Zeron's own consent window, as someone else's does, and replaces this
    // one only once you allow it there (`decide`).
    if asks(&folder, &package, false) {
        let shown = source.clone().unwrap_or_else(|| Source {
            kind: "file".into(),
            url: None,
            path: None,
            git_ref: None,
            commit: None,
            version: String::new(),
            installed_at: 0,
            store: None,
        });
        let from = shown.url.clone().unwrap_or_default();
        let pending = stage_as(dir, &package, &from, &shown, Some(id), false)?;
        let mut out = package.about();
        out["pending"] = json!(pending);
        out["update_of"] = json!(id);
        out["author"] = json!(package.author(&author_of));
        out["needs_consent"] = json!(true);
        return Ok(out);
    }
    if let Some(source) = &mut source {
        source.version = package.text("version");
        source.installed_at = super::now_secs();
    }
    let author = package.author(&author_of);
    replace(dir, id, &folder, &package, &author, source.as_ref())?;
    forget_found(dir, id);
    Ok(json!({
        "updated": id,
        "name": package.name(),
        "version": package.text("version"),
        "from": imported["version"],
    }))
}

/// A store template here, as the update check asks about it.
pub struct Installed {
    /// Its folder.
    pub id: String,
    pub name: String,
    pub version: String,
    /// The package it was installed from (`imported.hash`).
    pub hash: String,
    pub store: install::StoreSource,
}

/// Each store template here (`only`: in that folder).
pub fn installed_from_store(dir: &Path, only: Option<&str>) -> Vec<Installed> {
    from_store(dir)
        .into_iter()
        .filter(|(id, _)| only.is_none_or(|only| only == id))
        .filter_map(|(id, store)| {
            let folder = dir.join(&id);
            let record = imported(&folder)?;
            let name = templates::name(&folder, &id);
            Some(Installed { name, version: record["version"].as_str()?.to_owned(), hash: record["hash"].as_str()?.to_owned(), store, id })
        })
        .collect()
}

/// The package kept as store template `id`'s update, read and checked again:
/// the one that was found, as its record says it.
pub fn kept_from_store(dir: &Path, id: &str, found: &Found) -> Result<Package, String> {
    let store = found.source.as_ref().and_then(Source::store).ok_or(NOT_CHECKED)?;
    let package = parse_from_store(package::read(&found_package_path(dir, id), &WORDS)?, store.key.is_some())?;
    if package.hash != found.hash || package.content_hash != store.hash || package.key != store.key || package.id() != store.id {
        return Err("The update changed since it was found; check again".into());
    }
    Ok(package)
}

/// `UpdateTemplate` for store template `id`: the update found for it, by the
/// store's record of it as it is now (`now`). It waits for you (the item, as
/// [`pending`] lists it) but for one that's verified, by the same publisher,
/// over the store's version unedited, and changes its listing files alone:
/// that's installed in its place.
/// `alternative`: the verified version its sheet can offer instead.
pub fn update_from_store(dir: &Path, id: &str, now: &install::StoreSource, alternative: Option<install::Alternative>) -> Result<Value, String> {
    let _held = changing();
    let folder = templates::template_dir(dir, id).ok_or_else(|| format!("There's no template {id}"))?;
    let record = imported(&folder).ok_or(NOT_CHECKED)?;
    let was = came_from(&record["source"]).and_then(|s| s.store).ok_or(NOT_CHECKED)?;
    let found = found(dir, id, record["hash"].as_str().unwrap_or("")).ok_or("There's no update for it right now; check again")?;
    if let Some(refused) = found.refused {
        return Err(refused);
    }
    let package = kept_from_store(dir, id, &found)?;
    let mut source = found.source.ok_or(NOT_CHECKED)?;
    let store = source.store.as_mut().filter(|s| same_item(s, now) && s.id == was.id).ok_or(NOT_CHECKED)?;
    take_record(store, now);
    let before = record["version"].as_str().unwrap_or("").to_owned();
    store.replaced = Some(install::ReplacedVersion { version: before.clone(), verified: was.verified });
    // (As its sheet offers it, when it isn't verified.)
    store.verified_alternative = alternative.filter(|_| !now.verified);
    source.version = package.version();
    source.installed_at = super::now_secs();
    let asks = !now.verified || now.publisher_id != was.publisher_id || edited(&folder) != Some(false) || asks(&folder, &package, true);
    if asks {
        return stage_held(dir, &package, source, Some(id), false);
    }
    let identity = super::template_blocks::Identity { id: now.id.clone(), version: package.version(), hash: now.hash.clone() };
    super::template_blocks::refuse_package(dir, &package.content_hash, Some(&identity))?;
    package.compatible()?;
    if !install::newer_by_precedence(&package.version(), &before) {
        return Err(install::not_newer(&package.version(), &before));
    }
    replace(dir, id, &folder, &package, &super::scan::store_author(now), Some(&source))?;
    forget_found(dir, id);
    Ok(json!({ "updated": id, "name": package.about()["name"], "version": package.about()["version"], "from": before }))
}

/// The update found for the template in folder `id`, as it's shown.
pub fn shown_update(dir: &Path, id: &str, hash: &str) -> Option<Value> {
    let found = found(dir, id, hash)?;
    let mut shown = found.shown();
    shown["version"] = json!(listing::shown(&found.version, version::MAX_LEN));
    Some(shown)
}

/// Whether an update to `package` waits for you. One from the store
/// (`store`): its code (its listing files aside), or anything else its
/// manifest says but its version. One from a file or a link, as it always
/// has: its code, where everything but its preview and README is code.
fn asks(folder: &Path, package: &Package, store: bool) -> bool {
    if store { changes_code(folder, package) || changes_manifest(folder, package) } else { changes_files(folder, package) }
}

/// A file or link template's: any file added, removed or changed, but its
/// manifest, its preview and its README (which no package carried before it
/// was published to the store).
fn changes_files(folder: &Path, package: &Package) -> bool {
    let counts = |rel: &str| ![MANIFEST, templates::BASE, PREVIEW, listing::README].contains(&rel);
    let mut now: BTreeMap<String, String> = BTreeMap::new();
    for rel in templates::app_files(folder, &[MANIFEST, templates::BASE, PREVIEW, listing::README]) {
        let Some(bytes) = inside(folder, &rel).and_then(|p| templates::read_regular(&p)) else { return true };
        now.insert(rel, signing::sha256_hex(&bytes));
    }
    let next: BTreeMap<String, String> = package.files.iter().filter(|(rel, _)| counts(rel)).map(|(rel, bytes)| (rel.clone(), signing::sha256_hex(bytes))).collect();
    now != next
}

/// Whether installing `package` over `folder` changes any of its code: a
/// file of its own added, removed or changed, but its listing files (its
/// preview picture, README, changelog and screenshots).
fn changes_code(folder: &Path, package: &Package) -> bool {
    let mut now: BTreeMap<String, String> = BTreeMap::new();
    for rel in templates::own_files(folder) {
        let Some(bytes) = inside(folder, &rel).and_then(|p| templates::read_regular(&p)) else { return true };
        now.insert(rel, signing::sha256_hex(&bytes));
    }
    let next: BTreeMap<String, String> = package
        .files
        .iter()
        .filter(|(rel, _)| !listing::template_listing_file(rel))
        .map(|(rel, bytes)| (rel.clone(), signing::sha256_hex(bytes)))
        .collect();
    now != next
}

/// Whether installing `package` over `folder` changes what its manifest
/// says, but for its version (and who signed it): its name is what you'd
/// know it by.
fn changes_manifest(folder: &Path, package: &Package) -> bool {
    let said = |manifest: &Value| {
        let mut manifest = manifest.clone();
        for member in ["version", "signature", BUILTIN_THEME, BUILTIN_TEMPLATE] {
            manifest.as_object_mut().map(|m| m.remove(member));
        }
        manifest
    };
    let now = templates::read_regular(&folder.join(MANIFEST)).and_then(|b| serde_json::from_slice::<Value>(&b).ok());
    now.is_none_or(|now| said(&now) != said(&package.manifest))
}

/// `package` written in place of template `id`'s `folder`: the old aside,
/// the new in, then the old gone.
fn replace(dir: &Path, id: &str, folder: &Path, package: &Package, author: &Author, source: Option<&Source>) -> Result<(), String> {
    let tmp = write_folder(dir, id, package, author, source)?;
    // In its place: the old aside, the new in, then the old gone.
    let old = dir.join(format!(".tmp-old-{id}-{}-{}", std::process::id(), super::now_secs()));
    std::fs::rename(&folder, &old).map_err(|e| {
        let _ = std::fs::remove_dir_all(&tmp);
        format!("Couldn't update it: {e}")
    })?;
    if let Err(error) = std::fs::rename(&tmp, &folder) {
        let _ = std::fs::rename(&old, &folder);
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(format!("Couldn't update it: {error}"));
    }
    let _ = std::fs::remove_dir_all(&old);
    Ok(())
}

#[cfg(test)]
mod tests {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD as B64;

    use super::*;

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zeron-template-packages-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// A templates folder with a template `midnight` of a few files, and
    /// some it may not replace.
    fn with_template(name: &str) -> PathBuf {
        let d = dir(name).join("templates");
        let t = d.join("midnight");
        write(&t.join("app.luau"), "-- app");
        write(&t.join("ds/tokens.luau"), "-- midnight tokens");
        write(&t.join("screens/shell/sidebar.luau"), "-- sidebar");
        write(&t.join(".luaurc"), "{}");
        write(&t.join("screens/plugins/page.luau"), "-- plugins page");
        write(&t.join("account/sessions.luau"), "-- sessions");
        write(&t.join(PREVIEW), "png");
        write(&t.join(super::super::guides::README), "# Midnight");
        write(&t.join(templates::BASE), "{}");
        write(
            &t.join(MANIFEST),
            r#"{"id":"midnight","name":"Midnight","description":"Dark.","author":"you","version":"1.0.0","zeron_version":"0.9.0"}"#,
        );
        d
    }

    fn key() -> SigningKey {
        SigningKey::generate().unwrap().0
    }

    /// `mine` is You; anyone else unknown.
    fn author_as(mine: &SigningKey) -> impl Fn(&str) -> Author + '_ {
        move |k: &str| if k == mine.public() { Author::You } else { Author::unknown("Not registered") }
    }

    fn package_json(path: &Path) -> Value {
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    }

    /// What the store serves of a token's version: no signer. It waits, is
    /// allowed, listed and deleted as any package's template; its record
    /// makes it one, never a key.
    #[test]
    fn plugins_template_packages_with_no_signer_are_packages_all_the_same() {
        let root = dir("no-signer");
        let d = root.join("templates");
        std::fs::create_dir_all(d.join(templates::ZERON_DIR)).unwrap();
        write(&d.join(templates::ZERON_DIR).join(MANIFEST), r#"{"zeron_version":"0.9.0"}"#);
        let manifest = json!({ "id": "midnight", "name": "Midnight", "description": "Dark.", "version": "1.0.0", "zeron_version": "0.3.16", "author": "Zeron", "builtin_template": "midnight" });
        let files = BTreeMap::from([("app.luau".to_owned(), b"x".to_vec())]);
        let raw = pack(&manifest, &files, None).unwrap();
        let record = |hash: &str, id: &str| install::StoreSource { id: id.into(), publisher: "ada".into(), publisher_id: "pub_0123456789abcdef0123456789abcdef".into(), hash: hash.into(), ..Default::default() };
        let source = |store: install::StoreSource| Source { kind: install::STORE.into(), url: None, path: None, git_ref: None, commit: None, version: "1.0.0".into(), installed_at: 0, store: Some(store) };
        let nobody = |_: &str| -> Author { panic!("no key to ask about") };
        let wait = |source: &Source| {
            let package = parse_from_store(raw.clone(), false).unwrap();
            (stage(&d, &package, "store", source).unwrap(), package.hash.clone())
        };
        let hash = parse_from_store(raw.clone(), false).unwrap().content_hash;
        // Contents, or an id, the record doesn't vouch for; or no record at
        // all: never installed.
        for bad in [source(record(&"0".repeat(64), "midnight")), source(record(&hash, "other")), Source { kind: "file".into(), store: None, ..source(record(&hash, "midnight")) }] {
            let (pending, shown) = wait(&bad);
            assert!(decide(&d, &pending, true, &shown, bad.store.as_ref(), nobody).is_err());
            assert!(!d.join("midnight").exists());
        }
        // (It waited verified; the store's word now is that it isn't.)
        let (pending, shown) = wait(&source(install::StoreSource { verified: true, verified_at: Some(7), publisher_id: "pub_old".into(), ..record(&hash, "midnight") }));
        assert!(!d.join(super::super::template_blocks::FILE).exists());
        // Not without the store's word of it now, nor by another record's.
        let now = record(&hash, "midnight");
        for other in [None, Some(record(&"0".repeat(64), "midnight")), Some(record(&hash, "other")), Some(install::StoreSource { key: Some("k".into()), ..now.clone() })] {
            assert_eq!(decide(&d, &pending, true, &shown, other.as_ref(), nobody).unwrap_err(), NOT_CHECKED);
        }
        // What its record says now is kept of it, and of no other.
        let kept = |id: &str| serde_json::from_slice::<Value>(&std::fs::read(d.join(PENDING).join(format!("{id}.json"))).unwrap()).unwrap()["origin"]["store"]["publisher_id"].clone();
        keep_record(&d, &pending, &install::StoreSource { hash: "0".repeat(64), ..now.clone() }).unwrap();
        assert_eq!(kept(&pending), "pub_old");
        keep_record(&d, &pending, &now).unwrap();
        assert_eq!(kept(&pending), json!(now.publisher_id));
        // One that isn't the store's any more isn't installed by its record.
        let about = d.join(PENDING).join(format!("{pending}.json"));
        let was = std::fs::read_to_string(&about).unwrap();
        let mut forged: Value = serde_json::from_str(&was).unwrap();
        forged["origin"] = json!({ "kind": "file", "version": "1.0.0", "installed_at": 0 });
        write(&about, &forged.to_string());
        assert_eq!(decide(&d, &pending, true, &shown, Some(&now), nobody).unwrap_err(), NOT_CHECKED);
        write(&about, &was);
        let done = decide(&d, &pending, true, &shown, Some(&now), nobody).unwrap();
        assert_eq!((done["installed"].as_str(), done["author"]["username"].as_str()), (Some("midnight"), Some("ada")), "{done}");
        // (What's been taken down is kept from before it: it isn't blocked.)
        assert!(super::super::template_blocks::read(&d).is_ok_and(|b| b.is_empty()));
        let t = d.join("midnight");
        let kept = imported(&t).unwrap();
        assert!(kept["key"].is_null() && has_record(&t), "{kept}");
        assert_eq!(super::source(&t).unwrap().store(), Some(&now), "as the store says now");
        let listed = templates::list(&d).unwrap();
        let row = listed["templates"].as_array().unwrap().iter().find(|t| t["id"] == "midnight").unwrap().clone();
        assert_eq!((row["builtin"].as_bool(), row["author"].as_str(), row["imported"]["author"]["username"].as_str()), (Some(false), Some(""), Some("ada")), "{row}");
        // Never Zeron's own: not even marked so by hand, its record unreadable.
        write(&t.join(MANIFEST), r#"{"id":"midnight","name":"Midnight","builtin_template":"midnight"}"#);
        write(&t.join(IMPORTED), "not json");
        let listed = templates::list(&d).unwrap();
        assert!(listed["templates"].as_array().unwrap().iter().any(|t| t["id"] == "midnight" && t["builtin"] == false));
        assert!(templates::delete(&d, "midnight").is_ok());
    }

    /// A package taken down from the store isn't imported, allowed while it
    /// waits, or taken as an update, wherever it came from: by its contents.
    #[test]
    fn plugins_template_packages_taken_down_come_in_by_no_door() {
        use super::super::template_blocks::{self, Block, Change};
        let d = with_template("down");
        let out = d.parent().unwrap().join("midnight.zeron-template");
        let alice = key();
        let bob = key();
        export(&d, "midnight", &out, &alice).unwrap();
        let other = dir("down-other").join("templates");
        std::fs::create_dir_all(&other).unwrap();
        let downed = |path: &Path, kind: fn(Block) -> Change| {
            let hash = read(path).unwrap().content_hash;
            let block = Block { id: "elsewhere".into(), version: "9.9.9".into(), hash: Some(hash), reason: "Bad.".into(), at: super::super::now_secs() };
            template_blocks::apply(&other, vec![kind(block)], super::super::now_secs()).unwrap();
        };
        let restored = |b: Block| Change::Restored(b.id, b.version);
        // Waiting, then taken down: not allowed.
        let waiting = import(&other, &out, author_as(&bob)).unwrap();
        downed(&out, Change::TakenDown);
        let error = decide(&other, waiting["pending"].as_str().unwrap(), true, waiting["hash"].as_str().unwrap(), None, author_as(&bob)).unwrap_err();
        assert!(error.starts_with("taken_down: "), "{error}");
        // From a file: someone else's, or yours.
        for me in [&bob, &alice] {
            let error = import(&other, &out, author_as(me)).unwrap_err();
            assert!(error.starts_with("taken_down: "), "{error}");
        }
        assert!(!other.join("midnight").exists());
        downed(&out, restored);
        import(&other, &out, author_as(&alice)).unwrap();
        // As an update.
        write(&d.join("midnight").join(MANIFEST), r#"{"id":"midnight","name":"Midnight","description":"Dark.","version":"1.1.0","zeron_version":"0.9.0"}"#);
        export(&d, "midnight", &out, &alice).unwrap();
        let first = imported(&other.join("midnight")).unwrap();
        let source: Source = serde_json::from_value(first["source"].clone()).unwrap();
        let newer = read(&out).unwrap();
        let found = compare(&other, "midnight", &alice.public(), first["hash"].as_str().unwrap(), &newer, source).unwrap();
        keep_found(&other, "midnight", &found, Some(&newer)).unwrap();
        downed(&out, Change::TakenDown);
        let error = update(&other, "midnight", author_as(&alice)).unwrap_err();
        assert!(error.starts_with("taken_down: "), "{error}");
        downed(&out, restored);
        assert_eq!(update(&other, "midnight", author_as(&alice)).unwrap()["version"], "1.1.0");
    }

    /// What an update asks, by where the template came from: a file, link or
    /// release one exactly as it always has (its preview and README aside,
    /// any file is code; a new description or `zeron_version` asks nothing,
    /// a changelog or picture does), and one from the store its code and
    /// what it says of itself.
    #[test]
    fn plugins_template_packages_ask_as_their_origin_does() {
        let d = dir("origin").join("templates");
        std::fs::create_dir_all(&d).unwrap();
        let alice = key();
        let of = |name: &str, made_from: &str, extra: &[(&str, &str)]| {
            let manifest = json!({ "id": "midnight", "name": name, "description": "Dark.", "version": "1.1.0", "zeron_version": made_from });
            let mut files: BTreeMap<String, Vec<u8>> = [("app.luau", "x"), ("preview.png", "p"), ("README.md", "r"), ("CHANGELOG.md", "c"), ("screenshots/a.png", "s")].map(|(rel, t)| (rel.to_owned(), t.as_bytes().to_vec())).into();
            for (rel, text) in extra {
                files.insert((*rel).to_owned(), text.as_bytes().to_vec());
            }
            parse(pack(&manifest, &files, Some(&alice)).unwrap()).unwrap()
        };
        let id = install(&d, &of("Midnight", "0.3.16", &[]), &Author::unknown("x"), None).unwrap();
        let t = d.join(id);
        let cases = [
            ("the same", of("Midnight", "0.3.16", &[]), false, false),
            ("a new name", of("Zeron", "0.3.16", &[]), false, true),
            ("a new zeron_version", of("Midnight", "0.3.17", &[]), false, true),
            ("a new preview", of("Midnight", "0.3.16", &[("preview.png", "q")]), false, false),
            ("a new README", of("Midnight", "0.3.16", &[("README.md", "q")]), false, false),
            ("a new changelog", of("Midnight", "0.3.16", &[("CHANGELOG.md", "q")]), true, false),
            ("a new picture", of("Midnight", "0.3.16", &[("screenshots/b.png", "q")]), true, false),
            ("new code", of("Midnight", "0.3.16", &[("app.luau", "y")]), true, true),
        ];
        for (what, package, from_file, from_store) in cases {
            assert_eq!((asks(&t, &package, false), asks(&t, &package, true)), (from_file, from_store), "{what}");
        }
        // A folder with no manifest to compare with asks, from the store.
        std::fs::remove_file(t.join(MANIFEST)).unwrap();
        assert!(asks(&t, &of("Midnight", "0.3.16", &[]), true));
    }

    /// `update` asks by the origin its record names: a renamed file template
    /// installs, a renamed one from the store waits.
    #[tokio::test]
    async fn plugins_template_packages_an_update_asks_by_its_record_s_origin() {
        for store in [false, true] {
            let d = with_template(if store { "wired-store" } else { "wired-file" });
            let out = d.parent().unwrap().join("midnight.zeron-template");
            let alice = key();
            export(&d, "midnight", &out, &alice).unwrap();
            let other = dir(if store { "wired-store-other" } else { "wired-file-other" }).join("templates");
            import(&other, &out, author_as(&alice)).unwrap();
            let folder = other.join("midnight");
            let mut record = imported(&folder).unwrap();
            if store {
                // (By its record of the very package, verified.)
                let was = read(&out).unwrap();
                record["source"] = json!(store_source(&was.content_hash, ADA, true));
                record["source"]["store"]["key"] = json!(alice.public());
                super::super::template_blocks::begin(&other).unwrap();
                std::fs::write(folder.join(IMPORTED), record.to_string()).unwrap();
            }
            let hash = record["hash"].as_str().unwrap().to_owned();
            write(&d.join("midnight").join(MANIFEST), r#"{"id":"midnight","name":"Renamed","description":"Dark.","version":"1.1.0","zeron_version":"0.0.1"}"#);
            export(&d, "midnight", &out, &alice).unwrap();
            let newer = read(&out).unwrap();
            let mut source: Source = serde_json::from_value(record["source"].clone()).unwrap();
            if let Some(store) = source.store.as_mut() {
                store.hash = newer.content_hash.clone();
            }
            let now = source.store.clone();
            let found = compare(&other, "midnight", &alice.public(), &hash, &newer, source).unwrap();
            keep_found(&other, "midnight", &found, Some(&newer)).unwrap();
            let done = match &now {
                Some(now) => {
                    assert_eq!(update(&other, "midnight", author_as(&alice)).unwrap_err(), NOT_CHECKED, "never the file's way");
                    update_from_store(&other, "midnight", now, None).unwrap()
                }
                None => update(&other, "midnight", author_as(&alice)).unwrap(),
            };
            assert_eq!(done["needs_consent"].as_bool(), store.then_some(true), "{store}: {done}");
        }
    }

    /// What a folder being written looks like: a dot folder beside the
    /// templates, complete, never at its own name until it's renamed there.
    #[test]
    fn plugins_template_packages_are_seen_whole_or_not_at_all() {
        let d = with_template("whole");
        let package = {
            let out = d.parent().unwrap().join("midnight.zeron-template");
            export(&d, "midnight", &out, &key()).unwrap();
            read(&out).unwrap()
        };
        let other = dir("whole-other").join("templates");
        std::fs::create_dir_all(&other).unwrap();
        let tmp = write_folder(&other, "midnight", &package, &Author::unknown("x"), None).unwrap();
        assert!(tmp.file_name().unwrap().to_string_lossy().starts_with('.') && tmp.join(MANIFEST).is_file() && tmp.join(IMPORTED).is_file());
        assert!(!other.join("midnight").exists(), "not at its name until it's whole");
        assert!(templates::installed(&other).is_empty(), "and no template by itself");
    }

    /// An update that says something else of itself asks, its code the same;
    /// a new version alone, with new listing files, doesn't.
    #[test]
    fn plugins_template_packages_ask_when_the_manifest_says_something_else() {
        let d = dir("manifest").join("templates");
        std::fs::create_dir_all(&d).unwrap();
        let alice = key();
        let of = |name: &str, version: &str, made_from: &str| {
            let manifest = json!({ "id": "midnight", "name": name, "description": "Dark.", "version": version, "zeron_version": made_from });
            parse(pack(&manifest, &BTreeMap::from([("app.luau".to_owned(), b"x".to_vec())]), Some(&alice)).unwrap()).unwrap()
        };
        let id = install(&d, &of("Midnight", "1.0.0", "0.3.16"), &Author::unknown("x"), None).unwrap();
        let t = d.join(id);
        assert!(!changes_manifest(&t, &of("Midnight", "1.1.0", "0.3.16")));
        assert!(changes_manifest(&t, &of("Zeron", "1.1.0", "0.3.16")));
        assert!(changes_manifest(&t, &of("Midnight", "1.1.0", "0.3.17")));
        std::fs::remove_file(t.join(MANIFEST)).unwrap();
        assert!(changes_manifest(&t, &of("Midnight", "1.1.0", "0.3.16")), "nothing to compare with: asks");
    }

    /// Every path any released Zeron protected stays protected: the store
    /// refuses the union of them (contract: `template_protected` only
    /// grows). Add to this list with each release that adds one; never drop.
    #[test]
    fn plugins_template_packages_protected_only_grows() {
        for released in ["screens/plugins/", "screens/templates/", "account/", ".luaurc", "ds/tokens_template.luau", "ds/tokens_template/", "ds/tokens/"] {
            assert!(never_templated().iter().any(|p| p == released), "{released} was protected in a release: it stays so");
        }
    }

    /// Where a template's tokens are laid beside Zeron's, and a folder that
    /// would stand in for Zeron's tokens, is no template's to fill: refused at publish, never exported, and left out of an import, of
    /// a store's install and of what's put in its folder, each named.
    #[test]
    fn plugins_template_packages_no_one_brings_where_tokens_are_laid() {
        let manifest = json!({ "id": "midnight", "name": "Midnight", "description": "", "version": "1.0.0", "zeron_version": "0.3.16" });
        let rels = ["DS/Tokens_Template.luau", "ds/tokens/init.luau", "ds/tokens_template.luau", "ds/tokens_template/deep/a.luau", "ds/tokens_template/init.luau"];
        let mut files = BTreeMap::from([("app.luau".to_owned(), b"return 1".to_vec()), ("ds/tokens.luau".to_owned(), b"return {}".to_vec())]);
        files.extend(rels.map(|rel| (rel.to_owned(), b"planted".to_vec())));
        // At publish, each by name.
        let (problems, _) = listing::check_template(&manifest, &files);
        let refused: Vec<&str> = problems.iter().filter(|p| p.rule == listing::rule::PROTECTED_FILE).filter_map(|p| p.path.as_deref()).collect();
        assert_eq!(refused, rels);
        // As a file's or a link's, signed; and as the store's, its record's too.
        let d = dir("tokens-laid");
        for (signer, from_store) in [(Some(key()), false), (Some(key()), true), (None, true)] {
            let raw = pack(&manifest, &files, signer.as_ref()).unwrap();
            let package = if from_store { parse_from_store(raw.clone(), signer.is_some()) } else { parse(raw.clone()) }.unwrap();
            assert_eq!((package.ignored.iter().map(String::as_str).collect::<Vec<_>>(), package.files.keys().map(String::as_str).collect::<Vec<_>>()), (rels.to_vec(), vec!["app.luau", "ds/tokens.luau"]));
            let id = install(&d, &package, &Author::You, None).unwrap();
            let t = d.join(&id);
            assert!(t.join("ds/tokens.luau").is_file());
            assert!(rels.iter().all(|rel| !t.join(rel).exists()), "{id}");
            // Put there by hand, it's never sent, and said.
            for rel in &rels[1..] {
                write(&t.join(rel), "planted");
            }
            let (_, sent) = contents(&t, &id).unwrap();
            assert!(sent.keys().all(|rel| !rel.contains("tokens_template") && !rel.starts_with("ds/tokens/")), "{sent:?}");
            let left = left_out(&t, &sent);
            assert!(rels[1..].iter().all(|rel| left.files.iter().any(|(left, _)| left == rel)), "{:?}", left.files);
        }
    }

    /// A template is for the Zeron it was made from and later.
    #[test]
    fn plugins_template_packages_compatible_from_their_zeron_version() {
        let with = |made_from: Value| {
            let manifest = json!({ "id": "midnight", "name": "Midnight", "description": "", "version": "1.0.0", "zeron_version": made_from });
            let files = BTreeMap::from([("app.luau".to_owned(), b"x".to_vec())]);
            parse(pack(&manifest, &files, Some(&key())).unwrap()).unwrap().compatible()
        };
        let now = super::super::version::zeron();
        assert!(with(json!(now.to_string())).is_ok() && with(json!("0.0.1")).is_ok());
        for later in [json!("9999.0.0"), json!(""), json!("0.3"), Value::Null] {
            assert_eq!(install::reason_of(&with(later.clone()).unwrap_err()), Some("incompatible"), "{later}");
        }
    }

    /// A name that could be another file's on some disk: a Windows short name
    /// is left out and named, one with a character drawn as nothing refuses
    /// the package. Neither is ever exported.
    #[test]
    fn plugins_template_packages_names_that_could_be_another_s() {
        let manifest = json!({ "id": "midnight", "name": "Midnight", "description": "", "version": "1.0.0", "zeron_version": "0.3.16" });
        let package = |rels: &[&str]| {
            let files: BTreeMap<String, Vec<u8>> = rels.iter().map(|rel| ((*rel).to_owned(), b"x".to_vec())).collect();
            parse(pack(&manifest, &files, Some(&key())).unwrap())
        };
        let taken = package(&["app.luau", "screens/TEMPLA~1/page.luau", "LUAURC~1", "ds/a~1b.luau", "screenshots/A~1.png", "ds/a~b.luau"]).unwrap();
        assert_eq!(taken.files.keys().collect::<Vec<_>>(), ["app.luau", "ds/a~b.luau"]);
        assert_eq!(taken.ignored, ["LUAURC~1", "ds/a~1b.luau", "screens/TEMPLA~1/page.luau", "screenshots/A~1.png"]);
        for rel in ["screens/tem\u{200c}plates/page.luau", ".luau\u{feff}rc", "account\u{e0041}/x.luau", "ds/\u{fe00}.luau"] {
            let error = package(&["app.luau", rel]).unwrap_err();
            assert!(error.starts_with(BAD_FILE_START), "{rel:?}: {error}");
        }
        // An ordinary emoji's selector is a name's own, and hides no
        // protected one.
        let taken = package(&["app.luau", "ds/\u{2764}\u{fe0f}.luau", "screens/templa\u{fe0f}tes/page.luau", "\u{fe0e}.luaurc"]).unwrap();
        assert_eq!(taken.files.keys().collect::<Vec<_>>(), ["app.luau", "ds/\u{2764}\u{fe0f}.luau"]);
        assert!(package(&["app.luau", "zeron-template\u{fe0f}.json"]).unwrap_err().starts_with(CLASH));
        let d = with_template("names");
        let t = d.join("midnight");
        for rel in ["ds/TOKENS~1.LUA", "ds/to\u{200b}kens.luau", "screenshots/A~1.png"] {
            write(&t.join(rel), "x");
        }
        let (_, files) = contents(&t, "midnight").unwrap();
        assert!(files.keys().all(|rel| !short_name(rel) && !unseen(rel)) && files.contains_key("app.luau"), "{:?}", files.keys());
        // One installed before this is untouched: its own file still counts.
        assert!(templates::own_files(&t).iter().any(|rel| rel == "ds/TOKENS~1.LUA"));
    }

    /// Nothing but pictures is ever installed under `screenshots/`, and the
    /// app's own marks never are.
    #[test]
    fn plugins_template_packages_install_no_code_as_screenshots_nor_the_app_s_marks() {
        let manifest = json!({ "id": "midnight", "name": "Midnight", "description": "", "version": "1.0.0", "zeron_version": "0.3.16", "builtin_theme": "x", "builtin_template": "zeron-v2" });
        let files: BTreeMap<String, Vec<u8>> = ["app.luau", "screenshots/a.PNG", "screenshots/x.luau", "Screenshots/y.luau"].map(|rel| (rel.to_owned(), b"x".to_vec())).into();
        let mine = key();
        let package = parse(pack(&manifest, &files, Some(&mine)).unwrap()).unwrap();
        assert_eq!(package.files.keys().collect::<Vec<_>>(), ["app.luau", "screenshots/a.PNG"]);
        assert_eq!(package.ignored, ["Screenshots/y.luau", "screenshots/x.luau"]);
        let d = dir("no-code").join("templates");
        std::fs::create_dir_all(&d).unwrap();
        let id = install(&d, &package, &Author::You, None).unwrap();
        let written: Value = serde_json::from_slice(&std::fs::read(d.join(&id).join(MANIFEST)).unwrap()).unwrap();
        assert!(written.get(BUILTIN_THEME).is_none() && written.get(BUILTIN_TEMPLATE).is_none(), "{written}");
    }

    /// An export carries the store's files and fields (never the README
    /// Zeron wrote, which names a folder of its author's); they're installed
    /// beside the interface, are no part of it, and changing them alone
    /// isn't a change of its code.
    #[test]
    fn plugins_template_packages_carry_the_listing_beside_the_interface() {
        let d = with_template("listing");
        let t = d.join("midnight");
        write(&t.join("CHANGELOG.md"), "## 1.0.0\n- First.");
        write(&t.join("screenshots/chat.png"), "png");
        write(
            &t.join(MANIFEST),
            r#"{"id":"midnight","name":"Midnight","description":"Dark.","author":"you","version":"1.0.0","zeron_version":"0.9.0","keywords":["dark"],"screenshots":["screenshots/chat.png"],"license":"MIT","builtin_template":"zeron-v2","icon":"x.svg"}"#,
        );
        let (manifest, files) = contents(&t, "midnight").unwrap();
        for rel in ["README.md", "CHANGELOG.md", "screenshots/chat.png", PREVIEW, "app.luau"] {
            assert!(files.contains_key(rel), "{rel}");
        }
        assert_eq!((&manifest["keywords"][0], &manifest["license"]), (&json!("dark"), &json!("MIT")));
        assert!(manifest.get("builtin_template").is_none() && manifest.get("icon").is_none() && manifest.get("author").is_none());
        assert!(!templates::own_files(&t).iter().any(|rel| listing::template_listing_file(rel)));
        write(&t.join("README.md"), &super::super::guides::template_readme("Midnight", Path::new("/home/me/.zeron/docs/templates.md")));
        assert!(!contents(&t, "midnight").unwrap().1.contains_key("README.md"));

        let alice = key();
        let package = |files: &BTreeMap<String, Vec<u8>>| parse(pack(&manifest, files, Some(&alice)).unwrap()).unwrap();
        // (As installed: without what no template replaces.)
        for rel in ["screens/plugins", "account"] {
            std::fs::remove_dir_all(t.join(rel)).unwrap();
        }
        std::fs::remove_file(t.join(".luaurc")).unwrap();
        let mut same_code = files.clone();
        same_code.insert("CHANGELOG.md".into(), b"## 1.1.0\n- More.".to_vec());
        same_code.insert("screenshots/two.png".into(), b"png".to_vec());
        assert!(!changes_code(&t, &package(&same_code)), "only its listing changed");
        let mut code = files.clone();
        code.insert("app.luau".into(), b"-- other".to_vec());
        assert!(changes_code(&t, &package(&code)));
    }

    /// A package the store serves under a record naming no signer (a deploy
    /// token's): taken unsigned, by its contents; a signed one there is
    /// refused, and unsigned is refused anywhere else. Its content hash
    /// covers every file it carries, as a plugin's does.
    #[test]
    fn plugins_template_packages_from_the_store_unsigned_by_their_contents() {
        let d = with_template("store-unsigned");
        let out = d.parent().unwrap().join("midnight.zeron-template");
        export(&d, "midnight", &out, &key()).unwrap();
        let signed = std::fs::read(&out).unwrap();
        let mut value = package_json(&out);
        value["manifest"].as_object_mut().unwrap().remove("signature");
        let unsigned = serde_json::to_vec(&value).unwrap();

        let taken = parse_from_store(unsigned.clone(), false).unwrap();
        assert_eq!((taken.key.as_deref(), taken.id()), (None, "midnight".to_owned()));
        let mut manifest = value["manifest"].clone();
        manifest.as_object_mut().unwrap().remove("signature");
        let carried: BTreeMap<String, Vec<u8>> = value["files"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(rel, b64)| (rel.clone(), B64.decode(b64.as_str().unwrap()).unwrap()))
            .collect();
        assert_eq!(taken.content_hash, signing::content_hash_of(&manifest, &package::hashes(&carried)));
        // The same contents signed hash the same: the signature isn't part of it.
        assert_eq!(parse(signed.clone()).unwrap().content_hash, taken.content_hash);

        assert_eq!(refusal(&parse_from_store(signed.clone(), false).unwrap_err()), Some(install::reason::STORE_MISMATCH));
        assert_eq!(refusal(&parse(unsigned.clone()).unwrap_err()), Some(install::reason::UNSIGNED));
        assert_eq!(refusal(&parse_from_store(unsigned, true).unwrap_err()), Some(install::reason::UNSIGNED));
        assert!(parse_from_store(signed, true).unwrap().key.is_some());
    }

    fn store_source(hash: &str, publisher_id: &str, verified: bool) -> Source {
        let store = install::StoreSource { id: "midnight".into(), publisher: "ada".into(), publisher_id: publisher_id.into(), hash: hash.into(), verified, verified_at: verified.then_some(7), ..Default::default() };
        Source { kind: install::STORE.into(), url: None, path: None, git_ref: None, commit: None, version: "1.0.0".into(), installed_at: 0, store: Some(store) }
    }

    const ADA: &str = "pub_0123456789abcdef0123456789abcdef";

    /// A store template, unsigned, with a file no template replaces.
    fn from_the_store(version: &str) -> Package {
        let manifest = json!({ "id": "midnight", "name": "Midnight", "description": "Dark.", "version": version, "zeron_version": "0.3.16", "keywords": ["dark"] });
        let files = [("app.luau", "x"), ("ds/tokens.luau", "t"), ("README.md", "# Midnight"), ("preview.png", "p"), ("screens/plugins/x.luau", "never"), ("screenshots/a.png", "s")];
        let files = files.map(|(rel, text)| (rel.to_owned(), text.as_bytes().to_vec())).into();
        parse_from_store(pack(&manifest, &files, None).unwrap(), false).unwrap()
    }

    /// A row's mark is the store's version's alone: whatever's changed there
    /// since takes it away, and says so.
    #[test]
    fn plugins_template_packages_a_mark_is_the_store_s_version_s() {
        let d = dir("mark").join("templates");
        std::fs::create_dir_all(d.join(templates::ZERON_DIR)).unwrap();
        write(&d.join(templates::ZERON_DIR).join(MANIFEST), r#"{"zeron_version":"0.9.0"}"#);
        let package = from_the_store("1.0.0");
        assert_eq!(package.ignored, ["screens/plugins/x.luau"]);
        let source = store_source(&package.content_hash, ADA, true);
        let t = d.join("midnight");
        let install = || {
            let _ = std::fs::remove_dir_all(&t);
            super::install(&d, &package, &Author::unknown("x"), Some(&source)).unwrap();
        };
        let row = || templates::list(&d).unwrap()["templates"].as_array().unwrap().iter().find(|t| t["id"] == "midnight").unwrap().clone();
        install();
        let first = row();
        let shown = &first["imported"]["source"];
        assert_eq!((&shown["kind"], &shown["id"], &shown["verified"], &shown["verified_at"]), (&json!("store"), &json!("midnight"), &json!(true), &json!(7)), "{first}");
        assert!(first.get("edited").is_none() && first["imported"].get("left_out").is_none() && shown.get("store").is_none(), "{first}");
        assert_eq!((first["imported"]["author"]["username"].as_str(), &first["imported"]["key"]), (Some("ada"), &Value::Null));
        assert_eq!(row(), first, "the same when nothing changed");
        let changes: [(&str, &dyn Fn()); 9] = [
            ("a file changed", &|| write(&t.join("ds/tokens.luau"), "other")),
            ("a file added", &|| write(&t.join("ds/more.luau"), "x")),
            ("a file gone", &|| std::fs::remove_file(t.join("ds/tokens.luau")).unwrap()),
            ("its README", &|| write(&t.join("README.md"), "# Other")),
            ("a picture", &|| write(&t.join("screenshots/b.png"), "x")),
            ("the file that was left out, put there", &|| write(&t.join("screens/plugins/x.luau"), "never")),
            ("its name", &|| write(&t.join(MANIFEST), r#"{"id":"midnight","name":"Official","description":"Dark.","version":"1.0.0","zeron_version":"0.3.16","keywords":["dark"]}"#)),
            ("its manifest can't be read", &|| write(&t.join(MANIFEST), "nope")),
            ("what it left out can't be read", &|| {
                let mut record = imported(&t).unwrap();
                record["left_out"] = json!("nope");
                write(&t.join(IMPORTED), &record.to_string());
            }),
        ];
        for (what, change) in changes {
            install();
            assert_eq!((edited(&t), edited_for_row(&t)), (Some(false), Some(false)), "{what}");
            change();
            assert_eq!((edited(&t), edited_for_row(&t)), (Some(true), Some(true)), "{what}");
            let row = row();
            let shown = &row["imported"]["source"];
            assert_eq!((&row["edited"], &shown["verified"], shown.get("verified_at")), (&json!(true), &json!(false), None), "{what}: {row}");
        }
        // A name Zeron doesn't write, which the app may make a file of (a
        // backslash's a separator to it), and what isn't a file.
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt as _;
            for name in [&b"screens\\sidebar.luau"[..], b"app.luau.", b"CON.luau", b"x\xff.luau"] {
                install();
                std::fs::write(t.join(std::ffi::OsStr::from_bytes(name)), "x").unwrap();
                assert_eq!((edited(&t), edited_for_row(&t)), (Some(true), Some(true)), "{name:?}");
            }
            install();
            std::os::unix::fs::symlink(t.join("app.luau"), t.join("more.luau")).unwrap();
            assert_eq!(edited(&t), Some(true));
            install();
            write(&t.join("deep/er/a\\b.luau"), "x");
            assert_eq!(edited(&t), Some(true));
            // The one dot name that counts, as a link; and a folder of nothing.
            install();
            std::os::unix::fs::symlink("/etc/hostname", t.join(".luaurc")).unwrap();
            assert_eq!((edited(&t), edited_for_row(&t)), (Some(true), Some(true)));
            install();
            std::fs::create_dir_all(t.join("empty/er")).unwrap();
            assert_eq!((edited(&t), edited_for_row(&t)), (Some(false), Some(false)));
            install();
            write(&t.join(".hidden.luau"), "x");
            assert_eq!((edited(&t), edited_for_row(&t)), (Some(false), Some(false)), "never composed");
        }
        // What Zeron keeps beside it is no part of it.
        install();
        write(&t.join(templates::BASE), "{}");
        write(&t.join(super::super::template_blocks::LINEAGE), "[]");
        assert_eq!(edited(&t), Some(false));
        // A record that isn't the store's says nothing of it, whatever it holds.
        let mut record = imported(&t).unwrap();
        record["source"]["kind"] = json!("file");
        record["mine"] = json!(true);
        write(&t.join(IMPORTED), &record.to_string());
        let row = row();
        assert_eq!((edited(&t), &row["imported"]["source"], row["imported"].get("mine")), (None, &json!({ "kind": "file" }), None), "{row}");
    }

    /// A row doesn't read again what hasn't moved; whatever has, it does, a
    /// file put back to its size and time among them. A consent window reads
    /// everything.
    #[cfg(unix)]
    #[test]
    fn plugins_template_packages_a_row_reads_what_moved() {
        let d = dir("row-reads").join("templates");
        std::fs::create_dir_all(&d).unwrap();
        let package = from_the_store("1.0.0");
        let t = d.join("midnight");
        super::install(&d, &package, &Author::unknown("x"), Some(&store_source(&package.content_hash, ADA, true))).unwrap();
        let kept = || hashed().get(&t).map_or(0, BTreeMap::len);
        assert_eq!((edited_for_row(&t), kept()), (Some(false), 0), "written just now: none is kept");
        std::thread::sleep(std::time::Duration::from_millis(3100));
        assert_eq!((edited_for_row(&t), kept()), (Some(false), 5));
        // (What's kept is what it answers by: a wrong hash shows.)
        hashed().get_mut(&t).unwrap().get_mut("app.luau").unwrap().1 = "0".repeat(64);
        assert_eq!((edited_for_row(&t), edited(&t)), (Some(true), Some(false)));
        let next = from_the_store("1.1.0");
        stage_as(&d, &next, "store", &store_source(&next.content_hash, ADA, true), Some("midnight"), false).unwrap();
        hashed().get_mut(&t).unwrap().get_mut("app.luau").unwrap().1 = "0".repeat(64);
        assert!(pending(&d)[0].get("replaces_edits").is_none(), "{:?}", pending(&d));
        hashed().remove(&t);
        assert_eq!(edited_for_row(&t), Some(false));
        // The same size, its time put back.
        let file = t.join("app.luau");
        let was = std::fs::metadata(&file).unwrap().modified().unwrap();
        write(&file, "y");
        std::fs::File::options().write(true).open(&file).unwrap().set_modified(was).unwrap();
        assert_eq!((edited_for_row(&t), edited(&t)), (Some(true), Some(true)));
        // Another file in its place, the same size and time.
        write(&t.join("other"), "x");
        std::fs::rename(t.join("other"), &file).unwrap();
        std::fs::File::options().write(true).open(&file).unwrap().set_modified(was).unwrap();
        assert_eq!(edited_for_row(&t), Some(false));
        std::fs::remove_file(&file).unwrap();
        std::os::unix::fs::symlink("/etc/hostname", &file).unwrap();
        assert_eq!(edited_for_row(&t), Some(true));
    }

    /// A waiting package says what it is itself, and what Zeron checked of
    /// it: nothing the file beside it adds.
    #[test]
    fn plugins_template_packages_waiting_say_what_was_checked() {
        let d = dir("waiting").join("templates");
        std::fs::create_dir_all(d.join(templates::ZERON_DIR)).unwrap();
        write(&d.join(templates::ZERON_DIR).join(MANIFEST), r#"{"zeron_version":"0.9.0"}"#);
        let package = from_the_store("1.0.0");
        let name = stage(&d, &package, "store", &store_source(&package.content_hash, ADA, true)).unwrap();
        let about = d.join(PENDING).join(format!("{name}.json"));
        let forge = |members: Value| {
            let mut v: Value = serde_json::from_slice(&std::fs::read(&about).unwrap()).unwrap();
            for (k, value) in members.as_object().unwrap() {
                v[k] = value.clone();
            }
            write(&about, &v.to_string());
        };
        let one = || pending(&d).into_iter().next().unwrap();
        let first = one();
        let digest = zeron_plugin_guard::template::digest(&package.files);
        assert_eq!((first["digest"].as_str(), first["hash"].as_str(), &first["key"]), (Some(digest.as_str()), Some(package.hash.as_str()), &Value::Null));
        assert_eq!(first["store"], json!({ "id": "midnight", "version": "1.0.0", "publisher": "ada", "publisher_id": ADA, "verified": true, "verified_at": 7, "verified_alternative": null }));
        assert_eq!(first["author"]["username"], "ada");
        for absent in ["origin", "new_owner", "replaces_edits", "also_installed", "update_of"] {
            assert!(first.get(absent).is_none(), "{absent}: {first}");
        }
        forge(json!({ "name": "Zeron", "description": "x", "version": "9.9.9", "files": 1, "ignored": [], "hash": "h", "digest": "d", "author": { "kind": "you" }, "new_owner": true, "replaces_edits": true, "also_installed": [], "store": { "verified": true }, "extra": 1 }));
        assert_eq!(one(), first);
        // A signer its package hasn't: it can't be allowed, so it isn't listed.
        forge(json!({ "key": "k" }));
        assert!(pending(&d).is_empty());
        forge(json!({ "key": null, "update_of": "../x" }));
        assert!(pending(&d).is_empty(), "nor one that updates what no template is");
        forge(json!({ "update_of": null }));
        // Not verified: the one that is, instead.
        let mut source = store_source(&package.content_hash, ADA, false);
        source.store.as_mut().unwrap().verified_alternative = Some(install::Alternative { version: "0.9.0".into(), added: vec![], removed: vec![], notes: String::new(), verified_at: None });
        forge(json!({ "origin": source }));
        assert_eq!((&one()["store"]["verified"], &one()["store"]["verified_alternative"]["version"], one()["store"].get("verified_at")), (&json!(false), &json!("0.9.0"), None));
        // Under a record of something else, it can't be allowed: not listed.
        forge(json!({ "origin": store_source(&"0".repeat(64), ADA, true) }));
        assert!(pending(&d).is_empty());
        forge(json!({ "origin": store_source(&package.content_hash, ADA, true) }));
        // Allowed, it says what it's kept by.
        let now = store_source(&package.content_hash, ADA, true).store;
        let done = decide(&d, &name, true, &package.hash, now.as_ref(), |_| Author::unknown("x")).unwrap();
        assert_eq!((done["installed"].as_str(), done["digest"].as_str()), (Some("midnight"), Some(digest.as_str())));
        // Another of its id, then an update of the first: by a new publisher, over an edit.
        let again = from_the_store("1.1.0");
        let source = store_source(&again.content_hash, "pub_other", true);
        stage(&d, &again, "store", &source).unwrap();
        let beside = one();
        assert_eq!(beside["also_installed"], json!([{ "id": "midnight", "from": { "kind": "store", "id": "midnight", "publisher": "ada", "verified": true, "verified_at": 7 } }]), "{beside}");
        std::fs::remove_dir_all(d.join(PENDING)).unwrap();
        stage_as(&d, &again, "store", &source, Some("midnight"), false).unwrap();
        let update = one();
        assert_eq!((&update["update_of"], &update["new_owner"], update.get("replaces_edits"), update.get("also_installed")), (&json!("midnight"), &json!(true), None, None));
        write(&d.join("midnight/app.luau"), "mine");
        assert_eq!(one()["replaces_edits"], true);
        // Its template gone: listed, to be declined, and compared with nothing.
        std::fs::rename(d.join("midnight"), d.join("elsewhere")).unwrap();
        let gone = one();
        assert_eq!((&gone["update_of"], gone.get("also_installed"), gone.get("replaces_edits"), gone.get("new_owner")), (&json!("midnight"), None, None, None), "{gone}");
        std::fs::rename(d.join("elsewhere"), d.join("midnight")).unwrap();
        // Its package gone bad isn't listed.
        let pending_package = std::fs::read_dir(d.join(PENDING)).unwrap().flatten().find(|e| e.path().extension().is_some_and(|x| x == EXTENSION)).unwrap().path();
        write(&pending_package, "nope");
        assert!(pending(&d).is_empty());
    }

    /// Each guard alone: an update's record and its kept package, one taken
    /// down here, and an older version staged or decided without the person's
    /// choice of it.
    #[test]
    fn plugins_template_packages_each_guard_of_the_store_s_updates_holds_alone() {
        let d = dir("guards").join("templates");
        std::fs::create_dir_all(d.join(templates::ZERON_DIR)).unwrap();
        write(&d.join(templates::ZERON_DIR).join(MANIFEST), r#"{"zeron_version":"0.9.0"}"#);
        let of = |version: &str, readme: &str| {
            let manifest = json!({ "id": "midnight", "name": "Midnight", "description": "Dark.", "version": version, "zeron_version": "0.0.1" });
            let files = BTreeMap::from([("app.luau".to_owned(), b"x".to_vec()), ("README.md".to_owned(), readme.as_bytes().to_vec())]);
            parse_from_store(pack(&manifest, &files, None).unwrap(), false).unwrap()
        };
        let (was, next) = (of("1.0.0", "# One"), of("1.1.0", "# Two"));
        super::install(&d, &was, &Author::unknown("x"), Some(&store_source(&was.content_hash, ADA, true))).unwrap();
        let folder = d.join("midnight");
        let hash = imported(&folder).unwrap()["hash"].as_str().unwrap().to_owned();
        let found = Found { from: hash, version: "1.1.0".into(), hash: next.hash.clone(), source: Some(store_source(&next.content_hash, ADA, true)), ..Found::default() };
        keep_found(&d, "midnight", &found, Some(&next)).unwrap();
        let now = store_source(&next.content_hash, ADA, true).store.unwrap();
        let untouched = || imported(&folder).unwrap()["version"] == "1.0.0";
        for other in [install::StoreSource { hash: "0".repeat(64), ..now.clone() }, install::StoreSource { id: "other".into(), ..now.clone() }, install::StoreSource { key: Some("k".into()), ..now.clone() }] {
            assert_eq!(update_from_store(&d, "midnight", &other, None).unwrap_err(), NOT_CHECKED);
        }
        // The package kept isn't the one found.
        let kept = std::fs::read(found_package_path(&d, "midnight")).unwrap();
        write(&found_package_path(&d, "midnight"), std::str::from_utf8(&of("1.1.0", "# Other").raw).unwrap());
        assert!(update_from_store(&d, "midnight", &now, None).unwrap_err().contains("changed since it was found"));
        std::fs::write(found_package_path(&d, "midnight"), &kept).unwrap();
        // Taken down here.
        use super::super::template_blocks::{self, Block, Change};
        let block = Block { id: "midnight".into(), version: "1.1.0".into(), hash: None, reason: "Bad.".into(), at: super::super::now_secs() };
        template_blocks::apply(&d, vec![Change::TakenDown(block)], super::super::now_secs()).unwrap();
        assert!(update_from_store(&d, "midnight", &now, None).unwrap_err().starts_with("taken_down: "));
        template_blocks::apply(&d, vec![Change::Restored("midnight".into(), "1.1.0".into())], super::super::now_secs()).unwrap();
        assert!(untouched() && pending(&d).is_empty());
        assert_eq!(update_from_store(&d, "midnight", &now, None).unwrap()["updated"], "midnight");
        // Over an unverified one now: an older one that isn't verified, though
        // it was chosen; a verified one that wasn't.
        let mut record = imported(&folder).unwrap();
        record["source"]["store"]["verified"] = json!(false);
        write(&folder.join(IMPORTED), &record.to_string());
        let stage = |verified: bool, instead: bool| stage_from_store(&d, &was, store_source(&was.content_hash, ADA, verified), Some("midnight"), instead);
        assert!(stage(false, true).unwrap_err().starts_with("not_newer: "));
        assert!(stage(true, false).unwrap_err().starts_with("not_newer: "));
        let mut source = store_source(&was.content_hash, ADA, true);
        source.store.as_mut().unwrap().replaced = Some(install::ReplacedVersion { version: "1.1.0".into(), verified: false });
        let unchosen = stage_as(&d, &was, "store", &source, Some("midnight"), false).unwrap();
        let record = store_source(&was.content_hash, ADA, true).store;
        let allow = |name: &str| decide(&d, name, true, &was.hash, record.as_ref(), |_| Author::unknown("x"));
        assert!(allow(&unchosen).unwrap_err().starts_with("not_newer: "));
        let chosen = stage(true, true).unwrap();
        // What's here is another version by now: asked for again, it's another
        // item, saying so.
        let was_there = imported(&folder).unwrap();
        let mut record_then = was_there.clone();
        record_then["version"] = json!("1.0.5");
        write(&folder.join(IMPORTED), &record_then.to_string());
        let again = stage(true, true).unwrap();
        assert_eq!((again["pending"] != chosen["pending"], &again["from"]["replaced"]["version"]), (true, &json!("1.0.5")), "{again}");
        write(&folder.join(IMPORTED), &was_there.to_string());
        assert_eq!(allow(chosen["pending"].as_str().unwrap()).unwrap()["updated"], "midnight");
    }

    /// What's put in a template's place waits for whatever else is being: a
    /// verified older one chosen over what's here never lands over one just
    /// updated, whichever is first.
    #[test]
    fn plugins_template_packages_are_put_in_place_one_at_a_time() {
        let d = dir("one-at-a-time").join("templates");
        std::fs::create_dir_all(d.join(templates::ZERON_DIR)).unwrap();
        write(&d.join(templates::ZERON_DIR).join(MANIFEST), r#"{"zeron_version":"0.9.0"}"#);
        for _ in 0..10 {
            let _ = std::fs::remove_dir_all(d.join("midnight"));
            let _ = std::fs::remove_dir_all(d.join(PENDING));
            let _ = std::fs::remove_file(d.join(super::super::template_blocks::FILE));
            let (old, now, next) = (from_the_store("1.0.0"), from_the_store("1.1.0"), from_the_store("1.2.0"));
            super::install(&d, &now, &Author::unknown("x"), Some(&store_source(&now.content_hash, ADA, false))).unwrap();
            let older = stage_from_store(&d, &old, store_source(&old.content_hash, ADA, true), Some("midnight"), true).unwrap();
            let newer = stage_from_store(&d, &next, store_source(&next.content_hash, ADA, false), Some("midnight"), false).unwrap();
            let allow = |item: &Value, package: &Package, verified: bool| {
                let record = store_source(&package.content_hash, ADA, verified).store;
                decide(&d, item["pending"].as_str().unwrap(), true, &package.hash, record.as_ref(), |_| Author::unknown("x"))
            };
            let (first, second) = std::thread::scope(|s| {
                let a = s.spawn(|| allow(&older, &old, true));
                let b = s.spawn(|| allow(&newer, &next, false));
                (a.join().unwrap(), b.join().unwrap())
            });
            // (The newer one fits over either; the older only over what it named.)
            assert!(second.is_ok(), "{second:?}");
            assert_eq!(imported(&d.join("midnight")).unwrap()["version"], "1.2.0", "{first:?}");
        }
        let held = changing();
        let (d2, next) = (d.clone(), from_the_store("1.3.0"));
        let waits = std::thread::spawn(move || stage_from_store(&d2, &next, store_source(&next.content_hash, ADA, false), None, false).is_ok());
        std::thread::sleep(std::time::Duration::from_millis(150));
        assert!(!waits.is_finished());
        drop(held);
        assert!(waits.join().unwrap());
    }

    /// What waits and can't be allowed is dropped; what can be, what's being
    /// looked at and what isn't a waiting package stay.
    #[test]
    fn plugins_template_packages_drop_what_cannot_be_allowed() {
        let d = dir("unusable").join("templates");
        let package = from_the_store("1.0.0");
        let source = store_source(&package.content_hash, ADA, true);
        let stage = || stage_as(&d, &package, "store", &source, None, false).unwrap();
        let (good, bad_package, bad_record, bad_update, named) = (stage(), stage(), stage(), stage(), stage());
        let at = |name: &str, ext: &str| d.join(PENDING).join(format!("{name}.{ext}"));
        write(&at(&bad_package, EXTENSION), "nope");
        let forge = |name: &str, member: &str, value: Value| {
            let mut v: Value = serde_json::from_slice(&std::fs::read(at(name, "json")).unwrap()).unwrap();
            v[member] = value;
            write(&at(name, "json"), &v.to_string());
        };
        forge(&bad_record, "origin", json!(store_source(&"0".repeat(64), ADA, true)));
        forge(&bad_update, "update_of", json!("../x"));
        forge(&named, "pending", json!("other-1"));
        write(&at("half-1", "json"), &json!({ "pending": "half-1" }).to_string());
        write(&d.join(PENDING).join("notes.txt"), "mine");
        drop_unusable(&d);
        let left = |name: &str| (at(name, "json").exists(), at(name, EXTENSION).exists());
        assert_eq!(left(&good), (true, true));
        for name in [&bad_package, &bad_record, &bad_update, &named] {
            assert_eq!(left(name), (false, false), "{name}");
        }
        assert!(at("half-1", "json").exists() && d.join(PENDING).join("notes.txt").exists());
        assert_eq!(pending(&d).len(), 1);
        drop_unusable(&d.join("none"));
    }

    /// What's shown of a package is bounded, wherever it came from; what's
    /// signed and kept is as it is.
    #[test]
    fn plugins_template_packages_show_bounded_words() {
        let d = dir("bounded").join("templates");
        std::fs::create_dir_all(d.join(templates::ZERON_DIR)).unwrap();
        write(&d.join(templates::ZERON_DIR).join(MANIFEST), r#"{"zeron_version":"0.9.0"}"#);
        let long = "n".repeat(5000);
        let manifest = json!({ "id": "midnight", "name": format!("Mid\u{202e}\u{200b}night\n{long}"), "description": format!("\u{2764}\u{fe0f} {long}"), "version": long, "zeron_version": long });
        let mine = key();
        let raw = pack(&manifest, &BTreeMap::from([("app.luau".to_owned(), b"x".to_vec())]), Some(&mine)).unwrap();
        let path = d.parent().unwrap().join("x.zeron-template");
        std::fs::write(&path, &raw).unwrap();
        let check = |v: &Value| {
            assert_eq!(v["name"].as_str().unwrap(), format!("Midnight {}", "n".repeat(64 - 9)), "{v}");
            assert_eq!(v["description"].as_str().unwrap(), format!("\u{2764}\u{fe0f} {}", "n".repeat(300 - 3)));
            assert_eq!((v["version"].as_str().unwrap().len(), v["zeron_version"].as_str().unwrap().len()), (version::MAX_LEN, version::MAX_LEN));
        };
        let waiting = import(&d, &path, |_| Author::unknown("x")).unwrap();
        check(&waiting);
        check(&pending(&d)[0]);
        let done = decide(&d, waiting["pending"].as_str().unwrap(), true, waiting["hash"].as_str().unwrap(), None, |_| Author::unknown("x")).unwrap();
        assert!(done["name"].as_str().unwrap().len() <= 64, "{done}");
        let listed = templates::list(&d).unwrap();
        let row = listed["templates"].as_array().unwrap().iter().find(|t| t["id"] == "midnight").unwrap();
        check(row);
        assert_eq!(row["imported"]["version"].as_str().unwrap().len(), version::MAX_LEN);
        for (text, max, expected) in [
            ("Midnight\u{2028}Verified", 64, "Midnight Verified"),
            ("a\u{2029}\u{2800} \t\u{3000}b \n ", 64, "a b"),
            ("  a", 64, "a"),
            ("a b", 2, "a"),
            ("a b", 3, "a b"),
            ("\u{200b}a\u{e0041}\u{202e}b", 64, "ab"),
            ("", 64, ""),
        ] {
            assert_eq!(listing::shown(text, max), expected, "{text:?}");
        }
        // A forged record's words are bounded wherever they're shown.
        let mut record = imported(&d.join("midnight")).unwrap();
        let mut source = store_source("h", &long, true);
        let store = source.store.as_mut().unwrap();
        (store.publisher, store.id) = (long.clone(), long.clone());
        (source.url, source.git_ref) = (Some(long.clone()), Some(long.clone()));
        record["source"] = json!(source);
        record["author"] = json!(long);
        write(&d.join("midnight").join(IMPORTED), &record.to_string());
        let row = templates::list(&d).unwrap()["templates"].as_array().unwrap().iter().find(|t| t["id"] == "midnight").unwrap()["imported"].clone();
        assert_eq!((row["author"]["username"].as_str().map(str::len), row["source"]["publisher"].as_str().map(str::len)), (Some(64), Some(64)), "{row}");
        assert!(row["source"]["url"].as_str().unwrap().len() <= 1024 && row["source"]["id"].as_str().unwrap().len() <= 40);
        write(&d.join("mine").join(MANIFEST), &json!({ "name": "Mine", "author": long }).to_string());
        assert_eq!(templates::list(&d).unwrap()["templates"].as_array().unwrap().iter().find(|t| t["id"] == "mine").unwrap()["author"].as_str().unwrap().len(), 64);
        let kept: Value = serde_json::from_slice(&std::fs::read(d.join("midnight").join(MANIFEST)).unwrap()).unwrap();
        assert_eq!(kept["name"], manifest["name"], "what's signed is as it is");
        assert_eq!(imported(&d.join("midnight")).unwrap()["version"].as_str().unwrap(), long);
    }

    #[test]
    fn plugins_template_packages_round_trip_with_protected_files_left_out() {
        let d = with_template("round");
        let out = d.parent().unwrap().join("midnight.zeron-template");
        let alice = key();
        let made = export(&d, "midnight", &out, &alice).unwrap();
        assert_eq!(made["files"], 5);
        assert_eq!(made["key"], alice.public());
        // What it carries: the app files, its preview and its README; never
        // the protected ones, its base or an author.
        let package = package_json(&out);
        let mut names: Vec<&String> = package["files"].as_object().unwrap().keys().collect();
        names.sort();
        assert_eq!(names, ["README.md", "app.luau", "ds/tokens.luau", PREVIEW, "screens/shell/sidebar.luau"]);
        assert!(package["manifest"].get("author").is_none());
        assert_eq!(package["manifest"]["name"], "Midnight");

        // On Bob's device, it waits for Bob, and nothing is installed yet.
        let bob_dir = dir("round-bob").join("templates");
        let bob = key();
        let waiting = import(&bob_dir, &out, author_as(&bob)).unwrap();
        assert_eq!(waiting["needs_consent"], true);
        assert_eq!(waiting["name"], "Midnight");
        assert_eq!(waiting["key"], alice.public());
        assert_eq!(waiting["author"]["kind"], "unknown");
        assert!(!bob_dir.join("midnight").exists());
        let listed = pending(&bob_dir);
        assert_eq!(listed.len(), 1);
        let name = waiting["pending"].as_str().unwrap();
        assert_eq!(listed[0]["pending"], name);
        assert_eq!(listed[0]["source"], "midnight.zeron-template");

        // Allowed: installed as signed, and no longer waiting.
        let hash = waiting["hash"].as_str().unwrap();
        let done = decide(&bob_dir, name, true, hash, None, author_as(&bob)).unwrap();
        assert_eq!(done["installed"], "midnight");
        let t = bob_dir.join("midnight");
        assert_eq!(std::fs::read_to_string(t.join("ds/tokens.luau")).unwrap(), "-- midnight tokens");
        assert_eq!(std::fs::read_to_string(t.join(PREVIEW)).unwrap(), "png");
        assert!(!t.join("screens/plugins").exists() && !t.join("account").exists() && !t.join(".luaurc").exists());
        let imported = imported(&t).unwrap();
        assert_eq!(imported["key"], alice.public());
        assert_eq!(imported["author"]["kind"], "unknown");
        assert!(pending(&bob_dir).is_empty());
        // Listed with who it's from.
        write(&bob_dir.join(templates::ZERON_DIR).join(MANIFEST), r#"{"zeron_version":"0.9.0"}"#);
        let list = templates::list(&bob_dir).unwrap();
        let entry = list["templates"].as_array().unwrap().iter().find(|t| t["id"] == "midnight").unwrap().clone();
        assert_eq!(entry["imported"]["key"], alice.public());
        assert_eq!(entry["name"], "Midnight");

        // Imported again: beside the first, never over it.
        let again = import(&bob_dir, &out, author_as(&bob)).unwrap();
        let again = decide(&bob_dir, again["pending"].as_str().unwrap(), true, again["hash"].as_str().unwrap(), None, author_as(&bob)).unwrap();
        assert_eq!(again["installed"], "midnight-2");
    }

    #[test]
    fn plugins_template_packages_protected_files_in_a_package_are_ignored() {
        // A package that carries what no template replaces, signed as is
        // (by hand: Zeron's own export never puts them in).
        let d = dir("protected");
        let alice = key();
        let files: BTreeMap<String, Vec<u8>> = [
            ("app.luau", "-- app"),
            ("screens/plugins/page.luau", "-- not yours to replace"),
            ("screens/templates/page.luau", "-- nor this"),
            ("account/sessions.luau", "-- nor this"),
            (".luaurc", "{}"),
        ]
        .into_iter()
        .map(|(rel, text)| (rel.to_owned(), text.as_bytes().to_vec()))
        .collect();
        let hashes: FileHashes = files.iter().map(|(r, b)| (r.clone(), signing::sha256_hex(b))).collect();
        let manifest = alice.sign_manifest_for(TEMPLATE_PAYLOAD_PREFIX, &json!({ "id": "sneaky", "name": "Sneaky" }), &hashes);
        let package = json!({
            "format": FORMAT,
            "manifest": manifest,
            "files": files.iter().map(|(r, b)| (r.clone(), json!(B64.encode(b)))).collect::<serde_json::Map<_, _>>(),
        });
        let path = d.join("sneaky.zeron-template");
        std::fs::write(&path, serde_json::to_vec(&package).unwrap()).unwrap();

        let templates_dir = d.join("templates");
        let done = import(&templates_dir, &path, author_as(&alice)).unwrap();
        assert_eq!(done["installed"], "sneaky");
        let mut ignored: Vec<String> = serde_json::from_value(done["ignored"].clone()).unwrap();
        ignored.sort();
        assert_eq!(ignored, [".luaurc", "account/sessions.luau", "screens/plugins/page.luau", "screens/templates/page.luau"]);
        let t = templates_dir.join("sneaky");
        assert!(t.join("app.luau").is_file());
        for rel in ignored {
            assert!(!t.join(&rel).exists(), "{rel}");
        }
    }

    #[test]
    fn plugins_template_packages_paths_that_fold_alike_are_refused_before_anything_is_written() {
        let alice = key();
        let parsed = |paths: &[&str]| {
            let files: BTreeMap<String, Vec<u8>> = paths.iter().map(|rel| ((*rel).to_owned(), b"-- x".to_vec())).collect();
            let hashes: FileHashes = files.iter().map(|(r, b)| (r.clone(), signing::sha256_hex(b))).collect();
            let manifest = alice.sign_manifest_for(TEMPLATE_PAYLOAD_PREFIX, &json!({ "id": "dusk", "name": "Dusk" }), &hashes);
            let package = json!({
                "format": FORMAT,
                "manifest": manifest,
                "files": files.iter().map(|(r, b)| (r.clone(), json!(B64.encode(b)))).collect::<serde_json::Map<_, _>>(),
            });
            parse(serde_json::to_vec(&package).unwrap())
        };
        for (paths, names) in [
            (&["app.luau", "screens/Home.luau", "screens/home.luau"][..], &["screens/Home.luau", "screens/home.luau"][..]),
            (&["app.luau", "caf\u{e9}.luau", "cafe\u{301}.luau"], &["caf\u{e9}.luau", "cafe\u{301}.luau"]),
            (&["app.luau", "ds", "DS/tokens.luau"], &["ds", "DS/tokens.luau"]),
            (&["app.luau", "Zeron-Template.json"], &["Zeron-Template.json", MANIFEST]),
            (&["app.luau", "zeron-template.j\u{17f}on"], &["zeron-template.j\u{17f}on", MANIFEST]),
        ] {
            let error = parsed(paths).map(|_| ()).unwrap_err();
            assert!(error.starts_with("This template can't be installed:"), "{paths:?}: {error}");
            for name in names {
                assert!(error.contains(&format!("{name:?}")), "{paths:?}: {error}");
            }
        }
        // (Its manifest's exact name, a protected file and names that only
        // look alike are as before: set aside, or anyone's.)
        let fine = parsed(&["app.luau", "screens/home.luau", "screens/home2.luau", "zeron-template.json", "account/x.luau"]).unwrap();
        assert_eq!(fine.files.keys().collect::<Vec<_>>(), ["app.luau", "screens/home.luau", "screens/home2.luau"]);
    }

    #[test]
    fn plugins_template_packages_tampered_or_unsigned_are_refused() {
        let d = with_template("tampered");
        let out = d.parent().unwrap().join("midnight.zeron-template");
        let alice = key();
        export(&d, "midnight", &out, &alice).unwrap();
        let bob_dir = dir("tampered-bob").join("templates");
        let bob = key();
        let original = package_json(&out);
        let refused = |package: &Value, why: &str| {
            std::fs::write(&out, serde_json::to_vec(package).unwrap()).unwrap();
            let error = import(&bob_dir, &out, author_as(&bob)).err().unwrap_or_default();
            assert!(error.contains(why), "{error}");
            assert!(pending(&bob_dir).is_empty() && !bob_dir.join("midnight").exists());
        };

        // A file changed after signing.
        let mut changed = original.clone();
        changed["files"]["ds/tokens.luau"] = json!(B64.encode("-- something else"));
        refused(&changed, "changed after it was signed");
        // A file added.
        let mut added = original.clone();
        added["files"]["screens/extra.luau"] = json!(B64.encode("-- extra"));
        refused(&added, "changed after it was signed");
        // The manifest changed (its name), its signature kept.
        let mut renamed = original.clone();
        renamed["manifest"]["name"] = json!("Totally Official");
        refused(&renamed, "doesn't check out");
        // Signed by another key in its place.
        let mut swapped = original.clone();
        swapped["manifest"]["signature"]["key"] = json!(bob.public());
        refused(&swapped, "doesn't check out");
        // No signature at all.
        let mut unsigned = original.clone();
        unsigned["manifest"].as_object_mut().unwrap().remove("signature");
        refused(&unsigned, "isn't signed");
        // A plugin's signature over the same files isn't a template's.
        let hashes: FileHashes = original["files"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(r, b)| (r.clone(), signing::sha256_hex(&B64.decode(b.as_str().unwrap()).unwrap())))
            .collect();
        let mut as_plugin = original.clone();
        let mut bare = original["manifest"].clone();
        bare.as_object_mut().unwrap().remove("signature");
        as_plugin["manifest"] = alice.sign_manifest(&bare, &hashes);
        refused(&as_plugin, "doesn't check out");
        // Not a package, and a path that would escape.
        refused(&json!({ "hello": "world" }), "isn't a Zeron template");
        let mut escaping = original.clone();
        escaping["files"]["../../evil.luau"] = json!(B64.encode("-- evil"));
        refused(&escaping, "won't write");

        // A package swapped while it waited isn't installed.
        std::fs::write(&out, serde_json::to_vec(&original).unwrap()).unwrap();
        let waiting = import(&bob_dir, &out, author_as(&bob)).unwrap();
        let name = waiting["pending"].as_str().unwrap();
        let staged = bob_dir.join(PENDING).join(format!("{name}.{EXTENSION}"));
        std::fs::write(&staged, serde_json::to_vec(&changed).unwrap()).unwrap();
        let error = decide(&bob_dir, name, true, waiting["hash"].as_str().unwrap(), None, author_as(&bob)).unwrap_err();
        assert!(error.contains("changed after it was signed"), "{error}");
        assert!(!bob_dir.join("midnight").exists());
    }

    #[test]
    fn plugins_template_packages_consent_for_others_not_for_yours() {
        let d = with_template("consent");
        let out = d.parent().unwrap().join("midnight.zeron-template");
        let alice = key();
        export(&d, "midnight", &out, &alice).unwrap();

        // Alice's own, on her other device (same key): installed at once.
        let alice_dir = dir("consent-alice").join("templates");
        let done = import(&alice_dir, &out, author_as(&alice)).unwrap();
        assert_eq!(done["installed"], "midnight");
        assert_eq!(done["author"]["kind"], "you");
        assert!(done.get("needs_consent").is_none());
        assert!(pending(&alice_dir).is_empty());

        // Someone else's waits, and Not now drops it.
        let bob_dir = dir("consent-bob").join("templates");
        let bob = key();
        let waiting = import(&bob_dir, &out, author_as(&bob)).unwrap();
        assert_eq!(waiting["needs_consent"], true);
        let name = waiting["pending"].as_str().unwrap();
        // Only with what was shown: another hash installs nothing.
        assert!(decide(&bob_dir, name, true, "not-what-was-shown", None, author_as(&bob)).is_err());
        assert!(!bob_dir.join("midnight").exists());
        assert_eq!(decide(&bob_dir, name, false, "", None, author_as(&bob)).unwrap(), json!({ "declined": name }));
        assert!(pending(&bob_dir).is_empty());
        assert!(!bob_dir.join("midnight").exists());
        assert!(decide(&bob_dir, name, true, "", None, author_as(&bob)).is_err());
        assert!(decide(&bob_dir, "../../x", true, "", None, author_as(&bob)).is_err());
    }

    #[test]
    fn plugins_template_packages_export_checks_where_and_what() {
        let d = with_template("export");
        let k = key();
        let parent = d.parent().unwrap();
        assert!(export(&d, "zeron", &parent.join("z.zeron-template"), &k).is_err());
        assert!(export(&d, "nope", &parent.join("n.zeron-template"), &k).unwrap_err().contains("no template nope"));
        assert!(export(&d, "midnight", Path::new("rel.zeron-template"), &k).is_err());
        assert!(export(&d, "midnight", &parent.join("m.zip"), &k).is_err());
        assert!(export(&d, "midnight", &parent.join("missing/m.zeron-template"), &k).is_err());
        assert!(export(&d, "midnight", &d.join("midnight/m.zeron-template"), &k).unwrap_err().contains("outside"));
        // Replaces an earlier export.
        let out = parent.join("m.zeron-template");
        export(&d, "midnight", &out, &k).unwrap();
        export(&d, "midnight", &out, &k).unwrap();
        assert!(read(&out).is_ok());
    }

    #[test]
    fn plugins_template_packages_links_name_a_package_or_a_release() {
        let release = |repo: &str, tag: Option<&str>| Origin::Release { repo: repo.into(), tag: tag.map(Into::into) };
        assert_eq!(origin("ada/midnight").unwrap(), release("ada/midnight", None));
        assert_eq!(origin("github.com/ada/midnight").unwrap(), release("ada/midnight", None));
        assert_eq!(origin("https://github.com/ada/midnight/releases/latest").unwrap(), release("ada/midnight", None));
        assert_eq!(origin("https://github.com/ada/midnight/releases/tag/v2").unwrap(), release("ada/midnight", Some("v2")));
        assert_eq!(
            origin("example.com/t/midnight.zeron-template").unwrap(),
            Origin::Link("https://example.com/t/midnight.zeron-template".into())
        );
        // Not a template: a repository's files, a page that isn't one, a
        // plugin, plain http, nothing.
        assert!(origin("https://github.com/ada/midnight/tree/main/t").unwrap_err().contains("isn't a template"));
        assert!(origin("https://example.com/midnight").unwrap_err().contains("isn't a template"));
        assert!(origin("example.com/x/focus.zeron-plugin").unwrap_err().contains("isn't a template"));
        assert!(origin("http://example.com/m.zeron-template").unwrap_err().contains("https"));
        assert!(origin("  ").is_err());
    }

    #[tokio::test]
    async fn plugins_template_packages_update_in_place_from_where_it_came() {
        let d = with_template("update");
        let out = d.parent().unwrap().join("midnight.zeron-template");
        let alice = key();
        export(&d, "midnight", &out, &alice).unwrap();
        // Alice's other device: installed at once, its source the file.
        let other = dir("update-other").join("templates");
        write(&other.join(templates::ZERON_DIR).join(MANIFEST), r#"{"zeron_version":"0.9.0"}"#);
        assert_eq!(import(&other, &out, author_as(&alice)).unwrap()["installed"], "midnight");
        let t = other.join("midnight");
        let first = imported(&t).unwrap();
        assert_eq!(first["source"]["kind"], "file");
        assert_eq!(first["version"], "1.0.0");
        let source: Source = serde_json::from_value(first["source"].clone()).unwrap();
        let hash = first["hash"].as_str().unwrap().to_owned();
        let listed = checkable(&other);
        assert_eq!(listed.len(), 1);
        assert_eq!((listed[0].0.as_str(), listed[0].3.as_str()), ("midnight", hash.as_str()));

        // Nothing new where it came from.
        let (same, at) = fetch_again(&source).await.unwrap().unwrap();
        assert!(compare(&other, "midnight", &alice.public(), &hash, &same, at).is_none());

        // A new version, signed by the same key, that changes its code:
        // found and kept, then it waits for you as someone else's would,
        // and replaces the installed one in place once allowed.
        write(&d.join("midnight/ds/tokens.luau"), "-- midnight tokens, v2");
        write(
            &d.join("midnight").join(MANIFEST),
            r#"{"id":"midnight","name":"Midnight","version":"1.1.0","zeron_version":"0.9.0"}"#,
        );
        export(&d, "midnight", &out, &alice).unwrap();
        let (newer, at) = fetch_again(&source).await.unwrap().unwrap();
        let found_now = compare(&other, "midnight", &alice.public(), &hash, &newer, at).unwrap();
        assert_eq!((found_now.version.as_str(), found_now.refused.as_deref()), ("1.1.0", None));
        keep_found(&other, "midnight", &found_now, Some(&newer)).unwrap();
        assert!(found(&other, "midnight", &hash).is_some());
        let waiting = update(&other, "midnight", author_as(&alice)).unwrap();
        assert_eq!((waiting["needs_consent"].as_bool(), waiting["update_of"].as_str()), (Some(true), Some("midnight")));
        assert_eq!(std::fs::read_to_string(t.join("ds/tokens.luau")).unwrap(), "-- midnight tokens", "nothing changes before you allow it");
        let done = decide(&other, waiting["pending"].as_str().unwrap(), true, waiting["hash"].as_str().unwrap(), None, author_as(&alice)).unwrap();
        assert_eq!((done["installed"].as_str(), done["updated"].as_str()), (Some("midnight"), Some("midnight")));
        assert!(!other.join("midnight-2").exists(), "in place, never beside it");
        assert_eq!(std::fs::read_to_string(t.join("ds/tokens.luau")).unwrap(), "-- midnight tokens, v2");
        let after = imported(&t).unwrap();
        assert_eq!(after["source"]["kind"], "file");
        assert_ne!(after["hash"], json!(hash));
        assert!(found(&other, "midnight", after["hash"].as_str().unwrap()).is_none());
        assert!(update(&other, "midnight", author_as(&alice)).unwrap_err().contains("no update"));

        // Signed by someone else: named, refused, its package never kept.
        let eve = key();
        export(&d, "midnight", &out, &eve).unwrap();
        let installed = after["hash"].as_str().unwrap().to_owned();
        let (theirs, at) = fetch_again(&source).await.unwrap().unwrap();
        let refused = compare(&other, "midnight", &alice.public(), &installed, &theirs, at).unwrap();
        assert!(refused.refused.as_deref().unwrap().contains("someone else"));
        keep_found(&other, "midnight", &refused, None).unwrap();
        assert!(!other.join(UPDATES).join(format!("midnight.{EXTENSION}")).exists());
        assert!(update(&other, "midnight", author_as(&alice)).unwrap_err().contains("someone else"));
        assert_eq!(std::fs::read_to_string(t.join("ds/tokens.luau")).unwrap(), "-- midnight tokens, v2");
    }

    /// R23: a package signed by the same key but not a later version (a
    /// signed downgrade, or the same version again) is refused, found or
    /// installed.
    #[tokio::test]
    async fn plugins_template_packages_a_signed_downgrade_never_updates() {
        let d = with_template("downgrade");
        let out = d.parent().unwrap().join("midnight.zeron-template");
        let alice = key();
        write(&d.join("midnight").join(MANIFEST), r#"{"id":"midnight","name":"Midnight","version":"2.0.0"}"#);
        export(&d, "midnight", &out, &alice).unwrap();
        let other = dir("downgrade-other").join("templates");
        import(&other, &out, author_as(&alice)).unwrap();
        let first = imported(&other.join("midnight")).unwrap();
        let (hash, source): (String, Source) = (first["hash"].as_str().unwrap().into(), serde_json::from_value(first["source"].clone()).unwrap());
        // The old 1.0.0, signed again by the same key.
        write(&d.join("midnight").join(MANIFEST), r#"{"id":"midnight","name":"Midnight","version":"1.0.0"}"#);
        write(&d.join("midnight/ds/tokens.luau"), "-- the old, vulnerable tokens");
        export(&d, "midnight", &out, &alice).unwrap();
        let (older, at) = fetch_again(&source).await.unwrap().unwrap();
        let found_now = compare(&other, "midnight", &alice.public(), &hash, &older, at).unwrap();
        assert!(found_now.refused.as_deref().unwrap().contains("isn't later"), "{:?}", found_now.refused);
        // Kept anyway (an .updates file written by hand): update refuses it.
        let planted = Found { refused: None, ..found_now };
        keep_found(&other, "midnight", &planted, Some(&older)).unwrap();
        assert!(update(&other, "midnight", author_as(&alice)).unwrap_err().contains("isn't later"));
        assert_eq!(std::fs::read_to_string(other.join("midnight/ds/tokens.luau")).unwrap(), "-- midnight tokens");
    }

    /// R23: a template installed beside another of its id (`midnight-2`)
    /// still updates: it's told by its package's id, not its folder's.
    #[tokio::test]
    async fn plugins_template_packages_one_installed_under_a_free_id_still_updates() {
        let d = with_template("want2");
        let out = d.parent().unwrap().join("midnight.zeron-template");
        let alice = key();
        export(&d, "midnight", &out, &alice).unwrap();
        let other = dir("want2-other").join("templates");
        write(&other.join("midnight").join(MANIFEST), r#"{"id":"midnight","name":"Mine"}"#);
        assert_eq!(import(&other, &out, author_as(&alice)).unwrap()["installed"], "midnight-2");
        let first = imported(&other.join("midnight-2")).unwrap();
        let (hash, source): (String, Source) = (first["hash"].as_str().unwrap().into(), serde_json::from_value(first["source"].clone()).unwrap());
        write(&d.join("midnight").join(MANIFEST), r#"{"id":"midnight","name":"Midnight","version":"1.1.0"}"#);
        export(&d, "midnight", &out, &alice).unwrap();
        let (newer, at) = fetch_again(&source).await.unwrap().unwrap();
        let found_now = compare(&other, "midnight-2", &alice.public(), &hash, &newer, at).unwrap();
        assert_eq!(found_now.refused, None, "the same template, under a free id");
        keep_found(&other, "midnight-2", &found_now, Some(&newer)).unwrap();
        // (Its code is the same: in place, no asking.)
        let done = update(&other, "midnight-2", author_as(&alice)).unwrap();
        assert_eq!(done["updated"], "midnight-2");
        assert_eq!(imported(&other.join("midnight-2")).unwrap()["version"], "1.1.0");
    }

    /// R18: a package's manifest never carries the app's mark of Zeron's
    /// own built-in themes into what's installed.
    #[test]
    fn plugins_template_packages_builtin_theme_is_never_installed_from_a_package() {
        let d = with_template("posing");
        let alice = key();
        let out = d.parent().unwrap().join("midnight.zeron-template");
        // A package whose signed manifest says it's Zeron's Catppuccin.
        let mut files = BTreeMap::new();
        files.insert("theme.luau".to_owned(), b"return nil".to_vec());
        files.insert("screens/shell/sidebar.luau".to_owned(), b"-- takes over".to_vec());
        let hashes: FileHashes = files.iter().map(|(r, b)| (r.clone(), signing::sha256_hex(b))).collect();
        let manifest = json!({ "id": "theme-catppuccin", "name": "Catppuccin", "builtin_theme": "catppuccin", "version": "1.0.0" });
        let signed = alice.sign_manifest_for(TEMPLATE_PAYLOAD_PREFIX, &manifest, &hashes);
        let package = json!({
            "format": FORMAT,
            "manifest": signed,
            "files": files.iter().map(|(r, b)| (r.clone(), json!(B64.encode(b)))).collect::<serde_json::Map<_, _>>(),
        });
        std::fs::write(&out, serde_json::to_vec(&package).unwrap()).unwrap();
        let installed = import(&d, &out, author_as(&alice)).unwrap();
        let id = installed["installed"].as_str().unwrap();
        let written: Value = serde_json::from_slice(&std::fs::read(d.join(id).join(MANIFEST)).unwrap()).unwrap();
        assert!(written.get(BUILTIN_THEME).is_none(), "{written}");
        assert_eq!(written["name"], "Catppuccin");
    }

    /// R30: on a case-insensitive filesystem (macOS, Windows) these open
    /// the protected files, so they're protected too.
    #[test]
    fn protected_paths_fold_as_the_filesystem_does() {
        for same in [
            "screens/plugins/page.luau",
            "Screens/Plugins/page.luau",
            "\u{17f}creens/plugins/page.luau",
            "SCREENS/TEMPLATES/x.luau",
            "Account/x.luau",
            "account./x.luau",
            ".LUAURC",
            ".luaurc.",
        ] {
            assert!(protected(same), "{same}");
        }
        for not in ["screens/plugin/page.luau", "screens/shell/x.luau", "accounts/x.luau", "luaurc"] {
            assert!(!protected(not), "{not}");
        }
    }

    /// R29: an export lands only in a folder of yours, never over another
    /// kind of file, never in Zeron's data or a hidden folder of home.
    #[test]
    fn an_export_goes_only_where_it_should() {
        let home = tempfile::tempdir().unwrap();
        let data = home.path().join(".zeron");
        let ssh = home.path().join(".ssh");
        let docs = home.path().join("Documents");
        for d in [&data, &ssh, &docs] {
            std::fs::create_dir_all(d).unwrap();
        }
        let ok = |p: &Path| check_export_path(p, &data, Some(home.path()));
        assert!(ok(&docs.join("m.zeron-template")).is_ok());
        assert!(ok(&ssh.join("authorized_keys.zeron-template")).is_err());
        assert!(ok(&data.join("m.zeron-template")).is_err());
        // Over a file that isn't a package: refused; over an earlier export: fine.
        std::fs::write(docs.join("notes.zeron-template"), "my notes").unwrap();
        assert!(ok(&docs.join("notes.zeron-template")).is_err());
        std::fs::write(docs.join("old.zeron-template"), serde_json::to_vec(&json!({ "format": FORMAT })).unwrap()).unwrap();
        assert!(ok(&docs.join("old.zeron-template")).is_ok());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(ssh.join("x"), docs.join("link.zeron-template")).unwrap();
            assert!(ok(&docs.join("link.zeron-template")).is_err());
        }
        assert!(default_export_path(&data, "midnight").starts_with(&data));
    }
}
