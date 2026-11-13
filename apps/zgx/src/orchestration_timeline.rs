//! What each chat has been doing lately, for the Control room's Timeline map
//! (a line per chat over the last 15 or 60 minutes). Kept here, in the
//! window, from what `sessions` already says (src/sessions.rs): every time it
//! changes, each chat's state is compared with the last one kept, and a
//! change is written down. Nothing is asked of the engine and nothing runs on
//! a timer; a chat's history starts when this app first sees it.
//!
//! What scripts see, `orchestration.timeline`:
//!   { [chat id] = { { at, status, kind?, label? }, … } } (oldest first)
//! Each entry is when that chat became so: `at` Unix seconds, `status` its
//! `sessions` status ("working", "input", "approval", "failed", "done") or
//! "idle" when it has none, `kind` and `label` what it was doing while
//! working (its `activity`'s). An entry lasts until the next one; the last
//! lasts until now. A chat seen idle with nothing before isn't listed.
//!
//! What's kept: the last 2 hours, and for each chat the one entry before
//! them (so a state begun earlier still shows from the start of the map),
//! at most 64 entries a chat and 300 chats (those changed least recently go
//! first). A chat no longer in `sessions` keeps its history until it is all
//! older than that, so a moment's gap doesn't erase it.

use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet, VecDeque},
    rc::Rc,
};

use zgx_desktop::WindowContext;
use zgx_script::{ScriptHost, Shared, Value};

/// How far back entries are kept (seconds).
const RETAIN: i64 = 2 * 60 * 60;
/// The most entries kept for one chat.
const PER_CHAT: usize = 64;
/// The most chats kept.
const CHATS: usize = 300;

/// What a chat was doing from `at` on.
#[derive(Clone, Debug, PartialEq)]
struct Entry {
    at: i64,
    status: Rc<str>,
    kind: Option<Rc<str>>,
    label: Option<Rc<str>>,
}

impl Entry {
    fn is(&self, row: &Row) -> bool {
        &*self.status == row.status && self.kind.as_deref() == row.kind && self.label.as_deref() == row.label
    }

    fn value(&self) -> Value {
        let text = |s: &Option<Rc<str>>| s.as_ref().map_or(Value::Nil, |s| Value::Str(s.clone()));
        Value::record([
            ("at", Value::from(self.at as f64)),
            ("status", Value::Str(self.status.clone())),
            ("kind", text(&self.kind)),
            ("label", text(&self.label)),
        ])
    }
}

/// One chat as `sessions` shows it now: its status ("idle" for none), and
/// while working what it's doing and since when.
#[derive(Clone, Copy, Debug)]
struct Row<'a> {
    id: &'a str,
    status: &'a str,
    kind: Option<&'a str>,
    label: Option<&'a str>,
    since: Option<i64>,
}

/// One chat's entries, oldest first, and them as scripts see them (made
/// again only after they change).
#[derive(Default)]
struct Ring {
    entries: VecDeque<Entry>,
    value: Option<Value>,
}

impl Ring {
    fn latest(&self) -> Option<&Entry> {
        self.entries.back()
    }

    fn value(&mut self) -> Value {
        self.value
            .get_or_insert_with(|| Value::List(self.entries.iter().map(Entry::value).collect()))
            .clone()
    }

    /// Drops what's past keeping at `cutoff`: whatever is before the one
    /// entry before it, and the oldest beyond [`PER_CHAT`]. Whether any went.
    fn prune(&mut self, cutoff: i64) -> bool {
        let before = self.entries.len();
        let inside = self.entries.iter().position(|e| e.at >= cutoff).unwrap_or(self.entries.len());
        self.entries.drain(..inside.saturating_sub(1));
        while self.entries.len() > PER_CHAT {
            self.entries.pop_front();
        }
        let changed = self.entries.len() != before;
        if changed {
            self.value = None;
        }
        changed
    }
}

/// Every chat's history, as heard from `sessions`.
#[derive(Default)]
struct History {
    chats: HashMap<Rc<str>, Ring>,
    /// As scripts were last given it (none: made again when asked).
    value: Option<Value>,
}

impl History {
    /// `rows` heard at `now` (Unix seconds): a chat whose state differs from
    /// its last entry gets a new one, and what's past keeping goes. Whether
    /// anything changed.
    fn heard<'a>(&mut self, rows: impl IntoIterator<Item = Row<'a>>, now: i64) -> bool {
        let cutoff = now - RETAIN;
        let mut changed = false;
        let mut present: HashSet<&str> = HashSet::new();
        for row in rows {
            present.insert(row.id);
            let latest = self.chats.get(row.id).and_then(Ring::latest);
            // (A chat with no history counts as idle.)
            let same = match latest {
                Some(latest) => latest.is(&row),
                None => row.status == "idle",
            };
            if same {
                continue;
            }
            let at = match (latest, row.since) {
                // (When it began, if that's believable.)
                (None, Some(since)) if since <= now && since >= cutoff => since,
                (Some(latest), Some(since)) if since > latest.at && since <= now => since,
                _ => now,
            };
            // (Never before the last entry: the clock may have moved back.)
            let at = latest.map_or(at, |l| at.max(l.at));
            let entry = Entry {
                at,
                status: row.status.into(),
                kind: row.kind.map(Rc::from),
                label: row.label.map(Rc::from),
            };
            if !self.chats.contains_key(row.id) {
                self.chats.insert(row.id.into(), Ring::default());
            }
            let Some(ring) = self.chats.get_mut(row.id) else { continue };
            ring.entries.push_back(entry);
            ring.value = None;
            changed = true;
        }
        let before = self.chats.len();
        self.chats.retain(|id, ring| {
            changed |= ring.prune(cutoff);
            let Some(latest) = ring.latest() else { return false };
            // Gone from `sessions`, or idle, and nothing in the window: no
            // line to draw.
            let old = latest.at < cutoff;
            !(old && (!present.contains(&**id) || &*latest.status == "idle"))
        });
        if self.chats.len() > CHATS {
            // Those changed least recently go first; of those, the ones no
            // longer in `sessions` before those that are.
            let mut order: Vec<(bool, i64, Rc<str>)> = self
                .chats
                .iter()
                .map(|(id, ring)| (present.contains(&**id), ring.latest().map_or(i64::MIN, |e| e.at), id.clone()))
                .collect();
            order.sort();
            for (_, _, id) in order.into_iter().take(self.chats.len() - CHATS) {
                self.chats.remove(&id);
            }
        }
        changed |= self.chats.len() != before;
        if changed {
            self.value = None;
        }
        changed
    }

    fn value(&mut self) -> Value {
        if let Some(value) = &self.value {
            return value.clone();
        }
        let value = Value::record(self.chats.iter_mut().map(|(id, ring)| (id.clone(), ring.value())));
        self.value = Some(value.clone());
        value
    }
}

/// Every chat in `sessions`: its `chats` and each of its `children` lists.
fn rows(sessions: &Value) -> Vec<Row<'_>> {
    let chats = sessions.field("chats").map(Value::list).unwrap_or_default();
    let children = match sessions.field("children") {
        Some(Value::Map(children)) => &children[..],
        _ => &[],
    };
    let lists = std::iter::once(chats).chain(children.iter().map(|(_, list)| list.list()));
    lists.flat_map(|list| list.iter().filter_map(row)).collect()
}

fn row(chat: &Value) -> Option<Row<'_>> {
    let id = text(chat.field("id"))?;
    let status = text(chat.field("status")).unwrap_or("idle");
    // (What it's doing counts only while it works.)
    let activity = chat.field("activity").filter(|_| status == "working");
    let part = |key| activity.and_then(|a| a.field(key));
    Some(Row {
        id,
        status,
        kind: text(part("kind")),
        label: text(part("label")),
        since: part("since").and_then(Value::as_f64).map(|s| s as i64),
    })
}

fn text(v: Option<&Value>) -> Option<&str> {
    match v {
        Some(Value::Str(s)) => Some(s),
        _ => None,
    }
}

#[derive(Default)]
struct Timeline {
    history: RefCell<History>,
    /// Each open window's state, let go of as it closes.
    windows: RefCell<Vec<(u64, Shared)>>,
    next_window: Cell<u64>,
}

thread_local! {
    static TIMELINE: Rc<Timeline> = Rc::new(Timeline::default());
}

pub fn install(cx: &mut WindowContext, host: &ScriptHost) {
    let t = TIMELINE.with(Rc::clone);
    let window = t.next_window.get();
    t.next_window.set(window + 1);
    let value = t.history.borrow_mut().value();
    t.windows.borrow_mut().push((window, host.shared("orchestration.timeline", value)));
    let me = Rc::downgrade(&t);
    cx.on_closed(move || {
        if let Some(t) = me.upgrade() {
            t.windows.borrow_mut().retain(|(id, _)| *id != window);
        }
    });
}

/// `sessions` as just published: each chat's change written down, and
/// every window told if there was one.
pub(crate) fn heard_sessions(sessions: &Value) {
    let t = TIMELINE.with(Rc::clone);
    let now = chrono::Utc::now().timestamp();
    let value = {
        let mut history = t.history.borrow_mut();
        if !history.heard(rows(sessions), now) {
            return;
        }
        history.value()
    };
    // (Taken out first: setting one may run a script that calls back in.)
    let targets: Vec<Shared> = t.windows.borrow().iter().map(|(_, s)| s.clone()).collect();
    for shared in targets {
        shared.set(value.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_794_470_400;

    fn idle(id: &str) -> Row<'_> {
        Row { id, status: "idle", kind: None, label: None, since: None }
    }

    fn status<'a>(id: &'a str, status: &'a str) -> Row<'a> {
        Row { id, status, kind: None, label: None, since: None }
    }

    fn working<'a>(id: &'a str, kind: &'a str, label: &'a str, since: i64) -> Row<'a> {
        Row { id, status: "working", kind: Some(kind), label: Some(label), since: Some(since) }
    }

    fn ats(h: &History, id: &str) -> Vec<i64> {
        h.chats[id].entries.iter().map(|e| e.at).collect()
    }

    fn statuses(h: &History, id: &str) -> Vec<String> {
        h.chats[id].entries.iter().map(|e| e.status.to_string()).collect()
    }

    #[test]
    fn only_a_change_is_written_down() {
        let mut h = History::default();
        // Seen idle with nothing before: nothing to show.
        assert!(!h.heard([idle("a")], NOW));
        assert!(h.chats.is_empty());
        assert!(h.heard([working("a", "testing", "cargo test", NOW - 5)], NOW));
        // The same again: nothing new.
        assert!(!h.heard([working("a", "testing", "cargo test", NOW - 5)], NOW + 10));
        assert_eq!(h.chats["a"].entries.len(), 1);
        // Another label: a change.
        assert!(h.heard([working("a", "testing", "cargo clippy", NOW - 5)], NOW + 20));
        assert_eq!(ats(&h, "a"), [NOW - 5, NOW + 20]);
        // Waiting on you, then idle, then idle again.
        assert!(h.heard([status("a", "input")], NOW + 30));
        assert!(h.heard([idle("a")], NOW + 40));
        assert!(!h.heard([idle("a")], NOW + 50));
        assert_eq!(statuses(&h, "a"), ["working", "working", "input", "idle"]);
        assert_eq!(ats(&h, "a"), [NOW - 5, NOW + 20, NOW + 30, NOW + 40]);
    }

    #[test]
    fn since_dates_a_change_when_it_can() {
        let mut h = History::default();
        // First seen working: from when it began.
        assert!(h.heard([working("a", "testing", "cargo test", NOW - 300)], NOW));
        assert_eq!(ats(&h, "a"), [NOW - 300]);
        // A new activity begun after the last entry: from when it began.
        assert!(h.heard([working("a", "merging", "git merge", NOW + 15)], NOW + 20));
        assert_eq!(ats(&h, "a"), [NOW - 300, NOW + 15]);
        // A `since` no later than the last entry, or ahead of now: now.
        assert!(h.heard([working("a", "editing", "src", NOW + 15)], NOW + 30));
        assert!(h.heard([working("a", "testing", "x", NOW + 999)], NOW + 40));
        assert_eq!(ats(&h, "a"), [NOW - 300, NOW + 15, NOW + 30, NOW + 40]);
        // First seen with a `since` in the future or older than kept: now.
        assert!(h.heard([working("b", "testing", "x", NOW + 60)], NOW));
        assert!(h.heard([working("c", "testing", "x", NOW - RETAIN - 1)], NOW));
        assert_eq!((ats(&h, "b"), ats(&h, "c")), (vec![NOW], vec![NOW]));
        // First seen waiting on you: now.
        assert!(h.heard([status("d", "approval")], NOW));
        assert_eq!(ats(&h, "d"), [NOW]);
    }

    #[test]
    fn a_chat_keeps_its_last_two_hours_and_the_entry_before() {
        let mut h = History::default();
        let start = NOW - 3 * RETAIN;
        // One change a minute for over six hours, a different label each.
        let labels: Vec<String> = (0..400).map(|i| format!("step {i}")).collect();
        for (i, label) in labels.iter().enumerate() {
            let at = start + i as i64 * 60;
            h.heard([working("a", "testing", label, at)], at);
        }
        // Never more than 64.
        assert_eq!(h.chats["a"].entries.len(), PER_CHAT);
        assert_eq!(h.chats["a"].latest().unwrap().label.as_deref(), Some("step 399"));

        // Sparse: one change long ago, then quiet.
        let mut h = History::default();
        h.heard([working("a", "testing", "old", NOW - 3 * RETAIN)], NOW - 3 * RETAIN);
        h.heard([working("a", "testing", "older", NOW - 2 * RETAIN)], NOW - 2 * RETAIN);
        h.heard([working("a", "testing", "before", NOW - RETAIN - 600)], NOW - RETAIN - 600);
        h.heard([working("a", "testing", "inside", NOW - 600)], NOW - 600);
        assert!(h.heard([working("a", "testing", "inside", NOW - 600)], NOW));
        // The one before the window stays, those before it go.
        assert_eq!(ats(&h, "a"), [NOW - RETAIN - 600, NOW - 600]);
        // All of it long past, still in `sessions` and working: its latest stays.
        assert!(h.heard([working("a", "testing", "inside", NOW - 600)], NOW + 3 * RETAIN));
        assert_eq!(ats(&h, "a"), [NOW - 600]);
        // …but gone from `sessions` with nothing in the window: forgotten.
        assert!(h.heard([], NOW + 3 * RETAIN));
        assert!(h.chats.is_empty());
    }

    #[test]
    fn a_chat_gone_a_moment_keeps_its_history() {
        let mut h = History::default();
        h.heard([working("a", "testing", "x", NOW - 60), status("b", "input")], NOW);
        // `a` missing from one publish: kept, nothing changed.
        assert!(!h.heard([status("b", "input")], NOW + 5));
        assert_eq!(ats(&h, "a"), [NOW - 60]);
        // Back, the same: still nothing new.
        assert!(!h.heard([working("a", "testing", "x", NOW - 60), status("b", "input")], NOW + 10));
        // Idle and long quiet: no line, gone.
        h.heard([idle("a"), status("b", "input")], NOW + 20);
        assert!(h.heard([idle("a"), status("b", "input")], NOW + 20 + RETAIN + 1));
        assert!(!h.chats.contains_key("a"));
        assert!(h.chats.contains_key("b"));
    }

    #[test]
    fn at_most_three_hundred_chats_the_least_recently_changed_go() {
        let mut h = History::default();
        let ids: Vec<String> = (0..320).map(|i| format!("c{i}")).collect();
        for (i, id) in ids.iter().enumerate() {
            h.heard([status(id, "input")], NOW + i as i64);
        }
        assert_eq!(h.chats.len(), CHATS);
        assert!(!h.chats.contains_key("c19"));
        assert!(h.chats.contains_key("c20"));
        assert!(h.chats.contains_key("c319"));
        // Those still in `sessions` stay over those that aren't.
        let rows: Vec<Row> = ids[20..40].iter().map(|id| status(id, "input")).collect();
        assert!(h.heard(rows.iter().copied().chain([status("new", "failed")]), NOW + 400));
        assert!(h.chats.contains_key("new"));
        assert!(h.chats.contains_key("c20"));
        assert!(!h.chats.contains_key("c40"));
        assert_eq!(h.chats.len(), CHATS);
    }

    fn text(v: Option<&Value>) -> Option<String> {
        v.map(crate::auth::text)
    }

    fn nil(v: Option<&Value>) -> bool {
        v.is_none_or(|v| matches!(v, Value::Nil))
    }

    #[test]
    fn it_reads_sessions_and_shows_as_scripts_see_it() {
        let activity = |kind: &str, label: &str, since: i64| {
            Value::record([
                ("kind", Value::from(kind)),
                ("label", Value::from(label)),
                ("since", Value::from(since as f64)),
            ])
        };
        let sessions = Value::record([
            (
                "chats",
                Value::List(
                    vec![
                        Value::record([
                            ("id", Value::from("top")),
                            ("status", Value::from("working")),
                            ("activity", activity("testing", "cargo test", NOW - 30)),
                        ]),
                        Value::record([("id", Value::from("quiet"))]),
                    ]
                    .into(),
                ),
            ),
            (
                "children",
                Value::record([(
                    "top",
                    Value::List(
                        vec![Value::record([
                            ("id", Value::from("kid")),
                            ("status", Value::from("input")),
                            // (Not working: what it was doing doesn't count.)
                            ("activity", activity("merging", "git merge", NOW - 5)),
                        ])]
                        .into(),
                    ),
                )]),
            ),
        ]);
        let rows = rows(&sessions);
        assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), ["top", "quiet", "kid"]);
        let mut h = History::default();
        assert!(h.heard(rows, NOW));
        let v = h.value();
        assert!(v.field("quiet").is_none());
        let top = v.field("top").unwrap().list();
        assert_eq!(top.len(), 1);
        assert_eq!(top[0].field("at").and_then(Value::as_f64), Some((NOW - 30) as f64));
        assert_eq!(text(top[0].field("status")).as_deref(), Some("working"));
        assert_eq!(text(top[0].field("kind")).as_deref(), Some("testing"));
        assert_eq!(text(top[0].field("label")).as_deref(), Some("cargo test"));
        let kid = &v.field("kid").unwrap().list()[0];
        assert_eq!(kid.field("at").and_then(Value::as_f64), Some(NOW as f64));
        assert_eq!(text(kid.field("status")).as_deref(), Some("input"));
        assert!(nil(kid.field("kind")) && nil(kid.field("label")));
        // Asked again with nothing changed: the very same value.
        let again = h.value();
        assert!(matches!((&v, &again), (Value::Map(a), Value::Map(b)) if Rc::ptr_eq(a, b)));
        // One chat changed: the other's list is the same one as before.
        h.heard([status("top", "done"), status("kid", "input")], NOW + 10);
        let next = h.value();
        let (Some(Value::List(a)), Some(Value::List(b))) = (v.field("kid"), next.field("kid")) else { panic!() };
        assert!(Rc::ptr_eq(a, b));
        assert_eq!(next.field("top").unwrap().list().len(), 2);
    }
}
