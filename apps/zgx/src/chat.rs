//! A chat, for the chat page's scripts (ui/screens/chat): the session open
//! in a window, its transcript as it streams, the models to pick from, and
//! what can be asked of it. It speaks the engine's IPC as the shipping app's
//! chat view does (crates/ui: state.rs's transcript watch, composer.rs's
//! sends, queue.rs, pickers.rs).
//!
//! What scripts see, per window:
//!   `chat.session`   the open session: { id, state = "loading" | "ready" |
//!                    "missing" | "offline", title, project, device, harness,
//!                    model, effort, fast, access, plan, running, context,
//!                    context_tokens?, context_window?, ask?, queue, todos,
//!                    tasks }, nil for none
//!   `chat.messages`  its transcript as ds/transcript's messages, keyed by id
//!   `chat.draft`     the reply being written: { id, text }
//! and for every window, `chat.catalog`: { state, harnesses, models } as
//! ds.ModelPicker takes them, a model's `value` "<harness>/<model>".
//!
//! A reply streams as `append` frames, which carry only its new words: they
//! reach `chat.draft` alone, so a word costs a small state's write, not the
//! whole transcript's. Anything else (a tool, a new entry, the turn ending)
//! publishes `chat.messages` again whole, which `Shared::set` reconciles, so
//! the messages that didn't change keep their rows.
//!
//! Calls (each rejecting with an outcome word: offline, signed_out, missing,
//! failed):
//!   chat.open(id?) (an action)    watch that chat, the last one let go
//!   chat.catalog (an action)      the models, looked up again
//!   chat.send(req)                run a message, or queue it while a turn runs
//!   chat.interrupt(id)
//!   chat.respond({ id, request, decision = "once" | "always" | "deny" |
//!                  "answer", values?, text? })
//!   chat.configure({ id, model?, effort?, fast?, access?, plan? })
//!   chat.queue({ id, op = "remove" | "send" | "move", item, to? })
//!
//! The wire (crates/rpc, crates/doc): `WatchDocMessages` (a reset, then
//! deltas: upserts after an anchor, text appended to a part, removals, the
//! count to check against; byte lengths), `WatchSessions` and `WatchChats`
//! (whole lists), `WatchQueue`; runs, interrupts and answers through
//! `QueueCommand`; config through `Mutate setChatConfig`; uploads through
//! `UploadChunk` / `UploadCommit`. `ZERON_ZGX_MOCK=1` offers the engine's
//! mock agent, for trying a turn without a paid one.

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::{Rc, Weak},
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::{Value as Json, json};
use zgx_desktop::WindowContext;
use zgx_script::{ScriptHost, Shared, Value};

use crate::auth::text;
use crate::engine::{
    self,
    rpc::{Rpc, Watch},
};

type Outcome = &'static str;

/// A working session the engine hasn't touched for this long has died
/// with its host (proto/view.rs).
const STALE_SECONDS: i64 = 45;
/// How much of an upload goes in one chunk, in base64 characters (the
/// shipping app's).
const CHUNK: usize = 680_000;

thread_local! {
    static CHATS: Rc<Chats> = Rc::new(Chats::default());
}

/// What every window shares: the engine's chats and sessions, this device,
/// the agents and their models.
#[derive(Default)]
struct Chats {
    chats: RefCell<HashMap<String, Json>>,
    chats_heard: Cell<bool>,
    sessions: RefCell<HashMap<String, Json>>,
    here: RefCell<Option<String>>,
    harnesses: RefCell<Vec<Json>>,
    /// Models by harness, as the engine lists them.
    models: RefCell<HashMap<String, Vec<Json>>>,
    catalog: RefCell<Vec<Shared>>,
    catalog_state: Cell<CatalogState>,
    views: RefCell<Vec<Weak<View>>>,
    watches: RefCell<Vec<Watch>>,
    rpc: RefCell<Option<Rpc>>,
    /// What each chat waiting on its person asks: "approval" or
    /// "question", by chat (`chat.asks`, for the sidebar's rows).
    asks: RefCell<HashMap<String, &'static str>>,
    asks_shared: RefCell<Vec<Shared>>,
    /// A look at a waiting chat's latest entries, to tell which, until
    /// its first frame comes.
    ask_looks: RefCell<HashMap<String, Watch>>,
    /// The agents and models of the device the composer runs on, when that
    /// isn't this one (a remote session's, or Work in's for a new chat):
    /// `chat.catalog` is then its catalog, as the shipping app's pickers
    /// read their run device's (`list_harnesses(device)`).
    remote: RefCell<Option<Remote>>,
}

/// Another device's catalog, read through the engine (`targetDeviceId`).
struct Remote {
    device: String,
    harnesses: Vec<Json>,
    models: HashMap<String, Vec<Json>>,
    ready: bool,
}

#[derive(Clone, Copy, Default, PartialEq)]
enum CatalogState {
    #[default]
    Looking,
    Loading,
    Ready,
}

/// A window's open chat.
struct View {
    session: Shared,
    messages: Shared,
    draft: Shared,
    rpc: RefCell<Option<Rpc>>,
    open: RefCell<Option<Open>>,
    /// Moves on every open: an older watch's frames are dropped.
    generation: Cell<u64>,
    /// A finished subagent's transcript, read whole from its uploaded blob
    /// (a lane's): nothing to watch while it stands.
    snapshot: Cell<bool>,
}

/// A prompt sent and not yet in the transcript: shown under it meanwhile,
/// sending, or failed with what sends it again.
struct Pending {
    id: String,
    words: String,
    paths: Vec<String>,
    /// "sending" or "failed".
    waiting: &'static str,
    /// The command that sends it again, once it failed.
    retry: Option<Json>,
    /// When it was sent, as its hover strip says.
    at: Option<String>,
}

impl Pending {
    fn value(&self) -> Value {
        // (Its passages as chips, as the sent message shows them.)
        let (words, comments) = split_comments(&self.words);
        let (words, quotes) = split_quotes(&words);
        let (words, picks) = split_annotations(&words);
        Value::record([
            ("id", Value::from(self.id.as_str())),
            ("role", Value::from("user")),
            ("text", Value::from(words.as_str())),
            ("attachments", carried_value(&self.paths, &quotes, &picks, &comments)),
            ("waiting", Value::from(self.waiting)),
            ("at", self.at.as_deref().map_or(Value::Nil, Value::from)),
            ("copy", copy_value(&words)),
        ])
    }
}

/// An edit lease on a queued message (the engine's `BeginQueuedMessageEdit`).
#[derive(Clone)]
struct Lease {
    item: String,
    lease: String,
    hash: Json,
}

impl Open {
    /// Back a page in `request` (being answered here): its last page's
    /// answer taken off, and what was picked there. None on the first page,
    /// or for another chat's or request's question.
    fn back_a_page(&mut self, id: &str, request: &str) -> Option<Vec<String>> {
        if self.id != id || self.answering.as_deref() != Some(request) {
            return None;
        }
        self.answers.pop().map(|(_, labels)| labels)
    }
}

#[derive(Default)]
struct Open {
    id: String,
    /// The transcript's entries, as the engine's doc has them.
    entries: Vec<Json>,
    /// A reset has come (the newest entries, at least).
    heard: bool,
    context: Option<f64>,
    /// What the context holds as the agent last reported it: tokens in it
    /// and the window's size (either may be missing), for the ring's card.
    context_usage: (Option<f64>, Option<f64>),
    queue: Vec<Json>,
    /// Sent, not yet in the transcript: shown under it meanwhile.
    pending: Vec<Pending>,
    /// The session as last published, to republish the transcript when its
    /// turn starts or ends (the prompt's working line follows it).
    session_seen: Json,
    /// A question's answers so far, page by page, for the request asked.
    answers: Vec<(String, Vec<String>)>,
    answering: Option<String>,
    /// A queued message being edited here: its lease, held while the
    /// composer has its text.
    editing: Option<Lease>,
    watches: Vec<Watch>,
    /// Each entry as last published (nil: it shows nothing), by its id, and
    /// the model's name it was built with: a republish builds again only
    /// the entries a frame changed, and hands the rest over as the same
    /// values, so the script's copy (reconciled by id) does no work for
    /// them.
    built: HashMap<String, Option<Value>>,
    built_model: Option<Option<String>>,
}

pub fn install(_cx: &mut WindowContext, host: &ScriptHost) {
    let chats = CHATS.with(Rc::clone);
    chats.catalog.borrow_mut().push(host.shared("chat.catalog", chats.catalog_value()));
    chats.asks_shared.borrow_mut().push(host.shared("chat.asks", chats.asks_value()));
    let view = Rc::new(View {
        session: host.shared("chat.session", Value::Nil),
        messages: host.shared("chat.messages", Value::List(Rc::from([]))),
        draft: host.shared("chat.draft", draft_value("", "")),
        rpc: RefCell::default(),
        open: RefCell::default(),
        generation: Cell::new(0),
        snapshot: Cell::new(false),
    });
    chats.views.borrow_mut().push(Rc::downgrade(&view));
    if chats.views.borrow().len() == 1 {
        let weak = Rc::downgrade(&chats);
        engine::get().on_connect(move |rpc| {
            if let Some(chats) = weak.upgrade() {
                chats.connected(rpc);
            }
        });
    }
    let weak = Rc::downgrade(&view);
    engine::get().on_connect(move |rpc| {
        if let Some(view) = weak.upgrade() {
            *view.rpc.borrow_mut() = Some(rpc.clone());
            view.subscribe();
        }
    });

    let v = view.clone();
    host.action("chat.open", move |id| {
        let id = text(&id);
        v.open_chat((!id.is_empty()).then_some(id));
    });
    // A window's lanes: more chats watched beside the open one, each in a
    // slot of its own (a panel tab's side chat, or a subagent's doc), its
    // states `chat.lane.<slot>.session`, `.messages`, `.draft`, as the open
    // chat's are. `{ slot, id, frozen?, chat? }`; no id lets the slot's chat
    // go. `frozen`: a finished subagent's doc, read from its upload first.
    let lanes: Rc<RefCell<HashMap<String, Rc<View>>>> = Rc::default();
    {
        let (lanes, host) = (lanes.clone(), host.clone());
        host.clone().action("chat.open_lane", move |args| {
            let slot = args.field("slot").map(text).unwrap_or_default();
            if slot.is_empty() {
                return;
            }
            let id = args.field("id").map(text).filter(|s| !s.is_empty());
            let view = lanes.borrow_mut().entry(slot.clone()).or_insert_with(|| lane_view(&host, &slot)).clone();
            // A finished subagent: its uploaded transcript first.
            match (id, args.field("frozen").is_some_and(Value::truthy)) {
                (Some(id), true) => view.open_frozen(id, args.field("chat").map(text).filter(|s| !s.is_empty())),
                (id, _) => view.open_chat(id),
            }
        });
    }
    // The view a call is about: the lane that has its chat open, else the
    // window's open chat.
    let which = {
        let (main, lanes) = (view.clone(), lanes.clone());
        move |args: &Value| -> Rc<View> {
            let id = args.field("session").or_else(|| args.field("id")).map(text).unwrap_or_default();
            lanes.borrow().values().find(|l| l.open_id().as_deref() == Some(id.as_str())).cloned().unwrap_or_else(|| main.clone())
        }
    };
    let c = chats.clone();
    host.action("chat.catalog", move |_| {
        let c = c.clone();
        engine::get().spawn(async move { c.load_catalog(true).await });
    });
    // The composer's device: its catalog is `chat.catalog` from now on (nil
    // or this device: this one's).
    let c = chats.clone();
    host.action("chat.catalog_on", move |arg| {
        let device = arg.field("device").map(crate::auth::text).unwrap_or_default();
        c.catalog_on(&device);
    });
    let pick = which.clone();
    host.async_action("chat.send", move |req| {
        let v = pick(&req);
        async move { crate::plugins::preview::refuse("chat.send")?; v.send(req).await.map(|_| Value::Nil).map_err(String::from) }
    });
    host.async_action("chat.interrupt", move |id| async move {
        command(&text(&id), json!({ "kind": "interrupt" })).await?;
        Ok(Value::Nil)
    });
    // A background process of the chat's stopped, on its host (the shipping
    // app's `stop_background_task`): whether it's stopping.
    host.async_action("chat.stop_task", move |args| async move { stop_task(args).await.map_err(String::from) });
    let pick = which.clone();
    host.async_action("chat.respond", move |args| {
        let v = pick(&args);
        async move { crate::plugins::preview::refuse("chat.respond")?; v.respond(args).await.map(|_| Value::Nil).map_err(String::from) }
    });
    // A question's Back: its page before again, with what was picked there.
    let back = which.clone();
    host.async_action("chat.question_back", move |args| {
        let v = back(&args);
        async move { Ok::<Value, String>(v.question_back(&args)) }
    });
    let v = view.clone();
    host.async_action("chat.configure", move |args| {
        let v = v.clone();
        async move { v.configure(args).await.map(|_| Value::Nil).map_err(String::from) }
    });
    let v = view.clone();
    host.async_action("chat.queue", move |args| {
        let v = v.clone();
        async move { crate::plugins::preview::refuse("chat.queue")?; v.queue_op(args).await.map(|_| Value::Nil).map_err(String::from) }
    });
    let v = view.clone();
    host.async_action("chat.retry", move |args| {
        let v = v.clone();
        async move { v.retry(args).await.map(|_| Value::Nil).map_err(String::from) }
    });
    let v = view.clone();
    host.async_action("chat.queue_edit", move |args| {
        let v = v.clone();
        async move { v.begin_edit(args).await.map_err(String::from) }
    });
    let v = view.clone();
    host.async_action("chat.queue_finish", move |args| {
        let v = v.clone();
        async move { v.finish_edit(args).await.map(|_| Value::Nil).map_err(String::from) }
    });
    // A project's branches, from git where it is (the shipping app's
    // `ensure_refs`): `{ path, device? }`, the device only when it isn't this
    // one (the engine forwards it there).
    host.async_action("chat.refs", move |args| async move { refs(args).await.map_err(String::from) });
}

/// Where a subagent's uploaded transcript is: `{chat}/{doc}`, the chat it
/// was spawned in named by its doc (`{chat}--sub--{n}`; a copied spawn chip
/// keeps its first chat's), else the one it's shown under.
fn blob_ref(chat: Option<&str>, doc: &str) -> String {
    let source = doc.split_once("--sub--").map(|(source, _)| source).or(chat).unwrap_or(doc);
    format!("{source}/{doc}")
}

/// `FetchToolBlob`'s reply as a transcript's reset frame: its `text`, a
/// JSON list of entries.
fn snapshot_frame(reply: &Json) -> Option<Json> {
    let entries: Json = serde_json::from_str(reply.get("text")?.as_str()?).ok()?;
    entries.is_array().then(|| json!({ "reset": entries }))
}

/// A lane's view: its own states, heard as the window's open chat is (the
/// catalog, sessions and every connection reach it too).
fn lane_view(host: &ScriptHost, slot: &str) -> Rc<View> {
    let chats = CHATS.with(Rc::clone);
    let view = Rc::new(View {
        session: host.shared(&format!("chat.lane.{slot}.session"), Value::Nil),
        messages: host.shared(&format!("chat.lane.{slot}.messages"), Value::List(Rc::from([]))),
        draft: host.shared(&format!("chat.lane.{slot}.draft"), draft_value("", "")),
        rpc: RefCell::default(),
        open: RefCell::default(),
        generation: Cell::new(0),
        snapshot: Cell::new(false),
    });
    chats.views.borrow_mut().push(Rc::downgrade(&view));
    let weak = Rc::downgrade(&view);
    engine::get().on_connect(move |rpc| {
        if let Some(view) = weak.upgrade() {
            *view.rpc.borrow_mut() = Some(rpc.clone());
            view.subscribe();
        }
    });
    view
}

/// A project's refs, as the new chat's ref picker lists them: each branch,
/// whether it's the folder's current one, and the worktree it's checked out
/// in. The current first, then as git lists them.
async fn refs(args: Value) -> Result<Value, Outcome> {
    let field = |key: &str| args.field(key).map(text).filter(|s| !s.is_empty());
    let path = field("path").ok_or("missing")?;
    let mut params = json!({ "repoPath": path });
    if let Some(device) = field("device") {
        params["targetDeviceId"] = Json::from(device);
    }
    let found = engine::get().call("ListRefs", params).await.map_err(|e| engine::outcome(&e))?;
    Ok(refs_value(&found))
}

fn refs_value(found: &Json) -> Value {
    let mut rows: Vec<&Json> = found.as_array().into_iter().flatten().filter(|r| r["name"].is_string()).collect();
    rows.sort_by_key(|r| r["current"].as_bool() != Some(true));
    Value::List(
        rows.into_iter()
            .map(|r| {
                let mut fields = vec![
                    ("name", Value::from(r["name"].as_str().unwrap_or_default())),
                    ("current", Value::Bool(r["current"].as_bool() == Some(true))),
                ];
                if let Some(path) = r["worktreePath"].as_str() {
                    fields.push(("worktree", Value::from(path)));
                }
                Value::record(fields)
            })
            .collect::<Vec<_>>()
            .into(),
    )
}

// -- every window's -----------------------------------------------------------

impl Chats {
    fn connected(self: &Rc<Self>, rpc: &Rpc) {
        let mut watches = Vec::new();
        let weak = Rc::downgrade(self);
        watches.push(rpc.watch("WatchSessions", json!({}), move |item| {
            let Some(me) = weak.upgrade() else { return };
            let mut by = HashMap::new();
            for s in item.as_array().into_iter().flatten() {
                if let Some(id) = s["chatId"].as_str() {
                    by.insert(id.to_owned(), s.clone());
                }
            }
            *me.sessions.borrow_mut() = by;
            me.follow_asks();
            me.publish_views();
            // A turn starting or ending: the newest prompt's line follows.
            for view in me.views() {
                let changed = {
                    let mut slot = view.open.borrow_mut();
                    let Some(open) = slot.as_mut() else { continue };
                    let now = me.sessions.borrow().get(&open.id).cloned().unwrap_or(Json::Null);
                    let changed = now["status"] != open.session_seen["status"]
                        || now["startedAt"] != open.session_seen["startedAt"];
                    open.session_seen = now;
                    changed
                };
                if changed {
                    view.publish_messages();
                }
            }
        }));
        let weak = Rc::downgrade(self);
        watches.push(rpc.watch("WatchChats", json!({}), move |item| {
            let Some(me) = weak.upgrade() else { return };
            let mut by = HashMap::new();
            for c in item.as_array().into_iter().flatten() {
                if let Some(id) = c["id"].as_str() {
                    by.insert(id.to_owned(), c.clone());
                }
            }
            *me.chats.borrow_mut() = by;
            me.chats_heard.set(true);
            me.publish_views();
        }));
        *self.watches.borrow_mut() = watches;
        *self.rpc.borrow_mut() = Some(rpc.clone());
        self.ask_looks.borrow_mut().clear();
        let me = self.clone();
        let rpc = rpc.clone();
        engine::get().spawn(async move {
            if let Ok(answer) = rpc.call("LocalDevice", json!({})).await {
                *me.here.borrow_mut() = answer["deviceId"].as_str().map(Into::into);
            }
            // Every composer needs it: looked up once a connection is there.
            me.load_catalog(false).await;
        });
    }

    /// Keeps `chat.asks` to the chats waiting on their person: a new one
    /// is looked at (its latest entries) to tell an approval from a
    /// question; one no longer waiting goes.
    fn follow_asks(self: &Rc<Self>) {
        let waiting: Vec<String> = {
            let sessions = self.sessions.borrow();
            sessions
                .iter()
                .filter(|(_, s)| s["status"].as_str() == Some("awaitingInput") && !stale(&s["updatedAt"]))
                .map(|(id, _)| id.clone())
                .collect()
        };
        let mut changed = false;
        self.asks.borrow_mut().retain(|id, _| {
            let keep = waiting.contains(id);
            changed |= !keep;
            keep
        });
        self.ask_looks.borrow_mut().retain(|id, _| waiting.contains(id));
        let rpc = self.rpc.borrow().clone().filter(|r| !r.is_closed());
        for id in waiting {
            if self.asks.borrow().contains_key(&id) || self.ask_looks.borrow().contains_key(&id) {
                continue;
            }
            // The open chat's own transcript says it at once.
            let open = self.views().iter().find_map(|v| {
                let open = v.open.borrow();
                open.as_ref().filter(|o| o.id == id && o.heard).map(|o| ask_kind(&o.entries))
            });
            if let Some(kind) = open {
                self.asks.borrow_mut().insert(id, kind);
                changed = true;
                continue;
            }
            let Some(rpc) = &rpc else { continue };
            let weak = Rc::downgrade(self);
            let chat = id.clone();
            let look = rpc.watch(
                "WatchDocMessages",
                json!({ "chatId": id, "openingTail": true }),
                move |item| {
                    let Some(me) = weak.upgrade() else { return };
                    let Some(entries) = item["reset"].as_array() else { return };
                    let kind = ask_kind(entries);
                    me.asks.borrow_mut().insert(chat.clone(), kind);
                    // Seen: let it go (dropped once this callback returns).
                    let done = me.ask_looks.borrow_mut().remove(&chat);
                    me.publish_asks();
                    drop(done);
                },
            );
            self.ask_looks.borrow_mut().insert(id, look);
        }
        if changed {
            self.publish_asks();
        }
    }

    fn asks_value(&self) -> Value {
        Value::record(self.asks.borrow().iter().map(|(id, kind)| (id.as_str(), Value::from(*kind))))
    }

    fn publish_asks(&self) {
        let value = self.asks_value();
        for shared in self.asks_shared.borrow().iter() {
            shared.set(value.clone());
        }
    }

    fn views(&self) -> Vec<Rc<View>> {
        self.views.borrow().iter().filter_map(Weak::upgrade).collect()
    }

    fn publish_views(&self) {
        for view in self.views() {
            view.publish_session();
        }
    }

    /// The agents that can take a chat, and each one's models.
    async fn load_catalog(self: &Rc<Self>, again: bool) {
        if self.catalog_state.get() == CatalogState::Loading {
            return;
        }
        if self.catalog_state.get() == CatalogState::Ready && !again {
            return;
        }
        self.catalog_state.set(CatalogState::Loading);
        let mock = std::env::var("ZERON_ZGX_MOCK").is_ok_and(|v| v == "1");
        let list = match engine::get().call("ListHarnesses", json!({ "includeRegistry": true })).await {
            Ok(list) => list,
            Err(error) => {
                eprintln!("zeron-zgx: chat: the agents didn't list: {error}");
                self.catalog_state.set(CatalogState::Looking);
                return;
            }
        };
        let harnesses: Vec<Json> = list
            .as_array()
            .into_iter()
            .flatten()
            .filter(|h| {
                let id = h["id"].as_str().unwrap_or_default();
                if id == "mock" {
                    return mock;
                }
                h["installed"].as_bool() != Some(false) && h["enabled"].as_bool() != Some(false)
            })
            .cloned()
            .collect();
        // Each agent's models looked up at once, each shown as it comes (one
        // that has to start its agent takes seconds; one signed out fails).
        let ids: Vec<String> = harnesses
            .iter()
            .filter_map(|h| h["id"].as_str().map(Into::into))
            .collect();
        crate::agents::remember_icons(&harnesses);
        *self.harnesses.borrow_mut() = harnesses;
        if !again {
            self.models.borrow_mut().retain(|id, _| ids.contains(id));
        }
        let left = Rc::new(Cell::new(ids.len()));
        if ids.is_empty() {
            self.catalog_state.set(CatalogState::Ready);
            self.publish_catalog();
        }
        for id in ids {
            let (me, left) = (self.clone(), left.clone());
            engine::get().spawn(async move {
                match engine::get()
                    .call("ListModels", json!({ "harness": id, "force": again }))
                    .await
                {
                    Ok(list) => {
                        me.models
                            .borrow_mut()
                            .insert(id, list.as_array().cloned().unwrap_or_default());
                    }
                    Err(error) => eprintln!("zeron-zgx: chat: {id}'s models didn't list: {error}"),
                }
                left.set(left.get() - 1);
                if left.get() == 0 {
                    me.catalog_state.set(CatalogState::Ready);
                }
                me.publish_catalog();
            });
        }
    }

    fn publish_catalog(&self) {
        let value = self.catalog_value();
        for shared in self.catalog.borrow().iter() {
            shared.set(value.clone());
        }
        self.publish_views();
    }

    /// The composer now runs on `device` ("" or this one: here).
    fn catalog_on(self: &Rc<Self>, device: &str) {
        let here = self.here.borrow().clone();
        if device.is_empty() || here.as_deref() == Some(device) {
            if self.remote.borrow_mut().take().is_some() {
                self.publish_catalog();
            }
            return;
        }
        if self.remote.borrow().as_ref().is_some_and(|r| r.device == device) {
            return;
        }
        *self.remote.borrow_mut() = Some(Remote {
            device: device.to_owned(),
            harnesses: Vec::new(),
            models: HashMap::new(),
            ready: false,
        });
        self.publish_catalog();
        let (me, device) = (self.clone(), device.to_owned());
        engine::get().spawn(async move { me.load_remote(device).await });
    }

    /// `device`'s agents, then each one's models as they come, while the
    /// composer is still on it.
    async fn load_remote(self: &Rc<Self>, device: String) {
        let still = |me: &Chats| me.remote.borrow().as_ref().is_some_and(|r| r.device == device);
        let list = engine::get()
            .call("ListHarnesses", json!({ "targetDeviceId": device, "includeRegistry": true }))
            .await;
        if !still(self) {
            return;
        }
        let harnesses: Vec<Json> = match list {
            Ok(list) => list
                .as_array()
                .into_iter()
                .flatten()
                .filter(|h| {
                    h["id"].as_str() != Some("mock")
                        && h["installed"].as_bool() != Some(false)
                        && h["enabled"].as_bool() != Some(false)
                })
                .cloned()
                .collect(),
            Err(error) => {
                // (Unreachable: this device's catalog stands in, as before.)
                eprintln!("zeron-zgx: chat: {device}'s agents didn't list: {error}");
                self.remote.borrow_mut().take();
                self.publish_catalog();
                return;
            }
        };
        let ids: Vec<String> = harnesses.iter().filter_map(|h| h["id"].as_str().map(Into::into)).collect();
        if let Some(r) = self.remote.borrow_mut().as_mut() {
            crate::agents::remember_icons(&harnesses);
            r.harnesses = harnesses;
            r.ready = ids.is_empty();
        }
        self.publish_catalog();
        let left = Rc::new(Cell::new(ids.len()));
        for id in ids {
            let (me, left, device) = (self.clone(), left.clone(), device.clone());
            engine::get().spawn(async move {
                let list = engine::get()
                    .call("ListModels", json!({ "harness": id, "targetDeviceId": device }))
                    .await;
                left.set(left.get() - 1);
                {
                    let mut remote = me.remote.borrow_mut();
                    let Some(r) = remote.as_mut().filter(|r| r.device == device) else {
                        return;
                    };
                    match list {
                        Ok(list) => {
                            r.models.insert(id, list.as_array().cloned().unwrap_or_default());
                        }
                        Err(error) => eprintln!("zeron-zgx: chat: {id}'s models on {device} didn't list: {error}"),
                    }
                    r.ready = left.get() == 0;
                }
                me.publish_catalog();
            });
        }
    }

    fn catalog_value(&self) -> Value {
        if let Some(r) = self.remote.borrow().as_ref() {
            return catalog_of(&r.harnesses, &r.models, r.ready, Some(&r.device));
        }
        catalog_of(
            &self.harnesses.borrow(),
            &self.models.borrow(),
            self.catalog_state.get() == CatalogState::Ready,
            None,
        )
    }

    fn chat(&self, id: &str) -> Option<Json> {
        self.chats.borrow().get(id).cloned()
    }

    /// A chat this window just made (`sessions.create`), known before the
    /// engine's list says so: its first message, sent at once, goes where
    /// and how it was made (a side chat in its parent's folder, with its
    /// parent's agent and model). The engine's next list replaces it.
    fn made(&self, id: &str, row: Json) {
        self.chats.borrow_mut().entry(id.to_owned()).or_insert(row);
    }

    /// Whether the chat's turn runs: working or waiting on an answer, and
    /// heard from lately.
    fn running(&self, id: &str) -> bool {
        let sessions = self.sessions.borrow();
        let Some(s) = sessions.get(id) else { return false };
        let live = matches!(s["status"].as_str(), Some("working" | "awaitingInput"));
        live && !stale(&s["updatedAt"])
    }

    fn label_of(&self, harness: &str, model: &str) -> Option<String> {
        let find = |by: &HashMap<String, Vec<Json>>| {
            by.get(harness)?
                .iter()
                .find(|m| m["id"].as_str() == Some(model))
                .and_then(|m| m["label"].as_str().map(Into::into))
        };
        // (This device's, else the composer's other device's.)
        find(&self.models.borrow()).or_else(|| self.remote.borrow().as_ref().and_then(|r| find(&r.models)))
    }

    fn steers(&self, harness: &str) -> bool {
        self.harnesses
            .borrow()
            .iter()
            .find(|h| h["id"].as_str() == Some(harness))
            .is_some_and(|h| h["supportsSteering"].as_bool() == Some(true))
    }

    /// The device to name for a call routed to the chat's host, when that
    /// isn't this one.
    fn target(&self, chat: &Json) -> Option<String> {
        let device = chat["deviceId"].as_str()?;
        (self.here.borrow().as_deref() != Some(device)).then(|| device.to_owned())
    }
}

/// A chat just made here (`sessions.create`), as the engine's list will
/// have it: `params` is its `createChat` (`chatId`, `deviceId`, `spaceId`,
/// `config`, `cwd`, `branch`, `parentChatId`).
pub(crate) fn made(params: &Json) {
    let Some(id) = params["chatId"].as_str() else { return };
    let mut row = json!({ "id": id });
    for wire in ["deviceId", "spaceId", "config", "cwd", "branch", "parentChatId"] {
        if !params[wire].is_null() {
            row[wire] = params[wire].clone();
        }
    }
    CHATS.with(|c| c.made(id, row));
}

/// `chat.catalog`: the agents with models listed, each model as the picker
/// takes it; ready once any agent's models are in (more may follow), or all
/// have answered (`done`). (`device`: whose, when it isn't this one.)
fn catalog_of(listed: &[Json], by: &HashMap<String, Vec<Json>>, done: bool, device: Option<&str>) -> Value {
    let any = by.values().any(|l| !l.is_empty());
    let state = if any || done { "ready" } else { "looking" };
    let mut harnesses = Vec::new();
    let mut models = Vec::new();
    for h in listed {
        let id = h["id"].as_str().unwrap_or_default();
        let Some(list) = by.get(id).filter(|l| !l.is_empty()) else {
            continue;
        };
        harnesses.push(Value::record([
            ("id", Value::from(id)),
            ("label", Value::from(h["name"].as_str().unwrap_or(id))),
            ("mark", Value::from(mark(id).as_str())),
            // An agent added from the ACP registry draws its own icon.
            ("icon", h["icon"].as_str().map_or(Value::Nil, Value::from)),
        ]));
        for m in list {
            models.push(model_value(id, m));
        }
    }
    let mut fields = vec![
        ("state", Value::from(state)),
        ("harnesses", Value::List(harnesses.into())),
        ("models", Value::List(models.into())),
    ];
    if let Some(device) = device {
        fields.push(("device", Value::from(device)));
    }
    Value::record(fields)
}

/// The model picker's entry for a model: its value "<harness>/<id>", its
/// reasoning (low to high) and where it starts, whether it has Fast, and
/// its other options (Devin Fusion's lead and sidekick): each its id, its
/// name, its choices (id and name) and the one it starts on.
fn model_value(harness: &str, m: &Json) -> Value {
    let id = m["id"].as_str().unwrap_or_default();
    let levels: Vec<&str> = m["reasoningLevels"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Json::as_str)
        .collect();
    let efforts: Vec<Value> = levels
        .iter()
        .map(|l| Value::record([("value", Value::from(*l)), ("label", Value::from(effort_label(l)))]))
        .collect();
    let start = ["high", "medium"]
        .into_iter()
        .find(|l| levels.contains(l))
        .or(levels.last().copied());
    let fast = fast_option(m);
    let options: Vec<Value> = m["options"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|o| {
            // (Fast is `fast`, not an option of its own.)
            let id = o["id"].as_str().unwrap_or_default();
            !id.is_empty() && fast.as_ref().is_none_or(|(key, _, _)| key != id)
        })
        .filter_map(|o| {
            let id = o["id"].as_str()?;
            let choices: Vec<Value> = o["choices"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|c| {
                    let cid = c["id"].as_str()?;
                    Some(Value::record([
                        ("id", Value::from(cid)),
                        ("label", Value::from(c["label"].as_str().unwrap_or(cid))),
                    ]))
                })
                .collect();
            if choices.is_empty() {
                return None;
            }
            let default = o["defaultChoice"]
                .as_str()
                .filter(|d| o["choices"].as_array().is_some_and(|l| l.iter().any(|c| c["id"].as_str() == Some(d))))
                .map(str::to_owned)
                .or_else(|| o["choices"][0]["id"].as_str().map(str::to_owned))?;
            Some(Value::record([
                ("id", Value::from(id)),
                ("label", Value::from(o["label"].as_str().unwrap_or(id))),
                ("default", Value::from(default.as_str())),
                ("choices", Value::List(choices.into())),
            ]))
        })
        .collect();
    let mut fields = vec![
        ("value", Value::from(format!("{harness}/{id}").as_str())),
        ("label", Value::from(m["label"].as_str().unwrap_or(id))),
        ("harness", Value::from(harness)),
        ("fast", Value::Bool(fast.is_some())),
    ];
    if !efforts.is_empty() {
        fields.push(("efforts", Value::List(efforts.into())));
    }
    if let Some(start) = start {
        fields.push(("effort", Value::from(start)));
    }
    if !options.is_empty() {
        fields.push(("options", Value::List(options.into())));
    }
    Value::record(fields)
}

/// A model's Fast option: its id, the choice that turns it on, and the one
/// that turns it off (the shipping app's `fast_mode_values`: Claude's
/// `fastMode` on/off, Codex's `serviceTier` fast/default, Devin's `speed`
/// fast/standard).
fn fast_option(m: &Json) -> Option<(String, String, String)> {
    m["options"].as_array()?.iter().find_map(|o| {
        let id = o["id"].as_str()?;
        let choices: Vec<&str> = o["choices"].as_array()?.iter().filter_map(|c| c["id"].as_str()).collect();
        let has = |c: &str| choices.contains(&c);
        let on = if matches!(id, "fastMode" | "fast_mode") && has("on") {
            "on"
        } else if id == "fast" && has("true") {
            "true"
        } else if has("fast") {
            "fast"
        } else {
            return None;
        };
        let default = o["defaultChoice"].as_str().filter(|d| *d != on && has(d));
        let off = default.or_else(|| choices.iter().copied().find(|c| *c != on))?;
        Some((id.to_owned(), on.to_owned(), off.to_owned()))
    })
}

/// Whether a chat's model options have Fast on.
fn fast_in(options: &Json) -> bool {
    options["fastMode"].as_str() == Some("on")
        || options["fast_mode"].as_str() == Some("on")
        || options["fast"].as_str() == Some("true")
        || options["serviceTier"].as_str() == Some("fast")
        || options["speed"].as_str() == Some("fast")
}

/// A chat's model options as the composer's picks: each option's chosen
/// choice, by id (strings only; Fast's is `fast`, read by `fast_in`).
fn options_value(options: &Json) -> Value {
    let fields: Vec<(&str, Value)> = options
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(k, v)| v.as_str().map(|s| (k.as_str(), Value::from(s))))
        .collect();
    Value::record(fields)
}

fn effort_label(level: &str) -> &str {
    match level {
        "minimal" => "Minimal",
        "low" => "Low",
        "medium" => "Medium",
        "high" => "High",
        "xhigh" => "Extra high",
        "max" => "Max",
        "ultra" => "Ultra",
        "ultracode" => "Ultracode",
        "ultrathink" => "Ultrathink",
        other => other,
    }
}

/// ds.Mark's name for a harness. The mock agent (development only) borrows
/// Pi's.
fn mark(id: &str) -> String {
    crate::agents::glyph(id).unwrap_or_else(|| "pi".into())
}

// -- a window's open chat -----------------------------------------------------

impl View {
    fn open_chat(self: &Rc<Self>, id: Option<String>) {
        let same = self.open.borrow().as_ref().map(|o| o.id.clone()) == id;
        if same {
            return;
        }
        self.generation.set(self.generation.get() + 1);
        self.snapshot.set(false);
        *self.open.borrow_mut() = id.map(|id| Open { id, ..Open::default() });
        self.messages.set(Value::List(Rc::from([])));
        self.draft.set(draft_value("", ""));
        self.subscribe();
        self.publish_session();
        if let Some(id) = self.open_id() {
            // Its finished state seen, as the shipping app marks it.
            engine::get().spawn(async move {
                let _ = engine::get().call("Mutate", json!({ "op": "markChatSeen", "chatId": id })).await;
            });
        }
    }

    /// A finished subagent's doc `id`, under `chat`: its uploaded transcript
    /// first (the shipping app's `spawn_subagent_snapshot_fetch`), its live
    /// doc if that can't be had.
    fn open_frozen(self: &Rc<Self>, id: String, chat: Option<String>) {
        if self.open_id().as_deref() == Some(id.as_str()) {
            return;
        }
        self.generation.set(self.generation.get() + 1);
        self.snapshot.set(true);
        *self.open.borrow_mut() = Some(Open { id: id.clone(), ..Open::default() });
        self.messages.set(Value::List(Rc::from([])));
        self.draft.set(draft_value("", ""));
        self.publish_session();
        let generation = self.generation.get();
        let weak = Rc::downgrade(self);
        let blob = blob_ref(chat.as_deref(), &id);
        engine::get().spawn(async move {
            let reply = engine::get().call("FetchToolBlob", json!({ "blobRef": blob })).await;
            let Some(view) = weak.upgrade() else { return };
            if view.generation.get() != generation {
                return;
            }
            match reply.ok().as_ref().and_then(snapshot_frame) {
                Some(frame) => view.heard_frame(&frame),
                // (Not uploaded, or not readable: its live doc.)
                None => {
                    view.snapshot.set(false);
                    view.subscribe();
                }
            }
        });
    }

    fn open_id(&self) -> Option<String> {
        self.open.borrow().as_ref().map(|o| o.id.clone())
    }

    /// The open chat's watches, on the connection there is (again on each
    /// new one).
    fn subscribe(self: &Rc<Self>) {
        if self.snapshot.get() {
            return;
        }
        let Some(rpc) = self.rpc.borrow().clone().filter(|r| !r.is_closed()) else {
            return;
        };
        let Some(id) = self.open_id() else { return };
        let generation = self.generation.get();
        let weak = Rc::downgrade(self);
        let transcript = rpc.watch(
            "WatchDocMessages",
            json!({ "chatId": id, "openingTail": true }),
            move |item| {
                if let Some(view) = weak.upgrade()
                    && view.generation.get() == generation
                {
                    view.heard_frame(&item);
                }
            },
        );
        let weak = Rc::downgrade(self);
        let queue = rpc.watch("WatchQueue", json!({ "chatId": id }), move |item| {
            if let Some(view) = weak.upgrade()
                && view.generation.get() == generation
            {
                if let Some(open) = view.open.borrow_mut().as_mut() {
                    open.queue = item["items"].as_array().cloned().unwrap_or_default();
                }
                view.publish_session();
            }
        });
        if let Some(open) = self.open.borrow_mut().as_mut() {
            open.watches = vec![transcript, queue];
        }
    }

    /// Watches the open chat again from a reset (its copy went out of step).
    fn resubscribe(self: &Rc<Self>) {
        if let Some(open) = self.open.borrow_mut().as_mut() {
            open.watches.clear();
        }
        self.generation.set(self.generation.get() + 1);
        self.subscribe();
    }

    fn heard_frame(self: &Rc<Self>, frame: &Json) {
        let applied = {
            let mut slot = self.open.borrow_mut();
            let Some(open) = slot.as_mut() else { return };
            if let Some(usage) = frame.get("contextUsage").filter(|u| !u.is_null()) {
                let (tokens, window) = (usage["tokens"].as_f64(), usage["window"].as_f64());
                open.context = tokens.zip(window).filter(|(_, w)| *w > 0.).map(|(t, w)| (t / w).min(1.));
                open.context_usage = (tokens, window);
            }
            let result = apply(&mut open.entries, frame);
            // What it changed, to build again: every entry on a reset;
            // else the ones it touched.
            if frame.get("reset").is_some() || result.is_err() {
                open.built.clear();
            } else {
                for key in ["upsert", "append"] {
                    for item in frame[key].as_array().into_iter().flatten() {
                        let id = if key == "upsert" { &item["entry"]["id"] } else { &item["entry"] };
                        if let Some(id) = id.as_str() {
                            open.built.remove(id);
                        }
                    }
                }
                for id in frame["remove"].as_array().into_iter().flatten() {
                    if let Some(id) = id.as_str() {
                        open.built.remove(id);
                    }
                }
            }
            if result.is_ok() {
                open.heard = true;
                let ids: Vec<&str> = open.entries.iter().filter_map(|e| e["id"].as_str()).collect();
                open.pending.retain(|p| !ids.contains(&p.id.as_str()));
            }
            result
        };
        match applied {
            Ok(Change::Quiet) => {}
            Ok(Change::Words) => self.publish_draft(),
            Ok(Change::Rows) => {
                self.publish_messages();
                self.publish_session();
            }
            Err(()) => self.resubscribe(),
        }
    }

    /// The reply being written: the last entry's words while it streams.
    fn publish_draft(&self) {
        let value = {
            let open = self.open.borrow();
            match open.as_ref().and_then(|o| o.entries.last()) {
                Some(e) if e["status"].as_str() == Some("streaming") => {
                    // The words being written: the reply's last text part
                    // (its block, `part`), apart from the rest of the reply.
                    let tail = e["parts"].as_array().and_then(|parts| {
                        parts.iter().rev().find(|p| p["kind"].as_str() == Some("text"))
                    });
                    let id = e["id"].as_str().unwrap_or_default();
                    match tail {
                        // (`text`, the tail's words too: the page reads the
                        // block's, and of `text` only whether any are yet.)
                        Some(tail) => Value::record([
                            ("id", Value::from(id)),
                            ("text", Value::from(tail["text"].as_str().unwrap_or_default())),
                            ("part", Value::from(tail["id"].as_str().unwrap_or_default())),
                            ("part_text", Value::from(tail["text"].as_str().unwrap_or_default())),
                        ]),
                        None => draft_value(id, &entry_text(e)),
                    }
                }
                _ => draft_value("", ""),
            }
        };
        self.draft.set(value);
    }

    fn publish_messages(&self) {
        let chats = CHATS.with(Rc::clone);
        let list = {
            let mut slot = self.open.borrow_mut();
            let Some(open) = slot.as_mut() else { return };
            let chat = chats.chat(&open.id).unwrap_or(Json::Null);
            let harness = chat["config"]["harness"].as_str().unwrap_or("claude-code").to_owned();
            let model = chat["config"]["model"].as_str().map(|m| {
                chats.label_of(&harness, m).unwrap_or_else(|| m.to_owned())
            });
            // (Another model's name: every reply names it again.)
            if open.built_model.as_ref() != Some(&model) {
                open.built.clear();
                open.built_model = Some(model.clone());
            }
            // (Where a reply's visuals are read from: the chat's host, when
            // it isn't this device.)
            let device = chats.target(&chat);
            let Open { entries, built, .. } = open;
            let mut list: Vec<Value> = entries
                .iter()
                .filter_map(|e| {
                    let Some(id) = e["id"].as_str() else {
                        return message(e, model.as_deref());
                    };
                    built
                        .entry(id.to_owned())
                        .or_insert_with(|| match with_visual_device(e, device.as_deref()) {
                            Some(e) => message(&e, model.as_deref()),
                            None => message(e, model.as_deref()),
                        })
                        .clone()
                })
                .collect();
            let open = &*open;
            // The newest prompt, while its reply hasn't begun: sending,
            // then working, as the shipping app's working trailer says
            // (transcript.rs `sending_bridge`): sending while the send is
            // newer than the session's turn, working once the turn starts.
            if let Some(last) = open.entries.last()
                && last["role"].as_str() == Some("user")
                && let Some(Value::Map(_)) = list.last()
            {
                let sent = last["createdAt"].as_f64().unwrap_or(0.) / 1000.;
                let session = chats.sessions.borrow().get(&open.id).cloned().unwrap_or(Json::Null);
                let turn = seconds(&session["startedAt"]).map(|t| t as f64);
                let running = chats.running(&open.id);
                let fresh = now_seconds() as f64 - sent < SEND_GRACE_SECONDS;
                let waiting = match turn {
                    Some(turn) if running && turn + 1. >= sent => Some(("working", turn)),
                    _ if running || fresh => Some(("sending", sent)),
                    _ => None,
                };
                if let (Some((waiting, since)), Some(shown)) = (waiting, list.pop()) {
                    list.push(with_fields(
                        shown,
                        [("waiting", Value::from(waiting)), ("since", Value::Num(since))],
                    ));
                }
            }
            list.extend(open.pending.iter().map(Pending::value));
            list
        };
        // The list first: a reply that ended takes its words into its row
        // before the draft lets them go.
        self.messages.set(Value::List(list.into()));
        self.publish_draft();
    }

    fn publish_session(&self) {
        let chats = CHATS.with(Rc::clone);
        let value = {
            let open = self.open.borrow();
            let offline = self.rpc.borrow().as_ref().is_none_or(Rpc::is_closed);
            match open.as_ref() {
                None => Value::Nil,
                Some(open) => session_value(&chats, open, offline),
            }
        };
        self.session.set(value);
    }

    async fn send(self: &Rc<Self>, req: Value) -> Result<(), Outcome> {
        let chats = CHATS.with(Rc::clone);
        let id = req.field("session").map(text).filter(|s| !s.is_empty()).ok_or("missing")?;
        let fresh = req.field("fresh").is_some_and(Value::truthy);
        let chat = chats.chat(&id).unwrap_or(Json::Null);
        if chat.is_null() && !fresh {
            return Err("missing");
        }
        let target = chats.target(&chat);
        let words = req.field("text").map(text).unwrap_or_default();
        // Its id: the page's (it shows the prompt at once) or a new one.
        let message_id = req
            .field("message_id")
            .map(text)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(new_id);
        // Shown until the transcript has it (the page's own copy kept in
        // step as the list is published again meanwhile), sending.
        if self.open_id().as_deref() == Some(&id)
            && let Some(open) = self.open.borrow_mut().as_mut()
        {
            open.pending.push(Pending {
                id: message_id.clone(),
                words: words.clone(),
                paths: local_paths(&req),
                waiting: "sending",
                retry: None,
                at: stamp(now_seconds()),
            });
        }
        // What it carries, uploaded to the chat's host.
        let mut paths = Vec::new();
        let mut images = 0;
        // (Each file here, by what it was sent as: an Appshot's context names
        // its picture by the path the agent has.)
        let mut sent_as: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        for a in req.field("attachments").map(Value::list).unwrap_or_default() {
            let path = a.field("path").map(text).unwrap_or_default();
            let name = a.field("name").map(text).unwrap_or_else(|| path.clone());
            if path.is_empty() {
                continue;
            }
            if a.field("kind").map(text).as_deref() == Some("image") {
                images += 1;
            }
            // A file of the session's own project (the files pane's): on its
            // computer already, so it goes by its path there.
            if a.field("on_host").is_some_and(Value::truthy) {
                paths.push(path);
                continue;
            }
            match upload(&path, &name, target.as_deref()).await {
                Ok(at) => {
                    sent_as.insert(path.clone(), at.clone());
                    paths.push(at)
                }
                Err(why) => {
                    // Failed before it could go: the prompt says so.
                    self.mark_failed(&message_id, None);
                    return Err(why);
                }
            }
        }
        // Appshots: the apps they show, as untrusted observed content after
        // the words (src/appshots, the shipping app's `with_appshots`).
        let shots: Vec<crate::appshots::Context> = req
            .field("appshots")
            .map(Value::list)
            .unwrap_or_default()
            .iter()
            .map(|s| {
                let field = |k: &str| s.field(k).map(text).filter(|v| !v.is_empty());
                let path = field("path").unwrap_or_default();
                crate::appshots::Context {
                    app: field("app").unwrap_or_else(|| "Application".into()),
                    bundle: field("bundle"),
                    title: field("title"),
                    image: sent_as.get(&path).cloned().unwrap_or(path),
                    text: field("text").unwrap_or_default(),
                    truncated: s.field("truncated").is_some_and(Value::truthy),
                }
            })
            .collect();
        let words = crate::appshots::with_appshots(&words, &shots);
        let prompt = with_attachments(&words, &paths, images);
        let forget = |me: &Rc<Self>| {
            if let Some(open) = me.open.borrow_mut().as_mut() {
                open.pending.retain(|p| p.id != message_id);
            }
        };
        if !fresh && chats.running(&id) {
            // Queued: it shows in the queue, not the transcript.
            forget(self);
            self.publish_messages();
            let mut params = json!({ "chatId": id, "text": words, "holdForTurnEnd": true });
            if !paths.is_empty() {
                params["attachments"] = json!(paths);
            }
            return engine::get()
                .call("QueueMessage", params)
                .await
                .map(|_| ())
                .map_err(|e| engine::outcome(&e));
        }
        let config = merged_config(&chat["config"], &req);
        // A chat just made takes the config it was started with. (Never one
        // without an agent: the engine turns that away as bad params, and
        // the chat keeps the config it was made with: a side chat's, its
        // parent's.)
        if fresh && config["harness"].is_null() {
            eprintln!("zeron-zgx: chat: {id}: no agent to send with (the chat isn't known here yet)");
            self.mark_failed(&message_id, None);
            return Err("missing");
        }
        if fresh {
            engine::get()
                .call("Mutate", json!({ "op": "setChatConfig", "chatId": id, "config": config }))
                .await
                .map_err(|e| {
                    eprintln!("zeron-zgx: chat: a new chat's config wasn't set: {e}");
                    engine::outcome(&e)
                })?;
        }
        let field = |key: &str| req.field(key).map(text).filter(|s| !s.is_empty());
        let cwd = field("cwd")
            .or_else(|| chat["cwd"].as_str().map(Into::into))
            .unwrap_or_else(|| "~".into());
        let mut request = json!({
            "prompt": prompt,
            "harness": config["harness"],
            "model": config["model"],
            "reasoning": config["reasoning"],
            "modelOptions": config.get("modelOptions").cloned().unwrap_or(json!({})),
            "cwd": cwd,
            "sandbox": "workspace-write",
            "autoApprove": false,
            "resume": null,
        });
        if let Some(access) = config.get("access") {
            request["access"] = access.clone();
        }
        if let Some(plan) = config.get("plan") {
            request["plan"] = plan.clone();
        }
        if !paths.is_empty() {
            request["attachments"] = json!(paths);
        }
        if fresh && req.field("worktree").is_some_and(Value::truthy) {
            request["worktree"] = json!({
                "repoPath": cwd,
                "base": field("branch").unwrap_or_else(|| "HEAD".into()),
                "spaceId": field("project"),
            });
        }
        let run = json!({ "kind": "run", "messageId": message_id, "request": request });
        let sent = command(&id, run.clone()).await;
        if sent.is_err() {
            // Not delivered: the prompt stays, failed, to send again.
            self.mark_failed(&message_id, Some(run));
        } else if !fresh && runs_otherwise(&chat["config"], &config) {
            // Sent with another agent, model or effort than the chat had (the
            // composer's pick, kept as a draft until now): the chat takes
            // them, as the shipping app's next turn does, and its icon with
            // them. (Shown at once; the engine's word follows. A config that
            // doesn't stick leaves the run as sent.)
            if let Some(c) = chats.chats.borrow_mut().get_mut(&id) {
                c["config"] = config.clone();
            }
            chats.publish_views();
            if let Err(e) = engine::get()
                .call("Mutate", json!({ "op": "setChatConfig", "chatId": id, "config": config }))
                .await
            {
                eprintln!("zeron-zgx: chat: a sent pick didn't reach the chat's config: {e}");
            }
        }
        self.publish_messages();
        sent
    }

    /// A pending prompt that didn't go: failed, with the command that sends
    /// it again (none: it never got that far, and is sent again whole).
    fn mark_failed(&self, message_id: &str, retry: Option<Json>) {
        if let Some(open) = self.open.borrow_mut().as_mut()
            && let Some(p) = open.pending.iter_mut().find(|p| p.id == message_id)
        {
            p.waiting = "failed";
            p.retry = retry;
        }
        self.publish_messages();
    }

    /// A failed prompt sent again (its run as it was), sending meanwhile.
    async fn retry(self: &Rc<Self>, args: Value) -> Result<(), Outcome> {
        let id = args.field("id").map(text).unwrap_or_default();
        let message_id = args.field("message").map(text).unwrap_or_default();
        let run = {
            let mut slot = self.open.borrow_mut();
            let open = slot.as_mut().filter(|o| o.id == id).ok_or("missing")?;
            let p = open.pending.iter_mut().find(|p| p.id == message_id).ok_or("missing")?;
            p.waiting = "sending";
            p.retry.clone()
        };
        self.publish_messages();
        let Some(run) = run else {
            // It never got as far as a run (an upload failed): the composer
            // has the words; the row goes.
            if let Some(open) = self.open.borrow_mut().as_mut() {
                open.pending.retain(|p| p.id != message_id);
            }
            self.publish_messages();
            return Err("failed");
        };
        let sent = command(&id, run.clone()).await;
        if sent.is_err() {
            self.mark_failed(&message_id, Some(run));
        }
        self.publish_messages();
        sent
    }

    async fn respond(self: &Rc<Self>, args: Value) -> Result<(), Outcome> {
        let id = args.field("id").map(text).unwrap_or_default();
        let request = args.field("request").map(text).unwrap_or_default();
        let decision = args.field("decision").map(text).unwrap_or_default();
        let questions = {
            let open = self.open.borrow();
            let open = open.as_ref().filter(|o| o.id == id).ok_or("missing")?;
            asked(&open.entries)
                .filter(|p| p["requestId"].as_str() == Some(&request))
                .map(|p| p["questions"].as_array().cloned().unwrap_or_default())
                .ok_or("missing")?
        };
        let answers: Vec<Json> = match decision.as_str() {
            // The engine allows once: Yes is all it takes.
            "once" | "always" => single(&questions, "Yes"),
            "deny" if is_approval(&questions) => single(&questions, "No"),
            "deny" => Vec::new(),
            _ => {
                let mut labels: Vec<String> = args
                    .field("values")
                    .map(Value::list)
                    .unwrap_or_default()
                    .iter()
                    .map(text)
                    .collect();
                let typed = args.field("text").map(text).unwrap_or_default();
                if labels.is_empty() && !typed.trim().is_empty() {
                    labels.push(typed.trim().to_owned());
                }
                // A page answered: the next one, until the last.
                let done = {
                    let mut slot = self.open.borrow_mut();
                    let open = slot.as_mut().ok_or("missing")?;
                    if open.answering.as_deref() != Some(&request) {
                        open.answering = Some(request.clone());
                        open.answers.clear();
                    }
                    let page = open.answers.len();
                    let qid = questions.get(page).and_then(|q| q["id"].as_str()).unwrap_or_default();
                    open.answers.push((qid.to_owned(), labels));
                    open.answers.len() >= questions.len()
                };
                if !done {
                    self.publish_session();
                    return Ok(());
                }
                let open = self.open.borrow();
                open.as_ref()
                    .map(|o| {
                        o.answers
                            .iter()
                            .map(|(q, labels)| json!({ "questionId": q, "labels": labels }))
                            .collect()
                    })
                    .unwrap_or_default()
            }
        };
        let result = command(
            &id,
            json!({ "kind": "respondInput", "requestId": request, "answers": answers }),
        )
        .await;
        if let Some(open) = self.open.borrow_mut().as_mut() {
            open.answers.clear();
            open.answering = None;
        }
        self.publish_session();
        result
    }

    /// Back a page in the question being answered (the shipping app's
    /// wizard `back`): its last page's answer taken off, so that page shows
    /// again; what was picked there (its labels), for the composer to show
    /// chosen. Nil on the first page.
    fn question_back(&self, args: &Value) -> Value {
        let id = args.field("id").map(text).unwrap_or_default();
        let request = args.field("request").map(text).unwrap_or_default();
        let popped = self.open.borrow_mut().as_mut().and_then(|o| o.back_a_page(&id, &request));
        let Some(labels) = popped else {
            return Value::Nil;
        };
        self.publish_session();
        Value::List(labels.iter().map(|l| Value::from(l.as_str())).collect::<Vec<_>>().into())
    }

    async fn configure(self: &Rc<Self>, args: Value) -> Result<(), Outcome> {
        let chats = CHATS.with(Rc::clone);
        let id = args.field("id").map(text).unwrap_or_default();
        let chat = chats.chat(&id).ok_or("missing")?;
        let config = merged_config(&chat["config"], &args);
        // Shown at once; the engine's word follows.
        if let Some(c) = chats.chats.borrow_mut().get_mut(&id) {
            c["config"] = config.clone();
        }
        chats.publish_views();
        engine::get()
            .call("Mutate", json!({ "op": "setChatConfig", "chatId": id, "config": config }))
            .await
            .map(|_| ())
            .map_err(|e| engine::outcome(&e))
    }

    /// A queued message's text, taken to be edited here: the lease asked
    /// for, and kept (renewed before it lapses) until the edit finishes.
    async fn begin_edit(self: &Rc<Self>, args: Value) -> Result<Value, Outcome> {
        let chats = CHATS.with(Rc::clone);
        let id = args.field("id").map(text).unwrap_or_default();
        let item = args.field("item").map(text).unwrap_or_default();
        let chat = chats.chat(&id).ok_or("missing")?;
        let mut params = json!({
            "chatId": id,
            "id": item,
            "editorDeviceId": chats.here.borrow().clone(),
            "editorInstanceId": instance_id(),
        });
        if let Some(t) = chats.target(&chat) {
            params["targetDeviceId"] = json!(t);
        }
        let answer = engine::get()
            .call("BeginQueuedMessageEdit", params)
            .await
            .map_err(|e| engine::outcome(&e))?;
        match answer["outcome"].as_str() {
            Some("acquired") => {}
            Some("locked") => return Err("locked"),
            _ => return Err("missing"),
        }
        let lease = Lease {
            item: item.clone(),
            lease: answer["leaseId"].as_str().unwrap_or_default().to_owned(),
            hash: answer["baseTextHash"].clone(),
        };
        if let Some(open) = self.open.borrow_mut().as_mut().filter(|o| o.id == id) {
            open.editing = Some(lease.clone());
        }
        self.publish_session();
        self.keep_lease(id, lease, answer["expiresAtMs"].as_f64());
        Ok(Value::record([("text", Value::from(answer["text"].as_str().unwrap_or_default()))]))
    }

    /// Renews the lease halfway to each expiry, while it is still held.
    fn keep_lease(self: &Rc<Self>, chat: String, lease: Lease, expires: Option<f64>) {
        let weak = Rc::downgrade(self);
        engine::get().spawn(async move {
            let mut expires = expires;
            loop {
                let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0., |d| d.as_millis() as f64);
                let wait = expires.map_or(10_000., |at| ((at - now) / 2.).clamp(1_000., 30_000.));
                zgx::timer::sleep(std::time::Duration::from_millis(wait as u64)).await;
                let Some(me) = weak.upgrade() else { return };
                let held = me
                    .open
                    .borrow()
                    .as_ref()
                    .and_then(|o| o.editing.as_ref())
                    .is_some_and(|e| e.lease == lease.lease);
                if !held {
                    return;
                }
                let answer = engine::get()
                    .call(
                        "RenewQueuedMessageEdit",
                        json!({ "chatId": chat, "id": lease.item, "leaseId": lease.lease }),
                    )
                    .await;
                match answer {
                    Ok(a) if a["outcome"].as_str() == Some("renewed") => expires = a["expiresAtMs"].as_f64(),
                    _ => {
                        // Lost (taken elsewhere, or the message went): the
                        // composer keeps the words; saving says so.
                        return;
                    }
                }
            }
        });
    }

    /// The edit ended: its words saved over the queued message ("commit"),
    /// or the message left as it was ("cancel").
    async fn finish_edit(self: &Rc<Self>, args: Value) -> Result<(), Outcome> {
        let chats = CHATS.with(Rc::clone);
        let id = args.field("id").map(text).unwrap_or_default();
        let action = args.field("action").map(text).unwrap_or_else(|| "cancel".into());
        let lease = self
            .open
            .borrow_mut()
            .as_mut()
            .filter(|o| o.id == id)
            .and_then(|o| o.editing.take())
            .ok_or("missing")?;
        self.publish_session();
        let chat = chats.chat(&id).ok_or("missing")?;
        let mut params = json!({
            "chatId": id,
            "id": lease.item,
            "leaseId": lease.lease,
            "action": action,
        });
        if action == "commit" {
            params["text"] = json!(args.field("text").map(text).unwrap_or_default());
            params["expectedTextHash"] = lease.hash.clone();
        }
        if let Some(t) = chats.target(&chat) {
            params["targetDeviceId"] = json!(t);
        }
        let answer = engine::get()
            .call("FinishQueuedMessageEdit", params)
            .await
            .map_err(|e| engine::outcome(&e))?;
        match answer["outcome"].as_str() {
            Some("committed" | "cancelled" | "discarded" | "released") => Ok(()),
            Some("conflict") => Err("conflict"),
            _ => Err("missing"),
        }
    }

    async fn queue_op(self: &Rc<Self>, args: Value) -> Result<(), Outcome> {
        let chats = CHATS.with(Rc::clone);
        let id = args.field("id").map(text).unwrap_or_default();
        let item = args.field("item").map(text).unwrap_or_default();
        let chat = chats.chat(&id).ok_or("missing")?;
        let mut params = json!({ "chatId": id, "id": item });
        let target = chats.target(&chat);
        let routed = |mut p: Json| {
            if let Some(t) = &target {
                p["targetDeviceId"] = json!(t);
            }
            p
        };
        let (method, params) = match args.field("op").map(text).as_deref() {
            Some("remove") => ("RemoveQueuedMessage", routed(params)),
            Some("send") => {
                let harness = chat["config"]["harness"].as_str().unwrap_or_default();
                let method = if chats.running(&id) && chats.steers(harness) {
                    "SteerQueuedMessageNow"
                } else {
                    "SendQueuedMessageNow"
                };
                (method, routed(params))
            }
            Some("move") => {
                let to = args.field("to").and_then(Value::as_f64).unwrap_or(1.);
                params["toIndex"] = json!((to as i64 - 1).max(0));
                ("MoveQueuedMessage", params)
            }
            _ => return Err("failed"),
        };
        engine::get()
            .call(method, params)
            .await
            .map(|_| ())
            .map_err(|e| engine::outcome(&e))
    }
}

/// A command for a chat (`QueueCommand`): a run, an interrupt, an answer.
/// `StopBackgroundTask` for `{ session, task }`, routed to the chat's host.
async fn stop_task(args: Value) -> Result<Value, Outcome> {
    let id = args.field("session").map(text).unwrap_or_default();
    let task = args.field("task").map(text).unwrap_or_default();
    if id.is_empty() || task.is_empty() {
        return Err("missing");
    }
    let chats = CHATS.with(Rc::clone);
    let mut params = json!({ "chatId": id, "taskId": task });
    if let Some(t) = chats.chat(&id).and_then(|c| chats.target(&c)) {
        params["targetDeviceId"] = json!(t);
    }
    let answer = engine::get()
        .call("StopBackgroundTask", params)
        .await
        .map_err(|e| engine::outcome(&e))?;
    Ok(Value::Bool(answer["stopping"].as_bool() == Some(true)))
}

async fn command(chat: &str, command: Json) -> Result<(), Outcome> {
    engine::get()
        .call("QueueCommand", json!({ "chatId": chat, "command": command }))
        .await
        .map(|_| ())
        .map_err(|e| {
            eprintln!("zeron-zgx: chat: {} for {chat} wasn't taken: {e}", command["kind"]);
            engine::outcome(&e)
        })
}

/// A file uploaded to the device the chat runs on: its path there.
/// The most a file attached may weigh (the shipping app's
/// `MAX_ATTACHMENT_BYTES`).
const MAX_ATTACHMENT_BYTES: u64 = 24 * 1024 * 1024;

/// `work` on a thread of its own, its answer awaited here (None only if it
/// panicked): a file read and encoded off the window's thread.
async fn off_thread<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (tx, mut rx) = zgx::task::latest();
    std::thread::spawn(move || {
        tx.send(work());
    });
    rx.recv().await
}

async fn upload(path: &str, name: &str, target: Option<&str>) -> Result<String, Outcome> {
    // Read and encoded off the window's thread, and no larger than the cap.
    let owned = path.to_owned();
    let encoded = off_thread(move || -> Result<String, Outcome> {
        let meta = std::fs::metadata(&owned).map_err(|_| "failed")?;
        if !meta.is_file() {
            return Err("failed");
        }
        if meta.len() > MAX_ATTACHMENT_BYTES {
            return Err("too_large");
        }
        let bytes = std::fs::read(&owned).map_err(|_| "failed")?;
        Ok(base64(&bytes))
    })
    .await
    .ok_or("failed")??;
    let upload_id = new_id();
    let routed = |mut p: Json| {
        if let Some(t) = target {
            p["targetDeviceId"] = json!(t);
        }
        p
    };
    let mut seq = 0;
    let mut at = 0;
    while at < encoded.len() || seq == 0 {
        let end = (at + CHUNK).min(encoded.len());
        engine::get()
            .call(
                "UploadChunk",
                routed(json!({ "uploadId": upload_id, "seq": seq, "data": &encoded[at..end] })),
            )
            .await
            .map_err(|e| engine::outcome(&e))?;
        seq += 1;
        at = end;
    }
    let file_name = std::path::Path::new(name)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(name);
    let committed = engine::get()
        .call("UploadCommit", routed(json!({ "uploadId": upload_id, "fileName": file_name })))
        .await
        .map_err(|e| engine::outcome(&e))?;
    committed["path"].as_str().map(Into::into).ok_or("failed")
}

/// The chat's config with what the script changed: a model as
/// "<harness>/<model>", its effort, Fast, the access mode and plan mode.
/// Whether a run's config picks another agent, model, effort or model option
/// than the chat's.
fn runs_otherwise(chat: &Json, run: &Json) -> bool {
    ["harness", "model", "reasoning", "modelOptions"].iter().any(|k| {
        let a = chat.get(*k).filter(|v| !v.is_null());
        let b = run.get(*k).filter(|v| !v.is_null() && **v != json!({}));
        let a = a.filter(|v| **v != json!({}));
        a != b
    })
}

fn merged_config(config: &Json, args: &Value) -> Json {
    let mut c = if config.is_object() { config.clone() } else { json!({}) };
    if c.get("sandbox").is_none() {
        c["sandbox"] = json!("workspace-write");
    }
    if c.get("modelOptions").is_none() {
        c["modelOptions"] = json!({});
    }
    for key in ["harness", "model", "reasoning"] {
        if c.get(key).is_none() {
            c[key] = Json::Null;
        }
    }
    let field = |key: &str| args.field(key).map(text).filter(|s| !s.is_empty());
    if let Some(model) = field("model") {
        match model.split_once('/') {
            Some((harness, model)) => {
                c["harness"] = json!(harness);
                c["model"] = json!(model);
            }
            None => c["model"] = json!(model),
        }
    }
    if let Some(harness) = field("harness") {
        c["harness"] = json!(harness);
    }
    if let Some(effort) = field("effort") {
        c["reasoning"] = json!(effort);
    }
    if let Some(access) = field("access") {
        match access.as_str() {
            "supervised" => c["access"] = json!("supervised"),
            "edits" => c["access"] = json!("auto-accept-edits"),
            _ => {
                if let Some(map) = c.as_object_mut() {
                    map.remove("access");
                }
            }
        }
    }
    if let Some(plan) = args.field("plan") {
        if plan.truthy() {
            c["plan"] = json!(true);
        } else if let Some(map) = c.as_object_mut() {
            map.remove("plan");
        }
    }
    // The model's Fast option, as the catalog lists it (nil: it has none,
    // or the catalog isn't in yet).
    let fast_of_model = {
        let chats = CHATS.with(Rc::clone);
        let harness = c["harness"].as_str().unwrap_or_default().to_owned();
        let model = c["model"].as_str().unwrap_or_default().to_owned();
        let found = chats
            .models
            .borrow()
            .get(&harness)
            .and_then(|l| l.iter().find(|m| m["id"].as_str() == Some(&model)).cloned());
        found.and_then(|m| fast_option(&m))
    };
    // The model's other options, picked whole: each option's choice by id
    // (one at its default is left out, as the shipping app leaves it);
    // Fast's own stays as it is (`fast` below sets it).
    if let Some(Value::Map(picked)) = args.field("options") {
        let mut next = serde_json::Map::new();
        if let Some((key, _, _)) = &fast_of_model
            && let Some(v) = c["modelOptions"].get(key)
        {
            next.insert(key.clone(), v.clone());
        }
        for (key, choice) in picked.iter() {
            if let Value::Str(choice) = choice
                && !choice.is_empty()
            {
                next.insert(key.to_string(), json!(&**choice));
            }
        }
        c["modelOptions"] = Json::Object(next);
    }
    if let Some(fast) = args.field("fast")
        && let Some((key, on, off)) = fast_of_model
    {
        c["modelOptions"][key] = json!(if fast.truthy() { on } else { off });
    }
    c
}

/// The prompt with its attachments named after it, as the shipping app
/// writes it (crates/ui/src/attachments.rs).
fn with_attachments(words: &str, paths: &[String], images: usize) -> String {
    if paths.is_empty() {
        return words.to_owned();
    }
    let body = if !words.trim().is_empty() {
        words.to_owned()
    } else if images == paths.len() {
        "See the attached image(s).".into()
    } else {
        "See the attached file(s).".into()
    };
    let mut out = format!("{body}\n\nAttached images (local files — open them to view):");
    for p in paths {
        out.push_str("\n- ");
        out.push_str(p);
    }
    out
}

/// A person's words, without the attachments' trailer, and the paths it
/// named.
fn split_attachments(prompt: &str) -> (String, Vec<String>) {
    const TRAILER: &str = "\n\nAttached images (local files — open them to view):";
    let Some(at) = prompt.find(TRAILER) else {
        return (prompt.to_owned(), Vec::new());
    };
    let paths = prompt[at + TRAILER.len()..]
        .lines()
        .filter_map(|l| l.strip_prefix("- "))
        .map(|l| l.trim().to_owned())
        .collect();
    let body = &prompt[..at];
    let body = match body {
        "See the attached image(s)." | "See the attached file(s)." => "",
        other => other,
    };
    (body.to_owned(), paths)
}

/// The paths a request's attachments have on this device.
fn local_paths(req: &Value) -> Vec<String> {
    req.field("attachments")
        .map(Value::list)
        .unwrap_or_default()
        .iter()
        .filter_map(|a| a.field("path").map(text))
        .filter(|p| !p.is_empty())
        .collect()
}

/// The block the composer adds for passages attached from the conversation
/// (ds/quote.luau `with_quotes`): its header, then `- who, when: words`, a
/// further line of words set in two.
const QUOTES_HEADER: &str = "Selected from this conversation:";

/// A prompt's words apart from its attached passages (where each was said,
/// and its words), as the shipping app parts review comments from a prompt
/// (crates/ui comments.rs `extract_badge`): only a whole trailing block.
fn split_quotes(words: &str) -> (String, Vec<(String, String)>) {
    split_block(words, QUOTES_HEADER)
}

/// The block the composer adds for review comments left on a file (the
/// shipping app's comments.rs `with_comments`): its header, then `- path:line:
/// body`, a further line of a body set in two.
const COMMENTS_HEADER: &str = "Review comments (each cites the workspace file and line it belongs to):";

/// The block for comments left on a diff (the shipping app's
/// `COMMENT_BLOCK_HEADER`): each `- path:line (L|R): body`, a range
/// `path:10-14`.
const DIFF_COMMENTS_HEADER: &str = "Comments on the diff (each cites the file and line it belongs to; L = line number in the original file, R = in the changed file):";

/// A prompt's words apart from its review comments (each its file and line,
/// and its words): only a whole trailing block, as `extract_badge` takes it,
/// under either header.
fn split_comments(words: &str) -> (String, Vec<(String, String)>) {
    let (mut head, mut bullets) = split_block_raw(words, COMMENTS_HEADER);
    if bullets.is_empty() {
        (head, bullets) = split_block_raw(words, DIFF_COMMENTS_HEADER);
    }
    let comments = bullets
        .into_iter()
        .map(|bullet| {
            // "path:line (R): body" (a side's marker, the earliest), or
            // "path:line: body" (the first ": " after a line number).
            let side = [" (L): ", " (R): "].into_iter().filter_map(|m| bullet.find(m).map(|at| (at, m.len()))).min_by_key(|(at, _)| *at);
            let mut cut = None;
            for (at, _) in bullet.match_indices(": ") {
                let before = &bullet[..at];
                if before.rsplit_once(':').is_some_and(|(_, n)| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit() || b == b'-')) {
                    cut = Some((at, 2));
                    break;
                }
            }
            let cut = match (side, cut) {
                (Some(s), Some(c)) => Some(if s.0 < c.0 { s } else { c }),
                (s, c) => s.or(c),
            };
            match cut {
                Some((at, len)) => (bullet[..at].to_owned(), bullet[at + len..].to_owned()),
                None => (String::new(), bullet),
            }
        })
        .collect();
    (head, comments)
}

/// A comment chip on the sent message (the shipping app's badge): how many,
/// and where they are.
fn comments_chip(comments: &[(String, String)]) -> Value {
    let label = if comments.len() == 1 { "1 comment".to_owned() } else { format!("{} comments", comments.len()) };
    let places = comments.iter().map(|(place, _)| place.as_str()).filter(|p| !p.is_empty()).collect::<Vec<_>>().join(", ");
    Value::record([
        ("kind", Value::from("file")),
        ("icon", Value::from("chat-round-line")),
        ("name", Value::from(label.as_str())),
        ("meta", Value::from(places.as_str())),
    ])
}

/// A trailing block under `header` of `- …` bullets (continued in two):
/// the words before it and each bullet's text, joined with its continuation.
fn split_block_raw(words: &str, header: &str) -> (String, Vec<String>) {
    let marker = format!("\n\n{header}\n");
    let start = format!("{header}\n");
    let (head, block) = if let Some(at) = words.rfind(&marker) {
        (&words[..at], &words[at + marker.len()..])
    } else if let Some(rest) = words.strip_prefix(&start) {
        ("", rest)
    } else {
        return (words.to_owned(), Vec::new());
    };
    if block.is_empty() || !block.lines().all(|l| l.starts_with("- ") || l.starts_with("  ")) {
        return (words.to_owned(), Vec::new());
    }
    let mut out: Vec<String> = Vec::new();
    for line in block.lines() {
        if let Some(bullet) = line.strip_prefix("- ") {
            out.push(bullet.to_owned());
        } else if let (Some(more), Some(last)) = (line.strip_prefix("  "), out.last_mut()) {
            last.push('\n');
            last.push_str(more);
        }
    }
    (head.to_owned(), out)
}

/// A trailing block of `- place: words` bullets under `header`.
fn split_block(words: &str, header: &str) -> (String, Vec<(String, String)>) {
    let (head, bullets) = split_block_raw(words, header);
    let pairs = bullets
        .into_iter()
        .map(|bullet| match bullet.split_once(": ") {
            Some((place, body)) => (place.to_owned(), body.to_owned()),
            None => (String::new(), bullet),
        })
        .collect();
    (head, pairs)
}

/// A passage's chip on the sent message (as the composer showed it): where
/// it was said, its words on one line, cut at a word.
fn quote_chip(place: &str, body: &str) -> Value {
    let line = body.split_whitespace().collect::<Vec<_>>().join(" ");
    let excerpt = if line.chars().count() <= 72 {
        line
    } else {
        let cut: String = line.chars().take(72).collect();
        match cut.rfind(' ') {
            Some(at) => format!("{}…", &cut[..at]),
            None => format!("{cut}…"),
        }
    };
    Value::record([
        ("kind", Value::from("file")),
        ("icon", Value::from("cursor")),
        ("name", Value::from(place)),
        ("meta", Value::from(excerpt.as_str())),
    ])
}

/// The shipping app's browser annotation token (crates/proto annotation.rs):
/// `[Annotation N](zeron-annotation:<hex JSON>)`, as ds/annotation.luau
/// writes it after a prompt's words.
const ANNOTATION_SCHEME: &str = "](zeron-annotation:";

/// An element picked on a page, as a sent message shows it (the composer's
/// chip, ds/composer_cargo): its page, then its number and what it says.
struct Pick {
    index: u32,
    element: String,
    url: String,
    words: String,
}

/// A prompt's words apart from its annotation tokens (each read as its
/// label, "Annotation N", a line of nothing but a label dropped), and the
/// picks they stood for.
fn split_annotations(words: &str) -> (String, Vec<Pick>) {
    if !words.contains(ANNOTATION_SCHEME) {
        return (words.to_owned(), Vec::new());
    }
    let mut out = String::with_capacity(words.len());
    let mut picks = Vec::new();
    let mut rest = words;
    while let Some(at) = rest.find(ANNOTATION_SCHEME) {
        let head = &rest[..at];
        let Some(open) = head.rfind('[') else {
            out.push_str(&rest[..at + ANNOTATION_SCHEME.len()]);
            rest = &rest[at + ANNOTATION_SCHEME.len()..];
            continue;
        };
        let label = &head[open + 1..];
        let hex_start = at + ANNOTATION_SCHEME.len();
        let Some(close) = rest[hex_start..].find(')') else {
            out.push_str(rest);
            rest = "";
            break;
        };
        let hex = &rest[hex_start..hex_start + close];
        let pick = (hex.len() % 2 == 0)
            .then(|| {
                (0..hex.len())
                    .step_by(2)
                    .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
                    .collect::<Option<Vec<u8>>>()
            })
            .flatten()
            .and_then(|bytes| serde_json::from_slice::<Json>(&bytes).ok())
            .map(|json| {
                let text = |key: &str| json[key].as_str().unwrap_or_default().split_whitespace().collect::<Vec<_>>().join(" ");
                let words = [text("text"), text("name")].into_iter().find(|w| !w.is_empty()).unwrap_or_default();
                Pick {
                    index: json["index"].as_u64().unwrap_or(0) as u32,
                    element: json["element"].as_str().unwrap_or_default().to_owned(),
                    url: json["url"].as_str().unwrap_or_default().to_owned(),
                    words,
                }
            });
        match pick {
            Some(pick) => {
                out.push_str(&head[..open]);
                out.push_str(label);
                picks.push(pick);
            }
            None => out.push_str(&rest[..hex_start + close + 1]),
        }
        rest = &rest[hex_start + close + 1..];
    }
    out.push_str(rest);
    // A line that was only a token says nothing now.
    let kept: Vec<&str> = out
        .lines()
        .filter(|l| !(l.starts_with("Annotation ") && l[11..].trim().chars().all(|c| c.is_ascii_digit())))
        .collect();
    let mut body = kept.join("\n");
    while body.ends_with('\n') {
        body.pop();
    }
    (body, picks)
}

/// A page's address as a chip names it: no scheme, no "www.", no lone
/// trailing slash (ds/composer_cargo's `page_of`).
fn page_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let rest = rest.strip_prefix("www.").unwrap_or(rest);
    match rest.strip_suffix('/') {
        Some(host) if !host.contains('/') => host.to_owned(),
        _ => rest.to_owned(),
    }
}

/// The chip a sent pick shows, as the composer's did: its page over its
/// number and what it says (else its element).
fn annotation_chip(pick: &Pick) -> Value {
    let label = format!("Annotation {}", pick.index);
    let page = page_of(&pick.url);
    let what = if pick.words.is_empty() { pick.element.as_str() } else { pick.words.as_str() };
    let (name, meta) = if page.is_empty() {
        (label, pick.element.clone())
    } else {
        (page, format!("{label} · {what}"))
    };
    Value::record([
        ("kind", Value::from("file")),
        ("icon", Value::from("cursor")),
        ("name", Value::from(name.as_str())),
        ("meta", Value::from(meta.as_str())),
    ])
}

/// The chips a sent prompt carries: its files and pictures, then its
/// passages, then the elements picked on a page.
fn carried_value(paths: &[String], quotes: &[(String, String)], picks: &[Pick], comments: &[(String, String)]) -> Value {
    let Value::List(files) = attachments_value(paths) else { unreachable!() };
    let mut all: Vec<Value> = files.iter().cloned().collect();
    all.extend(quotes.iter().map(|(place, body)| quote_chip(place, body)));
    all.extend(picks.iter().map(annotation_chip));
    if !comments.is_empty() {
        all.push(comments_chip(comments));
    }
    Value::List(all.into())
}

fn attachments_value(paths: &[String]) -> Value {
    Value::List(
        paths
            .iter()
            .map(|p| {
                let name = std::path::Path::new(p)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(p);
                let image = is_image(p);
                let mut fields = vec![
                    ("kind", Value::from(if image { "image" } else { "file" })),
                    ("name", Value::from(name)),
                ];
                if image {
                    fields.push(("src", Value::from(p.as_str())));
                }
                Value::record(fields)
            })
            .collect::<Vec<_>>()
            .into(),
    )
}

/// A picture the window can draw (zgx decodes no GIF or HEIC yet: those go
/// as files, so a thumbnail never fails).
fn is_image(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    [".png", ".jpg", ".jpeg", ".webp"]
        .iter()
        .any(|e| lower.ends_with(e))
}

// -- the transcript -----------------------------------------------------------

/// What a frame changed: only words of a reply being written, rows, or
/// nothing that shows (more of a tool's output, which no row shows).
enum Change {
    Quiet,
    Words,
    Rows,
}

/// A frame applied to `entries` (crates/doc/src/transcript_delta.rs's
/// `apply_transcript_frame`); `Err` when the copy is out of step.
fn apply(entries: &mut Vec<Json>, frame: &Json) -> Result<Change, ()> {
    if let Some(reset) = frame.get("reset") {
        *entries = reset.as_array().cloned().unwrap_or_default();
        return Ok(Change::Rows);
    }
    let mut rows = false;
    let mut words_changed = false;
    for id in frame["remove"].as_array().into_iter().flatten() {
        let before = entries.len();
        entries.retain(|e| e["id"] != *id);
        rows |= entries.len() != before;
    }
    for up in frame["upsert"].as_array().into_iter().flatten() {
        let entry = &up["entry"];
        entries.retain(|e| e["id"] != entry["id"]);
        let at = match up["after"].as_str() {
            None => 0,
            Some(after) => entries
                .iter()
                .position(|e| e["id"].as_str() == Some(after))
                .map(|i| i + 1)
                .ok_or(())?,
        };
        entries.insert(at, entry.clone());
        rows = true;
    }
    for add in frame["append"].as_array().into_iter().flatten() {
        let entry = entries
            .iter_mut()
            .find(|e| e["id"] == add["entry"])
            .ok_or(())?;
        let streaming = entry["status"].as_str() == Some("streaming");
        let part = entry["parts"]
            .as_array_mut()
            .and_then(|parts| parts.iter_mut().find(|p| p["id"] == add["part"]))
            .ok_or(())?;
        let kind = part["kind"].as_str().unwrap_or_default().to_owned();
        let more = add["text"].as_str().unwrap_or_default();
        // Grown in place (a long reply isn't copied whole for each word).
        let was_empty = match part.get_mut("text") {
            Some(Json::String(text)) => {
                let empty = text.trim().is_empty();
                text.push_str(more);
                empty
            }
            _ => {
                part["text"] = Json::String(more.to_owned());
                true
            }
        };
        let len = part["text"].as_str().map_or(0, str::len);
        if add["len"].as_u64().is_some_and(|want| want as usize != len) {
            return Err(());
        }
        match kind.as_str() {
            // Words of the reply being written: the draft; of a finished
            // one, its row.
            "text" if streaming => words_changed = true,
            "text" => rows = true,
            // Thinking shows as a row once it has words; more of them, or
            // more of a tool's output, show nothing new.
            "reasoning" => rows |= was_empty && !more.trim().is_empty(),
            _ => {}
        }
    }
    if frame["count"].as_u64().is_some_and(|n| n as usize != entries.len()) {
        return Err(());
    }
    Ok(if rows {
        Change::Rows
    } else if words_changed {
        Change::Words
    } else {
        Change::Quiet
    })
}

/// `entry` with its visual parts naming `device`, their host, when it has
/// any and that isn't this one (nil otherwise: the entry as it is).
fn with_visual_device(entry: &Json, device: Option<&str>) -> Option<Json> {
    let device = device?;
    let parts = entry["parts"].as_array()?;
    if !parts.iter().any(|p| p["kind"].as_str() == Some("visual")) {
        return None;
    }
    let mut entry = entry.clone();
    for part in entry["parts"].as_array_mut().into_iter().flatten() {
        if part["kind"].as_str() == Some("visual") {
            part["device"] = Json::String(device.to_owned());
        }
    }
    Some(entry)
}

/// An entry's words: its text parts, each a paragraph.
fn entry_text(entry: &Json) -> String {
    let mut out = String::new();
    for p in entry["parts"].as_array().into_iter().flatten() {
        let words = match p["kind"].as_str() {
            Some("text") => p["text"].as_str().unwrap_or_default().to_owned(),
            Some("error") => format!("> {}", p["message"].as_str().unwrap_or_default()),
            _ => continue,
        };
        if words.trim().is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(&words);
    }
    out
}

/// An entry as ds/transcript's message; nil for one it doesn't show.
pub(crate) fn message(entry: &Json, model: Option<&str>) -> Option<Value> {
    let id = entry["id"].as_str()?;
    match entry["role"].as_str()? {
        "user" => {
            let text = entry["parts"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|p| p["kind"].as_str() == Some("text"))
                .filter_map(|p| p["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n\n");
            let (words, mut paths) = split_attachments(&text);
            // (An Appshot's context isn't what the person said: its picture
            // says which app instead.)
            let (words, shots) = crate::appshots::split_appshots(&words);
            let (words, comments) = split_comments(&words);
            let (words, quotes) = split_quotes(&words);
            let (words, picks) = split_annotations(&words);
            for p in entry["parts"].as_array().into_iter().flatten() {
                if p["kind"].as_str() == Some("image")
                    && let Some(path) = p["path"].as_str()
                    && !paths.iter().any(|q| q == path)
                {
                    paths.push(path.to_owned());
                }
            }
            // Always its time under it, as the shipping app's prompts carry
            // it (the strip shows on hover), and its words to copy.
            let created = entry["createdAt"].as_f64().unwrap_or(0.) / 1000.;
            Some(Value::record([
                ("id", Value::from(id)),
                ("role", Value::from("user")),
                ("text", Value::from(words.as_str())),
                ("attachments", crate::appshots::tag_attachments(carried_value(&paths, &quotes, &picks, &comments), &shots)),
                ("at", stamp(created as i64).map_or(Value::Nil, |at| Value::from(at.as_str()))),
                ("copy", copy_value(&words)),
            ]))
        }
        "assistant" => {
            let streaming = entry["status"].as_str() == Some("streaming");
            let created = entry["createdAt"].as_f64().unwrap_or(0.) / 1000.;
            let tools: Vec<Value> = entry["parts"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|p| !display_tool(p))
                .filter_map(tool)
                .collect();
            // A settled reply with nothing to show (only an empty thought,
            // say) has no row, as in the shipping app: drawn, it would be an
            // empty lane whose time floats over the next reply.
            let shown = blocks(entry);
            if !streaming && matches!(&shown, Value::List(list) if list.is_empty()) {
                return None;
            }
            let mut fields = vec![
                ("id", Value::from(id)),
                ("role", Value::from("assistant")),
                ("text", Value::from(entry_text(entry).as_str())),
                ("tools", Value::List(tools.into())),
                ("blocks", shown),
                ("streaming", Value::Bool(streaming)),
                ("started", Value::Num(created)),
            ];
            if let Some(model) = model {
                fields.push(("model", Value::from(model)));
            }
            // Its time and words to copy once it has settled, as the
            // shipping app's strip under a reply ("No timestamp hover
            // mid-stream").
            if !streaming {
                if let Some(at) = stamp(created as i64) {
                    fields.push(("at", Value::from(at.as_str())));
                }
                fields.push(("copy", copy_value(&reply_copy_text(entry))));
            }
            if let Some(ms) = entry["durationMs"].as_f64() {
                fields.push(("worked", Value::Num((ms / 1000.).round().max(1.))));
            }
            // The same reply laid out for Settings' compact transcript.
            if let Some(compact) = compact_blocks(entry) {
                fields.push(("compact", compact));
            }
            Some(Value::record(fields))
        }
        // The provider-switch seam (the engine's system entry when a chat
        // moves to another agent): `[mark] From → [mark] To` over the prompt
        // that switched, as the shipping app's `switch_marker`.
        "system" => {
            let part = entry["parts"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|p| p["kind"].as_str() == Some("switch"))?;
            let side = |name: &str, harness: &str| {
                let mark = part[harness].as_str().and_then(switch_mark);
                (
                    Value::from(single_line(part[name].as_str().unwrap_or_default()).as_str()),
                    mark.map_or(Value::Nil, |m| Value::from(m.as_str())),
                )
            };
            let (from, from_mark) = side("from", "fromHarness");
            let (to, to_mark) = side("to", "toHarness");
            Some(Value::record([
                ("id", Value::from(id)),
                ("role", Value::from("switch")),
                ("text", Value::from("")),
                ("from", from),
                ("to", to),
                ("from_mark", from_mark),
                ("to_mark", to_mark),
            ]))
        }
        _ => None,
    }
}

/// A switch side's mark: its agent's, the mock drawn as Claude, as the
/// shipping app's `harness_brand_icon`.
fn switch_mark(harness: &str) -> Option<String> {
    if harness == "mock" {
        return Some("claude".into());
    }
    crate::agents::glyph(harness)
}

/// A name on one line (the shipping app's `single_line`).
fn single_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// What a message's copy action copies; nil (no action) with nothing to copy.
fn copy_value(text: &str) -> Value {
    if text.trim().is_empty() { Value::Nil } else { Value::from(text) }
}

/// A reply's words to copy: its text parts as written, no tools, no errors
/// (the shipping app's `assistant_copy_text`).
fn reply_copy_text(entry: &Json) -> String {
    entry["parts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|p| p["kind"].as_str() == Some("text"))
        .filter_map(|p| p["text"].as_str())
        .filter(|t| !t.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// A reply's parts in order, as the shipping app lays them out (crates/ui
/// transcript.rs, its non-compact rows): its text, and between the text each
/// run of consecutive tools (thinking with them) as a group of its own,
/// keyed by its first part. A subagent's tools never share a group with the
/// others, nor thinking with them. A group is open while the reply streams
/// and it is the reply's tail.
fn blocks(entry: &Json) -> Value {
    let parts = entry["parts"].as_array().cloned().unwrap_or_default();
    let streaming = entry["status"].as_str() == Some("streaming");
    let last_ix = parts.len().saturating_sub(1);
    let mut out: Vec<Value> = Vec::new();
    // The group being gathered: its parts, and the last one's index.
    let mut group: Vec<&Json> = Vec::new();
    let mut group_last = 0usize;
    let flush = |out: &mut Vec<Value>, group: &mut Vec<&Json>, group_last: usize| {
        if group.is_empty() {
            return;
        }
        let first = group[0]["id"].as_str().unwrap_or_default();
        // (The part being written, if the group is the reply's tail: a
        // thought then shows its latest lines.)
        let live = streaming && group_last == last_ix;
        let tools: Vec<Value> = group
            .iter()
            .enumerate()
            .filter_map(|(i, p)| tool_live(p, live && i + 1 == group.len()))
            .collect();
        out.push(Value::record([
            ("id", Value::from(format!("g:{first}").as_str())),
            ("kind", Value::from("tools")),
            ("summary", Value::from(group_summary(group).as_str())),
            ("live", Value::Bool(streaming && group_last == last_ix)),
            ("tools", Value::List(tools.into())),
        ]));
        group.clear();
    };
    // The pictures an agent showed one after another (`show_image`, a
    // generated image), gathered into one block: a display call that worked
    // between them doesn't part them; anything else does.
    let mut pictures: Vec<&Json> = Vec::new();
    for (ix, part) in parts.iter().enumerate() {
        let kind = part["kind"].as_str();
        if kind == Some("image") {
            flush(&mut out, &mut group, group_last);
            pictures.push(part);
            continue;
        }
        if display_tool(part) && display_failed_block(part, &parts[ix + 1..]).is_none() {
            continue;
        }
        out.extend(images_block(&mut pictures));
        match kind {
            // A display tool (`show_visual`, `show_image`) is its output
            // alone: no row, in no group or count. Failed, a quiet line at
            // its place says so (unless a later call of it showed).
            Some("tool") if display_tool(part) => {
                if let Some(failed) = display_failed_block(part, &parts[ix + 1..]) {
                    flush(&mut out, &mut group, group_last);
                    out.push(failed);
                }
            }
            Some("tool") => {
                if group.first().is_some_and(|head| is_agent(head) != is_agent(part)) {
                    flush(&mut out, &mut group, group_last);
                }
                group.push(part);
                group_last = ix;
            }
            Some("reasoning") => {
                if part["text"].as_str().unwrap_or_default().trim().is_empty() {
                    continue;
                }
                if group.first().is_some_and(|head| is_agent(head)) {
                    flush(&mut out, &mut group, group_last);
                }
                group.push(part);
                group_last = ix;
            }
            kind => {
                flush(&mut out, &mut group, group_last);
                // An agent's visual (`show_visual`): its script on the
                // chat's host, run and drawn by src/visuals.rs and
                // ds/visual (`device` only when that isn't this one).
                if kind == Some("visual") {
                    out.push(visual_block(part));
                    continue;
                }
                let words = match kind {
                    Some("text") => part["text"].as_str().unwrap_or_default().to_owned(),
                    Some("error") => format!("> {}", part["message"].as_str().unwrap_or_default()),
                    _ => continue,
                };
                if words.trim().is_empty() {
                    continue;
                }
                out.push(Value::record([
                    ("id", Value::from(part["id"].as_str().unwrap_or_default())),
                    ("kind", Value::from("text")),
                    ("text", Value::from(words.as_str())),
                ]));
            }
        }
    }
    flush(&mut out, &mut group, group_last);
    out.extend(images_block(&mut pictures));
    Value::List(out.into())
}

/// A reply as the shipping app lays it out in compact mode (transcript.rs
/// `rows_for_entry` with `compact`): all its work, its tools, its thinking
/// and what it said between them, folded into one group where the first
/// of it was, its line the tools' summary while it works and "Worked for
/// 42s" once done; then the reply, the run of text it ends on, shown. While
/// it streams there is no reply yet: everything rides the group. Inside the
/// group, opened, the reply's ordinary blocks. Nil when it did no work.
fn compact_blocks(entry: &Json) -> Option<Value> {
    let parts = entry["parts"].as_array()?;
    let streaming = entry["status"].as_str() == Some("streaming");
    let said = |p: &Json| {
        p["kind"].as_str() == Some("text") && !p["text"].as_str().unwrap_or_default().trim().is_empty()
    };
    let thought = |p: &Json| {
        p["kind"].as_str() == Some("reasoning") && !p["text"].as_str().unwrap_or_default().trim().is_empty()
    };
    let reply_start = if streaming {
        None
    } else {
        parts.iter().rposition(said).map(|mut start| {
            while start > 0 && said(&parts[start - 1]) {
                start -= 1;
            }
            start
        })
    };
    // (A display tool is no work: its output shows in the reply.)
    let worked_tool = |p: &Json| p["kind"].as_str() == Some("tool") && !display_tool(p);
    let is_work = |ix: usize, p: &Json| {
        worked_tool(p) || thought(p) || (said(p) && reply_start.is_none_or(|start| ix < start))
    };
    let tools: Vec<&Json> = parts.iter().filter(|p| worked_tool(p) || thought(p)).collect();
    if tools.is_empty() {
        return None;
    }
    let mut work_entry = entry.clone();
    work_entry["parts"] = Json::Array(
        parts.iter().enumerate().filter(|(ix, p)| is_work(*ix, p)).map(|(_, p)| p.clone()).collect(),
    );
    let worked = (!streaming)
        .then(|| entry["durationMs"].as_f64())
        .flatten()
        .filter(|ms| *ms > 0.)
        .map(|ms| ((ms / 1000.) as i64).max(1));
    let line = match worked {
        Some(secs) => format!("Worked for {}", elapsed(secs)),
        None => group_summary(&tools),
    };
    let work = Value::record([
        ("id", Value::from("work")),
        ("kind", Value::from("work")),
        ("summary", Value::from(line.as_str())),
        ("live", Value::Bool(streaming)),
        ("tools", Value::List(tools.iter().filter_map(|p| tool(p)).collect::<Vec<_>>().into())),
        ("blocks", blocks(&work_entry)),
    ]);
    let mut out: Vec<Value> = Vec::new();
    let mut placed = false;
    let mut pictures: Vec<&Json> = Vec::new();
    for (ix, part) in parts.iter().enumerate() {
        if part["kind"].as_str() == Some("image") {
            pictures.push(part);
            continue;
        }
        if display_tool(part) && display_failed_block(part, &parts[ix + 1..]).is_none() {
            continue;
        }
        out.extend(images_block(&mut pictures));
        if is_work(ix, part) {
            if !placed {
                out.push(work.clone());
                placed = true;
            }
            continue;
        }
        let words = match part["kind"].as_str() {
            Some("text") => part["text"].as_str().unwrap_or_default().to_owned(),
            Some("error") => format!("> {}", part["message"].as_str().unwrap_or_default()),
            Some("visual") => {
                out.push(visual_block(part));
                continue;
            }
            Some("tool") => {
                out.extend(display_failed_block(part, &parts[ix + 1..]));
                continue;
            }
            _ => continue,
        };
        if words.trim().is_empty() {
            continue;
        }
        out.push(Value::record([
            ("id", Value::from(part["id"].as_str().unwrap_or_default())),
            ("kind", Value::from("text")),
            ("text", Value::from(words.as_str())),
        ]));
    }
    out.extend(images_block(&mut pictures));
    Some(Value::List(out.into()))
}

/// Tools whose output is what they show (a picture, a visual): drawn as
/// that output alone, never as a tool row, in no group, count or summary.
/// The images chat and the visuals chat share this rule.
const DISPLAY_TOOLS: &[&str] = &["show_image", "show_visual"];

/// Whether `part` is a display tool's call: an MCP tool by its name, bare or
/// as a harness names it (`mcp__zeron__show_visual`).
fn display_tool(part: &Json) -> bool {
    if part["kind"].as_str() != Some("tool") {
        return false;
    }
    let call = &part["call"];
    let name = match call["kind"].as_str() {
        Some("mcp") => call["tool"].as_str(),
        Some("unknown") => call["name"].as_str(),
        _ => None,
    };
    name.and_then(|n| n.rsplit("__").next()).is_some_and(|n| DISPLAY_TOOLS.contains(&n))
}

/// A display tool's name without its server (`show_image`), if `part` is one.
fn display_name(part: &Json) -> Option<&str> {
    if !display_tool(part) {
        return None;
    }
    let call = &part["call"];
    call["tool"].as_str().or(call["name"].as_str()).and_then(|n| n.rsplit("__").next())
}

/// A display tool's call that failed, as the quiet line ds/transcript draws
/// in its place (`display_failed`): which it was (`tool`: "image" or
/// "visual") and why, in its own words (ds/reply_picture's `plain` keeps
/// only what's plain of it). Nil for one that worked or is still working
/// (its output, or nothing yet, shows), and for one a later call of the
/// same tool in the reply (`later`, its parts after this one) got past:
/// the agent tried again and it showed, so the stumble isn't worth a line.
fn display_failed_block(part: &Json, later: &[Json]) -> Option<Value> {
    let name = display_name(part)?;
    if part["isError"].as_bool() != Some(true) {
        return None;
    }
    if later.iter().any(|p| display_name(p) == Some(name) && p["isError"].as_bool() != Some(true)) {
        return None;
    }
    let why = part["output"].as_str().map(str::trim).unwrap_or_default();
    Some(Value::record([
        ("id", Value::from(part["id"].as_str().unwrap_or_default())),
        ("kind", Value::from("display_failed")),
        ("tool", Value::from(name.strip_prefix("show_").unwrap_or(name))),
        ("text", Value::from(why)),
    ]))
}

/// The pictures gathered (image parts in a row) as one ds/transcript block,
/// `images`, keyed by the first: each its path on the chat's host and its
/// name. Nil for none; the gathering is emptied.
fn images_block(pictures: &mut Vec<&Json>) -> Option<Value> {
    let first = pictures.first()?["id"].as_str().unwrap_or_default().to_owned();
    let list: Vec<Value> = pictures
        .drain(..)
        .map(|p| {
            Value::record([
                ("id", Value::from(p["id"].as_str().unwrap_or_default())),
                ("path", Value::from(p["path"].as_str().unwrap_or_default())),
                ("name", Value::from(p["name"].as_str().unwrap_or_default())),
            ])
        })
        .collect();
    Some(Value::record([
        ("id", Value::from(first.as_str())),
        ("kind", Value::from("images")),
        ("images", Value::List(list.into())),
    ]))
}

/// A visual part as ds/transcript's block: its id (the visual's, which
/// seeds it), its script's path and title, and its host's device when that
/// isn't this one.
fn visual_block(part: &Json) -> Value {
    let field = |k: &str| part[k].as_str().map_or(Value::Nil, Value::from);
    Value::record([
        ("id", Value::from(part["id"].as_str().unwrap_or_default())),
        ("kind", Value::from("visual")),
        ("path", field("path")),
        ("title", field("title")),
        ("device", field("device")),
    ])
}

/// How long, as the shipping app says it (transcript.rs `format_elapsed`).
fn elapsed(secs: i64) -> String {
    let secs = secs.max(0);
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3_600 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else if secs < 86_400 {
        format!("{}h {}m", secs / 3_600, (secs % 3_600) / 60)
    } else {
        format!("{}d {}h", secs / 86_400, (secs % 86_400) / 3_600)
    }
}

/// A subagent's tab title, as the shipping app's `subagent_tab_title`: the
/// tool's name, else its description, else its prompt, the "Agent"/"Task"
/// genus taken off, the first line, at most 40 characters.
fn subagent_title(call: &Json) -> String {
    fn strip(text: &str) -> &str {
        let t = text.trim();
        for prefix in ["agent", "task"] {
            if t.len() >= prefix.len() && t.is_char_boundary(prefix.len()) && t[..prefix.len()].eq_ignore_ascii_case(prefix) {
                let rest = &t[prefix.len()..];
                if rest.is_empty() {
                    return "";
                }
                if rest.starts_with(':') || rest.starts_with(char::is_whitespace) {
                    return rest.trim_start_matches(':').trim();
                }
            }
        }
        t
    }
    fn line(text: &str) -> Option<String> {
        let first = text.lines().find(|l| !l.trim().is_empty())?.trim();
        let mut out: String = first.chars().take(40).collect();
        if first.chars().count() > 40 {
            out.push('…');
        }
        Some(out)
    }
    let name = match call["kind"].as_str() {
        Some("mcp") => call["tool"].as_str(),
        Some("unknown") => call["name"].as_str(),
        _ => return "Subagent".into(),
    };
    let input = &call["input"];
    [name, input["description"].as_str(), input["prompt"].as_str()]
        .into_iter()
        .flatten()
        .find_map(|t| line(strip(t)))
        .unwrap_or_else(|| "Subagent".into())
}

/// A subagent's call (its own group, as the shipping app's agent chips).
fn is_agent(part: &Json) -> bool {
    let call = &part["call"];
    let name = match call["kind"].as_str() {
        Some("mcp") => call["tool"].as_str(),
        Some("unknown") => call["name"].as_str(),
        _ => None,
    };
    part["kind"].as_str() == Some("tool") && name.is_some_and(|n| n == "Agent" || n.starts_with("Agent: "))
}

/// A group's line, as the shipping app writes it (crates/ui transcript.rs
/// `tool_group_summary` over crates/proto view.rs's): its thinking, then
/// what its tools did, then how many failed; the first letter raised.
fn group_summary(group: &[&Json]) -> String {
    let thoughts = group.iter().filter(|p| p["kind"].as_str() == Some("reasoning")).count();
    let calls: Vec<&&Json> = group.iter().filter(|p| p["kind"].as_str() == Some("tool")).collect();
    let mut segments: Vec<String> = Vec::new();
    match thoughts {
        0 => {}
        1 => segments.push("thought process".into()),
        n => segments.push(format!("thought {n} times")),
    }
    if !calls.is_empty() {
        let (mut commands, mut reads, mut searches, mut fetches, mut todos, mut other, mut failed) =
            (0, 0, 0, 0, 0, 0, 0);
        let mut edited: Vec<String> = Vec::new();
        for p in &calls {
            if p["isError"].as_bool() == Some(true) {
                failed += 1;
            }
            let call = &p["call"];
            match call["kind"].as_str().unwrap_or_default() {
                "exec" => commands += 1,
                "writeFile" | "editFile" => {
                    let path = call["path"].as_str().unwrap_or_default().to_owned();
                    if !edited.contains(&path) {
                        edited.push(path);
                    }
                }
                "applyPatch" => {
                    let path = call["path"].as_str().unwrap_or("patch").to_owned();
                    if !edited.contains(&path) {
                        edited.push(path);
                    }
                }
                "readFile" => reads += 1,
                "search" | "glob" | "webSearch" => searches += 1,
                "webFetch" => fetches += 1,
                "todo" => todos += 1,
                _ => other += 1,
            }
        }
        let plural = |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        let mut tools: Vec<String> = Vec::new();
        if commands > 0 {
            tools.push(format!("ran {}", plural(commands, "command", "commands")));
        }
        if !edited.is_empty() {
            tools.push(format!("edited {}", plural(edited.len(), "file", "files")));
        }
        if reads > 0 {
            tools.push(format!("read {}", plural(reads, "file", "files")));
        }
        if searches > 0 {
            tools.push(format!("searched {}", plural(searches, "time", "times")));
        }
        if fetches > 0 {
            tools.push(format!("fetched {}", plural(fetches, "page", "pages")));
        }
        if todos > 0 {
            tools.push("updated todos".into());
        }
        if other > 0 {
            tools.push(format!("called {}", plural(other, "tool", "tools")));
        }
        if tools.is_empty() {
            tools.push(plural(calls.len(), "tool", "tools"));
        }
        if failed > 0 {
            tools.push(format!("{failed} failed"));
        }
        segments.push(tools.join(" · "));
    }
    let mut summary = segments.join(" · ");
    if let Some(first) = summary.get_mut(..1) {
        first.make_ascii_uppercase();
    }
    summary
}

/// A part as ds/activity's tool: its kind, what it did, to what.
fn tool(part: &Json) -> Option<Value> {
    tool_live(part, false)
}

/// A thought's words as its row shows them: all of them. The row folds
/// them itself (ds/activity `thought_row`: four lines, "Show all N lines"),
/// so a cut here would leave "Show all" short of the whole thought.
fn thought_words(text: &str) -> String {
    text.trim().to_owned()
}

/// `tool`, for the part a reply is writing now (`live`).
fn tool_live(part: &Json, live: bool) -> Option<Value> {
    let id = part["id"].as_str()?;
    let (kind, verb, target): (&str, String, Option<String>) = match part["kind"].as_str()? {
        "reasoning" => {
            let words = part["text"].as_str().unwrap_or_default();
            if words.trim().is_empty() {
                return None;
            }
            // Its row opens to its words, as prose.
            return Some(Value::record([
                ("id", Value::from(id)),
                ("kind", Value::from("thought")),
                ("verb", Value::from("Thought process")),
                ("call", Value::from(thought_words(words).as_str())),
                ("prose", Value::Bool(true)),
                // (Its row opens itself while it's written.)
                ("live", Value::Bool(live)),
            ]));
        }
        "tool" => {
            let call = &part["call"];
            let s = |k: &str| call[k].as_str().map(str::to_owned);
            match call["kind"].as_str().unwrap_or_default() {
                "exec" => ("exec", "Ran".into(), s("command")),
                "readFile" => ("read", "Read".into(), s("path").map(|p| base(&p))),
                "writeFile" => ("write", "Wrote".into(), s("path").map(|p| base(&p))),
                "editFile" | "applyPatch" => ("edit", "Edited".into(), s("path").map(|p| base(&p))),
                "search" => ("search", "Searched".into(), s("pattern")),
                "glob" => ("glob", "Listed".into(), s("pattern")),
                "webFetch" => ("web", "Fetched".into(), s("url")),
                "webSearch" => ("web", "Searched the web".into(), s("query")),
                "todo" => ("todo", "Updated the plan".into(), None),
                "mcp" => {
                    let tool = s("tool").unwrap_or_default();
                    if tool == "Agent" || tool.starts_with("Agent: ") {
                        ("agent", "Ran a subagent".into(), tool.strip_prefix("Agent: ").map(Into::into))
                    } else {
                        ("exec", "Used".into(), Some(format!("{}.{tool}", s("server").unwrap_or_default())))
                    }
                }
                _ => {
                    let name = s("name").unwrap_or_else(|| "a tool".into());
                    if name == "Agent" || name.starts_with("Agent: ") {
                        ("agent", "Ran a subagent".into(), name.strip_prefix("Agent: ").map(Into::into))
                    } else {
                        ("exec", "Used".into(), Some(name))
                    }
                }
            }
        }
        _ => return None,
    };
    let mut fields = vec![
        ("id", Value::from(id)),
        ("kind", Value::from(kind)),
        ("verb", Value::from(verb.as_str())),
    ];
    // Its line is one line, whatever the agent wrote (the shipping app's
    // `tool_chip_content`: a command's lines, a pattern's newlines, joined).
    let target = target.map(|t| single_line(&t)).filter(|t| !t.is_empty());
    if let Some(target) = &target {
        fields.push(("target", Value::from(target.as_str())));
    }
    // The whole call, for the row opened, when it says more than the line.
    if let Some(call) = call_text(part).filter(|c| Some(c) != target.as_ref()) {
        fields.push(("call", Value::from(call.as_str())));
    }
    if part["isError"].as_bool() == Some(true) {
        fields.push(("failed", Value::Bool(true)));
    }
    // A subagent's spawn: the doc it runs in (its lane, a panel tab of its
    // own), how it stands, and its tab's title (the shipping app's).
    if kind == "agent"
        && let Some(doc) = part["subagentRef"].as_str().filter(|r| !r.is_empty())
    {
        fields.push((
            "subagent",
            Value::record([
                ("ref", Value::from(doc)),
                ("status", part["subagentStatus"].as_str().map_or(Value::Nil, Value::from)),
                ("title", Value::from(subagent_title(&part["call"]).as_str())),
            ]),
        ));
    }
    // What its row opens to (src/tool_details.rs).
    fields.extend(crate::tool_details::tool_fields(part));
    let stats = part["diffStats"].as_array();
    if let Some(stats) = stats.filter(|s| !s.is_empty()) {
        let sum = |k: &str| stats.iter().filter_map(|s| s[k].as_f64()).sum::<f64>();
        fields.push(("added", Value::Num(sum("additions"))));
        fields.push(("removed", Value::Num(sum("deletions"))));
    }
    Some(Value::record(fields))
}

/// How many lines of a call the opened row shows, and how long a line may
/// run (a minified megabyte isn't shaped whole).
const CALL_MAX_LINES: usize = 200;
const CALL_LINE_MAX_CHARS: usize = 2_000;

/// A tool's whole call, as the shipping app's `call_block` writes it: the
/// whole command, pattern (and where), url (and what was asked of it), the
/// plan's items one a line, an MCP or other tool's input as JSON. None for
/// thinking, and where the call is only a path (the line names it).
fn call_text(part: &Json) -> Option<String> {
    if part["kind"].as_str() != Some("tool") {
        return None;
    }
    let call = &part["call"];
    let s = |k: &str| call[k].as_str().filter(|v| !v.trim().is_empty());
    let input = |head: String| match call.get("input").filter(|v| !v.is_null()) {
        Some(v) => format!("{head}\n{}", serde_json::to_string_pretty(v).unwrap_or_default()),
        None => head,
    };
    let text = match call["kind"].as_str().unwrap_or_default() {
        "exec" => s("command")?.to_owned(),
        "search" => match s("path") {
            Some(path) => format!("{} in {path}", s("pattern")?),
            None => s("pattern")?.to_owned(),
        },
        "glob" => s("pattern")?.to_owned(),
        "webFetch" => match s("prompt") {
            Some(prompt) => format!("{}\n{prompt}", s("url")?),
            None => s("url")?.to_owned(),
        },
        "webSearch" => s("query")?.to_owned(),
        "todo" => call["items"]
            .as_array()?
            .iter()
            .map(|i| {
                let mark = match (i["status"].as_str(), i["done"].as_bool()) {
                    (Some("completed"), _) | (None, Some(true)) => "[x]",
                    (Some("inProgress"), _) => "[~]",
                    _ => "[ ]",
                };
                format!("{mark} {}", i["text"].as_str().unwrap_or_default())
            })
            .collect::<Vec<_>>()
            .join("\n"),
        "mcp" => input(format!("{} · {}", s("server").unwrap_or_default(), s("tool").unwrap_or_default())),
        "unknown" => input(s("name").unwrap_or("Tool").to_owned()),
        _ => return None,
    };
    let text = text.trim_end();
    if text.is_empty() {
        return None;
    }
    let mut lines: Vec<String> = text
        .lines()
        .take(CALL_MAX_LINES)
        .map(|l| match l.char_indices().nth(CALL_LINE_MAX_CHARS) {
            Some((cut, _)) => format!("{}…", &l[..cut]),
            None => l.to_owned(),
        })
        .collect();
    let more = text.lines().count().saturating_sub(CALL_MAX_LINES);
    if more > 0 {
        lines.push(format!("… {more} more lines"));
    }
    Some(lines.join("\n"))
}

fn base(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path)
        .to_owned()
}

/// The question the agent waits on: the last reply's unresolved input.
fn asked(entries: &[Json]) -> Option<&Json> {
    let last = entries.iter().rev().find(|e| e["role"].as_str() == Some("assistant"))?;
    last["parts"]
        .as_array()?
        .iter()
        .rev()
        .find(|p| p["kind"].as_str() == Some("input") && p["resolved"].as_bool() != Some(true))
}

/// What a waiting chat asks, from its latest entries: an approval, else a
/// question (also when the ask isn't among them).
fn ask_kind(entries: &[Json]) -> &'static str {
    match asked(entries).and_then(|p| p["questions"].as_array()) {
        Some(questions) if is_approval(questions) => "approval",
        _ => "question",
    }
}

fn is_approval(questions: &[Json]) -> bool {
    questions.len() == 1 && {
        let q = &questions[0];
        let options: Vec<&str> = q["options"].as_array().into_iter().flatten().filter_map(Json::as_str).collect();
        options == ["Yes", "No"] && q["header"].as_str().is_some_and(|h| h.starts_with("Approve"))
    }
}

fn single(questions: &[Json], label: &str) -> Vec<Json> {
    questions
        .iter()
        .filter_map(|q| q["id"].as_str())
        .map(|id| json!({ "questionId": id, "labels": [label] }))
        .collect()
}

/// What the composer shows of a question: an approval (what it would do),
/// or the page of questions not yet answered.
fn ask_value(part: &Json, answered: usize) -> Option<Value> {
    let request = part["requestId"].as_str()?;
    let questions = part["questions"].as_array()?;
    if is_approval(questions) {
        let q = &questions[0];
        let words = q["question"].as_str().unwrap_or_default();
        let title = match q["header"].as_str().unwrap_or_default() {
            "Approve command" => "Run this command?",
            "Approve file change" => "Make this change?",
            _ => "Allow this?",
        };
        // The command, when the question quotes one ("wants to run `x`").
        let command = words.split('`').nth(1).filter(|c| !c.is_empty());
        let mut fields = vec![("title", Value::from(title))];
        match command {
            Some(c) => fields.push(("command", Value::from(c))),
            None => fields.push(("detail", Value::from(words))),
        }
        return Some(Value::record([
            ("request", Value::from(request)),
            ("kind", Value::from("approval")),
            ("approval", Value::record(fields)),
        ]));
    }
    let page = answered.min(questions.len().saturating_sub(1));
    let q = questions.get(page)?;
    let options: Vec<Value> = q["options"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Json::as_str)
        .map(|o| Value::record([("value", Value::from(o)), ("label", Value::from(o))]))
        .collect();
    let mut question = vec![
        ("text", Value::from(q["question"].as_str().unwrap_or_default())),
        ("options", Value::List(options.into())),
        ("multi", Value::Bool(q["multiSelect"].as_bool() == Some(true))),
        ("page", Value::Num((page + 1) as f64)),
        ("pages", Value::Num(questions.len() as f64)),
    ];
    if let Some(h) = q["header"].as_str().filter(|h| !h.is_empty()) {
        question.push(("header", Value::from(h)));
    }
    Some(Value::record([
        ("request", Value::from(request)),
        ("kind", Value::from("question")),
        ("question", Value::record(question)),
    ]))
}

fn session_value(chats: &Chats, open: &Open, offline: bool) -> Value {
    let chat = chats.chat(&open.id);
    // Ready once its transcript has come and the chat itself (its config)
    // is known: the composer takes its model from it.
    let state = if open.heard && chat.is_some() {
        "ready"
    } else if chats.chats_heard.get() && chat.is_none() {
        "missing"
    } else if offline {
        "offline"
    } else {
        "loading"
    };
    let chat = chat.unwrap_or(Json::Null);
    let config = &chat["config"];
    let s = |j: &Json| j.as_str().map(Value::from).unwrap_or(Value::Nil);
    let harness = config["harness"].as_str().unwrap_or("claude-code");
    let model = config["model"]
        .as_str()
        .map(|m| Value::from(format!("{harness}/{m}").as_str()))
        .unwrap_or(Value::Nil);
    let access = match config["access"].as_str() {
        Some("supervised") => "supervised",
        Some("auto-accept-edits") => "edits",
        _ => "full",
    };
    let fast = fast_in(&config["modelOptions"]);
    let options = options_value(&config["modelOptions"]);
    let answered = if open.answering.is_some() { open.answers.len() } else { 0 };
    let ask = asked(&open.entries).and_then(|p| ask_value(p, answered)).unwrap_or(Value::Nil);
    let session = chats.sessions.borrow().get(&open.id).cloned().unwrap_or(Json::Null);
    let live = chats.running(&open.id);
    let tasks: Vec<Value> = if live {
        session["backgroundTasks"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|t| {
                let command = t["command"].as_str().unwrap_or_default();
                let name = t["description"].as_str().filter(|d| !d.is_empty()).unwrap_or(command);
                let mut fields = vec![
                    ("id", s(&t["id"])),
                    ("kind", Value::from("process")),
                    ("name", Value::from(name)),
                ];
                if name != command {
                    fields.push(("detail", Value::from(command)));
                }
                if let Some(since) = seconds(&t["startedAt"]) {
                    fields.push(("since", Value::Num(since as f64)));
                }
                Value::record(fields)
            })
            .collect()
    } else {
        Vec::new()
    };
    let queue: Vec<Value> = open
        .queue
        .iter()
        .map(|q| {
            let mut fields = vec![("id", s(&q["id"])), ("text", s(&q["text"]))];
            let mine = open.editing.as_ref().is_some_and(|e| q["id"].as_str() == Some(&e.item));
            if mine {
                fields.push(("editing", Value::Bool(true)));
            }
            let gate = &q["deliveryGate"];
            if gate["kind"].as_str() == Some("editing") && !mine {
                let who = gate["ownerDeviceId"].as_str().unwrap_or_default();
                if chats.here.borrow().as_deref() != Some(who) {
                    fields.push(("locked", Value::from("Being edited on another device")));
                }
            }
            Value::record(fields)
        })
        .collect();
    let mut fields = vec![
        ("id", Value::from(open.id.as_str())),
        ("state", Value::from(state)),
        ("title", s(&chat["title"])),
        ("project", s(&chat["spaceId"])),
        ("device", s(&chat["deviceId"])),
        // Its folder on that device (a reply's pictures resolve by it).
        ("cwd", s(&chat["cwd"])),
        ("harness", Value::from(harness)),
        ("model", model),
        ("effort", s(&config["reasoning"])),
        ("fast", Value::Bool(fast)),
        ("options", options),
        ("access", Value::from(access)),
        ("plan", Value::Bool(config["plan"].as_bool() == Some(true))),
        ("running", Value::Bool(live)),
        ("context", open.context.map(Value::Num).unwrap_or(Value::Nil)),
        ("context_tokens", open.context_usage.0.map(Value::Num).unwrap_or(Value::Nil)),
        ("context_window", open.context_usage.1.map(Value::Num).unwrap_or(Value::Nil)),
        ("ask", ask),
        ("queue", Value::List(queue.into())),
        ("todos", todos(&open.entries)),
        ("tasks", Value::List(tasks.into())),
    ];
    if let Some(branch) = chat["branch"].as_str() {
        fields.push(("branch", Value::from(branch)));
    }
    Value::record(fields)
}

/// The plan: the latest todo list the agent wrote.
fn todos(entries: &[Json]) -> Value {
    let latest = entries
        .iter()
        .rev()
        .flat_map(|e| e["parts"].as_array().into_iter().flatten().rev())
        .find(|p| p["kind"].as_str() == Some("tool") && p["call"]["kind"].as_str() == Some("todo"));
    let Some(part) = latest else {
        return Value::List(Rc::from([]));
    };
    let items: Vec<Value> = part["call"]["items"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(i, t)| {
            let state = match t["status"].as_str() {
                Some("completed") => "done",
                Some("inProgress") => "doing",
                _ if t["done"].as_bool() == Some(true) => "done",
                _ => "pending",
            };
            Value::record([
                ("id", Value::from(format!("todo{i}").as_str())),
                ("text", Value::from(t["text"].as_str().unwrap_or_default())),
                ("state", Value::from(state)),
            ])
        })
        .collect();
    Value::List(items.into())
}

/// How long a sent prompt reads "sending" with no turn to show for it, as
/// the shipping app's grace before it calls a send undelivered.
const SEND_GRACE_SECONDS: f64 = 60.;

/// A record with more fields (or some replaced).
fn with_fields<const N: usize>(record: Value, extra: [(&str, Value); N]) -> Value {
    let Value::Map(fields) = &record else { return record };
    let mut all: Vec<(std::rc::Rc<str>, Value)> = fields.iter().cloned().collect();
    for (key, value) in extra {
        all.retain(|(k, _)| &**k != key);
        all.push((key.into(), value));
    }
    Value::record(all)
}

fn draft_value(id: &str, text: &str) -> Value {
    Value::record([("id", Value::from(id)), ("text", Value::from(text))])
}

// -- small things ---------------------------------------------------------------

fn now_seconds() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// A time as Unix seconds: milliseconds since 1970, or RFC 3339.
fn seconds(value: &Json) -> Option<i64> {
    if let Some(ms) = value.as_f64() {
        return Some((ms / 1000.) as i64);
    }
    let stamp = value.as_str()?;
    let (date, time) = stamp.split_once('T')?;
    let mut date = date.split('-').map(|p| p.parse::<i64>().ok());
    let (y, m, d) = (date.next()??, date.next()??, date.next()??);
    let clock: String = time.chars().take(8).collect();
    let mut clock = clock.split(':').map(|p| p.parse::<i64>().ok());
    let (h, min, s) = (clock.next()??, clock.next()??, clock.next()??);
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let mut out = days * 86_400 + h * 3600 + min * 60 + s;
    if let Some(sign_at) = time.get(8..).and_then(|t| t.find(['+', '-'])) {
        let offset = &time[8 + sign_at..];
        let sign = if offset.starts_with('-') { -1 } else { 1 };
        let mut parts = offset[1..].split(':').map(|p| p.parse::<i64>().unwrap_or(0));
        let (oh, om) = (parts.next().unwrap_or(0), parts.next().unwrap_or(0));
        out -= sign * (oh * 3600 + om * 60);
    }
    Some(out)
}

fn stale(updated: &Json) -> bool {
    seconds(updated).is_some_and(|at| now_seconds() - at > STALE_SECONDS)
}

/// "Jul 1, 4:12 PM", in this Mac's time zone (the shipping app's
/// `format_timestamp`).
#[cfg(unix)]
fn stamp(at: i64) -> Option<String> {
    #[repr(C)]
    struct Tm {
        sec: i32,
        min: i32,
        hour: i32,
        mday: i32,
        mon: i32,
        year: i32,
        wday: i32,
        yday: i32,
        isdst: i32,
        gmtoff: std::ffi::c_long,
        zone: *const std::ffi::c_char,
    }
    unsafe extern "C" {
        fn localtime_r(time: *const i64, out: *mut Tm) -> *mut Tm;
    }
    if at <= 0 {
        return None;
    }
    let mut tm = std::mem::MaybeUninit::<Tm>::zeroed();
    // SAFETY: `localtime_r` writes the broken-down local time into the
    // caller's struct (the C library's `struct tm`, laid out as above on
    // macOS and glibc) and reads only `at`.
    let tm = unsafe {
        if localtime_r(&at, tm.as_mut_ptr()).is_null() {
            return None;
        }
        tm.assume_init()
    };
    let hour = match tm.hour % 12 {
        0 => 12,
        h => h,
    };
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let month = MONTHS.get(tm.mon as usize).copied().unwrap_or("Jan");
    Some(format!(
        "{month} {}, {hour}:{:02} {}",
        tm.mday,
        tm.min,
        if tm.hour < 12 { "AM" } else { "PM" }
    ))
}

#[cfg(not(unix))]
fn stamp(_at: i64) -> Option<String> {
    None
}

/// This window process's id, as an editor of queued messages.
fn instance_id() -> String {
    thread_local! {
        static ID: String = new_id();
    }
    ID.with(Clone::clone)
}

/// A random UUID (v4), as the engine's own are.
fn new_id() -> String {
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

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = match chunk.len() {
            3 => (u32::from(chunk[0]) << 16) | (u32::from(chunk[1]) << 8) | u32::from(chunk[2]),
            2 => (u32::from(chunk[0]) << 16) | (u32::from(chunk[1]) << 8),
            _ => u32::from(chunk[0]) << 16,
        };
        let sym = |shift: u32| TABLE[((n >> shift) & 63) as usize] as char;
        out.push(sym(18));
        out.push(sym(12));
        out.push(if chunk.len() > 1 { sym(6) } else { '=' });
        out.push(if chunk.len() > 2 { sym(0) } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_sent_pick_shows_its_page_over_its_number_and_words() {
        let pick = Pick {
            index: 2,
            element: "button#export.btn".into(),
            url: "https://www.example.com/dashboard/revenue?tab=1".into(),
            words: "Export report".into(),
        };
        let chip = annotation_chip(&pick);
        assert!(matches!(chip.field("name"), Some(Value::Str(s)) if s.as_ref() == "example.com/dashboard/revenue?tab=1"));
        assert!(matches!(chip.field("meta"), Some(Value::Str(s)) if s.as_ref() == "Annotation 2 · Export report"));
        assert_eq!(page_of("http://localhost:3000/"), "localhost:3000");
        let bare = Pick { index: 1, element: "div.card".into(), url: String::new(), words: String::new() };
        assert!(matches!(annotation_chip(&bare).field("name"), Some(Value::Str(s)) if s.as_ref() == "Annotation 1"));
    }

    #[test]
    fn a_subagent_spawn_carries_its_doc_status_and_title() {
        let spawn = serde_json::json!({
            "id": "t1", "kind": "tool",
            "call": { "kind": "unknown", "name": "Agent: scan the repo", "input": { "prompt": "Look around" } },
            "subagentRef": "chat-1--sub--7", "subagentStatus": "running",
        });
        let row = super::tool(&spawn).unwrap();
        let sub = row.field("subagent").unwrap();
        assert_eq!(sub.field("ref").map(crate::auth::text).as_deref(), Some("chat-1--sub--7"));
        assert_eq!(sub.field("status").map(crate::auth::text).as_deref(), Some("running"));
        assert_eq!(sub.field("title").map(crate::auth::text).as_deref(), Some("scan the repo"));
        // A spawn the engine hasn't stamped yet, and any other tool: none.
        let unstamped = serde_json::json!({ "id": "t2", "kind": "tool", "call": { "kind": "unknown", "name": "Agent" } });
        assert!(super::tool(&unstamped).unwrap().field("subagent").is_none());
        let read = serde_json::json!({ "id": "t3", "kind": "tool", "call": { "kind": "readFile", "path": "a.rs" }, "subagentRef": "x" });
        assert!(super::tool(&read).unwrap().field("subagent").is_none());
    }

    /// A side chat made here and sent to at once (before the engine's list
    /// has it): its sender knows it as it was made, its parent's agent and
    /// folder, never a config with no agent (which the engine refuses as
    /// bad params: the user's "Your message didn't send", Nov 2).
    #[test]
    fn a_chat_just_made_is_sent_as_it_was_made() {
        let id = "77777777-7777-4777-8777-777777777777";
        let params = serde_json::json!({
            "op": "createChat",
            "chatId": id,
            "parentChatId": "p",
            "deviceId": "d",
            "spaceId": "s",
            "cwd": "/repo",
            "config": { "harness": "devin", "model": "swe-2-high", "reasoning": "high", "modelOptions": {}, "sandbox": "workspace-write" },
        });
        super::made(&params);
        let row = super::CHATS.with(|c| c.chat(id)).expect("known at once");
        assert_eq!(row["cwd"], "/repo");
        assert_eq!(row["parentChatId"], "p");
        assert!(row.get("op").is_none() && row.get("chatId").is_none());
        let none = zgx_script::Value::from_json("{}").unwrap();
        let config = super::merged_config(&row["config"], &none);
        assert_eq!(config["harness"], "devin");
        assert_eq!(config["model"], "swe-2-high");
        // Unknown, with nothing picked: no agent (the sender stops there).
        assert!(super::merged_config(&serde_json::Value::Null, &none)["harness"].is_null());
        // The composer's pick wins.
        let picked = zgx_script::Value::from_json(r#"{"model":"codex/gpt-6.1"}"#).unwrap();
        let config = super::merged_config(&row["config"], &picked);
        assert_eq!((config["harness"].as_str(), config["model"].as_str()), (Some("codex"), Some("gpt-6.1")));
    }

    #[test]
    fn a_finished_subagent_s_upload_is_its_transcript() {
        assert_eq!(super::blob_ref(Some("c2"), "c1--sub--3"), "c1/c1--sub--3", "a copied chip keeps its first chat");
        assert_eq!(super::blob_ref(Some("c2"), "odd-doc"), "c2/odd-doc");
        let reply = serde_json::json!({ "text": "[{\"id\":\"e1\",\"role\":\"assistant\"}]" });
        assert_eq!(super::snapshot_frame(&reply).unwrap()["reset"][0]["id"], "e1");
        assert!(super::snapshot_frame(&serde_json::json!({ "text": "not json" })).is_none());
        assert!(super::snapshot_frame(&serde_json::json!({})).is_none());
    }

    #[test]
    fn a_thought_s_row_gets_all_its_words_and_folds_them_itself() {
        let part = json!({ "id": "r", "kind": "reasoning", "text": "Look at the parser.\nThen the host." });
        let row = super::tool(&part).unwrap();
        assert!(matches!(row.field("verb"), Some(Value::Str(s)) if s.as_ref() == "Thought process"));
        assert!(matches!(row.field("call"), Some(Value::Str(s)) if s.as_ref() == "Look at the parser.\nThen the host."));
        assert!(matches!(row.field("prose"), Some(Value::Bool(true))));
        // Settled, its fold shows its first lines; being written, its latest.
        assert!(matches!(row.field("live"), Some(Value::Bool(false))));
        let writing = super::tool_live(&part, true).unwrap();
        assert!(matches!(writing.field("live"), Some(Value::Bool(true))));
        assert!(super::tool(&json!({ "id": "e", "kind": "reasoning", "text": "  " })).is_none());
        // Long: every line reaches the row (it folds them under "Show all").
        let long: Vec<String> = (1..=30).map(|n| format!("line {n}")).collect();
        let words = super::thought_words(&format!("\n{}\n", long.join("\n")));
        assert!(words.starts_with("line 1\n") && words.ends_with("\nline 30"), "{words}");
        assert_eq!(words.lines().count(), 30);
    }

    #[test]
    fn another_devices_catalog_is_the_composers_and_names_its_device() {
        let chats = Rc::new(Chats::default());
        *chats.here.borrow_mut() = Some("mac".into());
        *chats.harnesses.borrow_mut() = vec![json!({ "id": "codex", "name": "Codex" })];
        chats.models.borrow_mut().insert("codex".into(), vec![json!({ "id": "gpt", "label": "GPT" })]);
        let local = chats.catalog_value();
        assert!(local.field("device").is_none());
        // On the box: its agents, once listed (looking until then).
        *chats.remote.borrow_mut() = Some(Remote {
            device: "box".into(),
            harnesses: vec![json!({ "id": "claude-code", "name": "Claude Code" })],
            models: HashMap::new(),
            ready: false,
        });
        let waiting = chats.catalog_value();
        assert!(matches!(waiting.field("state"), Some(Value::Str(s)) if s.as_ref() == "looking"));
        assert!(matches!(waiting.field("device"), Some(Value::Str(s)) if s.as_ref() == "box"));
        if let Some(r) = chats.remote.borrow_mut().as_mut() {
            r.models.insert("claude-code".into(), vec![json!({ "id": "opus", "label": "Opus" })]);
            r.ready = true;
        }
        let there = chats.catalog_value();
        let Some(Value::List(harnesses)) = there.field("harnesses") else { panic!("harnesses") };
        assert_eq!(harnesses.len(), 1);
        assert!(matches!(harnesses[0].field("id"), Some(Value::Str(s)) if s.as_ref() == "claude-code"));
        // Labels: this device's, else the box's.
        assert_eq!(chats.label_of("codex", "gpt").as_deref(), Some("GPT"));
        assert_eq!(chats.label_of("claude-code", "opus").as_deref(), Some("Opus"));
        // Back here: this device's again.
        chats.catalog_on("mac");
        assert!(chats.remote.borrow().is_none());
        assert!(chats.catalog_value().field("device").is_none());
    }

    #[test]
    fn back_takes_a_questions_last_page_off_and_gives_its_pick() {
        let mut open = Open { id: "c".into(), ..Open::default() };
        assert_eq!(open.back_a_page("c", "q"), None, "nothing answered yet");
        open.answering = Some("q".into());
        open.answers = vec![("branch".into(), vec!["fix".into()]), ("who".into(), vec!["ada".into()])];
        assert_eq!(open.back_a_page("other", "q"), None);
        assert_eq!(open.back_a_page("c", "r"), None);
        assert_eq!(open.back_a_page("c", "q"), Some(vec!["ada".to_owned()]));
        assert_eq!(open.answers.len(), 1, "the page before stays answered");
        assert_eq!(open.back_a_page("c", "q"), Some(vec!["fix".to_owned()]));
        assert_eq!(open.back_a_page("c", "q"), None, "on the first page");
    }

    #[test]
    fn subagent_titles_follow_the_shipping_app() {
        let t = |call: serde_json::Value| super::subagent_title(&call);
        assert_eq!(t(serde_json::json!({ "kind": "unknown", "name": "Agent: scan repo" })), "scan repo");
        // A bare genus has nothing to say: its description, then its prompt.
        assert_eq!(t(serde_json::json!({ "kind": "unknown", "name": "Agent", "input": { "description": "Find the flake" } })), "Find the flake");
        assert_eq!(t(serde_json::json!({ "kind": "mcp", "tool": "Task", "input": { "prompt": "\n  Read lib.rs\nthen more" } })), "Read lib.rs");
        assert_eq!(t(serde_json::json!({ "kind": "unknown", "name": "Taskmaster" })), "Taskmaster");
        assert_eq!(t(serde_json::json!({ "kind": "exec" })), "Subagent");
        let long = "x".repeat(50);
        assert_eq!(t(serde_json::json!({ "kind": "unknown", "name": long })).chars().count(), 41);
    }

    use super::*;

    /// The activity fixture (ui/activity_fixture.luau), headless.
    fn activity_app() -> zgx_script::testing::TestApp {
        let entry = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui/activity_fixture.luau");
        let app = zgx_script::testing::TestApp::new(|runtime| ScriptHost::new(runtime, entry));
        app.frames(10);
        app
    }

    const TOGGLE: &str = "role:button \"Read 1 file · ran 1 command\"";

    #[test]
    fn a_press_on_a_groups_toggle_selects_no_text() {
        let mut app = activity_app();
        app.click(TOGGLE);
        app.frames(20);
        assert!(!app.ui.has_text_selection(), "a click on the toggle selected text");
        app.click(TOGGLE);
        app.frames(20);
        app.double_click(TOGGLE);
        app.frames(20);
        assert!(!app.ui.has_text_selection(), "a double click on the toggle selected text");
    }

    /// The group open (its agent working), then closed by a press, a double
    /// press, a press that drags off: the fold moves the words under the
    /// pointer as it closes.
    #[test]
    fn closing_an_open_group_selects_no_text() {
        let mut app = activity_app();
        app.host.shared("activity_live", zgx_script::Value::Bool(false)).set(zgx_script::Value::Bool(true));
        app.frames(20);
        app.click(TOGGLE);
        app.frames(30);
        assert!(!app.ui.has_text_selection(), "closing the group with a click selected text");
        app.double_click(TOGGLE);
        app.frames(30);
        assert!(!app.ui.has_text_selection(), "a double click on the toggle selected text");
        app.drag(TOGGLE, "~\"And more words\"");
        app.frames(30);
        assert!(!app.ui.has_text_selection(), "a drag from the toggle selected text");
    }

    /// The same in a session's transcript (ui/chat_fixture.luau): its
    /// selectable list of measured rows, a reply's group folding in it.
    #[test]
    fn toggling_a_group_in_the_transcript_selects_no_text() {
        let entry = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui/chat_fixture.luau");
        let mut app = zgx_script::testing::TestApp::new(|runtime| ScriptHost::new(runtime, entry));
        app.frames(10);
        app.host.run_command("go_s1");
        app.frames(30);
        app.host.run_command("start_reply");
        app.host.run_command("add_tool");
        app.host.run_command("add_tool");
        app.frames(30);
        let toggle = "role:button \"Ran 2 commands\"";
        for _ in 0..3 {
            app.click(toggle);
            app.frames(30);
            assert!(!app.ui.has_text_selection(), "a click on the toggle selected text");
        }
        app.double_click(toggle);
        app.frames(30);
        assert!(!app.ui.has_text_selection(), "a double click on the toggle selected text");
        app.drag(toggle, "~\"Part 30 works\"");
        app.frames(30);
        assert!(!app.ui.has_text_selection(), "a drag from the toggle selected text");
    }

    /// The interface as boxes, without node ids (a rebuild makes new
    /// ones): what a person sees move, appear or go.
    /// A tree line's box: its top and height.
    fn top_and_height(line: &str) -> Option<(f32, f32)> {
        let words: Vec<&str> = line.split_whitespace().collect();
        words.windows(2).rev().find_map(|pair| {
            let (_, y) = pair[0].split_once(',')?;
            let (_, h) = pair[1].split_once('x')?;
            Some((y.parse().ok()?, h.parse().ok()?))
        })
    }

    fn boxes(app: &mut zgx_script::testing::TestApp) -> Vec<String> {
        let tree = app.tree();
        // The window's height (the root's box): what lies wholly above or
        // below it isn't seen, and moves no one sees.
        let window = tree.lines().next().and_then(top_and_height).map_or(f32::MAX, |(_, h)| h);
        tree.lines()
            // (The transcript's scrollbar shows only while it scrolls, and
            // goes once the view rests, on the clock: the frames here don't
            // run that clock, so it is still out from opening the session.)
            .filter(|line| !line.contains("#transcript-rail"))
            .filter(|line| top_and_height(line).is_none_or(|(y, h)| y + h > 0. && y < window))
            .map(|line| {
                let line = line.trim_start();
                match line.split_once(' ') {
                    Some((id, rest)) if id.starts_with('@') => rest.to_owned(),
                    _ => line.to_owned(),
                }
            })
            .collect()
    }

    /// What moved between two pictures of the interface, line by line.
    /// Two tree lines the same but for rounding: the tree prints boxes in
    /// whole points, so a node a hair either side of a half point (a pill
    /// at 156.9993 and at 157.0, its centred words at 163.4993 and 163.5)
    /// prints a point apart without moving.
    fn same_but_rounding(a: &str, b: &str) -> bool {
        let split = |s: &str| -> (String, Vec<i64>) {
            let mut shape = String::new();
            let mut numbers = Vec::new();
            let mut digits = String::new();
            for c in s.chars().chain(std::iter::once(' ')) {
                if c.is_ascii_digit() || (c == '-' && digits.is_empty()) {
                    digits.push(c);
                    continue;
                }
                if let Ok(n) = digits.parse::<i64>() {
                    numbers.push(n);
                    shape.push('#');
                } else {
                    shape.push_str(&digits);
                }
                digits.clear();
                shape.push(c);
            }
            (shape, numbers)
        };
        let ((sa, na), (sb, nb)) = (split(a), split(b));
        sa == sb && na.len() == nb.len() && na.iter().zip(&nb).all(|(x, y)| (x - y).abs() <= 1)
    }

    fn moved(before: &[String], after: &[String]) -> Vec<String> {
        let mut out = Vec::new();
        for i in 0..before.len().max(after.len()) {
            let (a, b) = (before.get(i), after.get(i));
            let same = match (a, b) {
                (Some(a), Some(b)) => same_but_rounding(a, b),
                _ => false,
            };
            if !same {
                out.push(format!("{a:?} -> {b:?}"));
            }
        }
        out
    }

    /// A save reloads the scripts, which rebuilds the interface from kept
    /// state: nothing should move, rise or fade for it, on a new chat or a
    /// session.
    #[test]
    fn a_reload_moves_nothing() {
        let entry = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui/chat_fixture.luau");
        let mut app = zgx_script::testing::TestApp::new(|runtime| ScriptHost::new(runtime, entry));
        // A new chat; a session; its composer's queue tray; an approval in
        // the composer; a reply streaming with its group open and
        // the working line; a reply's groups; long folded messages; a view
        // scrolled up, the way back to the latest out.
        let scenes: [(&str, &[&str]); 9] = [
            ("home", &["go_home"]),
            ("session", &["go_s1"]),
            ("queue tray", &["go_s2", "queue_one"]),
            ("approval", &["go_s2", "ask_approval"]),
            ("streaming", &["go_s1", "start_reply", "add_tool", "stream_words"]),
            ("groups", &["go_s5"]),
            ("long messages", &["go_s4"]),
            ("scrolled up", &["go_s1"]),
            // Replies far taller than a row's estimate: the view put back
            // among rows not yet measured, a reply's lane out under the
            // pointer.
            ("scrolled up among tall replies", &["go_s7"]),
        ];
        for (route, commands) in scenes {
            for command in commands {
                app.host.run_command(command);
                app.frames(10);
            }
            if route == "scrolled up" {
                app.scroll("#transcript", -600.);
            }
            if route == "scrolled up among tall replies" {
                // Far up, several replies' worth, then a reply hovered.
                for _ in 0..6 {
                    app.scroll("#transcript", -400.);
                    app.frames(5);
                }
                app.frames(30);
                app.hover("#transcript");
            }
            app.frames(60);
            // (What settles on the clock, a folded group's rows let go, a
            // scroll's rest: let it, as a person waits before a save.)
            // Twice: what the first wait's timers start (the way back to
            // the latest rising in once the view is away) runs its course.
            for _ in 0..2 {
                std::thread::sleep(std::time::Duration::from_millis(1000));
                app.frames(60);
            }
            let settled = boxes(&mut app);
            let _ = app.host.reload();
            let mut changes = Vec::new();
            for frame in 1..=40 {
                app.frame();
                let now = boxes(&mut app);
                let diff = moved(&settled, &now);
                if !diff.is_empty() {
                    changes.push(format!("{route}, frame {frame}: {}", diff.join("; ")));
                }
            }
            assert!(changes.is_empty(), "a reload moved things:\n{}", changes[..changes.len().min(6)].join("\n"));
        }
    }

    #[test]
    fn a_double_click_on_the_words_still_selects_them() {
        let mut app = activity_app();
        app.double_click("~\"Some reply text\"");
        app.frames(5);
        assert!(app.ui.has_text_selection(), "the reply's words should stay selectable");
    }

    #[test]
    fn review_comments_part_from_the_prompt_as_one_chip() {
        let text = "Fix these\n\nReview comments (each cites the workspace file and line it belongs to):\n- src/app.rs:12: Rename this.\n  It reads badly.\n- docs/a: b.md:3: Typo: here";
        let (words, comments) = split_comments(text);
        assert_eq!(words, "Fix these");
        assert_eq!(
            comments,
            vec![
                ("src/app.rs:12".to_owned(), "Rename this.\nIt reads badly.".to_owned()),
                ("docs/a: b.md:3".to_owned(), "Typo: here".to_owned()),
            ]
        );
        let chip = comments_chip(&comments);
        assert!(matches!(chip.field("name"), Some(Value::Str(s)) if s.as_ref() == "2 comments"));
        assert!(matches!(chip.field("meta"), Some(Value::Str(s)) if s.as_ref() == "src/app.rs:12, docs/a: b.md:3"));
        // The header mid-prompt, not a trailing block: left alone.
        let plain = "See Review comments (each cites the workspace file and line it belongs to): later";
        assert_eq!(split_comments(plain), (plain.to_owned(), Vec::new()));
    }

    #[test]
    fn comments_on_a_diff_part_from_the_prompt_with_their_sides_and_ranges() {
        // As screens/chat/comments writes them (production's
        // COMMENT_BLOCK_HEADER): a side each, a range, the code fenced under.
        let text = "Address the review comments below.\n\nComments on the diff (each cites the file and line it belongs to; L = line number in the original file, R = in the changed file):\n- src/a.rs:10-12 (R): Why: this?\n  ```\n  let x = 1;\n  ```\n- src/a.rs:4 (L): Gone.";
        let (words, comments) = split_comments(text);
        assert_eq!(words, "Address the review comments below.");
        assert_eq!(
            comments,
            vec![("src/a.rs:10-12".to_owned(), "Why: this?\n```\nlet x = 1;\n```".to_owned()), ("src/a.rs:4".to_owned(), "Gone.".to_owned())]
        );
    }

    #[test]
    fn appends_change_only_what_shows() {
        let mut entries = Vec::new();
        let reset = json!({ "reset": [
            { "id": "a", "role": "assistant", "status": "streaming", "parts": [
                { "kind": "tool", "id": "k", "text": "out" },
                { "kind": "reasoning", "id": "r", "text": "" },
                { "kind": "text", "id": "t", "text": "Hi" },
            ] },
            { "id": "b", "role": "assistant", "status": "complete", "parts": [{ "kind": "text", "id": "t", "text": "Done" }] },
        ] });
        apply(&mut entries, &reset).unwrap();
        let frame = |entry: &str, part: &str, text: &str, len: usize| {
            json!({ "upsert": [], "remove": [], "append": [{ "entry": entry, "part": part, "text": text, "len": len }], "count": 2 })
        };
        // More of a tool's output: nothing shows it.
        assert!(matches!(apply(&mut entries, &frame("a", "k", "put", 6)), Ok(Change::Quiet)));
        assert_eq!(entries[0]["parts"][0]["text"], "output");
        // Thinking's first words make its row; more of them don't.
        assert!(matches!(apply(&mut entries, &frame("a", "r", "Hmm", 3)), Ok(Change::Rows)));
        assert!(matches!(apply(&mut entries, &frame("a", "r", "…", 6)), Ok(Change::Quiet)));
        // Words of the reply being written: the draft; of a finished one,
        // its row.
        assert!(matches!(apply(&mut entries, &frame("a", "t", " there", 8)), Ok(Change::Words)));
        assert!(matches!(apply(&mut entries, &frame("b", "t", "!", 5)), Ok(Change::Rows)));
        // Out of step (a length that doesn't match): resubscribe.
        assert!(apply(&mut entries, &frame("a", "t", "x", 99)).is_err());
    }

    #[test]
    fn a_frame_grows_a_reply_by_its_words_alone() {
        let mut entries = Vec::new();
        let reset = json!({ "reset": [
            { "id": "u", "role": "user", "parts": [{ "kind": "text", "id": "t0", "text": "hi" }] },
            { "id": "a", "role": "assistant", "status": "streaming", "parts": [{ "kind": "text", "id": "t0", "text": "Hel" }] },
        ] });
        assert!(matches!(apply(&mut entries, &reset), Ok(Change::Rows)));
        let words = json!({ "upsert": [], "remove": [], "append": [{ "entry": "a", "part": "t0", "text": "lo", "len": 5 }], "count": 2 });
        assert!(matches!(apply(&mut entries, &words), Ok(Change::Words)));
        assert_eq!(entry_text(&entries[1]), "Hello");
        // Out of step: a length that doesn't add up.
        let wrong = json!({ "upsert": [], "remove": [], "append": [{ "entry": "a", "part": "t0", "text": "!", "len": 9 }], "count": 2 });
        assert!(apply(&mut entries, &wrong).is_err());
    }

    #[test]
    fn a_prompts_attached_passages_show_as_chips_on_the_sent_message() {
        let text = "Why this one?\n\nSelected from this conversation:\n- GPT-5.5, Sep 8, 1:43 PM: Layout dominates.\n  Paint is second.\n- You: the first ask\n\nAttached images (local files — open them to view):\n- /tmp/shot.png";
        let entry = json!({ "id": "u", "role": "user", "createdAt": 1_788_900_000_000_i64,
            "parts": [{ "id": "t", "kind": "text", "text": text }] });
        let shown = message(&entry, None).expect("a prompt shows");
        assert!(matches!(shown.field("text"), Some(Value::Str(s)) if s.as_ref() == "Why this one?"));
        let Some(Value::List(chips)) = shown.field("attachments") else { panic!("no attachments") };
        let names: Vec<String> = chips
            .iter()
            .map(|c| match c.field("name") {
                Some(Value::Str(s)) => s.to_string(),
                _ => String::new(),
            })
            .collect();
        assert_eq!(names, vec!["shot.png", "GPT-5.5, Sep 8, 1:43 PM", "You"]);
        assert!(matches!(chips[1].field("meta"), Some(Value::Str(s)) if s.as_ref() == "Layout dominates. Paint is second."));
        // A prompt only quoting (no words of its own), and one quoting the
        // header mid-text, untouched.
        let (words, quotes) = split_quotes("Selected from this conversation:\n- You: hi");
        assert_eq!((words.as_str(), quotes.len()), ("", 1));
        let plain = "See: Selected from this conversation: is a header";
        assert_eq!(split_quotes(plain), (plain.to_owned(), Vec::new()));
    }

    #[test]
    fn a_run_with_another_agent_model_or_effort_changes_the_chat_and_nothing_else_does() {
        let chat = json!({ "harness": "claude-code", "model": "sonnet", "reasoning": "medium", "modelOptions": {}, "sandbox": "workspace-write" });
        // The chat's own config, filled out as a run's is: the same.
        let same = merged_config(&chat, &Value::record([("text", Value::from("hi"))]));
        assert!(!runs_otherwise(&chat, &same));
        // A config with no options or nulls reads as one with empty ones.
        assert!(!runs_otherwise(&json!({ "harness": "claude-code", "model": "sonnet", "reasoning": "medium" }), &same));
        // Another agent's model, or another effort: the chat takes it.
        let other = merged_config(&chat, &Value::record([("model", Value::from("gpt")), ("harness", Value::from("codex"))]));
        assert!(runs_otherwise(&chat, &other));
        assert_eq!(other["harness"], json!("codex"));
        let effort = merged_config(&chat, &Value::record([("effort", Value::from("high"))]));
        assert!(runs_otherwise(&chat, &effort));
        // A model's options (Devin Fusion's lead and sidekick), picked
        // whole: what's picked is the chat's, what isn't is gone; a choice
        // at its default is left out of the picks.
        let chat = json!({ "harness": "devin", "model": "fusion", "reasoning": "high", "modelOptions": { "lead": "a", "sidekick": "x" } });
        let picked = merged_config(&chat, &Value::record([("options", Value::record([("lead", Value::from("b"))]))]));
        assert!(runs_otherwise(&chat, &picked));
        assert_eq!(picked["modelOptions"], json!({ "lead": "b" }));
        let none = merged_config(&chat, &Value::record([("options", Value::record::<&str>([]))]));
        assert_eq!(none["modelOptions"], json!({}));
    }

    #[test]
    fn a_models_options_reach_the_picker_but_fast_is_its_own() {
        let m = json!({
            "id": "fusion", "label": "Fusion", "reasoningLevels": ["low", "high"],
            "options": [
                { "id": "lead", "label": "Lead", "defaultChoice": "a", "choices": [{ "id": "a", "label": "Lead A" }, { "id": "b", "label": "Lead B" }] },
                { "id": "speed", "label": "Fast Mode", "defaultChoice": "standard", "choices": [{ "id": "standard", "label": "Standard" }, { "id": "fast", "label": "Fast" }] },
                { "id": "empty", "label": "Nothing", "defaultChoice": "", "choices": [] }
            ]
        });
        assert_eq!(fast_option(&m), Some(("speed".into(), "fast".into(), "standard".into())));
        let v = model_value("devin", &m);
        assert!(matches!(v.field("fast"), Some(Value::Bool(true))));
        let options = v.field("options").map(Value::list).unwrap_or_default();
        assert_eq!(options.len(), 1);
        assert_eq!(options[0].field("id").map(text).as_deref(), Some("lead"));
        assert_eq!(options[0].field("default").map(text).as_deref(), Some("a"));
        assert_eq!(options[0].field("choices").map(Value::list).map(<[Value]>::len), Some(2));
        // A chat's picks: Fast read as Fast, the rest as they are.
        let config = json!({ "speed": "fast", "lead": "b", "n": 3 });
        assert!(fast_in(&config));
        assert_eq!(options_value(&config), Value::record([("lead", Value::from("b")), ("speed", Value::from("fast"))]));
        // Claude's fastMode, off by default.
        let claude = json!({ "id": "opus", "options": [{ "id": "fastMode", "label": "Fast", "defaultChoice": "off", "choices": [{ "id": "off", "label": "Off" }, { "id": "on", "label": "On" }] }] });
        assert_eq!(fast_option(&claude), Some(("fastMode".into(), "on".into(), "off".into())));
        assert!(model_value("claude-code", &claude).field("options").is_none());
    }

    #[test]
    fn a_tools_line_is_one_line_and_its_whole_call_rides_with_it() {
        let str_of = |v: &Value, k: &str| match v.field(k) {
            Some(Value::Str(s)) => Some(s.to_string()),
            _ => None,
        };
        let part = |call: Json| json!({ "id": "p", "kind": "tool", "call": call });
        // A command of five lines: its line joins them; its call keeps them.
        let script = "echo '--- a'; git grep -n x\necho '--- b'; ls -d crates\necho '--- c'; git branch -a\necho '--- d'; gh pr list\necho '--- e'; gh issue list";
        let ran = tool(&part(json!({ "kind": "exec", "command": script }))).expect("a tool");
        let line = str_of(&ran, "target").expect("a line");
        assert!(!line.contains('\n'));
        assert!(line.starts_with("echo '--- a'; git grep -n x echo '--- b'"));
        assert_eq!(str_of(&ran, "call").as_deref(), Some(script));
        // A command of one short line: nothing more to open to.
        let short = tool(&part(json!({ "kind": "exec", "command": "cargo test" }))).expect("a tool");
        assert_eq!(str_of(&short, "target").as_deref(), Some("cargo test"));
        assert_eq!(str_of(&short, "call"), None);
        // A pattern with a newline and where it looked; an MCP call's input.
        let search = tool(&part(json!({ "kind": "search", "pattern": "fn a\\(\n", "path": "src" }))).expect("a tool");
        assert_eq!(str_of(&search, "target").as_deref(), Some("fn a\\("));
        assert_eq!(str_of(&search, "call").as_deref(), Some("fn a\\(\n in src"));
        let mcp = tool(&part(json!({ "kind": "mcp", "server": "gh", "tool": "issues", "input": { "q": "plugin" } }))).expect("a tool");
        assert_eq!(str_of(&mcp, "target").as_deref(), Some("gh.issues"));
        assert_eq!(str_of(&mcp, "call").as_deref(), Some("gh · issues\n{\n  \"q\": \"plugin\"\n}"));
        // An edit names its file; its strings never reach its line.
        let edit = tool(&part(json!({ "kind": "editFile", "path": "src/a.rs", "old_string": "x\ny", "new_string": "z" }))).expect("a tool");
        assert_eq!(str_of(&edit, "target").as_deref(), Some("a.rs"));
        assert_eq!(str_of(&edit, "call"), None);
        // A call past its cap says how much more there was.
        let long = (0..250).map(|i| format!("echo {i}")).collect::<Vec<_>>().join("\n");
        let big = tool(&part(json!({ "kind": "exec", "command": long }))).expect("a tool");
        let call = str_of(&big, "call").expect("a call");
        assert_eq!(call.lines().count(), CALL_MAX_LINES + 1);
        assert!(call.ends_with("… 50 more lines"));
    }

    #[test]
    fn a_compact_reply_folds_its_work_and_shows_the_text_it_ends_on() {
        let tool = |id: &str, command: &str| {
            json!({ "id": id, "kind": "tool", "isError": false, "resolved": true, "call": { "kind": "exec", "command": command } })
        };
        let text = |id: &str, words: &str| json!({ "id": id, "kind": "text", "text": words });
        let reply = |status: &str| {
            json!({ "id": "a", "role": "assistant", "status": status, "durationMs": 65_000,
                "parts": [text("p1", "Looking."), tool("k1", "ls"), text("p2", "Then:"), tool("k2", "cargo test"), text("p3", "Done."), text("p4", "All green.")] })
        };
        let kinds = |v: &Value| -> Vec<(String, String)> {
            let Value::List(list) = v else { panic!("a list") };
            list.iter()
                .map(|b| {
                    let field = |k: &str| match b.field(k) {
                        Some(Value::Str(s)) => s.to_string(),
                        _ => String::new(),
                    };
                    (field("id"), field("kind"))
                })
                .collect()
        };
        // Settled: one work group where the work began ("Worked for 1m 5s"),
        // then the run of text it ends on.
        let done = compact_blocks(&reply("complete")).expect("it did work");
        assert_eq!(
            kinds(&done),
            vec![("work".into(), "work".into()), ("p3".into(), "text".into()), ("p4".into(), "text".into())]
        );
        let Value::List(list) = &done else { unreachable!() };
        assert!(matches!(list[0].field("summary"), Some(Value::Str(s)) if s.as_ref() == "Worked for 1m 5s"));
        // Inside it, the ordinary blocks of its work: its words and groups.
        assert_eq!(
            kinds(list[0].field("blocks").unwrap()),
            vec![
                ("p1".into(), "text".into()),
                ("g:k1".into(), "tools".into()),
                ("p2".into(), "text".into()),
                ("g:k2".into(), "tools".into()),
            ]
        );
        // Still being written: no reply yet, everything rides the group, its
        // line the tools' summary.
        let live = compact_blocks(&reply("streaming")).expect("it did work");
        assert_eq!(kinds(&live), vec![("work".into(), "work".into())]);
        // No work, no group.
        let plain = json!({ "id": "b", "role": "assistant", "status": "complete", "parts": [text("t", "Hi.")] });
        assert!(compact_blocks(&plain).is_none());
    }

    #[test]
    fn a_settled_reply_with_nothing_to_show_has_no_row() {
        let reply = |status: &str| {
            json!({ "id": "a", "role": "assistant", "status": status, "createdAt": 1_788_900_000_000_i64,
                "parts": [{ "id": "r", "kind": "reasoning", "text": "" }] })
        };
        // Settled and empty: no row (no lane, no time over the next reply).
        assert!(message(&reply("complete"), None).is_none());
        // Still being written: its row holds the working line, and no time.
        let live = message(&reply("streaming"), None).expect("a reply being written has a row");
        assert!(live.field("at").is_none_or(|v| matches!(v, Value::Nil)));
    }

    #[test]
    fn an_upsert_lands_after_its_anchor() {
        let mut entries = vec![json!({ "id": "a" }), json!({ "id": "c" })];
        let frame = json!({ "upsert": [{ "after": "a", "entry": { "id": "b" } }], "remove": [], "append": [], "count": 3 });
        assert!(matches!(apply(&mut entries, &frame), Ok(Change::Rows)));
        let ids: Vec<_> = entries.iter().map(|e| e["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["a", "b", "c"]);
    }

    #[test]
    fn attachments_round_trip_through_the_prompt() {
        let prompt = with_attachments("", &["/tmp/a.png".into()], 1);
        let (words, paths) = split_attachments(&prompt);
        assert_eq!(words, "");
        assert_eq!(paths, ["/tmp/a.png"]);
    }

    #[test]
    fn a_replys_tools_group_between_its_text() {
        let text = |id: &str| json!({ "kind": "text", "id": id, "text": "words" });
        let exec = |id: &str| json!({ "kind": "tool", "id": id, "call": { "kind": "exec", "command": "ls" }, "resolved": true, "isError": false });
        let read = |id: &str| json!({ "kind": "tool", "id": id, "call": { "kind": "readFile", "path": "a.rs" }, "resolved": true, "isError": false });
        let entry = json!({ "id": "a", "role": "assistant", "status": "streaming", "parts": [
            { "kind": "reasoning", "id": "r1", "text": "hmm" },
            text("t1"), exec("x1"), exec("x2"), read("x3"), text("t2"), exec("x4"), read("x5"),
        ] });
        let Value::List(blocks) = blocks(&entry) else { panic!("no blocks") };
        let shape: Vec<String> = blocks
            .iter()
            .map(|b| format!("{}:{}", text_of(b, "kind"), text_of(b, "id")))
            .collect();
        assert_eq!(shape, ["tools:g:r1", "text:t1", "tools:g:x1", "text:t2", "tools:g:x4"]);
        assert_eq!(text_of(&blocks[0], "summary"), "Thought process");
        assert_eq!(text_of(&blocks[2], "summary"), "Ran 2 commands · read 1 file");
        // Open only at the tail of a streaming reply.
        assert!(!blocks[2].field("live").is_some_and(Value::truthy));
        assert!(blocks[4].field("live").is_some_and(Value::truthy));
    }

    fn text_of(v: &Value, key: &str) -> String {
        v.field(key).map(text).unwrap_or_default()
    }

    #[test]
    fn base64_pads() {
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"M"), "TQ==");
    }

    #[test]
    fn a_visual_part_is_a_block_of_its_own_read_from_its_host() {
        let entry = json!({ "id": "a1", "role": "assistant", "status": "complete", "parts": [
            { "kind": "text", "id": "t1", "text": "Here it is:" },
            { "kind": "visual", "id": "v:1", "path": "/u/v_1-visual.luau", "title": "Latency" },
        ]});
        let shown = blocks(&entry);
        let list = shown.list();
        assert_eq!(list.len(), 2);
        assert_eq!(list[1].field("kind"), Some(&Value::from("visual")));
        assert_eq!(list[1].field("id"), Some(&Value::from("v:1")));
        assert!(list[1].field("device").is_none_or(|d| *d == Value::Nil), "this device's: no device");
        // A chat on another device: its visuals are read from there.
        assert!(with_visual_device(&entry, None).is_none());
        let remote = with_visual_device(&entry, Some("dev-2")).expect("it has a visual");
        let shown = blocks(&remote);
        assert_eq!(shown.list()[1].field("device"), Some(&Value::from("dev-2")));
    }

    #[test]
    fn display_tools_are_their_output_alone() {
        let mcp = |id: &str, tool: &str, resolved: bool, error: bool| {
            json!({ "kind": "tool", "id": id, "resolved": resolved, "isError": error, "output": "no such file: chart.png",
                    "call": { "kind": "mcp", "server": "zeron", "tool": tool, "input": {} } })
        };
        let exec = |id: &str| json!({ "kind": "tool", "id": id, "resolved": true, "isError": false, "call": { "kind": "exec", "command": "ls" } });
        let reply = |parts: Vec<Json>| json!({ "id": "a1", "role": "assistant", "status": "complete", "parts": parts });
        // A call that worked, or is working: nothing of its own, in no group.
        let entry = reply(vec![exec("x1"), mcp("s1", "show_visual", true, false), exec("x2")]);
        let shown = blocks(&entry);
        assert_eq!(shown.list().len(), 1, "one group round it");
        assert_eq!(shown.list()[0].field("summary"), Some(&Value::from("Ran 2 commands")));
        assert_eq!(shown.list()[0].field("tools").unwrap().list().len(), 2);
        let running = reply(vec![mcp("s1", "mcp__zeron__show_image", false, false)]);
        assert!(blocks(&running).list().is_empty());
        let message = message(&entry, None).unwrap();
        assert_eq!(message.field("tools").unwrap().list().len(), 2, "the message's tools leave it out");
        // Failed: a quiet line with why, where it was.
        let failed = reply(vec![exec("x1"), mcp("s1", "show_image", true, true), exec("x2")]);
        let list = blocks(&failed);
        let kinds: Vec<String> = list.list().iter().map(|b| crate::auth::text(b.field("kind").unwrap())).collect();
        assert_eq!(kinds, ["tools", "display_failed", "tools"]);
        assert_eq!(list.list()[1].field("text"), Some(&Value::from("no such file: chart.png")));
        // Compact: no work of its own, the line in the reply.
        let only = reply(vec![mcp("s1", "show_visual", true, true), json!({ "kind": "text", "id": "t1", "text": "Done." })]);
        assert!(compact_blocks(&only).is_none(), "no work at all");
        let mixed = reply(vec![exec("x1"), mcp("s1", "show_visual", true, true), json!({ "kind": "text", "id": "t1", "text": "Done." })]);
        let compact = compact_blocks(&mixed).unwrap();
        let kinds: Vec<String> = compact.list().iter().map(|b| crate::auth::text(b.field("kind").unwrap())).collect();
        assert_eq!(kinds, ["work", "display_failed", "text"]);
    }

    #[test]
    fn pictures_an_agent_shows_sit_in_the_reply_in_place() {
        let mcp = |id: &str, error: bool| {
            json!({ "kind": "tool", "id": id, "resolved": true, "isError": error, "output": "visual:2: attempt to index nil",
                    "call": { "kind": "mcp", "server": "zeron", "tool": "mcp__zeron__show_image", "input": {} } })
        };
        let image = |id: &str, name: &str| json!({ "kind": "image", "id": id, "path": format!("/u/{name}"), "name": name, "mimeType": "image/png" });
        let exec = |id: &str| json!({ "kind": "tool", "id": id, "resolved": true, "isError": false, "call": { "kind": "exec", "command": "ls" } });
        let text = |id: &str, t: &str| json!({ "kind": "text", "id": id, "text": t });
        let reply = |parts: Vec<Json>| json!({ "id": "a1", "role": "assistant", "status": "complete", "parts": parts });
        let kinds = |v: &Value| -> Vec<String> { v.list().iter().map(|b| crate::auth::text(b.field("kind").unwrap())).collect() };
        // In a run of tools: the picture breaks out where its call was, the
        // tools either side fold as ever, and it counts in neither.
        let entry = reply(vec![exec("x1"), mcp("s1", false), image("i1", "a.png"), exec("x2"), exec("x3")]);
        let shown = blocks(&entry);
        assert_eq!(kinds(&shown), ["tools", "images", "tools"]);
        assert_eq!(shown.list()[0].field("summary"), Some(&Value::from("Ran 1 command")));
        assert_eq!(shown.list()[2].field("summary"), Some(&Value::from("Ran 2 commands")));
        let pics = shown.list()[1].field("images").unwrap().list();
        assert_eq!(pics.len(), 1);
        assert_eq!(pics[0].field("path"), Some(&Value::from("/u/a.png")));
        assert_eq!(pics[0].field("name"), Some(&Value::from("a.png")));
        // Two shown one after another (each its own call): one block, keyed
        // by the first, so a second arriving joins it.
        let two = reply(vec![text("t1", "Two:"), mcp("s1", false), image("i1", "a.png"), mcp("s2", false), image("i2", "b.png"), text("t2", "Done.")]);
        let shown = blocks(&two);
        assert_eq!(kinds(&shown), ["text", "images", "text"]);
        assert_eq!(shown.list()[1].field("id"), Some(&Value::from("i1")));
        assert_eq!(shown.list()[1].field("images").unwrap().list().len(), 2);
        // A reply of a picture alone has a row.
        assert!(message(&reply(vec![mcp("s1", false), image("i1", "a.png")]), None).is_some());
        // Compact: the picture in the reply, not the work.
        let compact = compact_blocks(&entry).unwrap();
        assert_eq!(kinds(&compact), ["work", "images"]);
        // Failed: which tool, and why as said (ds/reply_picture keeps what's plain).
        let failed = blocks(&reply(vec![mcp("s1", true), text("t1", "Sorry.")]));
        assert_eq!(kinds(&failed), ["display_failed", "text"]);
        assert_eq!(failed.list()[0].field("tool"), Some(&Value::from("image")));
        // Failed, then shown on another try: the stumble isn't drawn.
        let retried = blocks(&reply(vec![mcp("s1", true), mcp("s2", false), image("i1", "a.png")]));
        assert_eq!(kinds(&retried), ["images"]);
    }
}
