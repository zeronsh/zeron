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
//!     devices = { { id, name, platform, here, online, seen?, version? } },
//!     projects = { { id, name, device, path, git, github?,
//!                    icon? = { emoji?, image?, hue? } } },
//!     chats = { { id, title, project?, device, branch?, cwd?, status?,
//!                 updated, created, harness?, agent?, model?,
//!                 running_subagents, side_chats, working_side_chats } },
//!     children = { [parent id] = { { id, parent, title, device, status?,
//!                 updated, created, harness?, agent? } } } (its side chats,
//!                 newest first),
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
//!   sessions.delete(id)            gone for good (its row; the engine keeps
//!                                  the conversation's document)
//!   sessions.collapse({ id, collapsed }) (an action)
//!                                  a section of yours folded, on every device

use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet, VecDeque},
    rc::Rc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde_json::{Value as Json, json};
use zgx_desktop::WindowContext;
use zgx_script::{ScriptHost, Shared, Value};

use crate::auth::text;
use crate::engine::{self, rpc::Watch};

/// A live state older than this belongs to an engine that has gone (the
/// shipping app's `SESSION_STALE_MS`).
const STALE_SECS: i64 = 45;
/// A device whose presence beat is older than this is offline (the shipping
/// app's `device_online`).
const ONLINE_SECS: i64 = 70;
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
    /// When states are looked at again: as the soonest one showing goes
    /// stale, so it stops showing. None while nothing would change.
    due: Cell<Option<i64>>,
}

thread_local! {
    static SESSIONS: Rc<Sessions> = Rc::new(Sessions::default());
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
            let mut watches = Vec::new();
            for (method, slot) in [
                ("WatchChats", Slot::Chats),
                ("WatchSpaces", Slot::Spaces),
                ("WatchDevices", Slot::Devices),
                ("WatchSessions", Slot::Live),
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
                if let Ok(answer) = rpc.call("LocalDevice", json!({})).await {
                    *me.here.borrow_mut() = answer["deviceId"].as_str().map(Into::into);
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
    let a = s.clone();
    host.action("sessions.seen", move |id| {
        let a = a.clone();
        let params = json!({ "op": "markChatSeen", "chatId": text(&id) });
        engine::get().spawn(async move {
            let _ = a.mutate(params).await;
        });
    });
    let a = s.clone();
    host.async_action("sessions.delete", move |id| {
        let a = a.clone();
        let params = json!({ "op": "deleteChat", "chatId": text(&id) });
        async move { a.mutate(params).await.map_err(String::from) }
    });
    let a = s.clone();
    host.action("sessions.collapse", move |args| {
        let id = args.field("id").map(text).unwrap_or_default();
        let collapsed = args.field("collapsed").is_some_and(Value::truthy);
        a.collapse(id, collapsed);
    });
    let a = s;
    host.action("sessions.places", move |wanted| a.want(places_of_value(&wanted)));
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
    fn heard(&self, slot: Slot, item: Json) {
        match slot {
            Slot::Chats => {
                *self.chats.borrow_mut() = item;
                self.heard.set(true);
            }
            Slot::Spaces => *self.spaces.borrow_mut() = item,
            Slot::Devices => *self.devices.borrow_mut() = item,
            Slot::Live => *self.live.borrow_mut() = item,
            Slot::Connectivity => *self.connectivity.borrow_mut() = item,
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
        let value = self.value();
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
        let stale = next_stale(&self.live.borrow(), now);
        let offline = next_offline(&self.devices.borrow(), self.here.borrow().as_deref(), now);
        let Some(at) = stale.into_iter().chain(offline).min() else { return };
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

    fn value(&self) -> Value {
        let now = now_seconds();
        let state = if self.heard.get() { "ready" } else { "loading" };
        let here = self.here.borrow().clone();
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
                if let Some(at) = seen {
                    fields.push(("seen", Value::Num(at as f64)));
                }
                // The Zeron it runs, as it last said (Settings → Devices).
                if let Some(version) = d["version"].as_str().filter(|v| !v.is_empty()) {
                    fields.push(("version", Value::from(version)));
                }
                Some(Value::record(fields))
            })
            .collect();
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
                    // The picture its people chose: `{ emoji?, image?, hue? }`,
                    // nil for its default tile.
                    ("icon", project_icon(&p["icon"])),
                ]))
            })
            .collect();
        let live = live_states(&self.live.borrow(), now);
        let all = self.chats.borrow();
        let all = all.as_array().map(Vec::as_slice).unwrap_or_default();
        let archived = |c: &&Json| c["archived"].as_bool().unwrap_or(false);
        // Side chats, under their parents (as the shipping app's lanes list
        // them): each as a row of its own, and how many there are and how
        // many work now, on the parent's row.
        let mut children: Vec<(String, Vec<Value>)> = Vec::new();
        let mut working_children: HashMap<String, u32> = HashMap::new();
        for c in all.iter().filter(|c| !archived(c)) {
            let (Some(id), Some(parent)) = (c["id"].as_str(), c["parentChatId"].as_str()) else { continue };
            let status = status(c, live.get(id));
            if status.as_deref() == Some("working") {
                *working_children.entry(parent.to_owned()).or_default() += 1;
            }
            let created = c["createdAt"].as_str().and_then(unix_seconds).unwrap_or(0);
            let updated = c["lastMessageAt"].as_str().and_then(unix_seconds).unwrap_or(created);
            let harness = c["config"]["harness"].as_str();
            let opt = |v: Option<&str>| v.filter(|s| !s.is_empty()).map_or(Value::Nil, Value::from);
            // Its title, else what it last said, else the shipping app's word.
            let title = c["title"]
                .as_str()
                .filter(|t| !t.trim().is_empty())
                .or(c["lastMessagePreview"].as_str().filter(|t| !t.trim().is_empty()))
                .unwrap_or("New side chat");
            let row = Value::record([
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
                    (
                        "agent_icon",
                        harness.and_then(crate::agents::registry_icon).map_or(Value::Nil, |i| Value::from(i.as_str())),
                    ),
            ]);
            match children.iter_mut().find(|(p, _)| p == parent) {
                Some((_, list)) => list.push(row),
                None => children.push((parent.to_owned(), vec![row])),
            }
        }
        // (Newest first, as the lanes card orders them.)
        let newest = |v: &Value| v.field("updated").and_then(Value::as_f64).unwrap_or(0.);
        for (_, list) in &mut children {
            list.sort_by(|a, b| newest(b).total_cmp(&newest(a)));
        }
        let side_chats = |id: &str| children.iter().find(|(p, _)| p == id).map_or(0, |(_, l)| l.len());
        // A top-level chat as scripts see it (side chats and a voice call's
        // orchestrator are another's).
        let row = |c: &Json| -> Option<Value> {
                let id = c["id"].as_str()?;
                if id.starts_with(ORCHESTRATOR) || !c["parentChatId"].is_null() {
                    return None;
                }
                let created = c["createdAt"].as_str().and_then(unix_seconds).unwrap_or(0);
                let updated = c["lastMessageAt"].as_str().and_then(unix_seconds).unwrap_or(created);
                let harness = c["config"]["harness"].as_str();
                let opt = |v: Option<&str>| v.filter(|s| !s.is_empty()).map_or(Value::Nil, Value::from);
                // A project that isn't known (deleted, or not yet synced)
                // leaves the chat among those with none.
                let project = c["spaceId"].as_str().filter(|s| known_spaces.contains(s));
                Some(Value::record([
                    ("id", Value::from(id)),
                    ("title", Value::from(chat_title(c["title"].as_str()))),
                    ("project", opt(project)),
                    ("device", Value::from(c["deviceId"].as_str().unwrap_or_default())),
                    ("branch", opt(c["branch"].as_str())),
                    ("cwd", opt(c["cwd"].as_str())),
                    // The branch its latest run started on, as the engine
                    // stamped it (`sourceContext`): what its pull request is
                    // looked up by, as the shipping app's sidebar does.
                    ("source_branch", opt(c["sourceContext"]["branch"].as_str())),
                    ("status", status(c, live.get(id)).map_or(Value::Nil, Value::from)),
                    ("updated", Value::from(updated as f64)),
                    ("created", Value::from(created as f64)),
                    ("harness", opt(harness)),
                    ("agent", harness.and_then(crate::agents::glyph).map_or(Value::Nil, |g| Value::from(g.as_str()))),
                    // An agent from the ACP registry: its icon, as listed.
                    (
                        "agent_icon",
                        harness.and_then(crate::agents::registry_icon).map_or(Value::Nil, |i| Value::from(i.as_str())),
                    ),
                    ("model", opt(c["config"]["model"].as_str())),
                    // (Its harness's own id for it: the menu's Copy.)
                    ("harness_session", opt(c["harnessSessionId"].as_str())),
                    // Its subagents streaming now (the sidebar's pill), and
                    // its side chats: how many, how many working.
                    ("running_subagents", Value::from(live.get(id).map_or(0, |l| l.subagents) as f64)),
                    ("side_chats", Value::from(side_chats(id) as f64)),
                    ("working_side_chats", Value::from(working_children.get(id).copied().unwrap_or(0) as f64)),
                ]))
        };
        let chats: Vec<Value> = all.iter().filter(|c| !archived(c)).filter_map(row).collect();
        // Archived, the most recently moved first (Settings → Archived).
        let mut gone: Vec<&Json> = all.iter().filter(archived).collect();
        let moved = |c: &Json| {
            c["lastMessageAt"].as_str().or(c["createdAt"].as_str()).and_then(unix_seconds).unwrap_or(0)
        };
        gone.sort_by_key(|c| std::cmp::Reverse(moved(c)));
        let archived: Vec<Value> = gone.into_iter().filter_map(row).collect();
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
        Value::record([
            ("state", Value::from(state)),
            ("here", here.as_deref().map_or(Value::Nil, Value::from)),
            ("devices", Value::List(devices.into())),
            ("projects", Value::List(projects.into())),
            ("chats", Value::List(chats.into())),
            (
                "children",
                Value::record(children.iter().map(|(parent, list)| (parent.as_str(), Value::List(list.clone().into())))),
            ),
            ("archived", Value::List(archived.into())),
            (
                "places",
                Value::record([("pinned", ids(&places.pinned)), ("sections", Value::List(sections.into()))]),
            ),
            ("places_ready", Value::Bool(self.places_ready.get())),
            ("connection", connection(&self.connectivity.borrow()).map_or(Value::Nil, Value::from)),
        ])
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
fn is_cloud(d: &Json) -> bool {
    let id = d["id"].as_str().unwrap_or_default();
    let platform = d["platform"].as_str().unwrap_or_default();
    platform == "cloud"
        || id.starts_with("cloud-")
        || d["capabilities"]
            .as_array()
            .is_some_and(|c| c.iter().any(|c| matches!(c.as_str(), Some("cloud-session" | "cloud-account"))))
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
fn cloud_words(error: &str) -> String {
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

/// What a chat's live state says, by chat: its status, whether its last turn
/// was interrupted. Several hosts may report one chat: an active state wins.
struct Live {
    status: String,
    interrupted: bool,
    /// Its subagents streaming now (the session row's `runningSubagents`),
    /// none once the row has gone quiet.
    subagents: u32,
}

/// When the next live state that shows (not idle, not yet stale) goes
/// stale, in Unix seconds: the next moment a look changes anything.
fn next_stale(value: &Json, now: i64) -> Option<i64> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter(|row| row["status"].as_str().is_some_and(|s| s != "idle"))
        .filter_map(|row| row["updatedAt"].as_str().and_then(unix_seconds))
        .map(|updated| updated + STALE_SECS + 1)
        .filter(|&at| at > now)
        .min()
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

fn live_states(value: &Json, now: i64) -> HashMap<String, Live> {
    let mut out: HashMap<String, Live> = HashMap::new();
    for row in value.as_array().into_iter().flatten() {
        let Some(chat) = row["chatId"].as_str() else { continue };
        let updated = row["updatedAt"].as_str().and_then(unix_seconds).unwrap_or(now);
        let mut status = row["status"].as_str().unwrap_or("idle").to_owned();
        // Gone quiet: its engine went with it.
        if now - updated > STALE_SECS {
            status = "idle".into();
        }
        let interrupted = row["lastTurnOutcome"]["status"].as_str() == Some("interrupted");
        let subagents = if now - updated > STALE_SECS { 0 } else { row["runningSubagents"].as_u64().unwrap_or(0) as u32 };
        let entry = out.entry(chat.to_owned()).or_insert(Live { status: "idle".into(), interrupted, subagents: 0 });
        entry.subagents = entry.subagents.max(subagents);
        if entry.status == "idle" {
            *entry = Live { status, interrupted, subagents: entry.subagents };
        }
    }
    out
}

/// The sidebar's state for a chat (proto's `chat_indicator`): working,
/// waiting on you, failed or finished since you last looked; else none.
fn status(chat: &Json, live: Option<&Live>) -> Option<&'static str> {
    let unseen = match (
        chat["lastMessageAt"].as_str().and_then(unix_seconds),
        chat["lastSeenAt"].as_str().and_then(unix_seconds),
    ) {
        (Some(message), Some(seen)) => message > seen,
        (Some(_), None) => true,
        _ => false,
    };
    match live.map(|l| l.status.as_str()) {
        Some("working") => Some("working"),
        Some("awaitingInput") => Some("input"),
        Some("errored") if unseen => Some("failed"),
        _ if unseen && !live.is_some_and(|l| l.interrupted) => Some("done"),
        _ => None,
    }
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

fn now_seconds() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// An RFC 3339 time as Unix seconds ("2026-10-31T21:27:23.486Z", or with an
/// offset).
fn unix_seconds(stamp: &str) -> Option<i64> {
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
    fn states_follow_the_shipping_app() {
        let now = 1_000_000;
        let stamp = |secs: i64| {
            let days = secs.div_euclid(86_400);
            // Only the time of day matters for these: the epoch's first day.
            assert_eq!(days, 11);
            format!("1970-01-12T{:02}:{:02}:{:02}Z", (secs % 86_400) / 3600, (secs % 3600) / 60, secs % 60)
        };
        let live = live_states(
            &json!([
                { "chatId": "w", "status": "working", "updatedAt": stamp(now - 5) },
                { "chatId": "old", "status": "working", "updatedAt": stamp(now - 120) },
                { "chatId": "q", "status": "awaitingInput", "updatedAt": stamp(now - 5) },
            ]),
            now,
        );
        let chat = json!({ "lastMessageAt": stamp(now - 10), "lastSeenAt": stamp(now - 20) });
        let seen = json!({ "lastMessageAt": stamp(now - 30), "lastSeenAt": stamp(now - 20) });
        assert_eq!(status(&seen, live.get("w")), Some("working"));
        assert_eq!(status(&seen, live.get("q")), Some("input"));
        assert_eq!(status(&seen, live.get("old")), None);
        assert_eq!(status(&chat, live.get("old")), Some("done"));
        assert_eq!(status(&seen, None), None);
    }

    #[test]
    fn states_are_looked_at_again_only_as_one_goes_stale() {
        let now = 1_000_000;
        let stamp = |secs: i64| format!("1970-01-12T{:02}:{:02}:{:02}Z", (secs % 86_400) / 3600, (secs % 3600) / 60, secs % 60);
        let rows = json!([
            { "chatId": "w", "status": "working", "updatedAt": stamp(now - 5) },
            { "chatId": "q", "status": "awaitingInput", "updatedAt": stamp(now - 30) },
            { "chatId": "old", "status": "working", "updatedAt": stamp(now - 120) },
            { "chatId": "i", "status": "idle", "updatedAt": stamp(now - 1) },
        ]);
        // The soonest: q, stale once it's 45 s old.
        assert_eq!(next_stale(&rows, now), Some(now - 30 + STALE_SECS + 1));
        // Nothing showing, or only stale and idle states: no look at all.
        let quiet = json!([
            { "chatId": "old", "status": "working", "updatedAt": stamp(now - 120) },
            { "chatId": "i", "status": "idle", "updatedAt": stamp(now - 1) },
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
        *s.chats.borrow_mut() = serde_json::json!([
            { "id": "p", "title": "Parent", "deviceId": "d", "createdAt": "2026-11-01T10:00:00Z", "config": { "harness": "claude-code" } },
            { "id": "a", "parentChatId": "p", "title": "", "lastMessagePreview": "Looking at the tests", "deviceId": "d",
              "createdAt": "2026-11-01T10:05:00Z", "lastMessageAt": "2026-11-01T10:06:00Z", "config": { "harness": "codex" } },
            { "id": "b", "parentChatId": "p", "deviceId": "d", "createdAt": "2026-11-01T10:07:00Z" },
            { "id": "gone", "parentChatId": "p", "archived": true, "deviceId": "d", "createdAt": "2026-11-01T10:08:00Z" },
        ]);
        *s.live.borrow_mut() = serde_json::json!([
            { "chatId": "p", "status": "working", "updatedAt": subagents_at, "runningSubagents": 2 },
            { "chatId": "a", "status": "working", "updatedAt": "2099-01-01T00:00:00Z" },
        ]);
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
        let p = chat(&v, "p");
        assert_eq!(p.field("side_chats").and_then(|n| n.as_f64()), Some(2.));
        assert_eq!(p.field("working_side_chats").and_then(|n| n.as_f64()), Some(1.));
        assert_eq!(p.field("running_subagents").and_then(|n| n.as_f64()), Some(2.));
    }

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
