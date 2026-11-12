//! Times what one commit and one open cost on a real chat snapshot.
//! cargo run -p zeron-doc --release --example big_chat_costs -- <snapshot>
use std::time::Instant;
use loro::{ContainerTrait, ToJson};
use zeron_doc::*;

fn main() {
    let path = std::env::args().nth(1).expect("snapshot path");
    let bytes = std::fs::read(&path).unwrap();
    let t = Instant::now();
    let doc = loro::LoroDoc::new();
    doc.import(&bytes).unwrap();
    println!("import {} MB: {:?}", bytes.len() / 1_000_000, t.elapsed());
    let sd = SessionDoc::from_doc(doc);
    for i in 0..3 {
        let t = Instant::now();
        let entries = sd.read_entries().unwrap();
        let read = t.elapsed();
        let t = Instant::now();
        let joined = join_continuation_entries(entries.clone());
        let join = t.elapsed();
        let t = Instant::now();
        let frame = diff_transcript_with(&joined, &joined, DiffOptions { part_patches: true });
        let diff = t.elapsed();
        let t = Instant::now();
        let reset = serde_json::to_string(&TranscriptFrame::reset(&joined)).unwrap();
        let ser = t.elapsed();
        println!("#{i} read_entries {:?} ({} entries) join {:?} diff(same) {:?} empty={} reset json {:?} ({} MB)",
            read, entries.len(), join, diff, frame.is_empty_delta(), ser, reset.len() / 1_000_000);
    }
    let entries = sd.read_entries().unwrap();
    let t = Instant::now();
    let c = entries.clone();
    println!("clone all entries {:?} ({})", t.elapsed(), c.len());
    let list = sd.doc().get_list("messages");
    let t = Instant::now();
    let shallow = list.get_value();
    println!("shallow list value {:?} ({:?} items)", t.elapsed(), shallow.as_list().map(|l| l.len()));
    let t = Instant::now();
    let mut ids = Vec::new();
    for i in 0..list.len() { if let Some(loro::ValueOrContainer::Container(c)) = list.get(i) { ids.push(c.id()); } }
    println!("walk ids via get(i) {:?} ({})", t.elapsed(), ids.len());
    let t = Instant::now();
    if let Some(loro::ValueOrContainer::Container(c)) = list.get(list.len() - 1) { let loro::Container::Map(m) = c else { panic!() }; let v = m.get_deep_value(); println!("deep read last entry {:?} {}", t.elapsed(), v.to_json_value().to_string().len()); }
    let t = Instant::now();
    let tail = sd.read_opening_tail(128).unwrap();
    println!("opening tail {:?} ({} entries)", t.elapsed(), tail.len());
    // A reply streaming: one entry changes per commit.
    let _ = sd.read_entries().unwrap();
    let mut writer = SegmentWriter::begin(&sd, "bench-reply", "dev", 1).unwrap();
    for n in 1..=5 {
        writer.sync(&[MessagePart::Text { id: "t".into(), text: "word ".repeat(n) }]).unwrap();
        let t = Instant::now();
        let cached = sd.read_entries().unwrap();
        let c = t.elapsed();
        let t = Instant::now();
        let whole = sd.read_entries_whole().unwrap();
        let w = t.elapsed();
        assert_eq!(cached, whole);
        let t = Instant::now();
        let joined = join_continuation_entries(cached);
        let j = t.elapsed();
        println!("commit {n}: cached read {c:?} vs whole {w:?}; join {j:?} ({} rows)", joined.len());
    }
    let t = Instant::now();
    let snap = sd.doc().export(loro::ExportMode::Snapshot).unwrap();
    println!("export snapshot {:?} ({} MB)", t.elapsed(), snap.len() / 1_000_000);
}
