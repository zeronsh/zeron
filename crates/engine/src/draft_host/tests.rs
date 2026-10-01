use super::*;
use futures::StreamExt;

fn config(edge: Option<EdgeConfig>, tuning: DraftTuning) -> DraftHostConfig {
    DraftHostConfig {
        device_id: "dev-a".into(),
        org_id: "org".into(),
        edge,
        tuning,
    }
}

fn host_with(
    edge: Option<EdgeConfig>,
    tuning: DraftTuning,
) -> (DraftHost, Arc<DocsStore>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(DocsStore::open(dir.path()).unwrap());
    let host = DraftHost::new(store.clone(), config(edge, tuning));
    (host, store, dir)
}

fn fast() -> DraftTuning {
    DraftTuning {
        debounce: Duration::from_millis(40),
        ..DraftTuning::default()
    }
}

/// An edge nobody listens on: every room call fails fast, which is exactly "offline".
fn dead_edge() -> EdgeConfig {
    EdgeConfig::with_static_token("http://127.0.0.1:1", "t")
}

/// A fresh replica that typed `text`, plus the Loro update of that commit.
fn typed(text: &str) -> (DraftDoc, Vec<u8>) {
    let doc = DraftDoc::new();
    let updates = Arc::new(Mutex::new(Vec::new()));
    let sink = updates.clone();
    let _sub = doc.subscribe_local_update(move |bytes| lock(&sink).push(bytes.to_vec()));
    doc.set_text(text).unwrap();
    let update = lock(&updates).last().cloned().unwrap();
    (doc, update)
}

async fn first_frame(host: &DraftHost, chat: &str) -> (DraftFrame, BoxStream<'static, DraftFrame>) {
    let mut stream = host.watch(chat).unwrap();
    let frame = stream.next().await.unwrap();
    (frame, stream)
}

fn decode(frame: &DraftFrame) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(&frame.update)
        .unwrap()
}

fn replica(frame: &DraftFrame) -> DraftDoc {
    assert!(frame.reset);
    DraftDoc::from_snapshot(&decode(frame)).unwrap()
}

#[test]
fn row_codec_round_trips_and_rejects_foreign_bytes() {
    let row = encode_row(7, true, false, b"snap");
    let decoded = decode_row(&row).unwrap();
    assert_eq!(decoded.epoch, 7);
    assert!(decoded.dirty && !decoded.pending_discard);
    assert_eq!(decoded.snapshot, b"snap");
    let pending = decode_row(&encode_row(1, false, true, b"")).unwrap();
    assert!(pending.pending_discard && !pending.dirty);
    assert!(decode_row(b"not a draft row").is_none());
    assert!(decode_row(&[]).is_none());
}

#[test]
fn chat_ids_follow_the_room_id_rules() {
    assert!(is_valid_chat_id("chat-1_A"));
    assert!(is_valid_chat_id(&"a".repeat(128)));
    assert!(!is_valid_chat_id(""));
    assert!(!is_valid_chat_id(&"a".repeat(129)));
    assert!(!is_valid_chat_id("a/b"));
    assert!(!is_valid_chat_id("draft/x"));
    assert!(!is_valid_chat_id("a b"));
    assert!(!is_valid_chat_id("../x"));
    assert!(is_draft_doc_id(&draft_doc_id("x")));
    assert!(!is_draft_doc_id("x"));
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_ids_and_bytes_are_rejected() {
    let (host, _store, _dir) = host_with(None, fast());
    assert!(matches!(
        host.watch("bad/id"),
        Err(DraftError::InvalidChatId)
    ));
    assert!(matches!(host.clear(""), Err(DraftError::InvalidChatId)));
    assert!(matches!(
        host.edit("chat", b"definitely not loro"),
        Err(DraftError::InvalidUpdate(_))
    ));
    assert!(matches!(
        host.edit("chat", &vec![0u8; MAX_EDIT_BYTES + 1]),
        Err(DraftError::InvalidUpdate(_))
    ));
    // A rejected edit leaves the draft untouched.
    assert_eq!(host.status("chat").unwrap().text, "");
}

#[tokio::test(flavor = "multi_thread")]
async fn watchers_get_a_reset_then_every_change_including_echoes() {
    let (host, _store, _dir) = host_with(None, fast());
    let (frame, mut stream) = first_frame(&host, "chat").await;
    assert!(frame.reset);
    assert_eq!(replica(&frame).text(), "");

    let (_, update) = typed("hello");
    let (_, changed) = host.edit("chat", &update).unwrap();
    assert!(changed);
    let echo = stream.next().await.unwrap();
    assert!(!echo.reset);
    let window = replica(&frame);
    window.import(&decode(&echo)).unwrap();
    assert_eq!(window.text(), "hello");

    // Re-sending the same update changes nothing and emits nothing.
    let (_, changed) = host.edit("chat", &update).unwrap();
    assert!(!changed);
    host.clear("chat").unwrap();
    let reset = stream.next().await.unwrap();
    assert!(
        reset.reset,
        "the next frame is the clear, not a duplicate echo"
    );
    assert_eq!(replica(&reset).text(), "");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_watcher_starts_from_the_current_text() {
    let (host, _store, _dir) = host_with(None, fast());
    let (_, update) = typed("shared");
    host.edit("chat", &update).unwrap();
    let (frame, _stream) = first_frame(&host, "chat").await;
    assert_eq!(replica(&frame).text(), "shared");
}

#[tokio::test(flavor = "multi_thread")]
async fn persistence_is_debounced_and_restores_after_restart() {
    let (host, store, dir) = host_with(
        None,
        DraftTuning {
            debounce: Duration::from_millis(300),
            ..DraftTuning::default()
        },
    );
    let (doc, first) = typed("a");
    host.edit("chat", &first).unwrap();
    for text in ["ab", "abc", "abcd"] {
        let before = doc.version();
        doc.set_text(text).unwrap();
        host.edit("chat", &doc.export_since(&before).unwrap())
            .unwrap();
    }
    // Inside the debounce window nothing has been written yet: bursts coalesce.
    assert!(
        store
            .load_snapshot(&draft_doc_id("chat"))
            .unwrap()
            .is_none()
    );
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert!(
        store
            .load_snapshot(&draft_doc_id("chat"))
            .unwrap()
            .is_some()
    );
    drop(host);

    let reopened = DraftHost::new(
        Arc::new(DocsStore::open(dir.path()).unwrap()),
        config(None, fast()),
    );
    assert_eq!(reopened.status("chat").unwrap().text, "abcd");
}

#[tokio::test(flavor = "multi_thread")]
async fn local_only_clear_deletes_the_row_and_resets_watchers() {
    let (host, store, _dir) = host_with(None, fast());
    let (_, update) = typed("draft");
    host.edit("chat", &update).unwrap();
    host.flush_all();
    assert!(store.has_snapshot(&draft_doc_id("chat")).unwrap());
    let outcome = host.clear("chat").unwrap();
    assert!(!outcome.pending, "no edge: nothing to discard remotely");
    assert!(!store.has_snapshot(&draft_doc_id("chat")).unwrap());
    assert_eq!(host.status("chat").unwrap().text, "");
}

#[tokio::test(flavor = "multi_thread")]
async fn epoch_first_contact_keeps_local_text_but_a_new_epoch_drops_it() {
    let (host, _store, _dir) = host_with(None, fast());
    let (_, update) = typed("typed offline");
    host.edit("chat", &update).unwrap();
    let handle = host.open_handle("chat").unwrap();
    let (first, mut stream) = first_frame(&host, "chat").await;
    assert_eq!(first.epoch, 0);

    // First contact: never joined before, so the offline text survives and adopts the epoch.
    handle.adopt_epoch(3);
    assert_eq!(host.status("chat").unwrap().text, "typed offline");
    assert_eq!(host.status("chat").unwrap().epoch, 3);
    assert_eq!(handle.epoch_cell.load(Ordering::Acquire), 3);

    // Same epoch: nothing happens.
    handle.adopt_epoch(3);
    assert_eq!(host.status("chat").unwrap().text, "typed offline");

    // The draft was sent elsewhere (epoch moved on): local replica dropped, watchers reset.
    handle.adopt_epoch(4);
    let status = host.status("chat").unwrap();
    assert_eq!(status.text, "");
    assert_eq!(status.epoch, 4);
    assert!(
        !status.unacked,
        "a dropped replica has nothing left to push"
    );
    let reset = stream.next().await.unwrap();
    assert!(reset.reset);
    assert_eq!(reset.epoch, 4);
    assert_eq!(replica(&reset).text(), "");
}

#[tokio::test(flavor = "multi_thread")]
async fn pending_discard_survives_restart_and_ignores_old_epoch_content() {
    let (host, store, dir) = host_with(Some(dead_edge()), fast());
    let (_, update) = typed("about to be sent");
    host.edit("chat", &update).unwrap();
    let outcome = host.clear("chat").unwrap();
    assert!(
        outcome.pending,
        "the room is unreachable, so the discard stays pending"
    );
    let status = host.status("chat").unwrap();
    assert!(status.pending_discard);
    assert_eq!(status.text, "");

    // Remote content of the old epoch is not adopted while the discard is pending.
    let handle = host.open_handle("chat").unwrap();
    let (_, remote) = typed("stale remote text");
    handle.apply_remote(&remote).unwrap();
    assert_eq!(host.status("chat").unwrap().text, "");

    // New typing after the send belongs to the next draft and is kept.
    let (_, fresh) = typed("next");
    host.edit("chat", &fresh).unwrap();
    host.shutdown().await;
    drop(handle);
    drop(host);

    // The flag is durable: a restarted host still owes the room a discard.
    let row = decode_row(&store.load_snapshot(&draft_doc_id("chat")).unwrap().unwrap()).unwrap();
    assert!(row.pending_discard);
    let reopened = DraftHost::new(
        Arc::new(DocsStore::open(dir.path()).unwrap()),
        config(None, fast()),
    );
    let status = reopened.status("chat").unwrap();
    assert!(status.pending_discard);
    assert_eq!(status.text, "next");
}

#[tokio::test(flavor = "multi_thread")]
async fn unacked_handles_are_pinned_against_eviction() {
    // The edge is unreachable, so every edit stays unacknowledged.
    let (host, _store, _dir) = host_with(
        Some(dead_edge()),
        DraftTuning {
            handle_cap: 2,
            ..fast()
        },
    );
    for n in 0..6 {
        let (_, update) = typed(&format!("draft {n}"));
        host.edit(&format!("chat-{n}"), &update).unwrap();
    }
    assert_eq!(
        host.handle_count(),
        6,
        "unacknowledged drafts are never evicted"
    );
    for n in 0..6 {
        assert_eq!(
            host.status(&format!("chat-{n}")).unwrap().text,
            format!("draft {n}")
        );
    }
    host.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn clean_idle_handles_are_evicted_beyond_the_cap() {
    let (host, store, _dir) = host_with(
        None,
        DraftTuning {
            handle_cap: 2,
            ..fast()
        },
    );
    for n in 0..6 {
        host.status(&format!("chat-{n}")).unwrap();
    }
    assert!(host.handle_count() <= 2);
    assert!(!store.has_snapshot(&draft_doc_id("chat-0")).unwrap());
}

#[tokio::test(flavor = "multi_thread")]
async fn draft_rows_are_invisible_to_chat_sync_state() {
    let (host, store, _dir) = host_with(None, fast());
    let (_, update) = typed("hi");
    host.edit("chat", &update).unwrap();
    host.clear("chat").unwrap();
    let (_, update) = typed("again");
    host.edit("chat", &update).unwrap();
    host.flush_all();
    assert!(store.has_snapshot(&draft_doc_id("chat")).unwrap());
    // The chat sync scheduler pages `pending_sync_docs` / `pending_sync_jobs`; drafts never appear.
    assert!(store.pending_sync_docs("", 64).unwrap().is_empty());
    assert!(
        store
            .pending_sync_jobs("recovery", "", 64)
            .unwrap()
            .is_empty()
    );
    assert!(
        !store
            .has_pending_chat_updates(&draft_doc_id("chat"))
            .unwrap()
    );
}
