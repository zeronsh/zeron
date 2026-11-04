//! Cross-authority native import: a host's own projects move to its new device
//! even when another host's earlier import already carried them as inert copies,
//! and chats re-homed before that mapping existed are re-filed on re-run.

use std::sync::Arc;

use zeron_engine::native_profile_import::{NativeImportOptions, NativeImportTarget, NativeProfileImporter};
use zeron_engine::{EngineCore, EngineProfile, HarnessId, default_registry};

fn assemble(profile: EngineProfile) -> EngineCore {
    EngineCore::assemble_with_profile(profile, Arc::new(default_registry()), HarnessId::Mock, None)
        .expect("assemble profile")
}

const CHAT: &str = "5272964e-26da-4eaf-b6cb-5099a7aef3d3";
const SPACE: &str = "adbfbe9c-e369-4036-a386-d8366a119092";

#[tokio::test(flavor = "multi_thread")]
async fn host_projects_are_rehomed_and_misfiled_chats_refiled() {
    let dir = tempfile::tempdir().expect("tempdir");

    // The source host's profile: one project and one chat it hosts.
    let source_dir = dir.path().join("source-host");
    let source = assemble(EngineProfile::local(&source_dir).expect("local profile"));
    let source_device = source.device_id.clone();
    source.workspace.create_space(SPACE, &source_device, "/home/user/ashler-platform", None, true)
        .expect("source space");
    source.workspace.create_chat(CHAT, Some(SPACE), None, None, None).expect("source chat");
    let source_row = source.workspace.chat(CHAT).unwrap().expect("source chat row");
    source.shutdown().await;
    // A consistent stopped-writer copy, the way the operator scripts take it
    // (`sqlite3 .backup`; here `VACUUM INTO`): one file with the WAL folded in.
    let copy = dir.path().join("copy");
    std::fs::create_dir_all(&copy).unwrap();
    let live = rusqlite::Connection::open(source_dir.join("profiles/local/docs.sqlite3")).unwrap();
    live.execute("VACUUM INTO ?1", [copy.join("docs.sqlite3").to_str().unwrap()]).unwrap();
    drop(live);

    // The target already holds another host's inert copy of the project, and the
    // chat re-homed here but still filed under that copy.
    let target = assemble(EngineProfile::synced(&dir.path().join("target"), "org", "owner@example.com"));
    let target_device = target.device_id.clone();
    let inert: zeron_proto::Space = serde_json::from_value(serde_json::json!({
        "id": SPACE, "deviceId": source_device, "path": "/home/user/ashler-platform", "gitDetected": true,
        "createdAt": "2026-11-03T20:13:20Z",
    })).unwrap();
    target.workspace.import_space_row(&inert).unwrap();
    let mut misfiled = source_row;
    misfiled.device_id = target_device.clone();
    target.workspace.import_chat_row(&misfiled).unwrap();

    let importer = NativeProfileImporter {
        doc_host: target.doc_host.clone(),
        workspace: target.workspace.clone(),
        uploads: target.uploads.clone(),
        target: NativeImportTarget {
            owner_subject: "owner@example.com".into(),
            project_id: "ashler-staging".into(),
            deployment_id: Some("ashler-staging".into()),
        },
    };
    let options = |validate_only| NativeImportOptions { source_copy: copy.clone(), validate_only, ..Default::default() };

    let plan = importer.run(options(true)).await.expect("plan");
    assert_eq!((plan.chats_refiled, plan.spaces_imported, plan.spaces_mapped), (1, 1, 1), "{plan:?}");
    assert_eq!(target.workspace.chat(CHAT).unwrap().unwrap().space_id.as_deref(), Some(SPACE), "plan writes nothing");

    let report = importer.run(options(false)).await.expect("import");
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.chats_refiled, 1);
    let chat = target.workspace.chat(CHAT).unwrap().unwrap();
    let rehomed = chat.space_id.clone().expect("chat keeps a project");
    assert_ne!(rehomed, SPACE, "chat must leave the inert copy");
    let space = target.workspace.space(&rehomed).unwrap().expect("re-homed project exists");
    assert_eq!((space.device_id.as_str(), space.path.as_str()), (target_device.as_str(), "/home/user/ashler-platform"));
    assert_eq!(chat.device_id, target_device);
    let original = target.workspace.space(SPACE).unwrap().unwrap();
    assert_eq!(original.device_id, source_device, "another device's row is never rewritten");

    // Idempotent: the re-homed project now maps by path and nothing changes.
    let again = importer.run(options(false)).await.expect("re-run");
    assert_eq!((again.chats_refiled, again.spaces_imported), (0, 0), "{again:?}");
    assert_eq!(again.skipped_existing, again.chats_considered);
    assert_eq!(target.workspace.chat(CHAT).unwrap().unwrap().space_id.as_deref(), Some(rehomed.as_str()));
    target.shutdown().await;
}
