//! Incremental transcript mirror over a session doc.
//!
//! The legacy app's #1 cost was re-decoding the WHOLE doc (and rebuilding
//! every row) per streamed token. Here a `subscribe_root` observer records
//! which `messages` entry maps a commit/import touched ([`Dirty`]), and
//! [`Tracker::refresh`] re-decodes only those maps (plus newly inserted ones
//! when the list itself changed). Per streamed token the work is one entry's
//! decode + one joined-entry rebuild; every other entry keeps its `Arc`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use loro::event::DiffEvent;
use loro::{Container, ContainerID, Index, LoroDoc, LoroMap, ToJson, ValueOrContainer};
use zeron_doc::{MessagePart, SessionMessageEntry};

use super::snapshot::{AppendHint, Entry};

/// What a batch of doc events touched.
#[derive(Debug, Default)]
pub(crate) struct Dirty {
    /// The `messages` list itself changed (entries inserted/removed).
    pub list: bool,
    /// Entry maps (by container id) whose content changed.
    pub entries: HashSet<ContainerID>,
    pub queue: bool,
    pub commands: bool,
    pub meta: bool,
}

impl Dirty {
    pub fn any(&self) -> bool {
        self.list || !self.entries.is_empty() || self.queue || self.commands || self.meta
    }

    /// Everything (first projection / resync).
    pub fn all() -> Self {
        Self {
            list: true,
            entries: HashSet::new(),
            queue: true,
            commands: true,
            meta: true,
        }
    }

    pub fn observe(&mut self, event: &DiffEvent<'_>) {
        for change in &event.events {
            let path = change.path;
            let Some((_, Index::Key(root))) = path.first() else {
                continue;
            };
            match root.as_str() {
                "messages" => match path.get(1) {
                    Some((entry, _)) => {
                        self.entries.insert(entry.clone());
                    }
                    None => self.list = true,
                },
                "queue" => self.queue = true,
                "commands" => self.commands = true,
                "meta" => self.meta = true,
                _ => {}
            }
        }
    }
}

struct Slot {
    cid: ContainerID,
    /// Bumped on every re-decode of this raw entry.
    raw_rev: u64,
    entry: Option<Arc<SessionMessageEntry>>,
}

/// Joined-transcript change produced by one refresh.
#[derive(Debug, Default)]
pub(crate) struct TranscriptChange {
    pub reset: bool,
    pub changed: Vec<String>,
    pub removed: Vec<String>,
}

pub(crate) struct Tracker {
    slots: Vec<Slot>,
    slot_index: HashMap<ContainerID, usize>,
    /// Raw entry id → occurrences (adoption checks for pending echoes).
    raw_ids: HashMap<String, u32>,
    joined: Vec<Arc<Entry>>,
    by_root: HashMap<ContainerID, (Vec<u64>, Arc<Entry>)>,
    next_rev: u64,
    initialized: bool,
    /// Raw entry decodes performed (perf tests assert O(changed)).
    pub decodes: u64,
}

fn decode(map: &LoroMap) -> Option<Arc<SessionMessageEntry>> {
    let value = map.get_deep_value().to_json_value();
    match zeron_doc::decode_entry_json(value) {
        Ok(entry) => Some(Arc::new(entry)),
        Err(err) => {
            tracing::debug!(error = %err, "transcript entry not decodable yet");
            None
        }
    }
}

impl Tracker {
    pub fn new() -> Self {
        Self {
            slots: Vec::new(),
            slot_index: HashMap::new(),
            raw_ids: HashMap::new(),
            joined: Vec::new(),
            by_root: HashMap::new(),
            next_rev: 0,
            initialized: false,
            decodes: 0,
        }
    }

    pub fn entries(&self) -> &[Arc<Entry>] {
        &self.joined
    }

    /// A host-written entry with this id exists (echo adoption).
    pub fn contains_id(&self, id: &str) -> bool {
        self.raw_ids.contains_key(id)
    }

    pub fn next_rev(&mut self) -> u64 {
        self.next_rev += 1;
        self.next_rev
    }

    fn note_id(&mut self, id: Option<&str>, delta: i32) {
        let Some(id) = id else { return };
        if delta > 0 {
            *self.raw_ids.entry(id.to_owned()).or_default() += 1;
        } else if let Some(count) = self.raw_ids.get_mut(id) {
            *count -= 1;
            if *count == 0 {
                self.raw_ids.remove(id);
            }
        }
    }

    fn decode_slot(&mut self, cid: ContainerID, map: &LoroMap) -> Slot {
        self.decodes += 1;
        let entry = decode(map);
        let raw_rev = self.next_rev();
        Slot {
            cid,
            raw_rev,
            entry,
        }
    }

    /// Apply the dirty set. `None` when the joined transcript did not change.
    pub fn refresh(&mut self, doc: &LoroDoc, dirty: &Dirty) -> Option<TranscriptChange> {
        let first = !self.initialized;
        if !first && !dirty.list && dirty.entries.is_empty() {
            return None;
        }
        self.initialized = true;
        let mut touched = first || dirty.list;
        if first || dirty.list {
            let list = doc.get_list("messages");
            let mut old: HashMap<ContainerID, Slot> = self
                .slots
                .drain(..)
                .map(|slot| (slot.cid.clone(), slot))
                .collect();
            let mut slots = Vec::with_capacity(list.len());
            for i in 0..list.len() {
                let Some(ValueOrContainer::Container(Container::Map(map))) = list.get(i) else {
                    continue;
                };
                let cid = map.id();
                let slot = match old.remove(&cid) {
                    Some(slot) if !dirty.entries.contains(&cid) => slot,
                    _ => self.decode_slot(cid, &map),
                };
                slots.push(slot);
            }
            self.slots = slots;
            self.slot_index = self
                .slots
                .iter()
                .enumerate()
                .map(|(i, s)| (s.cid.clone(), i))
                .collect();
            self.raw_ids.clear();
            let ids: Vec<String> = self
                .slots
                .iter()
                .filter_map(|s| s.entry.as_ref().map(|e| e.id.clone()))
                .collect();
            for id in ids {
                self.note_id(Some(&id), 1);
            }
        } else {
            for cid in &dirty.entries {
                let Some(&ix) = self.slot_index.get(cid) else {
                    continue;
                };
                let Some(Container::Map(map)) = doc.get_container(cid.clone()) else {
                    continue;
                };
                let slot = self.decode_slot(cid.clone(), &map);
                let old_id = self.slots[ix].entry.as_ref().map(|e| e.id.clone());
                let new_id = slot.entry.as_ref().map(|e| e.id.clone());
                if old_id != new_id {
                    self.note_id(old_id.as_deref(), -1);
                    self.note_id(new_id.as_deref(), 1);
                }
                self.slots[ix] = slot;
                touched = true;
            }
        }
        if !touched {
            return None;
        }
        Some(self.rejoin(first))
    }

    /// Regroup continuations onto their roots; reuse every joined entry whose
    /// constituent raw revisions are unchanged.
    fn rejoin(&mut self, reset: bool) -> TranscriptChange {
        let mut groups: Vec<Vec<usize>> = Vec::new();
        let mut root_group: HashMap<&str, usize> = HashMap::new();
        for (i, slot) in self.slots.iter().enumerate() {
            let Some(entry) = &slot.entry else { continue };
            match entry
                .continuation_of
                .as_deref()
                .and_then(|root| root_group.get(root))
            {
                Some(&group) => groups[group].push(i),
                None => {
                    if entry.continuation_of.is_none() {
                        root_group.insert(entry.id.as_str(), groups.len());
                    }
                    groups.push(vec![i]);
                }
            }
        }

        let mut joined = Vec::with_capacity(groups.len());
        let mut by_root = HashMap::with_capacity(groups.len());
        let mut changed = Vec::new();
        for members in groups {
            let root = &self.slots[members[0]];
            let sig: Vec<u64> = members.iter().map(|&i| self.slots[i].raw_rev).collect();
            let previous = self.by_root.get(&root.cid);
            let entry = match previous {
                Some((old_sig, entry)) if *old_sig == sig => entry.clone(),
                _ => {
                    let Some(root_entry) = root.entry.as_ref() else {
                        continue;
                    };
                    let mut message = (**root_entry).clone();
                    for &i in &members[1..] {
                        if let Some(part) = self.slots[i].entry.as_ref() {
                            message.parts.extend(part.parts.iter().cloned());
                            if part.duration_ms.is_some() {
                                message.duration_ms = part.duration_ms;
                            }
                        }
                    }
                    let previous = previous.map(|(_, e)| e.clone());
                    self.next_rev += 1;
                    let rev = self.next_rev;
                    let append = previous.as_deref().and_then(|p| append_hint(p, &message));
                    changed.push(message.id.clone());
                    Arc::new(Entry {
                        id: message.id.clone(),
                        rev,
                        message,
                        echo: None,
                        append,
                    })
                }
            };
            by_root.insert(root.cid.clone(), (sig, entry.clone()));
            joined.push(entry);
        }
        let current: HashSet<&str> = joined.iter().map(|e| e.id.as_str()).collect();
        let removed = self
            .joined
            .iter()
            .filter(|e| !current.contains(e.id.as_str()))
            .map(|e| e.id.clone())
            .collect();
        self.joined = joined;
        self.by_root = by_root;
        TranscriptChange {
            reset,
            changed,
            removed,
        }
    }
}

/// Text-only growth of exactly one part relative to `prev`.
fn append_hint(prev: &Entry, next: &SessionMessageEntry) -> Option<AppendHint> {
    let old = &prev.message;
    if old.id != next.id
        || old.role != next.role
        || old.created_at != next.created_at
        || old.device_id != next.device_id
        || old.status != next.status
        || old.duration_ms != next.duration_ms
        || old.parts.len() != next.parts.len()
    {
        return None;
    }
    let mut grown = None;
    for (ix, (a, b)) in old.parts.iter().zip(&next.parts).enumerate() {
        if a == b {
            continue;
        }
        let ok = match (a, b) {
            (MessagePart::Text { id: ia, text: ta }, MessagePart::Text { id: ib, text: tb })
            | (
                MessagePart::Reasoning { id: ia, text: ta },
                MessagePart::Reasoning { id: ib, text: tb },
            ) => ia == ib && tb.len() > ta.len() && tb.starts_with(ta.as_str()),
            _ => false,
        };
        if !ok || grown.is_some() {
            return None;
        }
        grown = Some(ix);
    }
    let part_index = grown?;
    let base_rev = match prev.append {
        Some(hint) if hint.part_index == part_index => hint.base_rev,
        _ => prev.rev,
    };
    Some(AppendHint {
        base_rev,
        part_index,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use zeron_doc::{MessageRole, MessageStatus, SegmentWriter, SessionDoc};

    use super::*;

    fn user(id: &str, text: &str) -> SessionMessageEntry {
        SessionMessageEntry {
            id: id.into(),
            role: MessageRole::User,
            parts: vec![MessagePart::Text {
                id: "t0".into(),
                text: text.into(),
            }],
            created_at: 1,
            device_id: "phone".into(),
            status: Some(MessageStatus::Complete),
            continuation_of: None,
            duration_ms: None,
        }
    }

    struct Rig {
        doc: SessionDoc,
        dirty: Arc<Mutex<Dirty>>,
        _sub: loro::Subscription,
        tracker: Tracker,
    }

    impl Rig {
        fn new() -> Self {
            let doc = SessionDoc::init("chat").unwrap();
            let dirty = Arc::new(Mutex::new(Dirty::default()));
            let sink = dirty.clone();
            let sub = doc.doc().subscribe_root(Arc::new(move |event| {
                sink.lock().unwrap().observe(&event);
            }));
            Self {
                doc,
                dirty,
                _sub: sub,
                tracker: Tracker::new(),
            }
        }

        fn refresh(&mut self) -> Option<TranscriptChange> {
            let dirty = std::mem::take(&mut *self.dirty.lock().unwrap());
            self.tracker.refresh(self.doc.doc(), &dirty)
        }
    }

    #[test]
    fn streaming_redecodes_only_the_live_entry() {
        let mut rig = Rig::new();
        for i in 0..50 {
            rig.doc.push_message(&user(&format!("u{i}"), "hello")).unwrap();
        }
        let first = rig.refresh().unwrap();
        assert!(first.reset);
        assert_eq!(rig.tracker.entries().len(), 50);
        let before: Vec<Arc<Entry>> = rig.tracker.entries().to_vec();
        let decodes = rig.tracker.decodes;

        let mut writer = SegmentWriter::begin(&rig.doc, "a1", "host", 2).unwrap();
        rig.refresh().unwrap();
        let mut text = String::new();
        for word in ["streaming ", "one ", "word ", "at ", "a ", "time"] {
            text.push_str(word);
            writer
                .sync(&[MessagePart::Text {
                    id: "p0".into(),
                    text: text.clone(),
                }])
                .unwrap();
            let change = rig.refresh().unwrap();
            assert_eq!(change.changed, vec!["a1".to_owned()]);
        }
        // One decode for the new entry + one per streamed token — never the
        // 50 settled entries again.
        assert_eq!(rig.tracker.decodes - decodes, 1 + 6);
        for (old, new) in before.iter().zip(rig.tracker.entries()) {
            assert!(Arc::ptr_eq(old, new));
        }
        let live = rig.tracker.entries().last().unwrap();
        let hint = live.append.expect("text growth is an append");
        assert_eq!(hint.part_index, 0);
    }

    #[test]
    fn continuations_join_onto_their_root() {
        let mut rig = Rig::new();
        rig.doc.push_message(&user("root", "a")).unwrap();
        let mut cont = user("c1", "b");
        cont.continuation_of = Some("root".into());
        rig.doc.push_message(&cont).unwrap();
        rig.refresh();
        let entries = rig.tracker.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].message.parts.len(), 2);
        assert!(rig.tracker.contains_id("c1"));
    }
}
