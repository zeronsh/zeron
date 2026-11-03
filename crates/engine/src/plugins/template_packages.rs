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
//! (`{key, author, at, ignored}`): who signed it, as Zeron knew them then.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use serde_json::{Value, json};

use super::files::{MAX_BYTES, MAX_FILES};
use super::signing::{self, FileHashes, SigCheck, SigningKey, TEMPLATE_PAYLOAD_PREFIX};
use super::status::Author;
use super::templates::{self, MANIFEST, PREVIEW, ZERON, inside, valid_id};

pub const FORMAT: &str = "zeron-template:v1";
pub const EXTENSION: &str = "zeron-template";
pub const PENDING: &str = ".pending";
pub const IMPORTED: &str = ".imported.json";
/// A package's largest size on disk: its files' limit, as base64, and room.
const MAX_PACKAGE_BYTES: u64 = MAX_BYTES * 4 / 3 + 1024 * 1024;

/// What no template replaces: the app always loads Zeron's own (the app's
/// `NEVER_TEMPLATED`, apps/zgx/src/plugins/live.rs).
pub const PROTECTED: [&str; 4] = ["screens/plugins/", "screens/templates/", "account/", ".luaurc"];

pub fn protected(rel: &str) -> bool {
    PROTECTED
        .iter()
        .any(|p| if p.ends_with('/') { rel.starts_with(p) } else { rel == *p })
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

fn parse(raw: Vec<u8>) -> Result<Package, String> {
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
fn install(dir: &Path, package: &Package, author: &Author) -> Result<String, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("Couldn't make the templates folder: {e}"))?;
    let id = free_id(dir, &package.id())?;
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
        std::fs::write(tmp.join(MANIFEST), signing::pretty(&package.manifest))
            .map_err(|e| format!("Couldn't write {MANIFEST}: {e}"))?;
        let imported = json!({
            "key": package.key,
            "author": author,
            "at": super::now_secs(),
            "ignored": package.ignored,
        });
        std::fs::write(tmp.join(IMPORTED), signing::pretty(&imported))
            .map_err(|e| format!("Couldn't write {IMPORTED}: {e}"))?;
        std::fs::rename(&tmp, dir.join(&id)).map_err(|e| format!("Couldn't put the template in place: {e}"))
    })();
    if let Err(error) = written {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(error);
    }
    Ok(id)
}

/// `ImportTemplate`: the package at `path`, verified. Yours installs now
/// (`{installed, name, author, ignored}`); anyone else's waits for you in
/// Zeron (`{pending, …about, author, needs_consent: true}`).
pub fn import(dir: &Path, path: &Path, author_of: impl Fn(&str) -> Author) -> Result<Value, String> {
    let package = read(path)?;
    let author = author_of(&package.key);
    if author == Author::You {
        let id = install(dir, &package, &author)?;
        return Ok(json!({
            "installed": id,
            "name": package.name(),
            "author": author,
            "ignored": package.ignored,
        }));
    }
    let pending = stage(dir, &package, path)?;
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
fn stage(dir: &Path, package: &Package, from: &Path) -> Result<String, String> {
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
    about["source"] = json!(from.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
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
            let about: Value = serde_json::from_slice(&templates::read_regular(&e.path())?).ok()?;
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
    let id = install(dir, &package, &author)?;
    forget();
    Ok(json!({ "installed": id, "name": package.name(), "author": author }))
}

/// An installed template's `.imported.json`, if it came from a package.
pub fn imported(folder: &Path) -> Option<Value> {
    let value: Value = serde_json::from_slice(&templates::read_regular(&folder.join(IMPORTED))?).ok()?;
    value["key"].is_string().then_some(value)
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
}
