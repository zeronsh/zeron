//! Synced composer draft: one tiny Loro doc holding a single `LoroText`.
//!
//! A draft is edited live from several devices (and several windows on one device), so the
//! text lives in a `LoroText` — concurrent typing merges instead of last-writer-wins. The doc is
//! deliberately separate from the session doc: its history is thrown away with the room when the
//! draft is sent, so typing never bloats a transcript.
//!
//! This module is pure (no I/O, no UI). The engine hosts one replica per chat and syncs it
//! through a draft room; a composer window holds its own replica and exchanges Loro updates with
//! the engine, so caret preservation and merging are done by Loro itself rather than by
//! hand-rolled transforms. Offsets in this API are **UTF-8 byte offsets**, matching the
//! composer's text input.

use std::sync::Arc;

use loro::cursor::Side;
use loro::{ExportMode, LoroDoc, LoroError, Subscription, VersionVector};
use thiserror::Error;

/// Name of the root text container.
const TEXT: &str = "text";

#[derive(Debug, Error)]
pub enum DraftError {
    #[error("draft doc: {0}")]
    Loro(#[from] LoroError),
    #[error("draft doc export: {0}")]
    Export(String),
}

/// One replica of a draft.
pub struct DraftDoc {
    doc: LoroDoc,
}

impl Default for DraftDoc {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for DraftDoc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DraftDoc")
            .field("bytes", &self.text().len())
            .finish()
    }
}

impl DraftDoc {
    /// An empty draft with a fresh random peer id.
    pub fn new() -> Self {
        Self {
            doc: LoroDoc::new(),
        }
    }

    /// Rebuild a replica from [`DraftDoc::snapshot`] (or any update) bytes.
    pub fn from_snapshot(bytes: &[u8]) -> Result<Self, DraftError> {
        let draft = Self::new();
        draft.doc.import(bytes)?;
        Ok(draft)
    }

    /// The current draft text.
    pub fn text(&self) -> String {
        self.doc.get_text(TEXT).to_string()
    }

    pub fn is_empty(&self) -> bool {
        self.doc.get_text(TEXT).is_empty()
    }

    /// Full state, for checkpoints and for seeding a new replica.
    pub fn snapshot(&self) -> Vec<u8> {
        self.doc.export(ExportMode::Snapshot).unwrap_or_default()
    }

    pub fn version(&self) -> VersionVector {
        self.doc.oplog_vv()
    }

    /// Everything a peer at `since` is missing.
    pub fn export_since(&self, since: &VersionVector) -> Result<Vec<u8>, DraftError> {
        self.doc
            .export(ExportMode::updates(since))
            .map_err(|error| DraftError::Export(error.to_string()))
    }

    /// Replace the draft with `new_text` as a single contiguous splice (common prefix and suffix
    /// are kept, which is exactly how typing changes text) and commit. Returns whether anything
    /// changed. The commit fires [`DraftDoc::subscribe_local_update`] callbacks.
    pub fn set_text(&self, new_text: &str) -> Result<bool, DraftError> {
        let old = self.text();
        let Some((start, delete, insert)) = diff_splice(&old, new_text) else {
            return Ok(false);
        };
        let text = self.doc.get_text(TEXT);
        if delete > 0 {
            text.delete_utf8(start, delete)?;
        }
        if !insert.is_empty() {
            text.insert_utf8(start, insert)?;
        }
        self.doc.commit();
        Ok(true)
    }

    /// Apply `bytes` from a peer. Returns the new text when the import changed it.
    pub fn import(&self, bytes: &[u8]) -> Result<Option<String>, DraftError> {
        self.import_tracking(bytes, &mut [])
    }

    /// [`DraftDoc::import`] that also carries byte `offsets` (a caret, a selection edge) through
    /// the import using Loro cursors, so a remote edit before the caret shifts it and one after
    /// leaves it alone. Offsets are updated in place; the returned text is `Some` when changed.
    pub fn import_tracking(
        &self,
        bytes: &[u8],
        offsets: &mut [usize],
    ) -> Result<Option<String>, DraftError> {
        let text = self.doc.get_text(TEXT);
        let before = text.to_string();
        let cursors: Vec<_> = offsets
            .iter()
            .map(|&offset| {
                let index = byte_to_unicode(&before, offset);
                text.get_cursor(index, Side::Left)
            })
            .collect();
        self.doc.import(bytes)?;
        let after = text.to_string();
        if after == before {
            return Ok(None);
        }
        for (offset, cursor) in offsets.iter_mut().zip(cursors) {
            let index = cursor
                .and_then(|cursor| self.doc.get_cursor_pos(&cursor).ok())
                .map(|found| found.current.pos)
                .unwrap_or_else(|| byte_to_unicode(&after, *offset));
            *offset = unicode_to_byte(&after, index);
        }
        Ok(Some(after))
    }

    /// Called with the encoded update of every local commit (typing, [`DraftDoc::set_text`]).
    /// This is what gets pushed to the room or to the engine.
    pub fn subscribe_local_update(
        &self,
        callback: impl Fn(&[u8]) + Send + Sync + 'static,
    ) -> Subscription {
        self.doc.subscribe_local_update(Box::new(move |bytes| {
            callback(bytes);
            true
        }))
    }
}

/// Shared handle used by hosts that need `DraftDoc` behind an `Arc`.
pub type SharedDraftDoc = Arc<DraftDoc>;

/// Single-splice diff: `(start, delete_len, insert)` in UTF-8 bytes, `None` when equal.
pub fn diff_splice<'a>(old: &str, new: &'a str) -> Option<(usize, usize, &'a str)> {
    if old == new {
        return None;
    }
    let mut prefix = old
        .bytes()
        .zip(new.bytes())
        .take_while(|(a, b)| a == b)
        .count();
    while !old.is_char_boundary(prefix) || !new.is_char_boundary(prefix) {
        prefix -= 1;
    }
    let max_suffix = old.len().min(new.len()) - prefix;
    let mut suffix = old
        .bytes()
        .rev()
        .zip(new.bytes().rev())
        .take(max_suffix)
        .take_while(|(a, b)| a == b)
        .count();
    while !old.is_char_boundary(old.len() - suffix) || !new.is_char_boundary(new.len() - suffix) {
        suffix -= 1;
    }
    Some((
        prefix,
        old.len() - prefix - suffix,
        &new[prefix..new.len() - suffix],
    ))
}

fn byte_to_unicode(text: &str, byte: usize) -> usize {
    let mut byte = byte.min(text.len());
    while !text.is_char_boundary(byte) {
        byte -= 1;
    }
    text[..byte].chars().count()
}

fn unicode_to_byte(text: &str, index: usize) -> usize {
    text.char_indices()
        .nth(index)
        .map(|(byte, _)| byte)
        .unwrap_or(text.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Two replicas wired through their local-update streams, like two devices on a room.
    fn pair() -> (DraftDoc, DraftDoc) {
        (DraftDoc::new(), DraftDoc::new())
    }

    fn sync(from: &DraftDoc, to: &DraftDoc) {
        let bytes = from.export_since(&to.version()).unwrap();
        to.import(&bytes).unwrap();
    }

    #[test]
    fn diff_splice_finds_the_typed_region() {
        assert_eq!(diff_splice("abc", "abc"), None);
        assert_eq!(diff_splice("", "hi"), Some((0, 0, "hi")));
        assert_eq!(diff_splice("hi", ""), Some((0, 2, "")));
        assert_eq!(diff_splice("hello", "helXlo"), Some((3, 0, "X")));
        assert_eq!(diff_splice("hello", "helo"), Some((3, 1, "")));
        assert_eq!(diff_splice("aXb", "aYb"), Some((1, 1, "Y")));
        // Repeated characters: prefix and suffix must not overlap.
        assert_eq!(diff_splice("aa", "aaa"), Some((2, 0, "a")));
        assert_eq!(diff_splice("aaa", "aa"), Some((2, 1, "")));
    }

    #[test]
    fn diff_splice_respects_char_boundaries() {
        // 'é' (2 bytes) → 'è' shares its first byte's prefix range only by luck of encoding.
        let (start, delete, insert) = diff_splice("café!", "cafè!").unwrap();
        assert_eq!((start, delete, insert), (3, 2, "è"));
        let (start, delete, insert) = diff_splice("a😀b", "a😃b").unwrap();
        assert_eq!((start, delete, insert), (1, 4, "😃"));
        let mut applied = String::from("a😀b");
        applied.replace_range(start..start + delete, insert);
        assert_eq!(applied, "a😃b");
    }

    #[test]
    fn set_text_round_trips_through_updates() {
        let (a, b) = pair();
        assert!(a.set_text("hello world").unwrap());
        assert!(!a.set_text("hello world").unwrap());
        sync(&a, &b);
        assert_eq!(b.text(), "hello world");
        b.set_text("hello brave world").unwrap();
        sync(&b, &a);
        assert_eq!(a.text(), "hello brave world");
    }

    #[test]
    fn concurrent_typing_from_two_devices_merges() {
        let (a, b) = pair();
        a.set_text("Hello world").unwrap();
        sync(&a, &b);
        // Both type at once, without seeing each other.
        a.set_text("Hello, world").unwrap();
        b.set_text("Hello world!").unwrap();
        sync(&a, &b);
        sync(&b, &a);
        assert_eq!(a.text(), b.text());
        assert_eq!(a.text(), "Hello, world!");
    }

    #[test]
    fn concurrent_inserts_at_the_same_spot_both_survive() {
        let (a, b) = pair();
        a.set_text("ab").unwrap();
        sync(&a, &b);
        a.set_text("aXb").unwrap();
        b.set_text("aYb").unwrap();
        sync(&a, &b);
        sync(&b, &a);
        assert_eq!(a.text(), b.text());
        assert!(a.text().contains('X') && a.text().contains('Y'));
        assert_eq!(a.text().len(), 4);
    }

    #[test]
    fn snapshot_seeds_a_new_replica() {
        let a = DraftDoc::new();
        a.set_text("draft — with ünïcode 😀").unwrap();
        let b = DraftDoc::from_snapshot(&a.snapshot()).unwrap();
        assert_eq!(b.text(), "draft — with ünïcode 😀");
        assert!(!b.is_empty());
        assert!(DraftDoc::new().is_empty());
    }

    #[test]
    fn local_updates_are_emitted_once_per_commit() {
        let doc = DraftDoc::new();
        let seen = Arc::new(Mutex::new(Vec::<Vec<u8>>::new()));
        let sink = seen.clone();
        let _sub =
            doc.subscribe_local_update(move |bytes| sink.lock().unwrap().push(bytes.to_vec()));
        doc.set_text("a").unwrap();
        doc.set_text("ab").unwrap();
        doc.set_text("ab").unwrap();
        let updates = seen.lock().unwrap().clone();
        assert_eq!(updates.len(), 2);
        // Replaying the emitted updates rebuilds the draft elsewhere.
        let replica = DraftDoc::new();
        for bytes in &updates {
            replica.import(bytes).unwrap();
        }
        assert_eq!(replica.text(), "ab");
    }

    #[test]
    fn caret_moves_with_remote_edits_before_it_and_holds_for_edits_after() {
        let (mine, theirs) = pair();
        mine.set_text("hello world").unwrap();
        sync(&mine, &theirs);

        // Caret after "hello" (byte 5).
        let mut offsets = [5usize];
        theirs.set_text("well, hello world").unwrap(); // inserts "well, " at 0
        let update = theirs.export_since(&mine.version()).unwrap();
        let changed = mine.import_tracking(&update, &mut offsets).unwrap();
        assert_eq!(changed.as_deref(), Some("well, hello world"));
        assert_eq!(offsets[0], 11, "caret stays after 'hello'");

        // A remote edit after the caret leaves it alone.
        theirs.set_text("well, hello world!!").unwrap();
        let update = theirs.export_since(&mine.version()).unwrap();
        mine.import_tracking(&update, &mut offsets).unwrap();
        assert_eq!(offsets[0], 11);
    }

    #[test]
    fn caret_handles_multibyte_text_and_deleted_regions() {
        let (mine, theirs) = pair();
        mine.set_text("héllo 😀 wörld").unwrap();
        sync(&mine, &theirs);
        let world = "héllo 😀 ".len();
        let mut offsets = [world];
        // Remote deletes the emoji before the caret.
        theirs.set_text("héllo  wörld").unwrap();
        let update = theirs.export_since(&mine.version()).unwrap();
        mine.import_tracking(&update, &mut offsets).unwrap();
        assert_eq!(&mine.text()[offsets[0]..], "wörld");

        // Remote deletes the text the caret sits in: it collapses onto a valid boundary.
        let mut offsets = [mine.text().find("wörld").unwrap() + 2];
        theirs.set_text("héllo ").unwrap();
        let update = theirs.export_since(&mine.version()).unwrap();
        mine.import_tracking(&update, &mut offsets).unwrap();
        assert!(mine.text().is_char_boundary(offsets[0]));
        assert!(offsets[0] <= mine.text().len());
    }

    #[test]
    fn importing_nothing_new_reports_no_change_and_keeps_offsets() {
        let (a, b) = pair();
        a.set_text("same").unwrap();
        sync(&a, &b);
        let mut offsets = [2usize];
        let update = a.export_since(&VersionVector::default()).unwrap();
        let changed = b.import_tracking(&update, &mut offsets).unwrap();
        assert_eq!(changed, None);
        assert_eq!(offsets, [2]);
    }

    #[test]
    fn an_emptied_draft_is_empty_and_syncs_as_empty() {
        let (a, b) = pair();
        a.set_text("something").unwrap();
        sync(&a, &b);
        a.set_text("").unwrap();
        sync(&a, &b);
        assert!(b.is_empty());
    }
}
