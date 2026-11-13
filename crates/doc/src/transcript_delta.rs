//! Delta frames for `WatchDocMessages`.
//!
//! The watch used to re-serialize the entire transcript on every 120ms commit
//! tick (measured at 1.13MB per frame on a 1.6MB chat, ~4 copies deep through
//! the RPC hop). A frame is now either a full `reset` (first frame, and the
//! fallback when a diff would approach transcript size) or the changed
//! entries only — during streaming that is one entry per tick.
//!
//! Both viewports share this module (the `zeron_proto::view` rule: derivations
//! that must not diverge per surface live in one place).

use serde::{Deserialize, Serialize};
use std::borrow::Borrow;
use std::collections::HashMap;

use crate::parts::MessagePart;
use crate::schema::SessionMessageEntry;

/// Transcript changes and the current host-owned context snapshot travel together.
/// Older readers ignore `contextUsage`; older hosts decode as unknown usage.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptUpdate {
    #[serde(flatten)]
    pub frame: TranscriptFrame,
    #[serde(default)]
    pub context_usage: Option<zeron_proto::ContextUsage>,
    /// Historical content included in this update, independent of reset/delta
    /// encoding. Omitted on ordinary live updates and by older engines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replay_baseline: Option<TranscriptBaseline>,
}

/// Compact presentation watermark: stable part ids and text byte lengths.
/// Keeping the cutoff (rather than a one-frame boolean) lets a viewport
/// coalesce historical and live updates without animating the history or
/// swallowing the first live arrival. No transcript text is duplicated.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptBaseline {
    pub entries: HashMap<String, HashMap<String, usize>>,
}

impl TranscriptBaseline {
    /// Fully historical entries can reuse the viewport's already parsed rows.
    pub fn covers(&self, entry: &SessionMessageEntry) -> bool {
        let Some(parts) = self.entries.get(&entry.id) else {
            return false;
        };
        entry.parts.iter().all(|part| {
            parts.get(part.id()).is_some_and(|&len| match part {
                MessagePart::Text { text, .. } | MessagePart::Reasoning { text, .. } => {
                    len >= text.len()
                }
                _ => true,
            })
        })
    }

    pub fn capture<E: Borrow<SessionMessageEntry>>(entries: &[E]) -> Self {
        Self {
            entries: entries
                .iter()
                .map(Borrow::borrow)
                .map(|entry: &SessionMessageEntry| {
                    let parts = entry
                        .parts
                        .iter()
                        .map(|part| {
                            let len = match part {
                                MessagePart::Text { text, .. }
                                | MessagePart::Reasoning { text, .. } => text.len(),
                                _ => 0,
                            };
                            (part.id().to_owned(), len)
                        })
                        .collect();
                    (entry.id.clone(), parts)
                })
                .collect(),
        }
    }

    /// Reconstruct just the historical prefix from the current content.
    /// Invalid/stale byte cutoffs never slice through a UTF-8 character.
    pub fn historical_entry(&self, entry: &SessionMessageEntry) -> Option<SessionMessageEntry> {
        let parts = self.entries.get(&entry.id)?;
        let mut historical = entry.clone();
        historical.parts.retain_mut(|part| {
            let Some(&len) = parts.get(part.id()) else {
                return false;
            };
            if let MessagePart::Text { text, .. } | MessagePart::Reasoning { text, .. } = part {
                let mut end = len.min(text.len());
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                text.truncate(end);
            }
            true
        });
        Some(historical)
    }
}

/// The part of a transcript a reader asked for (`WatchDocMessages {window}`):
/// the newest `limit` entries, skipping the newest `offset`, and with `after`
/// only those positioned after that entry. Frames then carry just this slice,
/// so what a reader pays doesn't grow with the chat. Older hosts ignore the
/// param and send everything; [`Self::apply`] gives a reader the same slice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptWindow {
    pub limit: usize,
    #[serde(default)]
    pub offset: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
}

/// What a window's reader can't see from the slice itself. Sent as `window`
/// beside the frame; its presence says the host applied the window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptWindowInfo {
    /// Entries in the whole transcript.
    pub total: usize,
    /// How many entries sit before the slice.
    pub start: usize,
    /// The newest unanswered question anywhere in the transcript.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_input: Option<PendingInput>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingInput {
    pub request_id: String,
    pub questions: Vec<zeron_proto::UserInputQuestion>,
}

impl TranscriptWindow {
    pub fn tail(limit: usize) -> Self {
        Self {
            limit,
            offset: 0,
            after: None,
        }
    }

    /// Entries after `id`: nothing until `id` lands in the transcript.
    pub fn after(id: impl Into<String>, limit: usize) -> Self {
        Self {
            limit,
            offset: 0,
            after: Some(id.into()),
        }
    }

    pub fn apply<'a, E: Borrow<SessionMessageEntry>>(
        &self,
        entries: &'a [E],
    ) -> (&'a [E], TranscriptWindowInfo) {
        let total = entries.len();
        let floor = self.after.as_ref().map_or(0, |id| {
            entries
                .iter()
                .rposition(|e| &e.borrow().id == id)
                .map_or(total, |at| at + 1)
        });
        let end = total.saturating_sub(self.offset).max(floor);
        let start = end.saturating_sub(self.limit).max(floor);
        (
            &entries[start..end],
            TranscriptWindowInfo {
                total,
                start,
                pending_input: pending_input(entries),
            },
        )
    }
}

fn pending_input<E: Borrow<SessionMessageEntry>>(entries: &[E]) -> Option<PendingInput> {
    entries.iter().rev().find_map(|entry| {
        entry.borrow().parts.iter().rev().find_map(|part| match part {
            MessagePart::Input {
                request_id,
                questions,
                resolved: false,
                ..
            } => Some(PendingInput {
                request_id: request_id.clone(),
                questions: questions.clone(),
            }),
            _ => None,
        })
    })
}

/// One `WatchDocMessages` stream item.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TranscriptFrame {
    Reset {
        reset: Vec<SessionMessageEntry>,
    },
    Delta {
        #[serde(default)]
        upsert: Vec<TranscriptUpsert>,
        #[serde(default)]
        append: Vec<TextAppend>,
        #[serde(default)]
        remove: Vec<String>,
        /// Parts of an entry replaced or added, the entry otherwise as it
        /// was: a tool call arriving or finishing in a long reply sends that
        /// call alone, not the reply. Only to a consumer that asked for them
        /// (`DiffOptions::part_patches`); never sent empty, so an older
        /// reader never sees the field.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        parts: Vec<PartPatch>,
        /// Expected transcript length after applying this frame — the desync
        /// tripwire: a consumer that lands elsewhere resubscribes for a reset.
        count: usize,
    },
}

/// One part of entry `entry` set at `at` (from 0): a part there replaced (the
/// same part, changed), or one added at its end (`at` its part count). `len`
/// is the entry's part count after the frame's patches (desync tripwire).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartPatch {
    pub entry: String,
    pub at: usize,
    pub part: MessagePart,
    pub len: usize,
}

/// What a consumer of `WatchDocMessages` takes beyond the original frames.
#[derive(Debug, Clone, Copy, Default)]
pub struct DiffOptions {
    /// Part patches (`TranscriptFrame::Delta::parts`) for an entry whose
    /// header and part order are unchanged, rather than the entry whole.
    pub part_patches: bool,
}

/// An inserted or replaced entry, positioned after `after` (`None` = head).
/// Anchors are prior upserts of the same frame or unchanged entries, so
/// applying upserts in frame order always finds them settled.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptUpsert {
    pub after: Option<String>,
    pub entry: SessionMessageEntry,
}

/// A pure text-tail append to one part — the streaming hot path. Entry-level
/// upserts re-send the whole live entry per tick, which for a long single
/// reply is the whole reply again (continuations re-join before the watch);
/// this carries only the new tokens.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextAppend {
    pub entry: String,
    pub part: String,
    pub text: String,
    /// Total text length of the part after the append (desync tripwire).
    pub len: usize,
}

impl TranscriptFrame {
    pub fn reset<E: Borrow<SessionMessageEntry>>(entries: &[E]) -> Self {
        Self::Reset {
            reset: entries.iter().map(|e| e.borrow().clone()).collect(),
        }
    }

    pub fn is_empty_delta(&self) -> bool {
        matches!(
            self,
            Self::Delta { upsert, append, remove, parts, .. }
                if upsert.is_empty() && append.is_empty() && remove.is_empty() && parts.is_empty()
        )
    }
}

/// When `next` is `prev` plus text appended to exactly one text-bodied part
/// (text or reasoning; same position, all other fields and parts identical),
/// the change is a [`TextAppend`]. Any other difference falls back to a full
/// upsert.
fn try_text_append(prev: &SessionMessageEntry, next: &SessionMessageEntry) -> Option<TextAppend> {
    if prev.id != next.id
        || prev.role != next.role
        || prev.created_at != next.created_at
        || prev.device_id != next.device_id
        || prev.status != next.status
        || prev.continuation_of != next.continuation_of
        || prev.parts.len() != next.parts.len()
    {
        return None;
    }
    let mut append = None;
    for (p, n) in prev.parts.iter().zip(&next.parts) {
        if p == n {
            continue;
        }
        let ((pid, pt), (nid, nt)) = match (p, n) {
            (MessagePart::Text { id: pid, text: pt }, MessagePart::Text { id: nid, text: nt })
            | (
                MessagePart::Reasoning { id: pid, text: pt },
                MessagePart::Reasoning { id: nid, text: nt },
            ) => ((pid, pt), (nid, nt)),
            _ => return None,
        };
        if pid != nid || !nt.starts_with(pt.as_str()) || append.is_some() {
            return None;
        }
        append = Some(TextAppend {
            entry: next.id.clone(),
            part: nid.clone(),
            text: nt[pt.len()..].to_string(),
            len: nt.len(),
        });
    }
    append
}

/// When `next` is `prev` with some of its parts changed in place (each the
/// same part, by id) and/or parts added at its end, its header the same: those
/// parts alone, as patches. `None` for any other change (a part removed or
/// moved, the status or duration changed): the entry is sent whole.
fn try_part_patches(prev: &SessionMessageEntry, next: &SessionMessageEntry) -> Option<Vec<PartPatch>> {
    if prev.id != next.id
        || prev.role != next.role
        || prev.created_at != next.created_at
        || prev.device_id != next.device_id
        || prev.status != next.status
        || prev.continuation_of != next.continuation_of
        || prev.duration_ms != next.duration_ms
        || next.parts.len() < prev.parts.len()
    {
        return None;
    }
    let mut patches = Vec::new();
    for (at, n) in next.parts.iter().enumerate() {
        match prev.parts.get(at) {
            Some(p) if p == n => {}
            Some(p) if p.id() != n.id() => return None,
            _ => patches.push(PartPatch {
                entry: next.id.clone(),
                at,
                part: n.clone(),
                len: next.parts.len(),
            }),
        }
    }
    Some(patches)
}

/// Diff two transcript states into a frame. An entry is upserted when it is
/// new, its content changed, or its predecessor changed (a Loro list merge can
/// interleave a remote entry mid-list). Falls back to `Reset` when the delta
/// would carry most of the transcript anyway.
pub fn diff_transcript<E: Borrow<SessionMessageEntry>>(prev: &[E], next: &[E]) -> TranscriptFrame {
    diff_transcript_with(prev, next, DiffOptions::default())
}

/// `diff_transcript`, with what the consumer takes beyond the original
/// frames (`options`).
pub fn diff_transcript_with<E: Borrow<SessionMessageEntry>>(
    prev: &[E],
    next: &[E],
    options: DiffOptions,
) -> TranscriptFrame {
    let prev: Vec<&SessionMessageEntry> = prev.iter().map(Borrow::borrow).collect();
    let next: Vec<&SessionMessageEntry> = next.iter().map(Borrow::borrow).collect();
    let prev_by_id: std::collections::HashMap<&str, (usize, &SessionMessageEntry)> = prev
        .iter()
        .enumerate()
        .map(|(i, e)| (e.id.as_str(), (i, *e)))
        .collect();
    let next_ids: std::collections::HashSet<&str> = next.iter().map(|e| e.id.as_str()).collect();

    let remove: Vec<String> = prev
        .iter()
        .filter(|e| !next_ids.contains(e.id.as_str()))
        .map(|e| e.id.clone())
        .collect();

    let mut upsert = Vec::new();
    let mut append = Vec::new();
    let mut parts = Vec::new();
    for (i, entry) in next.iter().copied().enumerate() {
        let after = i.checked_sub(1).map(|p| next[p].id.as_str());
        match prev_by_id.get(entry.id.as_str()) {
            Some((prev_ix, prev_entry)) => {
                let prev_after = prev_ix.checked_sub(1).map(|p| prev[p].id.as_str());
                // (The same shared entry is the same without looking.)
                let same = std::ptr::eq(*prev_entry, entry) || *prev_entry == entry;
                if same && prev_after == after {
                    continue;
                }
                if prev_after == after
                    && let Some(text_append) = try_text_append(prev_entry, entry)
                {
                    append.push(text_append);
                    continue;
                }
                if options.part_patches
                    && prev_after == after
                    && let Some(patches) = try_part_patches(prev_entry, entry)
                {
                    parts.extend(patches);
                    continue;
                }
                upsert.push(TranscriptUpsert {
                    after: after.map(str::to_string),
                    entry: entry.clone(),
                });
            }
            None => upsert.push(TranscriptUpsert {
                after: after.map(str::to_string),
                entry: entry.clone(),
            }),
        }
    }

    // A delta touching most rows serializes like a reset but applies slower.
    if upsert.len() * 2 >= next.len().max(1) && next.len() > 4 {
        return TranscriptFrame::reset(&next);
    }
    TranscriptFrame::Delta {
        upsert,
        append,
        remove,
        parts,
        count: next.len(),
    }
}

/// A digest of an entry as JSON, the same in every process and build (the
/// app keeps one for each entry it holds): FNV-1a over the value, an
/// object's fields in key order. Two equal values always agree; a different
/// one almost never does.
pub fn entry_sum(json: &serde_json::Value) -> u64 {
    struct Fnv(u64);
    impl Fnv {
        fn bytes(&mut self, bytes: &[u8]) {
            for byte in bytes {
                self.0 ^= u64::from(*byte);
                self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
            }
        }
        fn len(&mut self, len: usize) {
            self.bytes(&(len as u64).to_le_bytes());
        }
    }
    fn feed(json: &serde_json::Value, h: &mut Fnv) {
        use serde_json::Value;
        match json {
            Value::Null => h.bytes(&[0]),
            Value::Bool(b) => h.bytes(&[1, u8::from(*b)]),
            Value::Number(n) => {
                let n = n.to_string();
                h.bytes(&[2]);
                h.len(n.len());
                h.bytes(n.as_bytes());
            }
            Value::String(s) => {
                h.bytes(&[3]);
                h.len(s.len());
                h.bytes(s.as_bytes());
            }
            Value::Array(items) => {
                h.bytes(&[4]);
                h.len(items.len());
                items.iter().for_each(|item| feed(item, h));
            }
            Value::Object(fields) => {
                h.bytes(&[5]);
                h.len(fields.len());
                let mut keys: Vec<&String> = fields.keys().collect();
                keys.sort();
                for key in keys {
                    h.len(key.len());
                    h.bytes(key.as_bytes());
                    feed(&fields[key], h);
                }
            }
        }
    }
    let mut h = Fnv(0xcbf2_9ce4_8422_2325);
    feed(json, &mut h);
    h.0
}

/// A digest of the entries' order (their ids, in order), the same in the
/// app (`order_sum`): a reader that applied a `since` frame checks it holds
/// the rows in the engine's order, and asks for them whole if not.
pub fn order_sum<'a>(ids: impl IntoIterator<Item = &'a str>) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for id in ids {
        for byte in id.as_bytes().iter().chain(&[0u8]) {
            h ^= u64::from(*byte);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

/// [`entry_sum`] of an entry as it's sent.
pub fn entry_sum_of(entry: &SessionMessageEntry) -> u64 {
    serde_json::to_value(entry).map_or(0, |json| entry_sum(&json))
}

/// What a reader holding `known` (each entry's id and [`entry_sum`], in
/// order) needs to have `next`: a delta of the entries that differ (or
/// moved), not the transcript. A reset when that's most of it.
pub fn delta_from_known<E: Borrow<SessionMessageEntry>>(
    known: &[(String, u64)],
    next: &[E],
) -> TranscriptFrame {
    let next: Vec<&SessionMessageEntry> = next.iter().map(Borrow::borrow).collect();
    let known_at: HashMap<&str, (usize, u64)> = known
        .iter()
        .enumerate()
        .map(|(i, (id, sum))| (id.as_str(), (i, *sum)))
        .collect();
    let next_ids: std::collections::HashSet<&str> = next.iter().map(|e| e.id.as_str()).collect();
    // The entries both hold must stand in the same order (an upsert places
    // an entry after its new predecessor, and an entry whose predecessor is
    // unchanged is left where the reader has it). If not, it's sent whole.
    let both_known = known.iter().map(|(id, _)| id.as_str()).filter(|id| next_ids.contains(id));
    let both_next = next.iter().map(|e| e.id.as_str()).filter(|id| known_at.contains_key(id));
    if !both_known.eq(both_next) {
        return TranscriptFrame::reset(&next);
    }
    let remove: Vec<String> = known
        .iter()
        .filter(|(id, _)| !next_ids.contains(id.as_str()))
        .map(|(id, _)| id.clone())
        .collect();
    let mut upsert = Vec::new();
    for (i, entry) in next.iter().copied().enumerate() {
        let after = i.checked_sub(1).map(|p| next[p].id.as_str());
        if let Some((at, sum)) = known_at.get(entry.id.as_str()) {
            let known_after = at.checked_sub(1).map(|p| known[p].0.as_str());
            if known_after == after && *sum == entry_sum_of(entry) {
                continue;
            }
        }
        upsert.push(TranscriptUpsert { after: after.map(str::to_string), entry: entry.clone() });
    }
    if upsert.len() * 2 >= next.len().max(1) && next.len() > 4 {
        return TranscriptFrame::reset(&next);
    }
    TranscriptFrame::Delta { upsert, append: Vec::new(), remove, parts: Vec::new(), count: next.len() }
}

/// A frame that could not be applied cleanly — the consumer's copy has
/// diverged (skew, missed frame) and it should resubscribe for a reset.
#[derive(Debug, thiserror::Error)]
#[error("transcript delta desync: {0}")]
pub struct TranscriptDesync(pub String);

/// Apply a frame in place. On any error the state is unreliable — resubscribe.
pub fn apply_transcript_frame(
    current: &mut Vec<SessionMessageEntry>,
    frame: TranscriptFrame,
) -> Result<(), TranscriptDesync> {
    match frame {
        TranscriptFrame::Reset { reset } => {
            *current = reset;
            Ok(())
        }
        TranscriptFrame::Delta {
            upsert,
            append,
            remove,
            parts,
            count,
        } => {
            if !remove.is_empty() {
                let gone: std::collections::HashSet<&str> =
                    remove.iter().map(String::as_str).collect();
                current.retain(|e| !gone.contains(e.id.as_str()));
            }
            for TranscriptUpsert { after, entry } in upsert {
                if let Some(existing) = current.iter().position(|e| e.id == entry.id) {
                    current.remove(existing);
                }
                let at = match &after {
                    None => 0,
                    Some(anchor) => match current.iter().position(|e| &e.id == anchor) {
                        Some(ix) => ix + 1,
                        None => {
                            return Err(TranscriptDesync(format!("missing anchor {anchor}")));
                        }
                    },
                };
                current.insert(at, entry);
            }
            for TextAppend {
                entry,
                part,
                text,
                len,
            } in append
            {
                let Some(target) = current.iter_mut().find(|e| e.id == entry) else {
                    return Err(TranscriptDesync(format!("missing append entry {entry}")));
                };
                let tail = target.parts.iter_mut().find_map(|p| match p {
                    MessagePart::Text { id, text } | MessagePart::Reasoning { id, text }
                        if *id == part =>
                    {
                        Some(text)
                    }
                    _ => None,
                });
                let Some(tail) = tail else {
                    return Err(TranscriptDesync(format!("missing append part {part}")));
                };
                tail.push_str(&text);
                if tail.len() != len {
                    return Err(TranscriptDesync(format!(
                        "append length mismatch on {entry}#{part}: have {}, expected {len}",
                        tail.len()
                    )));
                }
            }
            for PartPatch {
                entry,
                at,
                part,
                len,
            } in parts
            {
                let Some(target) = current.iter_mut().find(|e| e.id == entry) else {
                    return Err(TranscriptDesync(format!("missing patch entry {entry}")));
                };
                let have = target.parts.len();
                if at < have && target.parts[at].id() == part.id() {
                    target.parts[at] = part;
                } else if at == have {
                    target.parts.push(part);
                } else {
                    return Err(TranscriptDesync(format!(
                        "patch out of place on {entry}#{at}"
                    )));
                }
                if target.parts.len() > len {
                    return Err(TranscriptDesync(format!(
                        "patch length mismatch on {entry}: have {}, expected {len}",
                        target.parts.len()
                    )));
                }
            }
            if current.len() != count {
                return Err(TranscriptDesync(format!(
                    "count mismatch: have {}, expected {count}",
                    current.len()
                )));
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parts::MessagePart;
    use crate::schema::MessageRole;

    fn entry(id: &str, text: &str) -> SessionMessageEntry {
        SessionMessageEntry {
            id: id.into(),
            role: MessageRole::Assistant,
            parts: vec![MessagePart::Text {
                id: "t0".into(),
                text: text.into(),
            }],
            created_at: 0,
            device_id: "dev".into(),
            status: None,
            continuation_of: None,
            duration_ms: None,
        }
    }

    fn apply(prev: &[SessionMessageEntry], next: &[SessionMessageEntry]) {
        let frame = diff_transcript(prev, next);
        // Round-trip through JSON: the wire shape must survive serde.
        let json = serde_json::to_value(&frame).unwrap();
        let frame: TranscriptFrame = serde_json::from_value(json).unwrap();
        let mut current = prev.to_vec();
        apply_transcript_frame(&mut current, frame).unwrap();
        assert_eq!(&current, next);
    }

    /// A reader that kept a transcript (as the JSON it was sent) and says
    /// what it holds gets only what changed, and lands on the transcript.
    #[test]
    fn a_reader_with_a_kept_copy_gets_what_changed() {
        let was: Vec<_> = (0..40).map(|i| entry(&format!("e{i}"), &format!("words {i}"))).collect();
        // As the reader holds it: the wire's JSON, its sums of that.
        let sent = serde_json::to_value(TranscriptFrame::reset(&was)).unwrap();
        let known: Vec<(String, u64)> = sent["reset"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| (e["id"].as_str().unwrap().to_owned(), entry_sum(e)))
            .collect();
        let mut now = was.clone();
        now[3] = entry("e3", "words 3, edited");
        now.remove(10);
        now.insert(20, entry("new-mid", "landed between"));
        now.push(entry("new-end", "the newest"));
        let frame = delta_from_known(&known, &now);
        let TranscriptFrame::Delta { upsert, remove, .. } = &frame else {
            panic!("expected a delta, got a reset");
        };
        let ids: Vec<&str> = upsert.iter().map(|u| u.entry.id.as_str()).collect();
        // (e11 is upserted: its predecessor changed.)
        assert_eq!(ids, ["e3", "e11", "new-mid", "e21", "new-end"]);
        assert_eq!(remove, &["e10".to_owned()]);
        let frame: TranscriptFrame =
            serde_json::from_value(serde_json::to_value(&frame).unwrap()).unwrap();
        let mut current = was.clone();
        apply_transcript_frame(&mut current, frame).unwrap();
        assert_eq!(current, now);
        // Nothing changed: an empty delta.
        assert!(delta_from_known(&known, &was).is_empty_delta());
        // Most of it unknown (another engine's sums): a reset, as before.
        let strangers: Vec<_> = known.iter().map(|(id, _)| (id.clone(), 7)).collect();
        assert!(matches!(delta_from_known(&strangers, &now), TranscriptFrame::Reset { .. }));
    }

    /// `entry_sum` of `{"id":"x","n":1.5,"p":[{"k":"t","v":null}]}`.
    const PINNED_SUM: u64 = 0xec67_0b99_244f_a276;

    /// Rows the reader holds in another order than the engine (an opening
    /// tail appended after older rows, a concurrent insert) get the whole
    /// copy, never a delta that would leave them out of order.
    #[test]
    fn a_reader_holding_rows_in_another_order_gets_them_whole() {
        let row = |id: &str| entry(id, id);
        let sums = |rows: &[SessionMessageEntry]| -> Vec<(String, u64)> {
            rows.iter().map(|e| (e.id.clone(), entry_sum_of(e))).collect()
        };
        // The review's case: the reader has [A,B,C,P,X], the engine [A,P,X,B,C].
        let held = [row("A"), row("B"), row("C"), row("P"), row("X")];
        let engine = [row("A"), row("P"), row("X"), row("B"), row("C")];
        assert!(matches!(delta_from_known(&sums(&held), &engine), TranscriptFrame::Reset { .. }));
        // Same order with an insert and a removal: still a delta.
        let held: Vec<_> = (0..20).map(|i| row(&format!("r{i}"))).collect();
        let mut engine = held.clone();
        engine.remove(7);
        engine.insert(3, row("N"));
        let frame = delta_from_known(&sums(&held), &engine);
        assert!(matches!(frame, TranscriptFrame::Delta { .. }));
        let mut current = held.clone();
        apply_transcript_frame(&mut current, frame).unwrap();
        assert_eq!(current, engine);
        assert_eq!(
            order_sum(current.iter().map(|e| e.id.as_str())),
            order_sum(engine.iter().map(|e| e.id.as_str()))
        );
        assert_ne!(order_sum(["A", "B"]), order_sum(["B", "A"]));
        assert_ne!(order_sum(["AB"]), order_sum(["A", "B"]));
        // Pinned: the app's `order_sum` gives the same.
        assert_eq!(order_sum(["a", "b"]), ORDER_PINNED);
    }

    const ORDER_PINNED: u64 = 0xab40_d782_0d40_8076;

    #[test]
    fn an_entry_sum_ignores_field_order() {
        let a: serde_json::Value = serde_json::from_str(r#"{"id":"x","n":1.5,"p":[{"k":"t","v":null}]}"#).unwrap();
        let b: serde_json::Value = serde_json::from_str(r#"{"p":[{"v":null,"k":"t"}],"n":1.5,"id":"x"}"#).unwrap();
        let c: serde_json::Value = serde_json::from_str(r#"{"p":[{"v":null,"k":"u"}],"n":1.5,"id":"x"}"#).unwrap();
        assert_eq!(entry_sum(&a), entry_sum(&b));
        assert_ne!(entry_sum(&a), entry_sum(&c));
        // Pinned: the app (apps/zgx `sum_of`) computes the same.
        assert_eq!(entry_sum(&a), PINNED_SUM);
    }

    #[test]
    fn append_and_edit_round_trip() {
        let a = entry("a", "hello");
        let b0 = entry("b", "wor");
        let b1 = entry("b", "world");
        apply(&[], &[a.clone()]);
        apply(&[a.clone()], &[a.clone(), b0.clone()]);
        apply(&[a.clone(), b0], &[a, b1]);
    }

    #[test]
    fn remove_and_mid_insert_round_trip() {
        let a = entry("a", "1");
        let b = entry("b", "2");
        let c = entry("c", "3");
        apply(&[a.clone(), b.clone(), c.clone()], &[a.clone(), c.clone()]);
        // Remote merge lands b between a and c.
        apply(&[a.clone(), c.clone()], &[a, b, c]);
    }

    #[test]
    fn streaming_tick_is_a_text_append() {
        let a = entry("a", "prompt");
        let b0 = entry("b", "streaming…");
        let b1 = entry("b", "streaming… more");
        let frame = diff_transcript(&[a.clone(), b0.clone()], &[a.clone(), b1.clone()]);
        match &frame {
            TranscriptFrame::Delta {
                upsert,
                append,
                remove,
                ..
            } => {
                // The hot path: only the new tokens travel, never the entry.
                assert!(upsert.is_empty());
                assert_eq!(append.len(), 1);
                assert_eq!(append[0].text, " more");
                assert!(remove.is_empty());
            }
            other => panic!("expected delta, got {other:?}"),
        }
        apply(&[a.clone(), b0], &[a, b1]);
    }

    #[test]
    fn streaming_reasoning_tick_is_a_text_append() {
        let mut b0 = entry("b", "prompt");
        b0.parts = vec![MessagePart::Reasoning {
            id: "r0".into(),
            text: "thinking".into(),
        }];
        let mut b1 = b0.clone();
        b1.parts = vec![MessagePart::Reasoning {
            id: "r0".into(),
            text: "thinking more".into(),
        }];
        let frame = diff_transcript(std::slice::from_ref(&b0), std::slice::from_ref(&b1));
        match &frame {
            TranscriptFrame::Delta { upsert, append, .. } => {
                assert!(upsert.is_empty(), "reasoning growth must ride the hot path");
                assert_eq!(append.len(), 1);
                assert_eq!(append[0].text, " more");
            }
            other => panic!("expected delta, got {other:?}"),
        }
        apply(&[b0], &[b1]);
    }

    #[test]
    fn non_append_change_falls_back_to_upsert() {
        // Same id but a rewritten (non-prefix) text must re-send the entry.
        let b0 = entry("b", "draft text");
        let b1 = entry("b", "final");
        let frame = diff_transcript(&[b0.clone()], &[b1.clone()]);
        match &frame {
            TranscriptFrame::Delta { upsert, append, .. } => {
                assert_eq!(upsert.len(), 1);
                assert!(append.is_empty());
            }
            other => panic!("expected delta, got {other:?}"),
        }
        apply(&[b0], &[b1]);
    }

    #[test]
    fn append_length_mismatch_is_desync() {
        let frame = TranscriptFrame::Delta {
            upsert: vec![],
            append: vec![TextAppend {
                entry: "a".into(),
                part: "t0".into(),
                text: "x".into(),
                len: 99,
            }],
            remove: vec![],
            parts: vec![],
            count: 1,
        };
        let mut current = vec![entry("a", "hello")];
        assert!(apply_transcript_frame(&mut current, frame).is_err());
    }

    fn tool(id: &str, output: Option<&str>) -> MessagePart {
        MessagePart::Tool {
            id: id.into(),
            call: serde_json::from_value(serde_json::json!({ "kind": "exec", "command": "ls" })).unwrap(),
            is_error: false,
            resolved: output.is_some(),
            output: output.map(Into::into),
            diff: None,
            output_ref: None,
            output_bytes: None,
            diff_ref: None,
            diff_stats: None,
            subagent_ref: None,
            subagent_status: None,
            subagent_tail: None,
            exit_code: None,
            output_truncated: false,
            output_lines: None,
        }
    }

    fn working(calls: &[MessagePart]) -> SessionMessageEntry {
        let mut e = entry("b", "");
        e.parts = calls.to_vec();
        e.status = Some(crate::parts::MessageStatus::Streaming);
        e
    }

    /// A call arriving and another finishing in a long reply: asked for,
    /// those two parts alone travel; not asked for, the entry whole, as ever.
    #[test]
    fn a_reply_s_changed_parts_travel_alone_when_asked_for() {
        let a = entry("a", "prompt");
        let calls: Vec<_> = (0..50).map(|i| tool(&format!("t{i}"), Some("ok"))).collect();
        let b0 = working(&[calls.clone(), vec![tool("t50", None)]].concat());
        let b1 = working(&[calls, vec![tool("t50", Some("done")), tool("t51", None)]].concat());
        let options = DiffOptions { part_patches: true };
        let frame = diff_transcript_with(&[a.clone(), b0.clone()], &[a.clone(), b1.clone()], options);
        match &frame {
            TranscriptFrame::Delta { upsert, parts, .. } => {
                assert!(upsert.is_empty(), "the entry isn't sent whole");
                let at: Vec<_> = parts.iter().map(|p| (p.at, p.part.id().to_owned())).collect();
                assert_eq!(at, vec![(50, "t50".into()), (51, "t51".into())]);
            }
            other => panic!("expected delta, got {other:?}"),
        }
        let json = serde_json::to_value(&frame).unwrap();
        let mut current = vec![a.clone(), b0.clone()];
        apply_transcript_frame(&mut current, serde_json::from_value(json).unwrap()).unwrap();
        assert_eq!(current, vec![a.clone(), b1.clone()]);
        // Not asked for: whole, and no `parts` field on the wire.
        let frame = diff_transcript(&[a.clone(), b0.clone()], &[a.clone(), b1.clone()]);
        let json = serde_json::to_value(&frame).unwrap();
        assert!(json.get("parts").is_none(), "{json}");
        assert_eq!(json["upsert"].as_array().map(Vec::len), Some(1));
        apply(&[a.clone(), b0], &[a, b1]);
    }

    /// A change parts can't say (the reply settling, a part going) is the
    /// entry whole even when they're asked for.
    #[test]
    fn other_changes_send_the_entry_whole() {
        let b0 = working(&[tool("t0", None), tool("t1", None)]);
        let mut settled = b0.clone();
        settled.status = Some(crate::parts::MessageStatus::Complete);
        let fewer = working(&[tool("t0", None)]);
        let moved = working(&[tool("t1", None), tool("t0", None)]);
        let options = DiffOptions { part_patches: true };
        for next in [settled, fewer, moved] {
            let frame = diff_transcript_with(std::slice::from_ref(&b0), std::slice::from_ref(&next), options);
            match &frame {
                TranscriptFrame::Delta { upsert, parts, .. } => {
                    assert_eq!(upsert.len(), 1);
                    assert!(parts.is_empty());
                }
                other => panic!("expected delta, got {other:?}"),
            }
        }
    }

    #[test]
    fn a_patch_out_of_place_is_desync() {
        let frame = TranscriptFrame::Delta {
            upsert: vec![],
            append: vec![],
            remove: vec![],
            parts: vec![PartPatch { entry: "b".into(), at: 5, part: tool("t5", None), len: 6 }],
            count: 1,
        };
        let mut current = vec![working(&[tool("t0", None)])];
        assert!(apply_transcript_frame(&mut current, frame).is_err());
    }

    #[test]
    fn large_change_falls_back_to_reset() {
        let prev: Vec<_> = (0..10).map(|i| entry(&format!("p{i}"), "x")).collect();
        let next: Vec<_> = (0..10).map(|i| entry(&format!("n{i}"), "y")).collect();
        assert!(matches!(
            diff_transcript(&prev, &next),
            TranscriptFrame::Reset { .. }
        ));
    }

    #[test]
    fn desync_surfaces_instead_of_corrupting() {
        let frame = TranscriptFrame::Delta {
            upsert: vec![TranscriptUpsert {
                after: Some("missing".into()),
                entry: entry("x", "1"),
            }],
            append: vec![],
            remove: vec![],
            parts: vec![],
            count: 2,
        };
        let mut current = Vec::new();
        assert!(apply_transcript_frame(&mut current, frame).is_err());
    }
}

#[cfg(test)]
mod context_update_tests {
    use super::*;
    #[test]
    fn usage_envelope_is_compatible_with_old_hosts_and_readers() {
        let old = serde_json::json!({"reset": []});
        let update: TranscriptUpdate = serde_json::from_value(old).unwrap();
        assert_eq!(update.context_usage, None);
        assert!(update.replay_baseline.is_none());
        let value = serde_json::to_value(TranscriptUpdate {
            frame: TranscriptFrame::reset::<SessionMessageEntry>(&[]),
            replay_baseline: Some(TranscriptBaseline::default()),
            context_usage: Some(zeron_proto::ContextUsage {
                tokens: Some(0),
                window: Some(200000),
            }),
        })
        .unwrap();
        assert_eq!(value["contextUsage"]["tokens"], 0);
        let roundtrip: TranscriptUpdate = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(
            roundtrip.replay_baseline,
            Some(TranscriptBaseline::default())
        );
        assert!(matches!(
            serde_json::from_value::<TranscriptFrame>(value).unwrap(),
            TranscriptFrame::Reset { .. }
        ));
    }

    #[test]
    fn replay_cutoff_keeps_only_historical_parts_and_unicode_text_prefixes() {
        let mut entry = SessionMessageEntry {
            id: "message".into(),
            role: crate::MessageRole::Assistant,
            parts: vec![MessagePart::Text {
                id: "text".into(),
                text: "café".into(),
            }],
            created_at: 0,
            device_id: "host".into(),
            status: Some(crate::MessageStatus::Streaming),
            continuation_of: None,
            duration_ms: None,
        };
        let baseline = TranscriptBaseline::capture(&[entry.clone()]);
        assert!(baseline.covers(&entry));
        entry.parts[0] = MessagePart::Text {
            id: "text".into(),
            text: "café recién hecho".into(),
        };
        entry.parts.push(MessagePart::Reasoning {
            id: "new".into(),
            text: "live".into(),
        });
        assert!(!baseline.covers(&entry));
        let historical = baseline.historical_entry(&entry).unwrap();
        assert_eq!(
            historical.parts,
            vec![MessagePart::Text {
                id: "text".into(),
                text: "café".into()
            }]
        );
        // A replaced text can invalidate a former byte boundary.
        entry.parts[0] = MessagePart::Text {
            id: "text".into(),
            text: "🙂🙂".into(),
        };
        assert_eq!(
            baseline.historical_entry(&entry).unwrap().parts[0],
            MessagePart::Text {
                id: "text".into(),
                text: "🙂".into()
            }
        );
    }

    fn window_entries(ids: &[&str]) -> Vec<SessionMessageEntry> {
        ids.iter()
            .map(|id| {
                serde_json::from_value(serde_json::json!({
                    "id": id, "role": "assistant", "createdAt": 1, "deviceId": "d", "parts": [],
                }))
                .unwrap()
            })
            .collect()
    }

    fn slice_ids(
        window: &TranscriptWindow,
        entries: &[SessionMessageEntry],
    ) -> (Vec<String>, usize) {
        let (slice, info) = window.apply(entries);
        assert_eq!(info.total, entries.len());
        (slice.iter().map(|e| e.id.clone()).collect(), info.start)
    }

    #[test]
    fn a_window_counts_from_the_newest_message() {
        let entries = window_entries(&["a", "b", "c", "d"]);
        assert_eq!(
            slice_ids(&TranscriptWindow::tail(1), &entries),
            (vec!["d".to_owned()], 3)
        );
        let paged = TranscriptWindow {
            limit: 2,
            offset: 1,
            after: None,
        };
        assert_eq!(
            slice_ids(&paged, &entries),
            (vec!["b".to_owned(), "c".to_owned()], 1)
        );
        // Past either end: empty or clamped, never a panic.
        let past = TranscriptWindow {
            limit: 2,
            offset: 9,
            after: None,
        };
        assert_eq!(slice_ids(&past, &entries), (vec![], 0));
        assert_eq!(slice_ids(&TranscriptWindow::tail(9), &entries).0.len(), 4);
    }

    #[test]
    fn replies_start_after_the_message_that_began_the_turn() {
        let entries = window_entries(&["old", "sent", "reply-1", "reply-2"]);
        let after = TranscriptWindow::after("sent", 10);
        assert_eq!(
            slice_ids(&after, &entries),
            (vec!["reply-1".to_owned(), "reply-2".to_owned()], 2)
        );
        // Newest kept when the turn outruns the limit.
        assert_eq!(
            slice_ids(&TranscriptWindow::after("sent", 1), &entries).0,
            ["reply-2"]
        );
        // Sent but not landed yet: nothing is a reply, however old the chat.
        assert_eq!(
            slice_ids(&TranscriptWindow::after("later", 10), &entries),
            (vec![], 4)
        );
        // Landed as the newest: no replies yet.
        assert!(
            slice_ids(&TranscriptWindow::after("reply-2", 10), &entries)
                .0
                .is_empty()
        );
    }

    #[test]
    fn a_window_finds_a_question_older_than_its_slice() {
        let mut entries = window_entries(&["asks", "after"]);
        entries[0].parts.push(MessagePart::Input {
            id: "p".into(),
            request_id: "r1".into(),
            questions: Vec::new(),
            resolved: false,
            answers: None,
        });
        let (slice, info) = TranscriptWindow::tail(1).apply(&entries);
        assert_eq!(slice[0].id, "after");
        assert_eq!(info.pending_input.unwrap().request_id, "r1");
        let MessagePart::Input { resolved, .. } = &mut entries[0].parts[0] else {
            unreachable!()
        };
        *resolved = true;
        assert!(
            TranscriptWindow::tail(1)
                .apply(&entries)
                .1
                .pending_input
                .is_none()
        );
    }
}
