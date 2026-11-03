//! Session notes: one markdown scratchpad per chat, written by the user.
//!
//! The text lives in the session doc's root `notes` LoroText, so it rides the
//! same chat2 room as the transcript. The room relays opaque update blobs and
//! never parses them, which is why a new root container needs no edge change.
//! It is a ROOT container on purpose: two devices that each created a child
//! container (say under `meta`) concurrently would each make their own, and
//! one side's text would lose. Root containers are the same container on
//! every peer. Any device writes here, like `queue`; `meta` stays host-only.
//!
//! Viewports don't hold the doc (the desktop UI talks RPC to its engine), so
//! writes are compare-and-set on a hash of the notes text: a viewport sends the
//! whole text it wants plus the hash of the text it started from, and
//! [`SessionDoc::write_notes`] applies it only if the notes still read that
//! way. The doc then records the difference as character ops, so concurrent
//! edits from other devices still merge character by character in Loro. A
//! viewport that loses the race rebases its edit with [`rebase_notes`] and
//! retries. The hash covers the notes text alone; doc frontiers would move on
//! every streamed token and make every write during a turn a conflict.
//!
//! A hash says what the notes read, not when. A viewport's watch reports its
//! own writes back, sometimes after a newer one has landed, and another device
//! can restore text the viewport wrote earlier: same hash, opposite meaning.
//! So every snapshot also carries a version, `epoch` and `seq`. `seq` counts
//! the distinct states this doc instance has reported, and `epoch` names the
//! instance, so a reopened doc's restarted count is never taken for an old one.
//! A snapshot no newer than a viewport's base is stale and is dropped; any
//! newer one is a real change, even if its text is one the viewport wrote.

use std::ops::Range;

use serde::{Deserialize, Serialize};

use crate::constants::NOTES_MAX_BYTES;
use crate::schema::{DocError, SessionDoc};

/// Root container name of the notes text.
pub const NOTES_CONTAINER: &str = "notes";

/// Whether a doc event diff targets the notes text. A text has no child
/// containers, so the target alone decides.
pub fn is_notes_container(id: &loro::ContainerID) -> bool {
    matches!(id, loro::ContainerID::Root { name, .. } if name.as_str() == NOTES_CONTAINER)
}

/// What a batch of doc events touched, split at the notes. Lets the transcript
/// machinery skip batches that only carried someone's typing in the notes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NotesTouch {
    pub notes: bool,
    pub other: bool,
}

impl NotesTouch {
    pub fn of(event: &loro::event::DiffEvent<'_>) -> Self {
        let mut touch = Self::default();
        for change in &event.events {
            if is_notes_container(change.target) {
                touch.notes = true;
            } else {
                touch.other = true;
            }
        }
        touch
    }
}

/// The notes text as one viewport last saw it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotesSnapshot {
    pub text: String,
    /// [`notes_hash`] of `text`: the base of the viewport's next write.
    pub hash: String,
    /// The doc instance that reported this state (see the module docs).
    #[serde(default)]
    pub epoch: u64,
    /// Which of that instance's reported states this is.
    #[serde(default)]
    pub seq: u64,
}

impl NotesSnapshot {
    /// An unversioned snapshot of `text`.
    pub fn of(text: impl Into<String>) -> Self {
        let text = text.into();
        let hash = notes_hash(&text);
        Self {
            text,
            hash,
            epoch: 0,
            seq: 0,
        }
    }

    /// Whether this state came after `base`: from another doc instance, or
    /// later from the same one. A stale report of an older state is not.
    pub fn supersedes(&self, base: &NotesSnapshot) -> bool {
        self.epoch != base.epoch || self.seq > base.seq
    }
}

/// Where a doc instance's notes reports have got to. Lives inside the notes
/// gate, so a report and the state it names never come apart.
#[derive(Debug)]
pub(crate) struct NotesClock {
    epoch: u64,
    /// Hash of the last state reported; empty before the first.
    hash: String,
    seq: u64,
}

impl Default for NotesClock {
    fn default() -> Self {
        use std::hash::BuildHasher;
        // Random per instance, and inside 2^53 so a JSON number keeps it.
        let epoch = std::collections::hash_map::RandomState::new().hash_one(0u8) & ((1 << 53) - 1);
        Self {
            epoch,
            hash: String::new(),
            seq: 0,
        }
    }
}

impl NotesClock {
    /// The version of a state with `hash`: the current one if nothing
    /// changed since the last report, the next one otherwise.
    fn observe(&mut self, hash: &str) -> u64 {
        if self.hash != hash {
            self.hash = hash.to_owned();
            self.seq += 1;
        }
        self.seq
    }
}

/// What a compare-and-set write did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum NotesWrite {
    /// The notes now read as written (possibly unchanged): `hash`, `epoch`
    /// and `seq` are their new base.
    #[serde(rename_all = "camelCase")]
    Applied {
        hash: String,
        #[serde(default)]
        epoch: u64,
        #[serde(default)]
        seq: u64,
    },
    /// The notes changed since the writer's base. Nothing was written;
    /// rebase onto `current` and retry.
    Conflict { current: NotesSnapshot },
    /// The write would grow the notes past `limit` bytes. Nothing was written.
    #[serde(rename_all = "camelCase")]
    TooLarge { limit: usize },
}

/// Stable 64-bit FNV-1a over the UTF-8 bytes, as 16 lowercase hex digits.
/// Every engine and client must agree on it across releases and platforms,
/// so it is spelled out here rather than borrowed from `std::hash`, whose
/// output is unspecified.
pub fn notes_hash(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// One contiguous replacement: `range` (UTF-8 bytes of the old text) becomes
/// `insert`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEdit {
    pub range: Range<usize>,
    pub insert: String,
}

impl TextEdit {
    pub fn apply(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len() + self.insert.len());
        out.push_str(&text[..self.range.start]);
        out.push_str(&self.insert);
        out.push_str(&text[self.range.end..]);
        out
    }

    /// Where an offset into the old text lands in the new one. Offsets before
    /// the edit stay; offsets after it shift; offsets inside the replaced
    /// range move to the end of the insertion.
    pub fn map_offset(&self, offset: usize) -> usize {
        if offset <= self.range.start {
            offset
        } else if offset >= self.range.end {
            offset - self.range.len() + self.insert.len()
        } else {
            self.range.start + self.insert.len()
        }
    }
}

/// The smallest single replacement turning `old` into `new`: the common
/// prefix and suffix stay, everything between is replaced. `None` when the
/// texts are equal. Offsets always sit on char boundaries.
pub fn single_range_diff(old: &str, new: &str) -> Option<TextEdit> {
    if old == new {
        return None;
    }
    let (ob, nb) = (old.as_bytes(), new.as_bytes());
    let mut prefix = ob.iter().zip(nb).take_while(|(a, b)| a == b).count();
    while !old.is_char_boundary(prefix) || !new.is_char_boundary(prefix) {
        prefix -= 1;
    }
    let room = ob.len().min(nb.len()) - prefix;
    let mut suffix = ob
        .iter()
        .rev()
        .zip(nb.iter().rev())
        .take(room)
        .take_while(|(a, b)| a == b)
        .count();
    while !old.is_char_boundary(ob.len() - suffix) || !new.is_char_boundary(nb.len() - suffix) {
        suffix -= 1;
    }
    Some(TextEdit {
        range: prefix..ob.len() - suffix,
        insert: new[prefix..nb.len() - suffix].to_string(),
    })
}

/// Three-way merge of two single-range edits of `base`. Edits that don't
/// overlap both apply. Overlapping ones honor both deletions and keep both
/// insertions, the remote one first; the same goes for two insertions at one
/// spot. Nobody's typing is dropped, at worst it lands next to the other's.
pub fn rebase_notes(base: &str, local: &str, remote: &str) -> String {
    if local == remote {
        return local.to_string();
    }
    let (Some(l), Some(r)) = (
        single_range_diff(base, local),
        single_range_diff(base, remote),
    ) else {
        // One side didn't change anything: the other side is the merge.
        return if local == base { remote } else { local }.to_string();
    };
    let mut out = String::with_capacity(base.len() + l.insert.len() + r.insert.len());
    if r.range.end <= l.range.start {
        out.push_str(&base[..r.range.start]);
        out.push_str(&r.insert);
        out.push_str(&base[r.range.end..l.range.start]);
        out.push_str(&l.insert);
        out.push_str(&base[l.range.end..]);
    } else if l.range.end <= r.range.start {
        out.push_str(&base[..l.range.start]);
        out.push_str(&l.insert);
        out.push_str(&base[l.range.end..r.range.start]);
        out.push_str(&r.insert);
        out.push_str(&base[r.range.end..]);
    } else {
        // Every base byte in the union was deleted by one side or the other.
        let start = l.range.start.min(r.range.start);
        let end = l.range.end.max(r.range.end);
        out.push_str(&base[..start]);
        out.push_str(&r.insert);
        out.push_str(&l.insert);
        out.push_str(&base[end..]);
    }
    out
}

impl SessionDoc {
    pub fn notes_text(&self) -> String {
        self.doc().get_text(NOTES_CONTAINER).to_string()
    }

    /// The notes as they read now, versioned (see the module docs).
    pub fn notes_snapshot(&self) -> NotesSnapshot {
        let mut clock = self.lock_notes();
        self.snapshot_locked(&mut clock)
    }

    fn snapshot_locked(&self, clock: &mut NotesClock) -> NotesSnapshot {
        let text = self.notes_text();
        let hash = notes_hash(&text);
        let seq = clock.observe(&hash);
        NotesSnapshot {
            text,
            hash,
            epoch: clock.epoch,
            seq,
        }
    }

    /// Make the notes read `text`, recording only the changed range as text
    /// ops. Unconditional: for copies between docs (forks, rebuilds) and
    /// writers that already hold the doc. `false` when nothing changed.
    pub fn set_notes_text(&self, text: &str) -> Result<bool, DocError> {
        let _gate = self.lock_notes();
        self.apply_notes_text(text)
    }

    /// Updates from other replicas, imported under the notes gate so they
    /// never land between a notes write's read and its ops. Live sync imports
    /// into a shared doc go through here.
    pub fn import_with(
        &self,
        bytes: &[u8],
        origin: &str,
    ) -> Result<loro::ImportStatus, loro::LoroError> {
        let _gate = self.lock_notes();
        self.doc().import_with(bytes, origin)
    }

    fn lock_notes(&self) -> std::sync::MutexGuard<'_, NotesClock> {
        self.notes_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The edit behind [`Self::set_notes_text`]; the caller holds the gate.
    fn apply_notes_text(&self, text: &str) -> Result<bool, DocError> {
        let notes = self.doc().get_text(NOTES_CONTAINER);
        let Some(edit) = single_range_diff(&notes.to_string(), text) else {
            return Ok(false);
        };
        if !edit.range.is_empty() {
            notes.delete_utf8(edit.range.start, edit.range.len())?;
        }
        if !edit.insert.is_empty() {
            notes.insert_utf8(edit.range.start, &edit.insert)?;
        }
        self.doc().commit();
        Ok(true)
    }

    /// Compare-and-set write from a viewport (see the module docs). Shrinking
    /// is always allowed, so notes that arrived over the limit from another
    /// device can still be trimmed.
    pub fn write_notes(&self, base_hash: &str, text: &str) -> Result<NotesWrite, DocError> {
        // The check and the write are one step: two writers on one base must
        // not both be told Applied.
        let mut clock = self.lock_notes();
        let current = self.snapshot_locked(&mut clock);
        if current.hash != base_hash {
            return Ok(NotesWrite::Conflict { current });
        }
        if text.len() > NOTES_MAX_BYTES && text.len() > current.text.len() {
            return Ok(NotesWrite::TooLarge {
                limit: NOTES_MAX_BYTES,
            });
        }
        self.apply_notes_text(text)?;
        // Still under the gate: the notes read exactly `text`.
        let hash = notes_hash(text);
        let seq = clock.observe(&hash);
        Ok(NotesWrite::Applied {
            hash,
            epoch: clock.epoch,
            seq,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> SessionDoc {
        SessionDoc::init("chat-1").unwrap()
    }

    fn peer(source: &SessionDoc) -> SessionDoc {
        let other = SessionDoc::from_doc(loro::LoroDoc::new());
        other
            .doc()
            .import(&source.export_snapshot().unwrap())
            .unwrap();
        other
    }

    fn sync(a: &SessionDoc, b: &SessionDoc) {
        let to_b = a
            .doc()
            .export(loro::ExportMode::updates(&b.doc().oplog_vv()))
            .unwrap();
        let to_a = b
            .doc()
            .export(loro::ExportMode::updates(&a.doc().oplog_vv()))
            .unwrap();
        b.doc().import(&to_b).unwrap();
        a.doc().import(&to_a).unwrap();
    }

    #[test]
    fn hash_is_stable() {
        // Pinned: changing this breaks every viewport's base across versions.
        assert_eq!(notes_hash(""), "cbf29ce484222325");
        assert_eq!(notes_hash("a"), "af63dc4c8601ec8c");
        assert_ne!(notes_hash("ab"), notes_hash("ba"));
    }

    #[test]
    fn diff_finds_the_smallest_range() {
        assert_eq!(single_range_diff("same", "same"), None);
        let edit = single_range_diff("hello world", "hello brave world").unwrap();
        assert_eq!(edit.range, 6..6);
        assert_eq!(edit.insert, "brave ");
        let edit = single_range_diff("abcdef", "abef").unwrap();
        assert_eq!(edit.range, 2..4);
        assert_eq!(edit.insert, "");
        // Repeated letters: prefix wins, the suffix never overlaps it.
        let edit = single_range_diff("aaa", "aaaa").unwrap();
        assert_eq!(edit.apply("aaa"), "aaaa");
        assert_eq!(edit.range, 3..3);
    }

    #[test]
    fn diff_respects_char_boundaries() {
        // é = C3 A9, ê = C3 AA: a shared lead byte must not split the char.
        let edit = single_range_diff("café", "cafê").unwrap();
        assert_eq!(edit.range, 3..5);
        assert_eq!(edit.insert, "ê");
        // Two emoji sharing their first three bytes.
        let edit = single_range_diff("x😀y", "x😃y").unwrap();
        assert_eq!(edit.apply("x😀y"), "x😃y");
        for (old, new) in [("ñandú", "ñu"), ("日本語", "日本"), ("", "📝"), ("📝", "")]
        {
            let edit = single_range_diff(old, new).unwrap();
            assert_eq!(edit.apply(old), new);
        }
    }

    #[test]
    fn map_offset_follows_the_edit() {
        let edit = TextEdit {
            range: 2..4,
            insert: "XYZ".into(),
        };
        assert_eq!(edit.map_offset(1), 1);
        assert_eq!(edit.map_offset(2), 2);
        assert_eq!(edit.map_offset(3), 5);
        assert_eq!(edit.map_offset(4), 5);
        assert_eq!(edit.map_offset(6), 7);
    }

    #[test]
    fn rebase_disjoint_edits_apply_both() {
        let base = "one\ntwo\nthree\n";
        let local = "one!\ntwo\nthree\n";
        let remote = "one\ntwo\nthree?\n";
        assert_eq!(rebase_notes(base, local, remote), "one!\ntwo\nthree?\n");
        assert_eq!(rebase_notes(base, remote, local), "one!\ntwo\nthree?\n");
    }

    #[test]
    fn rebase_same_spot_keeps_both_remote_first() {
        assert_eq!(rebase_notes("ab", "aLb", "aRb"), "aRLb");
    }

    #[test]
    fn rebase_overlap_honors_both_deletions() {
        // Local rewrites "bcd", remote rewrites "cde": both deleted spans go,
        // both insertions stay.
        assert_eq!(rebase_notes("abcdef", "aXef", "abYf"), "aYXf");
    }

    #[test]
    fn rebase_one_sided() {
        assert_eq!(rebase_notes("a", "a", "b"), "b");
        assert_eq!(rebase_notes("a", "b", "a"), "b");
        assert_eq!(rebase_notes("a", "b", "b"), "b");
    }

    #[test]
    fn rebase_never_splits_chars() {
        let base = "ñ😀é日";
        let edits = ["", "ñ", "😀é日", "ñ😀X日", "ñ😀é日📝", "Ωñ😀é日"];
        for local in edits {
            for remote in edits {
                // Slicing would panic on a non-boundary offset.
                let merged = rebase_notes(base, local, remote);
                assert!(merged.is_char_boundary(merged.len()));
            }
        }
    }

    #[test]
    fn write_is_compare_and_set() {
        let doc = doc();
        let empty = doc.notes_snapshot();
        assert_eq!(empty.text, "");
        let applied = doc.write_notes(&empty.hash, "# Plan\n").unwrap();
        assert_eq!(
            applied,
            NotesWrite::Applied {
                hash: notes_hash("# Plan\n"),
                epoch: empty.epoch,
                seq: empty.seq + 1,
            }
        );
        // A writer still on the old base loses and is told what is there.
        match doc.write_notes(&empty.hash, "other").unwrap() {
            NotesWrite::Conflict { current } => {
                assert_eq!(current.text, "# Plan\n");
                assert_eq!(current.seq, empty.seq + 1);
            }
            other => panic!("expected conflict, got {other:?}"),
        }
        assert_eq!(doc.notes_text(), "# Plan\n");
    }

    #[test]
    fn versions_order_states_even_when_text_repeats() {
        let doc = doc();
        let a = doc.notes_snapshot();
        doc.set_notes_text("ab").unwrap();
        let ab = doc.notes_snapshot();
        // Unchanged notes report the same version.
        assert_eq!(doc.notes_snapshot().seq, ab.seq);
        // Another device restores "a": same text as before, a newer state.
        doc.set_notes_text("").unwrap();
        let restored = doc.notes_snapshot();
        assert_eq!(restored.hash, a.hash);
        assert!(restored.supersedes(&ab));
        assert!(
            !a.supersedes(&ab),
            "the old report of the same text is stale"
        );
        // A reopened doc counts from scratch under a new epoch.
        let reopened = peer(&doc).notes_snapshot();
        assert_ne!(reopened.epoch, ab.epoch);
        assert!(reopened.supersedes(&ab));
    }

    #[test]
    fn concurrent_writers_on_one_base_never_both_apply() {
        use std::sync::{Arc, Barrier};
        for _ in 0..200 {
            let doc = Arc::new(doc());
            doc.set_notes_text("base").unwrap();
            let base = doc.notes_snapshot().hash;
            let barrier = Arc::new(Barrier::new(2));
            let writers: Vec<_> = ["base + left", "right + base"]
                .into_iter()
                .map(|text| {
                    let (doc, base, barrier) = (doc.clone(), base.clone(), barrier.clone());
                    std::thread::spawn(move || {
                        barrier.wait();
                        (text, doc.write_notes(&base, text).unwrap())
                    })
                })
                .collect();
            let results: Vec<_> = writers.into_iter().map(|w| w.join().unwrap()).collect();
            let applied: Vec<_> = results
                .iter()
                .filter(|(_, write)| matches!(write, NotesWrite::Applied { .. }))
                .collect();
            assert_eq!(applied.len(), 1, "{results:?}");
            // The winner's text is exactly what the doc holds; the loser was
            // told so and shown it.
            assert_eq!(doc.notes_text(), applied[0].0);
            let lost = results
                .iter()
                .find(|(_, write)| matches!(write, NotesWrite::Conflict { .. }));
            match lost {
                Some((_, NotesWrite::Conflict { current })) => {
                    assert_eq!(current.text, applied[0].0)
                }
                other => panic!("expected a conflict, got {other:?}"),
            }
        }
    }

    #[test]
    fn write_caps_growth_but_allows_trimming() {
        let doc = doc();
        let big = "x".repeat(NOTES_MAX_BYTES + 1);
        let base = doc.notes_snapshot().hash;
        assert_eq!(
            doc.write_notes(&base, &big).unwrap(),
            NotesWrite::TooLarge {
                limit: NOTES_MAX_BYTES
            }
        );
        // Over the limit already (another device's paste): trimming works.
        doc.set_notes_text(&big).unwrap();
        let base = doc.notes_snapshot().hash;
        let trimmed = &big[..big.len() - 1];
        assert!(matches!(
            doc.write_notes(&base, trimmed).unwrap(),
            NotesWrite::Applied { .. }
        ));
    }

    #[test]
    fn writes_are_minimal_text_ops() {
        let doc = doc();
        doc.set_notes_text(&"line\n".repeat(1000)).unwrap();
        let before = doc.doc().oplog_vv();
        let mut text = doc.notes_text();
        text.insert_str(2500, "inserted");
        doc.set_notes_text(&text).unwrap();
        let delta = doc
            .doc()
            .export(loro::ExportMode::updates(&before))
            .unwrap();
        // One small insert, not a 5 KB rewrite.
        assert!(delta.len() < 200, "update was {} bytes", delta.len());
        assert!(!doc.set_notes_text(&text).unwrap(), "no-op write commits");
    }

    #[test]
    fn concurrent_edits_converge() {
        let a = doc();
        a.set_notes_text("shared line\n").unwrap();
        let b = peer(&a);
        a.set_notes_text("A: shared line\n").unwrap();
        b.set_notes_text("shared line\nB\n").unwrap();
        sync(&a, &b);
        assert_eq!(a.notes_text(), b.notes_text());
        assert_eq!(a.notes_text(), "A: shared line\nB\n");
    }

    #[test]
    fn notes_survive_snapshots() {
        let a = doc();
        a.set_notes_text("- [ ] ship notes\n").unwrap();
        let b = peer(&a);
        assert_eq!(b.notes_text(), "- [ ] ship notes\n");
        assert_eq!(b.notes_snapshot().hash, a.notes_snapshot().hash);
    }

    #[test]
    fn a_remote_batch_touching_both_classifies_as_both() {
        use std::sync::{Arc, Mutex};
        let doc = doc();
        doc.push_queued(&crate::QueuedMessage::new("q1", "later", "device"))
            .unwrap();
        let other = peer(&doc);
        let before = other.doc().oplog_vv();
        doc.set_notes_text("remote notes").unwrap();
        doc.remove_queued("q1").unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let _sub = other.doc().subscribe_root(Arc::new(move |event| {
            sink.lock().unwrap().push(NotesTouch::of(&event));
        }));
        other
            .doc()
            .import(
                &doc.doc()
                    .export(loro::ExportMode::updates(&before))
                    .unwrap(),
            )
            .unwrap();
        assert_eq!(
            *seen.lock().unwrap(),
            vec![NotesTouch {
                notes: true,
                other: true
            }]
        );
    }

    #[test]
    fn local_commits_classify() {
        use std::sync::{Arc, Mutex};
        let doc = doc();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let _sub = doc.doc().subscribe_root(Arc::new(move |event| {
            sink.lock().unwrap().push(NotesTouch::of(&event));
        }));
        doc.set_notes_text("only notes").unwrap();
        doc.push_queued(&crate::QueuedMessage::new("q1", "later", "device"))
            .unwrap();
        assert_eq!(
            *seen.lock().unwrap(),
            vec![
                NotesTouch {
                    notes: true,
                    other: false
                },
                NotesTouch {
                    notes: false,
                    other: true
                },
            ]
        );
    }

    #[test]
    fn notes_leave_the_transcript_alone() {
        let a = doc();
        a.set_notes_text("scratch").unwrap();
        assert!(a.read_entries().unwrap().is_empty());
        assert!(a.read_queue().unwrap().is_empty());
        assert!(a.read_commands().unwrap().is_empty());
    }
}
