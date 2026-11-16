//! `WatchOrchestration` as changes (`{"changes": true}`): the whole state
//! once, then per change only the rows that moved. The whole state is
//! ~0.5 MB with a few dozen cards, asks and dumps, and one card moving sent,
//! parsed and compared all of it, in every window.
//!
//! Frames, tagged by `frame`:
//! - `{"frame": "snapshot", "version": n, "state": {…}}`: the whole state, as
//!   the plain stream sends it. First, and again now and then (a reader that
//!   lost track is set right).
//! - `{"frame": "delta", "version": n, "items": {…}, …}`: per collection that
//!   changed (`orchestrators`, `crew`, `items`, `asks`, `dumps`), a
//!   [`ListDelta`]; and `settings`, whole, when they changed.
//!
//! `version` counts the frames of one subscription (the snapshot is 1). A
//! reader that sees a delta whose version isn't the one after the last it
//! applied has missed one: it watches again, and the new watch starts with
//! a snapshot. An engine from before this sends the bare state every time;
//! a reader takes a frame with no `frame` key as a snapshot with no version.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use super::{Ask, CrewMember, Dump, OrchestrationSettings, OrchestrationState, Orchestrator, WorkItem};

/// A row's key in its collection: its `id`, a crew row's `chatId`.
pub trait Keyed {
    fn key(&self) -> &str;
}

impl Keyed for Orchestrator {
    fn key(&self) -> &str {
        &self.id
    }
}
impl Keyed for CrewMember {
    fn key(&self) -> &str {
        &self.chat_id
    }
}
impl Keyed for WorkItem {
    fn key(&self) -> &str {
        &self.id
    }
}
impl Keyed for Ask {
    fn key(&self) -> &str {
        &self.id
    }
}
impl Keyed for Dump {
    fn key(&self) -> &str {
        &self.id
    }
}

/// What changed in one collection. Applying it (drop `removed`, replace or
/// add each of `changed`, then put the rows in `order` if it is there) gives
/// the collection as it now is. `replace` instead: the collection whole (its
/// keys aren't unique, so rows can't be told apart by them).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", bound(deserialize = "T: Deserialize<'de>", serialize = "T: Serialize"))]
pub struct ListDelta<T> {
    /// Rows new or not as they were, in the collection's order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changed: Vec<T>,
    /// The keys of rows gone.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<String>,
    /// Every key in order, sent only when applying the rest (kept rows where
    /// they were, new ones after) doesn't give the collection's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replace: Option<Vec<T>>,
}

impl<T: Keyed> ListDelta<T> {
    /// The keys of the rows it touches (`None`: all of them, a `replace`).
    pub fn touched(&self) -> Option<impl Iterator<Item = &str>> {
        if self.replace.is_some() {
            return None;
        }
        Some(self.changed.iter().map(Keyed::key).chain(self.removed.iter().map(String::as_str)))
    }
}

/// Each key of `rows` once, by place; `None` if one repeats.
fn places<T: Keyed>(rows: &[T]) -> Option<HashMap<&str, usize>> {
    let mut at = HashMap::with_capacity(rows.len());
    for (i, row) in rows.iter().enumerate() {
        if at.insert(row.key(), i).is_some() {
            return None;
        }
    }
    Some(at)
}

/// What turns `old` into `new` (`None`: they're the same).
pub fn diff_list<T: Keyed + PartialEq + Clone>(old: &[T], new: &[T]) -> Option<ListDelta<T>> {
    let (Some(was), Some(now)) = (places(old), places(new)) else {
        return (old != new).then(|| ListDelta {
            changed: Vec::new(),
            removed: Vec::new(),
            order: None,
            replace: Some(new.to_vec()),
        });
    };
    let changed: Vec<T> = new
        .iter()
        .filter(|row| was.get(row.key()).is_none_or(|&i| old[i] != **row))
        .cloned()
        .collect();
    let removed: Vec<String> =
        old.iter().map(Keyed::key).filter(|k| !now.contains_key(k)).map(str::to_owned).collect();
    // (The order applying the rest gives: kept rows where they were, new
    // ones after.)
    let kept = old.iter().map(Keyed::key).filter(|k| now.contains_key(k));
    let added = new.iter().map(Keyed::key).filter(|k| !was.contains_key(k));
    let reordered = !kept.chain(added).eq(new.iter().map(Keyed::key));
    if changed.is_empty() && removed.is_empty() && !reordered {
        return None;
    }
    Some(ListDelta {
        changed,
        removed,
        order: reordered.then(|| new.iter().map(|r| r.key().to_owned()).collect()),
        replace: None,
    })
}

/// `delta` applied to `rows`.
pub fn apply_list<T: Keyed + Clone>(rows: &mut Vec<T>, delta: &ListDelta<T>) {
    if let Some(all) = &delta.replace {
        rows.clone_from(all);
        return;
    }
    if !delta.removed.is_empty() {
        let gone: HashSet<&str> = delta.removed.iter().map(String::as_str).collect();
        rows.retain(|r| !gone.contains(r.key()));
    }
    let mut at: HashMap<String, usize> =
        rows.iter().enumerate().map(|(i, r)| (r.key().to_owned(), i)).collect();
    for row in &delta.changed {
        match at.get(row.key()) {
            Some(&i) => rows[i] = row.clone(),
            None => {
                at.insert(row.key().to_owned(), rows.len());
                rows.push(row.clone());
            }
        }
    }
    if let Some(order) = &delta.order {
        let place: HashMap<&str, usize> = order.iter().enumerate().map(|(i, k)| (k.as_str(), i)).collect();
        // (A row the order doesn't name, which a right delta never leaves,
        // goes last rather than being lost.)
        rows.sort_by_key(|r| place.get(r.key()).copied().unwrap_or(usize::MAX));
    }
}

/// What changed in the state, by collection.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestrationDelta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orchestrators: Option<ListDelta<Orchestrator>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crew: Option<ListDelta<CrewMember>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items: Option<ListDelta<WorkItem>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asks: Option<ListDelta<Ask>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dumps: Option<ListDelta<Dump>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<OrchestrationSettings>,
}

impl OrchestrationDelta {
    /// What turns `old` into `new` (`None`: they're the same).
    pub fn between(old: &OrchestrationState, new: &OrchestrationState) -> Option<Self> {
        let delta = Self {
            orchestrators: diff_list(&old.orchestrators, &new.orchestrators),
            crew: diff_list(&old.crew, &new.crew),
            items: diff_list(&old.items, &new.items),
            asks: diff_list(&old.asks, &new.asks),
            dumps: diff_list(&old.dumps, &new.dumps),
            settings: (old.settings != new.settings).then(|| new.settings.clone()),
        };
        (!delta.is_empty()).then_some(delta)
    }

    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// It applied to `state`.
    pub fn apply(&self, state: &mut OrchestrationState) {
        if let Some(d) = &self.orchestrators {
            apply_list(&mut state.orchestrators, d);
        }
        if let Some(d) = &self.crew {
            apply_list(&mut state.crew, d);
        }
        if let Some(d) = &self.items {
            apply_list(&mut state.items, d);
        }
        if let Some(d) = &self.asks {
            apply_list(&mut state.asks, d);
        }
        if let Some(d) = &self.dumps {
            apply_list(&mut state.dumps, d);
        }
        if let Some(s) = &self.settings {
            state.settings = s.clone();
        }
    }
}

/// One frame of `WatchOrchestration {changes: true}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "frame", rename_all = "camelCase")]
pub enum OrchestrationFrame {
    Snapshot { version: u64, state: OrchestrationState },
    Delta {
        version: u64,
        #[serde(flatten)]
        delta: OrchestrationDelta,
    },
}

/// A frame as a reader hears it: one of [`OrchestrationFrame`], or the bare
/// state an engine from before frames sends (a snapshot with no version).
#[derive(Debug, Clone, PartialEq)]
pub enum Heard {
    Snapshot { version: Option<u64>, state: OrchestrationState },
    Delta { version: u64, delta: OrchestrationDelta },
}

impl Heard {
    pub fn read(item: &serde_json::Value) -> Result<Self, serde_json::Error> {
        if item.get("frame").is_none() {
            return Ok(Heard::Snapshot { version: None, state: OrchestrationState::deserialize(item)? });
        }
        Ok(match OrchestrationFrame::deserialize(item)? {
            OrchestrationFrame::Snapshot { version, state } => Heard::Snapshot { version: Some(version), state },
            OrchestrationFrame::Delta { version, delta } => Heard::Delta { version, delta },
        })
    }
}

/// A subscription's frames, made one at a time from the states it is given:
/// a snapshot first (and when [`Framer::snapshot`] is asked), else a delta
/// from the last state given (nothing when nothing changed).
#[derive(Debug, Default)]
pub struct Framer {
    version: u64,
    last: Option<OrchestrationState>,
}

impl Framer {
    pub fn new() -> Self {
        Self::default()
    }

    /// The frame `state` makes, if any.
    pub fn frame(&mut self, state: &OrchestrationState) -> Option<OrchestrationFrame> {
        let frame = match &self.last {
            None => self.snapshot(state),
            Some(last) => {
                let delta = OrchestrationDelta::between(last, state)?;
                self.version += 1;
                OrchestrationFrame::Delta { version: self.version, delta }
            }
        };
        self.last = Some(state.clone());
        Some(frame)
    }

    /// `state` whole (the next frame, a snapshot whatever changed).
    pub fn snapshot(&mut self, state: &OrchestrationState) -> OrchestrationFrame {
        self.version += 1;
        self.last = Some(state.clone());
        OrchestrationFrame::Snapshot { version: self.version, state: state.clone() }
    }
}

/// The state a reader keeps from frames, and whether it lost track.
#[derive(Debug, Default)]
pub struct Follower {
    pub state: Option<OrchestrationState>,
    /// The last frame's version (`None`: none yet, or an engine without them).
    pub version: Option<u64>,
}

/// What a frame did to a [`Follower`].
#[derive(Debug, PartialEq)]
pub enum Followed {
    /// The state changed (a snapshot, or a delta that applied).
    Changed,
    /// A delta that doesn't follow the last frame (or came before any
    /// snapshot): nothing applied; the reader should watch again.
    Gap,
}

impl Follower {
    pub fn hear(&mut self, heard: Heard) -> Followed {
        match heard {
            Heard::Snapshot { version, state } => {
                self.state = Some(state);
                self.version = version;
                Followed::Changed
            }
            Heard::Delta { version, delta } => {
                let (Some(state), Some(last)) = (self.state.as_mut(), self.version) else {
                    return Followed::Gap;
                };
                if version != last + 1 {
                    return Followed::Gap;
                }
                delta.apply(state);
                self.version = Some(version);
                Followed::Changed
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    #[derive(Debug, Clone, PartialEq)]
    struct Row(&'static str, u32);
    impl Keyed for Row {
        fn key(&self) -> &str {
            self.0
        }
    }

    fn roundtrip(old: &[Row], new: &[Row]) -> Option<ListDelta<Row>> {
        let delta = diff_list(old, new);
        let mut rows = old.to_vec();
        if let Some(d) = &delta {
            apply_list(&mut rows, d);
        }
        assert_eq!(rows, new, "applying the delta gives the new list");
        delta
    }

    #[test]
    fn the_same_list_makes_no_delta() {
        let rows = [Row("a", 1), Row("b", 2)];
        assert_eq!(roundtrip(&rows, &rows), None);
    }

    #[test]
    fn only_changed_and_added_rows_are_sent() {
        let d = roundtrip(&[Row("a", 1), Row("b", 2), Row("c", 3)], &[Row("a", 1), Row("b", 9), Row("c", 3), Row("d", 4)])
            .unwrap();
        assert_eq!(d.changed, vec![Row("b", 9), Row("d", 4)]);
        assert!(d.removed.is_empty());
        assert_eq!(d.order, None);
        assert_eq!(d.replace, None);
    }

    #[test]
    fn removed_rows_are_sent_by_key() {
        let d = roundtrip(&[Row("a", 1), Row("b", 2), Row("c", 3)], &[Row("a", 1), Row("c", 3)]).unwrap();
        assert!(d.changed.is_empty());
        assert_eq!(d.removed, vec!["b".to_owned()]);
        assert_eq!(d.order, None);
    }

    #[test]
    fn a_new_order_is_sent_whole() {
        let d = roundtrip(&[Row("a", 1), Row("b", 2), Row("c", 3)], &[Row("c", 3), Row("a", 1), Row("b", 2)]).unwrap();
        assert!(d.changed.is_empty());
        assert_eq!(d.order, Some(vec!["c".into(), "a".into(), "b".into()]));
        // A new row first (newest-first dumps) is an order too.
        let d = roundtrip(&[Row("a", 1), Row("b", 2)], &[Row("z", 0), Row("a", 1), Row("b", 2)]).unwrap();
        assert_eq!(d.changed, vec![Row("z", 0)]);
        assert_eq!(d.order, Some(vec!["z".into(), "a".into(), "b".into()]));
    }

    #[test]
    fn repeated_keys_send_the_list_whole() {
        let d = roundtrip(&[Row("a", 1), Row("a", 2)], &[Row("a", 1), Row("a", 3)]).unwrap();
        assert_eq!(d.replace, Some(vec![Row("a", 1), Row("a", 3)]));
        // And going back to unique keys from repeated ones.
        let d = roundtrip(&[Row("a", 1), Row("a", 3)], &[Row("a", 1)]).unwrap();
        assert!(d.replace.is_some());
        assert_eq!(roundtrip(&[Row("a", 1), Row("a", 2)], &[Row("a", 1), Row("a", 2)]), None);
    }

    fn at(s: i64) -> chrono::DateTime<Utc> {
        Utc.timestamp_opt(1_794_000_000 + s, 0).unwrap()
    }

    fn item(id: &str, title: &str) -> WorkItem {
        serde_json::from_value(serde_json::json!({
            "id": id, "title": title, "state": "inbox", "priority": "normal",
            "order": 1.0, "createdAt": at(0), "updatedAt": at(0),
        }))
        .unwrap()
    }

    fn crew(chat: &str, role: &str) -> CrewMember {
        CrewMember {
            chat_id: chat.into(),
            orchestrator_id: "o1".into(),
            item_id: None,
            role: role.into(),
            last_report: None,
            released: false,
            joined_at: at(0),
            archived_seen_at: None,
            revived_at: None,
        }
    }

    fn state(items: Vec<WorkItem>, crew: Vec<CrewMember>) -> OrchestrationState {
        OrchestrationState { items, crew, ..Default::default() }
    }

    #[test]
    fn a_card_moved_sends_that_card_alone() {
        let old = state(vec![item("i1", "one"), item("i2", "two")], vec![crew("c1", "a")]);
        let mut new = old.clone();
        new.items[1].title = "two, renamed".into();
        let d = OrchestrationDelta::between(&old, &new).unwrap();
        assert_eq!(d.items.as_ref().unwrap().changed.len(), 1);
        assert!(d.crew.is_none() && d.asks.is_none() && d.dumps.is_none() && d.settings.is_none());
        // On the wire: only `items`, keyed by its card.
        let wire = serde_json::to_value(OrchestrationFrame::Delta { version: 2, delta: d.clone() }).unwrap();
        assert_eq!(wire["frame"], "delta");
        assert_eq!(wire["items"]["changed"][0]["id"], "i2");
        assert!(wire.get("crew").is_none() && wire.get("dumps").is_none());
        let mut applied = old.clone();
        d.apply(&mut applied);
        assert_eq!(applied, new);
        assert_eq!(OrchestrationDelta::between(&new, &new), None);
    }

    #[test]
    fn frames_follow_states_and_a_follower_keeps_up() {
        let s1 = state(vec![item("i1", "one")], vec![]);
        let s2 = state(vec![item("i1", "one"), item("i2", "two")], vec![crew("c1", "a")]);
        let mut s3 = s2.clone();
        s3.settings.crew_idle_hours = 12;
        let mut framer = Framer::new();
        let mut follower = Follower::default();
        let mut wire = Vec::new();
        for s in [&s1, &s1, &s2, &s3] {
            if let Some(f) = framer.frame(s) {
                wire.push(serde_json::to_value(f).unwrap());
            }
        }
        // Nothing for the state heard again.
        assert_eq!(wire.len(), 3);
        assert_eq!(wire[0]["frame"], "snapshot");
        assert_eq!(wire.iter().map(|w| w["version"].as_u64().unwrap()).collect::<Vec<_>>(), vec![1, 2, 3]);
        assert_eq!(wire[2]["settings"]["crewIdleHours"], 12);
        for w in &wire {
            assert_eq!(follower.hear(Heard::read(w).unwrap()), Followed::Changed);
        }
        assert_eq!(follower.state.as_ref(), Some(&s3));
    }

    #[test]
    fn a_missed_frame_is_a_gap_and_a_snapshot_sets_it_right() {
        let s1 = state(vec![item("i1", "one")], vec![]);
        let s2 = state(vec![item("i1", "uno")], vec![]);
        let s3 = state(vec![item("i1", "uno"), item("i2", "two")], vec![]);
        let mut framer = Framer::new();
        let f1 = framer.frame(&s1).unwrap();
        let _lost = framer.frame(&s2).unwrap();
        let f3 = framer.frame(&s3).unwrap();
        let mut follower = Follower::default();
        let read = |f: &OrchestrationFrame| Heard::read(&serde_json::to_value(f).unwrap()).unwrap();
        assert_eq!(follower.hear(read(&f1)), Followed::Changed);
        // Version 3 after 1: nothing applied.
        assert_eq!(follower.hear(read(&f3)), Followed::Gap);
        assert_eq!(follower.state.as_ref(), Some(&s1));
        // A delta before any snapshot is a gap too.
        assert_eq!(Follower::default().hear(read(&f3)), Followed::Gap);
        // Watching again: a new subscription's snapshot.
        let mut again = Framer::new();
        assert_eq!(follower.hear(read(&again.frame(&s3).unwrap())), Followed::Changed);
        assert_eq!(follower.state.as_ref(), Some(&s3));
        assert_eq!(follower.version, Some(1));
    }

    #[test]
    fn an_engine_without_frames_reads_as_snapshots() {
        let s = state(vec![item("i1", "one")], vec![crew("c1", "a")]);
        let bare = serde_json::to_value(&s).unwrap();
        let heard = Heard::read(&bare).unwrap();
        assert_eq!(heard, Heard::Snapshot { version: None, state: s.clone() });
        let mut follower = Follower::default();
        assert_eq!(follower.hear(heard.clone()), Followed::Changed);
        // Each bare state after it stands on its own (no versions to check).
        assert_eq!(follower.hear(heard), Followed::Changed);
        assert_eq!(follower.version, None);
    }

    #[test]
    fn a_snapshot_is_sent_again_when_asked() {
        let s = state(vec![item("i1", "one")], vec![]);
        let mut framer = Framer::new();
        framer.frame(&s);
        assert!(framer.frame(&s).is_none());
        assert!(matches!(framer.snapshot(&s), OrchestrationFrame::Snapshot { version: 2, .. }));
    }
}
