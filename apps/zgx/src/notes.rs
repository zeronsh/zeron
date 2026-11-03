//! A main chat's notes (ui/screens/panel/notes.luau): the session doc's
//! synced `notes` text, through this device's own engine (crates/doc
//! notes.rs, docs/session-notes.md), never the chat's host, so they stay
//! editable while the host is offline. Published as the shared state
//! `session_notes`, by chat:
//!
//!   { [chat] = { text, hash, epoch, seq } }   the notes as the engine
//!                                             last reported them
//!   { [chat] = { unsupported = true } }       an engine from before notes
//!   { [chat] = { error } }                    the watch failed (retried)
//!
//!   notes.watch { chatId }     watched while anyone asks (counted)
//!   notes.unwatch { chatId }   one asker fewer; at none, the watch ends
//!   notes.set { chatId, baseHash, text }  compare-and-set: { kind =
//!                              "applied", hash, epoch, seq } | { kind =
//!                              "conflict", current = { text, hash, epoch,
//!                              seq } } | { kind = "tooLarge", limit }; an
//!                              engine from before notes refuses it
//!                              "unsupported"
//!
//! Watches are made again as the engine comes back.

use std::{cell::RefCell, collections::BTreeMap, rc::Rc, time::Duration};

use serde_json::{Value as Json, json};
use zgx_script::{ScriptHost, Shared, Value};

use crate::{
    auth::text,
    engine::{
        self,
        rpc::{Rpc, Watch},
    },
};

/// Wait before a failed watch (not an old engine's refusal) is made again.
const RETRY: Duration = Duration::from_secs(2);

#[derive(Default)]
struct Notes {
    /// Each window's copy of the shared state.
    shared: RefCell<Vec<Shared>>,
    /// Who asks for each chat's notes, and its watch while there is one.
    watched: RefCell<BTreeMap<String, (usize, Option<Watch>)>>,
    /// What each chat's watch said last.
    reports: RefCell<BTreeMap<String, Json>>,
    listening: std::cell::Cell<bool>,
}

thread_local! {
    static NOTES: Rc<Notes> = Rc::new(Notes::default());
}

pub fn install(host: &ScriptHost) {
    let n = NOTES.with(Rc::clone);
    let current = n.value();
    n.shared.borrow_mut().push(host.shared("session_notes", current));
    if !n.listening.replace(true) {
        engine::get().on_connect(move |rpc| {
            NOTES.with(|n| {
                let chats: Vec<String> = n.watched.borrow().keys().cloned().collect();
                for chat in chats {
                    n.start_on(rpc, &chat);
                }
            });
        });
    }
    host.action("notes.watch", move |arg| {
        let Some(chat) = chat_of(&arg) else { return };
        NOTES.with(|n| {
            let first = {
                let mut watched = n.watched.borrow_mut();
                let entry = watched.entry(chat.clone()).or_insert((0, None));
                entry.0 += 1;
                entry.1.is_none()
            };
            if first {
                n.start(&chat);
            }
        });
    });
    host.action("notes.unwatch", move |arg| {
        let Some(chat) = chat_of(&arg) else { return };
        NOTES.with(|n| {
            let mut watched = n.watched.borrow_mut();
            if let Some(entry) = watched.get_mut(&chat) {
                entry.0 = entry.0.saturating_sub(1);
                if entry.0 == 0 {
                    watched.remove(&chat);
                }
            }
        });
    });
    host.async_action("notes.set", |arg| async move {
        let chat = chat_of(&arg).ok_or("no session")?;
        let base = arg.field("baseHash").map(text).unwrap_or_default();
        let body = arg.field("text").map(text).unwrap_or_default();
        let params = json!({ "chatId": chat, "baseHash": base, "text": body });
        let reply = engine::get().call("SetSessionNotes", params).await.map_err(said)?;
        Ok(value(&reply))
    });
}

impl Notes {
    /// (Re)makes `chat`'s watch on the connection there is, if any.
    fn start(&self, chat: &str) {
        if let Some(rpc) = engine::get().current().filter(|r| !r.is_closed()) {
            self.start_on(&rpc, chat);
        }
    }

    fn start_on(&self, rpc: &Rpc, chat: &str) {
        let on_item = {
            let chat = chat.to_owned();
            move |item: Json| {
                NOTES.with(|n| {
                    if n.watched.borrow().contains_key(&chat) {
                        n.report(&chat, item);
                    }
                });
            }
        };
        let on_end = {
            let chat = chat.to_owned();
            move |why: String| {
                NOTES.with(|n| {
                    if !n.watched.borrow().contains_key(&chat) {
                        return;
                    }
                    if let Some(entry) = n.watched.borrow_mut().get_mut(&chat) {
                        entry.1 = None;
                    }
                    // The connection's end: made again as it comes back.
                    if why == "closed" {
                        return;
                    }
                    let unsupported = said(why.clone()) == "unsupported";
                    n.report(&chat, if unsupported { json!({ "unsupported": true }) } else { json!({ "error": why }) });
                    if !unsupported {
                        let chat = chat.clone();
                        engine::get().spawn(async move {
                            zgx::timer::sleep(RETRY).await;
                            NOTES.with(|n| {
                                let idle = n.watched.borrow().get(&chat).is_some_and(|e| e.1.is_none());
                                if idle {
                                    n.start(&chat);
                                }
                            });
                        });
                    }
                });
            }
        };
        let watch = rpc.watch_ending("WatchSessionNotes", json!({ "chatId": chat }), on_item, on_end);
        if let Some(entry) = self.watched.borrow_mut().get_mut(chat) {
            entry.1 = Some(watch);
        }
    }

    fn report(&self, chat: &str, item: Json) {
        self.reports.borrow_mut().insert(chat.to_owned(), item);
        let value = self.value();
        for shared in self.shared.borrow().iter() {
            shared.set(value.clone());
        }
    }

    fn value(&self) -> Value {
        let reports = self.reports.borrow();
        let all: serde_json::Map<String, Json> = reports.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        value(&Json::Object(all))
    }
}

fn chat_of(arg: &Value) -> Option<String> {
    arg.field("chatId").map(text).filter(|s| !s.trim().is_empty())
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

fn value(json: &Json) -> Value {
    Value::from_json(&json.to_string()).unwrap_or(Value::Nil)
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
    fn a_report_reads_as_the_scripts_do() {
        let v = value(&json!({ "c1": { "text": "# a", "hash": "af63dc4c8601ec8c", "epoch": 9007199254740991u64, "seq": 2 } }));
        let c1 = v.field("c1").unwrap();
        assert_eq!(text(&c1.field("text").unwrap()), "# a");
        assert_eq!(text(&c1.field("hash").unwrap()), "af63dc4c8601ec8c");
    }
}
