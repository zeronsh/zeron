//! Keep this in its own test binary: the allocator counts all live allocations.
//! Measuring retained allocations avoids platform-dependent RSS/GC thresholds.
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use zeron_doc::{MessagePart, SegmentWriter, SessionDoc, fold_event_into_parts};
use zeron_proto::AgentEvent;

struct CountingAllocator;
static LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every allocation is delegated unchanged to System; we only count the
// requested sizes of successful allocations and their corresponding releases.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            LIVE_BYTES.fetch_add(layout.size(), Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        LIVE_BYTES.fetch_sub(layout.size(), Relaxed);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let new_ptr = unsafe { System.realloc(ptr, layout, size) };
        if !new_ptr.is_null() {
            LIVE_BYTES.fetch_add(size, Relaxed);
            LIVE_BYTES.fetch_sub(layout.size(), Relaxed);
        }
        new_ptr
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

#[test]
fn streaming_with_a_snapshot_every_tick_has_bounded_retained_memory() {
    let before = LIVE_BYTES.load(Relaxed);
    let doc = SessionDoc::init("streaming-memory").unwrap();
    let mut writer = SegmentWriter::begin(&doc, "assistant", "device", 1).unwrap();
    let mut folded = Vec::new();
    let token = "streaming text: ".repeat(16);
    let ticks = 2000;
    for _ in 0..ticks {
        fold_event_into_parts(
            &mut folded,
            &AgentEvent::TextDelta {
                text: token.clone(),
            },
        );
        writer.sync(&folded).unwrap();
        // ChatPersistence exports a dirty document each tick. Do not retain
        // these bytes: the regression was inside Loro, not the snapshot owner.
        drop(doc.export_snapshot().unwrap());
    }
    let retained = LIVE_BYTES.load(Relaxed).saturating_sub(before);
    eprintln!(
        "retained {retained} bytes for {} streamed bytes",
        token.len() * ticks
    );
    // Deliberately generous: fixed versions retain a few MiB; 1.13.9 retains
    // hundreds of MiB. Includes the application writer's mirrored strings.
    assert!(
        retained < 32 * 1024 * 1024,
        "retained {retained} bytes after repeated exports"
    );

    let restored = loro::LoroDoc::new();
    restored.import(&doc.export_snapshot().unwrap()).unwrap();
    assert_eq!(restored.get_deep_value(), doc.doc().get_deep_value());
    let entries = doc.read_entries().unwrap();
    match &entries[0].parts[0] {
        MessagePart::Text { text, .. } => assert_eq!(text, &token.repeat(ticks)),
        other => panic!("expected streamed text, got {other:?}"),
    }
}
