//! RegistryDoc unit tests. The merge cases mirror
//! `edge/src/registry-core.test.ts` — shared vectors; change both together.
//! The typed-API cases mirror `workspace.rs`'s tests so the drop-in claim is
//! tested, not asserted.

use super::*;
use zeron_proto::{HarnessId, SandboxLevel, SessionStatus};

fn ts(ms: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_millis(ms).unwrap_or(DateTime::UNIX_EPOCH)
}

fn hlc(ms: i64) -> String {
    encode_hlc(ms, 0, "dev-a")
}

fn hlc_by(ms: i64, device: &str) -> String {
    encode_hlc(ms, 0, device)
}

fn upsert(set: &[(&str, Value)], at: i64) -> RowOp {
    RowOp {
        kind: "chats".into(),
        id: "chat-1".into(),
        op: OpKind::Upsert,
        set: Some(
            set.iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
        ),
        hlc: hlc(at),
        clocks: None,
    }
}

fn update(set: &[(&str, Value)], hlc: String) -> RowOp {
    RowOp {
        kind: "chats".into(),
        id: "chat-1".into(),
        op: OpKind::Update,
        set: Some(
            set.iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
        ),
        hlc,
        clocks: None,
    }
}

fn delete(at: i64) -> RowOp {
    RowOp {
        kind: "chats".into(),
        id: "chat-1".into(),
        op: OpKind::Delete,
        set: None,
        hlc: hlc(at),
        clocks: None,
    }
}

fn applied(row: Option<&RegistryRow>, op: &RowOp) -> RegistryRow {
    let (next, changed) = apply_op(row, op);
    assert!(changed, "expected op to change the row");
    next.expect("row after change")
}

// ── merge semantics (mirror of registry-core.test.ts) ───────────────────────

#[test]
fn hlc_orders_lexicographically() {
    assert!(hlc(2) > hlc(1));
    assert!(encode_hlc(1, 2, "a") > encode_hlc(1, 1, "a"));
    assert!(encode_hlc(1, 1, "b") > encode_hlc(1, 1, "a"));
    assert!(hlc(10_000) > hlc(999));
}

#[test]
fn hlc_clock_is_monotonic_across_regressions() {
    let mut clock = HlcClock::default();
    let a = clock.next(1_000, "d");
    let b = clock.next(500, "d"); // wall clock went backwards
    let c = clock.next(1_000, "d"); // and stalled
    assert!(b > a);
    assert!(c > b);
}

#[test]
fn upsert_creates_update_never_does() {
    let row = applied(None, &upsert(&[("title", json!("hello"))], 1000));
    assert_eq!(row.fields["title"], json!("hello"));
    let (missing, changed) = apply_op(None, &update(&[("title", json!("x"))], hlc(2000)));
    assert!(!changed);
    assert!(missing.is_none());
}

#[test]
fn field_lww_newer_wins_older_and_ties_lose() {
    let row = applied(
        None,
        &upsert(
            &[("title", json!("hello")), ("archived", json!(false))],
            1000,
        ),
    );
    let (_, changed) = apply_op(Some(&row), &update(&[("title", json!("stale"))], hlc(500)));
    assert!(!changed);
    // Exact replay: strict-> compare makes re-pushes idempotent.
    let (_, changed) = apply_op(
        Some(&row),
        &upsert(
            &[("title", json!("hello")), ("archived", json!(false))],
            1000,
        ),
    );
    assert!(!changed);
    let renamed = applied(
        Some(&row),
        &update(&[("title", json!("renamed"))], hlc_by(2000, "dev-b")),
    );
    assert_eq!(renamed.fields["title"], json!("renamed"));
    assert_eq!(renamed.clocks["archived"], hlc(1000));
}

#[test]
fn same_ms_conflicts_settle_by_device_deterministically() {
    let base = applied(None, &upsert(&[("title", json!("hello"))], 1000));
    let from_a = update(&[("title", json!("A"))], hlc_by(5000, "dev-a"));
    let from_b = update(&[("title", json!("B"))], hlc_by(5000, "dev-b"));
    let ab = apply_op(apply_op(Some(&base), &from_a).0.as_ref(), &from_b)
        .0
        .unwrap();
    let ba = apply_op(apply_op(Some(&base), &from_b).0.as_ref(), &from_a)
        .0
        .unwrap();
    assert_eq!(ab.fields["title"], json!("B"));
    assert_eq!(ba.fields["title"], json!("B"));
}

#[test]
fn null_deletes_fields_with_a_clock() {
    let row = applied(
        None,
        &upsert(&[("title", json!("x")), ("name", json!("y"))], 1000),
    );
    let row = applied(Some(&row), &update(&[("name", Value::Null)], hlc(2000)));
    assert!(!row.fields.contains_key("name"));
    assert_eq!(row.clocks["name"], hlc(2000));
    let (_, changed) = apply_op(Some(&row), &update(&[("name", json!("zombie"))], hlc(1500)));
    assert!(!changed);
}

#[test]
fn delete_only_wins_when_causally_newer() {
    let row = applied(None, &upsert(&[("title", json!("hello"))], 1000));
    let (_, changed) = apply_op(Some(&row), &delete(500));
    assert!(!changed);
    let gone = applied(Some(&row), &delete(2000));
    assert!(gone.deleted);
    assert!(gone.fields.is_empty());
    // Updates never touch tombstones.
    let (_, changed) = apply_op(
        Some(&gone),
        &update(&[("title", json!("ghost"))], hlc(3000)),
    );
    assert!(!changed);
    // Older upsert can't revive; newer revives from ONLY its own fields.
    let (_, changed) = apply_op(Some(&gone), &upsert(&[("title", json!("old"))], 1500));
    assert!(!changed);
    let revived = applied(Some(&gone), &upsert(&[("title", json!("back"))], 4000));
    assert!(!revived.deleted);
    assert_eq!(revived.fields.len(), 1);
    assert_eq!(revived.fields["title"], json!("back"));
}

#[test]
fn delete_on_missing_plants_guard_tombstone() {
    let gone = applied(None, &delete(1000));
    assert!(gone.deleted);
    let (_, changed) = apply_op(Some(&gone), &upsert(&[("title", json!("late"))], 500));
    assert!(!changed);
}

#[test]
fn seed_ops_preserve_original_causality() {
    let row = applied(
        None,
        &upsert(&[("title", json!("old")), ("status", json!("idle"))], 1000),
    );
    let row = applied(
        Some(&row),
        &update(&[("status", json!("working"))], hlc(9000)),
    );
    let seeded = applied(None, &row_to_seed_op(&row));
    assert_eq!(seeded.fields, row.fields);
    assert_eq!(seeded.clocks, row.clocks);
    let (_, changed) = apply_op(
        Some(&seeded),
        &update(&[("status", json!("errored"))], hlc(5000)),
    );
    assert!(!changed);
    // Tombstones round-trip too.
    let gone = applied(None, &delete(7000));
    let reseeded = applied(None, &row_to_seed_op(&gone));
    assert!(reseeded.deleted);
    assert_eq!(reseeded.del_hlc.as_deref(), Some(hlc(7000).as_str()));
}

#[test]
fn wire_shapes_match_the_edge() {
    // The exact JSON the TS side produces/consumes (registry-core.ts).
    let op: RowOp = serde_json::from_value(json!({
        "kind": "chats", "id": "chat-1", "op": "upsert",
        "set": {"title": "hi", "gone": null},
        "hlc": "0000000001000-000000-dev-a",
        "clocks": {"title": "0000000002000-000000-dev-b"}
    }))
    .unwrap();
    assert_eq!(op.op, OpKind::Upsert);
    assert_eq!(op.clock_for("title"), "0000000002000-000000-dev-b");
    assert_eq!(op.clock_for("gone"), "0000000001000-000000-dev-a");
    let row: RegistryRow = serde_json::from_value(json!({
        "kind": "chats", "id": "chat-1", "seq": 7, "deleted": false,
        "fields": {"title": "hi"}, "clocks": {"title": "0000000001000-000000-dev-a"}
    }))
    .unwrap();
    assert_eq!(row.seq, 7);
    let back = serde_json::to_value(&row).unwrap();
    assert_eq!(back["delHlc"], Value::Null); // skipped, not "del_hlc"
    assert!(back.get("del_hlc").is_none());
    let del: RowOp = serde_json::from_value(json!({
        "kind": "chats", "id": "c", "op": "delete", "hlc": "0000000001000-000000-d"
    }))
    .unwrap();
    assert_eq!(del.op, OpKind::Delete);
}

// ── doc lifecycle ───────────────────────────────────────────────────────────

fn device(id: &str, name: &str) -> Device {
    Device {
        id: id.into(),
        name: name.into(),
        platform: "linux".into(),
        last_seen_at: Some(ts(1_000)),
        created_at: Some(ts(500)),
        version: Some("0.1.0".into()),
        cursor_sdk_version: Some("1.0.31".into()),
        capabilities: Vec::new(),
    }
}

fn chat(id: &str, device_id: &str) -> Chat {
    Chat {
        id: id.into(),
        device_id: device_id.into(),
        title: Some("First chat".into()),
        archived: false,
        cwd: Some("/tmp/repo".into()),
        branch: Some("main".into()),
        checkout_id: None,
        source_context: None,
        config: Some(ChatConfig {
            harness: HarnessId::Mock,
            model: Some("mock-1".into()),
            reasoning: None,
            model_options: Default::default(),
            sandbox: SandboxLevel::WorkspaceWrite,
        }),
        last_message_preview: None,
        last_message_at: None,
        created_at: ts(2_000),
        harness_session_id: None,
        harness_session_cwd: None,
        parent_chat_id: Some("parent-chat".into()),
        space_id: None,
        last_seen_at: None,
        room_gen: None,
    }
}

fn space(id: &str, device_id: &str, path: &str) -> Space {
    Space {
        id: id.into(),
        device_id: device_id.into(),
        path: path.into(),
        name: None,
        git_detected: false,
        git_checked_at: None,
        checkout_id: None,
        repository_id: None,
        created_at: ts(1_500),
    }
}

fn session(chat_id: &str, device_id: &str, status: SessionStatus) -> Session {
    Session {
        last_completed_turn: None,
        running_subagents: 0,
        chat_id: chat_id.into(),
        device_id: device_id.into(),
        status,
        started_at: Some(ts(3_000)),
        updated_at: ts(3_500),
    }
}

/// Stand-in for the server: applies every pushable batch from `docs` to a row
/// table with the SAME merge fn, acks, and broadcasts merged rows to all.
fn server_round(
    server: &mut HashMap<(String, String), RegistryRow>,
    seq: &mut u64,
    docs: &mut [&mut RegistryDoc],
) {
    // Collect each doc's pushable batches first (server applies in order).
    let mut acks: Vec<(usize, String)> = Vec::new();
    let mut touched_all: Vec<RegistryRow> = Vec::new();
    let mut batches: Vec<(usize, PendingBatch)> = Vec::new();
    for (i, doc) in docs.iter_mut().enumerate() {
        for batch in doc.take_pushable() {
            batches.push((i, batch));
        }
    }
    for (i, batch) in batches {
        *seq += 1;
        let mut touched: Vec<RegistryRow> = Vec::new();
        for op in &batch.ops {
            let key = (op.kind.clone(), op.id.clone());
            let (next, changed) = apply_op(server.get(&key), op);
            if let Some(mut next) = next
                && changed
            {
                next.seq = *seq;
                server.insert(key, next.clone());
                touched.retain(|r| !(r.kind == next.kind && r.id == next.id));
                touched.push(next);
            }
        }
        acks.push((i, batch.batch));
        touched_all.extend(touched);
    }
    for doc in docs.iter_mut() {
        let _ = doc.apply_rows(*seq, touched_all.clone());
    }
    for (i, batch) in acks {
        docs[i].ack_batch(&batch, *seq);
    }
}

#[test]
fn rows_round_trip_and_upsert_refreshes() {
    let mut doc = RegistryDoc::new("dev-a");
    let mut device = device("dev-a", "laptop");
    device.capabilities = vec![zeron_proto::capabilities::MESSAGE_QUEUE_V1.into()];
    doc.upsert_device(&device).unwrap();
    doc.upsert_chat(&chat("chat-1", "dev-a")).unwrap();
    doc.upsert_session(&session("chat-1", "dev-a", SessionStatus::Working))
        .unwrap();

    let state = doc.read_all().unwrap();
    assert_eq!(state.devices, vec![device]);
    assert_eq!(state.chats, vec![chat("chat-1", "dev-a")]);
    assert_eq!(
        state.sessions,
        vec![session("chat-1", "dev-a", SessionStatus::Working)]
    );

    let mut updated = chat("chat-1", "dev-a");
    updated.title = None;
    updated.last_message_preview = Some("hello".into());
    updated.last_message_at = Some(ts(9_000));
    doc.upsert_chat(&updated).unwrap();
    let chats = doc.read_chats().unwrap();
    assert_eq!(chats.len(), 1);
    assert_eq!(chats[0].title, None);
    assert_eq!(chats[0].last_message_preview.as_deref(), Some("hello"));
}

#[test]
fn own_push_ack_never_advances_the_cursor() {
    // Field incident: on the HTTPS transport, push ran before pull in one
    // cycle — the ack jumped the cursor to OUR batch's seq, and the pull
    // then fetched "since" that jumped cursor, permanently skipping every
    // batch other devices wrote in between (new sessions never appeared in
    // a laptop's sidebar while updates to known rows kept flowing).
    let mut ws = RegistryDoc::new("dev-a");
    ws.apply_state(10, true, 0, Vec::new());
    assert_eq!(ws.cursor(), 10);
    ws.upsert_chat(&chat("chat-1", "dev-a")).unwrap();
    let batch = ws.take_pushable().pop().expect("pending batch");
    // The server assigns our batch seq 15 — batches 11..=14 belong to peers
    // and have not reached us yet.
    assert!(ws.ack_batch(&batch.batch, 15));
    assert_eq!(
        ws.cursor(),
        10,
        "the next pull must still fetch the peers' batches 11..=14"
    );
    assert_eq!(ws.pending_len(), 0, "the ack still retires the batch");
}

#[test]
fn broadcast_seq_gap_applies_rows_but_holds_the_cursor() {
    let mut ws = RegistryDoc::new("dev-a");
    ws.apply_state(10, true, 0, Vec::new());
    let row = |id: &str, seq: u64| RegistryRow {
        kind: "chats".into(),
        id: id.into(),
        seq,
        deleted: false,
        del_hlc: None,
        fields: [
            ("id".to_owned(), json!(id)),
            ("deviceId".to_owned(), json!("dev-b")),
            ("createdAt".to_owned(), json!(1_000)),
        ]
        .into_iter()
        .collect(),
        clocks: Default::default(),
    };
    // Contiguous broadcast advances.
    assert!(ws.apply_rows(11, vec![row("chat-a", 11)]));
    assert_eq!(ws.cursor(), 11);
    // A GAPPED broadcast (12..=14 were missed) applies its rows but must
    // not advance — the caller resyncs and the reconnect hello backfills.
    assert!(!ws.apply_rows(15, vec![row("chat-b", 15)]));
    assert_eq!(ws.cursor(), 11, "cursor holds across the gap");
    assert_eq!(ws.read_chats().unwrap().len(), 2, "fresh rows still apply");
    // Idempotent replay of an already-seen batch is not a gap.
    assert!(ws.apply_rows(11, vec![row("chat-a", 11)]));
}

#[test]
fn pre_epoch_snapshots_resync_in_full_once() {
    let mut ws = RegistryDoc::new("dev-a");
    ws.apply_state(42, true, 0, Vec::new());
    ws.upsert_chat(&chat("chat-1", "dev-a")).unwrap();
    let bytes = ws.to_bytes().unwrap();

    // A current-epoch snapshot keeps its cursor.
    let restored = RegistryDoc::from_bytes(&bytes, "dev-a").unwrap();
    assert_eq!(restored.cursor(), 42);
    assert_eq!(restored.read_chats().unwrap().len(), 1);

    // A snapshot from BEFORE the cursor-integrity fixes (no epoch field)
    // may hide a jumped cursor: it zeroes on load so the next hello/pull is
    // a full state, healing any invisible-row window. Rows are kept.
    let mut v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    v.as_object_mut().unwrap().remove("resyncEpoch");
    let old = serde_json::to_vec(&v).unwrap();
    let healed = RegistryDoc::from_bytes(&old, "dev-a").unwrap();
    assert_eq!(
        healed.cursor(),
        0,
        "pre-epoch snapshot forces a full resync"
    );
    assert_eq!(
        healed.read_chats().unwrap().len(),
        1,
        "rows survive the reset"
    );
}

#[test]
fn future_harness_chat_rows_stay_visible_without_their_config() {
    // Field incident: a pre-v0.2.10 client received rows whose
    // config.harness said "opencode" — a variant it didn't have — and
    // dropped the WHOLE row ("skipping malformed registry row"), so new
    // sessions silently never appeared in that device's sidebar. Unknown
    // config values must cost the config, not the row.
    let mut ws = RegistryDoc::new("dev-a");
    ws.upsert_chat(&chat("chat-1", "dev-a")).unwrap();
    let fields: std::collections::BTreeMap<String, serde_json::Value> = [
        ("id".to_owned(), json!("chat-2")),
        ("deviceId".to_owned(), json!("dev-b")),
        ("createdAt".to_owned(), json!(1_000)),
        (
            "config".to_owned(),
            json!({
                "harness": "harness-from-the-future",
                "model": "novel/model",
                "sandbox": "workspace-write",
            }),
        ),
    ]
    .into_iter()
    .collect();
    let _ = ws.apply_rows(
        7,
        vec![RegistryRow {
            kind: "chats".into(),
            id: "chat-2".into(),
            seq: 7,
            deleted: false,
            del_hlc: None,
            fields,
            clocks: Default::default(),
        }],
    );

    let chats = ws.read_chats().unwrap();
    assert_eq!(chats.len(), 2, "the future-harness row must not vanish");
    let newcomer = chats.iter().find(|c| c.id == "chat-2").expect("visible");
    assert_eq!(
        newcomer.config, None,
        "unknown config degrades, row survives"
    );
    // The well-formed sibling keeps its config untouched.
    assert!(chats.iter().any(|c| c.id == "chat-1" && c.config.is_some()));
}

#[test]
fn field_mutators_round_trip() {
    let mut ws = RegistryDoc::new("dev-a");
    ws.upsert_device(&device("dev-a", "laptop")).unwrap();
    ws.upsert_chat(&chat("chat-1", "dev-a")).unwrap();

    assert!(ws.rename_chat("chat-1", "Renamed").unwrap());
    assert!(ws.set_chat_archived("chat-1", true).unwrap());
    assert!(
        ws.set_chat_last_message("chat-1", "preview text", ts(5_000))
            .unwrap()
    );
    assert!(ws.rename_device("dev-a", "workstation").unwrap());
    assert!(ws.set_device_last_seen("dev-a", ts(6_000)).unwrap());
    assert!(!ws.rename_chat("nope", "x").unwrap());
    assert!(!ws.set_chat_archived("nope", true).unwrap());
    assert!(!ws.rename_device("nope", "x").unwrap());

    let chat = ws.chat("chat-1").unwrap().unwrap();
    assert_eq!(chat.title.as_deref(), Some("Renamed"));
    assert!(chat.archived);
    assert_eq!(chat.last_message_preview.as_deref(), Some("preview text"));
    assert_eq!(chat.last_message_at, Some(ts(5_000)));
    let dev = &ws.read_devices().unwrap()[0];
    assert_eq!(dev.cursor_sdk_version.as_deref(), Some("1.0.31"));
    assert_eq!(dev.name, "workstation");
    assert_eq!(dev.last_seen_at, Some(ts(6_000)));
}

#[test]
fn delete_chat_tombstones_row_and_session() {
    let mut ws = RegistryDoc::new("dev-a");
    ws.upsert_chat(&chat("chat-1", "dev-a")).unwrap();
    ws.upsert_session(&session("chat-1", "dev-a", SessionStatus::Idle))
        .unwrap();
    assert!(ws.delete_chat("chat-1").unwrap());
    assert!(ws.read_chats().unwrap().is_empty());
    assert!(ws.read_sessions().unwrap().is_empty());
    assert!(!ws.delete_chat("chat-1").unwrap());
}

#[test]
fn spaces_round_trip_and_mutate() {
    let mut ws = RegistryDoc::new("dev-a");
    ws.upsert_space(&space("sp-1", "dev-a", "/home/u/project"))
        .unwrap();
    let row = ws.space("sp-1").unwrap().expect("row exists");
    assert_eq!(row.display_name(), "project");
    assert!(!row.git_detected);

    assert!(ws.rename_space("sp-1", Some("My Project")).unwrap());
    assert_eq!(
        ws.space("sp-1").unwrap().unwrap().display_name(),
        "My Project"
    );
    assert!(ws.rename_space("sp-1", None).unwrap());
    assert_eq!(ws.space("sp-1").unwrap().unwrap().display_name(), "project");

    assert!(
        ws.set_space_git(
            "sp-1",
            true,
            Some("checkout-abc"),
            Some("github.com/owner/project"),
            ts(4_000),
        )
        .unwrap()
    );
    let row = ws.space("sp-1").unwrap().unwrap();
    assert!(row.git_detected);
    assert_eq!(row.checkout_id.as_deref(), Some("checkout-abc"));
    assert_eq!(
        row.repository_id.as_deref(),
        Some("github.com/owner/project")
    );
    assert_eq!(row.git_checked_at, Some(ts(4_000)));

    assert!(!ws.rename_space("nope", Some("x")).unwrap());
    assert!(!ws.set_space_git("nope", true, None, None, ts(1)).unwrap());
}

#[test]
fn chat_seen_is_monotonic() {
    let mut ws = RegistryDoc::new("dev-a");
    ws.upsert_chat(&chat("chat-1", "dev-a")).unwrap();
    assert!(ws.set_chat_seen("chat-1", ts(5_000)).unwrap());
    assert_eq!(
        ws.chat("chat-1").unwrap().unwrap().last_seen_at,
        Some(ts(5_000))
    );
    // Older stamps are ignored without a write.
    let before = ws.pending_len();
    assert!(ws.set_chat_seen("chat-1", ts(4_000)).unwrap());
    assert_eq!(ws.pending_len(), before);
    assert_eq!(
        ws.chat("chat-1").unwrap().unwrap().last_seen_at,
        Some(ts(5_000))
    );
    assert!(!ws.set_chat_seen("nope", ts(1)).unwrap());
}

#[test]
fn two_docs_converge_through_a_server() {
    let mut a = RegistryDoc::new("dev-a");
    let mut b = RegistryDoc::new("dev-b");
    a.upsert_device(&device("dev-a", "laptop")).unwrap();
    a.upsert_chat(&chat("chat-a", "dev-a")).unwrap();
    a.upsert_session(&session("chat-a", "dev-a", SessionStatus::Working))
        .unwrap();
    b.upsert_device(&device("dev-b", "vps")).unwrap();
    b.upsert_chat(&chat("chat-b", "dev-b")).unwrap();

    let mut server = HashMap::new();
    let mut seq = 0u64;
    server_round(&mut server, &mut seq, &mut [&mut a, &mut b]);

    let sa = a.read_all().unwrap();
    let sb = b.read_all().unwrap();
    assert_eq!(sa, sb);
    assert_eq!(
        sa.devices.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(),
        vec!["dev-a", "dev-b"]
    );
    assert_eq!(
        sa.chats.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
        vec!["chat-a", "chat-b"]
    );
    assert_eq!(sa.sessions.len(), 1);
    assert_eq!(a.pending_len(), 0);
    assert_eq!(b.pending_len(), 0);

    // Concurrent same-field rename settles identically on both.
    a.rename_chat("chat-a", "from a").unwrap();
    b.rename_chat("chat-a", "from b").unwrap();
    server_round(&mut server, &mut seq, &mut [&mut a, &mut b]);
    let title_a = a.chat("chat-a").unwrap().unwrap().title;
    let title_b = b.chat("chat-a").unwrap().unwrap().title;
    assert_eq!(title_a, title_b);
    assert!(matches!(
        title_a.as_deref(),
        Some("from a") | Some("from b")
    ));
}

/// The command plane outruns the registry channel: the host's
/// claim-on-first-command lands with NEWER clocks than the viewer's earlier
/// `createChat`. The claim writes only the fields it knows, so the viewer's
/// `config`/`title` must still land on merge (a full-row claim's `Null`
/// writes deleted them — the missing-harness-icon clobber).
#[test]
fn late_create_chat_config_survives_a_prior_claim() {
    let mut viewer = RegistryDoc::new("dev-viewer");
    let mut host = RegistryDoc::new("dev-a");

    // Viewer writes the real row first (older clocks)…
    viewer.upsert_chat(&chat("chat-race", "dev-a")).unwrap();
    // …then the host claims the same chat before that row reaches it. (Real
    // claims trail by whole seconds; the sleep keeps the HLCs out of the
    // same-millisecond device-id tiebreak.)
    std::thread::sleep(std::time::Duration::from_millis(2));
    host.claim_chat("chat-race", Some("/tmp/repo"), Some("space-1"), ts(9_000));

    let mut server = HashMap::new();
    let mut seq = 0u64;
    server_round(&mut server, &mut seq, &mut [&mut viewer, &mut host]);

    for doc in [&viewer, &host] {
        let merged = doc.chat("chat-race").unwrap().expect("merged row");
        // Claimed fields (newer clocks) stand…
        assert_eq!(merged.device_id, "dev-a");
        assert_eq!(merged.space_id.as_deref(), Some("space-1"));
        // …and the fields the claim never wrote come from the createChat.
        assert_eq!(merged.config, chat("chat-race", "dev-a").config);
        assert_eq!(merged.title.as_deref(), Some("First chat"));
    }
}

#[test]
fn delete_space_cascades_and_converges() {
    let mut a = RegistryDoc::new("dev-a");
    a.upsert_space(&space("sp-1", "dev-a", "/tmp/one")).unwrap();
    a.upsert_space(&space("sp-2", "dev-a", "/tmp/two")).unwrap();
    let mut in_space = chat("chat-1", "dev-a");
    in_space.space_id = Some("sp-1".into());
    let mut other = chat("chat-2", "dev-a");
    other.space_id = Some("sp-2".into());
    a.upsert_chat(&in_space).unwrap();
    a.upsert_chat(&other).unwrap();
    a.upsert_session(&session("chat-1", "dev-a", SessionStatus::Working))
        .unwrap();
    let mut b = RegistryDoc::new("dev-b");
    let mut server = HashMap::new();
    let mut seq = 0u64;
    server_round(&mut server, &mut seq, &mut [&mut a, &mut b]);
    assert_eq!(a.read_all().unwrap(), b.read_all().unwrap());

    let deleted = a.delete_space("sp-1").unwrap();
    assert!(deleted.existed);
    assert_eq!(deleted.chat_ids, vec!["chat-1".to_string()]);
    // Overlay hides the cascade locally before the server even sees it.
    assert_eq!(a.read_spaces().unwrap().len(), 1);
    server_round(&mut server, &mut seq, &mut [&mut a, &mut b]);

    for ws in [&a, &b] {
        let state = ws.read_all().unwrap();
        assert_eq!(
            state
                .spaces
                .iter()
                .map(|s| s.id.as_str())
                .collect::<Vec<_>>(),
            vec!["sp-2"]
        );
        assert_eq!(
            state
                .chats
                .iter()
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>(),
            vec!["chat-2"]
        );
        assert!(state.sessions.is_empty());
    }
    let again = b.delete_space("sp-1").unwrap();
    assert!(!again.existed);
    assert!(again.chat_ids.is_empty());
}

#[test]
fn persistence_round_trips_rows_pending_and_cursor() {
    let mut doc = RegistryDoc::new("dev-a");
    doc.upsert_chat(&chat("chat-1", "dev-a")).unwrap();
    // Half-sync: authoritative row + one unacked rename.
    let mut server = HashMap::new();
    let mut seq = 0u64;
    server_round(&mut server, &mut seq, &mut [&mut doc]);
    doc.rename_chat("chat-1", "unacked").unwrap();
    assert_eq!(doc.pending_len(), 1);

    let bytes = doc.to_bytes().unwrap();
    let restored = RegistryDoc::from_bytes(&bytes, "dev-a").unwrap();
    assert_eq!(restored.cursor(), doc.cursor());
    assert_eq!(restored.pending_len(), 1);
    assert_eq!(restored.read_all().unwrap(), doc.read_all().unwrap());
    assert_eq!(
        restored.chat("chat-1").unwrap().unwrap().title.as_deref(),
        Some("unacked")
    );
}

#[test]
fn state_frames_delta_replace_and_reseed() {
    let mut doc = RegistryDoc::new("dev-a");
    doc.upsert_chat(&chat("chat-1", "dev-a")).unwrap();
    let mut server = HashMap::new();
    let mut seq = 0u64;
    server_round(&mut server, &mut seq, &mut [&mut doc]);
    assert_eq!(doc.cursor(), seq);

    // Delta: a row someone else pushed.
    let remote = applied(
        None,
        &RowOp {
            kind: "chats".into(),
            id: "chat-9".into(),
            op: OpKind::Upsert,
            set: Some(
                [
                    ("id".to_string(), json!("chat-9")),
                    ("deviceId".to_string(), json!("dev-b")),
                    ("createdAt".to_string(), json!(1)),
                ]
                .into(),
            ),
            hlc: hlc_by(50, "dev-b"),
            clocks: None,
        },
    );
    let outcome = doc.apply_state(seq + 1, false, 0, vec![remote.clone()]);
    assert_eq!(outcome, StateOutcome::Delta);
    assert_eq!(doc.read_chats().unwrap().len(), 2);
    assert_eq!(doc.cursor(), seq + 1);

    // Full replace at a newer seq drops rows the server no longer has…
    // …except it re-seeds rows the server never saw. Here the server returns
    // only chat-9, so chat-1 (authoritative locally) re-seeds.
    let outcome = doc.apply_state(seq + 2, true, 0, vec![remote]);
    assert_eq!(outcome, StateOutcome::Reseeded);
    assert!(doc.pending_len() > 0);
    let seeded = doc
        .chat("chat-1")
        .unwrap()
        .expect("kept via reseed overlay");
    assert_eq!(seeded.device_id, "dev-a");

    // Server behind us (wiped): local rows kept, reseed enqueued.
    let mut fresh = RegistryDoc::new("dev-a");
    fresh.upsert_chat(&chat("chat-1", "dev-a")).unwrap();
    let mut server2 = HashMap::new();
    let mut seq2 = 0u64;
    server_round(&mut server2, &mut seq2, &mut [&mut fresh]);
    assert_eq!(fresh.pending_len(), 0);
    let outcome = fresh.apply_state(0, true, 0, vec![]);
    assert_eq!(outcome, StateOutcome::Reseeded);
    assert!(fresh.pending_len() > 0);
    assert_eq!(fresh.read_chats().unwrap().len(), 1);
}

#[test]
fn reconnect_replay_is_idempotent() {
    let mut doc = RegistryDoc::new("dev-a");
    doc.upsert_chat(&chat("chat-1", "dev-a")).unwrap();
    let mut server = HashMap::new();
    let mut seq = 0u64;
    // Push once, but the ack is "lost": simulate by applying to the server
    // without acking the doc.
    let batches = doc.take_pushable();
    for batch in &batches {
        seq += 1;
        for op in &batch.ops {
            let key = (op.kind.clone(), op.id.clone());
            let (next, changed) = apply_op(server.get(&key), op);
            if let Some(mut next) = next
                && changed
            {
                next.seq = seq;
                server.insert(key, next);
            }
        }
    }
    // Reconnect: batches become pushable again and re-apply as no-ops.
    doc.mark_disconnected();
    let replay = doc.take_pushable();
    assert_eq!(replay.len(), batches.len());
    for batch in &replay {
        for op in &batch.ops {
            let key = (op.kind.clone(), op.id.clone());
            let (_, changed) = apply_op(server.get(&key), op);
            assert!(!changed, "replayed op must be a no-op");
        }
    }
}

fn pin_sessions(doc: &mut RegistryDoc, ids: &[&str]) {
    for id in ids {
        if doc.chat(id).unwrap().is_none() {
            doc.upsert_chat(&chat(id, "desktop")).unwrap();
        }
        doc.change_sidebar_pin(&zeron_proto::SidebarPinChange::Pin {
            session_id: (*id).into(),
            after: None,
            before: None,
        })
        .unwrap();
    }
}

#[test]
fn sidebar_preferences_preserve_unknown_empty_and_ordered_states() {
    let mut doc = RegistryDoc::new("dev-a");
    assert_eq!(doc.sidebar_preferences(), None);
    assert!(!doc.reconcile_sidebar_pins(false).unwrap());
    assert!(doc.reconcile_sidebar_pins(true).unwrap());
    assert!(
        doc.sidebar_preferences()
            .unwrap()
            .pinned_session_ids
            .is_empty()
    );
    pin_sessions(&mut doc, &["chat-b", "chat-a"]);
    assert_eq!(
        doc.sidebar_preferences().unwrap().pinned_session_ids,
        ["chat-b", "chat-a"]
    );
}

#[test]
fn sidebar_cleanup_waits_for_authority_and_keeps_archived_pins() {
    let mut doc = RegistryDoc::new("desktop");
    pin_sessions(&mut doc, &["live", "archived", "deleted"]);
    let mut archived = chat("archived", "desktop");
    archived.archived = true;
    doc.upsert_chat(&archived).unwrap();
    doc.delete_chat("deleted").unwrap();
    assert!(!doc.reconcile_sidebar_pins(false).unwrap());
    assert_eq!(
        doc.sidebar_preferences().unwrap().pinned_session_ids.len(),
        3
    );
    assert!(doc.reconcile_sidebar_pins(true).unwrap());
    assert_eq!(
        doc.sidebar_preferences().unwrap().pinned_session_ids,
        ["live", "archived"]
    );
    assert!(!doc.reconcile_sidebar_pins(true).unwrap());
}

#[test]
fn sidebar_ignores_old_whole_list_preferences() {
    let mut doc = RegistryDoc::new("desktop");
    doc.upsert_chat(&chat("old", "desktop")).unwrap();
    doc.write(
        KIND_PREFERENCES,
        "sidebar-v1",
        OpKind::Upsert,
        fields([("pinnedSessionIds", json!(["old"]))]),
    );
    assert!(doc.sidebar_preferences().is_none());
    doc.reconcile_sidebar_pins(true).unwrap();
    assert!(
        doc.sidebar_preferences()
            .unwrap()
            .pinned_session_ids
            .is_empty()
    );
    pin_sessions(&mut doc, &["new"]);
    assert_eq!(
        doc.sidebar_preferences().unwrap().pinned_session_ids,
        ["new"]
    );
}

#[test]
fn sidebar_rejects_invalid_or_missing_sessions_before_writing() {
    use zeron_proto::SidebarPinChange;
    let mut doc = RegistryDoc::new("dev-a");
    for id in ["", "invalid id", "missing"] {
        assert!(
            doc.change_sidebar_pin(&SidebarPinChange::Pin {
                session_id: id.into(),
                after: None,
                before: None
            })
            .is_err()
        );
        assert!(doc.sidebar_preferences().is_none());
    }
}

#[test]
fn sidebar_independent_moves_merge_and_only_write_the_moved_pin() {
    use zeron_proto::SidebarPinChange;
    let mut desktop = RegistryDoc::new("desktop");
    let mut phone = RegistryDoc::new("phone");
    let mut server = HashMap::new();
    let mut seq = 0;
    for id in ["a", "b", "c", "d"] {
        desktop.upsert_chat(&chat(id, "desktop")).unwrap();
    }
    pin_sessions(&mut desktop, &["a", "b", "c", "d"]);
    server_round(&mut server, &mut seq, &mut [&mut desktop, &mut phone]);
    desktop
        .change_sidebar_pin(&SidebarPinChange::Move {
            session_id: "d".into(),
            after: None,
            before: Some("a".into()),
        })
        .unwrap();
    phone
        .change_sidebar_pin(&SidebarPinChange::Move {
            session_id: "b".into(),
            after: Some("d".into()),
            before: None,
        })
        .unwrap();
    for doc in [&desktop, &phone] {
        assert_eq!(doc.pending.len(), 1);
        let ops = &doc.pending[0].ops;
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].kind, KIND_SIDEBAR_PINS);
        assert_eq!(
            ops[0].set.as_ref().unwrap().keys().collect::<Vec<_>>(),
            vec!["orderKey"]
        );
    }
    server_round(&mut server, &mut seq, &mut [&mut desktop, &mut phone]);
    assert_eq!(desktop.sidebar_preferences(), phone.sidebar_preferences());
    assert_eq!(
        desktop.sidebar_preferences().unwrap().pinned_session_ids,
        ["d", "a", "c", "b"]
    );
}

#[test]
fn sidebar_unpin_survives_concurrent_move_and_replayed_pin() {
    use zeron_proto::SidebarPinChange;
    for reverse in [false, true] {
        let mut a = RegistryDoc::new("a");
        let mut b = RegistryDoc::new("b");
        let mut server = HashMap::new();
        let mut seq = 0;
        a.upsert_chat(&chat("pin", "a")).unwrap();
        pin_sessions(&mut a, &["pin"]);
        let seed = a.pending.last().unwrap().clone();
        server_round(&mut server, &mut seq, &mut [&mut a, &mut b]);
        a.change_sidebar_pin(&SidebarPinChange::Unpin {
            session_id: "pin".into(),
        })
        .unwrap();
        b.change_sidebar_pin(&SidebarPinChange::Move {
            session_id: "pin".into(),
            after: None,
            before: None,
        })
        .unwrap();
        if reverse {
            server_round(&mut server, &mut seq, &mut [&mut b, &mut a]);
        } else {
            server_round(&mut server, &mut seq, &mut [&mut a, &mut b]);
        }
        b.enqueue_ops(seed.ops);
        server_round(&mut server, &mut seq, &mut [&mut a, &mut b]);
        assert_eq!(a.sidebar_preferences(), b.sidebar_preferences());
        assert!(
            a.sidebar_preferences()
                .unwrap()
                .pinned_session_ids
                .is_empty()
        );
    }
}

#[test]
fn sidebar_concurrent_additions_keep_overflow_and_allow_removal_and_moves() {
    use zeron_proto::SidebarPinChange;
    let mut a = RegistryDoc::new("a");
    let mut b = RegistryDoc::new("b");
    let mut server = HashMap::new();
    let mut seq = 0;
    let ids: Vec<_> = (0..199).map(|i| format!("pin-{i}")).collect();
    for id in ids
        .iter()
        .map(String::as_str)
        .chain(["extra-a", "extra-b", "extra-c"])
    {
        a.upsert_chat(&chat(id, "a")).unwrap();
    }
    pin_sessions(&mut a, &ids.iter().map(String::as_str).collect::<Vec<_>>());
    server_round(&mut server, &mut seq, &mut [&mut a, &mut b]);
    for (doc, id) in [(&mut a, "extra-a"), (&mut b, "extra-b")] {
        doc.change_sidebar_pin(&SidebarPinChange::Pin {
            session_id: id.into(),
            after: None,
            before: None,
        })
        .unwrap();
    }
    server_round(&mut server, &mut seq, &mut [&mut a, &mut b]);
    assert_eq!(
        a.sidebar_preferences().unwrap().pinned_session_ids.len(),
        201
    );
    assert!(!a.reconcile_sidebar_pins(true).unwrap());
    assert!(
        a.change_sidebar_pin(&SidebarPinChange::Pin {
            session_id: "extra-c".into(),
            after: None,
            before: None
        })
        .is_err()
    );
    a.change_sidebar_pin(&SidebarPinChange::Move {
        session_id: "extra-a".into(),
        after: None,
        before: Some("pin-0".into()),
    })
    .unwrap();
    a.change_sidebar_pin(&SidebarPinChange::Unpin {
        session_id: "extra-b".into(),
    })
    .unwrap();
    server_round(&mut server, &mut seq, &mut [&mut a, &mut b]);
    assert_eq!(
        a.sidebar_preferences().unwrap().pinned_session_ids.len(),
        200
    );
    assert_eq!(a.sidebar_preferences(), b.sidebar_preferences());
}

#[test]
fn sidebar_keys_and_pending_membership_survive_restart() {
    use zeron_proto::SidebarPinChange;
    let mut a = RegistryDoc::new("a");
    a.upsert_chat(&chat("pin", "a")).unwrap();
    a.change_sidebar_pin(&SidebarPinChange::Pin {
        session_id: "pin".into(),
        after: None,
        before: None,
    })
    .unwrap();
    let reopened = RegistryDoc::from_bytes(&a.to_bytes().unwrap(), "a").unwrap();
    assert_eq!(a.sidebar_preferences(), reopened.sidebar_preferences());
    assert_eq!(a.ordered_sidebar_pins(), reopened.ordered_sidebar_pins());
}

#[test]
fn sidebar_local_edits_follow_an_observed_future_clock() {
    use zeron_proto::SidebarPinChange;
    let mut doc = RegistryDoc::new("local");
    pin_sessions(&mut doc, &["pin"]);
    let remote = RowOp {
        kind: KIND_SIDEBAR_PINS.into(),
        id: "pin".into(),
        op: OpKind::Upsert,
        set: Some(fields([("pinned", json!(true)), ("orderKey", json!("8"))])),
        hlc: "9999999999999-000001-remote".into(),
        clocks: None,
    };
    let mut row = apply_op(None, &remote).0.unwrap();
    row.seq = 1;
    let _ = doc.apply_rows(1, vec![row]);
    doc.change_sidebar_pin(&SidebarPinChange::Unpin {
        session_id: "pin".into(),
    })
    .unwrap();
    assert!(
        doc.sidebar_preferences()
            .unwrap()
            .pinned_session_ids
            .is_empty()
    );
    doc.change_sidebar_pin(&SidebarPinChange::Pin {
        session_id: "pin".into(),
        after: None,
        before: None,
    })
    .unwrap();
    assert_eq!(
        doc.sidebar_preferences().unwrap().pinned_session_ids,
        ["pin"]
    );
    assert!(doc.pending.last().unwrap().ops[0].hlc > remote.hlc);
}

#[test]
fn sidebar_concurrent_moves_of_one_pin_converge_by_clock_in_either_order() {
    use zeron_proto::SidebarPinChange;
    for reverse in [false, true] {
        let mut a = RegistryDoc::new("a");
        let mut b = RegistryDoc::new("b");
        let mut server = HashMap::new();
        let mut seq = 0;
        pin_sessions(&mut a, &["first", "moved", "last"]);
        server_round(&mut server, &mut seq, &mut [&mut a, &mut b]);
        a.change_sidebar_pin(&SidebarPinChange::Move {
            session_id: "moved".into(),
            after: None,
            before: Some("first".into()),
        })
        .unwrap();
        b.change_sidebar_pin(&SidebarPinChange::Move {
            session_id: "moved".into(),
            after: Some("last".into()),
            before: None,
        })
        .unwrap();
        let a_wins = a.pending[0].ops[0].hlc > b.pending[0].ops[0].hlc;
        let expected = if a_wins {
            ["moved", "first", "last"]
        } else {
            ["first", "last", "moved"]
        };
        if reverse {
            server_round(&mut server, &mut seq, &mut [&mut b, &mut a]);
        } else {
            server_round(&mut server, &mut seq, &mut [&mut a, &mut b]);
        }
        assert_eq!(a.sidebar_preferences(), b.sidebar_preferences());
        assert_eq!(
            a.sidebar_preferences().unwrap().pinned_session_ids,
            expected
        );
    }
}

#[test]
fn migration_seeds_pending_upserts_that_lose_to_live_writes() {
    // Build a legacy loro workspace doc, materialize, seed.
    let legacy = crate::workspace::WorkspaceDoc::new();
    legacy.upsert_device(&device("dev-a", "laptop")).unwrap();
    legacy
        .upsert_space(&space("sp-1", "dev-a", "/tmp/one"))
        .unwrap();
    let mut legacy_chat = chat("chat-1", "dev-a");
    legacy_chat.space_id = Some("sp-1".into());
    legacy_chat.title = Some("migrated title".into());
    legacy_chat.last_message_at = Some(ts(400_000));
    legacy.upsert_chat(&legacy_chat).unwrap();
    legacy
        .upsert_session(&session("chat-1", "dev-a", SessionStatus::Idle))
        .unwrap();

    let mut doc = RegistryDoc::new("dev-a");
    let seeded = doc
        .seed_from_workspace(&legacy.read_all().unwrap())
        .unwrap();
    assert_eq!(seeded, 4);
    // Instant: the overlay serves the full state before any server contact.
    let state = doc.read_all().unwrap();
    assert_eq!(state, legacy.read_all().unwrap());

    // Two devices seeding the same converged doc = identical result.
    let mut other = RegistryDoc::new("dev-b");
    other
        .seed_from_workspace(&legacy.read_all().unwrap())
        .unwrap();
    let mut server = HashMap::new();
    let mut seq = 0u64;
    server_round(&mut server, &mut seq, &mut [&mut doc, &mut other]);
    assert_eq!(doc.read_all().unwrap(), other.read_all().unwrap());
    assert_eq!(doc.read_all().unwrap(), legacy.read_all().unwrap());

    // A live rename (now-clock) beats the migrated title everywhere.
    other.rename_chat("chat-1", "live rename").unwrap();
    server_round(&mut server, &mut seq, &mut [&mut doc, &mut other]);
    assert_eq!(
        doc.chat("chat-1").unwrap().unwrap().title.as_deref(),
        Some("live rename")
    );
}

#[test]
fn completion_marker_replicates_and_survives_next_turn() {
    let mut source = RegistryDoc::new("dev-a");
    let mut viewer = RegistryDoc::new("dev-b");
    let mut server = HashMap::new();
    let mut seq = 0;
    let mut row = session("chat-1", "dev-a", SessionStatus::Idle);
    row.last_completed_turn = Some("turn-one".into());
    source.upsert_session(&row).unwrap();
    server_round(&mut server, &mut seq, &mut [&mut source, &mut viewer]);
    assert_eq!(viewer.read_sessions().unwrap(), vec![row.clone()]);
    row.status = SessionStatus::Working;
    source.upsert_session(&row).unwrap();
    server_round(&mut server, &mut seq, &mut [&mut source, &mut viewer]);
    assert_eq!(viewer.read_sessions().unwrap(), vec![row]);
}

fn section_change(doc: &mut RegistryDoc, change: zeron_proto::SidebarSectionChange) {
    doc.change_sidebar_pin(&zeron_proto::SidebarPinChange::Section { change })
        .unwrap();
}

#[test]
fn sidebar_sections_keep_placed_order_across_devices() {
    use zeron_proto::SidebarSectionChange::*;
    let mut a = RegistryDoc::new("a");
    let mut b = RegistryDoc::new("b");
    let mut server = HashMap::new();
    let mut seq = 0;
    for id in ["one", "two", "three", "legacy"] {
        a.upsert_chat(&chat(id, "desktop")).unwrap();
    }
    section_change(
        &mut a,
        Create {
            id: "focus".into(),
            name: "Focus".into(),
        },
    );
    let place = |doc: &mut RegistryDoc, id: &str, after: Option<&str>, before: Option<&str>| {
        section_change(
            doc,
            Assign {
                session_id: id.into(),
                section_id: Some("focus".into()),
                after: after.map(Into::into),
                before: before.map(Into::into),
            },
        )
    };
    let order = |doc: &RegistryDoc| {
        doc.sidebar_preferences().unwrap().sections[0]
            .session_ids
            .clone()
    };
    // Anchorless assigns append, so membership no longer sorts by id.
    place(&mut a, "two", None, None);
    place(&mut a, "one", None, None);
    assert_eq!(order(&a), ["two", "one"]);
    // An engine that predates ordering writes a keyless member: it follows
    // the placed ones until the next placement keys it in place.
    a.write_sidebar_location("legacy", "focus");
    assert_eq!(order(&a), ["two", "one", "legacy"]);
    place(&mut a, "three", Some("two"), Some("one"));
    assert_eq!(order(&a), ["two", "three", "one", "legacy"]);
    server_round(&mut server, &mut seq, &mut [&mut a, &mut b]);
    assert_eq!(order(&b), ["two", "three", "one", "legacy"]);
    // A reorder on the other device converges back.
    place(&mut b, "legacy", None, Some("two"));
    server_round(&mut server, &mut seq, &mut [&mut b, &mut a]);
    assert_eq!(order(&a), ["legacy", "two", "three", "one"]);
    assert_eq!(order(&a), order(&b));
    // A keyless move into another section never inherits the old key.
    section_change(
        &mut a,
        Create {
            id: "later".into(),
            name: "Later".into(),
        },
    );
    a.write_sidebar_location("one", "later");
    assert_eq!(order(&a), ["legacy", "two", "three"]);
}

#[test]
fn sidebar_sections_sync_metadata_membership_and_delete_without_deleting_sessions() {
    use zeron_proto::SidebarSectionChange::*;
    let mut a = RegistryDoc::new("a");
    let mut b = RegistryDoc::new("b");
    let mut server = HashMap::new();
    let mut seq = 0;
    pin_sessions(&mut a, &["session"]);
    section_change(
        &mut a,
        Create {
            id: "focus".into(),
            name: "Focus".into(),
        },
    );
    section_change(
        &mut a,
        Assign {
            session_id: "session".into(),
            section_id: Some("focus".into()),
            after: None,
            before: None,
        },
    );
    server_round(&mut server, &mut seq, &mut [&mut a, &mut b]);
    let prefs = b.sidebar_preferences().unwrap();
    assert!(prefs.pinned_session_ids.is_empty());
    assert_eq!(prefs.sections[0].session_ids, ["session"]);
    section_change(
        &mut b,
        Rename {
            id: "focus".into(),
            name: "Today".into(),
        },
    );
    section_change(
        &mut a,
        Collapse {
            id: "focus".into(),
            collapsed: true,
        },
    );
    server_round(&mut server, &mut seq, &mut [&mut b, &mut a]);
    let prefs = a.sidebar_preferences().unwrap();
    assert_eq!(prefs, b.sidebar_preferences().unwrap());
    assert_eq!(prefs.sections[0].name, "Today");
    assert!(prefs.sections[0].collapsed);
    let mut archived = a.chat("session").unwrap().unwrap();
    archived.archived = true;
    a.upsert_chat(&archived).unwrap();
    server_round(&mut server, &mut seq, &mut [&mut a, &mut b]);
    assert_eq!(
        b.sidebar_preferences().unwrap().sections[0].session_ids,
        ["session"]
    );
    section_change(&mut b, Delete { id: "focus".into() });
    server_round(&mut server, &mut seq, &mut [&mut a, &mut b]);
    assert!(a.sidebar_preferences().unwrap().sections.is_empty());
    assert!(a.chat("session").unwrap().is_some());
}

#[test]
fn sidebar_sections_concurrent_pin_and_section_moves_converge() {
    use zeron_proto::{SidebarPinChange, SidebarSectionChange::*};
    for reverse in [false, true] {
        let mut a = RegistryDoc::new("a");
        let mut b = RegistryDoc::new("b");
        let mut server = HashMap::new();
        let mut seq = 0;
        pin_sessions(&mut a, &["session"]);
        for id in ["focus", "later"] {
            section_change(
                &mut a,
                Create {
                    id: id.into(),
                    name: id.into(),
                },
            );
        }
        server_round(&mut server, &mut seq, &mut [&mut a, &mut b]);
        section_change(
            &mut a,
            Assign {
                session_id: "session".into(),
                section_id: Some("focus".into()),
                after: None,
                before: None,
            },
        );
        section_change(
            &mut b,
            Assign {
                session_id: "session".into(),
                section_id: Some("later".into()),
                after: None,
                before: None,
            },
        );
        if reverse {
            server_round(&mut server, &mut seq, &mut [&mut b, &mut a]);
        } else {
            server_round(&mut server, &mut seq, &mut [&mut a, &mut b]);
        }
        let prefs = a.sidebar_preferences().unwrap();
        assert_eq!(prefs, b.sidebar_preferences().unwrap());
        assert_eq!(
            prefs
                .sections
                .iter()
                .map(|s| s.session_ids.len())
                .sum::<usize>(),
            1
        );
        a.change_sidebar_pin(&SidebarPinChange::Pin {
            session_id: "session".into(),
            after: None,
            before: None,
        })
        .unwrap();
        section_change(
            &mut b,
            Assign {
                session_id: "session".into(),
                section_id: Some("focus".into()),
                after: None,
                before: None,
            },
        );
        if reverse {
            server_round(&mut server, &mut seq, &mut [&mut b, &mut a]);
        } else {
            server_round(&mut server, &mut seq, &mut [&mut a, &mut b]);
        }
        let prefs = a.sidebar_preferences().unwrap();
        assert_eq!(prefs, b.sidebar_preferences().unwrap());
        assert_eq!(
            prefs.pinned_session_ids.len()
                + prefs
                    .sections
                    .iter()
                    .map(|s| s.session_ids.len())
                    .sum::<usize>(),
            1
        );
    }
}

#[test]
fn sidebar_sections_migration_is_replay_safe_and_preserves_newer_remote_intent() {
    use zeron_proto::SidebarSectionChange::*;
    let mut a = RegistryDoc::new("a");
    let mut b = RegistryDoc::new("b");
    let mut server = HashMap::new();
    let mut seq = 0;
    a.upsert_chat(&chat("session", "a")).unwrap();
    let import = Import {
        sections: vec![zeron_proto::SidebarSection {
            id: "focus".into(),
            name: "Focus".into(),
            collapsed: true,
            session_ids: vec!["session".into()],
        }],
    };
    section_change(&mut a, import.clone());
    // Offline outbox survives restart before it reaches the other device.
    let mut a = RegistryDoc::from_bytes(&a.to_bytes().unwrap(), "a").unwrap();
    server_round(&mut server, &mut seq, &mut [&mut a, &mut b]);
    assert!(b.sidebar_preferences().unwrap().sections[0].collapsed);
    section_change(
        &mut b,
        Assign {
            session_id: "session".into(),
            section_id: None,
            after: None,
            before: None,
        },
    );
    section_change(
        &mut b,
        Rename {
            id: "focus".into(),
            name: "Renamed".into(),
        },
    );
    server_round(&mut server, &mut seq, &mut [&mut a, &mut b]);
    section_change(&mut a, import.clone());
    assert_eq!(a.sidebar_preferences().unwrap().sections[0].name, "Renamed");
    assert!(
        a.sidebar_preferences().unwrap().sections[0]
            .session_ids
            .is_empty()
    );
    section_change(&mut b, Delete { id: "focus".into() });
    server_round(&mut server, &mut seq, &mut [&mut a, &mut b]);
    section_change(&mut a, import);
    assert!(a.sidebar_preferences().unwrap().sections.is_empty());
    assert!(a.chat("session").unwrap().is_some());
}

#[test]
fn sidebar_sections_honor_pin_toggles_from_older_clients() {
    use zeron_proto::SidebarSectionChange::*;
    let mut doc = RegistryDoc::new("a");
    pin_sessions(&mut doc, &["session"]);
    section_change(
        &mut doc,
        Create {
            id: "focus".into(),
            name: "Focus".into(),
        },
    );
    section_change(
        &mut doc,
        Assign {
            session_id: "session".into(),
            section_id: Some("focus".into()),
            after: None,
            before: None,
        },
    );
    assert!(
        doc.sidebar_preferences()
            .unwrap()
            .pinned_session_ids
            .is_empty()
    );
    // An older client still only writes the existing per-pin fields.
    doc.write(
        KIND_SIDEBAR_PINS,
        "session",
        OpKind::Upsert,
        fields([("pinned", json!(true))]),
    );
    let prefs = doc.sidebar_preferences().unwrap();
    assert_eq!(prefs.pinned_session_ids, ["session"]);
    assert!(prefs.sections[0].session_ids.is_empty());
    doc.write(
        KIND_SIDEBAR_PINS,
        "session",
        OpKind::Upsert,
        fields([("pinned", json!(false))]),
    );
    let prefs = doc.sidebar_preferences().unwrap();
    assert!(prefs.pinned_session_ids.is_empty());
    assert!(prefs.sections[0].session_ids.is_empty());
    section_change(
        &mut doc,
        Assign {
            session_id: "session".into(),
            section_id: Some("focus".into()),
            after: None,
            before: None,
        },
    );
    assert_eq!(
        doc.sidebar_preferences().unwrap().sections[0].session_ids,
        ["session"]
    );
}

#[test]
fn side_chat_origin_syncs_and_survives_updates_and_restart() {
    let mut a = RegistryDoc::new("dev-a");
    let mut b = RegistryDoc::new("dev-b");
    let mut side = chat("side", "dev-a");
    side.parent_chat_id = Some("main".into());
    side.room_gen = Some(2);
    a.upsert_chat(&side).unwrap();
    let mut server = HashMap::new();
    let mut seq = 0;
    server_round(&mut server, &mut seq, &mut [&mut a, &mut b]);
    assert_eq!(b.chat("side").unwrap(), Some(side.clone()));
    b.rename_chat("side", "Investigate caching").unwrap();
    server_round(&mut server, &mut seq, &mut [&mut a, &mut b]);
    assert_eq!(
        a.chat("side").unwrap().unwrap().parent_chat_id.as_deref(),
        Some("main")
    );
    let persisted = a.to_bytes().unwrap();
    let restored = RegistryDoc::from_bytes(&persisted, "dev-a").unwrap();
    assert_eq!(
        restored
            .chat("side")
            .unwrap()
            .unwrap()
            .parent_chat_id
            .as_deref(),
        Some("main")
    );
}

// ── bounded pending: local-only folding and tail coalescing ─────────────────

/// Deterministic xorshift — random op shapes without a rand dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % n
    }

    fn hlc(&mut self) -> String {
        let device = if self.next(2) == 0 { "dev-a" } else { "dev-b" };
        encode_hlc(1 + self.next(12) as i64, 0, device)
    }

    /// An op on `chats/chat-1` with a random kind, field subset (nulls
    /// included), revival clock and per-field clock overrides.
    fn op(&mut self) -> RowOp {
        let kind = match self.next(5) {
            0 => OpKind::Delete,
            1 | 2 => OpKind::Update,
            _ => OpKind::Upsert,
        };
        let mut set = BTreeMap::new();
        let mut clocks = BTreeMap::new();
        for field in ["a", "b", "c"] {
            if self.next(3) == 0 {
                continue;
            }
            let value = match self.next(4) {
                0 => Value::Null,
                v => json!(v),
            };
            set.insert(field.to_string(), value);
            if self.next(3) == 0 {
                clocks.insert(field.to_string(), self.hlc());
            }
        }
        RowOp {
            kind: "chats".into(),
            id: "chat-1".into(),
            op: kind,
            set: (kind != OpKind::Delete).then_some(set),
            hlc: self.hlc(),
            clocks: (kind != OpKind::Delete && !clocks.is_empty()).then_some(clocks),
        }
    }
}

/// Overlay-style sequential replay (tombstones kept, so they compare too).
fn replay(base: Option<RegistryRow>, ops: &[&RowOp]) -> Option<RegistryRow> {
    let mut row = base;
    for op in ops {
        if let Some(next) = apply_op(row.as_ref(), op).0 {
            row = Some(next);
        }
    }
    row
}

#[test]
fn coalesced_ops_are_exact_on_every_row_state() {
    let mut rng = Rng(0x5eed_cafe_f00d);
    let mut merged = 0;
    for _ in 0..50_000 {
        // Bases: missing, live, tombstoned and revived rows with mixed clocks.
        let history: Vec<RowOp> = (0..rng.next(4)).map(|_| rng.op()).collect();
        let base = replay(None, &history.iter().collect::<Vec<_>>());
        let (a, b) = (rng.op(), rng.op());
        let Some(c) = coalesce_ops(&a, &b) else {
            continue;
        };
        merged += 1;
        assert_eq!(
            replay(base.clone(), &[&c]),
            replay(base.clone(), &[&a, &b]),
            "a={a:?}\nb={b:?}\nc={c:?}\nbase={base:?}"
        );
    }
    assert!(merged > 10_000, "only {merged} pairs coalesced");
}

#[test]
fn steady_state_writers_coalesce_to_a_bounded_queue() {
    let mut doc = RegistryDoc::new("dev-a");
    doc.upsert_chat(&chat("chat-1", "dev-a")).unwrap();
    for i in 0..500 {
        let mut touch = session("chat-1", "dev-a", SessionStatus::Working);
        touch.updated_at = ts(10_000 + i * 10_000);
        doc.upsert_session(&touch).unwrap();
        doc.set_chat_last_message("chat-1", &format!("msg {i}"), ts(10_000 + i))
            .unwrap();
    }
    doc.rename_chat("chat-1", "Renamed").unwrap();
    // chat: the creating upsert (it gates notifications, so it never moves
    // past the session touches) + every later update folded into one;
    // sessions: the run head + one op for the other 499 keepalives.
    assert_eq!(doc.pending_ops_len(), 4);
    let chat = doc.chat("chat-1").unwrap().unwrap();
    assert_eq!(chat.title.as_deref(), Some("Renamed"));
    assert_eq!(chat.last_message_preview.as_deref(), Some("msg 499"));
    assert_eq!(
        doc.read_sessions().unwrap()[0].updated_at,
        ts(10_000 + 499 * 10_000)
    );
}

/// Session statuses of the queued `sessions` ops, in push order.
fn queued_statuses(doc: &RegistryDoc) -> Vec<String> {
    doc.pending_ops()
        .filter(|op| op.kind == KIND_SESSIONS)
        .map(|op| op.set.as_ref().unwrap()["status"].as_str().unwrap().into())
        .collect()
}

#[test]
fn status_changes_never_coalesce_but_keepalives_do() {
    let mut doc = RegistryDoc::new("dev-a");
    let mut at = 10_000;
    let mut touch = |doc: &mut RegistryDoc, status| {
        at += 10_000;
        let mut row = session("chat-1", "dev-a", status);
        row.updated_at = ts(at);
        doc.upsert_session(&row).unwrap();
    };
    touch(&mut doc, SessionStatus::Working);
    touch(&mut doc, SessionStatus::AwaitingInput);
    touch(&mut doc, SessionStatus::Working);
    // working → needs input → working: each transition can raise a push.
    assert_eq!(
        queued_statuses(&doc),
        ["working", "awaitingInput", "working"]
    );
    for _ in 0..50 {
        touch(&mut doc, SessionStatus::Working);
    }
    // The keepalives fold into one op behind the run head.
    assert_eq!(
        queued_statuses(&doc),
        ["working", "awaitingInput", "working", "working"]
    );
}

/// Mirror of `notificationFor` (edge/src/push-notify.ts).
fn notification(
    before: Option<&RegistryRow>,
    after: &RegistryRow,
    now: i64,
) -> Option<&'static str> {
    let before = before.filter(|r| !r.deleted)?;
    if after.deleted {
        return None;
    }
    let indicator = |row: &RegistryRow| {
        let updated = row
            .fields
            .get("updatedAt")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        match row.fields.get("status").and_then(Value::as_str) {
            Some("errored") => "errored",
            Some("working") if now - updated <= 45_000 => "working",
            Some("awaitingInput") if now - updated <= 45_000 => "awaiting",
            _ => "none",
        }
    };
    let (prev, next) = (indicator(before), indicator(after));
    if next == "errored" && prev != "errored" {
        return Some("failed");
    }
    if next == "awaiting" && prev != "awaiting" {
        return Some("input");
    }
    let turn = |row: &RegistryRow| {
        row.fields
            .get("lastCompletedTurn")
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    let updated = after
        .fields
        .get("updatedAt")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let fresh = now - updated <= 45_000;
    (fresh && turn(after).is_some() && turn(after) != turn(before)).then_some("done")
}

/// Push `batches` the way the room does: apply each atomically, then derive
/// one notification per touched session from its before/after rows.
fn pushed_notifications(
    base: &HashMap<(String, String), RegistryRow>,
    batches: &[Vec<RowOp>],
    now: i64,
) -> Vec<&'static str> {
    let mut rows = base.clone();
    let mut out = Vec::new();
    for batch in batches {
        let mut before: HashMap<String, Option<RegistryRow>> = HashMap::new();
        for op in batch {
            let key = (op.kind.clone(), op.id.clone());
            if op.kind == KIND_SESSIONS {
                before
                    .entry(op.id.clone())
                    .or_insert_with(|| rows.get(&key).cloned());
            }
            if let (Some(next), true) = apply_op(rows.get(&key), op) {
                rows.insert(key, next);
            }
        }
        for (id, prev) in before {
            if let Some(after) = rows.get(&(KIND_SESSIONS.to_string(), id)) {
                out.extend(notification(prev.as_ref(), after, now));
            }
        }
    }
    out
}

#[test]
fn coalescing_preserves_every_push_notification() {
    let mut rng = Rng(0xa1e7_5eed);
    let statuses = ["working", "awaitingInput", "errored", "idle"];
    let mut notified = 0;
    for _ in 0..300 {
        let mut doc = RegistryDoc::new("dev-a");
        let mut base = HashMap::new();
        let mut at = 100_000 + rng.next(100_000) as i64;
        let mut turn = 0;
        let row_for = |rng: &mut Rng, at: &mut i64, turn: &mut i32, ms: i64| {
            *at += 1 + rng.next(30_000) as i64;
            if rng.next(4) == 0 {
                *turn += 1;
            }
            let set: BTreeMap<String, Value> = [
                ("chatId".to_string(), json!("chat-1")),
                ("status".to_string(), json!(statuses[rng.next(4) as usize])),
                ("lastCompletedTurn".to_string(), json!(format!("t{turn}"))),
                ("updatedAt".to_string(), json!(*at)),
            ]
            .into();
            RowOp {
                kind: KIND_SESSIONS.into(),
                id: "chat-1".into(),
                op: OpKind::Upsert,
                set: Some(set),
                hlc: encode_hlc(ms, 0, "dev-a"),
                clocks: None,
            }
        };
        // The room already holds an older version of the row.
        let seed = row_for(&mut rng, &mut at, &mut turn, 1);
        base.insert(
            (KIND_SESSIONS.to_string(), "chat-1".to_string()),
            apply_op(None, &seed).0.unwrap(),
        );
        // Long status runs (keepalives) with occasional transitions.
        let mut ops = Vec::new();
        let mut current = row_for(&mut rng, &mut at, &mut turn, 2);
        for i in 0..20 + rng.next(40) as i64 {
            if rng.next(6) == 0 {
                current = row_for(&mut rng, &mut at, &mut turn, 3 + i);
            } else {
                at += 1 + rng.next(30_000) as i64;
                let set = current.set.as_mut().unwrap();
                set.insert("updatedAt".into(), json!(at));
                current.hlc = encode_hlc(3 + i, 0, "dev-a");
            }
            ops.push(current.clone());
            doc.enqueue_ops(vec![current.clone()]);
        }
        let original: Vec<Vec<RowOp>> = ops.iter().map(|op| vec![op.clone()]).collect();
        let coalesced: Vec<Vec<RowOp>> = doc.pending.iter().map(|b| b.ops.clone()).collect();
        assert!(coalesced.len() <= original.len());
        // Any push time: before, during and after the queued window.
        for _ in 0..8 {
            let now = 100_000 + rng.next((at - 50_000).max(1) as u64 + 200_000) as i64;
            let expected = pushed_notifications(&base, &original, now);
            assert_eq!(
                pushed_notifications(&base, &coalesced, now),
                expected,
                "now={now}"
            );
            notified += expected.len();
        }
    }
    assert!(notified > 500, "only {notified} notifications exercised");
}

#[test]
fn a_session_op_never_moves_past_a_notification_gating_chat_op() {
    let mut doc = RegistryDoc::new("dev-a");
    doc.upsert_chat(&chat("chat-1", "dev-a")).unwrap();
    let touch = |doc: &mut RegistryDoc, ms| {
        let mut row = session("chat-1", "dev-a", SessionStatus::Working);
        row.updated_at = ts(ms);
        doc.upsert_session(&row).unwrap();
    };
    touch(&mut doc, 10_000);
    touch(&mut doc, 20_000);
    // Archiving silences the chat's pushes: the follower touch before it
    // must not be carried past it (nor the archive past the next touch).
    doc.set_chat_archived("chat-1", true).unwrap();
    touch(&mut doc, 30_000);
    let order: Vec<(String, bool)> = doc
        .pending_ops()
        .map(|op| {
            (
                op.kind.clone(),
                op.set
                    .as_ref()
                    .is_some_and(|s| s.contains_key("archived") && op.op == OpKind::Update),
            )
        })
        .collect();
    assert_eq!(
        order,
        [
            ("chats".to_string(), false),
            ("sessions".to_string(), false),
            ("sessions".to_string(), false),
            ("chats".to_string(), true),
            ("sessions".to_string(), false),
        ]
    );
    // Freshness writes gate nothing and still fold across session touches.
    let before = doc.pending_ops_len();
    for i in 0..20 {
        doc.set_chat_last_message("chat-1", &format!("m{i}"), ts(40_000 + i))
            .unwrap();
        touch(&mut doc, 40_000 + i * 1_000);
    }
    assert!(
        doc.pending_ops_len() <= before + 2,
        "{}",
        doc.pending_ops_len()
    );
}

#[test]
fn merges_that_would_exceed_the_op_budget_stay_separate() {
    let big = |field: &str, at: i64| {
        let mut op = update(&[(field, json!("x".repeat(9 * 1024)))], hlc(at));
        op.kind = KIND_SPACES.into();
        op
    };
    let (a, b) = (big("name", 1), big("path", 2));
    assert!(fits_op_budget(&a) && fits_op_budget(&b));
    assert_eq!(coalesce_ops(&a, &b), None);
    // Same field: the merge is no larger than `b`, so it folds.
    assert!(coalesce_ops(&a, &big("name", 2)).is_some());

    let mut doc = RegistryDoc::new("dev-a");
    doc.enqueue_ops(vec![a.clone()]);
    doc.enqueue_ops(vec![b.clone()]);
    assert_eq!(doc.pending_ops_len(), 2);
    for batch in doc.take_pushable() {
        assert!(batch.ops.iter().all(fits_op_budget));
    }
}

#[test]
fn multi_op_batches_keep_their_ops_in_place() {
    let mut doc = RegistryDoc::new("dev-a");
    let state = WorkspaceState {
        devices: vec![device("dev-a", "laptop")],
        spaces: vec![space("space-1", "dev-a", "/p")],
        chats: vec![chat("chat-1", "dev-a")],
        sessions: vec![],
    };
    doc.seed_from_workspace(&state).unwrap();
    let seed = doc.pending[0].clone();
    assert_eq!(seed.ops.len(), 3);
    for i in 0..5 {
        doc.rename_chat("chat-1", &format!("t{i}")).unwrap();
    }
    // The migration batch is untouched (id and every op); the renames fold
    // among themselves.
    assert_eq!(doc.pending[0].batch, seed.batch);
    assert_eq!(doc.pending[0].ops, seed.ops);
    assert_eq!(doc.pending_ops_len(), 4);
    assert_eq!(
        doc.chat("chat-1").unwrap().unwrap().title.as_deref(),
        Some("t4")
    );
}

#[test]
fn tail_coalescing_preserves_the_overlay_and_the_server_outcome() {
    let mut rng = Rng(0x0dd_ba11);
    for _ in 0..300 {
        let mut doc = RegistryDoc::new("dev-a");
        let history: Vec<RowOp> = (0..rng.next(4)).map(|_| rng.op()).collect();
        let base = replay(None, &history.iter().collect::<Vec<_>>());
        if let Some(row) = base.clone() {
            doc.put_authoritative(row);
        }
        let ops: Vec<RowOp> = (0..1 + rng.next(30)).map(|_| rng.op()).collect();
        for op in &ops {
            doc.enqueue_ops(vec![op.clone()]);
        }
        let expected = replay(base.clone(), &ops.iter().collect::<Vec<_>>());
        assert!(doc.pending_ops_len() <= ops.len());
        // The local overlay…
        assert_eq!(
            doc.overlay_row("chats", "chat-1"),
            expected.clone().filter(|r| !r.deleted)
        );
        // …and a server holding the same base row applying the pushed queue.
        let pushed: Vec<RowOp> = doc
            .take_pushable()
            .into_iter()
            .flat_map(|b| b.ops)
            .collect();
        assert_eq!(replay(base, &pushed.iter().collect::<Vec<_>>()), expected);
    }
}

#[test]
fn in_flight_batches_are_never_coalesced_and_still_ack() {
    let mut doc = RegistryDoc::new("dev-a");
    doc.upsert_chat(&chat("chat-1", "dev-a")).unwrap();
    let sent = doc.take_pushable();
    assert_eq!(sent.len(), 1);
    for i in 0..10 {
        doc.rename_chat("chat-1", &format!("title {i}")).unwrap();
    }
    // The in-flight upsert keeps its id and ops; the ten renames are one op.
    assert_eq!(doc.pending_len(), 2);
    assert_eq!(doc.pending[0].batch, sent[0].batch);
    assert_eq!(doc.pending[0].ops, sent[0].ops);
    assert_eq!(doc.pending[1].ops.len(), 1);
    // The room broadcasts merged rows before the ack.
    assert!(doc.apply_rows(1, vec![applied(None, &sent[0].ops[0])]));
    assert!(doc.ack_batch(&sent[0].batch, 1));
    assert_eq!(doc.pending_len(), 1);
    assert_eq!(
        doc.chat("chat-1").unwrap().unwrap().title.as_deref(),
        Some("title 9")
    );
    let renames = doc.take_pushable();
    assert!(doc.ack_batch(&renames[0].batch, 2));
    assert_eq!(doc.pending_len(), 0);
}

#[test]
fn a_late_ack_for_a_coalesced_away_batch_cannot_drop_newer_writes() {
    let mut doc = RegistryDoc::new("dev-a");
    doc.upsert_chat(&chat("chat-1", "dev-a")).unwrap();
    // Pushed over HTTPS, then the socket drops before the ack lands.
    let raced = doc.take_pushable().pop().unwrap();
    doc.mark_disconnected();
    doc.rename_chat("chat-1", "after the drop").unwrap();
    // The upsert folded forward into a batch under a FRESH id…
    assert_eq!(doc.pending_len(), 1);
    assert_ne!(doc.pending[0].batch, raced.batch);
    // …so the stale ack retires nothing and the rename still pushes.
    assert!(!doc.ack_batch(&raced.batch, 1));
    assert_eq!(doc.pending_ops_len(), 1);
    assert_eq!(
        doc.chat("chat-1").unwrap().unwrap().title.as_deref(),
        Some("after the drop")
    );
}

#[test]
fn local_only_writes_fold_and_keep_pending_empty() {
    let mut local = RegistryDoc::new("dev-a");
    local.set_local_only(true);
    let mut queued = RegistryDoc::new("dev-a");
    for doc in [&mut local, &mut queued] {
        doc.upsert_device(&device("dev-a", "laptop")).unwrap();
        doc.upsert_space(&space("space-1", "dev-a", "/p")).unwrap();
        doc.upsert_chat(&chat("chat-1", "dev-a")).unwrap();
        doc.upsert_chat(&chat("chat-2", "dev-a")).unwrap();
        for _ in 0..50 {
            doc.upsert_session(&session("chat-1", "dev-a", SessionStatus::Working))
                .unwrap();
        }
        doc.rename_chat("chat-1", "renamed").unwrap();
        doc.delete_chat("chat-2").unwrap();
        pin_sessions(doc, &["chat-1"]);
    }
    assert_eq!(local.pending_len(), 0);
    assert!(queued.pending_len() > 0);
    assert_eq!(local.read_all().unwrap(), queued.read_all().unwrap());
    assert_eq!(local.sidebar_preferences(), queued.sidebar_preferences());
    assert_eq!(local.read_chats().unwrap().len(), 1);
}

/// A pre-fix snapshot: every write its own batch, none ever acked. Marking
/// each batch in flight as it is written keeps the doc from coalescing,
/// and `in_flight` is not persisted, so the snapshot reloads all-unsent.
fn bloated_snapshot() -> (Vec<u8>, Vec<RowOp>) {
    let mut doc = RegistryDoc::new("dev-a");
    let mut ops = Vec::new();
    let mut write = |doc: &mut RegistryDoc, f: &dyn Fn(&mut RegistryDoc)| {
        f(doc);
        ops.extend(doc.take_pushable().into_iter().flat_map(|b| b.ops));
    };
    write(&mut doc, &|d| {
        d.upsert_device(&device("dev-a", "laptop")).unwrap()
    });
    write(&mut doc, &|d| {
        d.upsert_chat(&chat("chat-1", "dev-a")).unwrap()
    });
    write(&mut doc, &|d| {
        d.upsert_chat(&chat("chat-2", "dev-a")).unwrap()
    });
    for i in 0..2_000 {
        write(&mut doc, &|d| {
            d.upsert_session(&session("chat-1", "dev-a", SessionStatus::Working))
                .unwrap()
        });
        if i % 10 == 0 {
            write(&mut doc, &|d| {
                d.set_chat_seen("chat-1", ts(5_000 + i)).unwrap();
            });
        }
    }
    write(&mut doc, &|d| {
        d.delete_chat("chat-2").unwrap();
    });
    assert_eq!(doc.pending_ops_len(), ops.len());
    (doc.to_bytes().unwrap(), ops)
}

#[test]
fn loading_a_bloated_snapshot_coalesces_it() {
    let (bytes, ops) = bloated_snapshot();
    assert!(ops.len() > 2_000);
    let doc = RegistryDoc::from_bytes(&bytes, "dev-a").unwrap();
    // device; chat-1's creating upsert + its folded seen-markers; session-1's
    // run head + folded keepalives; chat-2's upsert and its two-op delete
    // batch (deletes never fold, multi-op batches stay whole) — 8, not 2k.
    assert_eq!(doc.pending_ops_len(), 8);
    let expected = {
        let mut server = HashMap::new();
        for op in &ops {
            let key = (op.kind.clone(), op.id.clone());
            if let Some(next) = apply_op(server.get(&key), op).0 {
                server.insert(key, next);
            }
        }
        server
    };
    // Pushing the compacted queue lands the server on the same rows.
    let mut server = HashMap::new();
    let mut seq = 0;
    let mut doc = doc;
    server_round(&mut server, &mut seq, &mut [&mut doc]);
    assert_eq!(server.len(), expected.len());
    for (key, row) in &expected {
        let got = &server[key];
        assert_eq!(
            (&got.fields, &got.clocks, got.deleted),
            (&row.fields, &row.clocks, row.deleted)
        );
    }
    assert!(doc.to_bytes().unwrap().len() * 50 < bytes.len());
}

#[test]
fn loading_a_bloated_snapshot_local_only_folds_it() {
    let (bytes, _) = bloated_snapshot();
    let queued = RegistryDoc::from_bytes(&bytes, "dev-a").unwrap();
    let mut doc = RegistryDoc::from_bytes(&bytes, "dev-a").unwrap();
    doc.set_local_only(true);
    assert_eq!(doc.pending_len(), 0);
    assert_eq!(doc.read_all().unwrap(), queued.read_all().unwrap());
    // The compacted snapshot round-trips with nothing queued.
    let compact = doc.to_bytes().unwrap();
    assert!(compact.len() * 50 < bytes.len());
    let restored = RegistryDoc::from_bytes(&compact, "dev-a").unwrap();
    assert_eq!(restored.pending_len(), 0);
    assert_eq!(restored.read_all().unwrap(), queued.read_all().unwrap());
}

#[test]
fn attaching_an_edge_after_local_only_writes_reseeds_them() {
    // Synced once, then edited with no edge (e.g. a dev profile run
    // without a token), then attached again.
    let mut doc = RegistryDoc::new("dev-a");
    doc.upsert_chat(&chat("chat-1", "dev-a")).unwrap();
    let mut server = HashMap::new();
    let mut seq = 0;
    server_round(&mut server, &mut seq, &mut [&mut doc]);
    let mut doc = RegistryDoc::from_bytes(&doc.to_bytes().unwrap(), "dev-a").unwrap();
    doc.set_local_only(true);
    doc.rename_chat("chat-1", "offline edit").unwrap();
    doc.upsert_chat(&chat("chat-2", "dev-a")).unwrap();
    assert_eq!(doc.pending_len(), 0);

    // The flag survives a restart; leaving local-only queues a re-seed.
    let mut doc = RegistryDoc::from_bytes(&doc.to_bytes().unwrap(), "dev-a").unwrap();
    doc.set_local_only(false);
    assert!(doc.pending_len() > 0);
    // A full state carrying the server's OLDER chat-1 must not erase the
    // offline rename: the re-seed overlays it and wins LWW on push.
    let rows: Vec<RegistryRow> = server.values().cloned().collect();
    doc.apply_state(seq, true, 0, rows);
    let title = |d: &RegistryDoc| d.chat("chat-1").unwrap().unwrap().title;
    assert_eq!(title(&doc).as_deref(), Some("offline edit"));
    server_round(&mut server, &mut seq, &mut [&mut doc]);
    assert_eq!(doc.pending_len(), 0);
    assert_eq!(title(&doc).as_deref(), Some("offline edit"));
    assert_eq!(doc.read_chats().unwrap().len(), 2);
    let key = ("chats".to_string(), "chat-1".to_string());
    assert_eq!(server[&key].fields["title"], json!("offline edit"));

    // Re-seeding is one-shot: the next attach queues nothing.
    let mut doc = RegistryDoc::from_bytes(&doc.to_bytes().unwrap(), "dev-a").unwrap();
    doc.set_local_only(false);
    assert_eq!(doc.pending_len(), 0);
}
