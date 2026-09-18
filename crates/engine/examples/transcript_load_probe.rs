//! Read-only profiler for exported local snapshots. Prints timings, not content.
use std::time::Instant;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    for path in std::env::args().skip(1) {
        let started = Instant::now();
        let bytes = std::fs::read(&path)?;
        let read = started.elapsed();
        let doc = loro::LoroDoc::new();
        let t = Instant::now();
        doc.import(&bytes)?;
        let import = t.elapsed();
        let doc = zeron_doc::SessionDoc::from_doc(doc);
        let t = Instant::now();
        let tail = doc.read_opening_tail(128)?;
        let tail_materialize = t.elapsed();
        let t = Instant::now();
        let preview = zeron_doc::TranscriptUpdate {
            frame: zeron_doc::TranscriptFrame::reset(&tail),
            context_usage: doc.context_usage(),
            replay_baseline: Some(zeron_doc::TranscriptBaseline::capture(&tail)),
        };
        let tail_wire = serde_json::to_vec(&serde_json::to_value(preview)?)?;
        let _: zeron_doc::TranscriptUpdate =
            serde_json::from_value(serde_json::from_slice(&tail_wire)?)?;
        println!(
            "opening tail parts={} wire_bytes={} materialize={tail_materialize:?} serialize_and_decode={:?}",
            tail.iter().map(|e| e.parts.len()).sum::<usize>(),
            tail_wire.len(),
            t.elapsed()
        );
        let t = Instant::now();
        let entries = zeron_doc::join_continuation_entries(doc.read_entries()?);
        let materialize = t.elapsed();
        let mut kinds = std::collections::BTreeMap::<&str, (usize, usize, usize)>::new();
        for entry in &entries {
            for part in &entry.parts {
                let kind = match part {
                    zeron_doc::MessagePart::Text { .. } => "text",
                    zeron_doc::MessagePart::Reasoning { .. } => "reasoning",
                    zeron_doc::MessagePart::Tool { .. } => "tool",
                    _ => "other",
                };
                let size = part.byte_len();
                let slot = kinds.entry(kind).or_default();
                slot.0 += 1;
                slot.1 += size;
                slot.2 = slot.2.max(size);
            }
        }
        println!("part count/bytes/max: {kinds:?}");

        let t = Instant::now();
        let update = zeron_doc::TranscriptUpdate {
            frame: zeron_doc::TranscriptFrame::reset(&entries),
            context_usage: doc.context_usage(),
            replay_baseline: Some(zeron_doc::TranscriptBaseline::capture(&entries)),
        };
        let value = serde_json::to_value(update)?;
        let wire = serde_json::to_vec(&value)?;
        let serialize = t.elapsed();
        let t = Instant::now();
        let value: serde_json::Value = serde_json::from_slice(&wire)?;
        let _: zeron_doc::TranscriptUpdate = serde_json::from_value(value)?;
        let deserialize = t.elapsed();
        println!(
            "{} snapshot_bytes={} entries={} wire_bytes={} read={read:?} import={import:?} materialize={materialize:?} serialize={serialize:?} deserialize={deserialize:?}",
            path,
            bytes.len(),
            entries.len(),
            wire.len()
        );
    }
    Ok(())
}
