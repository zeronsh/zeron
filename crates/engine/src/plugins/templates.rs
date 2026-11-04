//! Templates, for agents: a template is a whole replacement of Zeron's
//! interface (its Luau app: `app.luau`, `screens/`, `ds/`, `assets/`…), kept
//! under `<data>/templates/<id>/`. The app runs the main window from Zeron's
//! own files overlaid with the active template's; this module only stores
//! templates and answers the agent tools (`ListTemplates`, `CreateTemplate`,
//! `UseTemplate`, `TemplateStatus`, `DeleteTemplate`, `DescribeApp`).
//!
//! The templates folder:
//!
//! - `.zeron/`: Zeron's own app, written by the app (read here only), with
//!   `zeron-template.json`, `.hashes.json` (`{rel: hash}` for each of its
//!   files) and `.catalog.json` (`{screens: [..]}`);
//! - `.active`: the active template's id (`zeron` when absent or empty);
//! - `.active.by.json`: who switched to it, when an agent or another device
//!   did (`{id, by, at}`; a record for the app's preview bar, nothing more);
//! - `.status.json`: written by the app, `{id: {root?, files: {rel: error}}}`;
//! - `<id>/`: a template: `zeron-template.json`, any subset of Zeron's files
//!   (it inherits the rest), `.base.json` (`{rel: Zeron's hash as copied}`)
//!   and an optional `preview.png`;
//! - `.removed/<id>-<unix>/`: deleted templates;
//! - `.pending/`: someone else's packages, waiting for you to allow them
//!   (`template_packages`); an installed one keeps `.imported.json`.
//!
//! Nothing here follows a symlink or writes outside the templates folder: an
//! id is one path component, every relative path is checked, and folders
//! are walked without following links.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;
use serde_json::{Value, json};

pub const ZERON: &str = "zeron";
pub const ZERON_DIR: &str = ".zeron";
pub const MANIFEST: &str = "zeron-template.json";
pub const HASHES: &str = ".hashes.json";
pub const CATALOG: &str = ".catalog.json";
pub const ACTIVE: &str = ".active";
/// Who switched to the active template when it wasn't this device's own app:
/// `{id, by, at}` (`UseTemplate`). A record for the app's preview bar.
pub const ACTIVE_BY: &str = ".active.by.json";
pub const STATUS: &str = ".status.json";
pub const BASE: &str = ".base.json";
pub const PREVIEW: &str = "preview.png";
pub const REMOVED: &str = ".removed";
/// The one dotfile an app keeps, at its root.
const LUAURC: &str = ".luaurc";

const NOT_SET_UP: &str = "Zeron hasn't set up its templates yet. Open Zeron once, then try again.";

/// The short guide `CreateTemplate` and `DescribeApp` give.
pub const MAP: &str = "\
app.luau is the root: it builds the window from the rest.
screens/shell is the window: sidebar.luau, title_bar.luau, page.luau, settings.luau.
screens/chat is a chat and the composer.
screens/files is the files pane.
ds/ is the design system: tokens.luau, themes/*.json, and the components.
assets/ holds icons and art.
Elements are named by id and class (id = \"…\", class = \"…\"): plugins place their nodes by those (describe_elements lists them), so keep the names to keep plugins working.
screens/plugins and account/ always load Zeron's own, whatever the template has.
Each saved edit shows live while Zeron is open and this template is in use.
A file with an error falls back to Zeron's own for that file; template_status gives the error.";

#[derive(Debug, Default, Deserialize)]
struct Manifest {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    zeron_version: Option<String>,
    /// One of Zeron's official templates (Zeron v2), as the app installs
    /// it: its id.
    #[serde(default)]
    builtin_template: Option<String>,
}

/// `.status.json[id]`.
#[derive(Debug, Default, Deserialize)]
struct Health {
    #[serde(default)]
    root: Option<String>,
    #[serde(default)]
    files: BTreeMap<String, String>,
}

/// `id` is a template's id: `^[a-z0-9][a-z0-9-]{1,39}$`.
pub fn valid_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    (2..=40).contains(&bytes.len())
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

/// `rel` as a path inside `dir`, if it is one: normal components only (no
/// `..`, no absolute part) and no dotfile, but `.luaurc` at the root.
pub fn inside(dir: &Path, rel: &str) -> Option<PathBuf> {
    // (And one that stays inside on every system: crates/plugin-guard.)
    if !zeron_plugin_guard::path::is_safe(rel) {
        return None;
    }
    let path = Path::new(rel);
    if path.is_absolute() {
        return None;
    }
    // Every `/`-separated part must be a plain name: `Path::components`
    // alone would quietly drop `.` and empty parts.
    if rel
        .split('/')
        .any(|part| part.is_empty() || part.starts_with('.') && rel != LUAURC)
    {
        return None;
    }
    for part in path.components() {
        match part {
            Component::Normal(name) => {
                let name = name.to_str()?;
                if name.starts_with('.') && rel != LUAURC {
                    return None;
                }
            }
            _ => return None,
        }
    }
    Some(dir.join(path))
}

/// Reads a regular file (never through a link).
pub(super) fn read_regular(path: &Path) -> Option<Vec<u8>> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    std::fs::read(path).ok()
}

fn read_json<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> T {
    read_regular(path)
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn is_real_dir(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.is_dir())
}

/// Zeron's own app folder, if the app has written it.
fn zeron_dir(dir: &Path) -> Result<PathBuf, String> {
    let zeron = dir.join(ZERON_DIR);
    if zeron.is_dir() {
        Ok(zeron)
    } else {
        Err(NOT_SET_UP.into())
    }
}

/// An installed template's folder: a valid id that names a real folder.
pub(super) fn template_dir(dir: &Path, id: &str) -> Option<PathBuf> {
    if !valid_id(id) || id == ZERON {
        return None;
    }
    let path = dir.join(id);
    is_real_dir(&path).then_some(path)
}

/// Every installed template id, unsorted.
fn installed(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|id| template_dir(dir, id).is_some())
        .collect()
}

/// The app files under `root`, as relative paths (`/`-separated), sorted:
/// regular files only, links skipped, no dotfiles but the root's `.luaurc`,
/// and none of `skip` at the root.
pub fn app_files(root: &Path, skip: &[&str]) -> Vec<String> {
    fn walk(root: &Path, at: &Path, prefix: &str, skip: &[&str], out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(at) else {
            return;
        };
        for entry in entries.flatten() {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            let rel = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            if prefix.is_empty() && skip.contains(&name.as_str()) {
                continue;
            }
            if inside(root, &rel).is_none() {
                continue;
            }
            // `DirEntry::file_type` doesn't follow links.
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                walk(root, &entry.path(), &rel, skip, out);
            } else if kind.is_file() {
                out.push(rel);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, "", skip, &mut out);
    out.sort();
    out
}

/// Zeron's own app files (its folder without its bookkeeping).
fn zeron_files(zeron: &Path) -> Vec<String> {
    app_files(zeron, &[MANIFEST, HASHES, CATALOG])
}

/// A template's own app files.
pub(super) fn own_files(template: &Path) -> Vec<String> {
    app_files(template, &[MANIFEST, BASE, PREVIEW, super::guides::README])
}

/// The active template's id, as `.active` says: `zeron` when it's absent,
/// empty, or names a template that isn't installed.
pub fn active(dir: &Path) -> String {
    let id = read_regular(&dir.join(ACTIVE))
        .map(|bytes| String::from_utf8_lossy(&bytes).trim().to_owned())
        .unwrap_or_default();
    if template_dir(dir, &id).is_some() {
        id
    } else {
        ZERON.into()
    }
}

fn health(dir: &Path, id: &str) -> Health {
    let mut all: BTreeMap<String, Value> = read_json(&dir.join(STATUS));
    all.remove(id)
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

/// Whether a template made for Zeron `made_for` is out of date in Zeron
/// `now`: both known, and not the same. One with no version recorded (written
/// by hand) never is, nor one stamped "0.0.0" (what development builds
/// stamped before Zeron's version reached them). A save to Zeron's own while
/// developing changes no version, so only a real update says so; which files
/// changed is `TemplateStatus`'s (`out_of_date` below).
fn version_out_of_date(made_for: &str, now: &str) -> bool {
    let known = |v: &str| !v.is_empty() && v != "0.0.0";
    let (made_for, now) = (made_for.trim(), now.trim());
    known(made_for) && known(now) && made_for != now
}

/// The template's files whose Zeron original has changed since they were
/// copied (and that Zeron still has).
fn out_of_date(template: &Path, hashes: &BTreeMap<String, String>) -> Vec<String> {
    let base: BTreeMap<String, String> = read_json(&template.join(BASE));
    base.into_iter()
        .filter(|(rel, copied)| {
            hashes.get(rel).is_some_and(|now| now != copied)
                && inside(template, rel).is_some_and(|p| is_regular_file(&p))
        })
        .map(|(rel, _)| rel)
        .collect()
}

pub(super) fn is_regular_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file())
}

pub(super) fn write_atomic(dir: &Path, name: &str, bytes: &[u8]) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("Couldn't make the templates folder: {e}"))?;
    let tmp = dir.join(format!("{name}.tmp-{}", std::process::id()));
    std::fs::write(&tmp, bytes).map_err(|e| format!("Couldn't write {name}: {e}"))?;
    std::fs::rename(&tmp, dir.join(name)).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("Couldn't write {name}: {e}")
    })
}

fn set_active(dir: &Path, id: &str) -> Result<(), String> {
    write_atomic(dir, ACTIVE, id.as_bytes())
}

/// Whether `folder` (template `id`) is one of Zeron's official templates as
/// the app installs it (apps/zgx `plugins::official`): marked with its id,
/// at that id (or `<id>-N`, where someone's has it), and no one's package.
/// Built in, as the default is: it comes and updates with Zeron, and it
/// isn't deleted. (Whether it's this build's, byte for byte, is the app's to
/// say before it runs it with Zeron's powers; this only keeps it listed.)
fn official(id: &str, folder: &Path, manifest: &Manifest) -> bool {
    let Some(mark) = manifest.builtin_template.as_deref().filter(|m| !m.is_empty()) else {
        return false;
    };
    let at_its_id = id == mark
        || id
            .strip_prefix(mark)
            .and_then(|rest| rest.strip_prefix('-'))
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    at_its_id && super::template_packages::imported(folder).is_none()
}

/// One template, as `ListTemplates` lists it.
fn entry(
    dir: &Path,
    id: &str,
    folder: &Path,
    active: &str,
    zeron_version: &str,
) -> Value {
    let manifest: Manifest = read_json(&folder.join(MANIFEST));
    let builtin = id == ZERON || official(id, folder, &manifest);
    let made_for = manifest.zeron_version.clone().unwrap_or_default();
    let health = health(dir, id);
    let (status, error) = if let Some(root) = health.root {
        ("broken", Some(root))
    } else if let Some((rel, error)) = health.files.iter().next() {
        let more = health.files.len() - 1;
        let error = match more {
            0 => format!("{rel}: {error}"),
            n => format!("{rel}: {error} (and {n} more)"),
        };
        ("errors", Some(error))
    } else if !builtin && version_out_of_date(&made_for, zeron_version) {
        ("out_of_date", None)
    } else {
        ("ok", None)
    };
    let mut out = json!({
        "id": id,
        "name": manifest.name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| id.to_owned()),
        "description": manifest.description.unwrap_or_default(),
        "author": manifest.author.unwrap_or_default(),
        "version": manifest.version.unwrap_or_default(),
        "zeron_version": made_for,
        "builtin": builtin,
        "active": id == active,
        "status": status,
        "dir": folder.display().to_string(),
    });
    if let Some(error) = error {
        out["error"] = json!(error);
    }
    let preview = folder.join(PREVIEW);
    if is_regular_file(&preview) {
        out["preview"] = json!(preview.display().to_string());
    }
    // Someone's, installed from a package: by whom, as it was signed.
    if !builtin && let Some(imported) = super::template_packages::imported(folder) {
        out["imported"] = imported;
    }
    out
}

/// `ListTemplates`: `{dir, active, templates: [..]}`, Zeron's first, then by
/// name.
pub fn list(dir: &Path) -> Result<Value, String> {
    let zeron = zeron_dir(dir)?;
    let active = active(dir);
    let zeron_manifest: Manifest = read_json(&zeron.join(MANIFEST));
    let now = zeron_manifest.zeron_version.unwrap_or_default();
    let mut rest: Vec<Value> = installed(dir)
        .into_iter()
        .map(|id| {
            let folder = dir.join(&id);
            entry(dir, &id, &folder, &active, &now)
        })
        .collect();
    rest.sort_by(|a, b| {
        let key = |v: &Value| {
            (
                v["name"].as_str().unwrap_or("").to_lowercase(),
                v["id"].as_str().unwrap_or("").to_owned(),
            )
        };
        key(a).cmp(&key(b))
    });
    let mut templates = vec![entry(dir, ZERON, &zeron, &active, &now)];
    templates.extend(rest);
    Ok(json!({ "dir": dir.display().to_string(), "active": active, "templates": templates }))
}

/// Copies `rel` from `from_root` to `to_root` (both checked, no links).
fn copy_file(from_root: &Path, to_root: &Path, rel: &str) -> Result<(), String> {
    let from = inside(from_root, rel).ok_or_else(|| format!("{rel} isn't an app file"))?;
    let to = inside(to_root, rel).ok_or_else(|| format!("{rel} isn't an app file"))?;
    let bytes = read_regular(&from).ok_or_else(|| format!("Couldn't read {rel}"))?;
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Couldn't make a folder for {rel}: {e}"))?;
    }
    std::fs::write(&to, bytes).map_err(|e| format!("Couldn't copy {rel}: {e}"))
}

/// Writes a new template into `tmp`: Zeron's files, then `from`'s, its
/// manifest and its `.base.json`.
fn fill(tmp: &Path, zeron: &Path, from: Option<&Path>, id: &str, name: &str, guide: &Path) -> Result<(), String> {
    let zeron_manifest: Manifest = read_json(&zeron.join(MANIFEST));
    let hashes: BTreeMap<String, String> = read_json(&zeron.join(HASHES));
    let mut base = BTreeMap::new();
    for rel in zeron_files(zeron) {
        copy_file(zeron, tmp, &rel)?;
        if let Some(hash) = hashes.get(&rel) {
            base.insert(rel, hash.clone());
        }
    }
    if let Some(from) = from {
        let from_base: BTreeMap<String, String> = read_json(&from.join(BASE));
        for rel in own_files(from) {
            copy_file(from, tmp, &rel)?;
            match from_base.get(&rel) {
                Some(hash) => base.insert(rel, hash.clone()),
                None => base.remove(&rel),
            };
        }
    }
    let manifest = json!({
        "id": id,
        "name": name,
        "description": "",
        "author": "you",
        "version": "1.0.0",
        "zeron_version": zeron_manifest.zeron_version.unwrap_or_default(),
    });
    let pretty = |v: &Value| serde_json::to_vec_pretty(v).unwrap_or_default();
    std::fs::write(tmp.join(MANIFEST), pretty(&manifest))
        .map_err(|e| format!("Couldn't write {MANIFEST}: {e}"))?;
    std::fs::write(tmp.join(BASE), pretty(&json!(base)))
        .map_err(|e| format!("Couldn't write {BASE}: {e}"))?;
    // Its README, linking the guide (never part of its app).
    std::fs::write(tmp.join(super::guides::README), super::guides::template_readme(name, guide))
        .map_err(|e| format!("Couldn't write {}: {e}", super::guides::README))
}

/// `CreateTemplate`: a full copy of Zeron's app (overlaid with `from`'s
/// files) as a new template. `{id, dir, files, map}`.
pub fn create(dir: &Path, id: &str, name: &str, from: Option<&str>, guide: &Path) -> Result<Value, String> {
    if !valid_id(id) {
        return Err(format!(
            "{id:?} can't be a template's id: use 2 to 40 lowercase letters, digits and dashes, starting with a letter or digit."
        ));
    }
    if id == ZERON {
        return Err("zeron is the default template's id. Pick another one.".into());
    }
    let target = dir.join(id);
    if std::fs::symlink_metadata(&target).is_ok() {
        return Err(format!(
            "There's already a template {id}. Pick another id, or delete it first."
        ));
    }
    let zeron = zeron_dir(dir)?;
    let from = from
        .map(str::trim)
        .filter(|f| !f.is_empty())
        .unwrap_or(ZERON);
    let from_dir = if from == ZERON {
        None
    } else {
        Some(
            template_dir(dir, from)
                .ok_or_else(|| format!("There's no template {from}. list_templates names them."))?,
        )
    };

    let tmp = dir.join(format!(
        ".tmp-{id}-{}-{}",
        std::process::id(),
        super::now_secs()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)
        .map_err(|e| format!("Couldn't make the template's folder: {e}"))?;
    let made = fill(&tmp, &zeron, from_dir.as_deref(), id, name, guide).and_then(|()| {
        std::fs::rename(&tmp, &target)
            .map_err(|e| format!("Couldn't put the template in place: {e}"))
    });
    if let Err(error) = made {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(error);
    }
    Ok(json!({
        "id": id,
        "dir": target.display().to_string(),
        "files": own_files(&target),
        "map": MAP,
    }))
}

/// `UseTemplate`: `{active, preview, by?}`. `zeron` works with nothing else
/// in place.
///
/// A plain switch, never an approval: the app runs any template the person
/// hasn't kept in its own consent window as a preview (a host bar to Keep
/// or Revert, and only what's safe until they keep it), so `preview` says
/// a switch to anything but Zeron's own may land as one; the app's bar is
/// the truth. `by` is who asked when it wasn't this device's own app (an
/// agent's chat, another device), kept beside `.active` in [`ACTIVE_BY`]
/// for the app to name in its bar: a record, never an authority.
pub fn use_template(dir: &Path, id: &str, by: Option<&Value>) -> Result<Value, String> {
    let id = id.trim();
    if id != ZERON && template_dir(dir, id).is_none() {
        return Err(format!(
            "There's no template {id}. list_templates names them."
        ));
    }
    match by {
        Some(by) => write_atomic(
            dir,
            ACTIVE_BY,
            &serde_json::to_vec(&json!({ "id": id, "by": by, "at": chrono::Utc::now().timestamp() }))
                .unwrap_or_default(),
        )?,
        None => {
            let _ = std::fs::remove_file(dir.join(ACTIVE_BY));
        }
    }
    set_active(dir, id)?;
    let mut reply = json!({ "active": id, "preview": id != ZERON });
    if let Some(by) = by {
        reply["by"] = by.clone();
    }
    Ok(reply)
}

/// `TemplateStatus`: `{id, active, root_error?, file_errors, out_of_date,
/// files, inherits}`.
pub fn status(dir: &Path, id: &str) -> Result<Value, String> {
    let zeron = zeron_dir(dir)?;
    let folder = if id == ZERON {
        zeron.clone()
    } else {
        template_dir(dir, id)
            .ok_or_else(|| format!("There's no template {id}. list_templates names them."))?
    };
    let health = health(dir, id);
    let theirs = zeron_files(&zeron);
    let (files, inherits, stale) = if id == ZERON {
        (theirs.len(), 0, Vec::new())
    } else {
        let mine = own_files(&folder);
        let inherits = theirs
            .iter()
            .filter(|rel| mine.binary_search(rel).is_err())
            .count();
        let hashes: BTreeMap<String, String> = read_json(&zeron.join(HASHES));
        (mine.len(), inherits, out_of_date(&folder, &hashes))
    };
    let mut out = json!({
        "id": id,
        "active": active(dir) == id,
        "file_errors": health.files,
        "out_of_date": stale,
        "files": files,
        "inherits": inherits,
    });
    if let Some(root) = health.root {
        out["root_error"] = json!(root);
    }
    Ok(out)
}

/// `DeleteTemplate`: moves it to `.removed/<id>-<unix>`. `{deleted, active}`.
pub fn delete(dir: &Path, id: &str) -> Result<Value, String> {
    if id == ZERON {
        return Err("The default template can't be deleted.".into());
    }
    let folder = template_dir(dir, id)
        .ok_or_else(|| format!("There's no template {id}. list_templates names them."))?;
    let manifest: Manifest = read_json(&folder.join(MANIFEST));
    if official(id, &folder, &manifest) {
        let name = manifest.name.unwrap_or_else(|| id.to_owned());
        return Err(format!("{name} comes with Zeron and can't be deleted."));
    }
    let was_active = active(dir) == id;
    let removed = dir.join(REMOVED);
    std::fs::create_dir_all(&removed).map_err(|e| format!("Couldn't set it aside: {e}"))?;
    let stamp = super::now_secs();
    let mut to = removed.join(format!("{id}-{stamp}"));
    let mut n = 1;
    while std::fs::symlink_metadata(&to).is_ok() {
        to = removed.join(format!("{id}-{stamp}-{n}"));
        n += 1;
    }
    std::fs::rename(&folder, &to).map_err(|e| format!("Couldn't set {id} aside: {e}"))?;
    if was_active {
        set_active(dir, ZERON)?;
    }
    Ok(json!({ "deleted": id, "active": active(dir) }))
}

/// `DescribeApp`: `{screens, map}`.
pub fn describe(dir: &Path) -> Result<Value, String> {
    let zeron = zeron_dir(dir)?;
    let catalog: BTreeMap<String, Value> = read_json(&zeron.join(CATALOG));
    let screens = catalog
        .get("screens")
        .cloned()
        .filter(Value::is_array)
        .unwrap_or_else(|| json!([]));
    Ok(json!({ "screens": screens, "map": MAP }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("zeron-templates-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// A templates folder with Zeron's own app written.
    fn with_zeron(name: &str) -> PathBuf {
        let d = dir(name);
        let z = d.join(ZERON_DIR);
        write(&z.join("app.luau"), "-- app");
        write(&z.join(".luaurc"), "{}");
        write(&z.join("screens/shell/sidebar.luau"), "-- sidebar");
        write(&z.join("screens/chat/init.luau"), "-- chat");
        write(&z.join("ds/tokens.luau"), "-- tokens");
        write(&z.join("ds/.DS_Store"), "junk");
        write(
            &z.join(MANIFEST),
            r#"{"id":"zeron","name":"Zeron","description":"Zeron's own.","author":"Zeron","version":"1","zeron_version":"0.9.0"}"#,
        );
        write(
            &z.join(HASHES),
            r#"{"app.luau":"a1",".luaurc":"r1","screens/shell/sidebar.luau":"s1","screens/chat/init.luau":"c1","ds/tokens.luau":"t1"}"#,
        );
        write(
            &z.join(CATALOG),
            r##"{"screens":[{"module":"@app/screens/shell/sidebar","file":"screens/shell/sidebar.luau","title":"Sidebar","description":"The sidebar.","elements":["#sidebar",".session-row"]}]}"##,
        );
        d
    }

    fn zeron_hashes(d: &Path, json: &str) {
        write(&d.join(ZERON_DIR).join(HASHES), json);
    }

    fn ids(list: &Value) -> Vec<String> {
        list["templates"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["id"].as_str().unwrap().to_owned())
            .collect()
    }

    #[test]
    fn plugins_templates_list_puts_zeron_first() {
        let d = with_zeron("list");
        create(&d, "zz-top", "Alpha", None, Path::new("docs/templates.md")).unwrap();
        create(&d, "aaa", "Zulu", None, Path::new("docs/templates.md")).unwrap();
        write(&d.join("aaa").join(PREVIEW), "png");
        let list = list(&d).unwrap();
        assert_eq!(ids(&list), ["zeron", "zz-top", "aaa"]);
        assert_eq!(list["active"], "zeron");
        assert_eq!(list["dir"], d.display().to_string());
        let zeron = &list["templates"][0];
        assert_eq!(zeron["builtin"], true);
        assert_eq!(zeron["active"], true);
        assert_eq!(zeron["name"], "Zeron");
        assert_eq!(zeron["status"], "ok");
        assert_eq!(zeron["dir"], d.join(ZERON_DIR).display().to_string());
        let aaa = &list["templates"][2];
        assert_eq!(aaa["builtin"], false);
        assert_eq!(aaa["author"], "you");
        assert_eq!(aaa["zeron_version"], "0.9.0");
        assert_eq!(
            aaa["preview"],
            d.join("aaa").join(PREVIEW).display().to_string()
        );
        assert!(list["templates"][1].get("preview").is_none());

        // Before the app has written its own, it says how to get it.
        assert!(
            super::list(&dir("list-empty"))
                .unwrap_err()
                .contains("Open Zeron once")
        );
    }

    #[test]
    fn plugins_templates_create_from_zeron_is_a_full_copy() {
        let d = with_zeron("create");
        let made = create(&d, "mine", "Mine", None, Path::new("docs/templates.md")).unwrap();
        let t = d.join("mine");
        assert_eq!(made["id"], "mine");
        assert_eq!(made["dir"], t.display().to_string());
        assert_eq!(
            made["files"],
            json!([
                ".luaurc",
                "app.luau",
                "ds/tokens.luau",
                "screens/chat/init.luau",
                "screens/shell/sidebar.luau"
            ])
        );
        assert!(made["map"].as_str().unwrap().contains("screens/shell"));
        assert_eq!(
            std::fs::read_to_string(t.join("screens/shell/sidebar.luau")).unwrap(),
            "-- sidebar"
        );
        // Its README links the guide, and isn't one of its files.
        let readme = std::fs::read_to_string(t.join(super::super::guides::README)).unwrap();
        assert!(readme.contains("docs/templates.md") && readme.starts_with("# Mine"));
        // Zeron's bookkeeping and stray dotfiles stay behind.
        assert!(
            !t.join(HASHES).exists()
                && !t.join(CATALOG).exists()
                && !t.join("ds/.DS_Store").exists()
        );
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(t.join(MANIFEST)).unwrap()).unwrap();
        assert_eq!(
            manifest,
            json!({ "id": "mine", "name": "Mine", "description": "", "author": "you", "version": "1.0.0", "zeron_version": "0.9.0" })
        );
        let base: Value = serde_json::from_slice(&std::fs::read(t.join(BASE)).unwrap()).unwrap();
        assert_eq!(base["app.luau"], "a1");
        assert_eq!(base["screens/shell/sidebar.luau"], "s1");
        assert_eq!(base.as_object().unwrap().len(), 5);
        // No temp folder is left behind.
        let left: Vec<_> = std::fs::read_dir(&d)
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert!(
            left.iter()
                .all(|n| !n.to_string_lossy().starts_with(".tmp")),
            "{left:?}"
        );
    }

    #[test]
    fn plugins_templates_create_from_another_overlays_it() {
        let d = with_zeron("overlay");
        create(&d, "first", "First", None, Path::new("docs/templates.md")).unwrap();
        let first = d.join("first");
        write(&first.join("app.luau"), "-- first's app");
        write(&first.join("screens/extra.luau"), "-- extra");
        write(&first.join(PREVIEW), "png");
        // first's copy of the sidebar was taken from an older Zeron.
        let mut base: BTreeMap<String, String> = read_json(&first.join(BASE));
        base.insert("screens/shell/sidebar.luau".into(), "s0".into());
        std::fs::write(first.join(BASE), serde_json::to_vec(&base).unwrap()).unwrap();

        let made = create(&d, "second", "Second", Some("first"), Path::new("docs/templates.md")).unwrap();
        let t = d.join("second");
        assert!(
            made["files"]
                .as_array()
                .unwrap()
                .contains(&json!("screens/extra.luau"))
        );
        assert_eq!(
            std::fs::read_to_string(t.join("app.luau")).unwrap(),
            "-- first's app"
        );
        assert_eq!(
            std::fs::read_to_string(t.join("ds/tokens.luau")).unwrap(),
            "-- tokens"
        );
        assert!(!t.join(PREVIEW).exists());
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(t.join(MANIFEST)).unwrap()).unwrap();
        assert_eq!(manifest["id"], "second");
        let base: BTreeMap<String, String> = read_json(&t.join(BASE));
        assert_eq!(base["screens/shell/sidebar.luau"], "s0");
        assert_eq!(base["ds/tokens.luau"], "t1");
        // A file of first's own has no Zeron original.
        assert!(!base.contains_key("screens/extra.luau"));

        assert!(
            create(&d, "third", "Third", Some("nope"), Path::new("docs/templates.md"))
                .unwrap_err()
                .contains("no template nope")
        );
        assert!(!d.join("third").exists());
    }

    #[test]
    fn plugins_templates_bad_and_taken_ids_are_refused() {
        let d = with_zeron("ids");
        for bad in [
            "",
            "a",
            "Mine",
            "-mine",
            "my_theme",
            "../x",
            "a/b",
            ".hidden",
            &"x".repeat(41),
        ] {
            assert!(create(&d, bad, "Bad", None, Path::new("docs/templates.md")).is_err(), "{bad:?}");
        }
        assert!(create(&d, "zeron", "Zeron", None, Path::new("docs/templates.md")).is_err());
        assert!(valid_id("ab") && valid_id(&"x".repeat(40)) && valid_id("0-day"));
        create(&d, "mine", "Mine", None, Path::new("docs/templates.md")).unwrap();
        assert!(
            create(&d, "mine", "Again", None, Path::new("docs/templates.md"))
                .unwrap_err()
                .contains("already")
        );
        // Nothing appeared outside the templates folder.
        assert!(!d.parent().unwrap().join("x").exists());
    }

    #[test]
    fn plugins_templates_use_writes_active() {
        let d = with_zeron("use");
        create(&d, "mine", "Mine", None, Path::new("docs/templates.md")).unwrap();
        assert_eq!(
            use_template(&d, "mine", None).unwrap(),
            json!({ "active": "mine", "preview": true })
        );
        assert_eq!(std::fs::read_to_string(d.join(ACTIVE)).unwrap(), "mine");
        assert_eq!(list(&d).unwrap()["active"], "mine");
        assert!(
            use_template(&d, "nope", None)
                .unwrap_err()
                .contains("no template nope")
        );
        assert!(use_template(&d, "../..", None).is_err());
        assert_eq!(std::fs::read_to_string(d.join(ACTIVE)).unwrap(), "mine");

        // The escape hatch works with nothing set up at all.
        let bare = dir("use-bare");
        assert_eq!(
            use_template(&bare, "zeron", None).unwrap(),
            json!({ "active": "zeron", "preview": false })
        );
        assert_eq!(std::fs::read_to_string(bare.join(ACTIVE)).unwrap(), "zeron");
        // An absent or empty .active is Zeron.
        std::fs::write(bare.join(ACTIVE), "  \n").unwrap();
        assert_eq!(active(&bare), "zeron");
    }

    #[test]
    fn plugins_templates_status_reports_errors_staleness_and_inheritance() {
        let d = with_zeron("status");
        create(&d, "mine", "Mine", None, Path::new("docs/templates.md")).unwrap();
        create(&d, "slim", "Slim", None, Path::new("docs/templates.md")).unwrap();
        create(&d, "dead", "Dead", None, Path::new("docs/templates.md")).unwrap();
        create(&d, "fresh", "Fresh", None, Path::new("docs/templates.md")).unwrap();
        use_template(&d, "mine", None).unwrap();
        // slim was made for an earlier Zeron; fresh for this one.
        write(
            &d.join("slim").join(MANIFEST),
            r#"{"id":"slim","name":"Slim","author":"you","version":"1.0.0","zeron_version":"0.8.0"}"#,
        );
        // slim keeps only two files and inherits the rest.
        for rel in ["ds/tokens.luau", "screens/chat/init.luau", ".luaurc"] {
            std::fs::remove_file(d.join("slim").join(rel)).unwrap();
        }
        write(
            &d.join(STATUS),
            r#"{"mine":{"files":{"screens/shell/sidebar.luau":"sidebar.luau:3: attempt to index nil"}},"dead":{"root":"app.luau:1: syntax error"}}"#,
        );
        // Zeron's sidebar and tokens changed since; its chat was dropped.
        zeron_hashes(
            &d,
            r#"{"app.luau":"a1",".luaurc":"r1","screens/shell/sidebar.luau":"s2","ds/tokens.luau":"t2"}"#,
        );

        let mine = status(&d, "mine").unwrap();
        assert_eq!(mine["id"], "mine");
        assert_eq!(mine["active"], true);
        assert_eq!(
            mine["file_errors"],
            json!({ "screens/shell/sidebar.luau": "sidebar.luau:3: attempt to index nil" })
        );
        assert_eq!(
            mine["out_of_date"],
            json!(["ds/tokens.luau", "screens/shell/sidebar.luau"])
        );
        assert_eq!(mine["files"], 5);
        assert_eq!(mine["inherits"], 0);
        assert!(mine.get("root_error").is_none());

        let slim = status(&d, "slim").unwrap();
        assert_eq!(slim["active"], false);
        assert_eq!(slim["files"], 2);
        assert_eq!(slim["inherits"], 3);
        // Its tokens are Zeron's again, so they can't be out of date.
        assert_eq!(slim["out_of_date"], json!(["screens/shell/sidebar.luau"]));

        assert_eq!(
            status(&d, "dead").unwrap()["root_error"],
            "app.luau:1: syntax error"
        );
        let zeron = status(&d, "zeron").unwrap();
        assert_eq!(
            (zeron["files"].as_u64(), zeron["inherits"].as_u64()),
            (Some(5), Some(0))
        );
        assert!(status(&d, "nope").is_err());

        let list = list(&d).unwrap();
        let of = |id: &str| {
            list["templates"]
                .as_array()
                .unwrap()
                .iter()
                .find(|t| t["id"] == id)
                .unwrap()
                .clone()
        };
        assert_eq!(of("dead")["status"], "broken");
        assert_eq!(of("dead")["error"], "app.luau:1: syntax error");
        assert_eq!(of("mine")["status"], "errors");
        assert!(
            of("mine")["error"]
                .as_str()
                .unwrap()
                .contains("attempt to index nil")
        );
        assert_eq!(of("slim")["status"], "out_of_date");
        assert_eq!(of("slim")["zeron_version"], "0.8.0");
        // Zeron's files changed under it (a save while developing), but it's
        // this Zeron's: not out of date, though its status names the files.
        assert_eq!(of("fresh")["status"], "ok");
        assert_eq!(
            status(&d, "fresh").unwrap()["out_of_date"],
            json!(["ds/tokens.luau", "screens/shell/sidebar.luau"])
        );
        assert_eq!(of("zeron")["status"], "ok");
        assert!(version_out_of_date("0.8.0", "0.9.0"));
        assert!(!version_out_of_date("", "0.9.0") && !version_out_of_date("0.0.0", "0.9.0"));
    }

    #[test]
    fn plugins_templates_delete_sets_aside_and_resets_active() {
        let d = with_zeron("delete");
        create(&d, "mine", "Mine", None, Path::new("docs/templates.md")).unwrap();
        create(&d, "other", "Other", None, Path::new("docs/templates.md")).unwrap();
        use_template(&d, "mine", None).unwrap();
        assert!(
            delete(&d, "zeron")
                .unwrap_err()
                .contains("can't be deleted")
        );
        assert!(delete(&d, "nope").is_err());
        assert!(delete(&d, "../.zeron").is_err());

        assert_eq!(
            delete(&d, "other").unwrap(),
            json!({ "deleted": "other", "active": "mine" })
        );
        assert_eq!(
            delete(&d, "mine").unwrap(),
            json!({ "deleted": "mine", "active": "zeron" })
        );
        assert_eq!(std::fs::read_to_string(d.join(ACTIVE)).unwrap(), "zeron");
        assert!(!d.join("mine").exists());
        let removed: Vec<String> = std::fs::read_dir(d.join(REMOVED))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().into_string().unwrap())
            .collect();
        assert_eq!(removed.len(), 2);
        assert!(removed.iter().any(|n| n.starts_with("mine-")));
        assert!(
            d.join(REMOVED)
                .join(removed.iter().find(|n| n.starts_with("mine-")).unwrap())
                .join("app.luau")
                .is_file()
        );
        assert_eq!(ids(&list(&d).unwrap()), ["zeron"]);
        assert!(d.join(ZERON_DIR).join("app.luau").is_file());
    }

    /// Zeron v2, as the app installs it, is built in: listed so, and never
    /// deleted. One that only says so (someone's package, another id) is
    /// an ordinary template.
    #[test]
    fn plugins_templates_an_official_template_is_built_in() {
        let d = with_zeron("official");
        let marked = |id: &str| {
            write(
                &d.join(id).join(MANIFEST),
                &format!(r#"{{"id":"{id}","name":"Zeron v2","author":"Zeron","builtin_template":"zeron-v2"}}"#),
            );
            write(&d.join(id).join("screens/shell/page.luau"), "-- v2");
        };
        marked("zeron-v2");
        marked("zeron-v2-2");
        marked("my-v2");
        marked("posing");
        write(&d.join("posing").join(".imported.json"), r#"{"key":"k","author":{"kind":"unknown"}}"#);
        let list = list(&d).unwrap();
        let builtin = |id: &str| {
            list["templates"].as_array().unwrap().iter().find(|t| t["id"] == id).unwrap()["builtin"].clone()
        };
        assert_eq!(builtin("zeron-v2"), true);
        assert_eq!(builtin("zeron-v2-2"), true);
        assert_eq!(builtin("my-v2"), false);
        assert_eq!(builtin("posing"), false);
        assert!(delete(&d, "zeron-v2").unwrap_err().contains("Zeron v2 comes with Zeron and can't be deleted"));
        assert!(d.join("zeron-v2").is_dir());
        assert!(delete(&d, "my-v2").is_ok());
        assert!(delete(&d, "posing").is_ok());
    }

    #[test]
    fn plugins_templates_describe_app() {
        let d = with_zeron("describe");
        let described = describe(&d).unwrap();
        assert_eq!(
            described["screens"][0]["module"],
            "@app/screens/shell/sidebar"
        );
        assert_eq!(
            described["screens"][0]["elements"],
            json!(["#sidebar", ".session-row"])
        );
        assert_eq!(described["map"], MAP);
        assert!(describe(&dir("describe-empty")).is_err());
    }

    #[test]
    fn plugins_templates_paths_never_escape() {
        let d = dir("paths");
        for bad in [
            "/etc/passwd",
            "../x.luau",
            "screens/../../x.luau",
            "screens/./x.luau",
            "screens//x.luau",
            "screens/x.luau/",
            "screens/.hidden.luau",
            ".git/config",
            "ds/.luaurc",
            "a\\..\\b",
            "",
            ".base.json",
        ] {
            assert!(inside(&d, bad).is_none(), "{bad:?}");
        }
        assert_eq!(inside(&d, ".luaurc"), Some(d.join(".luaurc")));
        assert_eq!(
            inside(&d, "screens/shell/x.luau"),
            Some(d.join("screens/shell/x.luau"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn plugins_templates_symlinks_are_never_followed() {
        use std::os::unix::fs::symlink;
        let d = with_zeron("links");
        let outside = dir("links-outside");
        write(&outside.join("secret.luau"), "-- secret");
        write(&outside.join("deep/x.luau"), "-- deep");
        let z = d.join(ZERON_DIR);
        symlink(outside.join("secret.luau"), z.join("screens/secret.luau")).unwrap();
        symlink(outside.join("deep"), z.join("ds/deep")).unwrap();

        let made = create(&d, "mine", "Mine", None, Path::new("docs/templates.md")).unwrap();
        let files: Vec<&str> = made["files"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert!(
            !files
                .iter()
                .any(|f| f.contains("secret") || f.contains("deep")),
            "{files:?}"
        );
        assert!(std::fs::symlink_metadata(d.join("mine/screens/secret.luau")).is_err());

        // A linked template folder isn't a template: it can't be used,
        // copied from, or deleted (which would move what it points at).
        let elsewhere = with_zeron("links-elsewhere");
        create(&elsewhere, "real", "Real", None, Path::new("docs/templates.md")).unwrap();
        symlink(elsewhere.join("real"), d.join("linked")).unwrap();
        assert!(use_template(&d, "linked", None).is_err());
        assert!(create(&d, "copy", "Copy", Some("linked"), Path::new("docs/templates.md")).is_err());
        assert!(delete(&d, "linked").is_err());
        assert!(elsewhere.join("real/app.luau").is_file());
        assert!(!ids(&list(&d).unwrap()).contains(&"linked".to_owned()));

        // A template whose id is taken by a link isn't overwritten.
        assert!(
            create(&d, "linked", "Linked", None, Path::new("docs/templates.md"))
                .unwrap_err()
                .contains("already")
        );
        // A linked .base.json isn't read.
        let mine = d.join("mine");
        std::fs::remove_file(mine.join(BASE)).unwrap();
        write(&outside.join("base.json"), r#"{"app.luau":"zzz"}"#);
        symlink(outside.join("base.json"), mine.join(BASE)).unwrap();
        assert_eq!(status(&d, "mine").unwrap()["out_of_date"], json!([]));
    }
}
