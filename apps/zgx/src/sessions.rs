//! The sessions behind the main window, for the scripts: the engine's
//! chats, projects (its spaces), devices, each chat's live state, and where
//! sessions are put by hand (pinned, your sections), as the shipping app's
//! sidebar reads them (crates/ui/src/state.rs: `WatchChats`, `WatchSpaces`,
//! `WatchDevices`, `WatchSessions`, `WatchSidebarPreferences`, `LocalDevice`)
//! and changes them (`Mutate`).
//!
//! What scripts see, `sessions`:
//!   { state = "loading" | "ready" | "offline",
//!     here = this Mac's device id?,
//!     devices = { { id, name, platform, here, online, seen? (while away), version? } },
//!     projects = { { id, name, device, path, git, github?, repository?, created?,
//!                    icon? = { emoji?, image?, hue? } } },
//!     chats = { { id, title, project?, device, branch?, cwd?, status?,
//!                 updated, created, harness?, agent?, model?,
//!                 running_subagents, side_chats, working_side_chats } },
//!     children = { [parent id] = { { id, parent, title, device, status?,
//!                 updated, created, harness?, agent?, child_kind? } } } (its
//!                 side chats, newest first; `child_kind` "subagent" or
//!                 "side" for one a Zeron chat started, none for the person's),
//!     places = { pinned = { id }, sections = { { id, name, sessions = { id },
//!                collapsed } } },
//!     archived = { chat, … } (as `chats`, the most recently moved first),
//!     places_ready,
//!     connection = "unsaved" | "offline" | "reconnecting" | nil }
//! Every list item carries an `id`, so a page keys its rows by it and they
//! update in place. `status` is the sidebar's state ("working", "input",
//! "failed", "done": finished but not yet seen), nil when idle; `updated`
//! and `created` are Unix seconds; `agent` is ds.Mark's name for the
//! chat's harness. Chats are the top-level ones that aren't archived.
//!
//! and call, each rejecting with an outcome word (offline, failed):
//!   sessions.create({ project?, device?, config?, branch?, cwd?, title?, parent?, id?, no_project? })
//!                                  the new chat's id (`project` a space id;
//!                                  without one, `device` hosts it; `parent`:
//!                                  a side chat of that chat, where it is;
//!                                  `cloud`: on a Cloud machine of its own,
//!                                  `{ id, cwd }`, or why not in words)
//!   sessions.fork({ source, parent?, id? })  a side chat starting as a copy of
//!                                  `source` (`ForkSideChat`): its id, or the
//!                                  engine's words for why not
//!   sessions.archive(id), sessions.unarchive(id)
//!   sessions.rename({ id, title })
//!   sessions.seen(id) (an action)  its finished state seen, on every device
//!   sessions.places({ pinned, sections }) (an action)
//!                                  where sessions should be put by hand, as
//!                                  the sidebar now has them: the changes
//!                                  from what the engine has go to it one at
//!                                  a time, in order (a later drop never
//!                                  overtakes an earlier one)
//!   sessions.rename_project({ id, name })
//!   sessions.set_project_icon({ id, emoji?, image?, hue? })
//!                                  its picture, whole, on every device (an
//!                                  emoji, or an uploaded image's https URL:
//!                                  projects.upload_icon; none: its default)
//!   sessions.rename_device({ id, name })  how a device is called, everywhere
//!   sessions.remove_device(id)     another of your devices off the account,
//!                                  everywhere (its chats and projects stay);
//!                                  refused "here", "online", "cloud" or
//!                                  "outdated"
//!   sessions.delete(id)            gone for good (its row; the engine keeps
//!                                  the conversation's document)
//!   sessions.collapse({ id, collapsed }) (an action)
//!                                  a section of yours folded, on every device

use std::{
    cell::{Cell, RefCell},
    collections::{BTreeSet, HashMap, HashSet, VecDeque},
    rc::Rc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde_json::{Value as Json, json};
use zeron_proto::{ChatIndicator, SeenMarks, Session, SleepState};
use zgx_desktop::WindowContext;
use zgx_script::{ScriptHost, Shared, Value};

use crate::auth::text;
use crate::engine::{self, lists, rpc::Watch};

/// A live state older than this belongs to an engine that has gone (the
/// shipping app's `SESSION_STALE_MS`).
const STALE_SECS: i64 = 45;
/// A device whose presence beat is older than this is offline (the shipping
/// app's `device_online`).
pub(crate) const ONLINE_SECS: i64 = 70;
/// The id prefix of a voice call's hidden orchestrator chat.
const ORCHESTRATOR: &str = "voice-orchestrator-";

type Outcome = &'static str;

#[derive(Clone, Default, PartialEq, Debug)]
struct Section {
    id: String,
    name: String,
    sessions: Vec<String>,
    collapsed: bool,
}

#[derive(Clone, Default, PartialEq, Debug)]
struct Places {
    pinned: Vec<String>,
    sections: Vec<Section>,
}

#[derive(Default)]
struct Sessions {
    /// Each open window's `sessions`, by a number of its own: let go of as
    /// its window closes, so a closed window isn't written to.
    shared: RefCell<Vec<(u64, Shared)>>,
    next_window: Cell<u64>,
    /// The engine watched (once, for every window).
    watching: Cell<bool>,
    heard: Cell<bool>,
    here: RefCell<Option<String>>,
    chats: RefCell<Json>,
    spaces: RefCell<Json>,
    devices: RefCell<Json>,
    live: RefCell<Json>,
    /// The engine's connection, as `WatchConnectivity` last said.
    connectivity: RefCell<Json>,
    /// Where the engine has sessions put, as its last frame said.
    confirmed: RefCell<Places>,
    places_ready: Cell<bool>,
    /// Where they are wanted: what the scripts last asked for while changes
    /// were on their way, else the engine's.
    expected: RefCell<Places>,
    /// Pin and section changes waiting to go, in order, and whether one is
    /// on its way.
    queue: RefCell<VecDeque<Json>>,
    sending: Cell<bool>,
    watches: RefCell<Vec<Watch>>,
    /// The engine's chat, device and live session lists, heard as changes.
    listening: RefCell<Vec<lists::Listening>>,
    /// Where each chat's entry, and each live state, is in its list.
    chats_at: RefCell<lists::Index>,
    live_at: RefCell<lists::Index>,
    devices_at: RefCell<lists::Index>,
    /// The live states read (parsed) once a change, and when each turns.
    live_rows: RefCell<LiveRows>,
    /// When each sleeping chat wakes on its own (`next_wake`), kept as its
    /// entry changes.
    wakes: RefCell<Times>,
    /// The chat lists as last given (None: to be built whole), and the
    /// chats whose rows may have moved since.
    layout: RefCell<Option<Layout>>,
    dirty_rows: RefCell<HashSet<String>>,
    /// When states are looked at again: as the soonest one showing goes
    /// stale, so it stops showing. None while nothing would change.
    due: Cell<Option<i64>>,
    /// Each chat's row as last built, by id: the engine sends the whole list
    /// with any chat's change, and only that chat's row is built again.
    rows: RefCell<HashMap<String, Row>>,
    /// What the scripts were last given of the live states and the devices,
    /// as `value` judged them: the engine sends every row again with each
    /// heartbeat (a row's `updatedAt`, a device's `lastSeenAt`), and a frame
    /// that leaves both as they were changes nothing shown, so isn't published.
    shown_live: RefCell<HashMap<String, Live>>,
    /// When `shown_live` was judged (Unix seconds).
    shown_at: Cell<Option<i64>>,
    shown_devices: RefCell<Option<Value>>,
    /// Publishes so far (for tests).
    #[cfg(test)]
    published: Cell<u32>,
    /// What was last published (for tests).
    #[cfg(test)]
    last_published: RefCell<Option<Value>>,
}

/// A chat's row for scripts, and what it was built from besides the chat's
/// own entry (a change to that drops it: `forget_changed`).
struct Row {
    /// When it last moved (Settings → Archived's order), its time for its
    /// parent's list, and whether it said anything since it was last seen.
    moved: i64,
    updated: f64,
    marks: SeenMarks,
    with: RowWith,
    value: Value,
}

#[derive(PartialEq)]
struct RowWith {
    status: Option<&'static str>,
    /// What its agent waits on, while that shows.
    waiting: Option<Vec<zeron_proto::WaitingOn>>,
    /// Its sleep: asleep until (`true`), or woke at (`false`), in Unix seconds.
    sleep: Option<(bool, i64)>,
    running_subagents: u32,
    side_chats: usize,
    working_side_chats: u32,
    known_project: bool,
    agent_icon: Option<String>,
}

/// `slot` holds `item` now; whether that's new.
fn replace_changed(slot: &RefCell<Json>, item: Json) -> bool {
    let mut slot = slot.borrow_mut();
    if *slot == item {
        return false;
    }
    *slot = item;
    true
}

/// Drops the rows of chats that aren't in `new`, or aren't the same there.
fn forget_changed(rows: &mut HashMap<String, Row>, old: &Json, new: &Json) {
    let by_id = |list: &'_ Json| -> HashMap<String, usize> {
        let mut at = HashMap::with_capacity(list.as_array().map_or(0, Vec::len));
        for (i, chat) in list.as_array().into_iter().flatten().enumerate() {
            if let Some(id) = chat["id"].as_str() {
                at.insert(id.to_owned(), i);
            }
        }
        at
    };
    let (was, is) = (by_id(old), by_id(new));
    rows.retain(|id, _| match (was.get(id), is.get(id)) {
        (Some(&a), Some(&b)) => old[a] == new[b],
        _ => false,
    });
}

/// When `chat` last moved, for Settings → Archived's order.
fn moved(chat: &Json) -> i64 {
    chat["lastMessageAt"].as_str().or(chat["createdAt"].as_str()).and_then(unix_seconds).unwrap_or(0)
}

/// Times by key (a chat's), the soonest after a moment found without
/// looking at them all.
#[derive(Default)]
struct Times {
    by: HashMap<String, Vec<i64>>,
    sorted: BTreeSet<(i64, String)>,
}

impl Times {
    /// `key`'s times are now `times` (none: it has none).
    fn put(&mut self, key: &str, times: Vec<i64>) {
        if let Some(old) = self.by.remove(key) {
            for t in old {
                self.sorted.remove(&(t, key.to_owned()));
            }
        }
        if times.is_empty() {
            return;
        }
        for &t in &times {
            self.sorted.insert((t, key.to_owned()));
        }
        self.by.insert(key.to_owned(), times);
    }

    fn clear(&mut self) {
        self.by.clear();
        self.sorted.clear();
    }

    /// The soonest time after `now`.
    fn next_after(&self, now: i64) -> Option<i64> {
        self.sorted.range((now.saturating_add(1), String::new())..).next().map(|(t, _)| *t)
    }

    /// The keys with a time after `from`, up to `to`.
    fn between(&self, from: i64, to: i64) -> Vec<String> {
        if to <= from {
            return Vec::new();
        }
        self.sorted
            .range((from.saturating_add(1), String::new())..(to.saturating_add(1), String::new()))
            .map(|(_, k)| k.clone())
            .collect()
    }
}

/// The live session list as `live_sessions`, `live_states` and `next_stale`
/// read it, kept a chat at a time: a heartbeat parses its own row, and the
/// rest are left as they were. A chat's state, judged at a moment, changes
/// only as its rows change or one of them goes stale (`turns`).
#[derive(Default)]
struct LiveRows {
    /// Each chat's rows, parsed, in the list's order (one, but for an
    /// engine that doesn't key them).
    by_chat: HashMap<String, Vec<Session>>,
    /// The moments a chat's rows go stale (`zeron_proto::view`'s rule, at
    /// whole seconds): its state can change then, and only then.
    turns: Times,
    /// When each row that shows goes stale, as `next_stale` reads it.
    stale: Times,
    /// Chats whose rows changed since `shown_live` was judged.
    changed: HashSet<String>,
    /// Whether the whole list did (a whole frame, or no index to trust).
    all_changed: bool,
    /// The latest moment a chat was found the same and let go of from
    /// `changed`: a look at an earlier one (the clock put back) can't
    /// trust `changed`, and judges all.
    let_go_at: Option<i64>,
}

impl LiveRows {
    /// Made again from the whole list.
    fn rebuild(&mut self, list: &Json) {
        let mut rows: HashMap<&str, Vec<&Json>> = HashMap::new();
        for row in list.as_array().into_iter().flatten() {
            rows.entry(row["chatId"].as_str().unwrap_or_default()).or_default().push(row);
        }
        self.by_chat.clear();
        self.turns.clear();
        self.stale.clear();
        for (chat, rows) in rows {
            self.put(chat, &rows);
        }
        self.changed.clear();
        self.all_changed = true;
    }

    /// `chat`'s rows are now `rows` (none: it has none).
    fn put(&mut self, chat: &str, rows: &[&Json]) {
        use serde::Deserialize;
        let sessions: Vec<Session> = rows
            .iter()
            .filter_map(|row| {
                #[cfg(test)]
                lists::work::add(1);
                Session::deserialize(*row).ok()
            })
            // (Its row's own chat, as `live_sessions` keys them.)
            .filter(|s| s.chat_id == chat)
            .collect();
        self.turns.put(chat, sessions.iter().map(|s| turn(s.updated_at)).collect());
        self.stale.put(chat, rows.iter().filter_map(|row| stale_at(row)).collect());
        if sessions.is_empty() {
            self.by_chat.remove(chat);
        } else {
            self.by_chat.insert(chat.to_owned(), sessions);
        }
        self.changed.insert(chat.to_owned());
    }

    /// The row of `chat`'s that says most at `now`, as `live_sessions` picks.
    fn winner(&self, chat: &str, now: chrono::DateTime<chrono::Utc>) -> Option<&Session> {
        let mut held: Option<&Session> = None;
        for session in self.by_chat.get(chat)? {
            match held {
                Some(h) if (says(h, now), h.updated_at) >= (says(session, now), session.updated_at) => {}
                _ => held = Some(session),
            }
        }
        held
    }

    /// `chat`'s `Live` at `now`, as `live_states` judges it.
    fn judge(&self, chat: &str, now: chrono::DateTime<chrono::Utc>) -> Option<Live> {
        self.winner(chat, now).map(|s| live_of(s, now))
    }

    /// Every chat's `Live` at `now`.
    fn states(&self, now: chrono::DateTime<chrono::Utc>) -> HashMap<String, Live> {
        self.by_chat.keys().filter_map(|chat| Some((chat.clone(), self.judge(chat, now)?))).collect()
    }

    /// The chats whose state may differ from the one judged at `since`.
    fn moved_since(&self, since: i64, now: i64) -> Vec<String> {
        let mut chats = self.turns.between(since, now);
        chats.extend(self.changed.iter().cloned());
        chats
    }
}

/// The first whole second at which a row stamped `updated` is stale
/// (`zeron_proto::view`'s `SESSION_STALE_MS`, judged as `value` does, at a
/// whole second).
fn turn(updated: chrono::DateTime<chrono::Utc>) -> i64 {
    let stale = |at: i64| {
        chrono::DateTime::from_timestamp(at, 0).unwrap_or_default().signed_duration_since(updated).num_milliseconds()
            > zeron_proto::view::SESSION_STALE_MS
    };
    // (Not stale a second before the limit; stale within a few after.)
    let mut at = updated.timestamp() + zeron_proto::view::SESSION_STALE_MS / 1000 - 1;
    for _ in 0..4 {
        if stale(at) {
            break;
        }
        at += 1;
    }
    at
}

/// When a live row that shows goes stale, as `next_stale` reads it.
fn stale_at(row: &Json) -> Option<i64> {
    let shows = row["status"].as_str().is_some_and(|s| s != "idle") || !row["waiting"].is_null();
    shows.then(|| row["updatedAt"].as_str().and_then(unix_seconds)).flatten().map(|updated| updated + STALE_SECS + 1)
}

/// When a chat wakes on its own, as `next_wake` reads it.
fn wake_at(chat: &Json) -> Option<i64> {
    if !chat["sleep"]["wokeAt"].is_null() {
        return None;
    }
    chat["sleep"]["until"].as_str().and_then(unix_seconds).map(|until| until + 1)
}

/// Where a chat shows in the scripts' lists.
#[derive(PartialEq, Debug)]
enum Place {
    /// Among `chats`.
    Top,
    /// Under its parent, in `children`.
    Child(String),
    /// Among `archived`.
    Gone,
    /// Nowhere (an archived side chat, a voice call's orchestrator).
    Hidden,
}

/// Where `chat` shows.
fn place_of(chat: &Json) -> Place {
    let id = chat["id"].as_str().unwrap_or_default();
    let hidden = id.starts_with(ORCHESTRATOR) || !chat["parentChatId"].is_null();
    if chat["archived"].as_bool().unwrap_or(false) {
        return if hidden { Place::Hidden } else { Place::Gone };
    }
    match chat["parentChatId"].as_str() {
        Some(parent) => Place::Child(parent.to_owned()),
        None if hidden => Place::Hidden,
        None => Place::Top,
    }
}

/// What deleting `root` takes with it, as the engine's `deleteChat` does:
/// its subagents and side chats on computers, and theirs in turn, archived
/// or not; a Cloud chat's children on Cloud too. A computer's chat keeps its
/// Cloud delegates. `(subagents, side chats)`.
fn deleted_with(chats: &[Json], root: &str) -> (u32, u32) {
    let on_cloud = |c: &Json| c["deviceId"].as_str().unwrap_or_default().starts_with("cloud-");
    let root_cloud = chats.iter().find(|c| c["id"].as_str() == Some(root)).is_some_and(on_cloud);
    let (mut subagents, mut sides) = (0, 0);
    let mut seen: HashSet<&str> = HashSet::from([root]);
    let mut frontier: Vec<(&str, bool)> = vec![(root, root_cloud)];
    while let Some((parent, parent_cloud)) = frontier.pop() {
        for c in chats {
            if c["parentChatId"].as_str() != Some(parent) {
                continue;
            }
            let Some(id) = c["id"].as_str() else { continue };
            let cloud = on_cloud(c);
            if (cloud && !parent_cloud) || !seen.insert(id) {
                continue;
            }
            if c["childKind"].as_str() == Some("subagent") {
                subagents += 1;
            } else {
                sides += 1;
            }
            frontier.push((id, cloud));
        }
    }
    (subagents, sides)
}

/// A parent's side chats: their ids in the list's order, their rows newest
/// first, and how many are under way.
struct Kids {
    ids: Vec<String>,
    value: Value,
    working: u32,
}

impl Kids {
    /// Its rows as now built, newest first, and how many work.
    fn sort(&mut self, rows: &HashMap<String, Row>) {
        let mut list: Vec<(f64, Value)> = Vec::with_capacity(self.ids.len());
        self.working = 0;
        for id in &self.ids {
            let Some(row) = rows.get(id) else { continue };
            // (Under way: running, or waiting on work that will wake it.)
            if matches!(row.with.status, Some("working" | "waiting")) {
                self.working += 1;
            }
            list.push((row.updated, row.value.clone()));
        }
        // (Newest first, as the lanes card orders them.)
        list.sort_by(|a, b| b.0.total_cmp(&a.0));
        self.value = Value::List(list.into_iter().map(|(_, row)| row).collect());
    }
}

/// The chat lists as the scripts were last given them, and where each chat
/// is in them: kept so that a change builds the rows it touches, not all.
struct Layout {
    /// When they were built (Unix seconds), the projects known then, and
    /// the registry's icons then.
    built_at: i64,
    spaces: HashSet<String>,
    icons: u64,
    place: HashMap<String, Place>,
    /// The top-level chats' rows in the list's order, and where each is.
    top: Vec<Value>,
    top_at: HashMap<String, usize>,
    /// Archived ones', the most recently moved first.
    gone: Vec<Value>,
    gone_at: HashMap<String, usize>,
    /// Side chats by parent, the parents as the list first names them.
    parents: Vec<String>,
    parent_at: HashMap<String, usize>,
    kids: Vec<Kids>,
    /// Chats with a sleep: looked at again each time (a sleep turns with
    /// time).
    sleepers: HashSet<String>,
    /// The lists as given.
    chats: Value,
    children: Value,
    archived: Value,
}

impl Layout {
    fn build<'a>(
        all: &[Json],
        rows: &mut HashMap<String, Row>,
        live: &dyn Fn(&str) -> Option<&'a Session>,
        at: chrono::DateTime<chrono::Utc>,
        known: &HashSet<&str>,
        now: i64,
        icons: u64,
    ) -> Layout {
        #[cfg(test)]
        BUILT_WHOLE.with(|n| n.set(n.get() + 1));
        let mut layout = Layout {
            built_at: now,
            spaces: known.iter().map(|s| s.to_string()).collect(),
            icons,
            place: HashMap::with_capacity(all.len()),
            top: Vec::new(),
            top_at: HashMap::new(),
            gone: Vec::new(),
            gone_at: HashMap::new(),
            parents: Vec::new(),
            parent_at: HashMap::new(),
            kids: Vec::new(),
            sleepers: HashSet::new(),
            chats: Value::Nil,
            children: Value::Nil,
            archived: Value::Nil,
        };
        // Side chats, under their parents (as the shipping app's lanes list
        // them): each as a row of its own, and how many there are and how
        // many work now, on the parent's row.
        for c in all {
            let Some(id) = c["id"].as_str() else { continue };
            if !c["sleep"].is_null() {
                layout.sleepers.insert(id.to_owned());
            }
            let place = place_of(c);
            if let Place::Child(parent) = &place {
                child_row(rows, c, id, parent, live(id), at);
                let i = *layout.parent_at.entry(parent.clone()).or_insert_with(|| {
                    layout.parents.push(parent.clone());
                    layout.kids.push(Kids { ids: Vec::new(), value: Value::Nil, working: 0 });
                    layout.kids.len() - 1
                });
                layout.kids[i].ids.push(id.to_owned());
            }
            layout.place.insert(id.to_owned(), place);
        }
        for kids in &mut layout.kids {
            kids.sort(rows);
        }
        layout.children = layout.children_value();
        let counts = |layout: &Layout, id: &str| layout.parent_at.get(id).map_or((0, 0), |&i| (layout.kids[i].ids.len(), layout.kids[i].working));
        // Archived, the most recently moved first (Settings → Archived), by
        // when each moved as its row was last built.
        // (Each entry by its own place: a list naming a chat twice, which
        // only a whole frame can, shows each as the old build did. The
        // places by id serve the kept build, which such a list never has.)
        let mut gone: Vec<(i64, &Json)> = Vec::new();
        for c in all {
            let Some(id) = c["id"].as_str() else { continue };
            match place_of(c) {
                Place::Top => {
                    let (side_chats, working) = counts(&layout, id);
                    let value = top_row(rows, c, id, known, live(id), at, side_chats, working);
                    layout.top_at.insert(id.to_owned(), layout.top.len());
                    layout.top.push(value);
                }
                Place::Gone => gone.push((rows.get(id).map_or_else(|| moved(c), |row| row.moved), c)),
                _ => {}
            }
        }
        gone.sort_by_key(|(moved, _)| std::cmp::Reverse(*moved));
        for (_, c) in gone {
            let id = c["id"].as_str().unwrap_or_default();
            let (side_chats, working) = counts(&layout, id);
            let value = top_row(rows, c, id, known, live(id), at, side_chats, working);
            layout.gone_at.insert(id.to_owned(), layout.gone.len());
            layout.gone.push(value);
        }
        layout.chats = Value::List(layout.top.clone().into());
        layout.archived = Value::List(layout.gone.clone().into());
        layout
    }

    fn children_value(&self) -> Value {
        Value::record(self.parents.iter().zip(&self.kids).map(|(parent, kids)| (parent.as_str(), kids.value.clone())))
    }

    fn values(&self) -> (Value, Value, Value) {
        (self.chats.clone(), self.children.clone(), self.archived.clone())
    }
}

/// A side chat's row, built again if what it shows moved.
fn child_row(rows: &mut HashMap<String, Row>, c: &Json, id: &str, parent: &str, session: Option<&Session>, at: chrono::DateTime<chrono::Utc>) {
    let status = status_of(rows.get(id).map_or_else(|| seen_marks(c), |row| row.marks), session, at);
    let harness = c["config"]["harness"].as_str();
    let with = RowWith {
        status,
        waiting: None,
        sleep: None,
        running_subagents: 0,
        side_chats: 0,
        working_side_chats: 0,
        known_project: false,
        agent_icon: harness.and_then(crate::agents::registry_icon),
    };
    if rows.get(id).is_some_and(|row| row.with == with) {
        return;
    }
    let created = c["createdAt"].as_str().and_then(unix_seconds).unwrap_or(0);
    let updated = c["lastMessageAt"].as_str().and_then(unix_seconds).unwrap_or(created);
    let opt = |v: Option<&str>| v.filter(|s| !s.is_empty()).map_or(Value::Nil, Value::from);
    // Its title, else what it last said, else the shipping app's word.
    let title = c["title"]
        .as_str()
        .filter(|t| !t.trim().is_empty())
        .or(c["lastMessagePreview"].as_str().filter(|t| !t.trim().is_empty()))
        .unwrap_or("New side chat");
    let value = Value::record([
        ("id", Value::from(id)),
        ("parent", Value::from(parent)),
        ("title", Value::from(title)),
        ("device", Value::from(c["deviceId"].as_str().unwrap_or_default())),
        ("status", status.map_or(Value::Nil, Value::from)),
        ("updated", Value::from(updated as f64)),
        ("created", Value::from(created as f64)),
        ("harness", opt(harness)),
        ("agent", harness.and_then(crate::agents::glyph).map_or(Value::Nil, |g| Value::from(g.as_str()))),
        // An agent from the ACP registry: its icon, as listed.
        ("agent_icon", with.agent_icon.as_deref().map_or(Value::Nil, Value::from)),
        // Started by a Zeron chat through Zeron's tools ("subagent" or
        // "side"); none for a side chat the person started.
        ("child_kind", opt(c["childKind"].as_str())),
    ]);
    rows.insert(id.to_owned(), Row { moved: moved(c), updated: updated as f64, marks: seen_marks(c), with, value });
}

/// A top-level chat as scripts see it (side chats and a voice call's
/// orchestrator are another's), built again if what it shows moved.
#[allow(clippy::too_many_arguments)]
fn top_row(
    rows: &mut HashMap<String, Row>,
    c: &Json,
    id: &str,
    known: &HashSet<&str>,
    session: Option<&Session>,
    at: chrono::DateTime<chrono::Utc>,
    side_chats: usize,
    working_side_chats: u32,
) -> Value {
    let harness = c["config"]["harness"].as_str();
    // A project that isn't known (deleted, or not yet synced) leaves the
    // chat among those with none.
    let project = c["spaceId"].as_str().filter(|s| known.contains(s));
    let marks = rows.get(id).map_or_else(|| seen_marks(c), |row| row.marks);
    let status = status_of(marks, session, at);
    let with = RowWith {
        status,
        waiting: (status == Some("waiting")).then(|| zeron_proto::view::waiting(session, at).map(|w| w.on.clone())).flatten(),
        sleep: sleep_of(c, marks, at),
        running_subagents: zeron_proto::view::running_subagents(session, at),
        side_chats,
        working_side_chats,
        known_project: project.is_some(),
        agent_icon: harness.and_then(crate::agents::registry_icon),
    };
    if let Some(row) = rows.get(id).filter(|row| row.with == with) {
        return row.value.clone();
    }
    let created = c["createdAt"].as_str().and_then(unix_seconds).unwrap_or(0);
    let updated = c["lastMessageAt"].as_str().and_then(unix_seconds).unwrap_or(created);
    let opt = |v: Option<&str>| v.filter(|s| !s.is_empty()).map_or(Value::Nil, Value::from);
    let value = Value::record([
        ("id", Value::from(id)),
        ("title", Value::from(chat_title(c["title"].as_str()))),
        ("project", opt(project)),
        ("device", Value::from(c["deviceId"].as_str().unwrap_or_default())),
        ("branch", opt(c["branch"].as_str())),
        ("cwd", opt(c["cwd"].as_str())),
        // The branch its latest run started on, as the engine stamped it
        // (`sourceContext`): what its pull request is looked up by, as the
        // shipping app's sidebar does.
        ("source_branch", opt(c["sourceContext"]["branch"].as_str())),
        ("status", with.status.map_or(Value::Nil, Value::from)),
        ("updated", Value::from(updated as f64)),
        ("created", Value::from(created as f64)),
        ("harness", opt(harness)),
        ("agent", harness.and_then(crate::agents::glyph).map_or(Value::Nil, |g| Value::from(g.as_str()))),
        // An agent from the ACP registry: its icon, as listed.
        ("agent_icon", with.agent_icon.as_deref().map_or(Value::Nil, Value::from)),
        ("model", opt(c["config"]["model"].as_str())),
        // (Its harness's own id for it: the menu's Copy.)
        ("harness_session", opt(c["harnessSessionId"].as_str())),
        // Its subagents streaming now (the sidebar's pill), and its side
        // chats: how many, how many working.
        ("running_subagents", Value::from(with.running_subagents as f64)),
        ("side_chats", Value::from(with.side_chats as f64)),
        ("working_side_chats", Value::from(with.working_side_chats as f64)),
        // What its agent waits on after its turn (status "waiting"):
        // `{ id, kind = "agent" | "process", label }`.
        ("waiting", with.waiting.as_deref().map_or(Value::Nil, waiting_value)),
        // Put to sleep: until when; or, woken and not looked at since, when
        // it woke (Unix seconds).
        ("sleep_until", with.sleep.filter(|s| s.0).map_or(Value::Nil, |s| Value::from(s.1 as f64))),
        ("woke", with.sleep.filter(|s| !s.0).map_or(Value::Nil, |s| Value::from(s.1 as f64))),
    ]);
    rows.insert(id.to_owned(), Row { moved: moved(c), updated: updated as f64, marks, with, value: value.clone() });
    value
}

thread_local! {
    static SESSIONS: Rc<Sessions> = Rc::new(Sessions::default());
}

#[cfg(test)]
pub(crate) fn delete_action_for_tests(host: &ScriptHost) {
    delete_action(host, SESSIONS.with(Rc::clone));
}

/// `sessions.delete`: a chat gone for good, so never an unkept preview's.
fn delete_action(host: &ScriptHost, a: Rc<Sessions>) {
    let s = a.clone();
    host.async_action("sessions.delete", move |id| {
        let a = a.clone();
        let params = json!({ "op": "deleteChat", "chatId": text(&id) });
        async move {
            crate::plugins::preview::refuse("sessions.delete")?;
            a.mutate(params).await.map_err(String::from)
        }
    });
    // What a delete takes with it, for "Delete session?" to say: read from
    // every chat known, archived ones too (the lists show none of those).
    host.async_action("sessions.deleted_with", move |id| {
        let (subagents, sides) = {
            let chats = s.chats.borrow();
            deleted_with(chats.as_array().map(Vec::as_slice).unwrap_or_default(), &text(&id))
        };
        async move {
            Ok::<_, String>(Value::record([
                ("subagents", Value::from(subagents as f64)),
                ("sides", Value::from(sides as f64)),
            ]))
        }
    });
}

pub fn install(cx: &mut WindowContext, host: &ScriptHost) {
    let s = SESSIONS.with(Rc::clone);
    let window = s.next_window.get();
    s.next_window.set(window + 1);
    s.shared.borrow_mut().push((window, host.shared("sessions", s.value())));
    let me = Rc::downgrade(&s);
    cx.on_closed(move || {
        if let Some(s) = me.upgrade() {
            s.shared.borrow_mut().retain(|(w, _)| *w != window);
        }
    });
    if !s.watching.replace(true) {
        let weak = Rc::downgrade(&s);
        engine::get().on_connect(move |rpc| {
            let Some(s) = weak.upgrade() else { return };
            let mut listening = Vec::new();
            for (method, slot) in [("WatchChats", Slot::Chats), ("WatchDevices", Slot::Devices), ("WatchSessions", Slot::Live)] {
                let heard = Rc::downgrade(&s);
                listening.push(lists::listen(rpc, method, move |frame| {
                    if let Some(s) = heard.upgrade() {
                        s.heard_list(slot, frame);
                    }
                }));
            }
            *s.listening.borrow_mut() = listening;
            let mut watches = Vec::new();
            for (method, slot) in [
                ("WatchSpaces", Slot::Spaces),
                ("WatchSidebarPreferences", Slot::Places),
                ("WatchConnectivity", Slot::Connectivity),
            ] {
                let heard = Rc::downgrade(&s);
                watches.push(rpc.watch(method, json!({}), move |item| {
                    if let Some(s) = heard.upgrade() {
                        s.heard(slot, item);
                    }
                }));
            }
            *s.watches.borrow_mut() = watches;
            let (me, rpc) = (s.clone(), rpc.clone());
            engine::get().spawn(async move {
                if let Some(here) = engine::local_device(&rpc).await {
                    *me.here.borrow_mut() = Some(here);
                    me.publish();
                }
            });
        });
    }

    let a = s.clone();
    host.async_action("sessions.create", move |args| {
        let a = a.clone();
        async move {
            if args.field("cloud").is_some_and(Value::truthy) {
                a.create_on_cloud(args).await
            } else {
                a.create(args).await.map_err(String::from)
            }
        }
    });
    let a = s.clone();
    host.async_action("sessions.fork", move |args| {
        let a = a.clone();
        async move { a.fork(args).await }
    });
    for (name, archived) in [("sessions.archive", true), ("sessions.unarchive", false)] {
        let a = s.clone();
        host.async_action(name, move |id| {
            let a = a.clone();
            let params = json!({ "op": "setChatArchived", "chatId": text(&id), "archived": archived });
            async move { a.mutate(params).await.map_err(String::from) }
        });
    }
    let a = s.clone();
    host.async_action("sessions.rename", move |args| {
        let a = a.clone();
        let id = args.field("id").map(text).unwrap_or_default();
        let title = args.field("title").map(text).unwrap_or_default();
        let params = json!({ "op": "renameChat", "chatId": id, "title": title.trim() });
        async move { a.mutate(params).await.map_err(String::from) }
    });
    let a = s.clone();
    host.async_action("sessions.rename_project", move |args| {
        let a = a.clone();
        let id = args.field("id").map(text).unwrap_or_default();
        let name = args.field("name").map(text).filter(|n| !n.trim().is_empty());
        let params = json!({ "op": "renameSpace", "spaceId": id, "name": name });
        async move { a.mutate(params).await.map_err(String::from) }
    });
    // A project's picture, whole (`SetSpaceIcon`): an emoji or an image's
    // URL, and a hue; none of them, its default tile again.
    let a = s.clone();
    host.async_action("sessions.set_project_icon", move |args| {
        let a = a.clone();
        let id = args.field("id").map(text).unwrap_or_default();
        let part = |key: &str| args.field(key).map(text).filter(|v| !v.trim().is_empty());
        let icon = json!({ "emoji": part("emoji"), "image": part("image"), "hue": part("hue") });
        let params = json!({ "op": "setSpaceIcon", "spaceId": id, "icon": icon });
        async move {
            match engine::get().call("Mutate", params).await {
                Ok(_) => Ok(Value::Nil),
                Err(error) => Err(icon_fail(&error).to_string()),
            }
        }
    });
    let a = s.clone();
    host.async_action("sessions.rename_device", move |args| {
        let a = a.clone();
        let id = args.field("id").map(text).unwrap_or_default();
        let name = args.field("name").map(text).unwrap_or_default();
        let params = json!({ "op": "renameDevice", "deviceId": id, "name": name.trim() });
        async move {
            if name.trim().is_empty() {
                return Err("invalid".to_string());
            }
            a.mutate(params).await.map_err(String::from)
        }
    });
    // Another of your devices removed from the account (`RemoveDevice`): its
    // chats and projects stay. Refused, in a word the page says plainly: this
    // device ("here"), one online now ("online"), one of Cloud's ("cloud"),
    // an engine from before it ("outdated").
    host.async_action("sessions.remove_device", move |id| {
        let id = text(&id);
        let params = json!({ "op": "removeDevice", "deviceId": id });
        async move {
            crate::plugins::preview::refuse("sessions.remove_device")?;
            if id.trim().is_empty() {
                return Err("invalid".to_string());
            }
            match engine::get().call("Mutate", params).await {
                Ok(_) => Ok(Value::Nil),
                Err(error) => Err(removal_refused(&error).to_string()),
            }
        }
    });
    // A session's Zeron conversation link (production's links.rs): its id
    // and its workspace's locator, the same hash, so either app opens it.
    host.async_action("sessions.link", move |id| {
        let id = text(&id);
        async move {
            current_locator()
                .await
                .map(|w| Value::from(conversation_link(&id, &w).as_str()))
                .ok_or_else(|| "not ready".to_string())
        }
    });
    // A chat put to sleep until `until` (Unix seconds), or woken (none):
    // synced, and its host wakes it early when it asks for you.
    host.async_action("sessions.sleep", move |args| {
        let id = args.field("id").map(text).unwrap_or_default();
        let until = args.field("until").and_then(|v| v.as_f64()).map(|secs| (secs * 1000.0) as i64);
        let params = json!({ "op": "setChatSleep", "chatId": id, "until": until });
        async move {
            if id.is_empty() {
                return Err("missing".to_string());
            }
            // An engine from before Sleep doesn't know the request at all:
            // "outdated", so the app can say what would let it.
            match engine::get().call("Mutate", params).await {
                Ok(_) => Ok(Value::Nil),
                Err(error) if error.contains("unknown variant") => Err("outdated".to_string()),
                Err(error) => Err(fail(&error).to_string()),
            }
        }
    });
    let a = s.clone();
    host.action("sessions.seen", move |id| {
        let a = a.clone();
        let params = json!({ "op": "markChatSeen", "chatId": text(&id) });
        engine::get().spawn(async move {
            let _ = a.mutate(params).await;
        });
    });
    let a = s.clone();
    delete_action(host, a);
    let a = s.clone();
    host.action("sessions.collapse", move |args| {
        let id = args.field("id").map(text).unwrap_or_default();
        let collapsed = args.field("collapsed").is_some_and(Value::truthy);
        a.collapse(id, collapsed);
    });
    let a = s;
    host.action("sessions.places", move |wanted| a.want(places_of_value(&wanted)));
}

/// The other devices online now, as the sidebar judges it (a Cloud one's no
/// place to look at ahead of time).
pub(crate) fn other_devices_online() -> Vec<String> {
    let now = now_seconds();
    SESSIONS.with(|s| {
        let here = s.here.borrow().clone();
        s.devices
            .borrow()
            .as_array()
            .into_iter()
            .flatten()
            .filter(|d| !is_cloud(d))
            .filter(|d| d["lastSeenAt"].as_str().and_then(unix_seconds).is_some_and(|at| now - at <= ONLINE_SECS))
            .filter_map(|d| d["id"].as_str().map(str::to_owned))
            .filter(|id| Some(id.as_str()) != here.as_deref())
            .collect()
    })
}

#[derive(Clone, Copy)]
enum Slot {
    Chats,
    Spaces,
    Devices,
    Live,
    Places,
    Connectivity,
}

impl Sessions {
    /// A frame of the chat, device or live session list: as `heard`, with
    /// only the rows it says changed looked at again (the engine sent the
    /// whole list for one row's change, and each was gone over in full).
    fn heard_list(&self, slot: Slot, frame: &lists::Frame) {
        let Some(touched) = frame.touched() else {
            return self.heard(slot, frame.list.clone());
        };
        match slot {
            Slot::Chats => {
                let mut rows = self.rows.borrow_mut();
                for id in &touched {
                    rows.remove(*id);
                }
                drop(rows);
                frame.apply_to_list(&mut self.chats.borrow_mut(), &mut self.chats_at.borrow_mut());
                // (Rows gone or put in another order: the lists are built whole.)
                if frame.changes.is_some_and(|c| c.reordered || !c.removed.is_empty()) {
                    self.layout.borrow_mut().take();
                }
                self.chats_moved(&touched);
            }
            Slot::Devices | Slot::Live => {
                if matches!(slot, Slot::Devices) {
                    frame.apply_to_list(&mut self.devices.borrow_mut(), &mut self.devices_at.borrow_mut());
                } else {
                    frame.apply_to_list(&mut self.live.borrow_mut(), &mut self.live_at.borrow_mut());
                    self.live_moved(&touched);
                }
                if self.shows_the_same() {
                    self.look_again();
                    return;
                }
            }
            Slot::Spaces | Slot::Places | Slot::Connectivity => return self.heard(slot, frame.list.clone()),
        }
        self.publish();
    }

    fn heard(&self, slot: Slot, item: Json) {
        match slot {
            Slot::Chats => {
                let first = !self.heard.replace(true);
                let mut chats = self.chats.borrow_mut();
                // (Nothing new: an idle window isn't woken for its sake.)
                if !first && *chats == item {
                    return;
                }
                forget_changed(&mut self.rows.borrow_mut(), &chats, &item);
                *chats = item;
                drop(chats);
                self.chats_replaced();
            }
            Slot::Spaces => {
                if !replace_changed(&self.spaces, item) {
                    return;
                }
            }
            Slot::Devices | Slot::Live => {
                let live = matches!(slot, Slot::Live);
                let slot = if live { &self.live } else { &self.devices };
                if !replace_changed(slot, item) {
                    return;
                }
                if live {
                    self.live_replaced();
                } else {
                    self.devices_at.borrow_mut().forget();
                }
                // A heartbeat: nothing shown moved. (When a state goes stale
                // or a device quiet is looked at again from what's now known.)
                if self.shows_the_same() {
                    self.look_again();
                    return;
                }
            }
            Slot::Connectivity => {
                if !replace_changed(&self.connectivity, item) {
                    return;
                }
            }
            Slot::Places => {
                let places = places_of_json(&item);
                self.places_ready
                    .set(item["synced"].as_bool().unwrap_or(false) || item["initialized"].as_bool().unwrap_or(false));
                // While our own changes are on their way, what we asked for
                // stands; once they're through, the engine's word is final.
                if self.queue.borrow().is_empty() && !self.sending.get() {
                    *self.expected.borrow_mut() = places.clone();
                }
                *self.confirmed.borrow_mut() = places;
            }
        }
        self.publish();
    }

    fn publish(&self) {
        #[cfg(test)]
        self.published.set(self.published.get() + 1);
        let value = self.value();
        #[cfg(test)]
        self.last_published.replace(Some(value.clone()));
        for (_, shared) in self.shared.borrow().iter() {
            shared.set(value.clone());
        }
        self.look_again();
    }

    /// Publishes again as the soonest live state showing goes stale, or a
    /// device shown online goes quiet, and only then: an idle window wakes
    /// for nothing (it once looked every 15 s, whatever there was).
    fn look_again(&self) {
        let now = now_seconds();
        let stale = self.live_rows.borrow().stale.next_after(now);
        let offline = next_offline(&self.devices.borrow(), self.here.borrow().as_deref(), now);
        let wake = self.wakes.borrow().next_after(now);
        let Some(at) = stale.into_iter().chain(offline).chain(wake).min() else { return };
        // (One sooner is already set; it looks again for this one then.)
        if self.due.get().is_some_and(|due| due <= at) {
            return;
        }
        self.due.set(Some(at));
        engine::get().spawn(async move {
            zgx::timer::sleep(Duration::from_secs((at - now).max(1) as u64)).await;
            SESSIONS.with(|s| {
                if s.due.get() == Some(at) {
                    s.due.set(None);
                    s.publish();
                }
            });
        });
    }

    /// Whether the live states and devices, judged now, are what the scripts
    /// were last given.
    fn shows_the_same(&self) -> bool {
        let now = now_seconds();
        self.live_shows_the_same(now) && self.shown_devices.borrow().as_ref() == Some(&self.devices_value(now))
    }

    /// Whether the live states, judged at `now`, are `shown_live`: only the
    /// chats whose rows changed, or went stale, since it was judged are
    /// judged again (a chat found the same drops out of those that changed).
    fn live_shows_the_same(&self, now: i64) -> bool {
        let at = chrono::DateTime::from_timestamp(now, 0).unwrap_or_default();
        let mut rows = self.live_rows.borrow_mut();
        let shown = self.shown_live.borrow();
        let since = self.shown_at.get().filter(|&since| since <= now && rows.let_go_at.is_none_or(|t| t <= now));
        let Some(since) = since.filter(|_| !rows.all_changed) else {
            return *shown == rows.states(at);
        };
        for chat in rows.moved_since(since, now) {
            if rows.judge(&chat, at).as_ref() != shown.get(&chat) {
                return false;
            }
            // (The same now, and it can only turn at a moment of its own
            // from here: no need to look at it again for this change.)
            if rows.changed.remove(&chat) {
                rows.let_go_at = Some(now);
            }
        }
        true
    }

    /// `shown_live` judged again at `now`: the chats that may have moved.
    fn show_live(&self, now: i64) {
        let at = chrono::DateTime::from_timestamp(now, 0).unwrap_or_default();
        let mut rows = self.live_rows.borrow_mut();
        let mut shown = self.shown_live.borrow_mut();
        let trusted = !rows.all_changed && rows.let_go_at.is_none_or(|t| t <= now);
        match self.shown_at.get().filter(|&since| since <= now && trusted) {
            None => *shown = rows.states(at),
            Some(since) => {
                for chat in rows.moved_since(since, now) {
                    match rows.judge(&chat, at) {
                        Some(live) => shown.insert(chat, live),
                        None => shown.remove(&chat),
                    };
                }
            }
        }
        rows.changed.clear();
        rows.all_changed = false;
        rows.let_go_at = None;
        self.shown_at.set(Some(now));
    }

    /// The live list was replaced whole.
    fn live_replaced(&self) {
        self.live_at.borrow_mut().forget();
        self.live_rows.borrow_mut().rebuild(&self.live.borrow());
        self.layout.borrow_mut().take();
    }

    /// The live rows of `touched` chats changed (or went).
    fn live_moved(&self, touched: &[&str]) {
        let live = self.live.borrow();
        let rows = live.as_array().map(Vec::as_slice).unwrap_or_default();
        let mut at = self.live_at.borrow_mut();
        let mut cache = self.live_rows.borrow_mut();
        for chat in touched {
            let row = at.find(rows, "chatId", chat).map(|i| &rows[i]);
            cache.put(chat, row.as_slice());
        }
        self.dirty_rows.borrow_mut().extend(touched.iter().map(|c| c.to_string()));
        // (A chat on two rows, from an engine that doesn't key them: the
        // index finds one, so all are read again.)
        if !at.covers(rows) {
            cache.rebuild(&live);
            self.layout.borrow_mut().take();
        }
    }

    /// The chat list was replaced whole.
    fn chats_replaced(&self) {
        self.chats_at.borrow_mut().forget();
        self.layout.borrow_mut().take();
        let mut wakes = self.wakes.borrow_mut();
        wakes.clear();
        for chat in self.chats.borrow().as_array().into_iter().flatten() {
            if let (id, Some(at)) = (chat["id"].as_str().unwrap_or_default(), wake_at(chat)) {
                wakes.by.entry(id.to_owned()).or_default().push(at);
                wakes.sorted.insert((at, id.to_owned()));
            }
        }
    }

    /// The entries of `touched` chats changed (or went).
    fn chats_moved(&self, touched: &[&str]) {
        let chats = self.chats.borrow();
        let rows = chats.as_array().map(Vec::as_slice).unwrap_or_default();
        let mut at = self.chats_at.borrow_mut();
        let mut wakes = self.wakes.borrow_mut();
        let mut layout = self.layout.borrow_mut();
        for id in touched {
            let entry = at.find(rows, "id", id).map(|i| &rows[i]);
            wakes.put(id, entry.and_then(wake_at).into_iter().collect());
            // Its row is built again; a chat new, gone, moved between the
            // lists or archived (its place by when it moved) has them built
            // whole.
            if let Some(l) = layout.as_mut() {
                let same = entry.is_some_and(|c| {
                    let place = place_of(c);
                    place != Place::Gone && l.place.get(*id) == Some(&place)
                });
                if !same {
                    *layout = None;
                } else if entry.is_some_and(|c| !c["sleep"].is_null()) {
                    l.sleepers.insert(id.to_string());
                } else {
                    l.sleepers.remove(*id);
                }
            }
        }
        self.dirty_rows.borrow_mut().extend(touched.iter().map(|c| c.to_string()));
        drop(layout);
        if !at.covers(rows) {
            drop((wakes, at, chats));
            self.chats_replaced();
        }
    }

    /// Lists set whole, as the engine's first frame does (for tests).
    #[cfg(test)]
    fn set_chats(&self, chats: Json) {
        *self.chats.borrow_mut() = chats;
        self.chats_replaced();
    }

    #[cfg(test)]
    fn set_live(&self, live: Json) {
        *self.live.borrow_mut() = live;
        self.live_replaced();
    }

    fn devices_value(&self, now: i64) -> Value {
        let here = self.here.borrow();
        let devices: Vec<Value> = self
            .devices
            .borrow()
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|d| {
                let id = d["id"].as_str()?;
                // Its presence heartbeat's last beat (unix seconds), and
                // whether that is fresh, as the shipping app judges it
                // (`device_online`: this device always; another seen in the
                // last 70 s; one never seen, not).
                let seen = d["lastSeenAt"].as_str().and_then(unix_seconds);
                let online = here.as_deref() == Some(id) || seen.is_some_and(|at| now - at <= ONLINE_SECS);
                let mut fields = vec![
                    ("id", Value::from(id)),
                    ("name", Value::from(d["name"].as_str().unwrap_or("Device"))),
                    ("platform", Value::from(d["platform"].as_str().unwrap_or(""))),
                    // A Cloud device (the account's, or a session's machine), as
                    // the shipping app tells one (`is_cloud_device`, the
                    // `cloud-session`/`cloud-account` capabilities): never a
                    // place to pick in a device list.
                    ("cloud", Value::Bool(is_cloud(d))),
                    ("here", Value::Bool(here.as_deref() == Some(id))),
                    ("online", Value::Bool(online)),
                ];
                // When it was last seen, while it's away (Settings → Devices
                // and the place picker say so). Online, its beat moves every
                // few seconds and shows nowhere; it going quiet is published
                // as that happens (`look_again`).
                if let Some(at) = seen.filter(|_| !online) {
                    fields.push(("seen", Value::Num(at as f64)));
                }
                // The Zeron it runs, as it last said (Settings → Devices).
                if let Some(version) = d["version"].as_str().filter(|v| !v.is_empty()) {
                    fields.push(("version", Value::from(version)));
                }
                Some(Value::record(fields))
            })
            .collect();
        Value::List(devices.into())
    }

    fn value(&self) -> Value {
        self.value_at(now_seconds())
    }

    /// What scripts see, judged at `now` (Unix seconds).
    fn value_at(&self, now: i64) -> Value {
        let state = if self.heard.get() { "ready" } else { "loading" };
        let here = self.here.borrow().clone();
        let devices = self.devices_value(now);
        *self.shown_devices.borrow_mut() = Some(devices.clone());
        let spaces = self.spaces.borrow();
        let mut known_spaces: HashSet<&str> = HashSet::new();
        let projects: Vec<Value> = spaces
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| {
                let id = p["id"].as_str()?;
                known_spaces.insert(id);
                let path = p["path"].as_str().unwrap_or_default();
                Some(Value::record([
                    ("id", Value::from(id)),
                    ("name", Value::from(project_name(p["name"].as_str(), path))),
                    ("device", Value::from(p["deviceId"].as_str().unwrap_or_default())),
                    ("path", Value::from(path)),
                    ("git", Value::Bool(p["gitDetected"].as_bool().unwrap_or(false))),
                    // Its GitHub repository (`owner/name`), the one a Cloud
                    // chat of it clones.
                    ("github", p["githubRepo"].as_str().filter(|r| !r.is_empty()).map_or(Value::Nil, Value::from)),
                    // Which repository it's a clone of, on any device (opaque:
                    // equal ones are one repository), and when it was added.
                    ("repository", p["repositoryId"].as_str().filter(|r| !r.is_empty()).map_or(Value::Nil, Value::from)),
                    ("created", p["createdAt"].as_str().and_then(unix_seconds).map_or(Value::Nil, |at| Value::from(at as f64))),
                    // The picture its people chose: `{ emoji?, image?, hue? }`,
                    // nil for its default tile.
                    ("icon", project_icon(&p["icon"])),
                ]))
            })
            .collect();
        let (chats, children, archived) = self.chat_lists(now, &known_spaces);
        let places = self.expected.borrow();
        let ids = |list: &[String]| Value::List(list.iter().map(|id| Value::from(id.as_str())).collect());
        let sections: Vec<Value> = places
            .sections
            .iter()
            .map(|s| {
                Value::record([
                    ("id", Value::from(s.id.as_str())),
                    ("name", Value::from(s.name.as_str())),
                    ("sessions", ids(&s.sessions)),
                    ("collapsed", Value::Bool(s.collapsed)),
                ])
            })
            .collect();
        let value = Value::record([
            ("state", Value::from(state)),
            ("here", here.as_deref().map_or(Value::Nil, Value::from)),
            ("devices", devices),
            ("projects", Value::List(projects.into())),
            ("chats", chats),
            ("children", children),
            ("archived", archived),
            (
                "places",
                Value::record([("pinned", ids(&places.pinned)), ("sections", Value::List(sections.into()))]),
            ),
            ("places_ready", Value::Bool(self.places_ready.get())),
            ("connection", connection(&self.connectivity.borrow()).map_or(Value::Nil, Value::from)),
        ]);
        self.show_live(now);
        value
    }

    /// The scripts' `chats`, `children` and `archived` at `now`: built whole
    /// the first time (and after a change to the lists' shape, the known
    /// projects or the registry's icons), then only the rows of chats whose
    /// entry or live state changed, or went stale, since; and the lists
    /// holding them.
    fn chat_lists(&self, now: i64, known: &HashSet<&str>) -> (Value, Value, Value) {
        let at = chrono::DateTime::from_timestamp(now, 0).unwrap_or_default();
        let live_rows = self.live_rows.borrow();
        let live = |id: &str| live_rows.winner(id, at);
        let chats = self.chats.borrow();
        let all = chats.as_array().map(Vec::as_slice).unwrap_or_default();
        let mut index = self.chats_at.borrow_mut();
        index.ensure(all, "id");
        let mut rows = self.rows.borrow_mut();
        let mut layout = self.layout.borrow_mut();
        let dirty = std::mem::take(&mut *self.dirty_rows.borrow_mut());
        let icons = crate::agents::icons_generation();
        let current = layout.as_ref().filter(|l| {
            l.built_at <= now
                && l.icons == icons
                && l.spaces.len() == known.len()
                && known.iter().all(|s| l.spaces.contains(*s))
                && index.covers(all)
        });
        let Some(since) = current.map(|l| l.built_at) else {
            let built = Layout::build(all, &mut rows, &live, at, known, now, icons);
            let lists = built.values();
            *layout = Some(built);
            return lists;
        };
        let layout = layout.as_mut().expect("current");
        #[cfg(test)]
        BUILT_KEPT.with(|n| n.set(n.get() + 1));
        // What may have moved: entries and live states changed, states gone
        // stale since, and sleeps (they turn with time).
        let mut dirty: HashSet<String> = dirty;
        dirty.extend(live_rows.turns.between(since, now));
        dirty.extend(layout.sleepers.iter().cloned());
        let entry = |id: &str, index: &mut lists::Index| index.find(all, "id", id).map(|i| &all[i]);
        // Side chats first: their parents count them.
        let mut parents: Vec<String> = Vec::new();
        for id in &dirty {
            let Some(Place::Child(parent)) = layout.place.get(id) else { continue };
            let Some(c) = entry(id, &mut index) else { continue };
            child_row(&mut rows, c, id, parent, live(id), at);
            if !parents.contains(parent) {
                parents.push(parent.clone());
            }
        }
        for parent in &parents {
            let i = layout.parent_at[parent];
            layout.kids[i].sort(&rows);
            dirty.insert(parent.clone());
        }
        if !parents.is_empty() {
            layout.children = layout.children_value();
        }
        let (mut top, mut gone) = (false, false);
        for id in &dirty {
            let (list, pos, changed) = match layout.place.get(id) {
                Some(Place::Top) => (&mut layout.top, layout.top_at[id], &mut top),
                Some(Place::Gone) => (&mut layout.gone, layout.gone_at[id], &mut gone),
                _ => continue,
            };
            let Some(c) = entry(id, &mut index) else { continue };
            let (side_chats, working) = layout.parent_at.get(id).map_or((0, 0), |&i| (layout.kids[i].ids.len(), layout.kids[i].working));
            let value = top_row(&mut rows, c, id, known, live(id), at, side_chats, working);
            if !matches!((&list[pos], &value), (Value::Map(a), Value::Map(b)) if Rc::ptr_eq(a, b)) {
                list[pos] = value;
                *changed = true;
            }
        }
        if top {
            layout.chats = Value::List(layout.top.clone().into());
        }
        if gone {
            layout.archived = Value::List(layout.gone.clone().into());
        }
        layout.built_at = now;
        layout.values()
    }

    async fn mutate(&self, params: Json) -> Result<Value, Outcome> {
        engine::get()
            .call("Mutate", params)
            .await
            .map(|_| Value::Nil)
            .map_err(|error| fail(&error))
    }

    async fn create(&self, args: Value) -> Result<Value, Outcome> {
        let field = |key: &str| args.field(key).map(text).filter(|s| !s.is_empty());
        // The caller's id, where it made one first (a side chat's tab, open
        // before its first message makes it), else a new one.
        let id = field("id").filter(|id| plain_id(id)).unwrap_or_else(new_id);
        let config = args
            .field("config")
            .filter(|c| !matches!(c, Value::Nil))
            .and_then(|c| serde_json::from_str::<Json>(&c.to_json()).ok());
        let mut params = json!({ "op": "createChat", "chatId": id });
        // A side chat: under its parent, where the parent is (its device,
        // project, config, branch and folder), as the shipping app's
        // `create_child_chat` copies them, unless the call says otherwise.
        if let Some(parent) = field("parent") {
            let Some(at) = where_parent_is(&self.chats.borrow(), &parent) else { return Err("missing") };
            for (k, v) in at.as_object().into_iter().flatten() {
                params[k] = v.clone();
            }
            // Moved to no project (its composer's place picker): none of the
            // parent's project, folder or branch.
            if args.field("no_project").is_some_and(Value::truthy)
                && let Some(map) = params.as_object_mut()
            {
                for wire in ["spaceId", "cwd", "branch"] {
                    map.remove(wire);
                }
            }
        }
        for (key, wire) in [("project", "spaceId"), ("device", "deviceId"), ("branch", "branch"), ("cwd", "cwd")] {
            if let Some(v) = field(key) {
                params[wire] = Json::from(v);
            }
        }
        if let Some(config) = config {
            params["config"] = config;
        }
        let device = field("device")
            .or_else(|| params["deviceId"].as_str().map(str::to_owned))
            .or_else(|| self.here.borrow().clone());
        if field("project").is_none() && params["spaceId"].is_null() {
            match device {
                Some(device) => params["deviceId"] = Json::from(device),
                None => return Err("offline"),
            }
        }
        self.mutate(params.clone()).await?;
        // Known to the chat's sender at once (a side chat's first message
        // follows straight away, before the engine's list has it).
        crate::chat::made(&params);
        if let Some(title) = field("title") {
            let _ = self.mutate(json!({ "op": "renameChat", "chatId": id, "title": title })).await;
        }
        Ok(Value::from(id.as_str()))
    }

    /// A chat on Cloud: a machine of its own (minted by the edge), cloning
    /// the project's GitHub repository from `branch` (its default with
    /// none), as the shipping app's composer starts one (`createChat` with
    /// `cloud`). `{ id, cwd }`, the folder on that machine its runs go in;
    /// or why not, in the engine's words (Cloud can't reach the repository,
    /// out of credits), or an outcome word.
    async fn create_on_cloud(&self, args: Value) -> Result<Value, String> {
        let field = |key: &str| args.field(key).map(text).filter(|s| !s.is_empty());
        let project = field("project").ok_or("Cloud sessions run in a project.")?;
        let id = field("id").filter(|id| plain_id(id)).unwrap_or_else(new_id);
        let mut params = json!({ "op": "createChat", "chatId": id, "spaceId": project, "cloud": true });
        if let Some(branch) = field("branch") {
            params["branch"] = Json::from(branch);
        }
        if let Some(config) = args
            .field("config")
            .filter(|c| !matches!(c, Value::Nil))
            .and_then(|c| serde_json::from_str::<Json>(&c.to_json()).ok())
        {
            params["config"] = config;
        }
        let made = engine::get().call("Mutate", params).await.map_err(|error| {
            eprintln!("zeron-zgx: sessions: no Cloud session: {error}");
            match engine::outcome(&error) {
                "failed" => cloud_words(&error),
                word => word.to_string(),
            }
        })?;
        let cwd = made["cwd"].as_str().unwrap_or_default();
        Ok(Value::record([("id", Value::from(id.as_str())), ("cwd", Value::from(cwd))]))
    }

    /// A side chat that starts as a copy of `source` (the shipping app's Fork
    /// chat: `ForkSideChat`, made on the source's device, the copy up to its
    /// last finished reply), under `parent` (the source by default): the new
    /// chat's id, or the engine's words for why not.
    async fn fork(&self, args: Value) -> Result<Value, String> {
        let field = |key: &str| args.field(key).map(text).filter(|s| !s.is_empty());
        let source = field("source").ok_or("Start a conversation before creating a side chat.")?;
        // (The caller's id, where its tab shows "Forking…" first: the same id
        // tried again is the same fork.)
        let id = field("id").filter(|id| plain_id(id)).unwrap_or_else(new_id);
        let params = fork_params(&self.chats.borrow(), &id, &source, field("parent").as_deref());
        engine::get().call("ForkSideChat", params).await?;
        Ok(Value::from(id.as_str()))
    }

    /// The places the scripts want: the changes from what's expected go to
    /// the engine in order.
    fn want(self: &Rc<Self>, mut wanted: Places) {
        let changes = {
            let expected = self.expected.borrow();
            // A section's fold is the engine's: the scripts' places don't
            // carry it.
            for s in &mut wanted.sections {
                if let Some(old) = expected.sections.iter().find(|o| o.id == s.id) {
                    s.collapsed = old.collapsed;
                }
            }
            if *expected == wanted {
                return;
            }
            changes(&expected, &wanted)
        };
        *self.expected.borrow_mut() = wanted;
        self.queue.borrow_mut().extend(changes);
        self.publish();
        self.drain();
    }

    /// A section folded or opened, on every device: through the same queue
    /// as the places, so it never overtakes a change made before it.
    fn collapse(self: &Rc<Self>, id: String, collapsed: bool) {
        {
            let mut expected = self.expected.borrow_mut();
            let Some(section) = expected.sections.iter_mut().find(|s| s.id == id) else { return };
            if section.collapsed == collapsed {
                return;
            }
            section.collapsed = collapsed;
        }
        self.queue
            .borrow_mut()
            .push_back(section(json!({ "action": "collapse", "id": id, "collapsed": collapsed })));
        self.publish();
        self.drain();
    }

    fn drain(self: &Rc<Self>) {
        if self.sending.get() {
            return;
        }
        self.sending.set(true);
        let me = self.clone();
        engine::get().spawn(async move {
            loop {
                let next = me.queue.borrow_mut().pop_front();
                let Some(change) = next else { break };
                if let Err(error) = engine::get()
                    .call("Mutate", json!({ "op": "changeSidebarPin", "change": change }))
                    .await
                {
                    eprintln!("zeron-zgx: sessions: a sidebar change didn't take: {error}");
                }
            }
            me.sending.set(false);
            // All through: the engine's places stand from here.
            let confirmed = me.confirmed.borrow().clone();
            *me.expected.borrow_mut() = confirmed;
            me.publish();
        });
    }
}

/// A device row of Cloud's: the logical Cloud device or a session's machine
/// (`zeron_proto::is_cloud_device` and the Cloud capabilities).
pub(crate) fn is_cloud(d: &Json) -> bool {
    let id = d["id"].as_str().unwrap_or_default();
    let platform = d["platform"].as_str().unwrap_or_default();
    platform == "cloud"
        || id.starts_with("cloud-")
        || d["capabilities"]
            .as_array()
            .is_some_and(|c| c.iter().any(|c| matches!(c.as_str(), Some("cloud-session" | "cloud-account"))))
}

/// Why the engine didn't remove a device, as a word for the page.
fn removal_refused(error: &str) -> Outcome {
    if error.contains("this_device") {
        "here"
    } else if error.contains("device_online") {
        "online"
    } else if error.contains("cloud_device") {
        "cloud"
    } else if error.contains("unknown variant") {
        "outdated"
    } else {
        fail(error)
    }
}

/// An engine error as an outcome: offline, or failed.
/// Why a project's picture wasn't set, as a word the dialog says plainly:
/// `outdated` (this Mac's Zeron predates project icons and refused the
/// request), `gone` (the project was removed meanwhile), `invalid` (the
/// picture was refused), `offline`, else `failed`.
fn icon_fail(error: &str) -> Outcome {
    if error.contains("unknown variant") && error.contains("setSpaceIcon") {
        return "outdated";
    }
    if error.contains("no such project") {
        return "gone";
    }
    if error.contains("invalid icon") {
        return "invalid";
    }
    fail(error)
}

fn fail(error: &str) -> Outcome {
    match engine::outcome(error) {
        "offline" => "offline",
        _ => "failed",
    }
}

/// Why a Cloud chat didn't start, for the screen: the engine's own sentence
/// (Cloud can't reach the repository, the project has none) without the
/// machine's prefixes; out of credits said plainly; anything else, or too
/// long to read, a plain retry.
pub(crate) fn cloud_words(error: &str) -> String {
    let lower = error.to_ascii_lowercase();
    if lower.contains("credit") {
        return "You're out of Cloud credits. Add some in Settings → Billing.".into();
    }
    let mut words = error.trim();
    for prefix in ["forwarded:", "github_not_connected:", "failed:"] {
        while let Some(rest) = words.strip_prefix(prefix) {
            words = rest.trim();
        }
    }
    let readable = !words.is_empty()
        && words.len() <= 160
        && words.chars().next().is_some_and(char::is_uppercase)
        && !words.contains('{');
    if readable { words.to_string() } else { "Cloud couldn't start this chat. Try again.".into() }
}

/// An id a caller may give a chat: letters, digits and dashes, 8 to 64.
fn plain_id(id: &str) -> bool {
    (8..=64).contains(&id.len()) && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Where a side chat of `parent` is made (the shipping app's
/// `create_child_chat`): under it, on its device, in its project, with its
/// config, branch and folder. None when there's no such chat.
fn where_parent_is(chats: &Json, parent: &str) -> Option<Json> {
    let found = chats.as_array()?.iter().find(|c| c["id"].as_str() == Some(parent))?;
    let mut out = json!({ "parentChatId": parent });
    for wire in ["deviceId", "spaceId", "config", "branch", "cwd"] {
        if !found[wire].is_null() {
            out[wire] = found[wire].clone();
        }
    }
    Some(out)
}

/// `ForkSideChat`'s params: the copy `id` of `source`, under `parent` (the
/// source by default), made on the source's device.
fn fork_params(chats: &Json, id: &str, source: &str, parent: Option<&str>) -> Json {
    let device = chats.as_array().into_iter().flatten().find(|c| c["id"].as_str() == Some(source)).and_then(|c| c["deviceId"].as_str());
    let mut params = json!({ "chatId": id, "sourceChatId": source, "parentChatId": parent.unwrap_or(source) });
    if let Some(device) = device {
        params["targetDeviceId"] = Json::from(device);
    }
    params
}

fn chat_title(title: Option<&str>) -> &str {
    title.map(str::trim).filter(|t| !t.is_empty()).unwrap_or("New chat")
}

/// A project's chosen picture as the scripts read it: its parts that are
/// set, nil when none is (or the engine's is malformed).
fn project_icon(icon: &Json) -> Value {
    let part = |key: &str| icon[key].as_str().map(str::trim).filter(|v| !v.is_empty()).map(Value::from);
    let fields: Vec<(&str, Value)> = ["emoji", "image", "hue"]
        .into_iter()
        .filter_map(|key| part(key).map(|v| (key, v)))
        .collect();
    if fields.is_empty() { Value::Nil } else { Value::record(fields) }
}

/// A project's name: its own, else its folder's (proto's `display_name`).
fn project_name<'a>(name: Option<&'a str>, path: &'a str) -> &'a str {
    if let Some(name) = name.filter(|n| !n.trim().is_empty()) {
        return name;
    }
    let trimmed = path.trim_end_matches(['/', '\\']);
    trimmed.rsplit(['/', '\\']).next().filter(|s| !s.is_empty()).unwrap_or(path)
}

/// What a chat's live state says, by chat, as far as anything shown reads
/// it (`status_of`'s `display_status_from`, the waits, the subagents): one
/// of these changing is what's worth publishing. Several hosts may report
/// one chat: `live_sessions` picks the one that says most.
#[derive(PartialEq, Debug)]
struct Live {
    indicator: zeron_proto::view::Indicator,
    /// What its agent waits on, while that shows.
    waiting: Option<Vec<zeron_proto::WaitingOn>>,
    /// Its subagents streaming now, none once the row has gone quiet.
    subagents: u32,
    /// Its last turn's outcome and the turn it marks, as the status reads
    /// them (whether a finish is new, or was interrupted).
    outcome: Option<String>,
    last_completed_turn: Option<String>,
}

/// Each chat's `Live`, judged at `now` (Unix seconds).
#[cfg(test)]
fn live_states(value: &Json, now: i64) -> HashMap<String, Live> {
    let at = chrono::DateTime::from_timestamp(now, 0).unwrap_or_default();
    live_sessions(value, at).into_iter().map(|(chat, s)| (chat, live_of(&s, at))).collect()
}

/// A chat's `Live` from the row that says most for it, at `at`.
fn live_of(s: &Session, at: chrono::DateTime<chrono::Utc>) -> Live {
    Live {
        indicator: zeron_proto::view::effective_indicator(Some(s), at),
        waiting: zeron_proto::view::waiting(Some(s), at).map(|w| w.on.clone()),
        subagents: zeron_proto::view::running_subagents(Some(s), at),
        outcome: s.last_turn_outcome.as_ref().and_then(|o| serde_json::to_string(o).ok()),
        last_completed_turn: s.last_completed_turn.clone(),
    }
}

/// When the next live state that shows (a run, or a wait: not idle, not yet
/// stale) goes stale, in Unix seconds: the next moment a look changes
/// anything. (`LiveRows::stale` keeps these as rows change.)
#[cfg(test)]
fn next_stale(value: &Json, now: i64) -> Option<i64> {
    value.as_array().into_iter().flatten().filter_map(stale_at).filter(|&at| at > now).min()
}

/// When the next device shown online (another one, seen within
/// `ONLINE_SECS`) stops being, in Unix seconds.
fn next_offline(devices: &Json, here: Option<&str>, now: i64) -> Option<i64> {
    devices
        .as_array()
        .into_iter()
        .flatten()
        .filter(|d| d["id"].as_str() != here)
        .filter_map(|d| d["lastSeenAt"].as_str().and_then(unix_seconds))
        .map(|seen| seen + ONLINE_SECS + 1)
        .filter(|&at| at > now)
        .min()
}

/// When the next sleeping chat wakes on its own, in Unix seconds.
/// (`Sessions::wakes` keeps these as entries change.)
#[cfg(test)]
fn next_wake(chats: &Json, now: i64) -> Option<i64> {
    chats.as_array().into_iter().flatten().filter_map(wake_at).filter(|&at| at > now).min()
}

/// Each chat's session row, as its hosts report them. Several may report
/// one chat (re-homed mid-flight): the one that says most wins, a run
/// showing, then a wait showing, then the newest.
/// (`LiveRows` keeps these parsed, a chat at a time, as rows change.)
#[cfg(test)]
fn live_sessions(value: &Json, now: chrono::DateTime<chrono::Utc>) -> HashMap<String, Session> {
    use serde::Deserialize;
    let mut out: HashMap<String, Session> = HashMap::new();
    for row in value.as_array().into_iter().flatten() {
        let Ok(session) = Session::deserialize(row) else { continue };
        match out.get(&session.chat_id) {
            Some(held) if (says(held, now), held.updated_at) >= (says(&session, now), session.updated_at) => {}
            _ => {
                out.insert(session.chat_id.clone(), session);
            }
        }
    }
    out
}

/// How much a session row says at `now`: a run showing, a wait showing,
/// nothing (`live_sessions` keeps the one that says most).
fn says(s: &Session, now: chrono::DateTime<chrono::Utc>) -> u8 {
    if zeron_proto::view::effective_indicator(Some(s), now) != zeron_proto::view::Indicator::None {
        2
    } else if zeron_proto::view::waiting(Some(s), now).is_some() {
        1
    } else {
        0
    }
}

/// What the sidebar's status reads from a chat's own row.
fn seen_marks(chat: &Json) -> SeenMarks {
    let at = |key: &str| {
        chat[key].as_str().and_then(unix_seconds).and_then(|s| chrono::DateTime::from_timestamp(s, 0))
    };
    SeenMarks { last_message_at: at("lastMessageAt"), last_seen_at: at("lastSeenAt") }
}

/// The sidebar's state for a chat, as every viewport reads it
/// (`zeron_proto::view::display_status`): working, waiting on work that will
/// wake it, waiting on you, failed or finished since you last looked; else
/// none.
fn status_of(
    marks: SeenMarks,
    session: Option<&Session>,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<&'static str> {
    match zeron_proto::view::display_status_from(marks, session, now) {
        ChatIndicator::Working => Some("working"),
        ChatIndicator::Waiting => Some("waiting"),
        ChatIndicator::AwaitingInput => Some("input"),
        ChatIndicator::Errored => Some("failed"),
        ChatIndicator::Completed => Some("done"),
        ChatIndicator::Idle => None,
    }
}

/// A chat's sleep at `now`: asleep until (`true`, its time), or woke and not
/// looked at since (`false`, when it woke).
fn sleep_of(chat: &Json, marks: SeenMarks, now: chrono::DateTime<chrono::Utc>) -> Option<(bool, i64)> {
    use serde::Deserialize;
    if chat["sleep"].is_null() {
        return None;
    }
    let sleep = zeron_proto::ChatSleep::deserialize(&chat["sleep"]).ok()?;
    match sleep.state(marks.last_seen_at, now) {
        SleepState::Awake => None,
        SleepState::Asleep { until } => Some((true, until.timestamp())),
        SleepState::Woke { at } => Some((false, at.timestamp())),
    }
}

/// What a chat's agent waits on, as scripts read it.
fn waiting_value(on: &[zeron_proto::WaitingOn]) -> Value {
    Value::List(on.iter().map(|w| {
        Value::record([
            ("id", Value::from(w.id.as_str())),
            (
                "kind",
                Value::from(match w.kind {
                    zeron_proto::WaitingKind::Agent => "agent",
                    _ => "process",
                }),
            ),
            ("label", Value::from(w.label.as_str())),
        ])
    }).collect())
}

fn places_of_json(value: &Json) -> Places {
    let ids = |v: &Json| -> Vec<String> {
        v.as_array().into_iter().flatten().filter_map(|i| i.as_str().map(Into::into)).collect()
    };
    Places {
        pinned: ids(&value["pinnedSessionIds"]),
        sections: value["sections"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|s| {
                Some(Section {
                    id: s["id"].as_str()?.into(),
                    name: s["name"].as_str().unwrap_or_default().into(),
                    sessions: ids(&s["session_ids"]),
                    collapsed: s["collapsed"].as_bool().unwrap_or(false),
                })
            })
            .collect(),
    }
}

fn places_of_value(value: &Value) -> Places {
    let ids = |v: Option<&Value>| -> Vec<String> { v.map(|v| v.list().iter().map(text).collect()).unwrap_or_default() };
    Places {
        pinned: ids(value.field("pinned")),
        sections: value
            .field("sections")
            .map(|v| v.list())
            .unwrap_or_default()
            .iter()
            .map(|s| Section {
                id: s.field("id").map(text).unwrap_or_default(),
                name: s.field("name").map(text).unwrap_or_default(),
                sessions: ids(s.field("sessions")),
                collapsed: s.field("collapsed").is_some_and(Value::truthy),
            })
            .filter(|s| !s.id.is_empty())
            .collect(),
    }
}

/// Places a session among `list` as the engine does (proto's
/// `SidebarPinChange::project`): before `before` if it's there, else after
/// `after`, else at the end.
fn put(list: &mut Vec<String>, id: &str, after: Option<&str>, before: Option<&str>) {
    list.retain(|v| v != id);
    let at = before
        .and_then(|b| list.iter().position(|v| v == b))
        .or_else(|| after.and_then(|a| list.iter().position(|v| v == a).map(|i| i + 1)))
        .unwrap_or(list.len());
    list.insert(at, id.to_owned());
}

fn section(change: Json) -> Json {
    json!({ "action": "section", "change": change })
}

/// The pin and section changes (the engine's `SidebarPinChange`s) that take
/// `from` to `to`, in the order to send them: sections made, gone, renamed
/// and reordered; then each section's sessions; then those let out of every
/// section; then pins. Each step is checked against `from` as the earlier
/// ones leave it, so one drop is one change.
fn changes(from: &Places, to: &Places) -> Vec<Json> {
    let mut out = Vec::new();
    let mut sim = from.clone();
    let wanted: HashSet<&str> = to.sections.iter().map(|s| s.id.as_str()).collect();
    for gone in sim.sections.iter().filter(|s| !wanted.contains(s.id.as_str())) {
        out.push(section(json!({ "action": "delete", "id": gone.id })));
    }
    sim.sections.retain(|s| wanted.contains(s.id.as_str()));
    for s in &to.sections {
        match sim.sections.iter_mut().find(|o| o.id == s.id) {
            None => {
                out.push(section(json!({ "action": "create", "id": s.id, "name": s.name })));
                sim.sections.push(Section { id: s.id.clone(), name: s.name.clone(), ..Section::default() });
            }
            Some(old) if old.name != s.name => {
                out.push(section(json!({ "action": "rename", "id": s.id, "name": s.name })));
                old.name = s.name.clone();
            }
            Some(_) => {}
        }
    }
    // Their order.
    for (i, s) in to.sections.iter().enumerate() {
        if sim.sections.get(i).map(|o| o.id.as_str()) == Some(s.id.as_str()) {
            continue;
        }
        let before = sim.sections.get(i).map(|o| o.id.clone());
        let after = i.checked_sub(1).map(|j| to.sections[j].id.clone());
        out.push(section(json!({ "action": "move", "id": s.id, "after": after, "before": before })));
        let at = sim.sections.iter().position(|o| o.id == s.id).unwrap_or(0);
        let moved = sim.sections.remove(at);
        let at = before
            .as_deref()
            .and_then(|b| sim.sections.iter().position(|o| o.id == b))
            .unwrap_or(sim.sections.len());
        sim.sections.insert(at, moved);
    }
    // Their sessions (an assignment takes a session out of the pins too).
    for s in &to.sections {
        for (i, id) in s.sessions.iter().enumerate() {
            let here = sim.sections.iter().find(|o| o.id == s.id).map(|o| o.sessions.get(i).cloned());
            if here == Some(Some(id.clone())) {
                continue;
            }
            let before = here.flatten();
            let after = i.checked_sub(1).map(|j| s.sessions[j].clone());
            let mut change = json!({ "action": "assign", "sessionId": id, "sectionId": s.id });
            if let Some(after) = &after {
                change["after"] = Json::from(after.as_str());
            }
            if let Some(before) = &before {
                change["before"] = Json::from(before.as_str());
            }
            out.push(section(change));
            sim.pinned.retain(|p| p != id);
            for o in &mut sim.sections {
                o.sessions.retain(|v| v != id);
            }
            if let Some(o) = sim.sections.iter_mut().find(|o| o.id == s.id) {
                put(&mut o.sessions, id, after.as_deref(), before.as_deref());
            }
        }
    }
    let placed: HashSet<&str> = to.sections.iter().flat_map(|s| s.sessions.iter().map(String::as_str)).collect();
    for o in &mut sim.sections {
        for id in o.sessions.clone() {
            if !placed.contains(id.as_str()) {
                out.push(section(json!({ "action": "assign", "sessionId": id, "sectionId": null })));
                o.sessions.retain(|v| *v != id);
            }
        }
    }
    // Pins.
    let pinned: HashSet<&str> = to.pinned.iter().map(String::as_str).collect();
    for id in sim.pinned.clone() {
        if !pinned.contains(id.as_str()) {
            out.push(json!({ "action": "unpin", "sessionId": id }));
            sim.pinned.retain(|v| *v != id);
        }
    }
    for (i, id) in to.pinned.iter().enumerate() {
        if sim.pinned.get(i) == Some(id) {
            continue;
        }
        let before = sim.pinned.get(i).cloned();
        let after = i.checked_sub(1).map(|j| to.pinned[j].clone());
        let action = if sim.pinned.contains(id) { "move" } else { "pin" };
        out.push(json!({ "action": action, "sessionId": id, "after": after, "before": before }));
        put(&mut sim.pinned, id, after.as_deref(), before.as_deref());
    }
    out
}

/// A new chat's id: a random UUID (v4), as the engine's own are.
/// What the sidebar's connection line says (production's
/// `connection_pill_caption`): a chat's changes that couldn't be saved
/// outrank the network; then offline, or reconnecting; healthy (or no edge
/// at all), nothing.
fn connection(c: &Json) -> Option<&'static str> {
    let unsaved = c["chats"].as_array().is_some_and(|chats| chats.iter().any(|chat| chat["syncState"] == "storageError"));
    if unsaved {
        return Some("unsaved");
    }
    match c["state"].as_str() {
        Some("offline") => Some("offline"),
        Some("reconnecting") => Some("reconnecting"),
        _ => None,
    }
}

/// The workspace a link opens in, opaque (production's
/// `links::workspace_locator`): the first 16 hex of SHA-256 over its scope
/// (as Rust names it: "Local", "Synced", "Development") and, synced, who is
/// signed in and in which organization ("personal" without one); local,
/// this device. Not yet knowable (signed out while synced): none.
fn workspace_locator(scope: &str, device: Option<&str>, signed_in: Option<(String, Option<String>)>) -> Option<String> {
    let (name, identity) = match scope {
        "synced" | "development" => {
            let (user, org) = signed_in?;
            let name = if scope == "synced" { "Synced" } else { "Development" };
            (name, format!("user:{user}:org:{}", org.as_deref().unwrap_or("personal")))
        }
        "local" => ("Local", format!("device:{}", device?)),
        _ => return None,
    };
    Some(zeron_notify::links::workspace_locator_of(name, &identity))
}

/// The locator of the workspace the engine has open now (none until it's
/// knowable): what a conversation link carries, and is checked against
/// when one is opened (src/links.rs).
pub(crate) async fn current_locator() -> Option<String> {
    let info = engine::get().call("EngineInfo", json!({})).await.ok()?;
    let scope = info["workspaceScope"].as_str().unwrap_or_default();
    workspace_locator(scope, info["deviceId"].as_str(), crate::auth::signed_in())
}

/// A Zeron conversation link (crates/notify, the shipping app's).
use zeron_notify::links::conversation_link;

pub(crate) fn new_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut bytes = [0u8; 16];
    for (i, chunk) in bytes.chunks_mut(8).enumerate() {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u128(SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
        hasher.write_usize(i);
        chunk.copy_from_slice(&hasher.finish().to_le_bytes());
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32])
}

pub(crate) fn now_seconds() -> i64 {
    #[cfg(test)]
    if let Some(now) = CLOCK.with(Cell::get) {
        return now;
    }
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

#[cfg(test)]
thread_local! {
    /// A test's own time (Unix seconds), while it sets one.
    static CLOCK: Cell<Option<i64>> = const { Cell::new(None) };
    /// How many times the chat lists were built whole, and kept.
    static BUILT_WHOLE: Cell<u32> = const { Cell::new(0) };
    static BUILT_KEPT: Cell<u32> = const { Cell::new(0) };
}

#[cfg(test)]
/// An RFC 3339 time for `secs` (Unix seconds), as the engine writes them.
pub(crate) fn rfc3339(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rest = secs.rem_euclid(86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.000Z", rest / 3600, (rest % 3600) / 60, rest % 60)
}

/// An RFC 3339 time as Unix seconds ("2026-10-31T21:27:23.486Z", or with an
/// offset).
pub(crate) fn unix_seconds(stamp: &str) -> Option<i64> {
    #[cfg(test)]
    lists::work::add(1);
    let (date, time) = stamp.split_once('T')?;
    let mut date = date.split('-').map(|p| p.parse::<i64>().ok());
    let (y, m, d) = (date.next()??, date.next()??, date.next()??);
    if time.len() < 8 {
        return None;
    }
    let mut clock = time[..8].split(':').map(|p| p.parse::<i64>().ok());
    let (h, min, s) = (clock.next()??, clock.next()??, clock.next()??);
    // Days from the civil date (Howard Hinnant's algorithm).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let mut seconds = days * 86_400 + h * 3600 + min * 60 + s;
    if let Some(sign_at) = time[8..].find(['+', '-']) {
        let offset = &time[8 + sign_at..];
        let sign = if offset.starts_with('-') { -1 } else { 1 };
        let mut parts = offset[1..].split(':').map(|p| p.parse::<i64>().unwrap_or(0));
        let (oh, om) = (parts.next().unwrap_or(0), parts.next().unwrap_or(0));
        seconds -= sign * (oh * 3600 + om * 60);
    }
    Some(seconds)
}

#[cfg(test)]
mod tests {
    /// "Delete session?" counts what the engine's `deleteChat` takes: its
    /// children on computers and theirs, archived ones too (the lists show
    /// none of those); a Cloud chat's Cloud children; never a computer's
    /// Cloud delegate, nor anyone else's chats.
    #[test]
    fn a_delete_counts_what_goes_with_it() {
        let chats = serde_json::json!([
            { "id": "p", "deviceId": "mac" },
            { "id": "side", "deviceId": "mac", "parentChatId": "p" },
            { "id": "own", "deviceId": "mac", "parentChatId": "p", "childKind": "side" },
            { "id": "sub", "deviceId": "mac", "parentChatId": "p", "childKind": "subagent", "archived": true },
            { "id": "subsub", "deviceId": "mac", "parentChatId": "sub", "childKind": "subagent" },
            { "id": "delegate", "deviceId": "cloud-d", "parentChatId": "p", "childKind": "subagent" },
            { "id": "other", "deviceId": "mac" },
            { "id": "others", "deviceId": "mac", "parentChatId": "other" },
            { "id": "c", "deviceId": "cloud-c" },
            { "id": "c-delegate", "deviceId": "cloud-e", "parentChatId": "c", "childKind": "subagent" },
            { "id": "c-side", "deviceId": "cloud-c", "parentChatId": "c", "childKind": "side" },
        ]);
        let all = chats.as_array().unwrap();
        assert_eq!(super::deleted_with(all, "p"), (2, 2));
        assert_eq!(super::deleted_with(all, "c"), (1, 1));
        assert_eq!(super::deleted_with(all, "others"), (0, 0));
        assert_eq!(super::deleted_with(all, "gone"), (0, 0));
    }

    /// Settings → Devices says why a device wasn't removed in a word of its
    /// own for each of the engine's refusals (crates/engine rpc.rs
    /// `RemoveDevice`), and an engine from before Remove says it's outdated.
    #[test]
    fn a_refused_removal_says_why() {
        assert_eq!(super::removal_refused("bad params: this_device"), "here");
        assert_eq!(super::removal_refused("device_online"), "online");
        assert_eq!(super::removal_refused("bad params: cloud_device"), "cloud");
        assert_eq!(
            super::removal_refused("bad params: unknown variant `removeDevice`, expected one of `createChat`"),
            "outdated"
        );
        assert_eq!(super::removal_refused("offline"), "offline");
        assert_eq!(super::removal_refused("disk full"), "failed");
    }

    /// A conversation link's workspace, hashed as production's links.rs
    /// does (so a link copied here opens there, and back).
    #[test]
    fn conversation_links_match_production() {
        let who = || Some(("u1".to_string(), Some("o1".to_string())));
        assert_eq!(super::workspace_locator("synced", None, who()).as_deref(), Some("9a37c6ea679064d8"));
        assert_eq!(super::workspace_locator("synced", None, Some(("u1".into(), None))).as_deref(), Some("0ddcd6f86c3d02d2"));
        assert_eq!(super::workspace_locator("development", None, who()).as_deref(), Some("a86094ce7b623374"));
        assert_eq!(super::workspace_locator("local", Some("d9"), None).as_deref(), Some("e2ee91fa9d910190"));
        assert_eq!(super::workspace_locator("synced", None, None), None, "signed out: not knowable");
        assert_eq!(super::workspace_locator("local", None, who()), None);
        assert_eq!(super::conversation_link("c 1/x", "abc"), "zeron://open/chat/c%201%2Fx?workspace=abc");
    }

    /// The connection line says what production's pill says, a chat's
    /// unsaved changes first, healthy (or no edge) nothing.
    #[test]
    fn the_connection_line_follows_production() {
        use serde_json::json;
        assert_eq!(super::connection(&json!({ "state": "connected" })), None);
        assert_eq!(super::connection(&json!({ "state": "disabled" })), None);
        assert_eq!(super::connection(&Json::Null), None);
        assert_eq!(super::connection(&json!({ "state": "offline" })), Some("offline"));
        assert_eq!(super::connection(&json!({ "state": "reconnecting" })), Some("reconnecting"));
        let unsaved = json!({ "state": "connected", "chats": [{ "chatId": "a", "syncState": "storageError", "connected": true }] });
        assert_eq!(super::connection(&unsaved), Some("unsaved"));
    }

    use super::*;

    fn places(pinned: &[&str], sections: &[(&str, &str, &[&str])]) -> Places {
        Places {
            pinned: pinned.iter().map(|s| s.to_string()).collect(),
            sections: sections
                .iter()
                .map(|(id, name, ids)| Section {
                    id: id.to_string(),
                    name: name.to_string(),
                    sessions: ids.iter().map(|s| s.to_string()).collect(),
                    collapsed: false,
                })
                .collect(),
        }
    }

    #[test]
    fn times_read_as_unix_seconds() {
        assert_eq!(unix_seconds("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(unix_seconds("2026-10-31T21:27:23.486Z"), Some(1_793_482_043));
        assert_eq!(unix_seconds("2026-10-31T22:27:23+01:00"), Some(1_793_482_043));
        assert_eq!(unix_seconds("nonsense"), None);
    }

    #[test]
    fn one_drop_is_one_change() {
        let from = places(&["a", "b", "c"], &[]);
        // c dragged to the top.
        let out = changes(&from, &places(&["c", "a", "b"], &[]));
        assert_eq!(out, vec![json!({ "action": "move", "sessionId": "c", "after": null, "before": "a" })]);
        // d pinned between a and b.
        let out = changes(&from, &places(&["a", "d", "b", "c"], &[]));
        assert_eq!(out, vec![json!({ "action": "pin", "sessionId": "d", "after": "a", "before": "b" })]);
        // b unpinned.
        let out = changes(&from, &places(&["a", "c"], &[]));
        assert_eq!(out, vec![json!({ "action": "unpin", "sessionId": "b" })]);
        assert!(changes(&from, &from).is_empty());
    }

    #[test]
    fn sections_are_made_filled_renamed_and_emptied() {
        let from = places(&["a"], &[]);
        let to = places(&[], &[("s1", "Later", &["a"])]);
        let out = changes(&from, &to);
        assert_eq!(
            out,
            vec![
                section(json!({ "action": "create", "id": "s1", "name": "Later" })),
                section(json!({ "action": "assign", "sessionId": "a", "sectionId": "s1" })),
            ]
        );
        let out = changes(&to, &places(&[], &[("s1", "Someday", &["a"])]));
        assert_eq!(out, vec![section(json!({ "action": "rename", "id": "s1", "name": "Someday" }))]);
        let out = changes(&to, &places(&[], &[("s1", "Later", &[])]));
        assert_eq!(out, vec![section(json!({ "action": "assign", "sessionId": "a", "sectionId": null }))]);
        let out = changes(&to, &places(&[], &[]));
        assert_eq!(out, vec![section(json!({ "action": "delete", "id": "s1" }))]);
    }

    #[test]
    fn states_are_every_viewports() {
        let now = 1_000_000;
        let stamp = |secs: i64| {
            let days = secs.div_euclid(86_400);
            // Only the time of day matters for these: the epoch's first day.
            assert_eq!(days, 11);
            format!("1970-01-12T{:02}:{:02}:{:02}Z", (secs % 86_400) / 3600, (secs % 3600) / 60, secs % 60)
        };
        let at = chrono::DateTime::from_timestamp(now, 0).unwrap();
        let live = live_sessions(
            &json!([
                { "chatId": "w", "deviceId": "d", "status": "working", "updatedAt": stamp(now - 5) },
                { "chatId": "old", "deviceId": "d", "status": "working", "updatedAt": stamp(now - 120) },
                { "chatId": "q", "deviceId": "d", "status": "awaitingInput", "updatedAt": stamp(now - 5) },
                { "chatId": "z", "deviceId": "d", "status": "idle", "updatedAt": stamp(now - 5),
                  "waiting": { "since": stamp(now - 5), "on": [{ "id": "bn1", "kind": "process", "label": "cargo test" }] } },
                { "chatId": "gone", "deviceId": "d", "status": "idle", "updatedAt": stamp(now - 120),
                  "waiting": { "since": stamp(now - 120), "on": [] } },
                // One chat on two hosts: the one running wins.
                { "chatId": "two", "deviceId": "a", "status": "idle", "updatedAt": stamp(now - 1) },
                { "chatId": "two", "deviceId": "b", "status": "working", "updatedAt": stamp(now - 5) },
            ]),
            at,
        );
        let unseen = seen_marks(&json!({ "lastMessageAt": stamp(now - 10), "lastSeenAt": stamp(now - 20) }));
        let seen = seen_marks(&json!({ "lastMessageAt": stamp(now - 30), "lastSeenAt": stamp(now - 20) }));
        assert_eq!(status_of(seen, live.get("w"), at), Some("working"));
        assert_eq!(status_of(seen, live.get("q"), at), Some("input"));
        assert_eq!(status_of(unseen, live.get("z"), at), Some("waiting"));
        assert_eq!(status_of(seen, live.get("two"), at), Some("working"));
        // A run gone quiet is no run, and no finish either.
        assert_eq!(status_of(seen, live.get("old"), at), None);
        assert_eq!(status_of(unseen, live.get("old"), at), None);
        // A wait gone quiet: its turn did end, so it's finished, unseen.
        assert_eq!(status_of(unseen, live.get("gone"), at), Some("done"));
        assert_eq!(status_of(seen, None, at), None);
        assert_eq!(status_of(unseen, None, at), Some("done"));
    }

    #[test]
    fn a_sleep_reads_asleep_until_its_time_then_woke_until_looked_at() {
        let at = chrono::DateTime::from_timestamp(1_000_000, 0).unwrap();
        let stamp = |secs: i64| chrono::DateTime::from_timestamp(secs, 0).unwrap().to_rfc3339();
        let chat = |seen: i64, sleep: serde_json::Value| json!({ "lastSeenAt": stamp(seen), "sleep": sleep });
        let asleep = chat(999_000, json!({ "until": stamp(1_003_600), "at": stamp(999_000) }));
        assert_eq!(sleep_of(&asleep, seen_marks(&asleep), at), Some((true, 1_003_600)));
        let woke = chat(999_000, json!({ "until": stamp(1_003_600), "at": stamp(999_000), "wokeAt": stamp(999_500) }));
        assert_eq!(sleep_of(&woke, seen_marks(&woke), at), Some((false, 999_500)));
        let looked = chat(999_600, json!({ "until": stamp(1_003_600), "at": stamp(999_000), "wokeAt": stamp(999_500) }));
        assert_eq!(sleep_of(&looked, seen_marks(&looked), at), None);
        assert_eq!(sleep_of(&json!({}), seen_marks(&json!({})), at), None);
        // The next wake on its own: a minute's look scheduled exactly then.
        assert_eq!(next_wake(&json!([asleep, woke]), 1_000_000), Some(1_003_601));
    }

    #[test]
    fn states_are_looked_at_again_only_as_one_goes_stale() {
        let now = 1_000_000;
        let stamp = |secs: i64| format!("1970-01-12T{:02}:{:02}:{:02}Z", (secs % 86_400) / 3600, (secs % 3600) / 60, secs % 60);
        let rows = json!([
            { "chatId": "w", "deviceId": "d", "status": "working", "updatedAt": stamp(now - 5) },
            { "chatId": "q", "deviceId": "d", "status": "awaitingInput", "updatedAt": stamp(now - 30) },
            { "chatId": "old", "deviceId": "d", "status": "working", "updatedAt": stamp(now - 120) },
            { "chatId": "i", "deviceId": "d", "status": "idle", "updatedAt": stamp(now - 1) },
        ]);
        // The soonest: q, stale once it's 45 s old.
        assert_eq!(next_stale(&rows, now), Some(now - 30 + STALE_SECS + 1));
        // A wait shows too, while it's fresh.
        let waits = json!([
            { "chatId": "z", "deviceId": "d", "status": "idle", "updatedAt": stamp(now - 40), "waiting": { "since": stamp(now - 40) } },
        ]);
        assert_eq!(next_stale(&waits, now), Some(now - 40 + STALE_SECS + 1));
        // Nothing showing, or only stale and idle states: no look at all.
        let quiet = json!([
            { "chatId": "old", "deviceId": "d", "status": "working", "updatedAt": stamp(now - 120) },
            { "chatId": "i", "deviceId": "d", "status": "idle", "updatedAt": stamp(now - 1) },
        ]);
        assert_eq!(next_stale(&quiet, now), None);
        assert_eq!(next_stale(&json!([]), now), None);
    }

    #[test]
    fn a_device_shown_online_is_looked_at_again_as_it_goes_quiet() {
        let now = 1_000_000;
        let stamp = |secs: i64| format!("1970-01-12T{:02}:{:02}:{:02}Z", (secs % 86_400) / 3600, (secs % 3600) / 60, secs % 60);
        let devices = json!([
            { "id": "here", "lastSeenAt": stamp(now - 1) },
            { "id": "laptop", "lastSeenAt": stamp(now - 50) },
            { "id": "phone", "lastSeenAt": stamp(now - 20) },
            { "id": "gone", "lastSeenAt": stamp(now - 500) },
            { "id": "never" },
        ]);
        // The laptop's beat goes stale first; this device never does.
        assert_eq!(next_offline(&devices, Some("here"), now), Some(now - 50 + ONLINE_SECS + 1));
        let quiet = json!([{ "id": "here", "lastSeenAt": stamp(now - 1) }, { "id": "gone", "lastSeenAt": stamp(now - 500) }]);
        assert_eq!(next_offline(&quiet, Some("here"), now), None);
    }

    #[test]
    fn ids_are_uuids() {
        let id = new_id();
        assert_eq!(id.len(), 36);
        assert_eq!(&id[14..15], "4");
        assert_ne!(id, new_id());
    }

    /// Chats (a parent, two side chats, one working, one archived) and
    /// their live rows, as the engine's watches give them.
    fn with_children(subagents_at: &str) -> super::Sessions {
        let s = super::Sessions::default();
        s.set_chats(serde_json::json!([
            { "id": "p", "title": "Parent", "deviceId": "d", "createdAt": "2026-11-01T10:00:00Z", "config": { "harness": "claude-code" } },
            { "id": "a", "parentChatId": "p", "title": "", "lastMessagePreview": "Looking at the tests", "deviceId": "d",
              "createdAt": "2026-11-01T10:05:00Z", "lastMessageAt": "2026-11-01T10:06:00Z", "config": { "harness": "codex" },
              "childKind": "subagent" },
            { "id": "b", "parentChatId": "p", "deviceId": "d", "createdAt": "2026-11-01T10:07:00Z" },
            { "id": "gone", "parentChatId": "p", "archived": true, "deviceId": "d", "createdAt": "2026-11-01T10:08:00Z" },
        ]));
        s.set_live(serde_json::json!([
            { "chatId": "p", "deviceId": "d", "status": "working", "updatedAt": subagents_at, "runningSubagents": 2 },
            { "chatId": "a", "deviceId": "d", "status": "working", "updatedAt": "2099-01-01T00:00:00Z" },
        ]));
        s
    }

    fn chat<'a>(v: &'a zgx_script::Value, id: &str) -> &'a zgx_script::Value {
        v.field("chats").unwrap().list().iter().find(|c| c.field("id").map(crate::auth::text).as_deref() == Some(id)).unwrap()
    }

    #[test]
    fn side_chats_are_their_parent_s_and_counted_on_its_row() {
        let v = with_children("2099-01-01T00:00:00Z").value();
        // Not in the sidebar's list.
        let ids: Vec<String> = v.field("chats").unwrap().list().iter().filter_map(|c| c.field("id").map(crate::auth::text)).collect();
        assert_eq!(ids, vec!["p"]);
        // Under their parent, newest first; an archived one isn't.
        let kids = v.field("children").unwrap().field("p").unwrap().list();
        let kid_ids: Vec<String> = kids.iter().filter_map(|c| c.field("id").map(crate::auth::text)).collect();
        assert_eq!(kid_ids, vec!["b", "a"]);
        // Its title, else what it said, else the shipping app's word.
        assert_eq!(kids[1].field("title").map(crate::auth::text).as_deref(), Some("Looking at the tests"));
        assert_eq!(kids[0].field("title").map(crate::auth::text).as_deref(), Some("New side chat"));
        assert_eq!(kids[1].field("status").map(crate::auth::text).as_deref(), Some("working"));
        assert_eq!(kids[1].field("parent").map(crate::auth::text).as_deref(), Some("p"));
        // Started by a Zeron chat, which kind; the person's, none.
        assert_eq!(kids[1].field("child_kind").map(crate::auth::text).as_deref(), Some("subagent"));
        assert!(kids[0].field("child_kind").is_none_or(|k| matches!(k, zgx_script::Value::Nil)));
        let p = chat(&v, "p");
        assert_eq!(p.field("side_chats").and_then(|n| n.as_f64()), Some(2.));
        assert_eq!(p.field("working_side_chats").and_then(|n| n.as_f64()), Some(1.));
        assert_eq!(p.field("running_subagents").and_then(|n| n.as_f64()), Some(2.));
    }

    #[test]
    fn a_chat_s_row_is_built_again_only_as_it_changes() {
        let s = with_children("2099-01-01T00:00:00Z");
        let same = |a: &zgx_script::Value, b: &zgx_script::Value| match (a, b) {
            (zgx_script::Value::Map(a), zgx_script::Value::Map(b)) => std::rc::Rc::ptr_eq(a, b),
            _ => false,
        };
        let first = s.value();
        let again = s.value();
        assert!(same(chat(&first, "p"), chat(&again, "p")));
        // Another chat's change: this one's row stands, that one's doesn't.
        let mut chats = s.chats.borrow().clone();
        chats[1]["title"] = serde_json::json!("Renamed");
        s.heard(super::Slot::Chats, chats.clone());
        let after = s.value();
        assert!(same(chat(&first, "p"), chat(&after, "p")));
        let kid = |v: &zgx_script::Value, at: usize| v.field("children").unwrap().field("p").unwrap().list()[at].clone();
        assert_eq!(kid(&after, 1).field("title").map(crate::auth::text).as_deref(), Some("Renamed"));
        assert!(same(&kid(&first, 0), &kid(&after, 0)));
        // Its live state's change shows, whatever its entry.
        s.heard(
            super::Slot::Live,
            serde_json::json!([{ "chatId": "p", "deviceId": "d", "status": "awaitingInput", "updatedAt": "2099-01-01T00:00:00Z" }]),
        );
        let waiting = s.value();
        assert!(!same(chat(&first, "p"), chat(&waiting, "p")));
        assert_eq!(chat(&waiting, "p").field("status").map(crate::auth::text).as_deref(), Some("input"));
        assert_eq!(chat(&waiting, "p").field("working_side_chats").and_then(|n| n.as_f64()), Some(0.));
        // Gone from the list: gone from what's kept.
        chats.as_array_mut().unwrap().remove(2);
        s.heard(super::Slot::Chats, chats);
        assert!(!s.rows.borrow().contains_key("b"));
    }

    #[test]
    fn an_update_that_changes_nothing_publishes_nothing() {
        let s = with_children("2099-01-01T00:00:00Z");
        let _ = s.value();
        s.due.set(None);
        let chats = s.chats.borrow().clone();
        s.heard.set(true);
        s.heard(super::Slot::Chats, chats);
        // (`publish` would have asked to look again at the live state going stale.)
        assert_eq!(s.due.get(), None);
    }

    /// The engine's heartbeats (every couple of seconds, the whole list
    /// again): a row whose `updatedAt` alone moved, or a device's beat while
    /// it's online, changes nothing shown and publishes nothing; a change that
    /// shows is published at once.
    #[test]
    fn a_heartbeat_that_shows_nothing_new_publishes_nothing() {
        let now = now_seconds();
        let stamp = |secs: i64| iso(secs);
        let s = with_children("2099-01-01T00:00:00Z");
        s.heard.set(true);
        *s.here.borrow_mut() = Some("here".into());
        *s.devices.borrow_mut() = json!([
            { "id": "here", "name": "This Mac", "lastSeenAt": stamp(now - 1) },
            { "id": "laptop", "name": "Laptop", "lastSeenAt": stamp(now - 5) },
            { "id": "gone", "name": "Old PC", "lastSeenAt": stamp(now - 5000) },
        ]);
        s.set_live(json!([
            { "chatId": "p", "deviceId": "here", "status": "working", "updatedAt": stamp(now - 2), "runningSubagents": 2 },
            { "chatId": "a", "deviceId": "here", "status": "idle", "updatedAt": stamp(now - 600) },
        ]));
        s.publish();
        let before = s.published.get();
        // The laptop beats; this Mac beats; the working chat's row is fresher;
        // an idle one's time moves.
        s.heard(Slot::Devices, json!([
            { "id": "here", "name": "This Mac", "lastSeenAt": stamp(now) },
            { "id": "laptop", "name": "Laptop", "lastSeenAt": stamp(now) },
            { "id": "gone", "name": "Old PC", "lastSeenAt": stamp(now - 5000) },
        ]));
        s.heard(Slot::Live, json!([
            { "chatId": "p", "deviceId": "here", "status": "working", "updatedAt": stamp(now), "runningSubagents": 2 },
            { "chatId": "a", "deviceId": "here", "status": "idle", "updatedAt": stamp(now) },
        ]));
        assert_eq!(s.published.get(), before, "nothing shown moved");
        // A real change: shown at once.
        s.heard(Slot::Live, json!([
            { "chatId": "p", "deviceId": "here", "status": "awaitingInput", "updatedAt": stamp(now), "runningSubagents": 2 },
            { "chatId": "a", "deviceId": "here", "status": "idle", "updatedAt": stamp(now) },
        ]));
        assert_eq!(s.published.get(), before + 1);
        assert_eq!(chat(&s.value(), "p").field("status").map(crate::auth::text).as_deref(), Some("input"));
        // The old PC comes back: shown at once, online.
        s.heard(Slot::Devices, json!([
            { "id": "here", "name": "This Mac", "lastSeenAt": stamp(now) },
            { "id": "laptop", "name": "Laptop", "lastSeenAt": stamp(now) },
            { "id": "gone", "name": "Old PC", "lastSeenAt": stamp(now) },
        ]));
        assert_eq!(s.published.get(), before + 2);
        let online = |v: &zgx_script::Value, id: &str| {
            v.field("devices").unwrap().list().iter().find(|d| d.field("id").map(crate::auth::text).as_deref() == Some(id)).unwrap().field("online").cloned()
        };
        assert_eq!(online(&s.value(), "gone"), Some(zgx_script::Value::Bool(true)));
    }

    /// A device's last beat is given while it's away (Settings → Devices and
    /// the place picker say when it was last seen); while it's online the beat
    /// shows nowhere, and isn't given.
    #[test]
    fn a_device_s_last_beat_is_given_while_it_s_away() {
        let now = now_seconds();
        let s = Sessions::default();
        *s.here.borrow_mut() = Some("here".into());
        *s.devices.borrow_mut() = json!([
            { "id": "here", "lastSeenAt": iso(now - 1) },
            { "id": "laptop", "lastSeenAt": iso(now - 5) },
            { "id": "gone", "lastSeenAt": iso(now - 5000) },
            { "id": "never" },
        ]);
        let v = s.value();
        let device = |id: &str| v.field("devices").unwrap().list().iter().find(|d| d.field("id").map(crate::auth::text).as_deref() == Some(id)).unwrap().clone();
        for (id, online, seen) in [("here", true, None), ("laptop", true, None), ("gone", false, Some((now - 5000) as f64)), ("never", false, None)] {
            let d = device(id);
            assert_eq!(d.field("online"), Some(&zgx_script::Value::Bool(online)), "{id}");
            assert_eq!(d.field("seen").and_then(|n| n.as_f64()), seen, "{id}");
        }
    }

    use super::rfc3339 as iso;

    #[test]
    fn running_subagents_go_quiet_with_their_row() {
        let v = with_children("2001-01-01T00:00:00Z").value();
        assert_eq!(chat(&v, "p").field("running_subagents").and_then(|n| n.as_f64()), Some(0.));
    }

    #[test]
    fn a_side_chat_is_made_where_its_parent_is_and_a_fork_on_its_device() {
        let chats = serde_json::json!([
            { "id": "p", "deviceId": "mac", "spaceId": "proj", "branch": "main", "cwd": "/w", "config": { "harness": "codex" } },
        ]);
        assert_eq!(
            super::where_parent_is(&chats, "p").unwrap(),
            serde_json::json!({ "parentChatId": "p", "deviceId": "mac", "spaceId": "proj", "branch": "main", "cwd": "/w", "config": { "harness": "codex" } })
        );
        assert!(super::where_parent_is(&chats, "nope").is_none());
        assert_eq!(
            super::fork_params(&chats, "new", "p", None),
            serde_json::json!({ "chatId": "new", "sourceChatId": "p", "parentChatId": "p", "targetDeviceId": "mac" })
        );
        // A side chat's own fork hangs beside it, under its parent.
        assert_eq!(super::fork_params(&chats, "new", "p", Some("top"))["parentChatId"], "top");
    }

    #[test]
    fn a_project_icon_refusal_says_why() {
        // (An engine from before project icons: serde's words for the op.)
        assert_eq!(super::icon_fail("bad params: unknown variant `setSpaceIcon`, expected one of `createChat`, `renameSpace`"), "outdated");
        assert_eq!(super::icon_fail("no such project: sp-1"), "gone");
        assert_eq!(super::icon_fail("bad params: invalid icon: image"), "invalid");
        assert_eq!(super::icon_fail("something else broke"), "failed");
    }

    #[test]
    fn a_caller_s_id_is_taken_when_it_s_plain() {
        assert!(super::plain_id("3f2a9c1e-0b4d-4e7a-9c3b-1d2e3f4a5b6c"));
        assert!(!super::plain_id("short"));
        assert!(!super::plain_id("../../etc/passwd-xxxxxxxx"));
    }
}

#[cfg(test)]
mod bench {
    use super::*;
    use crate::engine::lists::{Changes, Frame};

    /// The engine's lists for `n` chats: a few working, the rest idle.
    pub(super) fn lists(n: usize, now: i64) -> (Json, Json, Json) {
        let iso = super::rfc3339;
        let chats: Vec<Json> = (0..n)
            .map(|i| {
                let mut c = json!({ "id": format!("chat-{i:05}-aaaaaaaa-bbbb-cccc"), "title": format!("Chat {i}"), "deviceId": format!("dev-{}", i % 5),
                    "createdAt": iso(now - 100_000 + i as i64), "lastMessageAt": iso(now - 50_000 + i as i64), "lastSeenAt": iso(now - 40_000),
                    "config": { "harness": "claude-code", "model": "opus" }, "cwd": "/w", "branch": "main" });
                if i % 10 == 3 { c["parentChatId"] = json!(format!("chat-{:05}-aaaaaaaa-bbbb-cccc", i - 3)); }
                if i % 7 == 5 { c["archived"] = json!(true); }
                c
            })
            .collect();
        let live: Vec<Json> = (0..n)
            .map(|i| {
                let working = i % 13 == 0;
                json!({ "chatId": format!("chat-{i:05}-aaaaaaaa-bbbb-cccc"), "deviceId": format!("dev-{}", i % 5),
                    "status": if working { "working" } else { "idle" }, "updatedAt": iso(if working { now - 3 } else { now - 9_000 }),
                    "lastTurnOutcome": "completed", "lastCompletedTurn": format!("t{i}") })
            })
            .collect();
        let devices: Vec<Json> = (0..5).map(|i| json!({ "id": format!("dev-{i}"), "name": "D", "lastSeenAt": iso(now - 2) })).collect();
        (Json::Array(chats), Json::Array(live), Json::Array(devices))
    }

    pub(super) fn primed(n: usize, now: i64) -> (Sessions, Json, Json) {
        let (chats, live, devices) = lists(n, now);
        let s = Sessions::default();
        *s.here.borrow_mut() = Some("dev-0".into());
        s.heard_list(Slot::Chats, &Frame::for_tests(&chats, None, "id"));
        s.heard_list(Slot::Live, &Frame::for_tests(&live, None, "chatId"));
        s.heard_list(Slot::Devices, &Frame::for_tests(&devices, None, "id"));
        (s, chats, live)
    }

    /// One working chat's heartbeat (its `updatedAt` alone moves).
    pub(super) fn heartbeat(s: &Sessions, live: &mut Json, i: usize, now: i64) {
        // (Among the first hundred, so lists of any length change the same chats.)
        let _ = live.as_array().unwrap().len();
        let at = (i % 8) * 13;
        let mut row = live[at].clone();
        row["updatedAt"] = json!(super::rfc3339(now - (i % 3) as i64));
        live[at] = row.clone();
        let changes = Changes { changed: vec![row], removed: vec![], reordered: false };
        s.heard_list(Slot::Live, &Frame::for_tests(live, Some(&changes), "chatId"));
    }

    /// One chat's row moves (a new message somewhere).
    pub(super) fn chat_moves(s: &Sessions, chats: &mut Json, i: usize, now: i64) {
        // (A chat or a side chat among the first hundred, not archived ones:
        // a change to one of those has the lists built whole.)
        let _ = chats.as_array().unwrap().len();
        let at = [1, 3, 8, 13, 22, 23, 30, 41, 57, 73, 84, 99][i % 12];
        let mut row = chats[at].clone();
        row["lastMessageAt"] = json!(super::rfc3339(now - 10 + (i % 5) as i64));
        row["lastMessagePreview"] = json!(format!("said {i}"));
        chats[at] = row.clone();
        let changes = Changes { changed: vec![row], removed: vec![], reordered: false };
        s.heard_list(Slot::Chats, &Frame::for_tests(chats, Some(&changes), "id"));
    }

    /// A change to one chat (its live state's heartbeat, its entry) costs
    /// what one chat costs, with 100 chats or 1,000: the rows read, the
    /// times parsed and the lists looked over are the same in number.
    #[test]
    fn one_chat_s_change_costs_that_chat_not_the_list() {
        let now = now_seconds();
        let cost = |n: usize| {
            let (s, mut chats, mut live) = primed(n, now);
            s.heard.set(true);
            s.publish();
            // (The first change of each list makes its index.)
            heartbeat(&s, &mut live, 0, now);
            chat_moves(&s, &mut chats, 0, now);
            let before = crate::engine::lists::work::done();
            for i in 1..40 {
                heartbeat(&s, &mut live, i, now);
                chat_moves(&s, &mut chats, i, now);
            }
            crate::engine::lists::work::done() - before
        };
        let (small, large) = (cost(100), cost(1000));
        assert_eq!(small, large, "the work for a change grew with the list: {small} for 100 chats, {large} for 1,000");
    }

    struct Rng(u64);
    impl Rng {
        fn next(&mut self, below: u64) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0 % below
        }
    }

    /// The engine's list after a frame of changes, as a kept copy reads it
    /// (`lists::apply_rows`: a changed row where the last of its key is,
    /// else last; removed keys gone).
    fn applied(list: &mut Json, key: &str, changes: &Changes) {
        let rows = list.as_array_mut().unwrap();
        rows.retain(|r| !r[key].as_str().is_some_and(|k| changes.removed.iter().any(|g| g == k)));
        for row in &changes.changed {
            match row[key].as_str().and_then(|k| rows.iter().rposition(|r| r[key].as_str() == Some(k))) {
                Some(at) => rows[at] = row.clone(),
                None => rows.push(row.clone()),
            }
        }
    }

    /// What scripts see, from the lists alone (built whole), at the test's
    /// time.
    fn built_whole(chats: &Json, live: &Json, devices: &Json, spaces: &Json) -> Value {
        let fresh = Sessions::default();
        *fresh.here.borrow_mut() = Some("dev-0".into());
        fresh.heard(Slot::Spaces, spaces.clone());
        fresh.set_chats(chats.clone());
        fresh.set_live(live.clone());
        *fresh.devices.borrow_mut() = devices.clone();
        fresh.heard.set(true);
        fresh.value()
    }

    /// The devices as scripts would be given them, at `now`.
    fn devices_shown(devices: &Json, now: i64) -> Value {
        let s = Sessions::default();
        *s.here.borrow_mut() = Some("dev-0".into());
        *s.devices.borrow_mut() = devices.clone();
        s.devices_value(now)
    }

    /// What the scripts see, kept a change at a time, is what building it
    /// all from the lists gives, through every kind of change and as time
    /// passes (states going stale, devices quiet, sleeps ending) or is put
    /// back; and a change is published, and looked at again, when the
    /// whole lists say. Compared at every publish and at random moments
    /// between (heartbeats run on unpublished in between).
    #[test]
    fn kept_a_change_at_a_time_it_reads_as_built_whole() {
        let iso = super::rfc3339;
        let mut now = 1_900_000_000;
        CLOCK.with(|c| c.set(Some(now)));
        let id = |i: u64| format!("chat-{i:05}-aaaaaaaa-bbbb-cccc");
        let (mut chats, mut live, mut devices) = lists(40, now);
        let mut spaces = json!([{ "id": "sp1", "path": "/a" }, { "id": "sp2", "path": "/b" }]);
        let s = Sessions::default();
        *s.here.borrow_mut() = Some("dev-0".into());
        s.heard(Slot::Spaces, spaces.clone());
        s.heard_list(Slot::Chats, &Frame::for_tests(&chats, None, "id"));
        s.heard_list(Slot::Live, &Frame::for_tests(&live, None, "chatId"));
        s.heard_list(Slot::Devices, &Frame::for_tests(&devices, None, "id"));
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        let (mut publishes, mut kept, mut looks) = (0, 0, 0);
        for step in 0..6000 {
            // Time passes: a little, past a state's staleness, or put back.
            match rng.next(30) {
                0..=1 => now += 40 + rng.next(10) as i64,
                2..=9 => now += rng.next(4) as i64,
                10 => now -= 1 + rng.next(20) as i64,
                _ => {}
            }
            CLOCK.with(|c| c.set(Some(now)));
            // A few chats, none twice.
            let mut picked: Vec<String> = Vec::new();
            for _ in 0..1 + rng.next(3) {
                let chat = id(rng.next(48));
                if !picked.contains(&chat) {
                    picked.push(chat);
                }
            }
            // Whether the whole lists say a publish is due: a state or a
            // device shown moving, judged as `shows_the_same` judged it.
            let shows_new = |s: &Sessions, live: &Json, devices: &Json| {
                *s.shown_live.borrow() != live_states(live, now) || s.shown_devices.borrow().as_ref() != Some(&devices_shown(devices, now))
            };
            let (published, whole, kept_before) = (s.published.get(), BUILT_WHOLE.with(Cell::get), BUILT_KEPT.with(Cell::get));
            let expect: Option<bool> = match rng.next(20) {
                // Live states: heartbeats, new statuses, waits, rows gone;
                // now and then one with no key.
                0..=7 => {
                    let mut changes = Changes { changed: vec![], removed: vec![], reordered: false };
                    for chat in &picked {
                        if rng.next(8) == 0 {
                            changes.removed.push(chat.clone());
                            continue;
                        }
                        let mut row = json!({ "chatId": chat, "deviceId": "dev-1",
                            "status": (["working", "awaitingInput", "idle", "errored"][rng.next(4) as usize]),
                            "updatedAt": iso(now - rng.next(50) as i64), "runningSubagents": rng.next(3),
                            "lastTurnOutcome": (["completed", "interrupted"][rng.next(2) as usize]), "lastCompletedTurn": format!("t{}", rng.next(3)) });
                        if rng.next(4) == 0 {
                            row["waiting"] = json!({ "since": iso(now - 5), "on": [{ "id": "b1", "kind": "process", "label": "cargo test" }] });
                        }
                        // (Rare: it stays till a whole list drops it, and the
                        // lists are built whole while it does.)
                        if rng.next(400) == 0 {
                            row.as_object_mut().unwrap().remove("chatId");
                        }
                        changes.changed.push(row);
                    }
                    applied(&mut live, "chatId", &changes);
                    let expect = shows_new(&s, &live, &devices);
                    s.heard_list(Slot::Live, &Frame::for_tests(&live, Some(&changes), "chatId"));
                    Some(expect)
                }
                // Chats' entries: a message, a look, a title, a sleep, a
                // project, an agent, archived, made a side chat; new; gone.
                8..=12 => {
                    let mut changes = Changes { changed: vec![], removed: vec![], reordered: false };
                    for chat in &picked {
                        if rng.next(15) == 0 {
                            changes.removed.push(chat.clone());
                            continue;
                        }
                        let at = chats.as_array().unwrap().iter().rposition(|r| r["id"] == chat.as_str());
                        let mut c = at.map_or_else(|| json!({ "id": chat, "deviceId": "dev-2", "createdAt": iso(now - 100) }), |at| chats[at].clone());
                        // (Mostly what a chat's entry does while it works: a
                        // message, a look.)
                        match rng.next(16) {
                            0 | 9..=12 => c["lastMessageAt"] = json!(iso(now - rng.next(3) as i64)),
                            13..=15 => c["lastSeenAt"] = json!(iso(now - rng.next(3) as i64)),
                            1 => c["lastSeenAt"] = json!(iso(now - rng.next(3) as i64)),
                            2 => c["title"] = json!(format!("T{}", rng.next(5))),
                            3 => c["sleep"] = json!({ "until": iso(now + rng.next(30) as i64), "at": iso(now - 10) }),
                            4 => c["sleep"] = if rng.next(2) == 0 { Json::Null } else { json!({ "until": iso(now + 5), "at": iso(now - 10), "wokeAt": iso(now - 1) }) },
                            5 => c["spaceId"] = json!((["sp1", "sp2", "sp9"][rng.next(3) as usize])),
                            6 => c["archived"] = json!(rng.next(3) == 0),
                            7 => c["parentChatId"] = if rng.next(3) == 0 { Json::Null } else { json!(id(rng.next(48))) },
                            8 => c["config"]["harness"] = json!((["codex", "claude-code", "acp.x"][rng.next(3) as usize])),
                            _ => unreachable!(),
                        }
                        changes.changed.push(c);
                    }
                    applied(&mut chats, "id", &changes);
                    s.heard_list(Slot::Chats, &Frame::for_tests(&chats, Some(&changes), "id"));
                    Some(true)
                }
                // A device beats, goes quiet, or is new.
                13 => {
                    let d = format!("dev-{}", rng.next(7));
                    let row = json!({ "id": d, "name": "D", "lastSeenAt": iso(now - rng.next(120) as i64) });
                    let changes = Changes { changed: vec![row], removed: vec![], reordered: false };
                    applied(&mut devices, "id", &changes);
                    let expect = shows_new(&s, &live, &devices);
                    s.heard_list(Slot::Devices, &Frame::for_tests(&devices, Some(&changes), "id"));
                    Some(expect)
                }
                // A project comes or goes.
                14 => {
                    let was = spaces.clone();
                    let rows = spaces.as_array_mut().unwrap();
                    match rows.iter().position(|p| p["id"] == "sp9") {
                        Some(at) => drop(rows.remove(at)),
                        None => rows.push(json!({ "id": "sp9", "path": "/c" })),
                    }
                    s.heard(Slot::Spaces, spaces.clone());
                    Some(spaces != was)
                }
                // The registry's icon for an agent changes.
                15 => {
                    crate::agents::remember_icons(&[json!({ "id": "acp.x", "icon": format!("<svg>{}</svg>", rng.next(1000)) })]);
                    Some(false)
                }
                // A whole chat list again, now and then naming a chat twice
                // (once archived) or one with no id.
                16 => {
                    let was = chats.clone();
                    let rows = chats.as_array_mut().unwrap();
                    // (A list naming a chat twice, or one with no id, is built
                    // whole while it does: plain again at its next whole
                    // frame, and so mostly plain.)
                    let plain = {
                        let mut seen = HashSet::new();
                        rows.iter().all(|r| r["id"].as_str().is_some_and(|i| seen.insert(i.to_owned())))
                    };
                    match if plain { rng.next(16) } else { 2 } {
                        0 => {
                            let mut twice = rows[rng.next(rows.len() as u64) as usize].clone();
                            twice["archived"] = json!(rng.next(2) == 0);
                            rows.push(twice);
                        }
                        1 => rows.push(json!({ "title": "no id", "deviceId": "dev-2", "sleep": { "until": iso(now + 7), "at": iso(now) } })),
                        2..=12 => {
                            // (Each id once again.)
                            let mut seen = HashSet::new();
                            rows.retain(|r| r["id"].as_str().is_some_and(|i| seen.insert(i.to_owned())));
                        }
                        _ => {}
                    }
                    s.heard_list(Slot::Chats, &Frame::for_tests(&chats, None, "id"));
                    Some(chats != was)
                }
                // A whole live list again, now and then with a chat on two
                // rows (two hosts), or each chat once again.
                17 => {
                    let was = live.clone();
                    let rows = live.as_array_mut().unwrap();
                    let plain = {
                        let mut seen = HashSet::new();
                        rows.iter().all(|r| r["chatId"].as_str().is_some_and(|i| seen.insert(i.to_owned())))
                    };
                    if plain && rng.next(6) == 0 {
                        let mut second = rows[rng.next(rows.len() as u64) as usize].clone();
                        second["deviceId"] = json!("dev-3");
                        second["status"] = json!((["working", "idle"][rng.next(2) as usize]));
                        second["updatedAt"] = json!(iso(now - rng.next(50) as i64));
                        rows.push(second);
                    } else {
                        let mut seen = HashSet::new();
                        rows.retain(|r| r["chatId"].as_str().is_some_and(|i| seen.insert(i.to_owned())));
                    }
                    let expect = live != was && shows_new(&s, &live, &devices);
                    s.heard_list(Slot::Live, &Frame::for_tests(&live, None, "chatId"));
                    Some(expect)
                }
                // A list in another order.
                18 => {
                    live.as_array_mut().unwrap().rotate_left(1);
                    let expect = shows_new(&s, &live, &devices);
                    let changes = Changes { changed: vec![], removed: vec![], reordered: true };
                    s.heard_list(Slot::Live, &Frame::for_tests(&live, Some(&changes), "chatId"));
                    Some(expect)
                }
                _ => {
                    chats.as_array_mut().unwrap().rotate_left(1);
                    let changes = Changes { changed: vec![], removed: vec![], reordered: true };
                    s.heard_list(Slot::Chats, &Frame::for_tests(&chats, Some(&changes), "id"));
                    Some(true)
                }
            };
            let did = s.published.get() > published;
            if let Some(expect) = expect {
                assert_eq!(did, expect, "step {step}: published");
            }
            // The next looks, as the whole lists say.
            assert_eq!(s.live_rows.borrow().stale.next_after(now), next_stale(&live, now), "step {step}: next stale");
            assert_eq!(s.wakes.borrow().next_after(now), next_wake(&chats, now), "step {step}: next wake");
            if did {
                publishes += 1;
                // (Kept, not built whole, as it was published.)
                if BUILT_WHOLE.with(Cell::get) == whole && BUILT_KEPT.with(Cell::get) > kept_before {
                    kept += 1;
                }
                let want = built_whole(&chats, &live, &devices, &spaces);
                assert_eq!(s.last_published.borrow().as_ref(), Some(&want), "step {step}: published");
                assert_eq!(*s.shown_live.borrow(), live_states(&live, now), "step {step}: states shown");
            } else if rng.next(6) == 0 {
                // A look between publishes (a window opening).
                looks += 1;
                let want = built_whole(&chats, &live, &devices, &spaces);
                assert_eq!(s.value(), want, "step {step}: a look");
            }
        }
        CLOCK.with(|c| c.set(None));
        eprintln!("{publishes} publishes, {kept} kept, {looks} looks between");
        // (A quarter at least, ~750 of ~2,400: the rest are built whole,
        // after a change of shape, projects, icons, a clock put back, or
        // while a list names a chat twice or has a row with no key.)
        assert!(kept * 4 > publishes, "only {kept} of {publishes} publishes were kept a change at a time");
    }

    /// A list naming a chat twice (a whole frame from an engine that
    /// doesn't key them), once archived: each entry shows where it would
    /// on its own, as before.
    #[test]
    fn a_chat_named_twice_shows_each_entry_where_it_would() {
        let s = Sessions::default();
        s.set_chats(json!([
            { "id": "x", "title": "Open", "createdAt": "2026-11-01T10:00:00Z" },
            { "id": "x", "title": "Put away", "archived": true, "createdAt": "2026-11-01T10:00:00Z" },
        ]));
        let v = s.value();
        let ids = |list: &str| v.field(list).unwrap().list().iter().filter_map(|c| c.field("id").map(crate::auth::text)).collect::<Vec<_>>();
        assert_eq!(ids("chats"), vec!["x"]);
        assert_eq!(ids("archived"), vec!["x"]);
    }

    /// A clock put back: a chat's live change judged the same at a later
    /// moment (its run already stale) is still published at an earlier
    /// one, where the run shows, when another chat's heartbeat comes.
    #[test]
    fn a_clock_put_back_still_shows_a_live_change() {
        let mut now = 1_900_000_000;
        CLOCK.with(|c| c.set(Some(now)));
        let iso = super::rfc3339;
        let s = Sessions::default();
        *s.here.borrow_mut() = Some("dev-0".into());
        let chat = |id: &str| json!({ "id": id, "createdAt": iso(now - 1000), "lastMessageAt": iso(now - 900), "lastSeenAt": iso(now - 800) });
        s.set_chats(json!([chat("a"), chat("b")]));
        let row = |id: &str, status: &str, at: i64| json!({ "chatId": id, "deviceId": "dev-0", "status": status, "updatedAt": iso(at) });
        let mut live = json!([row("a", "idle", now - 900), row("b", "idle", now - 900)]);
        s.set_live(live.clone());
        s.heard.set(true);
        s.publish();
        let heard = |s: &Sessions, live: &Json, at: usize| {
            let changes = Changes { changed: vec![live[at].clone()], removed: vec![], reordered: false };
            s.heard_list(Slot::Live, &Frame::for_tests(live, Some(&changes), "chatId"));
        };
        // a runs from `now + 40`; heard at `now + 100`, that run is stale:
        // nothing new shows.
        let started = now + 40;
        now += 100;
        CLOCK.with(|c| c.set(Some(now)));
        live[0] = row("a", "working", started);
        let before = s.published.get();
        heard(&s, &live, 0);
        assert_eq!(s.published.get(), before, "stale: shows nothing new");
        // The clock is put back to while a's run is fresh, and b beats.
        now -= 50;
        CLOCK.with(|c| c.set(Some(now)));
        live[1] = row("b", "idle", now - 1);
        heard(&s, &live, 1);
        assert_eq!(s.published.get(), before + 1, "a's run shows");
        assert_eq!(*s.shown_live.borrow(), live_states(&live, now));
        CLOCK.with(|c| c.set(None));
    }

    /// `cargo test --release -p zeron-zgx bench::per_update -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn per_update() {
        let now = now_seconds();
        for n in [100usize, 436, 2000] {
            let (s, mut chats, mut live) = primed(n, now);
            let k = 2000;
            let t = std::time::Instant::now();
            for i in 0..k {
                heartbeat(&s, &mut live, i, now);
            }
            let hb = t.elapsed().as_nanos() as f64 / k as f64;
            let t = std::time::Instant::now();
            for i in 0..k {
                chat_moves(&s, &mut chats, i, now);
            }
            let cm = t.elapsed().as_nanos() as f64 / k as f64;
            println!("n={n:5}  heartbeat {:8.1} µs   chat row moves {:8.1} µs", hb / 1000., cm / 1000.);
        }
    }
}
