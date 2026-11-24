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
//! What can't be read is refused, so far as it came from the store: with no
//! list to be had, every template with a store record or a lineage; with
//! one, a template whose own record can't be read. It comes back by itself
//! once Zeron can check. (A template you kept runs as Zeron and can undo all
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

/// What's known to be taken down: the engine's list (kept as it's read), else
/// the last one read. None: there's one and neither can be read.
fn known(templates: &Path, copy: Option<&Path>) -> Option<Vec<Block>> {
    let theirs = list(&templates.join(FILE));
    if let Read::Says((written, blocks)) = theirs {
        if let Some(copy) = copy
            && let Err(error) = live::put(copy, &serde_json::to_vec_pretty(&written).unwrap_or_default())
        {
            eprintln!("zeron-zgx: templates: the taken-down list wasn't kept: {error}");
        }
        return Some(blocks);
    }
    match copy.map_or(Read::Missing, list) {
        Read::Says((_, blocks)) => Some(blocks),
        // (Neither has ever been: nothing has been taken down here.)
        Read::Missing if matches!(theirs, Read::Missing) => Some(Vec::new()),
        _ => None,
    }
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
            if v["source"]["kind"] == "store" {
                out.push((words(&v["source"]["store"]["id"])?, words(&v["version"])?));
            }
        }
    }
    match read(&dir.join(LINEAGE)) {
        Read::Missing => {}
        Read::Bad => return None,
        Read::Says(v) => {
            for entry in v.as_array()? {
                out.push((words(&entry["id"])?, words(&entry["version"])?));
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
    let known = known(templates, copy_path().as_deref());
    let came_from = came_from(&dir);
    match (known, came_from) {
        (Some(blocks), _) if blocks.is_empty() => None,
        (Some(_), None) => Some(Why::Unchecked),
        (Some(blocks), Some(from)) => blocks
            .into_iter()
            .find(|b| from.iter().any(|(id, version)| *id == b.id && same_version(version, &b.version)))
            .map(|b| Why::TakenDown { reason: b.reason }),
        (None, Some(from)) if from.is_empty() => None,
        (None, _) => Some(Why::Unchecked),
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

/// What was chosen can't be used: `(id, its name, why)`, if it can't. One
/// taken down is let go of for good (what you allowed of it forgotten, Zeron's
/// own chosen by safe mode's path), so a restore brings nothing back by
/// itself; one that can't be checked stays chosen, and comes back once it is.
pub fn stop(templates: &Path, approvals: Option<&Path>) -> Option<(String, Why)> {
    let id = live::chosen(templates);
    let why = blocked(templates, &id)?;
    if let Why::TakenDown { .. } = why {
        eprintln!("zeron-zgx: templates: {id} was taken down: Zeron's own from here");
        if let Some(approvals) = approvals {
            preview::forget_at(approvals, &id);
        }
        preview::enter_safe();
        if let Err(error) = live::set_active(templates, live::BUILTIN) {
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
            self.write(&format!("{folder}/{IMPORTED}"), &json!({ "key": null, "version": version, "source": { "kind": "store", "store": { "id": id } } }).to_string());
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
        r.write("copy/.lineage.json", r#"[{"id":"first","version":"1.0.0","hash":"h"},{"id":"midnight","version":"1.2.0","hash":"h"}]"#);
        r.template("mine");
        r.template("from-a-file");
        r.write("from-a-file/.imported.json", r#"{"key":"k","version":"1.2.0","source":{"kind":"file"}}"#);
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
        assert_eq!(r.why("midnight"), None);
    }

    #[test]
    fn with_no_list_to_be_had_only_the_store_s_are_refused() {
        let r = Rig::new("closed");
        r.store("midnight", "midnight", "1.2.0");
        r.template("copy");
        r.write("copy/.lineage.json", r#"[{"id":"other","version":"1.0.0"}]"#);
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
        // Neither has ever been: nothing is refused, not even an odd record.
        for id in ["midnight", "copy", "torn", "mine"] {
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
        for (file, bad) in [(IMPORTED, "{"), (IMPORTED, "[]"), (IMPORTED, r#"{"source":{"kind":"store"}}"#), (IMPORTED, r#"{"version":"1.0.0","source":{"kind":"store","store":{}}}"#), (LINEAGE, "{"), (LINEAGE, "{}"), (LINEAGE, r#"[{"id":"x"}]"#)] {
            r.write(&format!("torn/{file}"), bad);
            assert_eq!(r.why("torn"), Some(Why::Unchecked), "{file} {bad}");
            std::fs::remove_file(r.templates.join("torn").join(file)).unwrap();
            assert_eq!(r.why("torn"), None);
        }
        // Not a plain file.
        std::fs::create_dir(r.templates.join("torn").join(IMPORTED)).unwrap();
        assert_eq!(r.why("torn"), Some(Why::Unchecked));
        assert_eq!(r.why("mine"), None);
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
        let digest = preview::digest(&r.templates.join("midnight"));
        preview::approve_at(&approvals, "midnight", &digest).unwrap();
        live::set_active(&r.templates, "midnight").unwrap();
        assert_eq!((live::active(&r.templates), stop(&r.templates, Some(&approvals))), ("midnight".into(), None));
        r.write(FILE, "{");
        assert_eq!(live::active(&r.templates), live::BUILTIN, "refused before anything is laid down");
        assert_eq!(stop(&r.templates, Some(&approvals)), Some(("midnight".into(), Why::Unchecked)));
        assert!(preview::approved_at(&approvals, "midnight", &digest));
        r.blocks(json!([]));
        assert_eq!(live::active(&r.templates), "midnight", "back, by itself");
        r.midnight();
        assert_eq!(live::active(&r.templates), live::BUILTIN);
        assert_eq!(refuse(&r.templates, "midnight"), Err("template_taken_down".into()));
        assert!(matches!(stop(&r.templates, Some(&approvals)), Some((id, Why::TakenDown { .. })) if id == "midnight"));
        assert!(!preview::approved_at(&approvals, "midnight", &digest), "what was allowed is forgotten");
        assert_eq!(live::chosen(&r.templates), live::BUILTIN);
        assert_eq!(stop(&r.templates, Some(&approvals)), None, "once");
        r.blocks(json!([]));
        preview::enter_safe_for_test(false);
        assert_eq!(live::active(&r.templates), live::BUILTIN, "a restore brings nothing back");
    }
}
