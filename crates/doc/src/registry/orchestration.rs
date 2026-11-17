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
    Ask, AskState, CrewMember, Dump, OrchestrationSettings, OrchestrationState, Orchestrator, Progress, Route,
    WorkItem,
};

use super::*;

pub const KIND_ORCHESTRATORS: &str = "orchestrators";
pub const KIND_CREW: &str = "crew";
pub const KIND_WORK_ITEMS: &str = "workItems";
pub const KIND_ASKS: &str = "asks";
pub const KIND_DUMPS: &str = "dumps";
/// The profile's orchestration settings: one row, [`SETTINGS_ROW`].
pub const KIND_SETTINGS: &str = "orchestrationSettings";
/// The settings row's id.
pub const SETTINGS_ROW: &str = "settings";
/// How far each chat's work has got (`Progress`), keyed by chat id, written
/// by the chat's host only. Not one of [`ORCHESTRATION_KINDS`]: it changes
/// far more often than the rest and has its own watch (`WatchProgress`).
pub const KIND_PROGRESS: &str = "progress";

/// Every orchestration kind (publishers watch their versions together).
pub const ORCHESTRATION_KINDS: [&str; 6] = [
    KIND_ORCHESTRATORS,
    KIND_CREW,
    KIND_WORK_ITEMS,
    KIND_ASKS,
    KIND_DUMPS,
    KIND_SETTINGS,
];

/// `WatchOrchestration` sends at most this many dumps, newest first.
pub const MAX_DUMPS_IN_STATE: usize = 50;

/// Closed asks (answered or dismissed) stay in the state `WatchOrchestration`
/// sends this long after they closed. Older ones stay in the registry (synced,
/// saved, readable by id) but out of the state every reader copies.
pub const CLOSED_ASKS_IN_STATE_DAYS: i64 = 14;
/// The newest closed asks the state always keeps, however old (Recent answers
/// shows the latest few).
pub const CLOSED_ASKS_IN_STATE_AT_LEAST: usize = 20;

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

/// The settings as their row holds them: the settings and the row's id.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
struct SettingsRow {
    id: String,
    #[serde(flatten)]
    settings: OrchestrationSettings,
}

impl Entity for SettingsRow {
    const KIND: &'static str = KIND_SETTINGS;
    const REQUIRED: &'static [&'static str] = &["id"];
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

    fn read_entities_with<T: Entity, O>(&self, pick: impl FnOnce(&std::collections::HashMap<String, T>) -> O) -> O {
        self.read_cached_with::<Lenient<T>, T, O>(T::KIND, unwrap::<T>, pick)
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

    /// Write the profile's orchestration settings.
    pub fn upsert_orchestration_settings(&mut self, settings: &OrchestrationSettings) {
        self.write_entity(&SettingsRow { id: SETTINGS_ROW.to_owned(), settings: settings.clone() });
    }

    /// The profile's orchestration settings (the defaults until written).
    pub fn orchestration_settings(&self) -> OrchestrationSettings {
        self.read_entity::<SettingsRow>(SETTINGS_ROW).map(|row| row.settings.migrated()).unwrap_or_default()
    }

    /// Changes whenever a progress row may have changed.
    pub fn progress_version(&self) -> u64 {
        self.kind_version(KIND_PROGRESS)
    }

    /// The newest `n` dumps, newest first (only those are copied).
    pub fn read_newest_dumps(&self, n: usize) -> Vec<Dump> {
        self.read_entities_with(|rows: &std::collections::HashMap<String, Dump>| {
            let mut all: Vec<&Dump> = rows.values().collect();
            let newest_first = |a: &&Dump, b: &&Dump| b.created_at.cmp(&a.created_at).then_with(|| a.id.cmp(&b.id));
            if all.len() > n {
                all.select_nth_unstable_by(n, newest_first);
                all.truncate(n);
            }
            all.sort_by(newest_first);
            all.into_iter().cloned().collect()
        })
    }

    /// The asks the state holds as of `now`, oldest first: every open one,
    /// and the closed ones from the last [`CLOSED_ASKS_IN_STATE_DAYS`] or among
    /// the newest [`CLOSED_ASKS_IN_STATE_AT_LEAST`] closed.
    pub fn read_current_asks(&self, now: DateTime<Utc>) -> Vec<Ask> {
        let since = now - chrono::TimeDelta::days(CLOSED_ASKS_IN_STATE_DAYS);
        let closed_at = |a: &Ask| a.answered_at.or(a.updated_at).unwrap_or(a.created_at);
        self.read_entities_with(|rows: &std::collections::HashMap<String, Ask>| {
            let (open, mut closed): (Vec<&Ask>, Vec<&Ask>) =
                rows.values().partition(|a| a.state == AskState::Open);
            closed.sort_by(|a, b| closed_at(b).cmp(&closed_at(a)).then_with(|| a.id.cmp(&b.id)));
            let kept = closed
                .into_iter()
                .enumerate()
                .filter(|(i, a)| *i < CLOSED_ASKS_IN_STATE_AT_LEAST || closed_at(a) >= since)
                .map(|(_, a)| a);
            let mut asks: Vec<Ask> = open.into_iter().chain(kept).cloned().collect();
            asks.sort_by(|a, b| a.created_at.cmp(&b.created_at).then_with(|| a.id.cmp(&b.id)));
            asks
        })
    }

    /// Everything, as `WatchOrchestration` sends it: the newest
    /// [`MAX_DUMPS_IN_STATE`] dumps and the current asks
    /// ([`Self::read_current_asks`]). What's left out stays in the registry.
    pub fn read_orchestration(&self) -> OrchestrationState {
        let perf_t = std::time::Instant::now();
        let _perf = PerfLog(perf_t);
        OrchestrationState {
            orchestrators: self.read_orchestrators(),
            crew: self.read_crew(),
            // A card taken off the board is gone for every reader.
            items: self.read_work_items().into_iter().filter(|i| !i.removed).collect(),
            asks: self.read_current_asks(Utc::now()),
            dumps: self.read_newest_dumps(MAX_DUMPS_IN_STATE),
            settings: self.orchestration_settings(),
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
            space_id: None,
            device_id: None,
            waiting: None,
            moved_at: None,
            moved_route_state: None,
            approved_at: None,
            paused_at: None,
            check: None,
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
            review: None,
            answered_at: None,
            item_id: None,
            space_id: None,
            device_id: None,
            attachments: Default::default(),
            delivered_at: Default::default(),
            reference: None,
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
            refs: Vec::new(),
        }
    }

    #[test]
    fn a_decision_ask_keeps_its_pictures_pick_and_deferral_and_lets_them_go() {
        let mut doc = RegistryDoc::new("dev-a");
        let mut ask: Ask = serde_json::from_value(serde_json::json!({
            "id": "a1", "fromChat": "c1", "kind": "decision", "question": "Which?", "state": "open",
            "createdAt": "2026-11-13T10:00:00Z", "turnId": "m1"
        }))
        .unwrap();
        ask.options = vec![
            AskOption {
                label: "(a) Tag".into(),
                detail: "No lines".into(),
                images: vec!["/u/1-tag.png".into()],
                scores: Scores(vec![("Claude".into(), Score::Number(8.0)), ("GPT".into(), Score::Number(8.5))]),
            },
            AskOption::new("(b) Edge"),
        ];
        ask.images = vec!["https://x/all.png".into()];
        ask.recommendation = Some(AskRecommendation { option: Some(0), label: Some("(a) Tag".into()), why: Some("Cleaner".into()) });
        ask.links = vec![WorkLink { kind: LinkKind::Branch, label: "orch/x".into(), target: "orch/x".into() }];
        ask.context = Some("Built on `c1e`.".into());
        ask.deferred_until = Some(ts(9_000));
        doc.upsert_ask(&ask);
        assert_eq!(doc.ask("a1").unwrap(), ask);
        // Back from deferral, its pictures dropped: gone from the row too.
        ask.deferred_until = None;
        ask.images.clear();
        doc.upsert_ask(&ask);
        assert_eq!(doc.ask("a1").unwrap(), ask);
        assert_eq!(doc.read_asks(), vec![ask]);
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
            archived_seen_at: Some(ts(1_600)),
            revived_at: Some(ts(1_700)),
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
            options: vec![AskOption::new("Yes")],
            urgent: true,
            state: AskState::Open,
            answer: None,
            created_at: ts(1_600),
            answered_at: None,
            dump_id: None,
            route_id: None,
            quiet: Default::default(),
            origin: Default::default(),
            turn_id: None,
            updated_at: None,
            escalated_at: None,
            answered_via: None,
            images: Vec::new(),
            recommendation: None,
            links: Vec::new(),
            context: None,
            deferred_until: None,
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
        // The same fifty, in the same order, as a full read cut to fifty.
        let mut all = doc.read_dumps();
        all.truncate(MAX_DUMPS_IN_STATE);
        assert_eq!(state.dumps, all);
    }

    #[test]
    fn state_keeps_open_and_recent_asks_and_the_registry_keeps_them_all() {
        let now = ts(1_800_000_000_000);
        let day = 24 * 3600 * 1000;
        let ask = |id: &str, state: &str, created: i64, answered: Option<i64>| -> Ask {
            let mut a: Ask = serde_json::from_value(serde_json::json!({
                "id": id, "fromChat": "c1", "kind": "question", "question": id, "state": state,
                "createdAt": ts(created),
            }))
            .unwrap();
            a.answered_at = answered.map(ts);
            a
        };
        let mut doc = RegistryDoc::new("dev-a");
        let old = now.timestamp_millis() - 60 * day;
        // An open ask stays however old.
        doc.upsert_ask(&ask("open-old", "open", old, None));
        // 30 answered two months ago: only the newest 20 stay.
        for i in 0..30 {
            doc.upsert_ask(&ask(&format!("old{i:02}"), "answered", old + i, Some(old + i)));
        }
        // Answered or dismissed this week: all stay.
        for i in 0..25 {
            let at = now.timestamp_millis() - 3 * day + i;
            doc.upsert_ask(&ask(&format!("new{i:02}"), if i % 2 == 0 { "answered" } else { "dismissed" }, at, Some(at)));
        }
        let asks = doc.read_current_asks(now);
        let ids: std::collections::HashSet<&str> = asks.iter().map(|a| a.id.as_str()).collect();
        assert!(ids.contains("open-old"));
        assert!((0..25).all(|i| ids.contains(format!("new{i:02}").as_str())), "a recent closed ask was dropped");
        assert!(!ids.iter().any(|id| id.starts_with("old")), "25 recent ones are past the floor: old ones go");
        assert_eq!(asks.len(), 26);
        assert!(asks.windows(2).all(|w| w[0].created_at <= w[1].created_at), "oldest first");
        // Out of the state, not out of the registry.
        assert_eq!(doc.read_asks().len(), 56);
        assert!(doc.ask("old00").is_some());
        // With few recent ones, the newest closed stay up to the floor.
        let mut quiet = RegistryDoc::new("dev-a");
        for i in 0..30 {
            quiet.upsert_ask(&ask(&format!("old{i:02}"), "answered", old + i, Some(old + i)));
        }
        let kept = quiet.read_current_asks(now);
        assert_eq!(kept.len(), CLOSED_ASKS_IN_STATE_AT_LEAST);
        assert!(kept.iter().any(|a| a.id == "old29") && !kept.iter().any(|a| a.id == "old00"));
    }
}

/// perf-audit: logs how long a read_orchestration took (dropped at its end).
struct PerfLog(std::time::Instant);
impl Drop for PerfLog {
    fn drop(&mut self) {
        tracing::debug!(target: "perf", what = "read_orchestration", us = self.0.elapsed().as_micros() as u64);
    }
}
