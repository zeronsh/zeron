//! A main chat's notes, for a plugin allowed "notes" (Zeron's own Notes,
//! plugins-builtin/notes): the session doc's synced `notes` text (crates/doc
//! notes.rs, docs/session-notes.md), read and written through this device's
//! own engine (`WatchSessionNotes`, `SetSessionNotes`), never the chat's
//! host, so they stay editable while the host is offline.
//!
//! The grant is narrow: a plugin reaches only the notes of the chat it's
//! shown in. That's the session page's chat, and only a main chat (a
//! top-level one, archived or not; never a side chat or a subagent). A chat
//! is opened to the plugin as the plugin asks for it while that chat's page
//! shows, and stays open to it for this run, so what was typed still lands
//! after the person moves on. No other chat's notes are listed, read or
//! written, and a plugin without "notes" hears nothing and writes nothing.
//!
//!   plugin.<id>.notes        (its own, read only) { chat?, notes = { [chat]
//!                            = report } }: `chat` the page's main chat,
//!                            whose notes it may open now; a report is
//!                            { text, hash, epoch, seq }, { unsupported =
//!                            true } (an engine from before notes), or
//!                            { error } (made again shortly)
//!   plugin.<id>.notes_watch  [chat, …]: the chats whose notes it shows now,
//!                            each watched once opened to it; the rest let go
//!   plugin.<id>.notes_set    { chatId, baseHash, text }: compare-and-set, a
//!                            promise of { kind = "applied", hash, epoch, seq }
//!                            | { kind = "conflict", current } | { kind =
//!                            "tooLarge", limit }; refused "unsupported" by an
//!                            engine from before notes, "not_allowed" for a
//!                            chat it wasn't opened to (or without "notes")
//!
//! Watches are made again as the engine comes back.

use std::{
    any::Any,
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    future::Future,
    pin::Pin,
    rc::{Rc, Weak},
    time::Duration,
};

use serde_json::{Map, Value as Json, json};
use zgx::reactive::{Effect, Runtime};
use zgx_script::{ScriptHost, Shared, Value};

use super::{json_value, value_json};
use crate::auth::text;

/// The permission.
pub const PERMISSION: &str = "notes";
/// The engine's own bound on what a write may grow the notes to (crates/doc
/// `NOTES_MAX_BYTES`).
pub const MAX_BYTES: usize = 256 * 1024;
/// What a write may carry at all: the engine takes a write that shrinks
/// notes past its bound (another device's paste), never one this big.
const MAX_WRITE: usize = 4 * MAX_BYTES;
/// How many chats a plugin shows at once.
const MAX_WANTED: usize = 32;
/// Wait before a watch that failed (not an old engine's refusal) is made again.
const RETRY: Duration = Duration::from_secs(2);

pub type Reply = Pin<Box<dyn Future<Output = Result<Json, String>>>>;

/// The engine, as the notes reach it (a test plays one).
pub trait Engine {
    /// `chat`'s notes watched: each report to `on_item`, an end before the
    /// handle drops to `on_end` (its words; "closed": the connection's).
    /// None: no engine now (watched again as one connects).
    fn watch(&self, chat: &str, on_item: Box<dyn FnMut(Json)>, on_end: Box<dyn FnOnce(String)>) -> Option<Box<dyn Any>>;
    /// `SetSessionNotes`.
    fn set(&self, params: Json) -> Reply;
    /// `work` after `wait`.
    fn later(&self, wait: Duration, work: Box<dyn FnOnce()>);
}

/// This device's engine.
pub struct Live {
    pub tasks: zgx_desktop::TaskSpawner,
}

impl Engine for Live {
    fn watch(&self, chat: &str, on_item: Box<dyn FnMut(Json)>, on_end: Box<dyn FnOnce(String)>) -> Option<Box<dyn Any>> {
        let rpc = crate::engine::get().current().filter(|r| !r.is_closed())?;
        Some(Box::new(rpc.watch_ending("WatchSessionNotes", json!({ "chatId": chat }), on_item, on_end)))
    }

    fn set(&self, params: Json) -> Reply {
        Box::pin(async move { crate::engine::get().call("SetSessionNotes", params).await })
    }

    fn later(&self, wait: Duration, work: Box<dyn FnOnce()>) {
        self.tasks.spawn(async move {
            zgx::timer::sleep(wait).await;
            work();
        });
    }
}

/// One plugin's notes.
#[derive(Default)]
struct Own {
    shared: Option<Shared>,
    /// The chats it shows now.
    wanted: BTreeSet<String>,
    /// The chats opened to it this run (each its page's main chat as it
    /// asked).
    opened: BTreeSet<String>,
    /// Each chat watched (wanted and opened), and its watch while there is
    /// one.
    watches: BTreeMap<String, Option<Box<dyn Any>>>,
    /// What each watch said last.
    reports: BTreeMap<String, Json>,
    /// What it was told last.
    told: Json,
}

pub struct Notes {
    host: ScriptHost,
    runtime: Runtime,
    engine: Rc<dyn Engine>,
    /// Whether plugin `id` is on and allowed "notes".
    may: Box<dyn Fn(&str) -> bool>,
    /// The session page's session (`zeron.page_session`) and the sessions
    /// (`sessions`: `chats`, `archived`), as the app writes them, followed
    /// from the first plugin on: made before then, they'd hold nil over the
    /// app's own first values.
    follow: RefCell<Option<(Shared, Shared, Effect)>>,
    plugins: RefCell<BTreeMap<String, Own>>,
    me: RefCell<Weak<Notes>>,
}

impl Notes {
    pub fn new(host: &ScriptHost, runtime: &Runtime, engine: Rc<dyn Engine>, may: impl Fn(&str) -> bool + 'static) -> Rc<Notes> {
        let notes = Rc::new(Notes {
            host: host.clone(),
            runtime: runtime.clone(),
            engine,
            may: Box::new(may),
            follow: RefCell::default(),
            plugins: RefCell::default(),
            me: RefCell::default(),
        });
        *notes.me.borrow_mut() = Rc::downgrade(&notes);
        notes
    }

    /// The page and the sessions, followed from now on: as either changes,
    /// what each plugin may open.
    fn follow(&self) {
        if self.follow.borrow().is_some() {
            return;
        }
        let page = self.host.shared("zeron.page_session", Value::Nil);
        let sessions = self.host.shared("sessions", Value::Nil);
        let weak = self.weak();
        let (p, s) = (page.clone(), sessions.clone());
        let effect = self.runtime.effect(move || {
            // (Read here, tracked; each plugin's turn reads them again.)
            p.signal().with(|_| ());
            s.signal().with(|_| ());
            if let Some(n) = weak.upgrade() {
                n.refresh_all();
            }
        });
        *self.follow.borrow_mut() = Some((page, sessions, effect));
    }

    /// The sessions as the app has them (nothing yet: null).
    fn sessions(&self) -> Json {
        self.follow.borrow().as_ref().map(|(_, s, _)| s.signal().with_untracked(value_json)).unwrap_or(Json::Null)
    }

    fn weak(&self) -> Weak<Notes> {
        self.me.borrow().clone()
    }

    /// Plugin `id`'s shared state and actions (`plugin.<id>.notes*`).
    pub fn expose(&self, host: &ScriptHost, id: &str) {
        self.follow();
        let shared = host.shared(&format!("plugin.{id}.notes"), Value::record(Vec::<(&str, Value)>::new()));
        self.plugins.borrow_mut().entry(id.to_string()).or_default().shared = Some(shared);
        let me = self.weak();
        let plugin = id.to_string();
        host.action(format!("plugin.{id}.notes_watch"), {
            let (me, plugin) = (me.clone(), plugin.clone());
            move |list: Value| {
                if let Some(n) = me.upgrade() {
                    n.want(&plugin, &value_json(&list));
                }
            }
        });
        host.async_action(format!("plugin.{id}.notes_set"), move |asked: Value| {
            let reply = match me.upgrade() {
                Some(n) => n.set(&plugin, &asked),
                None => Err("not_allowed: Zeron isn't ready".to_string()),
            };
            async move {
                let reply = reply?.await.map_err(said)?;
                Ok(json_value(&reply))
            }
        });
        self.refresh(id);
    }

    /// The chats plugin `id` shows now.
    pub(super) fn want(&self, id: &str, list: &Json) {
        let wanted: BTreeSet<String> = list
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Json::as_str)
            .filter(|c| !c.trim().is_empty())
            .take(MAX_WANTED)
            .map(str::to_string)
            .collect();
        if let Some(own) = self.plugins.borrow_mut().get_mut(id) {
            own.wanted = wanted;
        }
        self.refresh(id);
    }

    /// A write, if plugin `id` may make it: the engine's answer to come.
    pub(super) fn set(&self, id: &str, asked: &Value) -> Result<Reply, String> {
        let chat = asked.field("chatId").map(text).unwrap_or_default();
        let sessions = self.sessions();
        let opened = self.plugins.borrow().get(id).is_some_and(|own| own.opened.contains(&chat));
        if !(self.may)(id) {
            return Err("not_allowed: This plugin wasn't allowed your chats' notes".into());
        }
        if !opened || !is_main(&sessions, &chat) {
            return Err("not_allowed: Only the notes of the chat it's shown in".into());
        }
        let body = asked.field("text").map(text).unwrap_or_default();
        if body.len() > MAX_WRITE {
            return Ok(Box::pin(async { Ok(json!({ "kind": "tooLarge", "limit": MAX_BYTES })) }));
        }
        let base = asked.field("baseHash").map(text).unwrap_or_default();
        Ok(self.engine.set(json!({ "chatId": chat, "baseHash": base, "text": body })))
    }

    /// Every plugin's turn: what it may open, as the page or its grant changed.
    pub fn refresh_all(&self) {
        let ids: Vec<String> = self.plugins.borrow().keys().cloned().collect();
        for id in ids {
            self.refresh(&id);
        }
    }

    /// The page's chat, if it's a main chat.
    fn page_main(&self, sessions: &Json) -> Option<String> {
        let page = self.follow.borrow().as_ref().map(|(p, _, _)| p.signal().with_untracked(|v| if matches!(v, Value::Nil) { String::new() } else { text(v) })).unwrap_or_default();
        (!page.is_empty() && is_main(sessions, &page)).then_some(page)
    }

    /// Plugin `id`: the page's main chat opened to it if it asks for it, its
    /// watches made and let go to match, and told.
    fn refresh(&self, id: &str) {
        let allowed = (self.may)(id);
        let sessions = self.sessions();
        let page = if allowed { self.page_main(&sessions) } else { None };
        let (start, stopped) = {
            let mut plugins = self.plugins.borrow_mut();
            let Some(own) = plugins.get_mut(id) else { return };
            if allowed {
                if let Some(p) = &page
                    && own.wanted.contains(p)
                {
                    own.opened.insert(p.clone());
                }
                // A chat gone from your sessions (deleted) is open no more.
                if loaded(&sessions) {
                    own.opened.retain(|c| is_main(&sessions, c));
                }
            } else {
                own.opened.clear();
            }
            let should: BTreeSet<String> = own.wanted.intersection(&own.opened).cloned().collect();
            let mut stopped = Vec::new();
            own.watches.retain(|chat, watch| {
                let keep = should.contains(chat);
                if !keep {
                    stopped.push(watch.take());
                }
                keep
            });
            own.reports.retain(|chat, _| should.contains(chat));
            let mut start = Vec::new();
            for chat in &should {
                if !own.watches.contains_key(chat) {
                    own.watches.insert(chat.clone(), None);
                    start.push(chat.clone());
                }
            }
            (start, stopped)
        };
        // (Let go, and made, with no borrow held: a watch may answer at once.)
        drop(stopped);
        for chat in start {
            self.start(id, &chat);
        }
        self.tell(id, page.as_deref(), allowed);
    }

    /// Plugin `id` told what it may open and what its watches said, if
    /// that changed (with no borrow held: its effects run at once).
    fn tell(&self, id: &str, page: Option<&str>, allowed: bool) {
        let publish = {
            let mut plugins = self.plugins.borrow_mut();
            let Some(own) = plugins.get_mut(id) else { return };
            let now = if allowed {
                let notes: Map<String, Json> = own.reports.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                let mut now = json!({ "notes": notes });
                if let Some(p) = page {
                    now["chat"] = json!(p);
                }
                now
            } else {
                json!({})
            };
            if now == own.told {
                return;
            }
            own.told = now.clone();
            own.shared.clone().map(|s| (s, json_value(&now)))
        };
        if let Some((shared, value)) = publish {
            shared.set(value);
        }
    }

    fn tell_now(&self, id: &str) {
        let allowed = (self.may)(id);
        let sessions = self.sessions();
        let page = if allowed { self.page_main(&sessions) } else { None };
        self.tell(id, page.as_deref(), allowed);
    }

    /// Plugin `id`'s watch of `chat`'s notes, (re)made.
    fn start(&self, id: &str, chat: &str) {
        let on_item = {
            let (me, id, chat) = (self.weak(), id.to_string(), chat.to_string());
            Box::new(move |item: Json| {
                if let Some(n) = me.upgrade() {
                    n.report(&id, &chat, item);
                }
            })
        };
        let on_end = {
            let (me, id, chat) = (self.weak(), id.to_string(), chat.to_string());
            Box::new(move |why: String| {
                if let Some(n) = me.upgrade() {
                    n.ended(&id, &chat, &why);
                }
            })
        };
        let Some(watch) = self.engine.watch(chat, on_item, on_end) else { return };
        let mut plugins = self.plugins.borrow_mut();
        if let Some(slot) = plugins.get_mut(id).and_then(|own| own.watches.get_mut(chat))
            && slot.is_none()
        {
            *slot = Some(watch);
        }
    }

    fn report(&self, id: &str, chat: &str, item: Json) {
        {
            let mut plugins = self.plugins.borrow_mut();
            let Some(own) = plugins.get_mut(id) else { return };
            if !own.watches.contains_key(chat) {
                return;
            }
            own.reports.insert(chat.to_string(), item);
        }
        self.tell_now(id);
    }

    fn ended(&self, id: &str, chat: &str, why: &str) {
        // (Its handle is spent: dropped after the borrow.)
        let spent = {
            let mut plugins = self.plugins.borrow_mut();
            let Some(slot) = plugins.get_mut(id).and_then(|own| own.watches.get_mut(chat)) else { return };
            slot.take()
        };
        drop(spent);
        // The connection's end: made again as it comes back.
        if why == "closed" {
            return;
        }
        let unsupported = said(why.to_string()) == "unsupported";
        self.report(id, chat, if unsupported { json!({ "unsupported": true }) } else { json!({ "error": why }) });
        if !unsupported {
            let (me, id, chat) = (self.weak(), id.to_string(), chat.to_string());
            self.engine.later(
                RETRY,
                Box::new(move || {
                    if let Some(n) = me.upgrade() {
                        n.again(&id, &chat);
                    }
                }),
            );
        }
    }

    /// `chat`'s watch made again if it's still wanted and has none.
    fn again(&self, id: &str, chat: &str) {
        let idle = self.plugins.borrow().get(id).and_then(|own| own.watches.get(chat)).is_some_and(Option::is_none);
        if idle {
            self.start(id, chat);
        }
    }

    /// The engine came (back): every watch made again on it.
    pub fn reconnect(&self) {
        let all: Vec<(String, String)> = {
            let mut plugins = self.plugins.borrow_mut();
            let mut all = Vec::new();
            let mut spent = Vec::new();
            for (id, own) in plugins.iter_mut() {
                for (chat, watch) in own.watches.iter_mut() {
                    spent.push(watch.take());
                    all.push((id.clone(), chat.clone()));
                }
            }
            drop(plugins);
            drop(spent);
            all
        };
        for (id, chat) in all {
            self.start(&id, &chat);
        }
    }
}

/// Whether `sessions` (the sessions bridge's) has loaded its chats.
fn loaded(sessions: &Json) -> bool {
    sessions["chats"].is_array()
}

/// Whether `chat` is a main chat: a top-level one, in your chats or archived
/// (side chats are under `children`; subagents are no chats of their own).
pub fn is_main(sessions: &Json, chat: &str) -> bool {
    !chat.is_empty()
        && ["chats", "archived"]
            .iter()
            .any(|list| sessions[list].as_array().into_iter().flatten().any(|c| c["id"].as_str() == Some(chat)))
}

/// An engine from before notes: "unsupported"; else its own words.
fn said(error: String) -> String {
    let lower = error.to_ascii_lowercase();
    if lower.contains("unknown method") || lower.contains("unknownmethod") {
        "unsupported".into()
    } else {
        error
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_old_engine_is_unsupported() {
        assert_eq!(said("unknown method: SetSessionNotes".into()), "unsupported");
        assert_eq!(said("offline".into()), "offline");
    }

    #[test]
    fn only_top_level_chats_are_main() {
        let sessions = json!({
            "chats": [{ "id": "c1" }],
            "archived": [{ "id": "c0" }],
            "children": { "c1": [{ "id": "side-1", "parent": "c1" }] },
        });
        assert!(is_main(&sessions, "c1"));
        assert!(is_main(&sessions, "c0"));
        assert!(!is_main(&sessions, "side-1"));
        assert!(!is_main(&sessions, "c1--sub--1"));
        assert!(!is_main(&sessions, ""));
        assert!(!is_main(&json!(null), "c1"));
    }
}
