//! Orchestrators, their crews, the board, asks and Brain dumps
//! (docs/design/orchestrators.md, wire types in `zeron_proto::orchestration`).
//!
//! Five synced kinds, one row per entity, keyed by its id (a crew row by its
//! thread's chat id). Each top-level field of the entity is its own registry
//! field, so concurrent edits of different fields on two devices both land
//! (per-field LWW, like chats). A dump's routes are one field each
//! (`route<id>`), so a route recorded on one device and a reroute made on
//! another never undo each other.
//!
//! A write carries only the fields whose value differs from what the row
//! shows here: an upsert of a whole entity is cheap, and never clobbers a
//! field another device changed meanwhile that this one didn't touch.
//!
//! Reads are lenient: a field this build can't read (a newer peer's shape) is
//! passed over instead of costing the row; only a row missing what identifies
//! it is skipped. Nothing here deletes a row.

use serde::de::DeserializeOwned;
use serde_json::Map;
use zeron_proto::orchestration::{
    Ask, CrewMember, Dump, OrchestrationState, Orchestrator, Progress, Route, WorkItem,
};

use super::*;

pub const KIND_ORCHESTRATORS: &str = "orchestrators";
pub const KIND_CREW: &str = "crew";
pub const KIND_WORK_ITEMS: &str = "workItems";
pub const KIND_ASKS: &str = "asks";
pub const KIND_DUMPS: &str = "dumps";
/// How far each chat's work has got (`Progress`), keyed by chat id, written
/// by the chat's host only. Not one of [`ORCHESTRATION_KINDS`]: it changes
/// far more often than the rest and has its own watch (`WatchProgress`).
pub const KIND_PROGRESS: &str = "progress";

/// Every orchestration kind (publishers watch their versions together).
pub const ORCHESTRATION_KINDS: [&str; 5] = [
    KIND_ORCHESTRATORS,
    KIND_CREW,
    KIND_WORK_ITEMS,
    KIND_ASKS,
    KIND_DUMPS,
];

/// `WatchOrchestration` sends at most this many dumps, newest first.
pub const MAX_DUMPS_IN_STATE: usize = 50;

/// A text field is cut to this many bytes before it's written: a row is an
/// index entry, and the room refuses (for good) an op over
/// [`MAX_OP_BYTES`].
pub const MAX_TEXT_FIELD_BYTES: usize = 12 * 1024;

/// The prefix of a dump's route fields.
const ROUTE_FIELD: &str = "route";
/// Position of a route in its dump, stored inside the route's field.
const ROUTE_ORDER: &str = "order";

/// One orchestration entity as registry rows hold it.
trait Entity: Serialize + DeserializeOwned + Clone + Send + std::fmt::Debug + 'static {
    const KIND: &'static str;
    /// Fields without which the row isn't this entity.
    const REQUIRED: &'static [&'static str];
    fn row_id(&self) -> &str;

    /// The registry fields of this value.
    fn to_fields(&self) -> Map<String, Value> {
        match serde_json::to_value(self) {
            Ok(Value::Object(map)) => map,
            _ => Map::new(),
        }
    }

    /// Row fields back into the entity's own shape, before decoding.
    fn from_fields(fields: Map<String, Value>) -> Map<String, Value> {
        fields
    }
}

impl Entity for Orchestrator {
    const KIND: &'static str = KIND_ORCHESTRATORS;
    const REQUIRED: &'static [&'static str] =
        &["id", "kind", "name", "hue", "createdAt", "updatedAt"];
    fn row_id(&self) -> &str {
        &self.id
    }
}

impl Entity for CrewMember {
    const KIND: &'static str = KIND_CREW;
    const REQUIRED: &'static [&'static str] = &["chatId", "orchestratorId", "joinedAt"];
    fn row_id(&self) -> &str {
        &self.chat_id
    }
}

impl Entity for WorkItem {
    const KIND: &'static str = KIND_WORK_ITEMS;
    const REQUIRED: &'static [&'static str] =
        &["id", "title", "state", "priority", "createdAt", "updatedAt"];
    fn row_id(&self) -> &str {
        &self.id
    }
}

impl Entity for Ask {
    const KIND: &'static str = KIND_ASKS;
    const REQUIRED: &'static [&'static str] =
        &["id", "fromChat", "kind", "question", "state", "createdAt"];
    fn row_id(&self) -> &str {
        &self.id
    }
}

impl Entity for Progress {
    const KIND: &'static str = KIND_PROGRESS;
    const REQUIRED: &'static [&'static str] = &["chatId", "deviceId", "state", "updatedAt"];
    fn row_id(&self) -> &str {
        &self.chat_id
    }
}

/// The registry field of a route: `route` + its id's letters and digits (the
/// room's field names are alphanumeric, ≤ 64 characters).
fn route_field(route_id: &str) -> String {
    let id: String = route_id
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(64 - ROUTE_FIELD.len())
        .collect();
    format!("{ROUTE_FIELD}{id}")
}

fn is_route_field(key: &str) -> bool {
    key.len() > ROUTE_FIELD.len() && key.starts_with(ROUTE_FIELD) && key != "routes"
}

impl Entity for Dump {
    const KIND: &'static str = KIND_DUMPS;
    const REQUIRED: &'static [&'static str] =
        &["id", "text", "state", "deviceId", "createdAt", "updatedAt"];
    fn row_id(&self) -> &str {
        &self.id
    }

    fn to_fields(&self) -> Map<String, Value> {
        let mut map = match serde_json::to_value(self) {
            Ok(Value::Object(map)) => map,
            _ => return Map::new(),
        };
        map.remove("routes");
        for (order, route) in self.routes.iter().enumerate() {
            if let Ok(Value::Object(mut value)) = serde_json::to_value(route) {
                value.insert(ROUTE_ORDER.into(), json!(order));
                map.insert(route_field(&route.id), Value::Object(value));
            }
        }
        map
    }

    fn from_fields(mut fields: Map<String, Value>) -> Map<String, Value> {
        let keys: Vec<String> = fields
            .keys()
            .filter(|k| is_route_field(k))
            .cloned()
            .collect();
        let mut routes: Vec<(u64, String, Value)> = Vec::new();
        for key in keys {
            let Some(value) = fields.remove(&key) else {
                continue;
            };
            let order = value
                .get(ROUTE_ORDER)
                .and_then(Value::as_u64)
                .unwrap_or(u64::MAX);
            // (A route this build can't read is passed over, never the dump.)
            if let Ok(route) = serde_json::from_value::<Route>(value) {
                routes.push((
                    order,
                    route.id.clone(),
                    serde_json::to_value(route).unwrap_or(Value::Null),
                ));
            }
        }
        routes.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        fields.insert(
            "routes".into(),
            Value::Array(routes.into_iter().map(|(_, _, v)| v).collect()),
        );
        fields
    }
}

/// Decode `fields` as `T`, passing over any field that doesn't read; `None`
/// when what's required doesn't.
fn lenient<T: DeserializeOwned>(fields: Map<String, Value>, required: &[&str]) -> Option<T> {
    if let Ok(value) = serde_json::from_value::<T>(Value::Object(fields.clone())) {
        return Some(value);
    }
    let mut kept: Map<String, Value> = fields
        .iter()
        .filter(|(k, _)| required.contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let mut decoded = serde_json::from_value::<T>(Value::Object(kept.clone())).ok()?;
    for (key, value) in fields {
        if required.contains(&key.as_str()) {
            continue;
        }
        kept.insert(key.clone(), value);
        match serde_json::from_value::<T>(Value::Object(kept.clone())) {
            Ok(next) => decoded = next,
            Err(_) => {
                kept.remove(&key);
            }
        }
    }
    Some(decoded)
}

/// What a registry row of `T` decodes as (`read_cached`'s raw type).
struct Lenient<T>(T);

impl<'de, T: Entity> Deserialize<'de> for Lenient<T> {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let fields = Map::<String, Value>::deserialize(d)?;
        lenient::<T>(T::from_fields(fields), T::REQUIRED)
            .map(Lenient)
            .ok_or_else(|| serde::de::Error::custom(format!("not a {} row", T::KIND)))
    }
}

fn unwrap<T>(lenient: Lenient<T>) -> T {
    lenient.0
}

/// `value` with any string longer than [`MAX_TEXT_FIELD_BYTES`] cut (at a
/// character boundary).
fn clamp_text(value: Value) -> Value {
    match value {
        Value::String(s) if s.len() > MAX_TEXT_FIELD_BYTES => {
            let mut end = MAX_TEXT_FIELD_BYTES;
            while !s.is_char_boundary(end) {
                end -= 1;
            }
            Value::String(s[..end].to_owned())
        }
        other => other,
    }
}

impl RegistryDoc {
    /// LWW write of the fields of `entity` that differ from its row (a
    /// field the row has that `entity` no longer has is cleared). Creates
    /// the row when missing. Split into several ops when one would pass the
    /// room's size limit; a single field too large for any op is left out
    /// (logged), never sent to wedge the queue.
    fn write_entity<T: Entity>(&mut self, entity: &T) {
        let id = entity.row_id().to_owned();
        if id.is_empty() {
            return;
        }
        let current = self.overlay_row(T::KIND, &id);
        let next = entity.to_fields();
        let mut set: BTreeMap<String, Value> = BTreeMap::new();
        for (key, value) in next.iter() {
            let value = clamp_text(value.clone());
            let now = current.as_ref().and_then(|row| row.fields.get(key));
            if now != Some(&value) && !(now.is_none() && value.is_null()) {
                set.insert(key.clone(), value);
            }
        }
        if let Some(row) = &current {
            for key in row.fields.keys() {
                if !next.contains_key(key) && !row.fields[key].is_null() {
                    set.insert(key.clone(), Value::Null);
                }
            }
        }
        if set.is_empty() {
            return;
        }
        let hlc = self.next_hlc();
        let op_with = |set: BTreeMap<String, Value>| RowOp {
            kind: T::KIND.to_owned(),
            id: id.clone(),
            op: OpKind::Upsert,
            set: Some(set),
            hlc: hlc.clone(),
            clocks: None,
        };
        let whole = op_with(set.clone());
        let ops = if fits_op_budget(&whole) {
            vec![whole]
        } else {
            let mut ops = Vec::new();
            let mut chunk: BTreeMap<String, Value> = BTreeMap::new();
            for (key, value) in set {
                let mut grown = chunk.clone();
                grown.insert(key.clone(), value.clone());
                if fits_op_budget(&op_with(grown.clone())) {
                    chunk = grown;
                    continue;
                }
                if !chunk.is_empty() {
                    ops.push(op_with(std::mem::take(&mut chunk)));
                }
                let alone = BTreeMap::from([(key.clone(), value)]);
                if fits_op_budget(&op_with(alone.clone())) {
                    chunk = alone;
                } else {
                    tracing::warn!(kind = T::KIND, row = %id, field = %key, "orchestration field too large to sync; left out");
                }
            }
            if !chunk.is_empty() {
                ops.push(op_with(chunk));
            }
            ops
        };
        self.enqueue_ops(ops);
    }

    fn read_entity<T: Entity>(&self, id: &str) -> Option<T> {
        let row = self.overlay_row(T::KIND, id)?;
        let fields: Map<String, Value> = row.fields.into_iter().collect();
        lenient::<T>(T::from_fields(fields), T::REQUIRED)
    }

    fn read_entities<T: Entity>(&self) -> Vec<T> {
        self.read_cached::<Lenient<T>, T>(T::KIND, unwrap::<T>)
    }

    /// Changes whenever any orchestration row may have changed.
    pub fn orchestration_version(&self) -> u64 {
        ORCHESTRATION_KINDS
            .iter()
            .fold(0u64, |sum, kind| sum.wrapping_add(self.kind_version(kind)))
    }

    // ── orchestrators ───────────────────────────────────────────────────────

    pub fn upsert_orchestrator(&mut self, orchestrator: &Orchestrator) {
        self.write_entity(orchestrator);
    }

    pub fn orchestrator(&self, id: &str) -> Option<Orchestrator> {
        self.read_entity(id)
    }

    /// Oldest first.
    pub fn read_orchestrators(&self) -> Vec<Orchestrator> {
        let mut all: Vec<Orchestrator> = self.read_entities();
        all.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        all
    }

    // ── crew ────────────────────────────────────────────────────────────────

    pub fn upsert_crew_member(&mut self, member: &CrewMember) {
        self.write_entity(member);
    }

    /// The crew row of a thread (released ones too).
    pub fn crew_member(&self, chat_id: &str) -> Option<CrewMember> {
        self.read_entity(chat_id)
    }

    /// By join time.
    pub fn read_crew(&self) -> Vec<CrewMember> {
        let mut all: Vec<CrewMember> = self.read_entities();
        all.sort_by(|a, b| {
            a.joined_at
                .cmp(&b.joined_at)
                .then_with(|| a.chat_id.cmp(&b.chat_id))
        });
        all
    }

    // ── work items ──────────────────────────────────────────────────────────

    pub fn upsert_work_item(&mut self, item: &WorkItem) {
        self.write_entity(item);
    }

    pub fn work_item(&self, id: &str) -> Option<WorkItem> {
        self.read_entity(id)
    }

    /// By `order`, then creation.
    pub fn read_work_items(&self) -> Vec<WorkItem> {
        let mut all: Vec<WorkItem> = self.read_entities();
        all.sort_by(|a, b| {
            a.order
                .total_cmp(&b.order)
                .then_with(|| a.created_at.cmp(&b.created_at))
                .then_with(|| a.id.cmp(&b.id))
        });
        all
    }

    // ── asks ────────────────────────────────────────────────────────────────

    pub fn upsert_ask(&mut self, ask: &Ask) {
        self.write_entity(ask);
    }

    pub fn ask(&self, id: &str) -> Option<Ask> {
        self.read_entity(id)
    }

    /// Oldest first.
    pub fn read_asks(&self) -> Vec<Ask> {
        let mut all: Vec<Ask> = self.read_entities();
        all.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        all
    }

    // ── dumps ───────────────────────────────────────────────────────────────

    /// Writes the dump's fields and each of its routes that changed.
    pub fn upsert_dump(&mut self, dump: &Dump) {
        self.write_entity(dump);
    }

    pub fn dump(&self, id: &str) -> Option<Dump> {
        self.read_entity(id)
    }

    /// Every dump, newest first.
    pub fn read_dumps(&self) -> Vec<Dump> {
        let mut all: Vec<Dump> = self.read_entities();
        all.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        all
    }

    // ── progress ────────────────────────────────────────────────────────────

    /// Writes the fields that changed (the host's own row).
    pub fn upsert_progress(&mut self, progress: &Progress) {
        self.write_entity(progress);
    }

    pub fn progress(&self, chat_id: &str) -> Option<Progress> {
        self.read_entity(chat_id)
    }

    /// Every chat's, newest change first.
    pub fn read_progress(&self) -> Vec<Progress> {
        let mut all: Vec<Progress> = self.read_entities();
        all.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| a.chat_id.cmp(&b.chat_id))
        });
        all
    }

    /// Changes whenever a progress row may have changed.
    pub fn progress_version(&self) -> u64 {
        self.kind_version(KIND_PROGRESS)
    }

    /// Everything, as `WatchOrchestration` sends it (the newest
    /// [`MAX_DUMPS_IN_STATE`] dumps).
    pub fn read_orchestration(&self) -> OrchestrationState {
        let mut dumps = self.read_dumps();
        dumps.truncate(MAX_DUMPS_IN_STATE);
        OrchestrationState {
            orchestrators: self.read_orchestrators(),
            crew: self.read_crew(),
            // A card taken off the board is gone for every reader.
            items: self.read_work_items().into_iter().filter(|i| !i.removed).collect(),
            asks: self.read_asks(),
            dumps,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::server_round;
    use super::*;
    use zeron_proto::orchestration::*;

    fn ts(ms: i64) -> DateTime<Utc> {
        DateTime::from_timestamp_millis(ms).unwrap()
    }

    fn orchestrator(id: &str) -> Orchestrator {
        Orchestrator {
            id: id.into(),
            kind: OrchestratorKind::Standard,
            name: "Auth".into(),
            goal: "Ship login".into(),
            rules: String::new(),
            crew_rules: String::new(),
            notes: String::new(),
            hue: seed_hue(id),
            crew: CrewPolicy::default(),
            wake: None,
            paused: false,
            archived: false,
            created_at: ts(1_000),
            updated_at: ts(1_000),
        }
    }

    fn item(id: &str) -> WorkItem {
        WorkItem {
            id: id.into(),
            orchestrator_id: Some("o1".into()),
            title: "Fix login".into(),
            detail: String::new(),
            state: WorkState::Inbox,
            priority: Priority::Normal,
            progress: None,
            threads: vec![],
            links: vec![],
            outcome: None,
            source: None,
            order: 0.0,
            created_at: ts(1_000),
            updated_at: ts(1_000),
            done_at: None,
            removed: Default::default(),
            removed_at: Default::default(),
        }
    }

    fn route(id: &str, excerpt: &str) -> Route {
        Route {
            id: id.into(),
            excerpt: excerpt.into(),
            span: Some((0, excerpt.len() as u32)),
            target: RouteTarget::Orchestrator { id: "o1".into() },
            intent: RouteIntent::Task,
            message: excerpt.into(),
            title: None,
            state: RouteState::Delivered,
            candidates: vec![],
            answer: None,
            answer_full: None,
            item_id: None,
            space_id: None,
            device_id: None,
            attachments: Default::default(),
            delivered_at: Default::default(),
        }
    }

    fn dump(id: &str) -> Dump {
        Dump {
            id: id.into(),
            text: "fix login. buy milk".into(),
            state: DumpState::Routing,
            routes: vec![],
            summary: None,
            device_id: "dev-a".into(),
            created_at: ts(2_000),
            updated_at: ts(2_000),
            attachments: Default::default(),
        }
    }

    #[test]
    fn every_kind_round_trips() {
        let mut doc = RegistryDoc::new("dev-a");
        let o = orchestrator("o1");
        doc.upsert_orchestrator(&o);
        let member = CrewMember {
            chat_id: "c1".into(),
            orchestrator_id: "o1".into(),
            item_id: Some("i1".into()),
            role: "auth PR".into(),
            last_report: None,
            released: false,
            joined_at: ts(1_500),
        };
        doc.upsert_crew_member(&member);
        doc.upsert_work_item(&item("i1"));
        let ask = Ask {
            id: "a1".into(),
            from_chat: "c1".into(),
            orchestrator_id: Some("o1".into()),
            item_id: None,
            kind: AskKind::Decision,
            question: "Merge?".into(),
            options: vec![AskOption {
                label: "Yes".into(),
                detail: String::new(),
            }],
            urgent: true,
            state: AskState::Open,
            answer: None,
            created_at: ts(1_600),
            answered_at: None,
            dump_id: None,
            route_id: None,
            quiet: Default::default(),
        };
        doc.upsert_ask(&ask);
        let mut d = dump("d1");
        d.routes = vec![route("r2", "buy milk"), route("r1", "fix login")];
        doc.upsert_dump(&d);

        let state = doc.read_orchestration();
        assert_eq!(state.orchestrators, vec![o.clone()]);
        assert_eq!(state.crew, vec![member.clone()]);
        assert_eq!(state.items, vec![item("i1")]);
        assert_eq!(state.asks, vec![ask]);
        // Routes keep the order they were recorded in.
        assert_eq!(state.dumps, vec![d.clone()]);
        assert_eq!(doc.crew_member("c1"), Some(member));
        assert_eq!(doc.dump("d1"), Some(d));

        // Unchanged: no write at all.
        let pending = doc.pending_ops_len();
        doc.upsert_orchestrator(&o);
        assert_eq!(doc.pending_ops_len(), pending);
    }

    #[test]
    fn a_write_carries_only_what_changed_and_clears_what_went() {
        let mut doc = RegistryDoc::new("dev-a");
        let mut o = orchestrator("o1");
        o.wake = Some(WakeSchedule {
            every_minutes: Some(30),
            at: vec![],
            weekdays_only: false,
            prompt: "check in".into(),
            last_woke_at: None,
        });
        doc.upsert_orchestrator(&o);
        let before = doc.pending_ops_len();
        o.notes = "plan".into();
        o.wake = None;
        doc.upsert_orchestrator(&o);
        let last = doc.pending_ops().last().unwrap().clone();
        let set = last.set.unwrap();
        assert_eq!(set.keys().collect::<Vec<_>>(), vec!["notes", "wake"]);
        assert_eq!(set["wake"], Value::Null);
        assert!(doc.pending_ops_len() > before);
        assert_eq!(doc.orchestrator("o1").unwrap(), o);
    }

    #[test]
    fn a_field_a_newer_peer_wrote_costs_the_field_not_the_row() {
        let mut doc = RegistryDoc::new("dev-a");
        doc.upsert_work_item(&item("i1"));
        // A newer peer: an unknown state, a progress this build can't read,
        // and a field it doesn't know.
        let hlc = doc.next_hlc();
        doc.enqueue_ops(vec![RowOp {
            kind: KIND_WORK_ITEMS.into(),
            id: "i1".into(),
            op: OpKind::Update,
            set: Some(fields([
                ("state", json!("someday")),
                ("progress", json!("half")),
                ("mood", json!("sunny")),
            ])),
            hlc,
            clocks: None,
        }]);
        let read = doc.work_item("i1").unwrap();
        assert_eq!(read.state, WorkState::Other);
        assert_eq!(read.progress, None);
        assert_eq!(read.title, "Fix login");
        assert_eq!(doc.read_work_items().len(), 1);

        // Without what identifies it, it isn't an item.
        let hlc = doc.next_hlc();
        doc.enqueue_ops(vec![RowOp {
            kind: KIND_WORK_ITEMS.into(),
            id: "junk".into(),
            op: OpKind::Upsert,
            set: Some(fields([("title", json!("x"))])),
            hlc,
            clocks: None,
        }]);
        assert_eq!(doc.read_work_items().len(), 1);
    }

    #[test]
    fn two_devices_editing_different_fields_both_land() {
        let mut a = RegistryDoc::new("dev-a");
        let mut b = RegistryDoc::new("dev-b");
        let mut server = HashMap::new();
        let mut seq = 0u64;
        a.upsert_orchestrator(&orchestrator("o1"));
        let mut d = dump("d1");
        d.routes = vec![route("r1", "fix login")];
        a.upsert_dump(&d);
        server_round(&mut server, &mut seq, &mut [&mut a, &mut b]);
        assert_eq!(a.read_orchestration(), b.read_orchestration());

        // a edits the notes, b pauses it, concurrently.
        let mut on_a = a.orchestrator("o1").unwrap();
        on_a.notes = "plan".into();
        a.upsert_orchestrator(&on_a);
        let mut on_b = b.orchestrator("o1").unwrap();
        on_b.paused = true;
        b.upsert_orchestrator(&on_b);
        // a records a second route while b reroutes the first.
        let mut da = a.dump("d1").unwrap();
        da.routes.push(route("r2", "buy milk"));
        a.upsert_dump(&da);
        let mut db = b.dump("d1").unwrap();
        db.routes[0].target = RouteTarget::Chat { id: "c9".into() };
        db.state = DumpState::Routed;
        b.upsert_dump(&db);
        server_round(&mut server, &mut seq, &mut [&mut a, &mut b]);

        let sa = a.read_orchestration();
        assert_eq!(sa, b.read_orchestration());
        let o = &sa.orchestrators[0];
        assert_eq!((o.notes.as_str(), o.paused), ("plan", true));
        let d = &sa.dumps[0];
        assert_eq!(d.state, DumpState::Routed);
        assert_eq!(
            d.routes.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            vec!["r1", "r2"]
        );
        assert_eq!(d.routes[0].target, RouteTarget::Chat { id: "c9".into() });
    }

    #[test]
    fn large_text_is_cut_and_never_makes_an_op_the_room_refuses() {
        let mut doc = RegistryDoc::new("dev-a");
        let mut o = orchestrator("o1");
        o.notes = "n".repeat(40_000);
        o.rules = "r".repeat(9_000);
        o.crew_rules = "c".repeat(9_000);
        doc.upsert_orchestrator(&o);
        for op in doc.pending_ops() {
            assert!(
                fits_op_budget(op),
                "{} bytes",
                serde_json::to_vec(op).unwrap().len()
            );
        }
        let read = doc.orchestrator("o1").unwrap();
        assert_eq!(read.notes.len(), MAX_TEXT_FIELD_BYTES);
        assert_eq!(read.rules.len(), 9_000);
        assert_eq!(read.crew_rules.len(), 9_000);
    }

    #[test]
    fn route_fields_are_names_the_room_accepts() {
        let field = route_field("3f2a-bc_9");
        assert_eq!(field, "route3f2abc9");
        assert!(field.chars().all(|c| c.is_ascii_alphanumeric()) && field.len() <= 64);
        assert!(route_field(&"a".repeat(100)).len() <= 64);
        assert!(!is_route_field("routes"));
    }

    #[test]
    fn state_holds_the_newest_fifty_dumps() {
        let mut doc = RegistryDoc::new("dev-a");
        for i in 0..60 {
            let mut d = dump(&format!("d{i:02}"));
            d.created_at = ts(10_000 + i);
            doc.upsert_dump(&d);
        }
        let state = doc.read_orchestration();
        assert_eq!(state.dumps.len(), MAX_DUMPS_IN_STATE);
        assert_eq!(state.dumps[0].id, "d59");
        assert_eq!(doc.read_dumps().len(), 60, "older dumps stay");
    }
}
