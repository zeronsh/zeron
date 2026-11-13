//! What the main window runs: Zeron's own interface, with the active template
//! laid over it. A template is a whole app (its `app.luau`, `screens/`,
//! `ds/`, `assets/`…, like a WordPress theme), and any file it doesn't have
//! is Zeron's, as a child theme inherits.
//!
//! zgx fixes a host's app file when the host is made, and keys each module's
//! state by its path from the app's folder (`screens/shell/sidebar.luau::…`).
//! So the main window always runs from one folder, `<data>/templates/.live/`,
//! and switching templates rewrites what's in it, then reloads: no new window,
//! and state kept wherever the two have the same module.
//!
//! ```text
//! <data>/templates/
//!   .zeron/        Zeron's own, as this build has it (`sync_zeron`), with
//!                  zeron-template.json, .hashes.json, .catalog.json
//!   .live/         .zeron with the active template on top (`compose`)
//!   .active        the active template's id ("zeron", Zeron's own)
//!   .status.json   what failed, per template
//!   <id>/          a template: zeron-template.json, any of .zeron's files
//! ```
//!
//! Files are copied, never linked, and rewritten only where their bytes
//! differ (zgx reloads for any file whose time moves). Some paths are never a
//! template's, whatever it holds (`NEVER_TEMPLATED`): Zeron's own always.

use std::{
    collections::{BTreeMap, BTreeSet},
    hash::{DefaultHasher, Hash, Hasher},
    path::{Component, Path, PathBuf},
};

pub const ZERON: &str = ".zeron";
pub const LIVE: &str = ".live";
pub const ACTIVE: &str = ".active";
pub const STATUS: &str = ".status.json";
/// Zeron's own template's id.
pub const BUILTIN: &str = "zeron";
/// This Zeron's version (the root workspace's, from build.rs): what `.zeron`
/// is stamped with, and what a template made from it records.
pub const ZERON_VERSION: &str = env!("ZERON_VERSION");

/// Paths no template replaces: allowing plugins, your account and signing
/// in, and the aliases every module resolves by. A template's copies of them
/// are left out.
pub const NEVER_TEMPLATED: &[&str] = &["screens/plugins/", "screens/templates/", "account/", ".luaurc"];

/// What of `ui/` isn't the main window's interface: the gallery and its
/// demos, tests and their fixtures, the intro, the consent windows (each
/// its own host, always Zeron's own), and the official templates' files
/// (`official`: each installed as a template of its own).
fn not_the_app(rel: &str) -> bool {
    let first = rel.split('/').next().unwrap_or(rel);
    matches!(first, "demos" | "tests" | ".zgx") || super::official::source_file(rel)
        || matches!(rel, "gallery.luau" | "intro.luau" | "consent.luau" | "template_consent.luau")
        || (!rel.contains('/') && rel.ends_with("_fixture.luau"))
}

/// Compared as a case-insensitive filesystem (macOS's, Windows') names
/// files (`zeron_plugin_guard::path::fold`): `Screens/Plugins/page.luau`,
/// `ſcreens/plugins/page.luau` (U+017F folds to s) and
/// `screens/plugins./page.luau` are Zeron's plugins page there too.
pub fn never_templated(rel: &str) -> bool {
    use zeron_plugin_guard::path::fold;
    let rel = fold(rel);
    NEVER_TEMPLATED.iter().any(|p| {
        let p = fold(p);
        rel == p || rel.starts_with(&format!("{p}/"))
    })
}

/// A template's own files that aren't part of its interface (its README
/// links the authoring guide).
pub fn template_meta(rel: &str) -> bool {
    matches!(rel, "zeron-template.json" | "preview.png" | "README.md")
}

pub fn hash(bytes: &[u8]) -> String {
    let mut h = DefaultHasher::new();
    bytes.hash(&mut h);
    format!("{:016x}", h.finish())
}

/// Whether `rel` is a plain relative path: normal parts, none a dotfile but
/// `.luaurc` at the top.
pub fn plain(rel: &str) -> bool {
    !rel.is_empty()
        && Path::new(rel).components().all(|c| match c {
            Component::Normal(name) => {
                let name = name.to_string_lossy();
                !name.starts_with('.') || rel == ".luaurc"
            }
            _ => false,
        })
}

/// Every regular file under `dir` (no links followed), by its path from `dir`,
/// that `keep` keeps. Dot folders are skipped.
pub fn files(dir: &Path, keep: impl Fn(&str) -> bool) -> BTreeMap<String, PathBuf> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(at) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&at) else { continue };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else { continue };
            let path = entry.path();
            let Ok(rel) = path.strip_prefix(dir) else { continue };
            let rel = rel.to_string_lossy().replace('\\', "/");
            if kind.is_dir() {
                if !entry.file_name().to_string_lossy().starts_with('.') {
                    stack.push(path);
                }
            } else if kind.is_file() && plain(&rel) && keep(&rel) {
                out.insert(rel, path);
            }
        }
    }
    out
}

/// Writes `bytes` to `path` unless it holds them already (through a file
/// beside it renamed over it, so nothing reads half a file): whether it wrote.
pub fn put(path: &Path, bytes: &[u8]) -> std::io::Result<bool> {
    if std::fs::read(path).is_ok_and(|now| now == bytes) {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.tmp"));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(true)
}

/// Makes `to` hold exactly `want` (by path from `to`: where each comes from),
/// touching only what differs and leaving `keep`'s paths alone: how many
/// files changed.
pub fn mirror(to: &Path, want: &BTreeMap<String, PathBuf>, keep: impl Fn(&str) -> bool) -> std::io::Result<usize> {
    let mut changed = 0;
    for (rel, from) in want {
        let bytes = std::fs::read(from)?;
        if put(&to.join(rel), &bytes)? {
            changed += 1;
        }
    }
    for (rel, path) in files(to, |_| true) {
        if !want.contains_key(&rel) && !keep(&rel) {
            std::fs::remove_file(path)?;
            changed += 1;
        }
    }
    Ok(changed)
}

/// `.zeron`, as `ui/` has it now: the app's files, their hashes, the catalog
/// of screens (what each draws, the hooks it declares) and Zeron's manifest;
/// and the official templates beside it, as this build ships them
/// (`official::install`). How many of `.zeron`'s files changed.
pub fn sync_zeron(ui: &Path, templates: &Path) -> std::io::Result<usize> {
    let dir = templates.join(ZERON);
    let want = files(ui, |rel| !not_the_app(rel));
    let changed = mirror(&dir, &want, |rel| rel == "zeron-template.json")?;
    let hashes: BTreeMap<String, String> = want
        .iter()
        .filter_map(|(rel, path)| std::fs::read(path).ok().map(|b| (rel.clone(), hash(&b))))
        .collect();
    let json = |v: serde_json::Value| serde_json::to_vec_pretty(&v).unwrap_or_default();
    put(&dir.join(".hashes.json"), &json(serde_json::json!(hashes)))?;
    put(&dir.join(".catalog.json"), &json(super::templates::catalog(ui)))?;
    put(
        &dir.join("zeron-template.json"),
        &json(serde_json::json!({
            "id": BUILTIN,
            "name": super::official::DEFAULT_NAME,
            "description": super::official::DEFAULT_DESCRIPTION,
            "author": "Zeron",
            "version": ZERON_VERSION,
            "zeron_version": ZERON_VERSION,
        })),
    )?;
    super::official::install(ui, templates);
    Ok(changed)
}

/// The active template's id, from `.active` (Zeron's own if none, or if it's
/// gone or isn't a plain id), and Zeron's own whatever it says in safe mode
/// (`preview::safe`): no template re-selects itself from there.
pub fn active(templates: &Path) -> String {
    if super::preview::safe() {
        return BUILTIN.to_string();
    }
    chosen(templates)
}

/// What `.active` names, safe mode or not (the Templates page shows it).
pub fn chosen(templates: &Path) -> String {
    let id = std::fs::read_to_string(templates.join(ACTIVE)).unwrap_or_default();
    let id = id.trim();
    let ok = !id.is_empty()
        && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && templates.join(id).join("zeron-template.json").is_file();
    if ok { id.to_string() } else { BUILTIN.to_string() }
}

pub fn set_active(templates: &Path, id: &str) -> std::io::Result<bool> {
    put(&templates.join(ACTIVE), id.as_bytes())
}

/// What `.live` holds: each file's source, and which came from the template.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Overlay {
    pub template: String,
    /// Files the template supplies (its own, or its version of Zeron's).
    pub from_template: BTreeSet<String>,
}

/// `.live` as `.zeron` with template `id` on top, leaving out the template's
/// `failed` files (Zeron's show in their place) and the paths never templated.
/// Zeron's own alone for `BUILTIN`, or `whole_failed`. How many files changed.
pub fn compose(templates: &Path, id: &str, failed: &BTreeSet<String>, whole_failed: bool) -> std::io::Result<(usize, Overlay)> {
    let mut want = files(&templates.join(ZERON), |rel| !template_meta(rel));
    let mut from_template = BTreeSet::new();
    if id != BUILTIN && !whole_failed {
        // Each file is written under one spelling: a template's `App.luau`
        // stands in for Zeron's `app.luau` (one file on a case-insensitive
        // filesystem), never both written, whichever sorts last winning.
        let mut spelled: BTreeMap<String, String> = want.keys().map(|rel| (zeron_plugin_guard::path::fold(rel), rel.clone())).collect();
        for (rel, path) in files(&templates.join(id), |rel| !template_meta(rel)) {
            if never_templated(&rel) || failed.contains(&rel) {
                continue;
            }
            if let Some(zeron_s) = spelled.insert(zeron_plugin_guard::path::fold(&rel), rel.clone())
                && zeron_s != rel
            {
                want.remove(&zeron_s);
                from_template.remove(&zeron_s);
            }
            from_template.insert(rel.clone());
            want.insert(rel, path);
        }
    }
    let changed = mirror(&templates.join(LIVE), &want, |_| false)?;
    Ok((changed, Overlay { template: id.to_string(), from_template }))
}

/// The files of `.live` an error names, first mention first: each
/// `<path>.luau:<line>`, its path from the app's folder (as zgx names the
/// app's modules) or through `.live` (as given, or as the system resolves
/// it: macOS's `/var/…` is `/private/var/…` to scripts). The VM's own name
/// before "runtime error" is who ran it, not where it failed, and is skipped.
///
/// Windows names them with `\` (`screens\card.luau:3`, `C:\…\.live\…`):
/// read as `/`, the way `files` names a template's files, so a template
/// file that breaks there falls back like anywhere. (A token stops at a `:`,
/// so `.live`'s spellings are matched without their drive, or `\\?\`.)
pub fn named(error: &str, templates: &Path) -> Vec<String> {
    let live = templates.join(LIVE);
    let spelt = |path: &Path| {
        let text = path.to_string_lossy().replace('\\', "/");
        let text = text.strip_prefix("//?/").unwrap_or(&text);
        let text = match text.as_bytes() {
            [drive, b':', ..] if drive.is_ascii_alphabetic() => &text[2..],
            _ => text,
        };
        format!("{text}/")
    };
    let mut spellings = vec![spelt(&live)];
    if let Ok(real) = std::fs::canonicalize(&live) {
        spellings.push(spelt(&real));
    }
    let body = match error.find(": runtime error: ") {
        Some(at) => &error[at + ": runtime error: ".len()..],
        None => error,
    };
    let is_path = |c: char| c.is_ascii_alphanumeric() || matches!(c, '/' | '\\' | '.' | '_' | '-' | '@');
    let mut out: Vec<String> = Vec::new();
    let mut from = 0;
    while let Some(at) = body[from..].find(".luau:") {
        let end = from + at + ".luau".len();
        let start = body[..end].rfind(|c: char| !is_path(c)).map(|i| i + 1).unwrap_or(0);
        let token = body[start..end].replace('\\', "/");
        let rel = spellings
            .iter()
            .find_map(|s| token.find(s.as_str()).map(|i| &token[i + s.len()..]))
            .unwrap_or(&token);
        if plain(rel) && live.join(rel).is_file() && !out.iter().any(|r| r == rel) {
            out.push(rel.to_string());
        }
        from = end;
    }
    out
}

/// Whom an error is on: the first file it names that the template supplies
/// (a template file required by Zeron's, or Zeron's called from the
/// template's), else none (Zeron's own error).
pub fn blame(error: &str, templates: &Path, overlay: &Overlay) -> Option<String> {
    named(error, templates).into_iter().find(|rel| overlay.from_template.contains(rel))
}

/// A file as it stands, without reading it: its size and the time it was
/// last written. A save changes it, so a failure recorded against it is
/// tried again once the file is edited.
pub fn file_stamp(path: &Path) -> String {
    match std::fs::metadata(path) {
        Ok(meta) => {
            let at = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).unwrap_or_default();
            format!("{}:{}", meta.len(), at.as_nanos())
        }
        Err(_) => String::new(),
    }
}

/// Everything in `dir` that `keep` keeps, as it stands (each file's path,
/// size and time written; nothing read): a template or `ui/` edited reads
/// as new. A stat each, where reading the bytes was most of a look's time.
pub fn stamp(dir: &Path, keep: impl Fn(&str) -> bool) -> String {
    let mut h = DefaultHasher::new();
    for (rel, path) in files(dir, keep) {
        rel.hash(&mut h);
        file_stamp(&path).hash(&mut h);
    }
    format!("{:016x}", h.finish())
}

/// A template's folder as it stands: what a failure of the whole template
/// is recorded against.
pub fn template_stamp(dir: &Path) -> String {
    stamp(dir, |_| true)
}

/// `ui/`'s app files (and the official templates') as they stand:
/// unchanged, `.zeron` needn't be copied again.
pub fn ui_stamp(ui: &Path) -> String {
    stamp(ui, synced)
}

/// What of `ui/` a sync copies: the app's files, and the official templates'.
fn synced(rel: &str) -> bool {
    !not_the_app(rel) || super::official::source_file(rel)
}

/// What a look found, off the UI thread: `.zeron` brought up to date (only
/// when `ui/` changed since the last look), the active template, and its
/// files as they stand.
#[derive(Debug)]
pub struct Survey {
    pub ui_stamp: String,
    /// `.zeron` copied again: how many files changed (none when `ui/` hadn't),
    /// or why it failed.
    pub synced: Result<usize, String>,
    pub id: String,
    pub template_stamp: String,
    /// Each of the active template's files, by its path, as it stands.
    pub files: Vec<(String, String)>,
}

pub fn survey(ui: &Path, templates: &Path, last_ui_stamp: Option<&str>) -> Survey {
    let ui_stamp = ui_stamp(ui);
    // (An official template gone, deleted by hand: put back, as `.zeron` is.)
    let current =
        last_ui_stamp == Some(ui_stamp.as_str()) && templates.join(ZERON).join(".hashes.json").is_file() && !super::official::missing(ui, templates);
    let synced = if current { Ok(0) } else { sync_zeron(ui, templates).map_err(|e| e.to_string()) };
    let id = active(templates);
    let (template_stamp, files_) = if id == BUILTIN {
        (String::new(), Vec::new())
    } else {
        let dir = templates.join(&id);
        let list = files(&dir, |_| true).into_iter().map(|(rel, path)| (rel, file_stamp(&path))).collect();
        (template_stamp(&dir), list)
    };
    Survey { ui_stamp, synced, id, template_stamp, files: files_ }
}

/// What `.live` was last laid down from: the same again needs no compose
/// (which reads every file of Zeron's and the template's).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Laid {
    pub ui_stamp: String,
    pub id: String,
    pub template_stamp: String,
    pub whole_failed: bool,
    pub failed: BTreeSet<String>,
}

/// What to watch so a change reaches the window: the templates folder
/// (`.active`, a template made or removed), the active template's files and
/// folders, and Zeron's own `ui/` (a save while developing).
///
/// `ui/` (over a thousand files, most of them art) is watched by its
/// folders, which hear a file made, removed or saved by replacing it, plus
/// its scripts and themes, the files saved in place. The gallery's and the
/// tests' folders aren't the app's and aren't watched.
pub fn watched(templates: &Path, ui: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut files_ = vec![templates.join(ACTIVE)];
    let mut dirs = vec![templates.to_path_buf()];
    let id = active(templates);
    let edited_in_place = |rel: &str| rel.ends_with(".luau") || rel.ends_with(".json") || rel == ".luaurc";
    // (A bundle's interface doesn't change while it runs: it's read again at
    // the next launch.)
    if !in_app_bundle(ui) {
        files_.extend(files(ui, |rel| synced(rel) && edited_in_place(rel)).into_values());
        folders(ui, &mut dirs, synced);
    }
    if id != BUILTIN {
        let dir = templates.join(&id);
        files_.extend(files(&dir, |_| true).into_values());
        folders(&dir, &mut dirs, |_| true);
    }
    (files_, dirs)
}

/// Whether `ui` is what a macOS app bundle shipped.
fn in_app_bundle(ui: &Path) -> bool {
    ui.ends_with("Contents/Resources/ui")
}

/// `root` and every folder under it that `keep` keeps (by its path from
/// `root`), but dot folders.
fn folders(root: &Path, out: &mut Vec<PathBuf>, keep: impl Fn(&str) -> bool) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(at) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&at) else { continue };
        out.push(at);
        for entry in entries.flatten() {
            let hidden = entry.file_name().to_string_lossy().starts_with('.');
            if hidden || !entry.file_type().is_ok_and(|k| k.is_dir()) {
                continue;
            }
            let path = entry.path();
            let rel = path.strip_prefix(root).map(|r| r.to_string_lossy().replace('\\', "/")).unwrap_or_default();
            if keep(&rel) {
                stack.push(path);
            }
        }
    }
}

/// A template, as its manifest and files say.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Installed {
    pub id: String,
    pub name: String,
    pub description: String,
    pub author: String,
    pub version: String,
    /// The Zeron it was made from (its manifest's `zeron_version`).
    pub zeron_version: String,
    /// It was made from another version of Zeron than this one (its
    /// manifest's `zeron_version` against `.zeron`'s): only a real update
    /// says so, never a save while developing. Which files changed is
    /// `template_status`'s (`.base.json` against `.zeron/.hashes.json`).
    pub out_of_date: bool,
    /// Installed from someone's signed package: `.imported.json`'s
    /// `{key, author}` (the engine's `template_packages`).
    pub imported: Option<serde_json::Value>,
    /// One of Zeron's official templates, exactly as this build wrote it
    /// (`official::verified`): built in, as the default is.
    pub official: Option<super::official::Official>,
}

/// Whether a template made for Zeron `made_for` is out of date in Zeron
/// `now`: both known, and not the same. One with no version recorded (written
/// by hand) never is, nor one stamped "0.0.0" (what development builds
/// stamped before Zeron's version reached them: no real Zeron's).
pub fn version_out_of_date(made_for: &str, now: &str) -> bool {
    let known = |v: &str| !v.is_empty() && v != "0.0.0";
    let (made_for, now) = (made_for.trim(), now.trim());
    known(made_for) && known(now) && made_for != now
}

fn read_json(path: &Path) -> serde_json::Value {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// The default first, then the official templates in Zeron's order, then
/// every other template, by name.
pub fn installed(templates: &Path) -> Vec<Installed> {
    let zeron = read_json(&templates.join(ZERON).join("zeron-template.json"));
    let now = zeron["zeron_version"].as_str().unwrap_or("").to_string();
    let manifest = |dir: &Path, id: &str| -> Option<Installed> {
        let m = read_json(&dir.join("zeron-template.json"));
        if !m.is_object() {
            return None;
        }
        let text = |k: &str| m[k].as_str().unwrap_or("").to_string();
        let out_of_date = id != BUILTIN && version_out_of_date(&text("zeron_version"), &now);
        Some(Installed {
            id: id.to_string(),
            name: if text("name").is_empty() { id.to_string() } else { text("name") },
            description: text("description"),
            author: text("author"),
            version: text("version"),
            zeron_version: text("zeron_version"),
            out_of_date,
            imported: (id != BUILTIN)
                .then(|| read_json(&dir.join(".imported.json")))
                .filter(|v| v["key"].is_string()),
            official: (id != BUILTIN).then(|| super::official::verified(dir)).flatten(),
        })
    };
    let mut out: Vec<Installed> = manifest(&templates.join(ZERON), BUILTIN).into_iter().collect();
    let mut rest: Vec<Installed> = std::fs::read_dir(templates)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|k| k.is_dir()))
        .filter_map(|e| {
            let id = e.file_name().to_string_lossy().into_owned();
            (!id.starts_with('.') && id != BUILTIN && plain(&id)).then(|| manifest(&e.path(), &id)).flatten()
        })
        .collect();
    rest.sort_by_key(|t| (super::official::order(&t.id, t.official), t.name.to_lowercase()));
    out.extend(rest);
    out
}

/// Packages waiting for you to allow them (the engine keeps each in
/// `.pending/<name>.json`, beside the package), oldest first.
pub fn pending(templates: &Path) -> Vec<serde_json::Value> {
    let dir = templates.join(".pending");
    let mut out: Vec<serde_json::Value> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let file = e.file_name().to_string_lossy().into_owned();
            let name = file.strip_suffix(".json")?.to_string();
            if !plain(&name) || !dir.join(format!("{name}.zeron-template")).is_file() {
                return None;
            }
            let mut about = read_json(&e.path());
            // Where it came from, as the window may show it (never a local
            // path): `from`.
            if let Some(origin) = about.as_object_mut().and_then(|a| a.remove("origin"))
                && let Some(kind) = origin["kind"].as_str()
            {
                let mut from = serde_json::json!({ "kind": kind });
                for key in ["url", "ref"] {
                    if let Some(v) = origin[key].as_str() {
                        from[key] = serde_json::json!(v);
                    }
                }
                about["from"] = from;
            }
            (about["pending"].as_str() == Some(name.as_str())).then_some(about)
        })
        .collect();
    out.sort_by_key(|v| (v["at"].as_i64().unwrap_or(0), v["pending"].as_str().unwrap_or("").to_string()));
    out
}

/// A new template's id from the name it's given: lowercase letters, digits
/// and dashes (a template id), never Zeron's, and free (`<slug>-2`…).
pub fn new_id(templates: &Path, name: &str) -> String {
    let mut slug = String::new();
    for c in name.trim().chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let mut slug: String = slug.trim_end_matches('-').chars().take(30).collect();
    slug = slug.trim_end_matches('-').to_string();
    if slug.len() < 2 || slug == BUILTIN {
        slug = format!("template-{slug}").trim_end_matches('-').to_string();
    }
    let mut id = slug.clone();
    let mut n = 2;
    while id == BUILTIN || templates.join(&id).exists() {
        id = format!("{slug}-{n}");
        n += 1;
    }
    id
}

/// A duplicate's id and name: `<id>-copy`, `<id>-copy-2`… and "<name> copy".
pub fn copy_name(templates: &Path, from: &str) -> (String, String) {
    let name = installed(templates).into_iter().find(|t| t.id == from).map(|t| t.name).unwrap_or_else(|| from.to_string());
    let stem: String = from.chars().take(30).collect();
    let mut id = format!("{stem}-copy");
    let mut n = 2;
    while templates.join(&id).exists() {
        id = format!("{stem}-copy-{n}");
        n += 1;
    }
    (id, format!("{name} copy"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zgx-live-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(dir: &Path, rel: &str, text: &str) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn read(dir: &Path, rel: &str) -> String {
        std::fs::read_to_string(dir.join(rel)).unwrap_or_default()
    }

    fn mtime(path: &Path) -> SystemTime {
        std::fs::metadata(path).unwrap().modified().unwrap()
    }

    /// The files an error names, however the platform spells them: `/`, or
    /// Windows' `\` (as zgx names the app's modules there, and in full
    /// paths through `.live`); what isn't in `.live`, and the VM that ran
    /// it, aren't.
    #[test]
    fn an_error_names_its_files_with_either_separator() {
        let templates = dir("named");
        let live = templates.join(LIVE);
        write(&live, "screens/card.luau", "return 1");
        write(&live, "app.luau", "return 1");
        let names = |error: &str| named(error, &templates);
        assert_eq!(names("app.luau: runtime error: screens/card.luau:3: broke"), vec!["screens/card.luau"]);
        assert_eq!(
            names("app.luau: runtime error: screens\\card.luau:3: broke later\nstack traceback:\n\t[C]: in function 'error'\n\tscreens\\card.luau:3: in function <screens\\card.luau:3>"),
            vec!["screens/card.luau"],
            "Windows' own spelling, named once"
        );
        let full = format!("{}\\screens\\card.luau", live.to_string_lossy().replace('/', "\\"));
        assert_eq!(names(&format!("app.luau: runtime error: {full}:3: broke")), vec!["screens/card.luau"], "through .live, with `\\`");
        assert_eq!(names("app.luau: runtime error: screens\\other.luau:1: not the template's"), Vec::<String>::new());
        assert_eq!(names("screens\\card.luau: runtime error: elsewhere"), Vec::<String>::new(), "the VM that ran it");
        assert_eq!(
            names("app.luau: runtime error: screens\\other.luau:1: from\n\tscreens\\card.luau:3: in function"),
            vec!["screens/card.luau"]
        );
        let _ = std::fs::remove_dir_all(&templates);
    }

    /// A templates folder with a small Zeron and a template `t`.
    fn setup(name: &str) -> PathBuf {
        let root = dir(name);
        let t = root.join("templates");
        let z = t.join(ZERON);
        write(&z, "app.luau", "-- zeron app");
        write(&z, "screens/shell/sidebar.luau", "-- zeron sidebar");
        write(&z, "screens/plugins/page.luau", "-- zeron plugins page");
        write(&z, "ds/tokens.luau", "-- zeron tokens");
        write(&z, ".luaurc", "{}");
        write(&z, "zeron-template.json", "{}");
        write(&t, "t/zeron-template.json", r#"{"id":"t"}"#);
        write(&t, "t/screens/shell/sidebar.luau", "-- t sidebar");
        write(&t, "t/screens/plugins/page.luau", "-- t's plugins page, never used");
        write(&t, "t/screens/extra.luau", "-- t only");
        write(&t, "t/.luaurc", r#"{"aliases":{"evil":"/"}}"#);
        t
    }

    #[test]
    fn a_sparse_template_inherits_and_never_replaces_what_s_kept() {
        let t = setup("inherit");
        let (_, overlay) = compose(&t, "t", &BTreeSet::new(), false).unwrap();
        let live = t.join(LIVE);
        assert_eq!(read(&live, "screens/shell/sidebar.luau"), "-- t sidebar");
        assert_eq!(read(&live, "app.luau"), "-- zeron app", "inherited");
        assert_eq!(read(&live, "ds/tokens.luau"), "-- zeron tokens", "inherited");
        assert_eq!(read(&live, "screens/extra.luau"), "-- t only");
        assert_eq!(read(&live, "screens/plugins/page.luau"), "-- zeron plugins page", "never templated");
        assert_eq!(read(&live, ".luaurc"), "{}", "never templated");
        assert!(!live.join("zeron-template.json").exists());
        assert_eq!(
            overlay.from_template,
            BTreeSet::from(["screens/extra.luau".to_string(), "screens/shell/sidebar.luau".to_string()])
        );
    }

    /// R30: a protected path spelled as a case-insensitive filesystem reads
    /// it the same (`Screens/`, `ſcreens/` with U+017F, a trailing dot) is
    /// still Zeron's, and never laid down over Zeron's own.
    #[test]
    fn a_protected_path_in_another_spelling_is_still_zeron_s() {
        for rel in ["Screens/Plugins/page.luau", "\u{17f}creens/plugins/page.luau", "screens/plugins./page.luau", "ACCOUNT/x.luau", ".LUAURC", "screens\\templates\\page.luau"] {
            assert!(never_templated(rel), "{rel}");
        }
        for rel in ["screens/pluginsx/page.luau", "screens/shell/sidebar.luau", "accounts.luau"] {
            assert!(!never_templated(rel), "{rel}");
        }
        let t = setup("folded");
        write(&t, "t/\u{17f}creens/plugins/page.luau", "-- the template's plugins page");
        write(&t, "t/App.luau", "-- the template's app");
        let (_, overlay) = compose(&t, "t", &BTreeSet::new(), false).unwrap();
        let live = t.join(LIVE);
        assert_eq!(read(&live, "screens/plugins/page.luau"), "-- zeron plugins page");
        assert!(!overlay.from_template.iter().any(|r| r.contains("plugins")), "{:?}", overlay.from_template);
        // One spelling of a file written: the template's, in Zeron's place.
        assert!(overlay.from_template.contains("App.luau"));
        let names: Vec<String> = files(&live, |_| true).into_keys().filter(|r| r.eq_ignore_ascii_case("app.luau")).collect();
        assert_eq!(names.len(), 1, "{names:?}");
        assert_eq!(read(&live, &names[0]), "-- the template's app");
    }

    #[test]
    fn switching_touches_only_the_files_that_differ() {
        let t = setup("touch");
        compose(&t, BUILTIN, &BTreeSet::new(), false).unwrap();
        let live = t.join(LIVE);
        let tokens = mtime(&live.join("ds/tokens.luau"));
        std::thread::sleep(std::time::Duration::from_millis(20));
        let (changed, _) = compose(&t, "t", &BTreeSet::new(), false).unwrap();
        assert_eq!(changed, 2, "the sidebar rewritten, extra added");
        assert_eq!(mtime(&live.join("ds/tokens.luau")), tokens, "an unchanged file isn't touched");
        let (again, _) = compose(&t, "t", &BTreeSet::new(), false).unwrap();
        assert_eq!(again, 0, "nothing differs, nothing written");
        // Back to Zeron: the template's file goes, its version reverts.
        let (back, overlay) = compose(&t, BUILTIN, &BTreeSet::new(), false).unwrap();
        assert_eq!(back, 2);
        assert!(!live.join("screens/extra.luau").exists());
        assert!(overlay.from_template.is_empty());
    }

    #[test]
    fn a_failed_file_shows_zeron_s_and_a_failed_whole_shows_only_zeron() {
        let t = setup("failed");
        let failed = BTreeSet::from(["screens/shell/sidebar.luau".to_string()]);
        let (_, overlay) = compose(&t, "t", &failed, false).unwrap();
        assert_eq!(read(&t.join(LIVE), "screens/shell/sidebar.luau"), "-- zeron sidebar");
        assert_eq!(read(&t.join(LIVE), "screens/extra.luau"), "-- t only", "the rest stays the template's");
        assert!(!overlay.from_template.contains("screens/shell/sidebar.luau"));
        let (_, whole) = compose(&t, "t", &BTreeSet::new(), true).unwrap();
        assert!(whole.from_template.is_empty());
        assert!(!t.join(LIVE).join("screens/extra.luau").exists());
    }

    #[test]
    fn an_error_is_blamed_on_the_template_file_it_names() {
        let t = setup("blame");
        let (_, overlay) = compose(&t, "t", &BTreeSet::new(), false).unwrap();
        let live = t.join(LIVE).display().to_string();
        // Zeron's shell required the template's sidebar, which failed.
        let e = format!("app.luau: runtime error: {live}/app.luau:3: in require\n\t{live}/screens/shell/sidebar.luau:12: attempt to index nil");
        assert_eq!(blame(&e, &t, &overlay).as_deref(), Some("screens/shell/sidebar.luau"));
        // The template's file called Zeron's tokens with something wrong: still the template's.
        let e = format!("{live}/ds/tokens.luau:4: bad argument\nstack traceback:\n\t{live}/screens/extra.luau:9: in function");
        assert_eq!(blame(&e, &t, &overlay).as_deref(), Some("screens/extra.luau"));
        // Zeron's own error: nobody's to drop.
        assert_eq!(blame(&format!("{live}/ds/tokens.luau:4: oops"), &t, &overlay), None);
        // The app's modules are named from its folder; the VM's own name
        // (who ran it) isn't where it failed.
        assert_eq!(blame("app.luau: runtime error: screens/extra.luau:3: broke later", &t, &overlay).as_deref(), Some("screens/extra.luau"));
        assert_eq!(blame("app.luau: runtime error: ds/tokens.luau:3: oops", &t, &overlay), None);
    }

    /// R12: safe mode runs Zeron's own whatever `.active` says, so a
    /// template re-selecting itself (from a timer, a terminal, an agent)
    /// changes nothing until you pick one on the Templates page.
    #[test]
    fn safe_mode_holds_zeron_s_own_whatever_active_says() {
        let t = setup("safe");
        set_active(&t, "t").unwrap();
        super::super::preview::enter_safe_for_test(true);
        assert_eq!(active(&t), BUILTIN);
        assert_eq!(chosen(&t), "t", "what was chosen, still shown");
        set_active(&t, "t").unwrap();
        assert_eq!(active(&t), BUILTIN, "re-selected from a timer: still Zeron's own");
        super::super::preview::enter_safe_for_test(false);
        assert_eq!(active(&t), "t");
    }

    #[test]
    fn the_active_template_is_read_safely() {
        let t = setup("active");
        assert_eq!(active(&t), BUILTIN);
        set_active(&t, "t").unwrap();
        assert_eq!(active(&t), "t");
        set_active(&t, "../../etc").unwrap();
        assert_eq!(active(&t), BUILTIN);
        set_active(&t, "gone").unwrap();
        assert_eq!(active(&t), BUILTIN);
        assert!(!plain("../x") && !plain("/x") && !plain("a/.b") && plain(".luaurc") && plain("screens/x.luau"));
    }

    #[test]
    fn zeron_s_copy_holds_the_app_and_not_the_gallery_or_tests() {
        let root = dir("zeron");
        let ui = Path::new(env!("CARGO_MANIFEST_DIR")).join("ui");
        let t = root.join("templates");
        let first = sync_zeron(&ui, &t).unwrap();
        assert!(first > 10);
        let z = t.join(ZERON);
        assert!(z.join("app.luau").is_file() && z.join("screens/shell/sidebar.luau").is_file() && z.join("ds/init.luau").is_file());
        assert!(!z.join("gallery.luau").exists() && !z.join("tests").exists() && !z.join("demos").exists());
        assert!(!z.join("consent.luau").exists());
        assert!(z.join(".hashes.json").is_file() && z.join(".catalog.json").is_file());
        // Stamped with Zeron's version, the root workspace's (build.rs).
        let manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(z.join("zeron-template.json")).unwrap()).unwrap();
        let root = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml")).unwrap();
        assert!(!ZERON_VERSION.is_empty() && root.contains(&format!("version = \"{ZERON_VERSION}\"")));
        assert_eq!(manifest["zeron_version"], ZERON_VERSION);
        assert_eq!(sync_zeron(&ui, &t).unwrap(), 0, "nothing new, nothing written");
    }

    #[test]
    fn a_survey_copies_zeron_s_own_only_when_ui_changed() {
        let root = dir("survey");
        let ui = root.join("ui");
        write(&ui, "app.luau", "-- app");
        write(&ui, "screens/x.luau", "-- x");
        write(&ui, "demos/d.luau", "-- not the app's");
        let t = root.join("templates");
        let first = survey(&ui, &t, None);
        assert!(first.synced.as_ref().is_ok_and(|n| *n > 0));
        assert_eq!(first.id, BUILTIN);
        // Nothing changed: nothing copied, the same stamp.
        let again = survey(&ui, &t, Some(&first.ui_stamp));
        assert_eq!((again.synced.clone().unwrap(), &again.ui_stamp), (0, &first.ui_stamp));
        // A demo isn't the app's.
        std::thread::sleep(std::time::Duration::from_millis(5));
        write(&ui, "demos/d.luau", "-- changed");
        assert_eq!(survey(&ui, &t, Some(&first.ui_stamp)).ui_stamp, first.ui_stamp);
        // A screen saved: copied again.
        write(&ui, "screens/x.luau", "-- x, saved");
        let saved = survey(&ui, &t, Some(&first.ui_stamp));
        assert_ne!(saved.ui_stamp, first.ui_stamp);
        assert_eq!(saved.synced.unwrap(), 1);
        assert_eq!(read(&t.join(ZERON), "screens/x.luau"), "-- x, saved");
        // A template in use: its files as they stand, and a stamp that follows them.
        write(&t, "mine/zeron-template.json", r#"{"name":"Mine"}"#);
        write(&t, "mine/screens/x.luau", "-- mine");
        set_active(&t, "mine").unwrap();
        let used = survey(&ui, &t, Some(&saved.ui_stamp));
        assert_eq!(used.id, "mine");
        assert!(used.files.iter().any(|(rel, s)| rel == "screens/x.luau" && !s.is_empty()));
        std::thread::sleep(std::time::Duration::from_millis(5));
        write(&t, "mine/screens/x.luau", "-- mine, edited");
        assert_ne!(survey(&ui, &t, Some(&saved.ui_stamp)).template_stamp, used.template_stamp);
    }

    #[test]
    fn ui_is_watched_by_its_folders_scripts_and_themes() {
        let ui = Path::new(env!("CARGO_MANIFEST_DIR")).join("ui");
        let t = setup("watched");
        let (files_, dirs) = watched(&t, &ui);
        // (By `/`, as on every platform: Windows' paths say `\`.)
        let rel = |p: &PathBuf| p.strip_prefix(&ui).map(|r| r.to_string_lossy().replace('\\', "/")).unwrap_or_default();
        assert!(files_.iter().any(|p| rel(p) == "app.luau"));
        assert!(files_.iter().any(|p| rel(p).starts_with("ds/themes/") && rel(p).ends_with(".json")));
        assert!(!files_.iter().any(|p| rel(p).ends_with(".svg") || rel(p).ends_with(".webp")), "art is heard by its folder");
        assert!(!files_.iter().chain(&dirs).any(|p| rel(p).starts_with("demos") || rel(p).starts_with("tests")));
        assert!(dirs.iter().any(|p| rel(p) == "assets"));
        // A template in use: all of it.
        set_active(&t, "t").unwrap();
        let (files_, dirs) = watched(&t, &ui);
        assert!(files_.contains(&t.join("t/screens/extra.luau")) && dirs.contains(&t.join("t/screens")));
    }

    #[test]
    fn templates_are_listed_zeron_first_and_out_of_date_is_told() {
        let t = setup("installed");
        std::fs::write(t.join(ZERON).join("zeron-template.json"), r#"{"name":"Zeron","author":"Zeron","zeron_version":"0.3.0"}"#).unwrap();
        std::fs::write(t.join(ZERON).join(".hashes.json"), r#"{"screens/shell/sidebar.luau":"now"}"#).unwrap();
        // Made for an earlier Zeron: out of date.
        write(&t, "t/zeron-template.json", r#"{"name":"Tidy","author":"you","zeron_version":"0.2.9"}"#);
        write(&t, "t/.base.json", r#"{"screens/shell/sidebar.luau":"then"}"#);
        // No version recorded: never out of date.
        write(&t, "a/zeron-template.json", r#"{"name":"Airy"}"#);
        // This Zeron's, though a file it copied has changed since (a save
        // while developing): not out of date.
        write(&t, "b/zeron-template.json", r#"{"name":"Brisk","zeron_version":"0.3.0"}"#);
        write(&t, "b/.base.json", r#"{"screens/shell/sidebar.luau":"then"}"#);
        write(&t, "notatemplate/file.txt", "x");
        let list = installed(&t);
        let ids: Vec<&str> = list.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, vec![BUILTIN, "a", "b", "t"]);
        let stale: Vec<bool> = list.iter().map(|i| i.out_of_date).collect();
        assert_eq!(stale, vec![false, false, false, true]);
        assert!(version_out_of_date("0.2.9", "0.3.0") && !version_out_of_date("", "0.3.0") && !version_out_of_date("0.3.0", ""));
        assert!(!version_out_of_date("0.0.0", "0.3.0"), "a development stamp isn't a version");
        let (id, name) = copy_name(&t, "t");
        assert_eq!((id.as_str(), name.as_str()), ("t-copy", "Tidy copy"));
        assert_eq!(new_id(&t, "My Dark  Theme!"), "my-dark-theme");
        assert_eq!(new_id(&t, "T"), "template-t");
        assert_eq!(new_id(&t, "Zeron"), "template-zeron");
        assert_eq!(new_id(&t, "  "), "template");
        std::fs::create_dir_all(t.join("airy")).unwrap();
        assert_eq!(new_id(&t, "Airy"), "airy-2");
        assert_eq!(list[3].zeron_version, "0.2.9");
    }

    #[test]
    fn a_bundle_s_interface_isn_t_watched_file_by_file() {
        let root = dir("watched-bundle");
        let templates = root.join("templates");
        let ui = root.join("Zeron.app/Contents/Resources/ui");
        write(&ui, "app.luau", "return 1");
        write(&ui, "screens/shell/page.luau", "return 2");
        std::fs::create_dir_all(&templates).unwrap();
        let (files_, dirs) = watched(&templates, &ui);
        assert_eq!(files_, vec![templates.join(ACTIVE)]);
        assert_eq!(dirs, vec![templates.clone()]);
        // A source tree, which is edited as it runs, still is.
        let source = root.join("apps/zgx/ui");
        write(&source, "app.luau", "return 1");
        let (files_, dirs) = watched(&templates, &source);
        assert!(files_.contains(&source.join("app.luau")));
        assert!(dirs.contains(&source));
    }
}
