//! The templates that ship with Zeron beside the default: Zeron v2, the
//! single-page layout from before Zeron v3 (the default's id stays
//! `zeron`, whatever its name).
//!
//! An official template is like anyone's: a folder of files laid over the
//! default (`live::compose`). Its files are in the app's own `ui/`, under
//! `templates/<id>/` (never part of the default's copy: `live::not_the_app`),
//! with its `.base.json`, the hash of each default file it replaces as it was
//! when the two were last brought in step (the staleness guard's record:
//! `tests::zeron_v2_is_in_step_with_the_default`, and `template_status`'s, as
//! for any template).
//!
//! Each sync of the default's copy writes each one into
//! `<data>/templates/<id>/` (`install`), rewriting only what changed, so it
//! comes and updates with Zeron:
//!
//! - A folder is Zeron's to write (`ours`) only if it's a real folder whose
//!   manifest is marked with the template's id (`MARK`), holding nothing but
//!   what Zeron writes there now or wrote before (its `.base.json` names
//!   those): one with anything more (an agent's screen, an imported
//!   package's record) or not marked is someone's, and never touched. The
//!   official one then takes the next free id (`<id>-2`…).
//! - One exactly as this build writes it (`verified`) is Zeron's: listed as
//!   built in (no Delete, no Export, "By Zeron"), and run with the app's
//!   powers without asking, as the default is, its files being Zeron's own.
//!   Anything else is an ordinary template: a preview until you keep it.

use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::Mutex,
};

use serde_json::json;

use super::live;

/// The manifest field that marks a folder as an official template's (its id).
pub const MARK: &str = "builtin_template";
/// Where in `ui/` the official templates' files are.
pub const SOURCE: &str = "templates";
const MANIFEST: &str = "zeron-template.json";
const BASE: &str = ".base.json";

/// An official template, as Zeron ships it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Official {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
}

/// Every official template, in the order the Templates page lists them
/// (after the default).
pub const OFFICIAL: &[Official] =
    &[Official { id: "zeron-v2", name: "Zeron v2", description: "The single-page layout from before Zeron v3." }];

/// The default template's name and description (its id is `live::BUILTIN`).
pub const DEFAULT_NAME: &str = "Zeron v3";
pub const DEFAULT_DESCRIPTION: &str = "The default interface. Chats can sit side by side.";

pub fn find(id: &str) -> Option<Official> {
    OFFICIAL.iter().copied().find(|o| o.id == id)
}

/// Whether `rel` (from `ui/`) is one of the official templates' files.
pub fn source_file(rel: &str) -> bool {
    rel.strip_prefix(SOURCE).is_some_and(|r| r.starts_with('/'))
}

fn source(ui: &Path, o: &Official) -> PathBuf {
    ui.join(SOURCE).join(o.id)
}

fn read_json(path: &Path) -> serde_json::Value {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn manifest(o: &Official, id: &str) -> serde_json::Value {
    json!({
        "id": id,
        "name": o.name,
        "description": o.description,
        "author": "Zeron",
        "version": live::ZERON_VERSION,
        "zeron_version": live::ZERON_VERSION,
        MARK: o.id,
    })
}

/// What this build writes for `o` at `id`: each file by its path, its bytes
/// (its interface's files, its `.base.json`, its manifest). Empty if this
/// build doesn't ship its files.
pub fn wanted(ui: &Path, o: &Official, id: &str) -> BTreeMap<String, Vec<u8>> {
    let dir = source(ui, o);
    let mut out: BTreeMap<String, Vec<u8>> = live::files(&dir, |rel| !live::template_meta(rel))
        .into_iter()
        .filter_map(|(rel, path)| std::fs::read(path).ok().map(|b| (rel, b)))
        .collect();
    if out.is_empty() {
        return out;
    }
    out.insert(BASE.to_string(), std::fs::read(dir.join(BASE)).unwrap_or_else(|_| b"{}".to_vec()));
    out.insert(MANIFEST.to_string(), serde_json::to_vec_pretty(&manifest(o, id)).unwrap_or_default());
    out
}

/// Every regular file under `dir`, dotfiles too, by its path from it (but
/// `live::put`'s temporary files, mid-write); none if anything in it is a
/// link or not a regular file or folder.
fn contents(dir: &Path) -> Option<BTreeMap<String, PathBuf>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(at) = stack.pop() {
        for entry in std::fs::read_dir(&at).ok()?.flatten() {
            let kind = entry.file_type().ok()?;
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if kind.is_dir() {
                stack.push(path);
            } else if kind.is_file() {
                if name.starts_with('.') && name.ends_with(".tmp") {
                    continue;
                }
                let rel = path.strip_prefix(dir).ok()?.to_string_lossy().replace('\\', "/");
                out.insert(rel, path);
            } else {
                return None;
            }
        }
    }
    Some(out)
}

/// Whether `dir` is `o`'s to write: free, or a real folder marked as `o`'s
/// holding only what Zeron writes there (`want`) or wrote there before (its
/// `.base.json`'s files).
fn ours(dir: &Path, o: &Official, want: &BTreeMap<String, Vec<u8>>) -> bool {
    let Ok(meta) = std::fs::symlink_metadata(dir) else { return true };
    if !meta.is_dir() {
        return false;
    }
    if read_json(&dir.join(MANIFEST))[MARK].as_str() != Some(o.id) {
        return false;
    }
    let Some(held) = contents(dir) else { return false };
    let before = read_json(&dir.join(BASE));
    held.keys().all(|rel| want.contains_key(rel) || before.get(rel).is_some())
}

/// Where `o` is (or goes): its own id where that's free or already its,
/// else the first `<id>-N` that is.
fn place(ui: &Path, templates: &Path, o: &Official) -> (String, PathBuf) {
    let mut id = o.id.to_string();
    let mut n = 2;
    loop {
        let dir = templates.join(&id);
        if ours(&dir, o, &wanted(ui, o, &id)) {
            return (id, dir);
        }
        id = format!("{}-{n}", o.id);
        n += 1;
    }
}

/// `o` written into `templates` from `ui` (or brought up to date there):
/// its id. Its manifest goes last (a folder with a manifest is a
/// template), and files an earlier Zeron wrote that this one doesn't, go.
pub fn install_one(ui: &Path, templates: &Path, o: &Official) -> Result<String, String> {
    let (id, dir) = place(ui, templates, o);
    let want = wanted(ui, o, &id);
    if want.is_empty() {
        return Err(format!("This build has no files for {}", o.name));
    }
    let write = |rel: &str, bytes: &[u8]| live::put(&dir.join(rel), bytes).map_err(|e| format!("Couldn't write {}'s {rel}: {e}", o.name));
    for (rel, bytes) in &want {
        if rel != MANIFEST {
            write(rel, bytes)?;
        }
    }
    write(MANIFEST, &want[MANIFEST])?;
    for (rel, path) in contents(&dir).unwrap_or_default() {
        if !want.contains_key(&rel) {
            std::fs::remove_file(path).map_err(|e| format!("Couldn't remove {}'s {rel}: {e}", o.name))?;
        }
    }
    forget(&dir);
    Ok(id)
}

/// Every official template written into `templates` (or brought up to
/// date there): each one's id. What failed is told and skipped.
pub fn install(ui: &Path, templates: &Path) -> Vec<String> {
    let mut ids = Vec::new();
    for o in OFFICIAL {
        match install_one(ui, templates, o) {
            Ok(id) => ids.push(id),
            Err(error) => eprintln!("zeron-zgx: the {} template wasn't installed: {error}", o.name),
        }
    }
    ids
}

/// Whether an official template this build ships (from `ui`) has no folder
/// of its own in `templates` (deleted by hand, or set aside): a look then
/// installs it again, so it stays as the default does. A read of the
/// folder, and a manifest or two.
pub fn missing(ui: &Path, templates: &Path) -> bool {
    let names: Vec<String> =
        std::fs::read_dir(templates).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    OFFICIAL.iter().filter(|o| source(ui, o).is_dir()).any(|o| {
        !names.iter().any(|id| {
            let ours = id == o.id || id.strip_prefix(&format!("{}-", o.id)).is_some_and(|n| n.chars().all(|c| c.is_ascii_digit()));
            ours && read_json(&templates.join(id).join(MANIFEST))[MARK].as_str() == Some(o.id)
        })
    })
}

/// The official template `dir` is, if this build wrote it as it stands:
/// marked, at its own id (or `<id>-N`), holding exactly what `install`
/// writes there, byte for byte.
pub fn verified_at(ui: &Path, dir: &Path) -> Option<Official> {
    let o = find(read_json(&dir.join(MANIFEST))[MARK].as_str()?)?;
    let id = dir.file_name()?.to_str()?;
    let at_its_id = id == o.id || id.strip_prefix(&format!("{}-", o.id)).is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
    if !at_its_id || !std::fs::symlink_metadata(dir).is_ok_and(|m| m.is_dir()) {
        return None;
    }
    let want = wanted(ui, &o, id);
    let held = contents(dir)?;
    let same = !want.is_empty()
        && held.len() == want.len()
        && want.iter().all(|(rel, bytes)| held.get(rel).and_then(|p| std::fs::read(p).ok()).is_some_and(|b| b == *bytes));
    same.then_some(o)
}

/// A folder as it stands, every file of it, dotfiles too (`.imported.json`
/// makes it someone's): each one's path, size and time written, a stat each.
fn folder_stamp(dir: &Path) -> String {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut h = DefaultHasher::new();
    match contents(dir) {
        Some(files) => {
            for (rel, path) in files {
                rel.hash(&mut h);
                live::file_stamp(&path).hash(&mut h);
            }
        }
        None => "not a plain folder".hash(&mut h),
    }
    format!("{:016x}", h.finish())
}

/// What `verified` found, by folder, and the folder as it stood
/// (`folder_stamp`), so a look reads the files again only once one changes.
static VERIFIED: Mutex<Option<HashMap<PathBuf, (String, Option<Official>)>>> = Mutex::new(None);

fn forget(dir: &Path) {
    if let Ok(mut cache) = VERIFIED.lock()
        && let Some(map) = cache.as_mut()
    {
        map.remove(dir);
    }
}

/// `verified_at`, against the `ui/` this app runs (`crate::ui_dir`).
pub fn verified(dir: &Path) -> Option<Official> {
    let stamp = folder_stamp(dir);
    if let Ok(cache) = VERIFIED.lock()
        && let Some((at, found)) = cache.as_ref().and_then(|m| m.get(dir))
        && *at == stamp
    {
        return *found;
    }
    let found = verified_at(&crate::ui_dir(), dir);
    if let Ok(mut cache) = VERIFIED.lock() {
        cache.get_or_insert_with(HashMap::new).insert(dir.to_path_buf(), (stamp, found));
    }
    found
}

/// A template's place in the Templates page's list: the default first,
/// then the official ones in Zeron's order, then the rest (by name).
pub fn order(id: &str, official: Option<Official>) -> usize {
    match official {
        _ if id == live::BUILTIN => 0,
        Some(o) => 1 + OFFICIAL.iter().position(|x| *x == o).unwrap_or(0),
        None => usize::MAX,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zgx-official-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(dir: &Path, rel: &str, text: &str) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn ui() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("ui")
    }

    const V2: Official = OFFICIAL[0];

    /// The files Zeron v2 lays over the default, as `ui/templates/zeron-v2`
    /// has them.
    fn overlay() -> Vec<String> {
        live::files(&source(&ui(), &V2), |_| true).into_keys().collect()
    }

    /// The staleness guard. Zeron v2 is the default's single-page layout
    /// as it stood before Zeron v3: each file it replaces is its own copy,
    /// which goes stale as the default moves on. Its `.base.json` records
    /// the hash of each of those default files as it was when v2 was last
    /// brought in step with it. Change one, and this fails until whoever
    /// changed it has looked at v2's copy: brought the fix over if it
    /// applies to the single-page layout (a bug fixed, a renamed ds prop),
    /// or not (a v3-only change), then recorded it with
    /// `ZERON_V2_BLESS=1 cargo test --bin zeron-zgx zeron_v2_is_in_step`.
    #[test]
    fn zeron_v2_is_in_step_with_the_default() {
        let src = source(&ui(), &V2);
        let base_path = src.join(BASE);
        let base: BTreeMap<String, String> = serde_json::from_slice(&std::fs::read(&base_path).unwrap_or_default()).unwrap_or_default();
        let now: BTreeMap<String, String> = overlay()
            .into_iter()
            .map(|rel| {
                let bytes = std::fs::read(ui().join(&rel)).unwrap_or_default();
                (rel, live::hash(&bytes))
            })
            .collect();
        if std::env::var_os("ZERON_V2_BLESS").is_some() {
            let mut json = serde_json::to_vec_pretty(&now).unwrap();
            json.push(b'\n');
            std::fs::write(&base_path, json).unwrap();
            return;
        }
        let moved: Vec<&String> = now.keys().filter(|rel| base.get(*rel) != now.get(*rel)).collect();
        let gone: Vec<&String> = base.keys().filter(|rel| !now.contains_key(*rel)).collect();
        assert!(
            moved.is_empty() && gone.is_empty(),
            "The default's {moved:?} changed since Zeron v2 was last brought in step with it (and {gone:?} left v2). \
             v2 keeps its own copy of each (ui/templates/zeron-v2/). Bring your change over to v2's copy if it \
             applies to the single-page layout, then record it: ZERON_V2_BLESS=1 cargo test --bin zeron-zgx zeron_v2_is_in_step"
        );
    }

    /// v2 replaces only the files where the two layouts part, each with
    /// something of its own (never the default's bytes again), and none
    /// that are never a template's.
    #[test]
    fn zeron_v2_differs_from_v3_where_the_layouts_part() {
        let files = overlay();
        assert_eq!(
            files,
            [
                "ds/sidebar.luau",
                "ds/tokens.luau",
                "screens/chat/composer.luau",
                "screens/chat/lane_tab.luau",
                "screens/panel/init.luau",
                "screens/shell/init.luau",
                "screens/shell/page.luau",
                "screens/shell/view.luau",
            ],
            "Zeron v2's files"
        );
        for rel in &files {
            let v2 = std::fs::read_to_string(source(&ui(), &V2).join(rel)).unwrap();
            let v3 = std::fs::read_to_string(ui().join(rel)).unwrap();
            assert_ne!(v2, v3, "{rel}: v2's copy is the default's own; leave it out of v2");
            assert!(!live::never_templated(rel), "{rel}");
            // v2 is one page: none of v3's split chats or inset cards.
            assert!(!v2.contains("screens/shell/split") && !v2.contains("zeron.inset"), "{rel}: v2 has v3's split or inset");
        }
        let page = std::fs::read_to_string(ui().join("screens/shell/page.luau")).unwrap();
        assert!(page.contains("@app/screens/shell/split"), "v3's page lays out split chats");
        assert!(ui().join("screens/shell/split.luau").is_file());
    }

    #[test]
    fn zeron_v2_installs_as_zeron_s_and_never_over_yours() {
        let t = dir("install");
        // Yours, under v2's id.
        write(&t, "zeron-v2/zeron-template.json", r#"{"name":"My v2","author":"you"}"#);
        write(&t, "zeron-v2/screens/shell/page.luau", "-- mine");
        let ids = install(&ui(), &t);
        assert_eq!(ids, ["zeron-v2-2"]);
        assert_eq!(std::fs::read_to_string(t.join("zeron-v2/screens/shell/page.luau")).unwrap(), "-- mine", "yours, untouched");
        assert!(verified_at(&ui(), &t.join("zeron-v2")).is_none());
        let at = t.join("zeron-v2-2");
        assert_eq!(verified_at(&ui(), &at), Some(V2));
        let held: Vec<String> = contents(&at).unwrap().into_keys().collect();
        let mut want: Vec<String> = overlay();
        want.extend([BASE.to_string(), MANIFEST.to_string()]);
        want.sort();
        assert_eq!(held, want, "its interface, its .base.json and its manifest: nothing else");
        let m = read_json(&at.join(MANIFEST));
        assert_eq!((m["name"].as_str(), m["author"].as_str(), m[MARK].as_str()), (Some("Zeron v2"), Some("Zeron"), Some("zeron-v2")));
        assert_eq!(m["zeron_version"], live::ZERON_VERSION);
        // Again: the same place, nothing rewritten.
        let stamp = live::template_stamp(&at);
        assert_eq!(install(&ui(), &t), ids);
        assert_eq!(live::template_stamp(&at), stamp);
        // Edited in place: not Zeron's (a preview, deletable) until the next
        // install puts Zeron's back.
        write(&t, "zeron-v2-2/screens/shell/page.luau", "ui.call('terminal.start', {})");
        assert!(verified_at(&ui(), &at).is_none());
        install(&ui(), &t);
        assert_eq!(verified_at(&ui(), &at), Some(V2));
        // A file an earlier Zeron wrote and this one doesn't: gone. One
        // added by someone: the folder isn't Zeron's any more.
        write(&t, "zeron-v2-2/.base.json", r#"{"screens/old.luau":"x"}"#);
        write(&t, "zeron-v2-2/screens/old.luau", "-- an earlier v2's");
        install(&ui(), &t);
        assert!(!at.join("screens/old.luau").exists());
        assert_eq!(verified_at(&ui(), &at), Some(V2));
        write(&t, "zeron-v2-2/screens/extra.luau", "-- an agent's");
        assert!(!ours(&at, &V2, &wanted(&ui(), &V2, "zeron-v2-2")));
        // Set aside (an agent's delete, by hand): a look puts it back.
        assert!(!missing(&ui(), &t));
        let gone = dir("missing");
        assert!(missing(&ui(), &gone));
        assert!(!missing(&gone.join("no-ui"), &gone), "only what this build ships");
        install(&ui(), &gone);
        assert!(!missing(&ui(), &gone));
        std::fs::remove_dir_all(gone.join("zeron-v2")).unwrap();
        assert!(missing(&ui(), &gone));
        assert_eq!(install(&ui(), &t), ["zeron-v2-3"]);
        assert_eq!(std::fs::read_to_string(at.join("screens/extra.luau")).unwrap(), "-- an agent's");
    }

    /// R18, as for the retired theme templates: a folder that says it's
    /// Zeron's (an imported package's, one written by hand) is never
    /// listed or trusted as Zeron's unless it's this build's, byte for byte.
    #[test]
    fn a_folder_posing_as_zeron_v2_is_never_zeron_s() {
        let t = dir("posing");
        install(&ui(), &t);
        let at = t.join("zeron-v2");
        assert!(verified_at(&ui(), &at).is_some());
        // An imported package's record beside it.
        write(&t, "zeron-v2/.imported.json", r#"{"key":"k","author":{"kind":"unknown"}}"#);
        assert!(verified_at(&ui(), &at).is_none());
        // Marked, at another id.
        let elsewhere = t.join("my-v2");
        for (rel, bytes) in wanted(&ui(), &V2, "my-v2") {
            let path = elsewhere.join(&rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
        assert!(verified_at(&ui(), &elsewhere).is_none());
        // A link in it.
        let linked = dir("linked");
        install(&ui(), &linked);
        let page = linked.join("zeron-v2/screens/shell/page.luau");
        std::fs::remove_file(&page).unwrap();
        std::os::unix::fs::symlink(ui().join("templates/zeron-v2/screens/shell/page.luau"), &page).unwrap();
        assert!(verified_at(&ui(), &linked.join("zeron-v2")).is_none());
        // The page and the trust follow `verified`.
        let listed = live::installed(&t);
        assert!(listed.iter().find(|i| i.id == "zeron-v2").unwrap().official.is_none());
        std::fs::remove_file(t.join("zeron-v2/.imported.json")).unwrap();
        write(&t, ".zeron/zeron-template.json", r#"{"name":"Zeron v3","author":"Zeron"}"#);
        let listed = live::installed(&t);
        assert_eq!(listed.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), [live::BUILTIN, "zeron-v2", "my-v2"], "the default, Zeron's, then the rest");
        assert_eq!(listed[1].official, Some(V2));
    }
}
