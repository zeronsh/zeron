//! Equivalence oracle for the incremental transcript mirror: after every
//! publish point the mirror must equal `join_continuation_entries(read_entries())`
//! entry for entry, and the shared delta diff must serialize byte-identically
//! to the deep one. Seeded xorshift op sequences over a doc, a concurrent peer,
//! torn imports and replay-origin imports. `PERF_SEEDS=N` runs a long soak.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use loro::{
    Container, ExportMode, LoroDoc, LoroList, LoroMap, LoroMovableList, LoroText, ValueOrContainer,
    VersionVector,
};
use zeron_doc::transcript_delta::{diff_transcript, diff_transcript_shared};
use zeron_doc::{
    MessagePart, MessageRole, MessageStatus, SegmentWriter, SessionDoc, SessionMessageEntry,
    SharedEntry, TranscriptMirror, Verify, join_continuation_entries,
};
use zeron_proto::{ToolCall, UserInputAnswer, UserInputQuestion};

const REPLAY_ORIGIN: &str = "zeron:transcript-replay";

struct Xorshift(u64);

impl Xorshift {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const WORDS: [&str; 6] = ["café ", "naïve ", "日本語 ", "emoji😀 ", "plain ", "ünï "];

fn entry(
    id: &str,
    role: MessageRole,
    text: &str,
    continuation_of: Option<&str>,
) -> SessionMessageEntry {
    SessionMessageEntry {
        id: id.into(),
        role,
        parts: vec![MessagePart::Text {
            id: format!("{id}-t"),
            text: text.into(),
        }],
        created_at: 1,
        device_id: "dev".into(),
        status: Some(MessageStatus::Complete),
        continuation_of: continuation_of.map(str::to_owned),
        duration_ms: None,
    }
}

/// One streaming assistant entry: where it lives and what was written.
struct Live {
    state: (usize, Vec<MessagePart>),
    folded: Vec<MessagePart>,
    tool: u32,
}

struct World {
    a: Arc<SessionDoc>,
    b: SessionDoc,
    b_sent: VersionVector,
    mirror: Arc<TranscriptMirror>,
    watched: Arc<AtomicBool>,
    _sub: loro::Subscription,
    rng: Xorshift,
    ids: Vec<String>,
    live: Vec<Live>,
    next_id: u32,
    prev: Option<Arc<Vec<SharedEntry>>>,
    checks: u64,
}

impl World {
    fn new(seed: u64) -> Self {
        let a = Arc::new(SessionDoc::init("chat").unwrap());
        let b = SessionDoc::from_doc(LoroDoc::new());
        b.doc().import(&a.export_snapshot().unwrap()).unwrap();
        let b_sent = b.doc().oplog_vv();
        let mirror = Arc::new(TranscriptMirror::new().with_verify(Verify::Never));
        let watched = Arc::new(AtomicBool::new(true));
        let sub = {
            let mirror = mirror.clone();
            let watched = watched.clone();
            a.doc().subscribe_root(Arc::new(move |event| {
                if watched.load(Ordering::SeqCst) {
                    mirror.observe(&event);
                } else {
                    mirror.reset();
                }
            }))
        };
        Self {
            a,
            b,
            b_sent,
            mirror,
            watched,
            _sub: sub,
            rng: Xorshift::new(seed),
            ids: Vec::new(),
            live: Vec::new(),
            next_id: 0,
            prev: None,
            checks: 0,
        }
    }

    fn fresh_id(&mut self, prefix: &str) -> String {
        self.next_id += 1;
        format!("{prefix}{}", self.next_id)
    }

    fn pick_id(&mut self) -> String {
        if self.ids.is_empty() || self.rng.below(12) == 0 {
            return format!("ghost{}", self.rng.below(3));
        }
        self.ids[self.rng.below(self.ids.len())].clone()
    }

    fn word(&mut self) -> &'static str {
        WORDS[self.rng.below(WORDS.len())]
    }

    /// The oracle: mirror == join(read_entries()), and both delta encoders
    /// agree on the wire against the previous publish.
    fn check(&mut self) {
        self.checks += 1;
        let joined = self.mirror.refresh(&self.a);
        let expected = join_continuation_entries(self.a.read_entries().unwrap());
        assert_eq!(
            joined.len(),
            expected.len(),
            "joined length (check {})",
            self.checks
        );
        for (ix, (got, want)) in joined.iter().zip(&expected).enumerate() {
            assert_eq!(&**got, want, "joined entry {ix} (check {})", self.checks);
        }
        if let Some(prev) = &self.prev {
            let shared = serde_json::to_string(&diff_transcript_shared(prev, &joined)).unwrap();
            let prev_deep: Vec<SessionMessageEntry> = prev.iter().map(|e| (**e).clone()).collect();
            let deep = serde_json::to_string(&diff_transcript(&prev_deep, &expected)).unwrap();
            assert_eq!(shared, deep, "delta frames (check {})", self.checks);
        }
        self.prev = Some(joined);
    }

    fn sync_b(&mut self, replay: bool) {
        let bytes = self
            .b
            .doc()
            .export(ExportMode::updates(&self.b_sent))
            .unwrap();
        self.b_sent = self.b.doc().oplog_vv();
        if replay {
            self.a.doc().import_with(&bytes, REPLAY_ORIGIN).unwrap();
        } else {
            self.a.doc().import(&bytes).unwrap();
        }
    }

    fn op(&mut self) {
        match self.rng.below(28) {
            0..=2 => {
                let id = self.fresh_id("m");
                let role = [MessageRole::User, MessageRole::System][self.rng.below(2)];
                let text = self.word();
                // Occasionally reuse an id: duplicate ids must behave alike.
                let id = if self.rng.below(15) == 0 && !self.ids.is_empty() {
                    self.pick_id()
                } else {
                    id
                };
                self.a.push_message(&entry(&id, role, text, None)).unwrap();
                self.ids.push(id);
            }
            3..=4 => {
                let id = self.fresh_id("s");
                let writer = SegmentWriter::begin(&self.a, &id, "host", 5).unwrap();
                self.ids.push(id);
                self.live.push(Live {
                    state: writer.into_state(),
                    folded: Vec::new(),
                    tool: 0,
                });
            }
            5..=9 => self.grow_live(),
            10 => self.finish_live(),
            11 => {
                // Continuation of a present root, or an orphan.
                let id = self.fresh_id("c");
                let root = self.pick_id();
                let text = self.word();
                self.a
                    .push_message(&entry(&id, MessageRole::Assistant, text, Some(&root)))
                    .unwrap();
            }
            12 => {
                let id = self.fresh_id("i");
                let text = self.word();
                let from_end = self.rng.below(4);
                self.a
                    .insert_message_before_last(
                        from_end,
                        &entry(&id, MessageRole::User, text, None),
                    )
                    .unwrap();
                self.ids.push(id);
            }
            13 => {
                let id = self.pick_id();
                let status = [MessageStatus::Aborted, MessageStatus::Complete][self.rng.below(2)];
                self.a.set_message_status(&id, status).unwrap();
            }
            14 => {
                let id = self.pick_id();
                let part = self.fresh_id("e");
                self.a.append_error_part(&id, &part, "boom ünï").unwrap();
            }
            15 => {
                let id = self.pick_id();
                let _ = self.a.update_subagent_chip(
                    &id,
                    Some("sub-doc"),
                    Some("running"),
                    Some("tail …"),
                );
            }
            16 => {
                let id = self.pick_id();
                let _ = self.a.resolve_input(
                    &id,
                    &[UserInputAnswer {
                        question_id: "q".into(),
                        labels: vec!["yes".into()],
                    }],
                );
            }
            17..=18 => self.peer_edit(),
            19 => self.torn_import(),
            20 => self.delete_entry(),
            21 => {
                // Leave and re-enter the watched state: events in between are
                // not observed, the mirror resets, and must still converge.
                let was = self.watched.fetch_xor(true, Ordering::SeqCst);
                if was {
                    self.mirror.reset();
                    self.prev = None;
                }
            }
            22 => self.edit_continuation(),
            23 => self.edit_nested(),
            24 => self.torn_import_reordered(),
            25 => self.batched_import(),
            _ => {
                // A reset mid-stream (stale-mirror recovery) is always safe.
                if self.rng.below(4) == 0 {
                    self.mirror.reset();
                    self.prev = None;
                }
            }
        }
    }

    fn pick_map(&mut self) -> Option<LoroMap> {
        let messages = self.a.doc().get_list("messages");
        if messages.is_empty() {
            return None;
        }
        match messages.get(self.rng.below(messages.len()))? {
            ValueOrContainer::Container(Container::Map(map)) => Some(map),
            _ => None,
        }
    }

    fn edit_continuation(&mut self) {
        let Some(map) = self.pick_map() else { return };
        let root = self.pick_id();
        if self.rng.below(3) == 0 {
            map.delete("continuationOf").unwrap();
        } else {
            map.insert("continuationOf", root).unwrap();
        }
        map.insert("durationMs", self.rng.below(100) as i64)
            .unwrap();
        self.a.doc().commit();
    }

    fn edit_nested(&mut self) {
        let Some(map) = self.pick_map() else { return };
        // Movable parts lists are also JSON arrays to the decoder. Their move
        // and delete diffs must dirty the owning entry, not the messages order.
        let parts = match map.get("parts") {
            Some(ValueOrContainer::Container(Container::MovableList(parts))) => parts,
            _ => {
                let parts = map
                    .insert_container("parts", LoroMovableList::new())
                    .unwrap();
                for i in 0..3 {
                    let part = parts.push_container(LoroMap::new()).unwrap();
                    part.insert("id", format!("nested-{i}")).unwrap();
                    part.insert("kind", "text").unwrap();
                    let text = part.insert_container("text", LoroText::new()).unwrap();
                    text.insert(0, WORDS[i]).unwrap();
                }
                parts
            }
        };
        match self.rng.below(4) {
            0 if parts.len() > 1 => parts.mov(0, parts.len() - 1).unwrap(),
            1 if !parts.is_empty() => parts.delete(0, 1).unwrap(),
            _ => {
                if let Some(ValueOrContainer::Container(Container::Map(part))) = parts.get(0) {
                    if let Some(ValueOrContainer::Container(Container::Text(text))) =
                        part.get("text")
                    {
                        text.insert(0, self.word()).unwrap();
                        if text.len_unicode() > 2 {
                            text.delete(1, 1).unwrap();
                        }
                    }
                    // Unknown nested containers exercise strict parse + salvage.
                    let extra = part.insert_container("future", LoroMap::new()).unwrap();
                    extra.insert("value", self.word()).unwrap();
                }
            }
        }
        self.a.doc().commit();
    }

    fn batched_import(&mut self) {
        // Many entry and descendant diffs in a single import, including roots
        // that arrive after orphan continuations in messages order.
        let root = self.fresh_id("batch-root");
        for i in 0..8 {
            let id = self.fresh_id("batch");
            let continuation = (i % 2 == 0).then_some(root.as_str());
            let word = self.word();
            self.b
                .push_message(&entry(&id, MessageRole::Assistant, word, continuation))
                .unwrap();
        }
        self.b
            .push_message(&entry(&root, MessageRole::Assistant, "late root", None))
            .unwrap();
        self.ids.push(root);
        self.sync_b(true);
    }

    fn torn_import_reordered(&mut self) {
        self.torn_import_impl(true);
    }

    fn grow_live(&mut self) {
        if self.live.is_empty() {
            return;
        }
        let ix = self.rng.below(self.live.len());
        let word = self.word();
        let action = self.rng.below(7);
        let tool_id = {
            let live = &mut self.live[ix];
            live.tool += 1;
            format!("tool{ix}-{}", live.tool)
        };
        let live = &mut self.live[ix];
        match action {
            0..=2 => match live.folded.last_mut() {
                Some(MessagePart::Text { text, .. }) => text.push_str(word),
                _ => live.folded.push(MessagePart::Text {
                    id: format!("txt{ix}-{}", live.folded.len()),
                    text: word.to_owned(),
                }),
            },
            3 => live.folded.push(MessagePart::Reasoning {
                id: format!("r{ix}-{}", live.folded.len()),
                text: word.to_owned(),
            }),
            4 => live.folded.push(MessagePart::Tool {
                id: tool_id,
                call: if live.tool % 2 == 0 {
                    ToolCall::Unknown {
                        name: "Agent".into(),
                        input: None,
                    }
                } else {
                    ToolCall::Exec {
                        command: "ls".into(),
                    }
                },
                is_error: false,
                resolved: false,
                output: None,
                diff: None,
                output_ref: None,
                output_bytes: None,
                diff_ref: None,
                diff_stats: None,
                subagent_ref: None,
                subagent_status: None,
                subagent_tail: None,
            }),
            5 => {
                if let Some(MessagePart::Tool {
                    resolved,
                    output,
                    is_error,
                    ..
                }) = live
                    .folded
                    .iter_mut()
                    .rev()
                    .find(|p| matches!(p, MessagePart::Tool { .. }))
                {
                    *resolved = true;
                    *is_error = word.len() % 2 == 0;
                    *output = Some(word.to_owned());
                }
            }
            _ => live.folded.push(MessagePart::Input {
                id: format!("req{ix}-{}", live.folded.len()),
                request_id: format!("req{ix}-{}", live.folded.len()),
                questions: vec![UserInputQuestion {
                    id: "q".into(),
                    header: "h".into(),
                    question: "?".into(),
                    options: vec!["yes".into(), "no".into()],
                    multi_select: false,
                    prefill: None,
                    multiline: false,
                }],
                resolved: false,
                answers: None,
            }),
        }
        let (index, written) = std::mem::take(&mut live.state);
        let mut writer = SegmentWriter::resume(&self.a, index, written);
        // Positional state can go stale after deletes/imports; a failed sync
        // is just another op sequence the oracle has to survive.
        let _ = writer.sync(&live.folded);
        live.state = writer.into_state();
    }

    fn finish_live(&mut self) {
        if self.live.is_empty() {
            return;
        }
        let live = self.live.remove(self.rng.below(self.live.len()));
        let (index, written) = live.state;
        let writer = SegmentWriter::resume(&self.a, index, written);
        let _ = writer.finish(&live.folded, MessageStatus::Complete);
    }

    fn delete_entry(&mut self) {
        let messages = self.a.doc().get_list("messages");
        if messages.len() == 0 {
            return;
        }
        let at = self.rng.below(messages.len());
        messages.delete(at, 1).unwrap();
        self.a.doc().commit();
    }

    fn peer_edit(&mut self) {
        match self.rng.below(5) {
            0 => {
                let id = self.fresh_id("b");
                let word = self.word();
                self.b
                    .push_message(&entry(&id, MessageRole::User, word, None))
                    .unwrap();
            }
            1 => {
                let id = self.fresh_id("bs");
                let writer = SegmentWriter::begin(&self.b, &id, "peer", 9).unwrap();
                let (index, _) = writer.into_state();
                let mut writer = SegmentWriter::resume(&self.b, index, Vec::new());
                let _ = writer.sync(&[MessagePart::Text {
                    id: "bt".into(),
                    text: "peer text".into(),
                }]);
            }
            2 => {
                let id = self.pick_id();
                let _ = self.b.set_message_status(&id, MessageStatus::Aborted);
            }
            3 => {
                let id = self.fresh_id("bc");
                let root = self.pick_id();
                self.b
                    .push_message(&entry(
                        &id,
                        MessageRole::Assistant,
                        "peer cont",
                        Some(&root),
                    ))
                    .unwrap();
            }
            _ => {
                let messages = self.b.doc().get_list("messages");
                if messages.len() > 0 {
                    messages.delete(self.rng.below(messages.len()), 1).unwrap();
                    self.b.doc().commit();
                }
            }
        }
        let replay = self.rng.below(3) == 0;
        self.sync_b(replay);
    }

    /// The peer builds an entry across several commits and each update is
    /// imported on its own, so the mirror sees every intermediate (torn) state.
    fn torn_import(&mut self) {
        self.torn_import_impl(false);
    }

    fn torn_import_impl(&mut self, reorder: bool) {
        let id = self.fresh_id("torn");
        let continuation = self.rng.below(3) == 0;
        let root = self.pick_id();
        let list = self.b.doc().get_list("messages");
        let map = list.push_container(LoroMap::new()).unwrap();
        let mut stages = Vec::new();
        let mut stage = |w: &mut World| {
            w.b.doc().commit();
            let bytes = w.b.doc().export(ExportMode::updates(&w.b_sent)).unwrap();
            w.b_sent = w.b.doc().oplog_vv();
            stages.push(bytes);
        };
        stage(self);
        map.insert("id", id.as_str()).unwrap();
        map.insert("role", "assistant").unwrap();
        stage(self);
        map.insert("createdAt", 7i64).unwrap();
        map.insert("deviceId", "peer").unwrap();
        if continuation {
            map.insert("continuationOf", root.as_str()).unwrap();
        }
        stage(self);
        let parts = map.insert_container("parts", LoroList::new()).unwrap();
        let part = parts.push_container(LoroMap::new()).unwrap();
        stage(self);
        part.insert("id", format!("{id}-p")).unwrap();
        part.insert("kind", "text").unwrap();
        let text = part.insert_container("text", LoroText::new()).unwrap();
        text.insert(0, "tørn ✓").unwrap();
        stage(self);
        map.insert("status", "complete").unwrap();
        stage(self);
        if reorder {
            // Deliver missing-dependency updates first, then close their gaps.
            for i in (1..stages.len()).rev() {
                let j = self.rng.below(i + 1);
                stages.swap(i, j);
            }
        }
        for (ix, bytes) in stages.into_iter().enumerate() {
            if ix % 2 == 0 {
                self.a.doc().import(&bytes).unwrap();
            } else {
                self.a.doc().import_with(&bytes, REPLAY_ORIGIN).unwrap();
            }
            if self.watched.load(Ordering::SeqCst) {
                self.check();
            }
        }
        self.ids.push(id);
    }
}

fn run(seed: u64, ops: usize) {
    let mut world = World::new(seed);
    for step in 0..ops {
        world.op();
        if world.rng.below(3) == 0 && world.watched.load(Ordering::SeqCst) {
            world.check();
        }
        let _ = step;
    }
    world.watched.store(true, Ordering::SeqCst);
    world.mirror.reset();
    world.check();
}

fn seeds() -> u64 {
    std::env::var("PERF_SEEDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(300)
}

#[test]
fn mirror_equals_full_rebuild_over_random_op_sequences() {
    for seed in 1..=seeds() {
        let result = std::panic::catch_unwind(|| run(seed, 150));
        if let Err(panic) = result {
            eprintln!("transcript mirror divergence at seed {seed}");
            std::panic::resume_unwind(panic);
        }
    }
}

/// Fixed regression seeds guarantee the newly added operations run even
/// when PERF_SEEDS is reduced: regroup edits, nested list/text mutations,
/// dependency-reordered torn updates, and imports with many diffs.
#[test]
fn seeded_regroup_nested_and_reordered_import_regressions() {
    for seed in [0xc071, 0x7e57, 0x0de2, 0xba7c] {
        let mut world = World::new(seed);
        for (id, root) in [("root", None), ("cont", Some("root")), ("other", None)] {
            world
                .a
                .push_message(&entry(id, MessageRole::Assistant, "original", root))
                .unwrap();
            world.ids.push(id.to_owned());
        }
        world.check();
        for _ in 0..12 {
            world.edit_continuation();
            world.check();
            world.edit_nested();
            world.check();
        }
        world.torn_import_reordered();
        world.check();
        world.batched_import();
        world.check();
    }
}

#[test]
fn uncommitted_edits_are_visible_without_committing_them() {
    let mut world = World::new(0xdec0de);
    world
        .a
        .push_message(&entry("root", MessageRole::User, "old", None))
        .unwrap();
    world.check();
    let map = world.pick_map().unwrap();
    map.insert("continuationOf", "orphan").unwrap();
    assert!(world.a.doc().get_pending_txn_len() > 0);
    world.check();
    assert!(
        world.a.doc().get_pending_txn_len() > 0,
        "refresh must remain read-only"
    );
    world.a.doc().commit();
    world.check();
}

#[test]
fn committed_versions_recover_even_without_an_observer() {
    let doc = SessionDoc::init("chat").unwrap();
    let mirror = TranscriptMirror::new().with_verify(Verify::Always);
    for i in 0..5 {
        doc.push_message(&entry(&format!("m{i}"), MessageRole::User, "a", None))
            .unwrap();
    }
    mirror.refresh(&doc);
    // No observer is attached: the mirror never hears about this edit.
    doc.set_message_status("m2", MessageStatus::Aborted)
        .unwrap();
    let got = mirror.refresh(&doc);
    assert_eq!(
        mirror.mismatches(),
        0,
        "version reconciliation catches missed events"
    );
    assert_eq!(
        got[2].status,
        Some(MessageStatus::Aborted),
        "full result wins"
    );
    let again = mirror.refresh(&doc);
    assert_eq!(mirror.mismatches(), 0, "the mirror stays current");
    assert_eq!(again[2].status, Some(MessageStatus::Aborted));
}

/// Seed 0xd1ff: refresh after a second map changed, before its event is
/// delivered, with another map already pending. Draining only pending events
/// silently retains the second map's old Arc in release.
#[test]
fn refresh_before_event_delivery_does_not_reuse_changed_arcs() {
    let mut rng = Xorshift::new(0xd1ff);
    let doc = Arc::new(SessionDoc::init("chat").unwrap());
    let mirror = Arc::new(TranscriptMirror::new().with_verify(Verify::Never));
    for i in 0..3 {
        doc.push_message(&entry(&format!("m{i}"), MessageRole::User, "old", None))
            .unwrap();
    }
    let before = mirror.refresh(&doc);
    let delayed = rng.below(3);
    let other = (delayed + 1) % 3;
    let captured = Arc::new(Mutex::new(None));
    let _sub = {
        let doc = doc.clone();
        let mirror = mirror.clone();
        let captured = captured.clone();
        doc.doc().clone().subscribe_root(Arc::new(move |event| {
            // Deliberately publish while this event has not reached observe.
            *captured.lock().unwrap() = Some(mirror.refresh(&doc));
            mirror.observe(&event);
        }))
    };
    doc.set_message_status(&format!("m{other}"), MessageStatus::Aborted)
        .unwrap();
    doc.set_message_status(&format!("m{delayed}"), MessageStatus::Aborted)
        .unwrap();
    let got = captured.lock().unwrap().take().unwrap();
    let expected = join_continuation_entries(doc.read_entries().unwrap());
    assert_eq!(
        got.iter().map(|e| (**e).clone()).collect::<Vec<_>>(),
        expected
    );
    assert!(!Arc::ptr_eq(&before[delayed], &got[delayed]));
    assert_eq!(mirror.mismatches(), 0);
}

#[test]
fn streaming_redecodes_only_the_live_entry() {
    let doc = Arc::new(SessionDoc::init("chat").unwrap());
    let mirror = Arc::new(TranscriptMirror::new().with_verify(Verify::Never));
    let seen = Arc::new(Mutex::new(0usize));
    let sub = {
        let mirror = mirror.clone();
        let seen = seen.clone();
        doc.doc().subscribe_root(Arc::new(move |event| {
            *seen.lock().unwrap() += 1;
            mirror.observe(&event);
        }))
    };
    for i in 0..400 {
        doc.push_message(&entry(
            &format!("old{i}"),
            MessageRole::User,
            "history",
            None,
        ))
        .unwrap();
    }
    let before = mirror.refresh(&doc);
    assert_eq!(before.len(), 400);
    let decodes = mirror.decodes();

    let writer = SegmentWriter::begin(&doc, "live", "host", 1).unwrap();
    let mut state = writer.into_state();
    let mut text = String::new();
    for _ in 0..20 {
        text.push_str("tok ");
        let mut writer = SegmentWriter::resume(&doc, state.0, state.1);
        writer
            .sync(&[MessagePart::Text {
                id: "p".into(),
                text: text.clone(),
            }])
            .unwrap();
        state = writer.into_state();
        mirror.refresh(&doc);
    }
    // One decode when the entry appears plus one per streamed tick; the 400
    // settled entries are never decoded again.
    assert!(
        mirror.decodes() - decodes <= 1 + 1 + 20,
        "decoded {} entries for 20 ticks",
        mirror.decodes() - decodes
    );
    let after = mirror.refresh(&doc);
    for (old, new) in before.iter().zip(after.iter()) {
        assert!(Arc::ptr_eq(old, new), "settled entries keep their Arc");
    }
    drop(sub);
}

/// Timing probe: `cargo test --release -p zeron-doc --test transcript_mirror_eq
/// streaming_tick_cost -- --ignored --nocapture`. A 2 MB transcript, one
/// streamed tick each way.
#[test]
#[ignore]
fn streaming_tick_cost() {
    use std::time::Instant;
    let doc = Arc::new(SessionDoc::init("chat").unwrap());
    let mirror = Arc::new(TranscriptMirror::new().with_verify(Verify::Never));
    let _sub = {
        let mirror = mirror.clone();
        doc.doc()
            .subscribe_root(Arc::new(move |event| mirror.observe(&event)))
    };
    let body = "x".repeat(2048);
    for i in 0..1000 {
        doc.push_message(&entry(&format!("old{i}"), MessageRole::User, &body, None))
            .unwrap();
    }
    let t = Instant::now();
    let mut prev = mirror.refresh(&doc);
    println!("attach (full rebuild) {:?}", t.elapsed());
    let t = Instant::now();
    let full = join_continuation_entries(doc.read_entries().unwrap());
    println!(
        "read_entries + join   {:?} ({} entries)",
        t.elapsed(),
        full.len()
    );

    let writer = SegmentWriter::begin(&doc, "live", "host", 1).unwrap();
    let mut state = writer.into_state();
    let mut text = String::new();
    let (mut inc, mut old, mut diff_shared, mut diff_deep) = (0u128, 0u128, 0u128, 0u128);
    const TICKS: u32 = 50;
    for _ in 0..TICKS {
        text.push_str("another streamed token ");
        let mut writer = SegmentWriter::resume(&doc, state.0, state.1);
        writer
            .sync(&[MessagePart::Text {
                id: "p".into(),
                text: text.clone(),
            }])
            .unwrap();
        state = writer.into_state();
        let t = Instant::now();
        let next = mirror.refresh(&doc);
        inc += t.elapsed().as_micros();
        let t = Instant::now();
        let full = join_continuation_entries(doc.read_entries().unwrap());
        old += t.elapsed().as_micros();
        let t = Instant::now();
        let a = diff_transcript_shared(&prev, &next);
        diff_shared += t.elapsed().as_micros();
        let prev_deep: Vec<SessionMessageEntry> = prev.iter().map(|e| (**e).clone()).collect();
        let t = Instant::now();
        let b = diff_transcript(&prev_deep, &full);
        diff_deep += t.elapsed().as_micros();
        assert_eq!(
            serde_json::to_string(&a).unwrap(),
            serde_json::to_string(&b).unwrap()
        );
        prev = next;
    }
    let per = |total: u128| total / TICKS as u128;
    println!(
        "per tick: mirror.refresh {}us | read_entries+join {}us",
        per(inc),
        per(old)
    );
    println!(
        "per tick: diff_shared {}us | diff_deep {}us",
        per(diff_shared),
        per(diff_deep)
    );
}
