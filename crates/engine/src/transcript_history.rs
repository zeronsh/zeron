//! Presentation provenance from the containers changed by an import. Never
//! materialize the transcript on the per-row sync path.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use loro::{Container, ContainerID, EventTriggerKind, Index, LoroDoc, LoroValue, ValueOrContainer};
use zeron_doc::{SessionMessageEntry, TranscriptBaseline};

pub(crate) const REPLAY_ORIGIN: &str = "zeron:transcript-replay";

#[derive(Default)]
struct Part {
    historical_len: Option<usize>,
    live: bool,
}

#[derive(Default)]
pub(crate) struct TranscriptHistory {
    parts: HashMap<ContainerID, Part>,
    // Resolve stable message/part ids at publication, including torn imports
    // whose ids arrive after their content. No text or tool output is copied.
    owners: HashMap<ContainerID, ContainerID>,
    dirty: bool,
    baseline: Arc<TranscriptBaseline>,
    #[cfg(test)]
    pub(crate) inspected_parts: usize,
}

fn string(map: &loro::LoroMap, key: &str) -> Option<String> {
    match map.get(key)? {
        ValueOrContainer::Value(LoroValue::String(value)) => Some(value.to_string()),
        _ => None,
    }
}

impl TranscriptHistory {
    pub(crate) fn observe(&mut self, doc: &LoroDoc, event: &loro::event::DiffEvent<'_>) {
        let replay =
            event.triggered_by == EventTriggerKind::Import && event.origin == REPLAY_ORIGIN;
        let mut changed = HashSet::new();
        for change in &event.events {
            let path = change.path;
            if !matches!(path.first(), Some((_, Index::Key(key))) if key.as_str() == "messages") {
                continue;
            }
            // Message insertion/deletion or late scalar fields can affect the
            // ids/continuation mapping, but must not reclassify existing parts.
            self.dirty = true;
            if path.len() >= 4 && matches!(&path[2].1, Index::Key(key) if key.as_str() == "parts") {
                changed.insert((path[1].0.clone(), path[3].0.clone()));
            }
        }
        for (entry, id) in changed {
            #[cfg(test)]
            {
                self.inspected_parts += 1;
            }
            let Some(Container::Map(map)) = doc.get_container(id.clone()) else {
                continue;
            };
            let part = self.parts.entry(id.clone()).or_default();
            self.owners.insert(id, entry);
            if !replay {
                // Once live bytes have arrived, a later historical tool update
                // or text append must not consume their entrance animation.
                part.live = true;
            } else if !part.live {
                let field = match string(&map, "kind").as_deref() {
                    Some("text") => Some("text"),
                    Some("reasoning") => Some("reasoning"),
                    _ => None,
                };
                let len = field
                    .and_then(|field| map.get(field))
                    .map_or(0, |value| match value {
                        ValueOrContainer::Container(Container::Text(text)) => text.len_utf8(),
                        ValueOrContainer::Value(LoroValue::String(text)) => text.len(),
                        _ => 0,
                    });
                part.historical_len = Some(len);
            }
        }
    }

    pub(crate) fn snapshot(
        &mut self,
        doc: &LoroDoc,
        entries: &[SessionMessageEntry],
    ) -> Arc<TranscriptBaseline> {
        if !self.dirty {
            return self.baseline.clone();
        }
        self.dirty = false;
        let mut roots = HashSet::new();
        let mut joined_ids = HashMap::new();
        for entry in entries {
            let joined = match &entry.continuation_of {
                Some(root) if roots.contains(root) => root,
                _ => &entry.id,
            };
            joined_ids.insert(entry.id.as_str(), joined.as_str());
            if entry.continuation_of.is_none() {
                roots.insert(entry.id.clone());
            }
        }
        let mut next = TranscriptBaseline::default();
        for (id, part) in &self.parts {
            let Some(len) = part.historical_len else {
                continue;
            };
            let Some(Container::Map(map)) = doc.get_container(id.clone()) else {
                continue;
            };
            let Some(Container::Map(entry)) = doc.get_container(self.owners[id].clone()) else {
                continue;
            };
            let (Some(entry_id), Some(part_id)) = (string(&entry, "id"), string(&map, "id")) else {
                continue;
            };
            let Some(&joined) = joined_ids.get(entry_id.as_str()) else {
                continue;
            };
            next.entries
                .entry(joined.to_owned())
                .or_default()
                .insert(part_id, len);
        }
        if *self.baseline != next {
            self.baseline = Arc::new(next);
        }
        self.baseline.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use zeron_doc::{MessagePart, MessageRole, MessageStatus, SegmentWriter, SessionDoc};

    fn entry(id: &str) -> SessionMessageEntry {
        SessionMessageEntry {
            id: id.into(),
            role: MessageRole::Assistant,
            parts: vec![MessagePart::Text {
                id: "text".into(),
                text: "café".into(),
            }],
            created_at: 0,
            device_id: "host".into(),
            status: Some(MessageStatus::Streaming),
            continuation_of: None,
            duration_ms: None,
        }
    }

    fn observe(doc: &Arc<SessionDoc>) -> (Arc<Mutex<TranscriptHistory>>, loro::Subscription) {
        let history = Arc::new(Mutex::new(TranscriptHistory::default()));
        let tracker = history.clone();
        let weak = Arc::downgrade(doc);
        let sub = doc.doc().subscribe_root(Arc::new(move |event| {
            tracker
                .lock()
                .unwrap()
                .observe(weak.upgrade().unwrap().doc(), &event);
        }));
        (history, sub)
    }

    fn snapshot(doc: &SessionDoc, history: &Mutex<TranscriptHistory>) -> Arc<TranscriptBaseline> {
        history
            .lock()
            .unwrap()
            .snapshot(doc.doc(), &doc.read_entries().unwrap())
    }

    #[test]
    fn metadata_replay_does_not_consume_local_or_remote_live_parts() {
        let source = SessionDoc::init("chat").unwrap();
        let doc = Arc::new(SessionDoc::from_doc(loro::LoroDoc::new()));
        doc.doc()
            .import(&source.export_snapshot().unwrap())
            .unwrap();
        let (history, _sub) = observe(&doc);
        doc.push_message(&entry("local-live")).unwrap();
        source.push_message(&entry("remote-live")).unwrap();
        doc.doc()
            .import(&source.export_snapshot().unwrap())
            .unwrap();
        let before = snapshot(&doc, &history);
        let inspected = history.lock().unwrap().inspected_parts;
        source.update_context_usage(Some(10), Some(100)).unwrap();
        doc.doc()
            .import_with(&source.export_snapshot().unwrap(), REPLAY_ORIGIN)
            .unwrap();
        let after = snapshot(&doc, &history);
        assert!(
            Arc::ptr_eq(&before, &after),
            "metadata must not replace the watermark"
        );
        assert!(after.entries.is_empty(), "neither live entry is historical");
        assert_eq!(
            history.lock().unwrap().inspected_parts,
            inspected,
            "metadata must inspect zero transcript parts"
        );
    }

    #[test]
    fn interleaved_replay_preserves_live_prefix_and_joins_continuations() {
        let source = SessionDoc::init("chat").unwrap();
        let doc = Arc::new(SessionDoc::from_doc(loro::LoroDoc::new()));
        let (history, _sub) = observe(&doc);
        let mut writer = SegmentWriter::begin(&source, "root", "host", 0).unwrap();
        let old = MessagePart::Text {
            id: "old".into(),
            text: "histórico".into(),
        };
        writer.sync(&[old.clone()]).unwrap();
        doc.doc()
            .import_with(&source.export_snapshot().unwrap(), REPLAY_ORIGIN)
            .unwrap();
        let live = MessagePart::Text {
            id: "live".into(),
            text: "nuevo".into(),
        };
        writer.sync(&[old.clone(), live.clone()]).unwrap();
        doc.doc()
            .import(&source.export_snapshot().unwrap())
            .unwrap();
        let away = MessagePart::Text {
            id: "away".into(),
            text: "recuperado".into(),
        };
        writer
            .sync(&[old.clone(), live.clone(), away.clone()])
            .unwrap();
        doc.doc()
            .import_with(&source.export_snapshot().unwrap(), REPLAY_ORIGIN)
            .unwrap();
        let mut continuation = entry("continuation");
        continuation.continuation_of = Some("root".into());
        source.push_message(&continuation).unwrap();
        doc.doc()
            .import_with(&source.export_snapshot().unwrap(), REPLAY_ORIGIN)
            .unwrap();
        let baseline = snapshot(&doc, &history);
        let parts = &baseline.entries["root"];
        assert_eq!(parts["old"], "histórico".len());
        assert_eq!(parts["away"], "recuperado".len());
        assert_eq!(parts["text"], "café".len());
        assert!(!parts.contains_key("live"));
        assert!(!baseline.entries.contains_key("continuation"));
        // Later replay of the same live part must not cancel its animation.
        let mut longer_live = live;
        if let MessagePart::Text { text, .. } = &mut longer_live {
            text.push_str(" todavía");
        }
        writer.sync(&[old, longer_live, away]).unwrap();
        doc.doc()
            .import_with(&source.export_snapshot().unwrap(), REPLAY_ORIGIN)
            .unwrap();
        assert!(!snapshot(&doc, &history).entries["root"].contains_key("live"));
    }

    #[test]
    fn replay_work_scales_with_changed_parts_not_transcript_size() {
        let source = SessionDoc::init("chat").unwrap();
        for ix in 0..1000 {
            source.push_message(&entry(&format!("old-{ix}"))).unwrap();
        }
        let doc = Arc::new(SessionDoc::from_doc(loro::LoroDoc::new()));
        doc.doc()
            .import(&source.export_snapshot().unwrap())
            .unwrap();
        let (history, _sub) = observe(&doc);
        for ix in 0..20 {
            let version = source.doc().oplog_vv();
            source
                .push_message(&entry(&format!("recovered-{ix}")))
                .unwrap();
            let bytes = source
                .doc()
                .export(loro::ExportMode::updates(&version))
                .unwrap();
            doc.doc().import_with(&bytes, REPLAY_ORIGIN).unwrap();
        }
        assert_eq!(history.lock().unwrap().inspected_parts, 20);
        assert_eq!(snapshot(&doc, &history).entries.len(), 20);
    }
}
