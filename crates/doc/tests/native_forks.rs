use zeron_doc::*;
use zeron_proto::{HarnessId, NativeForkBoundary, NativeForkPoint, NativeForkResult};

fn point() -> NativeForkPoint {
    NativeForkPoint {
        format_version: 1,
        harness: HarnessId::Codex,
        source_device_id: "host".into(),
        source_session_id: "canonical".into(),
        cwd: "/project".into(),
        boundary: NativeForkBoundary::AppServerTurn {
            turn_id: "turn1".into(),
        },
    }
}
fn entry(id: &str, continuation: Option<&str>) -> SessionMessageEntry {
    SessionMessageEntry {
        id: id.into(),
        role: MessageRole::Assistant,
        parts: vec![MessagePart::Text {
            id: format!("{id}-text"),
            text: id.into(),
        }],
        created_at: 1,
        device_id: "host".into(),
        status: Some(MessageStatus::Complete),
        continuation_of: continuation.map(str::to_owned),
        duration_ms: None,
        native_fork_point: None,
    }
}
#[test]
fn native_fork_point_survives_continuations_snapshot_and_two_generations() {
    let source = SessionDoc::init("source").unwrap();
    source.push_message(&entry("a1", None)).unwrap();
    source.push_message(&entry("a1-tail", Some("a1"))).unwrap();
    source.set_native_fork_point("a1", &point()).unwrap();
    let entries = source.read_entries().unwrap();
    assert!(entries[0].native_fork_point.is_none());
    assert_eq!(entries[1].native_fork_point, Some(point()));
    assert_eq!(
        join_continuation_entries(entries.clone())[0].native_fork_point,
        Some(point())
    );
    for id in ["child", "grandchild"] {
        let child = SessionDoc::init(id).unwrap();
        for entry in &entries {
            child.push_message(entry).unwrap();
        }
        let lineage = NativeForkLineage {
            strategy: HistoryStrategy::NativeFork,
            request_id: id.into(),
            source_chat_id: "source".into(),
            source_message_id: "a1".into(),
            point: point(),
            child: NativeForkResult {
                session_id: id.into(),
                cwd: "/project".into(),
            },
        };
        child.set_native_fork_lineage(&lineage).unwrap();
        let raw = loro::LoroDoc::new();
        raw.import(&child.export_snapshot().unwrap()).unwrap();
        let reopened = SessionDoc::from_doc(raw);
        assert_eq!(reopened.native_fork_lineage().unwrap(), Some(lineage));
        assert_eq!(reopened.read_entries().unwrap(), entries);
        let thin = rebuild_thin_doc(&reopened).unwrap().doc;
        assert_eq!(
            thin.native_fork_lineage().unwrap(),
            reopened.native_fork_lineage().unwrap()
        );
    }
}
#[test]
fn native_fork_point_late_arrival_is_an_upsert_and_old_entries_remain_readable() {
    let before = entry("reply", None);
    let mut value = serde_json::to_value(&before).unwrap();
    assert!(value.get("nativeForkPoint").is_none());
    assert!(
        decode_entry_json(value.clone())
            .unwrap()
            .native_fork_point
            .is_none()
    );
    value["nativeForkPoint"] = serde_json::to_value(point()).unwrap();
    let after = decode_entry_json(value).unwrap();
    let frame = diff_transcript(&[before], &[after]);
    assert!(!frame.is_empty_delta());
    assert_eq!(point().source_session_id, "canonical");
}
