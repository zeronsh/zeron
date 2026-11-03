//! The dev servers running in a session's project, for the panel's browser
//! start page (ui/screens/panel/browser.luau): the engine's `WatchPreviews`
//! stream for a chat (crates/engine rpc.rs; its proxy gives each a
//! `*.localhost` address), published as the shared state `previews`:
//!
//!   { chat, ready, error?, services = { { id, name, port, url, project,
//!     device, cwd, remote } } }
//!
//! One chat is watched at a time (the panel shows one session):
//!
//!   previews.watch { chatId }   watch this chat's (the last one's stream ends)
//!   previews.unwatch            none; `previews` is nil
//!
//! The watch is made again as the engine comes back. The engine answers a
//! failed subscription with no item (its error ends the stream unseen
//! here), so `ready` stays false until the first frame: the scripts give up
//! waiting on their own clock.

use serde_json::{Value as Json, json};
use std::{cell::RefCell, rc::Rc};
use zgx_script::{ScriptHost, Shared, Value};

use crate::engine::{
    self,
    rpc::{Rpc, Watch},
};

#[derive(Default)]
struct Previews {
    /// Each window's copy of the shared state.
    shared: RefCell<Vec<Shared>>,
    chat: RefCell<Option<String>>,
    watch: RefCell<Option<Watch>>,
    listening: std::cell::Cell<bool>,
}

thread_local! {
    static PREVIEWS: Rc<Previews> = Rc::new(Previews::default());
}

pub fn install(host: &ScriptHost) {
    let p = PREVIEWS.with(Rc::clone);
    p.shared.borrow_mut().push(host.shared("previews", Value::Nil));
    if !p.listening.replace(true) {
        let weak = Rc::downgrade(&p);
        engine::get().on_connect(move |rpc| {
            let Some(p) = weak.upgrade() else { return };
            let chat = p.chat.borrow().clone();
            if let Some(chat) = chat {
                p.start(rpc, chat);
            }
        });
    }
    let me = p.clone();
    host.action("previews.watch", move |arg| {
        let chat = arg.field("chatId").map(crate::auth::text).unwrap_or_default();
        if chat.is_empty() {
            return;
        }
        if me.chat.borrow().as_deref() == Some(chat.as_str()) && me.watch.borrow().is_some() {
            return;
        }
        *me.chat.borrow_mut() = Some(chat.clone());
        match engine::get().current() {
            Some(rpc) => me.start(&rpc, chat),
            None => me.publish(waiting(&chat)),
        }
    });
    let me = p.clone();
    host.action("previews.unwatch", move |_| {
        *me.chat.borrow_mut() = None;
        *me.watch.borrow_mut() = None;
        me.publish(Value::Nil);
    });
}

impl Previews {
    fn start(&self, rpc: &Rpc, chat: String) {
        self.publish(waiting(&chat));
        let weak = PREVIEWS.with(Rc::downgrade);
        let watched = chat.clone();
        let watch = rpc.watch("WatchPreviews", json!({ "chatId": chat }), move |item| {
            if let Some(p) = weak.upgrade() {
                // (A frame from a watch that was replaced: dropped with it.)
                if p.chat.borrow().as_deref() == Some(watched.as_str()) {
                    p.publish(snapshot(&watched, &item));
                }
            }
        });
        *self.watch.borrow_mut() = Some(watch);
    }

    fn publish(&self, value: Value) {
        for shared in self.shared.borrow().iter() {
            shared.set(value.clone());
        }
    }
}

fn waiting(chat: &str) -> Value {
    Value::record([
        ("chat", Value::from(chat)),
        ("ready", Value::Bool(false)),
        ("services", Value::List(Vec::<Value>::new().into())),
    ])
}

/// The engine's snapshot (zeron_proto::PreviewSnapshot) as the scripts read it.
fn snapshot(chat: &str, item: &Json) -> Value {
    let proxy_port = item["proxyPort"].as_u64().unwrap_or(0);
    let services: Vec<Value> = item["services"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|s| {
            let hostname = s["hostname"].as_str().unwrap_or_default();
            let port = s["port"].as_u64().unwrap_or(0);
            // (The service's address through the local proxy, as the shipping
            // app opens it: PreviewService::url.)
            let url = if proxy_port > 0 && !hostname.is_empty() {
                format!("http://{hostname}:{proxy_port}")
            } else {
                format!("http://localhost:{port}")
            };
            Value::record([
                ("id", Value::from(s["id"].as_str().unwrap_or_default())),
                ("name", Value::from(s["name"].as_str().unwrap_or_default())),
                ("port", Value::Num(port as f64)),
                ("url", Value::from(url.as_str())),
                ("project", Value::from(s["projectName"].as_str().unwrap_or_default())),
                ("device", Value::from(s["deviceName"].as_str().unwrap_or_default())),
                ("cwd", Value::from(s["cwd"].as_str().unwrap_or_default())),
            ])
        })
        .collect();
    let mut fields = vec![
        ("chat", Value::from(chat)),
        ("ready", Value::Bool(true)),
        ("remote", Value::Bool(item["remote"].as_bool().unwrap_or(false))),
        ("services", Value::List(services.into())),
    ];
    if let Some(error) = item["error"].as_str() {
        fields.push(("error", Value::from(error)));
    }
    if let Some(project) = item["projectName"].as_str() {
        fields.push(("project", Value::from(project)));
    }
    Value::record(fields)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_snapshot_names_each_service_through_the_proxy() {
        let item = json!({
            "proxyPort": 27800,
            "services": [{ "id": "a", "name": "web", "port": 3000, "hostname": "web.zeron.localhost", "projectName": "site", "deviceName": "mac", "cwd": "/w" }],
            "remote": false
        });
        let v = snapshot("c1", &item);
        let Value::List(list) = v.field("services").unwrap() else { panic!("services") };
        assert_eq!(list.len(), 1);
        assert_eq!(crate::auth::text(&list[0].field("url").unwrap()), "http://web.zeron.localhost:27800");
        assert!(v.field("ready").unwrap().truthy());
    }
}
