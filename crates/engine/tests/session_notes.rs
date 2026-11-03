//! Session notes over the RPC surface the UIs call.
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use zeron_doc::{NOTES_MAX_BYTES, NotesSnapshot, NotesWrite, SessionDoc, notes_hash};
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_proto::HarnessId;
use zeron_rpc::methods;

const CHAT: &str = "notes-chat";

fn assemble_at(path: &Path) -> EngineCore {
    EngineCore::assemble(
        path,
        Arc::new(HarnessRegistry::new()),
        HarnessId::Mock,
        None,
    )
    .expect("engine core assembles")
}

fn create_chat(core: &EngineCore) {
    core.workspace
        .create_chat(CHAT, None, Some(&core.device_id), None, Some("/tmp".into()))
        .unwrap();
}

async fn next_snapshot(rx: &mut tokio::sync::mpsc::Receiver<serde_json::Value>) -> NotesSnapshot {
    let frame = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("a notes frame")
        .expect("stream open");
    serde_json::from_value(frame).expect("a notes snapshot")
}

async fn set(client: &zeron_rpc::RpcClient, base_hash: &str, text: &str) -> NotesWrite {
    client
        .call_as(
            methods::SET_SESSION_NOTES,
            serde_json::json!({ "chatId": CHAT, "baseHash": base_hash, "text": text }),
        )
        .await
        .expect("SetSessionNotes")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn notes_stream_and_compare_and_set() {
    let dir = tempfile::tempdir().unwrap();
    let core = assemble_at(dir.path());
    create_chat(&core);
    let client = zeron_rpc::memory_client(core.rpc_service());
    let mut rx = client
        .subscribe(
            methods::WATCH_SESSION_NOTES,
            serde_json::json!({ "chatId": CHAT }),
        )
        .await
        .expect("WatchSessionNotes");
    let empty = next_snapshot(&mut rx).await;
    assert_eq!(empty.text, "");
    assert_eq!(empty.hash, NotesSnapshot::of("").hash);

    let written = set(&client, &empty.hash, "# Plan\n").await;
    assert_eq!(
        written,
        NotesWrite::Applied {
            hash: notes_hash("# Plan\n"),
            epoch: empty.epoch,
            seq: empty.seq + 1,
        }
    );
    // The watch reports the same state under the same version.
    let plan = next_snapshot(&mut rx).await;
    assert_eq!(plan.text, "# Plan\n");
    assert_eq!((plan.epoch, plan.seq), (empty.epoch, empty.seq + 1));
    assert!(plan.supersedes(&empty));

    // A second writer still on the empty base loses, and learns why.
    match set(&client, &empty.hash, "stale").await {
        NotesWrite::Conflict { current } => assert_eq!(current.text, "# Plan\n"),
        other => panic!("expected a conflict, got {other:?}"),
    }
    let base = notes_hash("# Plan\n");
    let huge = "x".repeat(NOTES_MAX_BYTES + 1);
    assert_eq!(
        set(&client, &base, &huge).await,
        NotesWrite::TooLarge {
            limit: NOTES_MAX_BYTES
        }
    );
    assert_eq!(
        core.doc_host.open(CHAT).unwrap().doc().notes_text(),
        "# Plan\n"
    );
    core.shutdown().await;
}

/// Another device's edit arrives as a remote import, not a local commit; the
/// watch must still carry it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_notes_edits_reach_the_watch() {
    let dir = tempfile::tempdir().unwrap();
    let core = assemble_at(dir.path());
    create_chat(&core);
    let handle = core.doc_host.open(CHAT).unwrap();
    handle.doc().set_notes_text("shared\n").unwrap();
    let client = zeron_rpc::memory_client(core.rpc_service());
    let mut rx = client
        .subscribe(
            methods::WATCH_SESSION_NOTES,
            serde_json::json!({ "chatId": CHAT }),
        )
        .await
        .expect("WatchSessionNotes");
    assert_eq!(next_snapshot(&mut rx).await.text, "shared\n");

    let other = SessionDoc::from_doc(loro::LoroDoc::new());
    other
        .doc()
        .import(&handle.doc().export_snapshot().unwrap())
        .unwrap();
    let before = other.doc().oplog_vv();
    other.set_notes_text("shared\nfrom the phone\n").unwrap();
    let update = other
        .doc()
        .export(loro::ExportMode::updates(&before))
        .unwrap();
    handle.doc().doc().import(&update).unwrap();

    assert_eq!(
        next_snapshot(&mut rx).await.text,
        "shared\nfrom the phone\n"
    );
    core.shutdown().await;
}

/// Local-only profiles keep notes across restarts like the rest of the doc.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn notes_persist_across_restarts() {
    let dir = tempfile::tempdir().unwrap();
    {
        let core = assemble_at(dir.path());
        create_chat(&core);
        let client = zeron_rpc::memory_client(core.rpc_service());
        let written = set(&client, &notes_hash(""), "- [ ] survive a restart\n").await;
        assert!(matches!(written, NotesWrite::Applied { .. }));
        core.shutdown().await;
    }
    let core = assemble_at(dir.path());
    assert_eq!(
        core.doc_host.open(CHAT).unwrap().doc().notes_text(),
        "- [ ] survive a restart\n"
    );
    core.shutdown().await;
}
