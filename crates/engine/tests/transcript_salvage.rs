//! Boot-time transcript salvage (born-gen2 aftermath): a hosted roomGen-2
//! chat whose doc is blank while its run journal has events gets its entries
//! re-appended from the fat lineage (here: the `.pre-chat2` rollback copy —
//! the on-disk stand-in for the legacy s2 room used in production). The doc
//! must fill back in without any run, command, or user action.

use std::sync::Arc;
use std::time::Duration;

use zeron_doc::{MessagePart, MessageRole, MessageStatus, SessionDoc, SessionMessageEntry};
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_proto::HarnessId;

const CHAT: &str = "chat-salvage";

fn assemble(dir: &std::path::Path) -> EngineCore {
    EngineCore::assemble(dir, Arc::new(HarnessRegistry::new()), HarnessId::Mock, None)
        .expect("engine core assembles")
}

fn entry(
    id: &str,
    role: MessageRole,
    text: &str,
    status: Option<MessageStatus>,
) -> SessionMessageEntry {
    SessionMessageEntry {
        id: id.into(),
        role,
        parts: vec![MessagePart::Text {
            id: "t0".into(),
            text: text.into(),
        }],
        created_at: 1_700_000_000_000,
        device_id: "lost-device".into(),
        status,
        continuation_of: None,
        duration_ms: None,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn blank_journaled_chat_recovers_entries_from_fat_rollback() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("data");

    // Phase 1: mint the chat (roomGen 2 stamped) on this device, then stop —
    // the doc itself never gets a single entry, like a post-loss reopen.
    {
        let core = assemble(&dir);
        let client = zeron_rpc::memory_client(core.rpc_service());
        client
            .call(
                zeron_rpc::methods::MUTATE,
                serde_json::json!({
                    "op": "createChat",
                    "chatId": CHAT,
                    "deviceId": core.device_id,
                }),
            )
            .await
            .expect("createChat");
        core.shutdown().await;
    }

    // Phase 2: plant the loss artifacts the salvage keys on — a non-empty run
    // journal, and the fat lineage holding the real transcript.
    let org_dir = dir.join("orgs").join("dev-org").join("dev-user");
    let journals = org_dir.join("journals");
    std::fs::create_dir_all(&journals).unwrap();
    std::fs::write(
        journals.join(format!("{CHAT}.jsonl")),
        "{\"seq\":1,\"event\":{\"type\":\"sessionStarted\"}}\n",
    )
    .unwrap();
    let fat = SessionDoc::init(CHAT).unwrap();
    fat.push_message(&entry(
        "u-1",
        MessageRole::User,
        "the codeword is PINEAPPLE",
        None,
    ))
    .unwrap();
    fat.push_message(&entry(
        "a-1",
        MessageRole::Assistant,
        "acknowledged: PINEAPPLE",
        Some(MessageStatus::Complete),
    ))
    .unwrap();
    let fat_bytes = fat.export_snapshot().unwrap();
    {
        let store = zeron_sync::DocsStore::open(&org_dir).unwrap();
        store
            .save_snapshot(&format!("{CHAT}.pre-chat2"), &fat_bytes)
            .unwrap();
    }

    // Phase 3: boot — the salvage sweep must refill the blank doc on its own.
    let core = assemble(&dir);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let entries = loop {
        let entries = core
            .doc_host
            .open(CHAT)
            .ok()
            .and_then(|h| h.doc().read_entries().ok())
            .unwrap_or_default();
        if entries.len() == 2 {
            break entries;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "salvage never refilled the transcript: {entries:?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert_eq!(entries[0].id, "u-1");
    assert_eq!(entries[0].role, MessageRole::User);
    assert_eq!(entries[1].id, "a-1");
    assert_eq!(entries[1].status, Some(MessageStatus::Complete));

    // Reboot once more: salvage must be a no-op on a healthy doc (no dupes).
    core.shutdown().await;
    drop(core);
    let core = assemble(&dir);
    tokio::time::sleep(Duration::from_secs(7)).await;
    let after = core
        .doc_host
        .open(CHAT)
        .ok()
        .and_then(|h| h.doc().read_entries().ok())
        .unwrap_or_default();
    assert_eq!(
        after.len(),
        2,
        "salvage duplicated entries on reboot: {after:?}"
    );
    core.shutdown().await;
}
