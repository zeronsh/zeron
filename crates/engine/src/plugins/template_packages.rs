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

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use serde_json::{Value, json};

use super::files::{MAX_BYTES, MAX_FILES};
use super::install::{self, Found, Origin, Source};
use super::signing::{self, FileHashes, SigCheck, SigningKey, TEMPLATE_PAYLOAD_PREFIX};
use super::status::Author;
use super::templates::{self, MANIFEST, PREVIEW, ZERON, inside, valid_id};

pub const FORMAT: &str = "zeron-template:v1";
pub const EXTENSION: &str = "zeron-template";
pub const PENDING: &str = ".pending";
pub const IMPORTED: &str = ".imported.json";
/// Updates found, waiting to be installed (`<id>.json`, `<id>.zeron-template`).
pub const UPDATES: &str = ".updates";
/// The app's mark of one of Zeron's built-in themes (apps/zgx
/// theme_templates `MARK`): never written from a package.
const BUILTIN_THEME: &str = "builtin_theme";
/// A package's largest size on disk: its files' limit, as base64, and room.
const MAX_PACKAGE_BYTES: u64 = MAX_BYTES * 4 / 3 + 1024 * 1024;

/// What no template replaces: the app always loads Zeron's own (the app's
/// `NEVER_TEMPLATED`, apps/zgx/src/plugins/live.rs).
pub const PROTECTED: [&str; 4] = ["screens/plugins/", "screens/templates/", "account/", ".luaurc"];

/// Whether `rel` is (or, on a case-insensitive filesystem, opens) one of
/// [`PROTECTED`]: compared folded (R30), so `Screens/Plugins/…`,
/// `ſcreens/plugins/…` or `account./x` is protected too.
pub fn protected(rel: &str) -> bool {
    let rel = zeron_plugin_guard::path::fold(rel);
    PROTECTED.iter().any(|p| {
        let p = zeron_plugin_guard::path::fold(p);
        rel == p || rel.starts_with(&format!("{p}/"))
    })
}

const UNSIGNED: &str = "This template isn't signed, so Zeron can't tell who made it or whether it was changed. Ask its author to export it again from Zeron.";
const TAMPERED: &str = "This template was changed after it was signed, so Zeron won't install it. Ask its author for a fresh copy.";
const BAD_SIGNATURE: &str = "This template's signature doesn't check out, so Zeron won't install it. Ask its author for a fresh copy.";
const NOT_A_PACKAGE: &str = "This isn't a Zeron template. Templates are shared as .zeron-template files, exported from Zeron.";

/// A package, read and verified.
pub struct Package {
    /// Its manifest, signed.
    pub manifest: Value,
    /// The key that signed it (b64url).
    pub key: String,
    /// The files it installs: its app files and its preview.
    pub files: BTreeMap<String, Vec<u8>>,
    /// Files it carried that no template may replace, left out.
    pub ignored: Vec<String>,
    /// sha256 of the package file as read: what you're shown is what's
    /// installed.
    pub hash: String,
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

    /// What the consent window and the tools show of it.
    fn about(&self) -> Value {
        json!({
            "id": self.id(),
            "name": self.name(),
            "description": self.text("description"),
            "version": self.text("version"),
            "zeron_version": self.text("zeron_version"),
            "key": self.key,
            "files": self.files.len(),
            "ignored": self.ignored,
            "hash": self.hash,
        })
    }
}

/// `ExportTemplate`: template `id` as a signed package at `out` (a
/// `.zeron-template` file outside the templates folder; replaced if there).
/// `{id, path, files, key}`.
pub fn export(dir: &Path, id: &str, out: &Path, key: &SigningKey) -> Result<Value, String> {
    if id == ZERON {
        return Err("Zeron's own interface is already everyone's. Export one of your templates.".into());
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

    let mut files = BTreeMap::new();
    let mut total = 0u64;
    let mut rels = templates::own_files(&folder);
    if templates::is_regular_file(&folder.join(PREVIEW)) {
        rels.push(PREVIEW.into());
    }
    for rel in rels {
        if protected(&rel) {
            continue;
        }
        let path = inside(&folder, &rel).ok_or_else(|| format!("{rel} isn't an app file"))?;
        let bytes = templates::read_regular(&path).ok_or_else(|| format!("Couldn't read {rel}"))?;
        total += bytes.len() as u64;
        files.insert(rel, bytes);
    }
    if files.len() > MAX_FILES || total > MAX_BYTES {
        return Err(format!(
            "This template is too big to share: at most {MAX_FILES} files and {} MB.",
            MAX_BYTES / 1024 / 1024
        ));
    }
    let hashes: FileHashes = files
        .iter()
        .map(|(rel, bytes)| (rel.clone(), signing::sha256_hex(bytes)))
        .collect();

    // Its description, fresh: never a signature or an author it carried
    // (who made it is whoever's key signs it).
    let own: Value = templates::read_regular(&folder.join(MANIFEST))
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let field = |k: &str| own[k].as_str().unwrap_or("").trim().to_owned();
    let name = if field("name").is_empty() { id.to_owned() } else { field("name") };
    let manifest = json!({
        "id": id,
        "name": name,
        "description": field("description"),
        "version": field("version"),
        "zeron_version": field("zeron_version"),
    });
    let signed = key.sign_manifest_for(TEMPLATE_PAYLOAD_PREFIX, &manifest, &hashes);
    let package = json!({
        "format": FORMAT,
        "manifest": signed,
        "files": files.iter().map(|(rel, bytes)| (rel.clone(), json!(B64.encode(bytes)))).collect::<serde_json::Map<_, _>>(),
    });
    let bytes = serde_json::to_vec(&package).map_err(|e| format!("Couldn't write it: {e}"))?;
    let file = out.file_name().and_then(|n| n.to_str()).ok_or("Say where to save it.")?;
    templates::write_atomic(parent, file, &bytes)?;
    Ok(json!({
        "id": id,
        "path": out.display().to_string(),
        "files": files.len(),
        "key": key.public(),
    }))
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
    let refuse = || Err("Zeron saves a template only in a folder of yours: not a hidden one, not Zeron's own, and never over another kind of file.".to_string());
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
    let value: Value = serde_json::from_slice(&raw).map_err(|_| NOT_A_PACKAGE.to_string())?;
    if value["format"].as_str() != Some(FORMAT) {
        return Err(NOT_A_PACKAGE.into());
    }
    let manifest = value["manifest"].clone();
    let files = value["files"].as_object().ok_or(NOT_A_PACKAGE)?;
    if !manifest.is_object() {
        return Err(NOT_A_PACKAGE.into());
    }
    if files.len() > MAX_FILES {
        return Err("This template has too many files to install.".into());
    }
    let mut decoded = BTreeMap::new();
    let mut total = 0u64;
    for (rel, data) in files {
        // Every name a plain path inside the template (`templates::inside`:
        // no `..`, no absolute part, no dotfile but `.luaurc`).
        if inside(Path::new("t"), rel).is_none() {
            return Err(format!("This template has a file Zeron won't write: {rel:?}."));
        }
        let bytes = data
            .as_str()
            .and_then(|s| B64.decode(s).ok())
            .ok_or_else(|| format!("This template's {rel} can't be read."))?;
        total += bytes.len() as u64;
        decoded.insert(rel.clone(), bytes);
    }
    if total > MAX_BYTES {
        return Err("This template is too big to install.".into());
    }
    let hashes: FileHashes = decoded
        .iter()
        .map(|(rel, bytes)| (rel.clone(), signing::sha256_hex(bytes)))
        .collect();
    let key = match signing::check_raw(TEMPLATE_PAYLOAD_PREFIX, &manifest, &hashes) {
        SigCheck::Valid(key) => key,
        SigCheck::Unsigned => return Err(UNSIGNED.into()),
        SigCheck::FilesDiffer => return Err(TAMPERED.into()),
        SigCheck::BadSignature => return Err(BAD_SIGNATURE.into()),
    };
    let id = manifest["id"].as_str().unwrap_or("").trim();
    if !valid_id(id) || id == ZERON {
        return Err(format!("This template's id ({id:?}) can't be used."));
    }
    // Checked as signed; then what no template replaces is set aside, and
    // what isn't an app file (a manifest of its own) too.
    let mut ignored = Vec::new();
    decoded.retain(|rel, _| {
        let keep = !protected(rel) && rel != MANIFEST;
        if !keep {
            ignored.push(rel.clone());
        }
        keep
    });
    Ok(Package {
        manifest,
        key,
        files: decoded,
        ignored,
        hash: signing::sha256_hex(&raw),
        raw,
    })
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
    let id = free_id(dir, &package.id())?;
    let tmp = write_folder(dir, &id, package, author, source)?;
    if let Err(error) = std::fs::rename(&tmp, dir.join(&id)) {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(format!("Couldn't put the template in place: {error}"));
    }
    Ok(id)
}

/// The package written as template `id`'s folder, beside the templates
/// (a dot folder, never listed): the caller puts it in place.
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
    };
    let from = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    import_package(dir, package, &from, source, author_of)
}

/// A package read and verified (from a file, or a link: `source`), as
/// `import` does with it: yours installs, anyone else's waits.
pub fn import_package(dir: &Path, package: Package, from: &str, source: Source, author_of: impl Fn(&str) -> Author) -> Result<Value, String> {
    let author = author_of(&package.key);
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
    stage_as(dir, package, from, source, None)
}

/// `stage`, as an update of installed template `update_of` when it is one
/// (allowed, it replaces that template in place: `decide`).
fn stage_as(dir: &Path, package: &Package, from: &str, source: &Source, update_of: Option<&str>) -> Result<String, String> {
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
    templates::write_atomic(&pending_dir, &format!("{name}.json"), &signing::pretty(&about))?;
    Ok(name)
}

/// The packages waiting for you, oldest first (each `{pending, id, name,
/// description, version, zeron_version, key, files, ignored, hash, at,
/// source}`; the caller adds who signed it).
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
            let mut about: Value = serde_json::from_slice(&templates::read_regular(&e.path())?).ok()?;
            if let Some(origin) = about.as_object_mut().and_then(|a| a.remove("origin"))
                && let Ok(source) = serde_json::from_value::<Source>(origin)
            {
                about["from"] = source.shown();
            }
            (about["pending"].as_str() == Some(name)).then_some(about)
        })
        .collect();
    out.sort_by_key(|v| (v["at"].as_i64().unwrap_or(0), v["pending"].as_str().unwrap_or("").to_owned()));
    out
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

/// `DecideTemplate`: a waiting package allowed (checked again, then
/// installed: `{installed, name, author}`) or not (dropped: `{declined}`).
/// `hash` is the package's as you were shown it: a different one isn't
/// installed.
pub fn decide(dir: &Path, pending: &str, allow: bool, hash: &str, author_of: impl Fn(&str) -> Author) -> Result<Value, String> {
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
    let package = match read(&package_path) {
        Ok(package) => package,
        Err(error) => {
            forget();
            return Err(error);
        }
    };
    if package.hash != hash || about["key"].as_str() != Some(package.key.as_str()) {
        return Err("This template changed while it waited for you. Import it again to see what it is now.".into());
    }
    let author = author_of(&package.key);
    let mut source: Option<Source> = serde_json::from_value(about["origin"].clone()).ok();
    // An update that changes its code, allowed: in place of the template it
    // updates, if that's still the one it was for.
    if let Some(of) = about["update_of"].as_str() {
        let folder = templates::template_dir(dir, of).ok_or_else(|| format!("There's no template {of} to update any more"))?;
        let installed = imported(&folder).ok_or("It wasn't installed from a package")?;
        if installed["key"].as_str() != Some(package.key.as_str()) {
            return Err("The new version is signed by someone else, so it won't be installed".into());
        }
        let was = installed["version"].as_str().unwrap_or("");
        if !install::newer(&package.version(), was) {
            return Err(install::not_newer(&package.version(), was));
        }
        if let Some(source) = &mut source {
            source.version = package.version();
            source.installed_at = super::now_secs();
        }
        replace(dir, of, &folder, &package, &author, source.as_ref())?;
        forget();
        forget_found(dir, of);
        return Ok(json!({ "installed": of, "updated": of, "name": package.name(), "author": author }));
    }
    let id = install(dir, &package, &author, source.as_ref())?;
    forget();
    Ok(json!({ "installed": id, "name": package.name(), "author": author }))
}

/// An installed template's `.imported.json`, if it came from a package.
pub fn imported(folder: &Path) -> Option<Value> {
    let value: Value = serde_json::from_slice(&templates::read_regular(&folder.join(IMPORTED))?).ok()?;
    value["key"].is_string().then_some(value)
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
            Source { kind: "link".into(), url: Some(url.clone()), path: None, git_ref: None, commit: None, version: String::new(), installed_at: 0 },
        ),
        Origin::Release { repo, tag } => {
            let asset = install::release_asset_of(repo, tag.as_deref(), EXTENSION).await.map_err(|error| {
                // (A repository with no release yet, or none with a template.)
                if tag.is_none() { "That repository has no release with a template in it".to_string() } else { error }
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
    } else if fetched.key != key {
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
    })
}

/// The templates installed from a package that may have a newer version
/// where they came from: each one's id, name, key, package hash and source.
pub fn checkable(dir: &Path) -> Vec<(String, String, String, String, Source)> {
    let Ok(list) = templates::list(dir) else { return Vec::new() };
    list["templates"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| {
            let id = t["id"].as_str()?.to_owned();
            let folder = templates::template_dir(dir, &id)?;
            let imported = imported(&folder)?;
            let source: Source = serde_json::from_value(imported["source"].clone()).ok()?;
            Some((
                id,
                t["name"].as_str().unwrap_or("").to_owned(),
                imported["key"].as_str()?.to_owned(),
                imported["hash"].as_str()?.to_owned(),
                source,
            ))
        })
        .collect()
}

/// `UpdateTemplate`: the update found for `id` installed in its place (its
/// id, its use and its look kept), checked again first: the same key as
/// the one installed, the same package as was found. `{updated, name,
/// version, from}`.
pub fn update(dir: &Path, id: &str, author_of: impl Fn(&str) -> Author) -> Result<Value, String> {
    let folder = templates::template_dir(dir, id).ok_or_else(|| format!("There's no template {id}"))?;
    let imported = imported(&folder).ok_or("It wasn't installed from a package, so it has nothing to update from")?;
    let hash = imported["hash"].as_str().unwrap_or("");
    let found = found(dir, id, hash).ok_or("There's no update for it right now; check again")?;
    if let Some(refused) = found.refused {
        return Err(refused);
    }
    let package = read(&found_package_path(dir, id))?;
    if package.hash != found.hash {
        return Err("The update changed since it was found; check again".into());
    }
    if imported["key"].as_str() != Some(package.key.as_str()) {
        return Err("The new version is signed by someone else, so it won't be installed".into());
    }
    let was = imported["version"].as_str().unwrap_or("");
    if !install::newer(&package.version(), was) {
        return Err(install::not_newer(&package.version(), was));
    }
    let mut source: Option<Source> = found.source.clone().or_else(|| serde_json::from_value(imported["source"].clone()).ok());
    // A new version whose code differs from what's installed (anything but
    // its preview picture) is a new template to allow: it waits for you in
    // Zeron's own consent window, as someone else's does, and replaces this
    // one only once you allow it there (`decide`).
    if changes_code(&folder, &package) {
        let shown = source.clone().unwrap_or_else(|| Source {
            kind: "file".into(),
            url: None,
            path: None,
            git_ref: None,
            commit: None,
            version: String::new(),
            installed_at: 0,
        });
        let from = shown.url.clone().unwrap_or_default();
        let pending = stage_as(dir, &package, &from, &shown, Some(id))?;
        let mut out = package.about();
        out["pending"] = json!(pending);
        out["update_of"] = json!(id);
        out["author"] = json!(author_of(&package.key));
        out["needs_consent"] = json!(true);
        return Ok(out);
    }
    if let Some(source) = &mut source {
        source.version = package.text("version");
        source.installed_at = super::now_secs();
    }
    let author = author_of(&package.key);
    replace(dir, id, &folder, &package, &author, source.as_ref())?;
    forget_found(dir, id);
    Ok(json!({
        "updated": id,
        "name": package.name(),
        "version": package.text("version"),
        "from": imported["version"],
    }))
}

/// Whether installing `package` over `folder` changes any of its code: a
/// file of its own added, removed or changed, but its preview picture.
fn changes_code(folder: &Path, package: &Package) -> bool {
    let mut now: BTreeMap<String, String> = BTreeMap::new();
    for rel in templates::own_files(folder) {
        let Some(bytes) = inside(folder, &rel).and_then(|p| templates::read_regular(&p)) else { return true };
        now.insert(rel, signing::sha256_hex(&bytes));
    }
    let next: BTreeMap<String, String> = package
        .files
        .iter()
        .filter(|(rel, _)| rel.as_str() != PREVIEW)
        .map(|(rel, bytes)| (rel.clone(), signing::sha256_hex(bytes)))
        .collect();
    now != next
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

    #[test]
    fn plugins_template_packages_round_trip_with_protected_files_left_out() {
        let d = with_template("round");
        let out = d.parent().unwrap().join("midnight.zeron-template");
        let alice = key();
        let made = export(&d, "midnight", &out, &alice).unwrap();
        assert_eq!(made["files"], 4);
        assert_eq!(made["key"], alice.public());
        // What it carries: the app files and its preview; never the
        // protected ones, its README, its base or an author.
        let package = package_json(&out);
        let mut names: Vec<&String> = package["files"].as_object().unwrap().keys().collect();
        names.sort();
        assert_eq!(names, ["app.luau", "ds/tokens.luau", PREVIEW, "screens/shell/sidebar.luau"]);
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
        let done = decide(&bob_dir, name, true, hash, author_as(&bob)).unwrap();
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
        let again = decide(&bob_dir, again["pending"].as_str().unwrap(), true, again["hash"].as_str().unwrap(), author_as(&bob)).unwrap();
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
        let error = decide(&bob_dir, name, true, waiting["hash"].as_str().unwrap(), author_as(&bob)).unwrap_err();
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
        assert!(decide(&bob_dir, name, true, "not-what-was-shown", author_as(&bob)).is_err());
        assert!(!bob_dir.join("midnight").exists());
        assert_eq!(decide(&bob_dir, name, false, "", author_as(&bob)).unwrap(), json!({ "declined": name }));
        assert!(pending(&bob_dir).is_empty());
        assert!(!bob_dir.join("midnight").exists());
        assert!(decide(&bob_dir, name, true, "", author_as(&bob)).is_err());
        assert!(decide(&bob_dir, "../../x", true, "", author_as(&bob)).is_err());
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
        let done = decide(&other, waiting["pending"].as_str().unwrap(), true, waiting["hash"].as_str().unwrap(), author_as(&alice)).unwrap();
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
