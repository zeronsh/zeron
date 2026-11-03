//! Actual 1.13.7 persisted bytes, shared with the edge compatibility tests.
use loro::{ExportMode, LoroDoc, ToJson};
use serde_json::{Value, json};

const SNAPSHOT: &[u8] = include_bytes!("fixtures/loro-1.13.7/snapshot.bin");
const SHALLOW: &[u8] = include_bytes!("fixtures/loro-1.13.7/shallow.bin");
const UPDATE: &[u8] = include_bytes!("fixtures/loro-1.13.7/update.bin");

#[test]
fn legacy_snapshots_merge_updates_and_survive_new_version_persistence() {
    for snapshot in [SNAPSHOT, SHALLOW] {
        let doc = LoroDoc::new();
        doc.import(snapshot).unwrap();
        let initial: Value =
            serde_json::from_str(include_str!("fixtures/loro-1.13.7/initial.json")).unwrap();
        assert_eq!(doc.get_deep_value().to_json_value(), initial);
        doc.get_map("metadata").insert("modern", true).unwrap();
        doc.commit();
        assert!(doc.import(UPDATE).unwrap().pending.is_none());
        let mut expected: Value =
            serde_json::from_str(include_str!("fixtures/loro-1.13.7/updated.json")).unwrap();
        expected["metadata"]["modern"] = json!(true);
        assert_eq!(doc.get_deep_value().to_json_value(), expected);

        let restored = LoroDoc::new();
        restored
            .import(&doc.export(ExportMode::Snapshot).unwrap())
            .unwrap();
        // Replay after restart must be idempotent, including movable-list ops.
        restored.import(UPDATE).unwrap();
        assert_eq!(restored.get_deep_value().to_json_value(), expected);
    }
}

#[test]
fn legacy_update_waiting_for_snapshot_is_not_lost() {
    let doc = LoroDoc::new();
    assert!(doc.import(UPDATE).unwrap().pending.is_some());
    doc.import(SNAPSHOT).unwrap();
    let expected: Value =
        serde_json::from_str(include_str!("fixtures/loro-1.13.7/updated.json")).unwrap();
    assert_eq!(doc.get_deep_value().to_json_value(), expected);
}
