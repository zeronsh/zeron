//! Templates Zeron took down, refused before any of one is laid over Zeron's
//! own: at a launch (a warm one too), at every look, and where one is used,
//! copied or exported. With no engine running, or its answer not here yet.
//!
//! The engine writes what it knows, whole, as `.blocked.json` in the
//! templates folder (`{ format, blocks = { { id, version, reason, at } } }`,
//! `id` the store's). The last one read is kept beside what you allowed
//! (`template-trust/blocked.json`), and stands while the engine's is gone or
//! can't be read: deleting one file lifts nothing.
//!
//! Which template a folder is, its own files say, never its name: the record
//! it was installed from (`.imported.json`, from the store: its id and
//! version) and what it was copied from (`.lineage.json`, through copies of
//! copies). A file's or a link's isn't followed by any take-down.
//!
//! The engine's word on a row (`ListTemplates`, `blocked`) is a block at once,
//! kept beside the copy (`blocked-rows.json`): the only cover while it can't
//! write its file. One is let go of only when a later answer has that row
//! without it and the file reads and doesn't name it.
//!
//! What can't be read is refused, so far as it came from the store: with no
//! list to be had (it's gone, or can't be read), every template with a store
//! record or a lineage; with one, a template whose own record can't be read.
//! It comes back by itself once Zeron can check. (A template you kept runs as Zeron and can undo all
//! of this: it guards the honest case.)

use std::path::{Path, PathBuf};

use serde_json::{Value as Json, json};

use super::{live, preview};

/// The engine's, in the templates folder.
pub const FILE: &str = ".blocked.json";
const FORMAT: &str = "zeron-template-blocks:v1";
const IMPORTED: &str = ".imported.json";
const LINEAGE: &str = ".lineage.json";
/// The most any of these files is read of.
const MOST: u64 = 8 * 1024 * 1024;

/// A version of a store template that was taken down.
#[derive(Clone, Debug, PartialEq)]
struct Block {
    id: String,
    version: String,
    reason: Option<String>,
}

/// Why a template can't be used.
#[derive(Clone, Debug, PartialEq)]
pub enum Why {
    /// Zeron took it down, and why, if it was said.
    TakenDown { reason: Option<String> },
    /// Whether it did can't be told for now.
    Unchecked,
}

impl Why {
    /// The word the Templates page says in its own words.
    pub fn word(&self) -> &'static str {
        match self {
            Why::TakenDown { .. } => "template_taken_down",
            Why::Unchecked => "template_unchecked",
        }
    }
}

/// A file, read: it isn't there, it can't be read (or isn't a plain file, or
/// isn't what it should be), or what it says.
enum Read<T> {
    Missing,
    Bad,
    Says(T),
}

fn read(path: &Path) -> Read<Json> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Read::Missing,
        Err(_) => return Read::Bad,
    };
    if !meta.is_file() || meta.len() > MOST {
        return Read::Bad;
    }
    match std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()) {
        Some(v) => Read::Says(v),
        None => Read::Bad,
    }
}

fn words(v: &Json) -> Option<String> {
    v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned)
}

/// A list as it's written: every entry of it, or it can't be read.
fn list(path: &Path) -> Read<(Json, Vec<Block>)> {
    let v = match read(path) {
        Read::Missing => return Read::Missing,
        Read::Bad => return Read::Bad,
        Read::Says(v) => v,
    };
    if v["format"] != FORMAT {
        return Read::Bad;
    }
    let Some(entries) = v["blocks"].as_array() else { return Read::Bad };
    let blocks: Option<Vec<Block>> =
        entries.iter().map(|b| Some(Block { id: words(&b["id"])?, version: words(&b["version"])?, reason: words(&b["reason"]) })).collect();
    match blocks {
        Some(blocks) => Read::Says((v, blocks)),
        None => Read::Bad,
    }
}

/// Where the last list read is kept.
fn copy_path() -> Option<PathBuf> {
    #[cfg(test)]
    return TEST_COPY.with(|c| c.borrow().clone());
    #[cfg(not(test))]
    zgx_script::app_data_dir(crate::APP_ID).map(|d| d.join("template-trust").join("blocked.json"))
}

#[cfg(test)]
thread_local! {
    /// A test's own copy's place (none: it keeps none).
    static TEST_COPY: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub fn copy_for_test(path: Option<PathBuf>) {
    TEST_COPY.with(|c| *c.borrow_mut() = path);
}

/// What's known to be taken down.
enum Known {
    /// The engine's list (kept as it's read), else the last one read.
    Says(Vec<Block>),
    /// There's one, and neither can be read.
    Unreadable,
    /// There's none: not written yet, or gone.
    Missing,
}

fn known(templates: &Path, copy: Option<&Path>) -> Known {
    let theirs = list(&templates.join(FILE));
    if let Read::Says((written, blocks)) = theirs {
        if let Some(copy) = copy
            && let Err(error) = live::put(copy, &serde_json::to_vec_pretty(&written).unwrap_or_default())
        {
            eprintln!("zeron-zgx: templates: the taken-down list wasn't kept: {error}");
        }
        return Known::Says(blocks);
    }
    match copy.map_or(Read::Missing, list) {
        Read::Says((_, blocks)) => Known::Says(blocks),
        Read::Missing if matches!(theirs, Read::Missing) => Known::Missing,
        _ => Known::Unreadable,
    }
}

/// Where what the engine said of rows is kept.
fn rows_path(copy: &Path) -> PathBuf {
    copy.with_file_name("blocked-rows.json")
}

/// What the engine said of rows, each with the folder it said it of. (One
/// that can't be read is as if there were none: the list is what's read
/// before it.)
fn said_of_rows(copy: Option<&Path>) -> Vec<(String, Block)> {
    let Some(Read::Says((_, entries))) = copy.map(|copy| rows(&rows_path(copy))) else { return Vec::new() };
    entries
}

fn rows(path: &Path) -> Read<(Json, Vec<(String, Block)>)> {
    let v = match read(path) {
        Read::Missing => return Read::Missing,
        Read::Bad => return Read::Bad,
        Read::Says(v) => v,
    };
    let Some(all) = v.as_array() else { return Read::Bad };
    let entries = all
        .iter()
        .filter_map(|b| Some((words(&b["folder"])?, Block { id: words(&b["id"])?, version: words(&b["version"])?, reason: words(&b["reason"]) })))
        .collect();
    Read::Says((v, entries))
}

/// What the engine answered of the templates (`ListTemplates`): each row it
/// says is taken down is, from now on, and one it no longer says (or whose
/// folder is gone) is let go of if the list reads and doesn't name it. Whether anything changed.
pub fn take_rows(templates: &Path, said: &Json) -> bool {
    let Some(copy) = copy_path() else { return false };
    let Some(answered) = said["templates"].as_array() else { return false };
    let listed = match list(&templates.join(FILE)) {
        Read::Says((_, blocks)) => Some(blocks),
        _ => None,
    };
    let was = said_of_rows(Some(&copy));
    let mut now: Vec<(String, Block)> = Vec::new();
    for (folder, block) in &was {
        let row = answered.iter().find(|row| row["id"] == folder.as_str());
        // (Its folder gone, no row of it will ever come: the list is the word.)
        let unsaid = row.map_or_else(|| !templates.join(folder).exists(), |row| !row["blocked"].is_object());
        let clear = unsaid
            && listed.as_ref().is_some_and(|l| !l.iter().any(|b| b.id == block.id && same_version(&b.version, &block.version)));
        if !clear {
            now.push((folder.clone(), block.clone()));
        }
    }
    for row in answered {
        let blocked = &row["blocked"];
        // (Unchecked is of now, not of what it is: nothing to keep.)
        let (Some(folder), Some(id), Some(version)) = (words(&row["id"]), words(&blocked["id"]), words(&blocked["version"])) else { continue };
        let block = Block { id, version, reason: words(&blocked["reason"]) };
        now.retain(|(f, b)| !(*f == folder && b.id == block.id && same_version(&b.version, &block.version)));
        now.push((folder, block));
    }
    now.sort_by(|a, b| (&a.0, &a.1.id, &a.1.version).cmp(&(&b.0, &b.1.id, &b.1.version)));
    let mut before = was;
    before.sort_by(|a, b| (&a.0, &a.1.id, &a.1.version).cmp(&(&b.0, &b.1.id, &b.1.version)));
    if now == before {
        return false;
    }
    let written = Json::Array(now.iter().map(|(folder, b)| json!({ "folder": folder, "id": b.id, "version": b.version, "reason": b.reason })).collect());
    if let Err(error) = live::put(&rows_path(&copy), &serde_json::to_vec_pretty(&written).unwrap_or_default()) {
        eprintln!("zeron-zgx: templates: what was taken down wasn't kept: {error}");
    }
    true
}

/// A store version as a record names one: the store's id, a version and its
/// content hash, each as it's written; else it's no record (the engine's rule
/// too).
fn of_store(id: &Json, version: &Json, hash: &Json) -> Option<(String, String)> {
    let (id, version, hash) = (id.as_str()?, version.as_str()?, hash.as_str()?);
    let named = (2..=40).contains(&id.len())
        && id.bytes().enumerate().all(|(i, b)| b.is_ascii_lowercase() || b.is_ascii_digit() || (b == b'-' && i > 0));
    let hashed = hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    (named && hashed && (1..=128).contains(&version.len())).then(|| (id.to_owned(), version.to_owned()))
}

/// The store versions a folder is or came from. `None`: it can't be told.
fn came_from(dir: &Path) -> Option<Vec<(String, String)>> {
    let mut out = Vec::new();
    match read(&dir.join(IMPORTED)) {
        Read::Missing => {}
        Read::Bad => return None,
        Read::Says(v) => {
            if !v.is_object() {
                return None;
            }
            let source = &v["source"];
            // (A source there with no kind is no word of where it's from.)
            match source["kind"].as_str() {
                None if source.is_null() => {}
                None => return None,
                Some("store") => out.push(of_store(&source["store"]["id"], &v["version"], &source["store"]["hash"])?),
                Some(_) => {}
            }
        }
    }
    match read(&dir.join(LINEAGE)) {
        Read::Missing => {}
        Read::Bad => return None,
        Read::Says(v) => {
            for entry in v.as_array()? {
                out.push(of_store(&entry["id"], &entry["version"], &entry["hash"])?);
            }
        }
    }
    Some(out)
}

/// Whether two versions are one: by precedence, which build metadata
/// (`+…`) is no part of.
fn same_version(a: &str, b: &str) -> bool {
    let plain = |v: &str| v.split('+').next().unwrap_or_default().to_owned();
    plain(a) == plain(b)
}

/// Why template `id` in `templates` can't be used, if it can't. Never
/// Zeron's own.
pub fn blocked(templates: &Path, id: &str) -> Option<Why> {
    if id == live::BUILTIN || !live::plain(id) {
        return None;
    }
    let dir = templates.join(id);
    let copy = copy_path();
    let came_from = came_from(&dir);
    let named = |blocks: &[Block]| {
        let from = came_from.as_deref().unwrap_or_default();
        blocks
            .iter()
            .find(|b| from.iter().any(|(id, version)| *id == b.id && same_version(version, &b.version)))
            .map(|b| Why::TakenDown { reason: b.reason.clone() })
    };
    let rows: Vec<(String, Block)> = said_of_rows(copy.as_deref());
    // (The engine's word on this very folder, whatever its record says now;
    // and on any other of the same version.)
    let said: Vec<Block> = rows.iter().map(|(_, b)| b.clone()).collect();
    if let Some(why) = named(&said) {
        return Some(why);
    }
    if came_from.is_none()
        && let Some((_, b)) = rows.iter().find(|(folder, _)| folder == id)
    {
        return Some(Why::TakenDown { reason: b.reason.clone() });
    }
    match (known(templates, copy.as_deref()), &came_from) {
        (Known::Says(blocks), _) if blocks.is_empty() => None,
        (Known::Says(_), None) => Some(Why::Unchecked),
        (Known::Says(blocks), Some(_)) => named(&blocks),
        (Known::Unreadable, Some(from)) | (Known::Missing, Some(from)) if from.is_empty() => None,
        (Known::Unreadable, _) | (Known::Missing, Some(_)) => Some(Why::Unchecked),
        // (None written yet, and a record that can't be read: as it was.)
        (Known::Missing, None) => None,
    }
}

/// `blocked`, as a refusal of what was asked of it.
pub fn refuse(templates: &Path, id: &str) -> Result<(), String> {
    match blocked(templates, id) {
        Some(why) => {
            eprintln!("zeron-zgx: templates: {id} refused: {}", why.word());
            Err(why.word().into())
        }
        None => Ok(()),
    }
}

/// What you allowed of every template taken down, forgotten: not only of the
/// one that runs, or a restore would give the others Zeron's powers back with
/// nothing asked. (One that can't be checked keeps its: it comes back.)
pub fn forget_taken_down(templates: &Path, approvals: &Path) {
    for id in preview::allowed_at(approvals) {
        if let Some(Why::TakenDown { .. }) = blocked(templates, &id) {
            eprintln!("zeron-zgx: templates: {id} was taken down: what was allowed of it is forgotten");
            preview::forget_at(approvals, &id);
        }
    }
}

/// What runs (`running`; at a launch, what's chosen) can't be used: its id and
/// why, if it can't, whatever is chosen since. One taken down is let go of for good (what you allowed of it forgotten, Zeron's
/// own chosen by safe mode's path), so a restore brings nothing back by
/// itself; one that can't be checked stays chosen, and comes back once it is.
pub fn stop(templates: &Path, approvals: Option<&Path>, running: Option<&str>) -> Option<(String, Why)> {
    if let Some(approvals) = approvals {
        forget_taken_down(templates, approvals);
    }
    let id = running.map_or_else(|| live::chosen(templates), str::to_owned);
    let why = blocked(templates, &id)?;
    if let Why::TakenDown { .. } = why {
        eprintln!("zeron-zgx: templates: {id} was taken down: Zeron's own from here");
        if let Some(approvals) = approvals {
            preview::forget_at(approvals, &id);
        }
        preview::enter_safe();
        if live::chosen(templates) == id
            && let Err(error) = live::set_active(templates, live::BUILTIN) {
            eprintln!("zeron-zgx: Zeron's template wasn't chosen: {error}");
        }
    }
    Some((id, why))
}

/// `why`, for the Templates page's row.
pub fn row(why: &Why) -> Json {
    match why {
        Why::TakenDown { reason } => json!({ "reason": reason }),
        Why::Unchecked => json!({ "unchecked": true }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Rig {
        root: PathBuf,
        templates: PathBuf,
        copy: PathBuf,
    }

    const HASH: &str = "4fee345a38a1f2a65523a1d25ac01fd7cdde15f4524afbe1d7ce72933f47827e";

    impl Rig {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!("zgx-blocks-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            let (templates, copy) = (root.join("templates"), root.join("trust/blocked.json"));
            std::fs::create_dir_all(&templates).unwrap();
            copy_for_test(Some(copy.clone()));
            Rig { root, templates, copy }
        }

        fn template(&self, id: &str) {
            self.write(&format!("{id}/zeron-template.json"), &json!({ "id": id, "name": id, "version": "9.9.9" }).to_string());
            self.write(&format!("{id}/screens/x.luau"), "-- x");
        }

        fn store(&self, folder: &str, id: &str, version: &str) {
            self.template(folder);
            self.write(&format!("{folder}/{IMPORTED}"), &json!({ "key": null, "version": version, "source": { "kind": "store", "store": { "id": id, "hash": HASH } } }).to_string());
        }

        fn write(&self, rel: &str, text: &str) {
            let path = self.templates.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }

        fn blocks(&self, blocks: Json) {
            self.write(FILE, &json!({ "format": FORMAT, "blocks": blocks }).to_string());
        }

        fn midnight(&self) {
            self.blocks(json!([{ "id": "midnight", "version": "1.2.0", "reason": "It reads your keys.", "at": 5 }]));
        }

        fn why(&self, id: &str) -> Option<Why> {
            blocked(&self.templates, id)
        }

        fn down(&self, id: &str) -> bool {
            matches!(self.why(id), Some(Why::TakenDown { .. }))
        }
    }

    impl Drop for Rig {
        fn drop(&mut self) {
            copy_for_test(None);
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn a_template_is_known_by_its_record_and_what_it_was_copied_from() {
        let r = Rig::new("known");
        r.store("midnight", "midnight", "1.2.0");
        r.store("midnight-2", "midnight", "1.2.0+build.7");
        r.store("newer", "midnight", "1.2.1");
        r.store("other", "other", "1.2.0");
        // Its folder named for another, its manifest saying another version.
        r.template("copy");
        r.write("copy/.lineage.json", &json!([{ "id": "first", "version": "1.0.0", "hash": HASH }, { "id": "midnight", "version": "1.2.0", "hash": HASH }]).to_string());
        r.template("mine");
        r.template("from-a-file");
        r.write("from-a-file/.imported.json", r#"{"key":"k","version":"1.2.0","source":{"kind":"file"}}"#);
        r.blocks(json!([]));
        assert!(["midnight", "midnight-2", "copy"].iter().all(|id| r.why(id).is_none()), "nothing taken down yet");
        r.midnight();
        assert_eq!(r.why("midnight"), Some(Why::TakenDown { reason: Some("It reads your keys.".into()) }));
        assert!(r.down("midnight-2") && r.down("copy"));
        for id in ["newer", "other", "mine", "from-a-file", live::BUILTIN, "../midnight", "gone"] {
            assert_eq!(r.why(id), None, "{id}");
        }
        // A restore: its entry gone.
        r.blocks(json!([]));
        assert_eq!(r.why("midnight"), None);
    }

    #[test]
    fn the_last_list_read_stands_while_the_engine_s_can_t_be() {
        let r = Rig::new("copy");
        r.store("midnight", "midnight", "1.2.0");
        r.template("mine");
        r.midnight();
        assert!(r.down("midnight"));
        std::fs::remove_file(r.templates.join(FILE)).unwrap();
        assert!(r.down("midnight"), "deleting one lifts nothing");
        for bad in ["", "{", "[]", r#"{"format":"zeron-template-blocks:v2","blocks":[]}"#, r#"{"format":"zeron-template-blocks:v1"}"#, r#"{"format":"zeron-template-blocks:v1","blocks":[{"id":"x"}]}"#] {
            r.write(FILE, bad);
            assert!(r.down("midnight") && r.why("mine").is_none(), "{bad}");
        }
        // And is what a restore replaces.
        r.blocks(json!([]));
        assert_eq!(r.why("midnight"), None);
        std::fs::remove_file(r.templates.join(FILE)).unwrap();
        assert_eq!(r.why("midnight"), None, "the copy says nothing is");
    }

    #[test]
    fn with_no_list_to_be_had_only_the_store_s_are_refused() {
        let r = Rig::new("closed");
        r.store("midnight", "midnight", "1.2.0");
        r.template("copy");
        r.write("copy/.lineage.json", &json!([{ "id": "other", "version": "1.0.0", "hash": HASH }]).to_string());
        r.template("torn");
        r.write("torn/.imported.json", "{");
        r.template("mine");
        r.template("from-a-link");
        r.write("from-a-link/.imported.json", r#"{"key":"k","source":{"kind":"link","url":"https://x"}}"#);
        let closed = |why: &str| {
            for id in ["midnight", "copy", "torn"] {
                assert_eq!(r.why(id), Some(Why::Unchecked), "{why}: {id}");
            }
            for id in ["mine", "from-a-link", live::BUILTIN] {
                assert_eq!(r.why(id), None, "{why}: {id}");
            }
        };
        // None there, and none kept: the store's are refused until there is;
        // a file's whose record can't be read is as it was.
        for id in ["midnight", "copy"] {
            assert_eq!(r.why(id), Some(Why::Unchecked), "{id}");
        }
        for id in ["torn", "mine", "from-a-link"] {
            assert_eq!(r.why(id), None, "{id}");
        }
        r.write(FILE, "{");
        closed("the engine's unreadable, no copy");
        std::fs::create_dir_all(r.copy.parent().unwrap()).unwrap();
        std::fs::write(&r.copy, "{").unwrap();
        closed("both unreadable");
        std::fs::remove_file(r.templates.join(FILE)).unwrap();
        closed("the engine's gone, the copy unreadable");
        // Read again: what it says, and the copy written again.
        r.blocks(json!([]));
        assert_eq!(r.why("midnight"), None);
        assert!(matches!(list(&r.copy), Read::Says(_)));
    }

    #[test]
    fn a_record_that_can_t_be_read_is_refused_while_anything_is_taken_down() {
        let r = Rig::new("torn");
        r.template("torn");
        r.template("mine");
        r.midnight();
        for (file, bad) in [(IMPORTED, "{"), (IMPORTED, "[]"), (IMPORTED, r#"{"source":{"kind":"store"}}"#), (IMPORTED, r#"{"source":{"store":{}}}"#), (IMPORTED, r#"{"source":"store"}"#), (IMPORTED, r#"{"source":{"kind":5}}"#), (IMPORTED, r#"{"version":"1.0.0","source":{"kind":"store","store":{}}}"#), (LINEAGE, "{"), (LINEAGE, "{}"), (LINEAGE, r#"[{"id":"x"}]"#)] {
            r.write(&format!("torn/{file}"), bad);
            assert_eq!(r.why("torn"), Some(Why::Unchecked), "{file} {bad}");
            std::fs::remove_file(r.templates.join("torn").join(file)).unwrap();
            assert_eq!(r.why("torn"), None);
        }
        // One that isn't a store id, a version and a content hash is no record.
        let long = "9".repeat(129);
        for (id, version, hash) in [("Midnight", "1.0.0", HASH), ("-x", "1.0.0", HASH), ("x", "1.0.0", HASH), ("midnight ", "1.0.0", HASH), ("dusk", "", HASH), ("dusk", &long, HASH), ("dusk", "1.0.0", ""), ("dusk", "1.0.0", &HASH.to_uppercase()), ("dusk", "1.0.0", &HASH[1..])] {
            let entry = json!({ "id": id, "version": version, "hash": hash });
            r.write(&format!("torn/{LINEAGE}"), &json!([entry]).to_string());
            assert_eq!(r.why("torn"), Some(Why::Unchecked), "{entry}");
            std::fs::remove_file(r.templates.join("torn").join(LINEAGE)).unwrap();
            r.write(&format!("torn/{IMPORTED}"), &json!({ "version": version, "source": { "kind": "store", "store": { "id": id, "hash": hash } } }).to_string());
            assert_eq!(r.why("torn"), Some(Why::Unchecked), "{entry}");
            std::fs::remove_file(r.templates.join("torn").join(IMPORTED)).unwrap();
        }
        r.store("torn", "dusk", "1.0.0");
        assert_eq!(r.why("torn"), None);
        std::fs::remove_file(r.templates.join("torn").join(IMPORTED)).unwrap();
        // Not a plain file.
        std::fs::create_dir(r.templates.join("torn").join(IMPORTED)).unwrap();
        assert_eq!(r.why("torn"), Some(Why::Unchecked));
        assert_eq!(r.why("mine"), None);
    }

    /// Taken down while something else runs: what was allowed of it (and of a
    /// copy of it) is forgotten too, so a restore gives nothing back unasked.
    #[test]
    fn what_was_allowed_of_every_one_taken_down_is_forgotten() {
        let r = Rig::new("forget");
        let approvals = r.root.join("trust/approvals.json");
        r.store("midnight", "midnight", "1.2.0");
        r.store("dusk", "dusk", "1.0.0");
        r.template("copy");
        r.write(&format!("copy/{LINEAGE}"), &json!([{ "id": "midnight", "version": "1.2.0", "hash": HASH }]).to_string());
        r.template("mine");
        r.blocks(json!([]));
        let all = ["midnight", "dusk", "copy", "mine"];
        let digests: Vec<String> = all.iter().map(|id| preview::digest(&r.templates.join(id))).collect();
        let allowed = || -> Vec<bool> { all.iter().zip(&digests).map(|(id, d)| preview::approved_at(&approvals, id, d)).collect() };
        for (id, digest) in all.iter().zip(&digests) {
            preview::approve_at(&approvals, id, digest).unwrap();
        }
        assert_eq!(stop(&r.templates, Some(&approvals), Some("mine")), None);
        assert_eq!(allowed(), [true; 4]);
        // Not to be checked: kept.
        std::fs::remove_file(&r.copy).unwrap();
        r.write(FILE, "{");
        assert_eq!(stop(&r.templates, Some(&approvals), Some("mine")), None);
        assert_eq!(allowed(), [true; 4]);
        r.midnight();
        assert_eq!(stop(&r.templates, Some(&approvals), Some("mine")), None);
        assert_eq!(allowed(), [false, true, false, true]);
        r.blocks(json!([]));
        assert_eq!((r.why("midnight"), allowed()), (None, vec![false, true, false, true]));
    }

    /// One taken down never runs, and is let go of: what you allowed of it is
    /// forgotten and Zeron's own chosen, so a restore brings nothing back. One
    /// that can't be checked stays chosen, and comes back once it can be.
    #[test]
    fn a_template_taken_down_is_let_go_of_and_one_unchecked_comes_back() {
        let r = Rig::new("stop");
        let approvals = r.root.join("trust/approvals.json");
        preview::enter_safe_for_test(false);
        r.store("midnight", "midnight", "1.2.0");
        r.blocks(json!([]));
        let digest = preview::digest(&r.templates.join("midnight"));
        preview::approve_at(&approvals, "midnight", &digest).unwrap();
        live::set_active(&r.templates, "midnight").unwrap();
        assert_eq!((live::active(&r.templates), stop(&r.templates, Some(&approvals), None)), ("midnight".into(), None));
        std::fs::remove_file(&r.copy).unwrap();
        r.write(FILE, "{");
        assert_eq!(live::active(&r.templates), live::BUILTIN, "refused before anything is laid down");
        assert_eq!(stop(&r.templates, Some(&approvals), None), Some(("midnight".into(), Why::Unchecked)));
        assert!(preview::approved_at(&approvals, "midnight", &digest));
        r.blocks(json!([]));
        assert_eq!(live::active(&r.templates), "midnight", "back, by itself");
        r.midnight();
        assert_eq!(live::active(&r.templates), live::BUILTIN);
        assert_eq!(refuse(&r.templates, "midnight"), Err("template_taken_down".into()));
        assert!(matches!(stop(&r.templates, Some(&approvals), None), Some((id, Why::TakenDown { .. })) if id == "midnight"));
        assert!(!preview::approved_at(&approvals, "midnight", &digest), "what was allowed is forgotten");
        assert_eq!(live::chosen(&r.templates), live::BUILTIN);
        assert_eq!(stop(&r.templates, Some(&approvals), None), None, "once");
        r.blocks(json!([]));
        preview::enter_safe_for_test(false);
        assert_eq!(live::active(&r.templates), live::BUILTIN, "a restore brings nothing back");
        // Another switched it away first: what ran is what's told, and let go
        // of; what's chosen since isn't touched.
        preview::approve_at(&approvals, "midnight", &digest).unwrap();
        r.template("mine");
        live::set_active(&r.templates, "mine").unwrap();
        r.midnight();
        assert!(matches!(stop(&r.templates, Some(&approvals), Some("midnight")), Some((id, Why::TakenDown { .. })) if id == "midnight"));
        assert!(!preview::approved_at(&approvals, "midnight", &digest));
        assert_eq!(live::chosen(&r.templates), "mine");
        assert_eq!(stop(&r.templates, Some(&approvals), Some("mine")), None);
    }

    /// The engine's word on a row is a block at once and at the next launch
    /// with no engine, its file not naming it (it couldn't be written); let go
    /// of only by an answer without it and a file that reads without it.
    #[test]
    fn the_engine_s_word_on_a_row_is_kept_until_both_it_and_the_list_drop_it() {
        let r = Rig::new("rows");
        r.store("midnight-2", "midnight", "1.2.0");
        r.template("copy");
        r.write("copy/.lineage.json", &json!([{ "id": "midnight", "version": "1.2.0", "hash": HASH }]).to_string());
        r.store("other", "other", "1.0.0");
        r.blocks(json!([]));
        let row = |blocked: Json| json!({ "templates": [{ "id": "midnight-2", "blocked": blocked }, { "id": "other" }] });
        let down = row(json!({ "id": "midnight", "version": "1.2.0", "reason": "It reads your keys.", "at": 1 }));
        assert!(!take_rows(&r.templates, &row(Json::Null)), "nothing said");
        assert!(take_rows(&r.templates, &down));
        assert!(!take_rows(&r.templates, &down), "the same again");
        for id in ["midnight-2", "copy"] {
            assert_eq!(r.why(id), Some(Why::TakenDown { reason: Some("It reads your keys.".into()) }), "{id}");
        }
        assert_eq!(r.why("other"), None);
        // A launch with no engine, its file gone or unreadable: as said.
        for file in [None, Some("{")] {
            match file {
                Some(bad) => r.write(FILE, bad),
                None => std::fs::remove_file(r.templates.join(FILE)).unwrap(),
            }
            assert!(r.down("midnight-2"));
            // An answer without it lets nothing go while the file can't be read.
            assert!(!take_rows(&r.templates, &row(Json::Null)));
            assert!(r.down("midnight-2"));
        }
        // Nor one while the list names it, nor an answer without the row.
        r.midnight();
        assert!(!take_rows(&r.templates, &row(Json::Null)));
        r.blocks(json!([]));
        assert!(!take_rows(&r.templates, &json!({ "templates": [{ "id": "other" }] })));
        assert!(!take_rows(&r.templates, &Json::Null) && !take_rows(&r.templates, &json!({ "error": "offline" })));
        assert!(r.down("midnight-2"));
        // Both without it: let go of.
        assert!(take_rows(&r.templates, &row(Json::Null)));
        assert_eq!((r.why("midnight-2"), r.why("copy")), (None, None));
        // Its own record torn since: still the folder it said it of.
        take_rows(&r.templates, &down);
        r.write("midnight-2/.imported.json", "{");
        assert!(r.down("midnight-2"));
        // Its folder gone (deleted, and restored at the store): the same
        // version installed again, in another, isn't blocked for good.
        std::fs::remove_dir_all(r.templates.join("midnight-2")).unwrap();
        r.store("midnight-3", "midnight", "1.2.0");
        let gone = json!({ "templates": [{ "id": "midnight-3" }] });
        assert!(r.down("midnight-3"), "until a list says so");
        r.midnight();
        assert!(!take_rows(&r.templates, &gone), "the list still names it");
        r.write(FILE, "{");
        assert!(!take_rows(&r.templates, &gone), "nor while it can't be read");
        r.blocks(json!([]));
        assert!(take_rows(&r.templates, &gone));
        assert_eq!((r.why("midnight-3"), r.why("copy")), (None, None));
    }
}
