//! Fork must survive the child being opened before the fork lands.
//!
//! The composer selects the new chat the moment the user confirms, so the
//! engine opens an empty doc for it — and the born-on-chat2 path persists that
//! empty placeholder — BEFORE `Mutate forkChat` arrives. An idempotency check
//! keyed on "a snapshot exists" reads that placeholder as "already forked" and
//! silently skips the copy, leaving the user with an empty branch. These tests
//! pin the transcript onto the child in both arrival orders.

use std::sync::Arc;

use zeron_doc::MessageRole;
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_proto::HarnessId;

const SOURCE: &str = "chat-fork-source";
const CHILD: &str = "chat-fork-child";

fn assemble(dir: &std::path::Path) -> EngineCore {
    let registry = HarnessRegistry::new();
    EngineCore::assemble(dir, Arc::new(registry), HarnessId::Mock, None)
        .expect("engine core assembles")
}

fn seed_source(core: &EngineCore) -> Arc<zeron_engine::ChatDocHandle> {
    core.workspace
        .create_chat(SOURCE, None, Some(&core.device_id), None, None)
        .expect("create source row");
    let handle = core.doc_host.open(SOURCE).expect("open source");
    for (index, text) in ["first", "second", "third"].iter().enumerate() {
        handle
            .write_user_message(&format!("m{index}"), text, index as i64)
            .expect("write source message");
    }
    // Keep the handle alive: production forks the chat the user is looking at,
    // so the source doc is open and authoritative (its snapshot may still lag
    // the debounce).
    handle
}

fn child_ids(core: &EngineCore) -> Vec<String> {
    core.doc_host
        .open(CHILD)
        .ok()
        .and_then(|h| h.doc().read_entries().ok())
        .map(|entries| entries.into_iter().map(|e| e.id).collect())
        .unwrap_or_default()
}

async fn fork(core: &EngineCore, source: &str, child: &str) {
    let client = zeron_rpc::memory_client(core.rpc_service());
    client
        .call(
            zeron_rpc::methods::MUTATE,
            serde_json::json!({
                "op": "forkChat",
                "sourceChatId": source,
                "newChatId": child,
            }),
        )
        .await
        .expect("forkChat succeeds");
}

/// The production order: the UI opens (and persists) the child first, then the
/// fork RPC arrives. The transcript must still land on the child.
#[tokio::test(flavor = "multi_thread")]
async fn fork_after_the_child_was_already_opened_keeps_the_transcript() {
    let tmp = tempfile::tempdir().unwrap();
    let core = assemble(tmp.path());

    let _source = seed_source(&core);
    core.workspace
        .create_chat(CHILD, None, Some(&core.device_id), None, None)
        .expect("create child row");
    // The composer's optimistic select: the empty doc is minted and persisted
    // before the fork RPC is even sent.
    let child = core.doc_host.open(CHILD).expect("open child early");
    assert_eq!(child.doc().message_count(), 0, "child starts empty");

    fork(&core, SOURCE, CHILD).await;

    let ids = child_ids(&core);
    assert_eq!(
        ids,
        vec!["m0", "m1", "m2"],
        "forked transcript missing from an already-open child"
    );
    let roles: Vec<MessageRole> = child
        .doc()
        .read_entries()
        .unwrap()
        .into_iter()
        .map(|e| e.role)
        .collect();
    assert!(roles.iter().all(|r| *r == MessageRole::User));

    core.shutdown().await;
}

/// The cold order: nothing is open, so the fork seeds the store and the child
/// is read back fresh. Guards the non-live branch of `fork_chat`.
#[tokio::test(flavor = "multi_thread")]
async fn fork_without_an_open_child_seeds_the_store() {
    let tmp = tempfile::tempdir().unwrap();
    let core = assemble(tmp.path());

    let _source = seed_source(&core);
    fork(&core, SOURCE, CHILD).await;

    assert_eq!(child_ids(&core), vec!["m0", "m1", "m2"]);

    core.shutdown().await;
}

/// A retry after a successful fork must not duplicate rows.
#[tokio::test(flavor = "multi_thread")]
async fn forking_twice_does_not_duplicate_the_transcript() {
    let tmp = tempfile::tempdir().unwrap();
    let core = assemble(tmp.path());

    let _source = seed_source(&core);
    core.workspace
        .create_chat(CHILD, None, Some(&core.device_id), None, None)
        .expect("create child row");
    core.doc_host.open(CHILD).expect("open child early");

    fork(&core, SOURCE, CHILD).await;
    fork(&core, SOURCE, CHILD).await;

    assert_eq!(
        child_ids(&core),
        vec!["m0", "m1", "m2"],
        "a retried fork duplicated rows"
    );

    core.shutdown().await;
}
