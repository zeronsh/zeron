//! Templates the store took down (contract "Templates: after the trust
//! review"): what this device keeps of it, and what's refused for it.
//!
//! `<templates>/.blocked.json` is written by the engine alone, whole
//! (`{format, blocks: [{id, version, hash?, reason, at}]}`, `id` the store's),
//! and read by the app too, before it lays any template's code down (a launch
//! with no engine among them). It fails closed: a file that's there and can't
//! be read, or an entry of it, blocks every template that came from the store
//! or from one that did.
//!
//! A template is known by what its record says (`.imported.json`: the store's
//! id and the version), never by its folder's name, and by its lineage
//! (`.lineage.json`: the store versions it was copied from, copies of copies
//! too). What it holds now doesn't matter: an edit lifts nothing.
//!
//! One that couldn't be written is held to while this engine runs, and said
//! of its row; the app's own record beside what you allowed, kept once a row
//! said so, covers a launch before it's written.
//!
//! An entry is kept while something here is it, and 30 days besides: so long
//! a file or a link with that package (its content `hash`) isn't imported.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::install::{STORE, Source};
use super::templates::{self, read_regular, write_atomic};
use super::version::Version;

pub const FILE: &str = ".blocked.json";
pub const FORMAT: &str = "zeron-template-blocks:v1";
pub const LINEAGE: &str = ".lineage.json";
/// How long an entry nothing here is is kept (the feed's own length).
const KEEP_SECS: i64 = 30 * 24 * 60 * 60;
/// The word a blocked template's refusal starts with.
pub const TAKEN_DOWN: &str = "taken_down";

/// A template version the store took down.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Block {
    pub id: String,
    pub version: String,
    /// Its content hash, when the store says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    pub reason: String,
    /// When this device learned of it (seconds).
    pub at: i64,
}

#[derive(Serialize, Deserialize)]
struct Blocks {
    format: String,
    blocks: Vec<Block>,
}

/// A store version a template is, or was copied from.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Identity {
    pub id: String,
    pub version: String,
    pub hash: String,
}

/// The longest version the store is asked about.
const MAX_VERSION: usize = 128;

impl Identity {
    /// Whether the store can be asked about it: a store id, a version, and a
    /// content hash. Anything else isn't what a store template is (a
    /// `.lineage.json` is a file anyone may write), and is never sent: the
    /// store refuses the whole request for one.
    fn valid(&self) -> bool {
        templates::valid_id(&self.id)
            && (1..=MAX_VERSION).contains(&self.version.len())
            && self.hash.len() == 64
            && self.hash.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }
}

/// Couldn't be read: whoever asks fails closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unreadable;

/// Held over each change (a change reads the whole file before writing it).
fn changing() -> MutexGuard<'static, ()> {
    static CHANGING: Mutex<()> = Mutex::new(());
    CHANGING.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Whether `a` and `b` are one version: by precedence when both are strict,
/// else as written.
fn same_version(a: &str, b: &str) -> bool {
    match (Version::parse(a), Version::parse(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

impl Block {
    fn is(&self, identity: &Identity) -> bool {
        self.id == identity.id && same_version(&self.version, &identity.version)
    }
}

/// Whether the file isn't there at all. Written at the first full check that
/// is kept (`[]` when nothing is down), so from then on it being gone means
/// it was taken away: the list starts over, never from the next event alone.
pub fn missing(dir: &Path) -> bool {
    std::fs::symlink_metadata(dir.join(FILE)).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
}

/// Take-downs that couldn't be written (a full disk, a folder made
/// read-only), by templates folder: held to while this engine runs, and
/// written with the next that can be. The feed's place waits for them.
fn unwritten() -> MutexGuard<'static, BTreeMap<PathBuf, Vec<Block>>> {
    static UNWRITTEN: Mutex<BTreeMap<PathBuf, Vec<Block>>> = Mutex::new(BTreeMap::new());
    UNWRITTEN.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `blocks`, and those that couldn't be written of `dir`'s.
fn with_unwritten(dir: &Path, mut blocks: Vec<Block>) -> Vec<Block> {
    for block in unwritten().get(dir).into_iter().flatten() {
        if !blocks.iter().any(|b| b.id == block.id && same_version(&b.version, &block.version)) {
            blocks.push(block.clone());
        }
    }
    blocks
}

/// What's blocked here; none when there's never been a take-down.
pub fn read(dir: &Path) -> Result<Vec<Block>, Unreadable> {
    read_file(dir).map(|blocks| with_unwritten(dir, blocks))
}

fn read_file(dir: &Path) -> Result<Vec<Block>, Unreadable> {
    let path = dir.join(FILE);
    if std::fs::symlink_metadata(&path).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) {
        return Ok(Vec::new());
    }
    let bytes = read_regular(&path).ok_or(Unreadable)?;
    let read: Blocks = serde_json::from_slice(&bytes).map_err(|_| Unreadable)?;
    if read.format == FORMAT { Ok(read.blocks) } else { Err(Unreadable) }
}

fn write(dir: &Path, blocks: Vec<Block>) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(&json!({ "format": FORMAT, "blocks": blocks })).map_err(|e| e.to_string())?;
    write_atomic(dir, FILE, &bytes)?;
    unwritten().remove(dir);
    Ok(())
}

/// A dot file of a template's folder as JSON: `Ok(None)` when it isn't there.
fn dot_file<T: serde::de::DeserializeOwned>(folder: &Path, name: &str) -> Result<Option<T>, Unreadable> {
    let path = folder.join(name);
    if std::fs::symlink_metadata(&path).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) {
        return Ok(None);
    }
    let bytes = read_regular(&path).ok_or(Unreadable)?;
    serde_json::from_slice(&bytes).map(Some).map_err(|_| Unreadable)
}

/// Every store version the template in `folder` is or was copied from: its
/// record's, and its lineage's.
pub fn identities(folder: &Path) -> Result<Vec<Identity>, Unreadable> {
    let mut out: Vec<Identity> = dot_file(folder, LINEAGE)?.unwrap_or_default();
    if let Some(record) = dot_file::<serde_json::Value>(folder, super::template_packages::IMPORTED)? {
        out.extend(identities_of(&record)?);
    }
    if !out.iter().all(Identity::valid) {
        return Err(Unreadable);
    }
    out.sort();
    out.dedup();
    Ok(out)
}

/// What a record (`.imported.json`) says the template is.
fn identities_of(record: &serde_json::Value) -> Result<Vec<Identity>, Unreadable> {
    if !record.is_object() {
        return Err(Unreadable);
    }
    if record["source"].is_null() {
        return Ok(Vec::new());
    }
    let source: Source = serde_json::from_value(record["source"].clone()).map_err(|_| Unreadable)?;
    if source.kind != STORE {
        return Ok(Vec::new());
    }
    let store = source.store().ok_or(Unreadable)?;
    let version = record["version"].as_str().ok_or(Unreadable)?;
    Ok(vec![Identity { id: store.id.clone(), version: version.to_owned(), hash: store.hash.clone() }])
}

/// Keep `identities` as the lineage of the template in `folder`.
pub fn keep_lineage(folder: &Path, identities: &[Identity]) -> Result<(), String> {
    if identities.is_empty() {
        return Ok(());
    }
    let bytes = serde_json::to_vec_pretty(identities).map_err(|e| e.to_string())?;
    write_atomic(folder, LINEAGE, &bytes)
}

/// What blocks the template in `folder` of `dir`, if anything does.
pub fn blocked(dir: &Path, folder: &Path) -> Option<Block> {
    let unreadable = |what: &str| Block { id: String::new(), version: String::new(), hash: None, reason: format!("Zeron can't read {what}"), at: 0 };
    match (read(dir), identities(folder)) {
        (Ok(blocks), Ok(identities)) => blocks.into_iter().find(|block| identities.iter().any(|i| block.is(i))),
        (Ok(blocks), Err(_)) => (!blocks.is_empty()).then(|| unreadable("where it came from, and a template has been taken down from the store")),
        (Err(_), Ok(identities)) => (!identities.is_empty()).then(|| unreadable("which templates have been taken down from the store")),
        (Err(_), Err(_)) => Some(unreadable("which templates have been taken down from the store")),
    }
}

/// Whether `block` holds the template in `folder` by its lineage alone, not
/// its own record.
pub fn by_lineage(folder: &Path, block: &Block) -> bool {
    let own: Result<Vec<Identity>, Unreadable> = match dot_file::<serde_json::Value>(folder, super::template_packages::IMPORTED) {
        Ok(Some(record)) if record["source"]["kind"] == STORE => identities_of(&record),
        _ => Ok(Vec::new()),
    };
    own.is_ok_and(|own| !own.iter().any(|i| block.is(i))) && dot_file::<Vec<Identity>>(folder, LINEAGE).is_ok_and(|l| l.unwrap_or_default().iter().any(|i| block.is(i)))
}

/// Refuses `what` of the template in `folder` when it's blocked.
pub fn refuse(dir: &Path, folder: &Path, what: &str) -> Result<(), String> {
    let Some(block) = blocked(dir, folder) else { return Ok(()) };
    let why = if block.reason.is_empty() { String::new() } else { format!(" ({})", block.reason) };
    Err(super::install::failure(TAKEN_DOWN, format!("This template was taken down from the store{why}, so it can't be {what}.")))
}

/// Refuses a package (its content hash, and the store's id and version when
/// it's from there) that was taken down.
pub fn refuse_package(dir: &Path, content_hash: &str, identity: Option<&Identity>) -> Result<(), String> {
    // With nothing here for it to block, one that can't be read is started
    // again: else nothing would ever be imported.
    if read(dir).is_err() && installed(dir).is_empty() {
        let _ = settle(dir, &[], Vec::new(), 0);
    }
    let blocks = read(dir).map_err(|_| super::install::failure(TAKEN_DOWN, "Zeron can't read which templates have been taken down from the store, so it isn't installing any"))?;
    let hit = blocks.iter().find(|b| b.hash.as_deref().is_some_and(|h| h.eq_ignore_ascii_case(content_hash)) || identity.is_some_and(|i| b.is(i)));
    match hit {
        None => Ok(()),
        Some(block) => {
            let why = if block.reason.is_empty() { String::new() } else { format!(" ({})", block.reason) };
            Err(super::install::failure(TAKEN_DOWN, format!("This template was taken down from the store{why}, so it wasn't installed.")))
        }
    }
}

/// Each installed template's folder name and what it is, for those that came
/// from the store or were copied from one that did. `Err` for one that can't
/// be read.
pub fn installed(dir: &Path) -> Vec<(String, Result<Vec<Identity>, Unreadable>)> {
    templates::installed(dir)
        .into_iter()
        .map(|id| {
            let found = identities(&dir.join(&id));
            (id, found)
        })
        .filter(|(_, found)| found.as_ref().is_ok_and(|i| !i.is_empty()) || found.is_err())
        .collect()
}

/// `all` as the requests that ask about them: an id once in each (a template
/// and a copy of an earlier version of it are two versions of one id), at
/// most `most` in one.
pub fn requests(all: &[Identity], most: usize) -> Vec<Vec<Identity>> {
    let mut rounds: Vec<Vec<Identity>> = Vec::new();
    for identity in all {
        match rounds.iter_mut().find(|r| r.len() < most && !r.iter().any(|i| i.id == identity.id)) {
            Some(round) => round.push(identity.clone()),
            None => rounds.push(vec![identity.clone()]),
        }
    }
    rounds
}

/// Every store version anything here is, once each.
pub fn all_identities(dir: &Path) -> Vec<Identity> {
    let mut all: Vec<Identity> = installed(dir).into_iter().filter_map(|(_, found)| found.ok()).flatten().collect();
    all.sort();
    all.dedup();
    all
}

/// A change to what's blocked.
#[derive(Debug, Clone)]
pub enum Change {
    TakenDown(Block),
    /// (id, version)
    Restored(String, String),
}

/// `changes` made in order, entries past keeping dropped: `Err` when it
/// couldn't be read or written. What was taken down is then held to all the
/// same while this engine runs; what was restored stays blocked.
pub fn apply(dir: &Path, changes: Vec<Change>, now: i64) -> Result<(), String> {
    let _held = changing();
    let downs: Vec<Block> = changes.iter().filter_map(|c| if let Change::TakenDown(block) = c { Some(block.clone()) } else { None }).collect();
    apply_held(dir, changes, now).inspect_err(|_| hold_to(dir, downs))
}

/// `blocks` held to while this engine runs: they couldn't be written.
fn hold_to(dir: &Path, blocks: Vec<Block>) {
    let mut unwritten = unwritten();
    let kept = unwritten.entry(dir.to_owned()).or_default();
    for block in blocks {
        kept.retain(|b| !(b.id == block.id && same_version(&b.version, &block.version)));
        kept.push(block);
    }
}

fn apply_held(dir: &Path, changes: Vec<Change>, now: i64) -> Result<(), String> {
    let before = read_file(dir).map_err(|_| "What's been taken down can't be read".to_string())?;
    let mut blocks = with_unwritten(dir, before.clone());
    for change in changes {
        match change {
            Change::TakenDown(block) => {
                blocks.retain(|b| !(b.id == block.id && same_version(&b.version, &block.version)));
                blocks.push(block);
            }
            Change::Restored(id, version) => blocks.retain(|b| !(b.id == id && same_version(&b.version, &version))),
        }
    }
    let kept = kept(dir, blocks, now);
    if kept == before { Ok(()) } else { write(dir, kept) }
}

/// What the full check said: `asked` (everything it was asked about and
/// answered), of which `blocks` are down. Those asked about are exactly as it
/// says; any other entry stays until it's past keeping. Whatever couldn't be
/// read before is replaced. (Nothing of the feed's is applied between the
/// question and this: `Plugins::template_checks`.)
pub fn settle(dir: &Path, asked: &[Identity], blocks: Vec<Block>, now: i64) -> Result<(), String> {
    let _held = changing();
    let before = read_file(dir);
    let mut all: Vec<Block> = with_unwritten(dir, before.clone().unwrap_or_default());
    // (One already known keeps when it was learned of, and its hash.)
    let blocks = blocks.into_iter().map(|b| match all.iter().find(|k| k.id == b.id && same_version(&k.version, &b.version)) {
        Some(known) => Block { at: known.at, hash: b.hash.or_else(|| known.hash.clone()), ..b },
        None => b,
    });
    let blocks: Vec<Block> = blocks.collect();
    all.retain(|b| !asked.iter().any(|i| b.is(i)));
    all.extend(blocks.iter().cloned());
    let kept = kept(dir, all, now);
    if before == Ok(kept.clone()) && !missing(dir) { Ok(()) } else { write(dir, kept).inspect_err(|_| hold_to(dir, blocks)) }
}

/// `blocks` without those past keeping: 30 days old, and nothing here is one.
fn kept(dir: &Path, mut blocks: Vec<Block>, now: i64) -> Vec<Block> {
    if blocks.iter().all(|b| now - b.at <= KEEP_SECS) {
        return blocks;
    }
    let found = installed(dir);
    // (Something that can't be read may be any of them.)
    if found.iter().any(|(_, f)| f.is_err()) {
        return blocks;
    }
    let here: Vec<Identity> = found.into_iter().filter_map(|(_, f)| f.ok()).flatten().collect();
    blocks.retain(|b| now - b.at <= KEEP_SECS || here.iter().any(|i| b.is(i)));
    blocks
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000;

    fn block(id: &str, version: &str, at: i64) -> Block {
        Block { id: id.into(), version: version.into(), hash: Some("ab".repeat(32)), reason: "It read your chats.".into(), at }
    }

    fn template(dir: &Path, folder: &str, record: Option<serde_json::Value>) -> std::path::PathBuf {
        let at = dir.join(folder);
        std::fs::create_dir_all(&at).unwrap();
        std::fs::write(at.join("zeron-template.json"), json!({ "id": folder, "name": folder }).to_string()).unwrap();
        if let Some(record) = record {
            std::fs::write(at.join(".imported.json"), record.to_string()).unwrap();
        }
        at
    }

    /// Store id `id`'s record, with no signer.
    fn record(id: &str, version: &str) -> serde_json::Value {
        json!({ "key": null, "version": version, "source": { "kind": "store", "version": version, "installed_at": 0, "store": { "id": id, "publisher": "ada", "hash": "cd".repeat(32) } } })
    }

    /// By the record's store id and version, never the folder's name; and by
    /// what it was copied from, copies of copies too.
    #[test]
    fn a_block_holds_by_the_record_and_the_lineage() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let store = template(dir, "midnight-2", Some(record("midnight", "1.0.0+build")));
        let named_so = template(dir, "midnight", None);
        let from_file = template(dir, "filed", Some(json!({ "key": "k", "version": "1.0.0", "source": { "kind": "file", "version": "1.0.0", "installed_at": 0 } })));
        let copy = template(dir, "copy", None);
        keep_lineage(&copy, &identities(&store).unwrap()).unwrap();
        let second = template(dir, "second", None);
        keep_lineage(&second, &identities(&copy).unwrap()).unwrap();
        let other = template(dir, "other", Some(record("midnight", "1.1.0")));
        for t in [&store, &named_so, &from_file, &copy, &second, &other] {
            assert_eq!(blocked(dir, t), None, "never had a take-down");
        }
        apply(dir, vec![Change::TakenDown(block("midnight", "1.0.0", NOW))], NOW).unwrap();
        for t in [&store, &copy, &second] {
            assert_eq!(blocked(dir, t).unwrap().reason, "It read your chats.", "{t:?}");
            assert!(refuse(dir, t, "used").unwrap_err().starts_with("taken_down: "));
        }
        for t in [&named_so, &from_file, &other] {
            assert_eq!(blocked(dir, t), None, "{t:?}");
        }
        // A copy edited through and through is still one.
        std::fs::write(second.join("app.luau"), "-- all mine").unwrap();
        assert!(blocked(dir, &second).is_some());
        apply(dir, vec![Change::Restored("midnight".into(), "1.0.0".into())], NOW).unwrap();
        assert_eq!(blocked(dir, &store), None);
    }

    #[test]
    fn what_cannot_be_read_blocks() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let store = template(dir, "aa", Some(record("aa", "1.0.0")));
        let mine = template(dir, "mine", None);
        let broken = template(dir, "broken", None);
        std::fs::write(broken.join(".imported.json"), "nope").unwrap();
        let lineage = template(dir, "lineage", None);
        std::fs::write(lineage.join(LINEAGE), "{}").unwrap();
        assert_eq!((blocked(dir, &store), blocked(dir, &broken), blocked(dir, &lineage)), (None, None, None), "no take-down ever");
        // One listed, of something else: only what can't be told is blocked.
        apply(dir, vec![Change::TakenDown(block("z", "1.0.0", NOW))], NOW).unwrap();
        assert!(blocked(dir, &store).is_none() && blocked(dir, &mine).is_none());
        assert!(blocked(dir, &broken).is_some() && blocked(dir, &lineage).is_some());
        let entry = serde_json::to_value(block("z", "1.0.0", NOW)).unwrap();
        for text in ["", "nope", "[]", r#"{"format":"v2","blocks":[]}"#, &json!({ "format": FORMAT, "blocks": [entry, { "id": "y" }] }).to_string()] {
            std::fs::write(dir.join(FILE), text).unwrap();
            assert!(blocked(dir, &store).is_some() && blocked(dir, &broken).is_some(), "{text}");
            assert!(blocked(dir, &mine).is_none(), "what never came from the store still runs: {text}");
            assert!(refuse_package(dir, "00", None).is_err(), "{text}");
            assert!(apply(dir, Vec::new(), NOW).is_err(), "{text}");
        }
        // A folder where the file should be.
        std::fs::remove_file(dir.join(FILE)).unwrap();
        std::fs::create_dir(dir.join(FILE)).unwrap();
        assert!(blocked(dir, &store).is_some());
        std::fs::remove_dir(dir.join(FILE)).unwrap();
        // The full check's word replaces what couldn't be read.
        std::fs::write(dir.join(FILE), "nope").unwrap();
        settle(dir, &all_identities(dir), Vec::new(), NOW).unwrap();
        assert_eq!((read(dir), blocked(dir, &store)), (Ok(Vec::new()), None));
    }

    /// The store refuses a request that names an id twice, or anything that
    /// isn't a template's: never sent so.
    #[test]
    fn what_is_asked_is_what_the_store_takes() {
        let identity = |id: &str, version: &str| Identity { id: id.into(), version: version.into(), hash: "ab".repeat(32) };
        let all = [identity("aa", "1.0.0"), identity("aa", "1.1.0"), identity("aa", "1.2.0"), identity("bb", "1.0.0"), identity("cc", "1.0.0")];
        let asks: Vec<Vec<String>> = requests(&all, 2).iter().map(|r| r.iter().map(|i| format!("{}@{}", i.id, i.version)).collect()).collect();
        assert_eq!(asks, [vec!["aa@1.0.0", "bb@1.0.0"], vec!["aa@1.1.0", "cc@1.0.0"], vec!["aa@1.2.0"]]);
        assert!(requests(&[], 2).is_empty());
        let tmp = tempfile::tempdir().unwrap();
        let t = template(tmp.path(), "copy", None);
        for bad in [identity("Not An Id", "1.0.0"), identity("aa", ""), identity("aa", &"1".repeat(129)), Identity { hash: "h".into(), ..identity("aa", "1.0.0") }, Identity { hash: "AB".repeat(32), ..identity("aa", "1.0.0") }] {
            std::fs::write(t.join(LINEAGE), serde_json::to_vec(&[identity("aa", "1.0.0"), bad.clone()]).unwrap()).unwrap();
            assert_eq!(identities(&t), Err(Unreadable), "{bad:?}");
            assert!(all_identities(tmp.path()).is_empty(), "{bad:?}");
        }
    }

    /// What can't be read is started again where there's nothing it could
    /// block.
    #[test]
    fn what_cannot_be_read_is_started_again_where_nothing_is_here() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        template(dir, "aa", Some(record("aa", "1.0.0")));
        std::fs::write(dir.join(FILE), "nope").unwrap();
        assert!(refuse_package(dir, "00", None).is_err(), "a store template is here");
        std::fs::remove_dir_all(dir.join("aa")).unwrap();
        template(dir, "mine", None);
        assert!(refuse_package(dir, "00", None).is_ok());
        assert_eq!(read(dir), Ok(Vec::new()));
    }

    /// A take-down that couldn't be written holds all the same, and is
    /// written with the next that can be; a restore that couldn't, lifts
    /// nothing.
    #[cfg(unix)]
    #[test]
    fn what_cannot_be_written_is_held_to() {
        use std::os::unix::fs::PermissionsExt as _;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let t = template(dir, "aa", Some(record("aa", "1.0.0")));
        let mode = |mode| std::fs::set_permissions(dir, std::fs::Permissions::from_mode(mode)).unwrap();
        mode(0o555);
        assert!(apply(dir, vec![Change::TakenDown(block("aa", "1.0.0", NOW))], NOW).is_err());
        assert!(!dir.join(FILE).exists() && blocked(dir, &t).is_some());
        mode(0o755);
        apply(dir, vec![Change::TakenDown(block("bb", "1.0.0", NOW))], NOW).unwrap();
        assert_eq!(read_file(dir).unwrap().len(), 2, "both are written");
        // So does the full check's.
        mode(0o555);
        assert!(settle(dir, &[], vec![block("cc", "1.0.0", NOW)], NOW).is_err());
        assert!(read(dir).unwrap().iter().any(|b| b.id == "cc"));
        assert!(apply(dir, vec![Change::Restored("aa".into(), "1.0.0".into())], NOW).is_err());
        assert!(blocked(dir, &t).is_some());
        mode(0o755);
        apply(dir, vec![Change::Restored("aa".into(), "1.0.0".into())], NOW).unwrap();
        assert!(blocked(dir, &t).is_none());
    }

    /// An entry stays while something here is it, and 30 days besides.
    #[test]
    fn entries_are_kept_while_they_matter() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        template(dir, "aa", Some(record("aa", "1.0.0")));
        let past = NOW - KEEP_SECS - 1;
        apply(dir, vec![Change::TakenDown(block("aa", "1.0.0", past)), Change::TakenDown(block("gone", "1.0.0", past)), Change::TakenDown(block("fresh", "1.0.0", NOW))], NOW).unwrap();
        let ids = || read(dir).unwrap().into_iter().map(|b| b.id).collect::<Vec<_>>();
        assert_eq!(ids(), ["aa", "fresh"]);
        assert!(refuse_package(dir, &"AB".repeat(32), None).unwrap_err().starts_with("taken_down: "), "by its contents, from anywhere");
        assert!(refuse_package(dir, "00", Some(&Identity { id: "fresh".into(), version: "1.0.0".into(), hash: "00".into() })).is_err());
        assert!(refuse_package(dir, "00", Some(&Identity { id: "fresh".into(), version: "1.0.1".into(), hash: "00".into() })).is_ok());
        // The full check lifts only what it was asked about.
        settle(dir, &all_identities(dir), Vec::new(), NOW).unwrap();
        assert_eq!(ids(), ["fresh"]);
        // And keeps when one was learned of, and its hash.
        apply(dir, vec![Change::TakenDown(block("aa", "1.0.0", NOW - 5))], NOW).unwrap();
        settle(dir, &all_identities(dir), vec![Block { hash: None, at: NOW, ..block("aa", "1.0.0", NOW) }], NOW).unwrap();
        let a = read(dir).unwrap().into_iter().find(|b| b.id == "aa").unwrap();
        assert_eq!((a.at, a.hash.is_some()), (NOW - 5, true));
        // Something unreadable here may be any of them: nothing is dropped.
        std::fs::write(template(dir, "broken", None).join(LINEAGE), "nope").unwrap();
        apply(dir, vec![Change::TakenDown(block("old", "1.0.0", past))], NOW).unwrap();
        assert!(ids().contains(&"old".to_owned()));
    }
}
