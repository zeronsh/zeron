//! Equivalence oracle for the change mask and the commands cache.
//!
//! The mask must never miss a root that really changed (a missed bit would
//! skip a publish or a drain), and the cache must equal `read_commands()` at
//! every point while re-reading only after a `commands` change. Seeded
//! xorshift op sequences over a doc, a concurrent peer and torn imports.
//! `PERF_SEEDS=N` runs a long soak.

use std::sync::{Arc, Mutex};

use loro::{ExportMode, LoroDoc, LoroMap, VersionVector};
use zeron_doc::{
    AtomicChangeMask, ChangeMask, CommandsCache, MessageRole, MessageStatus, QueuedMessage,
    SessionCommandEntry, SessionCommandPayload, SessionCommandStatus, SessionDoc,
    SessionMessageEntry, Verify,
};

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

fn command(id: &str, payload: SessionCommandPayload, issued_at: i64) -> SessionCommandEntry {
    SessionCommandEntry {
        id: id.into(),
        payload,
        issued_by: "dev".into(),
        issued_at,
        based_on: None,
        expires_at: None,
        status: SessionCommandStatus::Pending,
        resolution: None,
    }
}

fn payload(rng: &mut Xorshift, n: u32) -> SessionCommandPayload {
    match rng.below(3) {
        0 => SessionCommandPayload::Steer {
            prompt: format!("steer {n} café 日本語"),
            message_id: (rng.below(2) == 0).then(|| format!("msg{n}")),
        },
        1 => SessionCommandPayload::Interrupt {},
        _ => SessionCommandPayload::RespondInput {
            request_id: format!("req{n}"),
            answers: Vec::new(),
        },
    }
}

fn message(id: &str) -> SessionMessageEntry {
    SessionMessageEntry {
        id: id.into(),
        role: MessageRole::User,
        parts: vec![zeron_doc::MessagePart::Text {
            id: format!("{id}-t"),
            text: "hello".into(),
        }],
        created_at: 1,
        device_id: "dev".into(),
        status: Some(MessageStatus::Complete),
        continuation_of: None,
        duration_ms: None,
    }
}

/// The roots whose deep value differs between two doc states.
fn changed_roots(before: &serde_json::Value, after: &serde_json::Value) -> ChangeMask {
    let mut mask = ChangeMask::NONE;
    let keys = before
        .as_object()
        .into_iter()
        .chain(after.as_object())
        .flat_map(|o| o.keys().cloned())
        .collect::<std::collections::BTreeSet<_>>();
    for key in keys {
        // A root nobody has touched yet reads as null, an empty one as []/{}:
        // creating it is not a change.
        let present = |v: &serde_json::Value| match v.get(&key) {
            None | Some(serde_json::Value::Null) => None,
            Some(serde_json::Value::Array(a)) if a.is_empty() => None,
            Some(serde_json::Value::Object(o)) if o.is_empty() => None,
            Some(other) => Some(other.clone()),
        };
        if present(before) == present(after) {
            continue;
        }
        mask = mask.union(match key.as_str() {
            "messages" => ChangeMask::MESSAGES,
            "commands" => ChangeMask::COMMANDS,
            "queue" => ChangeMask::QUEUE,
            _ => ChangeMask::META,
        });
    }
    mask
}

fn snapshot(doc: &SessionDoc) -> serde_json::Value {
    use loro::ToJson;
    doc.doc().get_deep_value().to_json_value()
}

struct World {
    a: Arc<SessionDoc>,
    b: SessionDoc,
    b_sent: VersionVector,
    /// Everything the doc callback saw since the last op.
    seen: Arc<AtomicChangeMask>,
    cache: Arc<CommandsCache>,
    _sub: loro::Subscription,
    rng: Xorshift,
    ids: Vec<String>,
    next_id: u32,
    /// Mask bits a multi-stage op already took for its own checks.
    carry: ChangeMask,
    last_op: usize,
    gets: u64,
    skipped_fills: u64,
}

impl World {
    fn new(seed: u64) -> Self {
        let a = Arc::new(SessionDoc::init("chat").unwrap());
        let b = SessionDoc::from_doc(LoroDoc::new());
        b.doc().import(&a.export_snapshot().unwrap()).unwrap();
        let b_sent = b.doc().oplog_vv();
        let seen = Arc::new(AtomicChangeMask::default());
        let cache = Arc::new(CommandsCache::new().with_verify(Verify::Never));
        let sub = {
            let seen = seen.clone();
            let cache = cache.clone();
            a.doc().subscribe_root(Arc::new(move |event| {
                let mask = ChangeMask::of_event(&event);
                seen.set(mask);
                if mask.contains(ChangeMask::COMMANDS) {
                    cache.invalidate();
                }
            }))
        };
        Self {
            a,
            b,
            b_sent,
            seen,
            cache,
            _sub: sub,
            rng: Xorshift::new(seed),
            ids: Vec::new(),
            next_id: 0,
            carry: ChangeMask::NONE,
            last_op: 0,
            gets: 0,
            skipped_fills: 0,
        }
    }

    fn fresh_id(&mut self, prefix: &str) -> String {
        self.next_id += 1;
        format!("{prefix}{}", self.next_id)
    }

    fn pick_id(&mut self) -> String {
        if self.ids.is_empty() || self.rng.below(10) == 0 {
            return format!("ghost{}", self.rng.below(3));
        }
        self.ids[self.rng.below(self.ids.len())].clone()
    }

    /// The cache oracle plus the "no ledger read without a ledger change" rule.
    fn check_cache(&mut self, commands_changed: bool) {
        let fills = self.cache.fills();
        let got = self.cache.get(&self.a).unwrap();
        let want = self.a.read_commands().unwrap();
        assert_eq!(*got, want, "cache == read_commands (get {})", self.gets);
        self.gets += 1;
        if !commands_changed && self.gets > 1 {
            assert_eq!(
                self.cache.fills(),
                fills,
                "a read without a commands change must be a cache hit"
            );
            self.skipped_fills += 1;
        }
    }

    fn sync_b(&mut self) {
        let bytes = self
            .b
            .doc()
            .export(ExportMode::updates(&self.b_sent))
            .unwrap();
        self.b_sent = self.b.doc().oplog_vv();
        self.a.doc().import(&bytes).unwrap();
    }

    fn sync_a_to_b(&mut self) {
        let bytes = self.a.doc().export(ExportMode::all_updates()).unwrap();
        self.b.doc().import(&bytes).unwrap();
        self.b_sent = self.b.doc().oplog_vv();
    }

    /// Run one op; return the roots it is allowed to touch (`None` = an
    /// import, which may touch any root and is checked for soundness only).
    fn op(&mut self) -> Option<ChangeMask> {
        let which = self.rng.below(22);
        self.last_op = which;
        match which {
            0..=3 => {
                let id = self.fresh_id("c");
                let payload = payload(&mut self.rng, self.next_id);
                let issued = self.next_id as i64;
                self.a
                    .queue_command(&command(&id, payload, issued))
                    .unwrap();
                self.ids.push(id);
                Some(ChangeMask::COMMANDS)
            }
            4..=6 => {
                let id = self.pick_id();
                let status = [
                    SessionCommandStatus::Applied,
                    SessionCommandStatus::Rejected,
                    SessionCommandStatus::Cancelled,
                ][self.rng.below(3)];
                let _ = self.a.set_command_status(&id, status, Some("because ✓"));
                Some(ChangeMask::COMMANDS)
            }
            7..=9 => {
                let id = self.fresh_id("m");
                self.a.push_message(&message(&id)).unwrap();
                self.ids.push(id);
                Some(ChangeMask::MESSAGES)
            }
            10 => {
                let id = self.pick_id();
                let _ = self.a.set_message_status(&id, MessageStatus::Aborted);
                Some(ChangeMask::MESSAGES)
            }
            11..=12 => {
                let id = self.fresh_id("q");
                self.a
                    .push_queued(&QueuedMessage::new(&id, "queued text", "dev"))
                    .unwrap();
                self.ids.push(id);
                Some(ChangeMask::QUEUE)
            }
            13 => {
                let _ = self.a.take_queue_head();
                Some(ChangeMask::QUEUE)
            }
            14 => {
                let n = self.rng.next();
                self.a
                    .doc()
                    .get_map("meta")
                    .insert("probe", n as i64)
                    .unwrap();
                self.a.doc().commit();
                Some(ChangeMask::META)
            }
            15..=16 => {
                // Peer appends a command and a message in one update.
                let id = self.fresh_id("bc");
                let payload = payload(&mut self.rng, self.next_id);
                self.b.queue_command(&command(&id, payload, 9)).unwrap();
                if self.rng.below(2) == 0 {
                    let m = self.fresh_id("bm");
                    self.b.push_message(&message(&m)).unwrap();
                }
                self.sync_b();
                self.ids.push(id);
                None
            }
            17 => {
                // Peer resolves a command it learned of earlier.
                self.sync_a_to_b();
                let id = self.pick_id();
                let _ = self
                    .b
                    .set_command_status(&id, SessionCommandStatus::Cancelled, None);
                self.sync_b();
                None
            }
            18 => {
                let n = self.rng.next();
                self.b
                    .doc()
                    .get_map("meta")
                    .insert("peer", n as i64)
                    .unwrap();
                self.b.doc().commit();
                self.sync_b();
                None
            }
            19 => {
                self.torn_command_import();
                None
            }
            // A no-op commit and a re-import of known updates emit nothing.
            20 => {
                self.a.doc().commit();
                Some(ChangeMask::NONE)
            }
            _ => {
                let bytes = self.a.doc().export(ExportMode::all_updates()).unwrap();
                self.a.doc().import(&bytes).unwrap();
                Some(ChangeMask::NONE)
            }
        }
    }

    /// The peer builds a command across several commits and each update is
    /// imported on its own: the cache sees the entry as an empty map, then a
    /// half-written one, then complete, and must stay equal to a fresh read.
    fn torn_command_import(&mut self) {
        let id = self.fresh_id("torn");
        let list = self.b.doc().get_list("commands");
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
        map.insert("kind", "interrupt").unwrap();
        stage(self);
        let payload = std::collections::HashMap::from([("kind".to_string(), "interrupt".into())]);
        map.insert("payload", loro::LoroValue::from(payload))
            .unwrap();
        map.insert("issuedBy", "peer").unwrap();
        map.insert("issuedAt", 5i64).unwrap();
        stage(self);
        map.insert("status", "pending").unwrap();
        stage(self);
        self.ids.push(id);
        for bytes in stages {
            self.a.doc().import(&bytes).unwrap();
            let seen = self.seen.take();
            assert!(
                seen.contains(ChangeMask::COMMANDS),
                "torn stage reports commands"
            );
            self.carry = self.carry.union(seen);
            self.check_cache(true);
        }
    }

    fn step(&mut self) {
        let before = snapshot(&self.a);
        let allowed = self.op();
        let after = snapshot(&self.a);
        let seen = self.seen.take().union(std::mem::take(&mut self.carry));
        let truth = changed_roots(&before, &after);
        assert!(
            seen.contains(truth),
            "op {} mask {seen:?} must cover the roots that changed {truth:?}",
            self.last_op
        );
        if let Some(allowed) = allowed {
            assert!(
                allowed.union(seen) == allowed,
                "a local op touching {allowed:?} reported {seen:?}"
            );
        }
        // A skipped torn import already checked itself; this read is the
        // ordinary per-tick one.
        self.check_cache(seen.contains(ChangeMask::COMMANDS));
    }
}

fn seeds() -> u64 {
    std::env::var("PERF_SEEDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(300)
}

#[test]
fn mask_covers_every_change_and_cache_equals_read_commands() {
    let mut avoided = 0;
    for seed in 1..=seeds() {
        let mut world = World::new(seed);
        for _ in 0..150 {
            world.step();
        }
        avoided += world.skipped_fills;
    }
    assert!(avoided > 0, "the generator exercised cache hits");
}

#[test]
fn unclassified_events_invalidate_everything() {
    // A doc whose root cannot be attributed reports every bit.
    let doc = Arc::new(SessionDoc::init("chat").unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let _sub = {
        let seen = seen.clone();
        doc.doc().subscribe_root(Arc::new(move |event| {
            seen.lock().unwrap().push(ChangeMask::of_event(&event));
        }))
    };
    doc.queue_command(&command("c1", SessionCommandPayload::Interrupt {}, 1))
        .unwrap();
    doc.push_message(&message("m1")).unwrap();
    doc.push_queued(&QueuedMessage::new("q1", "text", "dev"))
        .unwrap();
    assert_eq!(
        *seen.lock().unwrap(),
        vec![
            ChangeMask::COMMANDS,
            ChangeMask::MESSAGES,
            ChangeMask::QUEUE
        ]
    );
}

#[test]
fn queue_command_invalidates_before_it_returns() {
    // Precondition for the cache: Loro emits synchronously inside the commit,
    // so a write is visible to the very next `get`.
    let doc = Arc::new(SessionDoc::init("chat").unwrap());
    let cache = Arc::new(CommandsCache::new().with_verify(Verify::Never));
    let _sub = {
        let cache = cache.clone();
        doc.doc().subscribe_root(Arc::new(move |event| {
            if ChangeMask::of_event(&event).contains(ChangeMask::COMMANDS) {
                cache.invalidate();
            }
        }))
    };
    assert!(cache.get(&doc).unwrap().is_empty());
    doc.queue_command(&command("c1", SessionCommandPayload::Interrupt {}, 1))
        .unwrap();
    assert_eq!(cache.get(&doc).unwrap().len(), 1);
    doc.set_command_status("c1", SessionCommandStatus::Applied, None)
        .unwrap();
    assert_eq!(
        cache.get(&doc).unwrap()[0].status,
        SessionCommandStatus::Applied
    );
    assert_eq!(cache.fills(), 3);
    // Unrelated writes keep the cache.
    doc.push_message(&message("m1")).unwrap();
    doc.push_queued(&QueuedMessage::new("q1", "text", "dev"))
        .unwrap();
    cache.get(&doc).unwrap();
    assert_eq!(cache.fills(), 3);
}

#[test]
fn cache_reconciles_writes_without_an_observer() {
    let doc = SessionDoc::init("chat").unwrap();
    let cache = CommandsCache::new().with_verify(Verify::Always);
    doc.queue_command(&command("c1", SessionCommandPayload::Interrupt {}, 1))
        .unwrap();
    assert_eq!(cache.get(&doc).unwrap().len(), 1);
    // No observer is wired, so the cache is never told about this write.
    doc.queue_command(&command("c2", SessionCommandPayload::Interrupt {}, 2))
        .unwrap();
    let got = cache.get(&doc).unwrap();
    assert_eq!(cache.mismatches(), 0, "operation spans catch callback lag");
    assert_eq!(got.len(), 2, "the fresh result wins");
    assert_eq!(cache.get(&doc).unwrap().len(), 2);
    assert_eq!(cache.mismatches(), 0);
}

#[test]
fn cache_sees_command_outcomes_before_callback_invalidation() {
    for verify in [Verify::Never, Verify::Always] {
        for remote in [false, true] {
            let doc = Arc::new(SessionDoc::init("chat").unwrap());
            doc.queue_command(&command("c1", SessionCommandPayload::Interrupt {}, 1))
                .unwrap();
            let cache = Arc::new(CommandsCache::new().with_verify(verify));
            assert_eq!(
                cache.get(&doc).unwrap()[0].status,
                SessionCommandStatus::Pending
            );
            let peer = SessionDoc::from_doc(LoroDoc::new());
            peer.doc().import(&doc.export_snapshot().unwrap()).unwrap();
            peer.set_command_status("c1", SessionCommandStatus::Applied, None)
                .unwrap();
            let update = peer.doc().export(ExportMode::all_updates()).unwrap();

            let (entered_tx, entered_rx) = std::sync::mpsc::channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel();
            let release_rx = Mutex::new(release_rx);
            let _sub = {
                let cache = cache.clone();
                doc.doc().subscribe_root(Arc::new(move |_| {
                    entered_tx.send(()).unwrap();
                    release_rx.lock().unwrap().recv().unwrap();
                    cache.invalidate();
                }))
            };
            let writer = {
                let doc = doc.clone();
                std::thread::spawn(move || {
                    if remote {
                        doc.doc().import(&update).unwrap();
                    } else {
                        doc.set_command_status("c1", SessionCommandStatus::Applied, None)
                            .unwrap();
                    }
                })
            };
            entered_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            let fresh = doc.read_commands().unwrap();
            let (read_tx, read_rx) = std::sync::mpsc::channel();
            let reader = {
                let (cache, doc) = (cache.clone(), doc.clone());
                std::thread::spawn(move || read_tx.send(cache.get(&doc).unwrap()).unwrap())
            };
            // A reader may wait for Loro's transaction lock, but it must not
            // return the old cached status while invalidation is held back.
            let early = read_rx.recv_timeout(std::time::Duration::from_millis(50));
            release_tx.send(()).unwrap();
            writer.join().unwrap();
            let got = early.unwrap_or_else(|_| {
                read_rx
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .unwrap()
            });
            reader.join().unwrap();
            assert_eq!(*got, fresh, "remote={remote}, verify={verify:?}");
            assert_eq!(
                cache.mismatches(),
                0,
                "callback lag is not cache divergence"
            );
        }
    }
}

#[test]
fn cache_sees_uncommitted_command_edits() {
    let doc = SessionDoc::init("chat").unwrap();
    doc.queue_command(&command("c1", SessionCommandPayload::Interrupt {}, 1))
        .unwrap();
    let cache = CommandsCache::new().with_verify(Verify::Never);
    cache.get(&doc).unwrap();
    let Some(loro::ValueOrContainer::Container(loro::Container::Map(map))) =
        doc.doc().get_list("commands").get(0)
    else {
        panic!("command map")
    };
    map.insert("status", "cancelled").unwrap();
    assert_eq!(*cache.get(&doc).unwrap(), doc.read_commands().unwrap());
}
