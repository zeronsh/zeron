//! The engine's keyed lists (`WatchChats`, `WatchSessions`, `WatchDevices`),
//! each watched once a connection for every module that reads it, and as
//! changes: the whole list once, then only the rows that moved
//! (`{"changes": true}`, the engine's `keyed_changes_stream`). An engine
//! from before that sends the whole list every time, which reads the same.
//!
//! The chat list is ~0.4 MB with a few hundred chats. Each module watched it
//! on its own, so one chat's seen mark or a session's heartbeat sent it, and
//! parsed it, twice in every window; and each module then went over all of
//! it. A listener here hears the list as it now is ([`Frame::list`]) and
//! which rows changed ([`Frame::changes`]), and keeps its own copy with
//! [`Frame::apply_to_list`] or [`Frame::apply_to_map`].

use std::{
    cell::RefCell,
    collections::HashMap,
    rc::{Rc, Weak},
};

use serde_json::{Value as Json, json};

use super::rpc::{Rpc, Watch};

/// What changed in a list since the frame before.
pub struct Changes {
    /// Rows added or not as they were, in the list's order.
    pub changed: Vec<Json>,
    /// The keys of rows gone.
    pub removed: Vec<String>,
    /// Whether the rows' order isn't the one applying the rest gives (kept
    /// rows where they were, new ones after).
    pub reordered: bool,
}

/// A list as it now is, and what changed (`None`: the whole list, new).
pub struct Frame<'a> {
    pub list: &'a Json,
    pub changes: Option<&'a Changes>,
    key: &'static str,
}

impl Frame<'_> {
    /// `list` (a copy of the list kept by its listener) made this one.
    pub fn apply_to_list(&self, list: &mut Json) {
        match (self.changes, list.as_array_mut()) {
            (Some(changes), Some(rows)) if !changes.reordered => apply_rows(rows, changes, self.key),
            _ => *list = self.list.clone(),
        }
    }

    /// `map` (rows by their key) made this one's rows.
    pub fn apply_to_map(&self, map: &mut HashMap<String, Json>) {
        let Some(changes) = self.changes else {
            *map = by_key(self.list, self.key);
            return;
        };
        for key in &changes.removed {
            map.remove(key);
        }
        for row in &changes.changed {
            if let Some(key) = row[self.key].as_str() {
                map.insert(key.to_owned(), row.clone());
            }
        }
    }

    /// The keys of the rows that changed or went (`None`: any of them).
    pub fn touched(&self) -> Option<Vec<&str>> {
        let changes = self.changes?;
        Some(
            changes
                .changed
                .iter()
                .filter_map(|row| row[self.key].as_str())
                .chain(changes.removed.iter().map(String::as_str))
                .collect(),
        )
    }
}

/// A list's rows by their key.
pub fn by_key(list: &Json, key: &str) -> HashMap<String, Json> {
    list.as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| Some((row[key].as_str()?.to_owned(), row.clone())))
        .collect()
}

fn apply_rows(rows: &mut Vec<Json>, changes: &Changes, key: &str) {
    if !changes.removed.is_empty() {
        rows.retain(|row| !row[key].as_str().is_some_and(|k| changes.removed.iter().any(|r| r == k)));
    }
    let mut at: HashMap<String, usize> = HashMap::with_capacity(rows.len());
    for (i, row) in rows.iter().enumerate() {
        if let Some(k) = row[key].as_str() {
            at.insert(k.to_owned(), i);
        }
    }
    for row in &changes.changed {
        match row[key].as_str().and_then(|k| at.get(k)) {
            Some(&i) => rows[i] = row.clone(),
            None => rows.push(row.clone()),
        }
    }
}

/// The key a list method's rows go by.
fn key_of(method: &str) -> &'static str {
    if method == "WatchSessions" { "chatId" } else { "id" }
}

type Listener = Box<dyn Fn(&Frame)>;

/// One list on one connection: its watch, the list as last heard, and who
/// listens.
struct List {
    rpc: Rpc,
    method: &'static str,
    key: &'static str,
    list: Option<Rc<Json>>,
    listeners: Vec<(u64, Rc<Listener>)>,
    _watch: Option<Watch>,
}

thread_local! {
    static LISTS: RefCell<Vec<Rc<RefCell<List>>>> = const { RefCell::new(Vec::new()) };
    static NEXT: std::cell::Cell<u64> = const { std::cell::Cell::new(1) };
}

/// A listener's place: dropped, it hears no more (and the list's watch
/// ends with its last listener).
pub struct Listening {
    list: Weak<RefCell<List>>,
    id: u64,
}

impl Drop for Listening {
    fn drop(&mut self) {
        let Some(list) = self.list.upgrade() else { return };
        let empty = {
            let mut list = list.borrow_mut();
            list.listeners.retain(|(id, _)| *id != self.id);
            list.listeners.is_empty()
        };
        if empty {
            LISTS.with(|lists| lists.borrow_mut().retain(|l| !Rc::ptr_eq(l, &list)));
            // (Its watch ends as the list goes.)
            list.borrow_mut()._watch.take();
        }
    }
}

/// Hear `method`'s list on `rpc`: at once, if it's been heard already (the
/// whole list), then every change. One watch for every listener.
pub fn listen(rpc: &Rpc, method: &'static str, on_frame: impl Fn(&Frame) + 'static) -> Listening {
    let id = NEXT.with(|n| {
        let id = n.get();
        n.set(id + 1);
        id
    });
    let listener: Rc<Listener> = Rc::new(Box::new(on_frame));
    // (Lists on connections that ended go as they're passed.)
    LISTS.with(|lists| lists.borrow_mut().retain(|l| !l.borrow().rpc.is_closed()));
    let found = LISTS.with(|lists| lists.borrow().iter().find(|l| l.borrow().method == method && l.borrow().rpc.same(rpc)).cloned());
    let list = match found {
        Some(list) => {
            list.borrow_mut().listeners.push((id, listener.clone()));
            let key = list.borrow().key;
            let now = list.borrow().list.clone();
            if let Some(now) = now {
                listener(&Frame { list: &now, changes: None, key });
            }
            list
        }
        None => {
            let list = Rc::new(RefCell::new(List {
                rpc: rpc.clone(),
                method,
                key: key_of(method),
                list: None,
                listeners: vec![(id, listener)],
                _watch: None,
            }));
            let weak = Rc::downgrade(&list);
            let watch = rpc.watch(method, json!({ "changes": true }), move |item| {
                if let Some(list) = weak.upgrade() {
                    heard(&list, item);
                }
            });
            list.borrow_mut()._watch = Some(watch);
            LISTS.with(|lists| lists.borrow_mut().push(list.clone()));
            list
        }
    };
    Listening { list: Rc::downgrade(&list), id }
}

/// A frame from the engine: the list made from it, told to every listener.
fn heard(list: &Rc<RefCell<List>>, item: Json) {
    let key = list.borrow().key;
    let changes = match item {
        Json::Array(_) => {
            list.borrow_mut().list = Some(Rc::new(item));
            None
        }
        Json::Object(mut frame) => {
            let changes = Changes {
                changed: match frame.remove("changed") {
                    Some(Json::Array(rows)) => rows,
                    _ => Vec::new(),
                },
                removed: frame
                    .get("removed")
                    .and_then(Json::as_array)
                    .map(|keys| keys.iter().filter_map(|k| k.as_str().map(str::to_owned)).collect())
                    .unwrap_or_default(),
                reordered: frame.get("order").is_some(),
            };
            let mut l = list.borrow_mut();
            // (Not shared now: listeners only borrow it while they hear.)
            let Some(Json::Array(rows)) = l.list.as_mut().map(Rc::make_mut) else { return };
            apply_rows(rows, &changes, key);
            if let Some(order) = frame.get("order").and_then(Json::as_array) {
                let mut by: HashMap<String, Json> = HashMap::with_capacity(rows.len());
                for row in rows.drain(..) {
                    if let Some(k) = row[key].as_str().map(str::to_owned) {
                        by.insert(k, row);
                    }
                }
                *rows = order.iter().filter_map(|k| by.remove(k.as_str()?)).collect();
            }
            Some(changes)
        }
        _ => return,
    };
    // (Listeners may listen or stop as they hear: told from what's here now.)
    let (now, listeners) = {
        let l = list.borrow();
        (l.list.clone(), l.listeners.iter().map(|(_, f)| f.clone()).collect::<Vec<_>>())
    };
    let Some(now) = now else { return };
    let frame = Frame { list: &now, changes: changes.as_ref(), key };
    for listener in listeners {
        listener(&frame);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame_of(list: &Json, changes: Option<&Changes>) -> Frame<'static> {
        // (Leaked for the test's lifetime: a frame borrows what it's told.)
        let list: &'static Json = Box::leak(Box::new(list.clone()));
        let changes = changes.map(|c| &*Box::leak(Box::new(Changes { changed: c.changed.clone(), removed: c.removed.clone(), reordered: c.reordered })));
        Frame { list, changes, key: "id" }
    }

    /// A listener's copy, kept with the changes alone, is the list the
    /// engine's whole-list stream would have sent; a row not touched is the
    /// very row it was.
    #[test]
    fn a_copy_kept_from_changes_is_the_whole_list() {
        let before = json!([{ "id": "a", "n": 1 }, { "id": "b", "n": 1 }, { "id": "c", "n": 1 }]);
        let after = json!([{ "id": "a", "n": 1 }, { "id": "c", "n": 2 }, { "id": "d", "n": 0 }]);
        let changes = Changes { changed: vec![json!({ "id": "c", "n": 2 }), json!({ "id": "d", "n": 0 })], removed: vec!["b".into()], reordered: false };
        let frame = frame_of(&after, Some(&changes));
        let mut list = before.clone();
        frame.apply_to_list(&mut list);
        assert_eq!(list, after);
        let mut map = by_key(&before, "id");
        frame.apply_to_map(&mut map);
        assert_eq!(map, by_key(&after, "id"));
        assert_eq!(frame.touched(), Some(vec!["c", "d", "b"]));
        // Reordered, or the whole list: the list as it is.
        let reordered = Changes { changed: vec![], removed: vec![], reordered: true };
        let swapped = json!([{ "id": "c", "n": 2 }, { "id": "a", "n": 1 }, { "id": "d", "n": 0 }]);
        frame_of(&swapped, Some(&reordered)).apply_to_list(&mut list);
        assert_eq!(list, swapped);
        let whole = frame_of(&before, None);
        assert_eq!(whole.touched(), None);
        whole.apply_to_list(&mut list);
        assert_eq!(list, before);
    }
}
