//! Incremental joined-transcript mirror.
//!
//! Every watched chat used to rebuild its whole transcript on each ~120ms
//! commit: a deep value of the messages list, a decode of every entry, the
//! continuation join, and then a deep `==` of every entry against the previous
//! snapshot per watcher. That is O(n) per tick and O(n²) per stream.
//!
//! The mirror keeps the decoded entries between ticks. A `subscribe_root`
//! observer only *classifies* diff events ([`TranscriptMirror::observe`]:
//! which entry maps were touched, whether the list itself changed), and
//! [`TranscriptMirror::refresh`] re-decodes just those maps through the same
//! decoder `read_entries` uses ([`decode_entry_value`]), then regroups
//! continuations with a pointer walk. Every untouched entry keeps its `Arc`, so
//! consumers can short-circuit on `Arc::ptr_eq`
//! ([`crate::diff_transcript_shared`]).
//!
//! The full rebuild stays available as [`joined_full`] and is the oracle:
//! under `cfg(debug_assertions)`, and in release when `ZERON_PERF_VERIFY=1`
//! (sampled 1 in 32), `refresh` compares against it. On a mismatch it logs an
//! error, returns the full result and resets the mirror. It never panics.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use loro::event::DiffEvent;
use loro::{ContainerID, Index, LoroDoc, LoroValue, ToJson, ValueOrContainer, VersionVector};

use crate::schema::{
    DocError, SessionDoc, SessionMessageEntry, decode_entry_value, join_continuation_entries,
};

/// One decoded transcript entry shared between the mirror, the published
/// snapshot and every watcher.
pub type SharedEntry = Arc<SessionMessageEntry>;

/// How often a release build cross-checks against the full rebuild when
/// `ZERON_PERF_VERIFY=1`.
const VERIFY_SAMPLE: u64 = 32;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// When [`TranscriptMirror::refresh`] cross-checks against the full rebuild.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Verify {
    /// Every refresh in debug builds; in release only when `ZERON_PERF_VERIFY=1`,
    /// sampled 1 in 32.
    #[default]
    Auto,
    Always,
    Never,
}

/// What the observer has seen since the last refresh.
#[derive(Default)]
struct Pending {
    /// Entry maps whose content (or a descendant's) changed.
    dirty: HashSet<ContainerID>,
    /// The messages list itself changed (insert, delete).
    structure: bool,
    /// Unclassifiable event, or a fresh/reset mirror: rebuild everything.
    full: bool,
}

impl Pending {
    fn classify(&mut self, path: &[(ContainerID, Index)]) {
        match path.first() {
            Some((_, Index::Key(key))) if key.as_str() == "messages" => match path.get(1) {
                None => self.structure = true,
                Some((entry, Index::Seq(_))) => {
                    self.dirty.insert(entry.clone());
                }
                _ => self.full = true,
            },
            Some((_, Index::Key(_))) => {}
            _ => self.full = true,
        }
    }
}

struct Slot {
    /// `None` for a plain value stored inline in the list (never a map
    /// container; such a slot only changes with the list itself).
    cid: Option<ContainerID>,
    /// `None` = unsalvageable, skipped exactly as `read_entries` skips it.
    entry: Option<SharedEntry>,
}

#[derive(Default)]
struct State {
    /// False until the first rebuild, and again after a reset.
    primed: bool,
    /// Committed state represented by the cache, independently of callback delivery.
    version: VersionVector,
    slots: Vec<Slot>,
    index: HashMap<ContainerID, usize>,
    /// Published `join_continuation_entries(read_entries())`.
    joined: Arc<Vec<SharedEntry>>,
    /// Joined root with continuations, keyed by the root's allocation address.
    /// The members are retained so a cached address cannot be recycled by a
    /// different entry while the key is live.
    joined_cache: HashMap<usize, (Vec<SharedEntry>, SharedEntry)>,
    /// Entries whose decode failure was already logged this generation.
    warned: HashSet<ContainerID>,
    decodes: u64,
}

/// Incrementally maintained `join(read_entries())` for one chat doc.
///
/// Interior mutability is split: the observer only touches `pending` (a short
/// lock taken inside the doc's commit callback), while `refresh` holds `state`
/// for the duration of a rebuild. A multi-millisecond rebuild therefore never
/// blocks the writer whose commit fired the callback.
pub struct TranscriptMirror {
    pending: Mutex<Pending>,
    state: Mutex<State>,
    verify: Verify,
    refreshes: AtomicU64,
    mismatches: AtomicU64,
}

impl Default for TranscriptMirror {
    fn default() -> Self {
        Self::new()
    }
}

impl TranscriptMirror {
    pub fn new() -> Self {
        Self {
            pending: Mutex::new(Pending {
                full: true,
                ..Pending::default()
            }),
            state: Mutex::new(State::default()),
            verify: Verify::Auto,
            refreshes: AtomicU64::new(0),
            mismatches: AtomicU64::new(0),
        }
    }

    pub fn with_verify(mut self, verify: Verify) -> Self {
        self.verify = verify;
        self
    }

    /// Classify a doc event. O(#diffs); never touches the doc or the entries.
    pub fn observe(&self, event: &DiffEvent<'_>) {
        let mut pending = lock(&self.pending);
        for change in &event.events {
            if change.is_unknown {
                pending.full = true;
                continue;
            }
            pending.classify(change.path);
        }
    }

    /// Forget everything: the next refresh rebuilds from the doc. Called when
    /// nobody watches (events are no longer observed) and after a mismatch.
    pub fn reset(&self) {
        {
            let mut pending = lock(&self.pending);
            pending.dirty.clear();
            pending.structure = false;
            pending.full = true;
        }
        // Release the memory unless a refresh is mid-flight; that refresh sees
        // `full` on its next drain and rebuilds regardless.
        if let Ok(mut state) = self.state.try_lock() {
            *state = State::default();
        }
    }

    /// Bring the mirror up to date and return the joined transcript.
    ///
    /// Drain-then-read: the pending set is taken before the doc is read, so a
    /// commit racing the read re-marks itself. Also reconcile committed operation
    /// spans: document state can advance before its callback reaches observe.
    pub fn refresh(&self, doc: &SessionDoc) -> Arc<Vec<SharedEntry>> {
        self.refreshes.fetch_add(1, Ordering::Relaxed);
        let mut state = lock(&self.state);
        let mut pending = std::mem::take(&mut *lock(&self.pending));
        let loro = doc.doc();
        let version = loro.state_vv();
        // Uncommitted edits are already visible to read_entries, but have no
        // diff event or finalized op span. Read one atomic full snapshot and
        // leave the incremental indexes unprimed until a later refresh.
        if loro.get_pending_txn_len() != 0 {
            return self.fallback(doc, &mut state);
        }
        if state.primed && !pending.full {
            if state.version.sub_iter(&version).next().is_some() {
                // Checkout can remove ops, including in an unwatched interval.
                pending.full = true;
            } else {
                // This read-only API does NOT implicitly commit (unlike
                // get_changed_containers_in). Only newly committed spans are
                // expanded; settled transcript content is never copied.
                let mut touched = HashSet::new();
                for span in version.sub_iter(&state.version) {
                    let changes = loro.export_json_in_id_span(span);
                    let len: usize = changes.iter().map(|change| change.op_len()).sum();
                    if len != (span.counter.end - span.counter.start) as usize {
                        // Missing history, e.g. a shallow snapshot import.
                        pending.full = true;
                        break;
                    }
                    touched.extend(
                        changes
                            .into_iter()
                            .flat_map(|change| change.ops.into_iter().map(|op| op.container)),
                    );
                }
                for cid in touched {
                    match loro.get_path_to_container(&cid) {
                        Some(path) => pending.classify(&path),
                        None => pending.full = true,
                    }
                }
            }
        }
        let full = pending.full || !state.primed;
        let mut changed = full;
        if full {
            state.rewalk(doc, &mut pending.dirty, false);
        } else {
            if pending.structure {
                state.rewalk(doc, &mut pending.dirty, true);
                changed = true;
            }
            for cid in &pending.dirty {
                let Some(&ix) = state.index.get(cid) else {
                    continue;
                };
                let entry = state.decode(doc, loro, cid);
                // A touched map that decodes to the same entry keeps its Arc,
                // so nothing downstream sees a change.
                if entry != state.slots[ix].entry {
                    state.slots[ix].entry = entry;
                    changed = true;
                }
            }
        }
        if changed {
            state.rejoin();
        }
        if !version_is_current(loro, &version) {
            // Per-entry reads raced a writer. A full list deep value is taken
            // under one Loro state lock; decoding/joining happens after release.
            return self.fallback(doc, &mut state);
        }
        state.version = version.clone();
        let joined = state.joined.clone();
        if !self.verify_due() {
            return joined;
        }
        let Ok(raw) = doc.read_entries() else {
            return joined;
        };
        let expected: Vec<_> = join_continuation_entries(raw.clone())
            .into_iter()
            .map(Arc::new)
            .collect();
        if !version_is_current(loro, &version) {
            state.use_full(raw);
            lock(&self.pending).full = true;
            return state.joined.clone();
        }
        if let Some(at) = first_difference(&joined, &expected) {
            self.mismatches.fetch_add(1, Ordering::Relaxed);
            tracing::error!(
                chat = %doc.chat_id().unwrap_or_default(),
                at,
                incremental = joined.len(),
                full = expected.len(),
                "transcript mirror diverged from the full rebuild; using the full result"
            );
            // doc_host resolves presentation provenance from with_raw right
            // after refresh. Keep the raw side of the same full snapshot, even
            // though the incremental indexes must be rebuilt next time.
            state.use_full(raw);
            lock(&self.pending).full = true;
            return state.joined.clone();
        }
        joined
    }

    fn fallback(&self, doc: &SessionDoc, state: &mut State) -> Arc<Vec<SharedEntry>> {
        if let Ok(raw) = doc.read_entries() {
            state.use_full(raw);
        }
        lock(&self.pending).full = true;
        state.joined.clone()
    }

    /// The raw (continuations not joined) decodable entries in list order, as
    /// of the last refresh. Used for presentation provenance, which resolves
    /// each entry's joined id itself.
    pub fn with_raw<R>(
        &self,
        f: impl FnOnce(&mut dyn Iterator<Item = &SessionMessageEntry>) -> R,
    ) -> R {
        let state = lock(&self.state);
        f(&mut state.slots.iter().filter_map(|slot| slot.entry.as_deref()))
    }

    /// Cross-check failures seen so far.
    pub fn mismatches(&self) -> u64 {
        self.mismatches.load(Ordering::Relaxed)
    }

    /// Entry decodes performed so far (perf tests assert O(changed)).
    pub fn decodes(&self) -> u64 {
        lock(&self.state).decodes
    }

    fn verify_due(&self) -> bool {
        match self.verify {
            Verify::Never => false,
            Verify::Always => true,
            Verify::Auto => {
                if cfg!(debug_assertions) {
                    return true;
                }
                static ENABLED: OnceLock<bool> = OnceLock::new();
                let enabled = *ENABLED
                    .get_or_init(|| std::env::var("ZERON_PERF_VERIFY").is_ok_and(|v| v == "1"));
                enabled && self.refreshes.load(Ordering::Relaxed) % VERIFY_SAMPLE == 0
            }
        }
    }
}

/// The full rebuild the mirror must agree with: `read_entries` followed by
/// `join_continuation_entries`.
pub fn joined_full(doc: &SessionDoc) -> Result<Vec<SharedEntry>, DocError> {
    Ok(join_continuation_entries(doc.read_entries()?)
        .into_iter()
        .map(Arc::new)
        .collect())
}

// Check both sides of the transaction-length read: a commit can finalize
// visible edits between reading the frontiers and acquiring the txn lock.
fn version_is_current(doc: &LoroDoc, version: &VersionVector) -> bool {
    doc.state_vv() == *version && doc.get_pending_txn_len() == 0 && doc.state_vv() == *version
}

fn first_difference(a: &[SharedEntry], b: &[SharedEntry]) -> Option<usize> {
    if let Some(at) = a
        .iter()
        .zip(b)
        .position(|(x, y)| !Arc::ptr_eq(x, y) && **x != **y)
    {
        return Some(at);
    }
    (a.len() != b.len()).then(|| a.len().min(b.len()))
}

impl State {
    fn use_full(&mut self, raw: Vec<SessionMessageEntry>) {
        *self = Self::default();
        self.slots = raw
            .into_iter()
            .map(|entry| Slot {
                cid: None,
                entry: Some(Arc::new(entry)),
            })
            .collect();
        self.rejoin();
    }

    /// Decode one entry container exactly as `read_entries` does: deep value
    /// to JSON, then the shared strict-then-salvage decoder.
    fn decode(
        &mut self,
        doc: &SessionDoc,
        loro: &LoroDoc,
        cid: &ContainerID,
    ) -> Option<SharedEntry> {
        self.decodes += 1;
        let container = loro.get_container(cid.clone())?;
        let json = ValueOrContainer::Container(container)
            .get_deep_value()
            .to_json_value();
        match decode_entry_value(&json) {
            Ok(entry) => {
                self.warned.remove(cid);
                Some(Arc::new(entry))
            }
            Err(err) => {
                if self.warned.insert(cid.clone()) {
                    tracing::warn!(
                        chat = %doc.chat_id().unwrap_or_default(),
                        error = %err,
                        "skipping unsalvageable transcript entry"
                    );
                }
                None
            }
        }
    }

    /// Rebuild `slots` from the list's shallow value (container ids only, no
    /// deep copy). With `reuse`, an unchanged container keeps its decoded
    /// entry; a container in `dirty`, or one not seen before, is decoded and
    /// removed from `dirty`.
    fn rewalk(&mut self, doc: &SessionDoc, dirty: &mut HashSet<ContainerID>, reuse: bool) {
        let loro = doc.doc();
        let items = match loro.get_list("messages").get_value() {
            LoroValue::List(items) => items,
            _ => Default::default(),
        };
        let mut old: HashMap<ContainerID, Option<SharedEntry>> = if reuse {
            std::mem::take(&mut self.slots)
                .into_iter()
                .filter_map(|slot| Some((slot.cid?, slot.entry)))
                .collect()
        } else {
            self.slots.clear();
            self.warned.clear();
            HashMap::new()
        };
        let mut slots = Vec::with_capacity(items.len());
        let mut index = HashMap::with_capacity(items.len());
        for item in items.iter() {
            match item {
                LoroValue::Container(cid) => {
                    let was_dirty = dirty.remove(cid);
                    let entry = match old.remove(cid) {
                        Some(entry) if !was_dirty => entry,
                        _ => self.decode(doc, loro, cid),
                    };
                    index.insert(cid.clone(), slots.len());
                    slots.push(Slot {
                        cid: Some(cid.clone()),
                        entry,
                    });
                }
                // A plain value in the list: decode it as `read_entries` would.
                value => slots.push(Slot {
                    cid: None,
                    entry: decode_entry_value(&value.to_json_value())
                        .ok()
                        .map(Arc::new),
                }),
            }
        }
        self.slots = slots;
        self.index = index;
        self.primed = true;
    }

    /// Regroup continuations onto their roots: a pointer walk, with every
    /// joined root whose members are unchanged reused from the cache. Mirrors
    /// `join_continuation_entries`, including the orphan-continuation rule and
    /// "the last continuation's `duration_ms` wins".
    fn rejoin(&mut self) {
        let State {
            slots,
            joined,
            joined_cache,
            ..
        } = self;
        let entries = || slots.iter().filter_map(|slot| slot.entry.as_ref());
        if !entries().any(|entry| entry.continuation_of.is_some()) {
            joined_cache.clear();
            *joined = Arc::new(entries().cloned().collect());
            return;
        }
        struct Group<'a> {
            root: &'a SharedEntry,
            continuations: Vec<&'a SharedEntry>,
        }
        let mut groups: Vec<Group<'_>> = Vec::new();
        let mut root_index: HashMap<&str, usize> = HashMap::new();
        for entry in entries() {
            match &entry.continuation_of {
                Some(root_id) => match root_index.get(root_id.as_str()) {
                    Some(&at) => groups[at].continuations.push(entry),
                    // Orphan continuation: its own entry, and not a root.
                    None => groups.push(Group {
                        root: entry,
                        continuations: Vec::new(),
                    }),
                },
                None => {
                    root_index.insert(entry.id.as_str(), groups.len());
                    groups.push(Group {
                        root: entry,
                        continuations: Vec::new(),
                    });
                }
            }
        }
        let mut previous = std::mem::take(joined_cache);
        let mut out = Vec::with_capacity(groups.len());
        for group in groups {
            if group.continuations.is_empty() {
                out.push(group.root.clone());
                continue;
            }
            let key = Arc::as_ptr(group.root) as usize;
            let reusable = previous.remove(&key).filter(|(members, _)| {
                members.len() == 1 + group.continuations.len()
                    && Arc::ptr_eq(&members[0], group.root)
                    && members[1..]
                        .iter()
                        .zip(&group.continuations)
                        .all(|(a, b)| Arc::ptr_eq(a, b))
            });
            let (members, entry) = reusable.unwrap_or_else(|| {
                let mut message = (**group.root).clone();
                for continuation in &group.continuations {
                    message.parts.extend(continuation.parts.iter().cloned());
                    if continuation.duration_ms.is_some() {
                        message.duration_ms = continuation.duration_ms;
                    }
                }
                let members = std::iter::once(group.root)
                    .chain(group.continuations.iter().copied())
                    .cloned()
                    .collect();
                (members, Arc::new(message))
            });
            joined_cache.insert(key, (members, entry.clone()));
            out.push(entry);
        }
        *joined = Arc::new(out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_callbacks_do_not_wait_for_the_rebuild_lock() {
        use std::sync::mpsc;
        use std::time::Duration;
        let doc = Arc::new(SessionDoc::init("chat").unwrap());
        let mirror = Arc::new(TranscriptMirror::new());
        let _sub = {
            let mirror = mirror.clone();
            doc.doc().subscribe_root(Arc::new(move |event| {
                mirror.observe(&event);
                mirror.reset();
            }))
        };
        // Stand in for a deliberately stalled rebuild. Both observer paths
        // must finish while this guard is still held.
        let rebuilding = lock(&mirror.state);
        let (tx, rx) = mpsc::channel();
        let writer = std::thread::spawn(move || {
            doc.doc().get_map("meta").insert("test", "changed").unwrap();
            doc.doc().commit();
            tx.send(()).unwrap();
        });
        let completed = rx.recv_timeout(Duration::from_secs(2));
        drop(rebuilding);
        writer.join().unwrap();
        completed.expect("commit waited for the mirror rebuild lock");
    }

    #[test]
    fn oracle_fallback_keeps_raw_entries_for_presentation_history() {
        let doc = SessionDoc::init("chat").unwrap();
        let mirror = TranscriptMirror::new().with_verify(Verify::Always);
        let map = doc
            .doc()
            .get_list("messages")
            .push_container(loro::LoroMap::new())
            .unwrap();
        map.insert("id", "root").unwrap();
        doc.doc().commit();
        mirror.refresh(&doc);
        // Simulate cache corruption at an unchanged document version so the
        // oracle itself, rather than version reconciliation, must recover.
        lock(&mirror.state).joined = Arc::default();
        let joined = mirror.refresh(&doc);
        assert_eq!(joined.len(), 1);
        assert_eq!(mirror.mismatches(), 1);
        mirror.with_raw(|raw| {
            assert_eq!(
                raw.cloned().collect::<Vec<_>>(),
                doc.read_entries().unwrap()
            );
        });
        assert_eq!(*mirror.refresh(&doc), *joined);
        assert_eq!(mirror.mismatches(), 1);
    }
}
