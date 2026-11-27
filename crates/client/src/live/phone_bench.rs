//! A phone's send while a big chat streams in, on privately supplied data (a
//! chat's snapshot and a registry, never written back): the chat's real
//! persister and room sink take remote rows every 150 ms while another thread
//! sends, doing on its own thread what a send's doc write does. Prints each
//! send's time on the sender's thread, bytes written per second and peak RSS.
//!
//! `ZERON_PHONE_BENCH_SEND=persist` saves the whole registry on every send (as
//! sends did before a chat's saved row was remembered); `lookup` only reads the
//! chat's row, as a send on a chat whose row is on disk does now.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use zeron_doc::{
    REGISTRY_DOC_ID, RegistryDoc, SessionCommandEntry, SessionCommandPayload, SessionCommandStatus,
};
use zeron_sync::chat_client::ChatDocSink;
use zeron_sync::DocsStore;

use super::room::{CHAT2_DOC_EPOCH, Persister, ViewerSink, load_local};

const STREAM_FOR: Duration = Duration::from_secs(20);
const ROW_EVERY: Duration = Duration::from_millis(150);
const SEND_EVERY: Duration = Duration::from_millis(500);

fn proc_field(file: &str, key: &str) -> u64 {
    std::fs::read_to_string(file)
        .unwrap_or_default()
        .lines()
        .find_map(|line| {
            let rest = line.strip_prefix(key)?.trim_start_matches(':').trim();
            rest.split_whitespace().next()?.parse().ok()
        })
        .unwrap_or(0)
}

#[test]
#[ignore = "requires ZERON_PHONE_BENCH_CHAT and ZERON_PHONE_BENCH_REGISTRY; ~30 s"]
fn phone_send_while_a_big_chat_streams() {
    let chat_bytes = std::fs::read(std::env::var("ZERON_PHONE_BENCH_CHAT").unwrap()).unwrap();
    let registry_bytes =
        std::fs::read(std::env::var("ZERON_PHONE_BENCH_REGISTRY").unwrap()).unwrap();
    let persist = std::env::var("ZERON_PHONE_BENCH_SEND").as_deref() != Ok("lookup");
    let dir = tempfile::tempdir().unwrap();
    let chat = "bench-chat";

    // On disk as the app left it: the chat saved whole, the registry saved.
    {
        let store = DocsStore::open(dir.path()).unwrap();
        let raw = loro::LoroDoc::new();
        raw.import(&chat_bytes).unwrap();
        let snapshot = raw.export(loro::ExportMode::Snapshot).unwrap();
        store.save_verified_snapshot_with_cursor(chat, &snapshot, 1, CHAT2_DOC_EPOCH).unwrap();
        store.save_snapshot(REGISTRY_DOC_ID, &registry_bytes).unwrap();
    }
    // The next launch (its open folds the WAL into the file).
    let store = Arc::new(DocsStore::open(dir.path()).unwrap());
    let registry = Arc::new(Mutex::new(RegistryDoc::from_bytes(&registry_bytes, "bench-phone").unwrap()));
    let known_chat = registry.lock().unwrap().read_chats().unwrap()[0].id.clone();
    drop(chat_bytes);
    drop(registry_bytes);

    // The chat opened as a session opens it.
    let local = load_local(&store, chat);
    let doc = Arc::new(local.doc);
    let persister = Persister::new(&doc, store.clone(), chat, local.cursor);
    let cancel = tokio_util::sync::CancellationToken::new();
    persister.start(cancel.clone());
    let sink = ViewerSink {
        doc: Arc::downgrade(&doc),
        store: store.clone(),
        chat_id: chat.into(),
        persister: persister.clone(),
        on_applied: Arc::new(|| {}),
        relay: false,
        defer_checkpoint: Arc::new(AtomicBool::new(false)),
    };
    // The room's own-writes hook: outbox row, then the doc marked changed.
    let _local_updates = {
        let (store, persister) = (store.clone(), persister.clone());
        doc.doc().subscribe_local_update(Box::new(move |bytes: &Vec<u8>| {
            store.enqueue_chat_update(chat, &crate::new_id(), bytes).unwrap();
            persister.dirty(false);
            true
        }))
    };

    // Measure from here: the streaming minute, not the load.
    let _ = std::fs::write("/proc/self/clear_refs", "5");
    let wchar0 = proc_field("/proc/self/io", "wchar");
    let started = Instant::now();
    let stop = Arc::new(AtomicBool::new(false));

    let sender = {
        let (doc, store, registry, stop) = (doc.clone(), store.clone(), registry.clone(), stop.clone());
        std::thread::spawn(move || {
            let mut times = Vec::new();
            let mut n = 0u64;
            while !stop.load(Ordering::Relaxed) {
                n += 1;
                let t = Instant::now();
                // What a send does on its own thread, in order.
                let _ = store.has_active_held_send(chat).unwrap();
                if persist {
                    let bytes = registry.lock().unwrap().to_bytes().unwrap();
                    store.save_snapshot(REGISTRY_DOC_ID, &bytes).unwrap();
                } else {
                    let row = registry.lock().unwrap().chat(&known_chat).unwrap();
                    assert!(row.is_some());
                }
                let id = crate::new_id();
                doc.queue_command(&SessionCommandEntry {
                    id: id.clone(),
                    payload: SessionCommandPayload::Steer {
                        prompt: format!("bench message {n}: please keep going with the plan"),
                        message_id: Some(id),
                    },
                    issued_by: "bench-phone".into(),
                    issued_at: crate::now_ms(),
                    based_on: None,
                    expires_at: None,
                    status: SessionCommandStatus::Pending,
                    resolution: None,
                })
                .unwrap();
                store.schedule_sync_job(chat, super::delivery::JOB).unwrap();
                times.push(t.elapsed());
                std::thread::sleep(SEND_EVERY);
            }
            times
        })
    };

    // Remote rows, as the room delivers them.
    let remote = loro::LoroDoc::new();
    let text = remote.get_text("bench-remote-stream");
    let mut at = remote.oplog_vv();
    let mut cursor = local.cursor.max(1);
    let mut rows = 0;
    while started.elapsed() < STREAM_FOR {
        cursor += 1;
        text.insert(text.len_unicode(), &format!("{cursor:06} {}", "streamed agent output ".repeat(9)))
            .unwrap();
        remote.commit();
        let bytes = remote.export(loro::ExportMode::updates(&at)).unwrap();
        at = remote.oplog_vv();
        let _ = sink.apply_row(&bytes, cursor);
        rows += 1;
        std::thread::sleep(ROW_EVERY);
    }
    stop.store(true, Ordering::Relaxed);
    let mut times = sender.join().unwrap();
    let first = times[0];
    let elapsed = started.elapsed().as_secs_f64();
    let written = proc_field("/proc/self/io", "wchar") - wchar0;
    let hwm_kb = proc_field("/proc/self/status", "VmHWM");
    times.sort();
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    let mean = times.iter().map(|d| ms(*d)).sum::<f64>() / times.len() as f64;
    let p95 = ms(times[(times.len() * 95 / 100).min(times.len() - 1)]);
    let worst = ms(*times.last().unwrap());
    let next_worst = ms(times[times.len() - 2]);
    eprintln!(
        "BENCH send={} sends={} rows={rows} mean_ms={mean:.1} p95_ms={p95:.1} worst_ms={worst:.1} \
         second_worst_ms={next_worst:.1} first_send_ms={:.1} \
         written_mb_per_s={:.2} peak_rss_mb={:.0}",
        if persist { "persist-registry" } else { "lookup" },
        times.len(),
        ms(first),
        written as f64 / elapsed / 1_048_576.0,
        hwm_kb as f64 / 1024.0,
    );
    cancel.cancel();
}
