//! Orchestrators for the scripts: the long-lived manager chats, their crews,
//! the board, the asks waiting on you and the Brain dump's receipts, as the
//! engine streams them (`WatchOrchestration`), with the devices' load while a
//! screen shows it (`WatchDeviceLoads`) and the messages moving between agents
//! (`WatchComms`); and the one call that changes any of it (`Orchestrate`).
//! docs/design/orchestrators.md ("Bridge") is the contract;
//! ui/account/orchestration_sample.luau is a whole example of each.
//!
//! What scripts see (snake_case keys, times in Unix seconds, enum words in
//! snake_case: "brain_dump", "needs_you", "new_orchestrator"…; a field that
//! isn't there is nil):
//!   `orchestration`
//!     { state = "loading" | "ready",
//!       orchestrators = { { id, kind, name, goal, rules, crew_rules, notes, hue,
//!                           crew = { devices, max_running?, harness?, model? },
//!                           wake? = { every_minutes?, at, weekdays_only, prompt, last_woke? },
//!                           paused, archived, created, updated,
//!                           workflow? (ds/orch_flow's shape), flow_run? = { id, flow, task,
//!                           state, cards, returns, at?, reason?, started, ended? } } },
//!       crew = { { chat_id, orchestrator_id, item_id?, role,
//!                  last_report? = { id, state, summary, at }, released, joined } },
//!       items = { { id, orchestrator_id?, title, detail, state, priority, progress?,
//!                   threads, links = { { kind, label, target } }, outcome?,
//!                   source? = { dump_id, route_id }, order, created, updated, done?,
//!                   space_id?, device_id?, waiting?,
//!                   flow? = { run, node, kind, flow, step, index, of }, depends_on } },
//!       cards = { { id (its item's), place = "needs_you" | "snoozed" | "running" | "planned" |
//!                   "done" | "dropped", status, note?, until?, since, status_since,
//!                   seq, from?, reason, cause, by = "you" | "engine" | "agent",
//!                   asks = { ask id }, chip? = { kind, label, target? },
//!                   finish = { line = "report" | "merged" | "pr" | "accept", branch?, into?, pr? },
//!                   closed? = { at, by, how, note? }, held, work_at?,
//!                   word? = { state, at, from, summary, report? }, guests = { chat id },
//!                   checking? = { since, why }, seen_at?, unseen,
//!                   events = { { at, kind, text, by?, from?, to?, seq?, about? } } (oldest first),
//!                   old_state (the item `state` it was written with: a card whose item
//!                   says another was moved since by an engine from before places, and
//!                   shows by its item's), updated } }
//!                   (where each card stands, worked out by the engine: no row for a card
//!                   whose engine predates places),
//!       asks = { { id, from_chat, orchestrator_id?, item_id?, kind, question,
//!                  options = { { label, detail?, images = { path_or_url },
//!                                scores = { { name, value } } } },
//!                  urgent, state, answer?,
//!                  created, answered?, origin = "tool" | "turn_end", turn_id?,
//!                  updated?, escalated?, answered_via?, quiet,
//!                  images = { path_or_url }, recommendation? = { option? (1-based),
//!                  label?, why? }, links = { { kind, label, target } }, context?,
//!                  deferred_until? } },
//!       dumps = { { id, text, state, routes = { { id, excerpt, span? = { from, to },
//!                   target = { kind, id? }, intent, message, state,
//!                   candidates = { target }, answer?, answer_full?, answered?, delivered?,
//!                   direct, item_id?,
//!                   attachments = { path } } },
//!                   summary?, device, created, updated,
//!                   attachments = { { path, name } } } } (newest first) }
//!   `orchestration.loads`  { [device id] = { cpu?, mem_used?, mem_total?, load1?,
//!                            cores?, online, at,
//!                            history = { { at, cpu } } } } (while a screen
//!                            wants them; `history` the CPU samples heard
//!                            this run, oldest first, the last six minutes)
//!   `orchestration.comms`  { { id, kind, from, to, summary, at } } (newest last,
//!                            the latest 100; `from`/`to` a chat id or "you")
//!
//! and call:
//!   orchestration.op(op)           one `Orchestrate` op, as the wire has it
//!                                  (camelCase: `{ op = "answerAsk", id, answer }`):
//!                                  its reply (`{ id }` for what it made), or
//!                                  rejected with the engine's words
//!   orchestration.answer_ask({ id, option?, text? })
//!                                  the person's answer: `option` 1-based (the
//!                                  engine's 0-based), `text` their words (either
//!                                  or both); `{ id }`, or rejected
//!   orchestration.defer_ask({ id, until? })
//!                                  put an ask off until `until` (Unix seconds),
//!                                  nil: back now; `{ id }`, or rejected
//!   orchestration.remove_later({ id, ms }) (an action) a card the person
//!                                  deleted: `removeItem` sent after `ms` (its
//!                                  Undo's while), from here, so a window
//!                                  closed meanwhile still sends it, and the
//!                                  app's quit sends what still waits
//!   orchestration.remove_undo(id)  (an action) that delete undone: nothing sent
//!   orchestration.loads_wanted(on) (an action) a screen showing loads mounts
//!                                  (true) or goes (false): they're sampled only
//!                                  while one wants them

use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, VecDeque},
    rc::Rc,
    time::Duration,
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use zeron_proto::orchestration::frames::{Followed, Follower, Heard, Keyed, ListDelta, OrchestrationDelta};
use zeron_proto::orchestration::flow::{FlowEdge, FlowNode, FlowRun, Workflow};
use zeron_proto::orchestration::board::{BoardCard, CardEvent};
use zeron_proto::orchestration::{
    Ask, ChatActivity, CommEvent, CrewMember, DeviceLoad, Dump, OrchestrationState, Orchestrator, Route, RouteTarget,
    Score, TodoProgress, WorkItem, WorkLink,
};
use zgx_desktop::WindowContext;
use zgx_script::{ScriptHost, Shared, Value};

use crate::engine::{self, rpc::{Rpc, Watch}};

/// The most comms kept (the engine's own ring, `WatchComms`).
const COMMS: usize = 100;
/// The CPU samples kept for each device's line: six minutes at
/// `WatchDeviceLoads`' one every five seconds.
const HISTORY: usize = 72;

/// One open window's states, and how many of its screens want loads.
struct Window {
    id: u64,
    now: Shared,
    loads: Shared,
    comms: Shared,
    wants_loads: u32,
}

#[derive(Default)]
struct Orchestration {
    /// Each open window's, let go of as it closes.
    windows: RefCell<Vec<Window>>,
    next_window: Cell<u64>,
    /// The engine watched (once, for every window).
    watching: Cell<bool>,
    /// The state as the engine's frames have made it (none yet: loading),
    /// each row as scripts are given it, and that state as scripts were
    /// given it.
    heard: RefCell<Follower>,
    rows: RefCell<RowValues>,
    /// Watching again after a missed frame, until its snapshot comes.
    resyncing: Cell<bool>,
    now: RefCell<Option<Value>>,
    comms: RefCell<Comms>,
    loads: RefCell<Value>,
    /// Each device's CPU as it was heard, for its line (kept while the app
    /// runs: a screen shown again has its last minutes at once).
    history: RefCell<History>,
    /// Whether something the engine sent couldn't be read, said once.
    complained: Cell<bool>,
    watches: RefCell<Vec<Watch>>,
    /// `WatchDeviceLoads`, while a screen wants it.
    loads_watch: RefCell<Option<Watch>>,
}

thread_local! {
    static ORCHESTRATION: Rc<Orchestration> = Rc::new(Orchestration::default());
    /// Cards the person deleted, waiting out their Undo: each id with its
    /// wait's turn (deleted again after an Undo, a new wait).
    static REMOVING: RefCell<HashMap<String, u64>> = RefCell::new(HashMap::new());
    static REMOVING_TURN: Cell<u64> = const { Cell::new(0) };
}

/// The longest a delete waits for its Undo.
const REMOVE_WAIT_MAX: Duration = Duration::from_secs(60);

/// Card `id`'s delete starts its wait: its turn.
fn remove_later(id: &str) -> u64 {
    let turn = REMOVING_TURN.with(|t| {
        t.set(t.get() + 1);
        t.get()
    });
    REMOVING.with(|r| r.borrow_mut().insert(id.to_owned(), turn));
    turn
}

/// Card `id`'s delete undone: whether it was still waiting (nothing sent).
fn undo_remove(id: &str) -> bool {
    REMOVING.with(|r| r.borrow_mut().remove(id).is_some())
}

/// Whether the wait `turn` of card `id`'s delete is over and still wanted
/// (its Undo not pressed, no newer wait): then it's sent, and no longer waits.
fn remove_due(id: &str, turn: u64) -> bool {
    REMOVING.with(|r| {
        let mut r = r.borrow_mut();
        if r.get(id) == Some(&turn) {
            r.remove(id);
            true
        } else {
            false
        }
    })
}

/// How card `id`'s delete went (`true`: sent; `false`: undone), told to
/// the window that asked; refused, said in the log too. Whether the window
/// was still there to hear it.
fn tell_removal(to: tokio::sync::oneshot::Sender<Result<bool, String>>, id: &str, outcome: Result<bool, String>) -> bool {
    if let Err(error) = &outcome {
        eprintln!("zeron-zgx: orchestration: deleting card {id}: {error}");
    }
    to.send(outcome).is_ok()
}

/// The window's promise of a delete: `true` once sent, nil when undone (or
/// left for the quit: no engine to send it to), rejected with the engine's
/// words when it refused.
fn removal_reply(heard: Result<Result<bool, String>, tokio::sync::oneshot::error::RecvError>) -> Result<Value, String> {
    match heard {
        Ok(Ok(true)) => Ok(Value::Bool(true)),
        Ok(Ok(false)) | Err(_) => Ok(Value::Nil),
        Ok(Err(error)) => Err(error),
    }
}

/// The deletes still waiting, taken: the app is quitting.
fn removing_now() -> Vec<String> {
    let mut ids: Vec<String> = REMOVING.with(|r| r.borrow_mut().drain().map(|(id, _)| id).collect());
    ids.sort();
    ids
}

fn remove_op(id: &str) -> Json {
    json!({ "op": "removeItem", "id": id })
}

/// The app's quit (src/main.rs, once its windows are gone): the deletes
/// still waiting for their Undo are sent now, as they would have been.
pub fn after_quit() {
    let ids = removing_now();
    if ids.is_empty() {
        return;
    }
    let Some((port, data_dir)) = engine::get().ipc_endpoint() else {
        return;
    };
    for id in ids {
        if let Err(error) = engine::rpc::call_blocking(port, &data_dir, "Orchestrate", remove_op(&id), Duration::from_secs(5)) {
            eprintln!("zeron-zgx: orchestration: a card deleted before quitting wasn't ({error})");
        }
    }
}

pub fn install(cx: &mut WindowContext, host: &ScriptHost) {
    let o = ORCHESTRATION.with(Rc::clone);
    let window = o.next_window.get();
    o.next_window.set(window + 1);
    o.windows.borrow_mut().push(Window {
        id: window,
        now: host.shared("orchestration", o.now_value()),
        loads: host.shared("orchestration.loads", o.loads_now()),
        comms: host.shared("orchestration.comms", o.comms.borrow().value()),
        wants_loads: 0,
    });
    let me = Rc::downgrade(&o);
    cx.on_closed(move || {
        if let Some(o) = me.upgrade() {
            o.windows.borrow_mut().retain(|w| w.id != window);
            // (Its screens can't want loads any more.)
            o.follow_loads();
        }
    });
    if !o.watching.replace(true) {
        let weak = Rc::downgrade(&o);
        engine::get().on_connect(move |rpc| {
            let Some(o) = weak.upgrade() else { return };
            let state = watch_state(rpc, Rc::downgrade(&o));
            let heard = Rc::downgrade(&o);
            let comms = rpc.watch("WatchComms", json!({}), move |item| {
                if let Some(o) = heard.upgrade() {
                    o.heard_comms(&item);
                }
            });
            *o.watches.borrow_mut() = vec![state, comms];
            // (The old connection's is gone with it.)
            let old = o.loads_watch.borrow_mut().take();
            drop(old);
            o.follow_loads_on(Some(rpc));
        });
    }

    host.async_action("orchestration.op", move |op| {
        let mut params = crate::plugins::value_json(&op);
        async move {
            crate::plugins::preview::refuse("orchestration.op")?;
            person_op(&params)?;
            if params["op"] == "dump" {
                carry_dump(&op, &mut params).await?;
            }
            let reply = engine::get().call("Orchestrate", params).await?;
            Ok(if reply.is_null() { Value::Nil } else { crate::plugins::json_value(&reply) })
        }
    });
    host.async_action("orchestration.answer_ask", move |args| {
        let params = answer_params(&args);
        async move {
            crate::plugins::preview::refuse("orchestration.answer_ask")?;
            let params = params?;
            let reply = engine::get().call("Orchestrate", params).await?;
            Ok(if reply.is_null() { Value::Nil } else { crate::plugins::json_value(&reply) })
        }
    });
    host.async_action("orchestration.defer_ask", move |args| {
        let params = defer_params(&args);
        async move {
            crate::plugins::preview::refuse("orchestration.defer_ask")?;
            let params = params?;
            let reply = engine::get().call("Orchestrate", params).await?;
            Ok(if reply.is_null() { Value::Nil } else { crate::plugins::json_value(&reply) })
        }
    });
    host.async_action("orchestration.remove_later", move |args| {
        let id = args.field("id").map(crate::auth::text).filter(|id| !id.is_empty());
        let ms = match args.field("ms") {
            Some(Value::Num(ms)) if *ms >= 0.0 => *ms as u64,
            _ => 0,
        };
        async move {
            crate::plugins::preview::refuse("orchestration.remove_later")?;
            let id = id.ok_or("a card id")?;
            let wait = Duration::from_millis(ms).min(REMOVE_WAIT_MAX);
            let turn = remove_later(&id);
            let (told, heard) = tokio::sync::oneshot::channel();
            // (On the connection's tasks, not the window's: a window closed
            // meanwhile still sends it, and its refusal is only logged. With
            // no engine now, the app's quit sends it.)
            engine::get().spawn(async move {
                zgx::timer::sleep(wait).await;
                let outcome = if remove_due(&id, turn) {
                    engine::get().call("Orchestrate", remove_op(&id)).await.map(|_| true)
                } else {
                    Ok(false)
                };
                tell_removal(told, &id, outcome);
            });
            removal_reply(heard.await)
        }
    });
    host.action("orchestration.remove_undo", move |id| {
        if crate::plugins::preview::refuse("orchestration.remove_undo").is_err() {
            return;
        }
        undo_remove(&crate::auth::text(&id));
    });
    let a = o.clone();
    host.action("orchestration.loads_wanted", move |on| {
        if crate::plugins::preview::refuse("orchestration.loads_wanted").is_err() {
            return;
        }
        a.want_loads(window, on.truthy())
    });
}

/// `WatchOrchestration` as changes (`frames`: the whole state once, then
/// what moved; an engine from before them sends the whole state each time,
/// which reads as a snapshot).
fn watch_state(rpc: &Rpc, o: std::rc::Weak<Orchestration>) -> Watch {
    rpc.watch("WatchOrchestration", json!({ "changes": true }), move |item| {
        if let Some(o) = o.upgrade() {
            o.heard_state(item);
        }
    })
}

/// A dump's picks (`picks`: the composer's model, effort…) as `config`, and
/// its files (`attachments`, as the composer packs a message's) sent
/// on as a chat message's are (chat.rs `carry`): uploaded toward the Brain
/// dump's chat (`chat`, its id), and the op given what the engine reads, as
/// `QueueMessage` has them: `attachments` (each file's path where the run
/// reads it) and `transfers` (what it delivers first). None: the op as it
/// was, its words alone.
async fn carry_dump(op: &Value, params: &mut Json) -> Result<(), &'static str> {
    params.as_object_mut().map(|o| o.remove("chat"));
    // The composer's picks, made the engine's config for the Brain dump's
    // chat (as a send to it sets it: chat.rs `config_with`), applied
    // before the run.
    params.as_object_mut().map(|o| o.remove("picks"));
    if let (Some(picks), Some(chat)) = (op.field("picks"), op.field("chat").map(crate::auth::text)) {
        params["config"] = crate::chat::config_with(&chat, picks);
    }
    if op.field("attachments").map(Value::list).unwrap_or_default().is_empty() {
        params.as_object_mut().map(|o| o.remove("attachments"));
        return Ok(());
    }
    let target = op.field("chat").map(crate::auth::text).and_then(|id| crate::chat::target_of(&id));
    let carried = crate::chat::carry(op, target.as_deref()).await?;
    params["attachments"] = json!(carried.paths);
    if !carried.transfers.is_empty() {
        params["transfers"] = json!(carried.transfers);
    } else {
        params.as_object_mut().map(|o| o.remove("transfers"));
    }
    Ok(())
}

/// `answer_ask`'s table as the `answerAsk` op: its 1-based `option` made
/// 0-based, `text` the answer's words.
fn answer_params(args: &Value) -> Result<Json, String> {
    let id = args.field("id").map(crate::auth::text).filter(|id| !id.is_empty()).ok_or("an ask id")?;
    let text = args.field("text").map(crate::auth::text).unwrap_or_default();
    let option = match args.field("option") {
        Some(Value::Num(n)) if n.fract() == 0.0 && *n >= 1.0 => Some((*n as u64) - 1),
        None | Some(Value::Nil) => None,
        Some(_) => return Err("option is a whole number from 1".into()),
    };
    if option.is_none() && text.trim().is_empty() {
        return Err("an option or words".into());
    }
    let mut params = json!({ "op": "answerAsk", "id": id, "answer": text.trim() });
    if let Some(option) = option {
        params["option"] = json!(option);
    }
    Ok(params)
}

/// `defer_ask`'s table as the `deferAsk` op: `until` (Unix seconds) as the
/// engine's time; none, back now.
fn defer_params(args: &Value) -> Result<Json, String> {
    let id = args.field("id").map(crate::auth::text).filter(|id| !id.is_empty()).ok_or("an ask id")?;
    let until = match args.field("until") {
        Some(Value::Num(secs)) => {
            let at = DateTime::<Utc>::from_timestamp_millis((*secs * 1000.0) as i64).ok_or("until is out of range")?;
            Some(at.to_rfc3339())
        }
        None | Some(Value::Nil) => None,
        Some(_) => return Err("until is Unix seconds".into()),
    };
    let mut params = json!({ "op": "deferAsk", "id": id });
    if let Some(until) = until {
        params["until"] = json!(until);
    }
    Ok(params)
}

/// The ops the person sends from the window. The rest (`report`, `ask`,
/// `route`, `dumpDone`) are an agent's, through Zeron's tools, and never come
/// from here.
const PERSON_OPS: &[&str] = &[
    "createOrchestrator",
    "promoteChat",
    "updateOrchestrator",
    // (An orchestrator's flow run for a task, or its run stopped.)
    "startFlow",
    "stopFlow",
    "addToCrew",
    "releaseFromCrew",
    // (A chat put on the board: its card, made if it has none open.)
    "addToBoard",
    // (A card taken off the board: kept, marked removed; its chat untouched.)
    "removeItem",
    // (An Inbox or Next card started: its thread made.)
    "startItem",
    // (A card in Review: approved, Done; or changes asked for, its thread
    // told and the card back in Now.)
    "approveItem",
    "requestChanges",
    // (A card paused by the person: its threads stopped, their messages
    // held; or resumed, into a column, its held messages sent.)
    "pauseItem",
    "resumeItem",
    // (A card marked done or dropped by the person; one closed, opened
    // again; one kept for later, or let go; one they opened, seen.)
    "closeItem",
    "reopenItem",
    "holdItem",
    "seenItem",
    // (The profile's orchestration settings: crew auto-archive, notes…)
    "updateSettings",
    "upsertItem",
    "answerAsk",
    "dismissAsk",
    // (An ask put off until a time, or brought back.)
    "deferAsk",
    "dump",
    "reroute",
    // (The chat's note made fresh; never sent to the chat itself.)
    "progressCheck",
];

/// Whether `params` is one of the person's ops: else refused, unsent.
fn person_op(params: &Json) -> Result<(), String> {
    match params["op"].as_str() {
        Some(op) if PERSON_OPS.contains(&op) => Ok(()),
        Some(_) => Err("not yours".to_string()),
        None => Err("invalid".to_string()),
    }
}

impl Orchestration {
    fn now_value(&self) -> Value {
        self.now.borrow().clone().unwrap_or_else(loading_value)
    }

    fn loads_now(&self) -> Value {
        let loads = self.loads.borrow();
        if matches!(*loads, Value::Nil) { loads_value(&[]) } else { loads.clone() }
    }

    /// A frame of `WatchOrchestration`: applied to the state kept, and the
    /// rows it touched made again for the scripts (the rest are the same
    /// values, so what reads them does nothing). A frame that doesn't follow
    /// the last (one was missed) changes nothing: the state is watched again,
    /// and the new watch starts whole.
    fn heard_state(&self, item: Json) {
        let heard = match Heard::read(&item) {
            Ok(heard) => heard,
            Err(error) => return self.complain("WatchOrchestration", &error),
        };
        // What it changes, from the state kept (a snapshot: compared with it).
        let delta = match &heard {
            Heard::Delta { delta, .. } => Some(delta.clone()),
            Heard::Snapshot { state, .. } => self.heard.borrow().state.as_ref().map_or_else(
                || Some(OrchestrationDelta::default()),
                |was| OrchestrationDelta::between(was, state),
            ),
        };
        let first = self.heard.borrow().state.is_none();
        if matches!(heard, Heard::Snapshot { .. }) {
            self.resyncing.set(false);
        }
        if self.heard.borrow_mut().hear(heard) == Followed::Gap {
            return self.resync();
        }
        // (The same again: nothing to give.)
        let Some(delta) = delta else { return };
        let value = {
            let heard = self.heard.borrow();
            let Some(state) = heard.state.as_ref() else { return };
            let mut rows = self.rows.borrow_mut();
            if first {
                *rows = RowValues::default();
            }
            rows.forget(&delta);
            rows.value(state)
        };
        *self.now.borrow_mut() = Some(value.clone());
        for shared in self.targets(|w| &w.now) {
            shared.set(value.clone());
        }
    }

    /// Watch the state again: a frame was missed. Until the new watch's
    /// snapshot comes, no delta applies; the state kept stays, for the
    /// snapshot to be compared with (rows it doesn't change keep their
    /// values). (Later, not inside the watch's own callback.)
    fn resync(&self) {
        self.heard.borrow_mut().version = None;
        if self.resyncing.replace(true) {
            return;
        }
        #[cfg(test)]
        RESYNCS.with(|n| n.set(n.get() + 1));
        engine::get().spawn(async move {
            let Some(rpc) = engine::get().current() else { return };
            ORCHESTRATION.with(|o| {
                let state = watch_state(&rpc, Rc::downgrade(o));
                if let Some(slot) = o.watches.borrow_mut().first_mut() {
                    *slot = state;
                }
            });
        });
    }

    fn heard_comms(&self, item: &Json) {
        let changed = self.comms.borrow_mut().heard(item);
        match changed {
            Ok(false) => {}
            Ok(true) => {
                let value = self.comms.borrow().value();
                for shared in self.targets(|w| &w.comms) {
                    shared.set(value.clone());
                }
            }
            Err(error) => self.complain("WatchComms", &error),
        }
    }

    fn heard_loads(&self, item: &Json) {
        let Some(rows) = item.as_array() else {
            return self.complain("WatchDeviceLoads", &"not a list");
        };
        // (A row this build can't read costs that row, not the rest.)
        let loads: Vec<DeviceLoad> = rows.iter().filter_map(|row| DeviceLoad::deserialize(row).ok()).collect();
        self.history.borrow_mut().heard(&loads);
        let value = loads_value_with(&loads, &self.history.borrow());
        if *self.loads.borrow() == value {
            return;
        }
        *self.loads.borrow_mut() = value.clone();
        for shared in self.targets(|w| &w.loads) {
            shared.set(value.clone());
        }
    }

    fn complain(&self, method: &str, error: &dyn std::fmt::Display) {
        if !self.complained.replace(true) {
            eprintln!("zeron-zgx: orchestration: {method} sent what this build can't read ({error}); keeping what it had");
        }
    }

    /// Every window's state `pick` names, taken out first: setting one may
    /// run a script that calls back in.
    fn targets(&self, pick: impl Fn(&Window) -> &Shared) -> Vec<Shared> {
        self.windows.borrow().iter().map(|w| pick(w).clone()).collect()
    }

    /// One of `window`'s screens wants loads (`on`), or no longer does.
    fn want_loads(&self, window: u64, on: bool) {
        if let Some(w) = self.windows.borrow_mut().iter_mut().find(|w| w.id == window) {
            w.wants_loads = if on { w.wants_loads + 1 } else { w.wants_loads.saturating_sub(1) };
        }
        self.follow_loads();
    }

    /// `WatchDeviceLoads` on while any screen wants it, off once none does.
    fn follow_loads(&self) {
        self.follow_loads_on(None);
    }

    /// [`Self::follow_loads`] on `rpc`, else the connection there is now.
    fn follow_loads_on(&self, rpc: Option<&Rpc>) {
        let wanted = self.windows.borrow().iter().any(|w| w.wants_loads > 0);
        if !wanted {
            let watch = self.loads_watch.borrow_mut().take();
            drop(watch);
            return;
        }
        if self.loads_watch.borrow().is_some() {
            return;
        }
        // (No connection now: its `on_connect` starts it.)
        let Some(rpc) = rpc.cloned().or_else(|| engine::get().current()).filter(|rpc| !rpc.is_closed()) else { return };
        let heard = Rc::downgrade(&ORCHESTRATION.with(Rc::clone));
        let watch = rpc.watch("WatchDeviceLoads", json!({}), move |item| {
            if let Some(o) = heard.upgrade() {
                o.heard_loads(&item);
            }
        });
        *self.loads_watch.borrow_mut() = Some(watch);
    }
}

/// The comms as `WatchComms` gives them, newest last, the latest [`COMMS`].
#[derive(Default)]
struct Comms {
    events: VecDeque<CommEvent>,
}

impl Comms {
    /// `item` heard: a list of events (the stream's every frame: the
    /// backlog first, then what happened since, or the whole ring again
    /// after falling behind), or one event. Each is added, or put in place
    /// of the one with its id; the ring stays in time order, the newest
    /// [`COMMS`]. Whether anything changed.
    fn heard(&mut self, item: &Json) -> Result<bool, serde_json::Error> {
        let events: Vec<CommEvent> = match item.as_array() {
            // (A row this build can't read costs that row, not the rest.)
            Some(rows) => rows.iter().filter_map(|row| CommEvent::deserialize(row).ok()).collect(),
            None => vec![CommEvent::deserialize(item)?],
        };
        let mut changed = false;
        let mut out_of_order = false;
        for event in events {
            if let Some(held) = self.events.iter_mut().find(|e| e.id == event.id) {
                if *held != event {
                    out_of_order |= held.at != event.at;
                    *held = event;
                    changed = true;
                }
                continue;
            }
            out_of_order |= self.events.back().is_some_and(|last| last.at > event.at);
            self.events.push_back(event);
            changed = true;
        }
        if out_of_order {
            self.events.make_contiguous().sort_by_key(|e| e.at);
        }
        while self.events.len() > COMMS {
            self.events.pop_front();
        }
        Ok(changed)
    }

    fn value(&self) -> Value {
        Value::List(self.events.iter().map(comm_value).collect())
    }
}

/// Each device's CPU samples as `WatchDeviceLoads` gave them, oldest first,
/// the latest [`HISTORY`].
#[derive(Default)]
struct History {
    by_device: std::collections::HashMap<String, VecDeque<(DateTime<Utc>, f32)>>,
}

impl History {
    /// A round of loads heard: each device's CPU onto its line, once per
    /// sample (a round sent again adds nothing).
    fn heard(&mut self, loads: &[DeviceLoad]) {
        for l in loads {
            let Some(cpu) = l.cpu else { continue };
            let line = self.by_device.entry(l.device_id.clone()).or_default();
            if line.back().is_some_and(|(at, _)| *at >= l.at) {
                continue;
            }
            line.push_back((l.at, cpu));
            while line.len() > HISTORY {
                line.pop_front();
            }
        }
    }

    fn value(&self, device: &str) -> Value {
        let line = self.by_device.get(device);
        Value::List(
            line.into_iter()
                .flatten()
                .map(|(at, cpu)| Value::record([("at", secs(at)), ("cpu", fraction(*cpu))]))
                .collect(),
        )
    }
}

// ── Conversions (the wire's camelCase to the scripts' snake_case) ──────────

/// What scripts see before the engine has said anything.
fn loading_value() -> Value {
    let none = || Value::List(Vec::new().into());
    Value::record([
        ("state", Value::from("loading")),
        ("orchestrators", none()),
        ("crew", none()),
        ("items", none()),
        ("asks", none()),
        ("dumps", none()),
    ])
}

#[cfg(test)]
thread_local! {
    static RESYNCS: Cell<u32> = const { Cell::new(0) };
}

/// One collection's rows as scripts were given them, by key, and the list
/// of them (none: made again from the rows).
struct Collection {
    rows: std::collections::HashMap<String, Value>,
    list: Option<Value>,
}

impl Default for Collection {
    fn default() -> Self {
        Self { rows: Default::default(), list: None }
    }
}

impl Collection {
    /// What `delta` touched made again when next asked.
    fn forget<T: Keyed>(&mut self, delta: Option<&ListDelta<T>>) {
        let Some(delta) = delta else { return };
        self.list = None;
        match delta.touched() {
            Some(keys) => {
                for key in keys {
                    self.rows.remove(key);
                }
            }
            None => self.rows.clear(),
        }
    }

    /// `rows` as scripts see them: each row's value kept from before unless
    /// it was touched. (Rows whose keys repeat are each made again: they
    /// can't be told apart.)
    fn value<T: Keyed>(&mut self, rows: &[T], f: impl Fn(&T) -> Value) -> Value {
        if let Some(list) = &self.list {
            return list.clone();
        }
        let mut keys = std::collections::HashSet::with_capacity(rows.len());
        let unique = rows.iter().all(|r| keys.insert(r.key()));
        if !unique {
            self.rows.clear();
            let list = list(rows, f);
            self.list = Some(list.clone());
            return list;
        }
        self.rows.retain(|k, _| keys.contains(k.as_str()));
        let list = Value::List(
            rows.iter()
                .map(|r| self.rows.entry(r.key().to_owned()).or_insert_with(|| f(r)).clone())
                .collect(),
        );
        self.list = Some(list.clone());
        list
    }
}

/// Each collection's rows as scripts were given them.
#[derive(Default)]
struct RowValues {
    orchestrators: Collection,
    crew: Collection,
    items: Collection,
    cards: Collection,
    asks: Collection,
    dumps: Collection,
}

impl RowValues {
    fn forget(&mut self, delta: &OrchestrationDelta) {
        self.orchestrators.forget(delta.orchestrators.as_ref());
        self.crew.forget(delta.crew.as_ref());
        self.items.forget(delta.items.as_ref());
        self.cards.forget(delta.cards.as_ref());
        self.asks.forget(delta.asks.as_ref());
        self.dumps.forget(delta.dumps.as_ref());
    }

    /// The whole state, as scripts see it, from the rows kept.
    fn value(&mut self, s: &OrchestrationState) -> Value {
        Value::record([
            ("state", Value::from("ready")),
            ("orchestrators", self.orchestrators.value(&s.orchestrators, orchestrator_value)),
            ("crew", self.crew.value(&s.crew, crew_value)),
            ("items", self.items.value(&s.items, item_value)),
            ("cards", self.cards.value(&s.cards, card_value)),
            ("asks", self.asks.value(&s.asks, ask_value)),
            ("dumps", self.dumps.value(&s.dumps, dump_value)),
            ("settings", settings_value(s)),
        ])
    }
}

fn settings_value(s: &OrchestrationState) -> Value {
    // The profile's settings: `{ crew_auto_archive, crew_idle_hours }`.
    Value::record([
        ("crew_auto_archive", Value::Bool(s.settings.crew_auto_archive)),
        ("crew_idle_hours", Value::from(f64::from(s.settings.crew_idle_hours))),
    ])
}

/// The whole state, as scripts see it, made afresh.
#[cfg(test)]
fn state_value(s: &OrchestrationState) -> Value {
    RowValues::default().value(s)
}

fn orchestrator_value(o: &Orchestrator) -> Value {
    let crew = Value::record([
        ("devices", strings(&o.crew.devices)),
        ("max_running", opt(o.crew.max_running, |n| Value::from(f64::from(n)))),
        ("harness", opt_str(o.crew.harness.as_deref())),
        ("model", opt_str(o.crew.model.as_deref())),
        // What it and its crew do when their account hits its usage limit:
        // `{ switch_account, switch_harness }` (both off: wait for the reset).
        (
            "on_limit",
            Value::record([
                ("switch_account", Value::Bool(o.crew.on_limit.switch_account)),
                ("switch_harness", Value::Bool(o.crew.on_limit.switch_harness)),
            ]),
        ),
    ]);
    let wake = o.wake.as_ref().map_or(Value::Nil, |w| {
        Value::record([
            ("every_minutes", opt(w.every_minutes, |n| Value::from(f64::from(n)))),
            ("at", strings(&w.at)),
            ("weekdays_only", Value::Bool(w.weekdays_only)),
            ("prompt", Value::from(w.prompt.as_str())),
            ("last_woke", opt(w.last_woke_at.as_ref(), secs)),
        ])
    });
    Value::record([
        ("id", Value::from(o.id.as_str())),
        ("kind", word(&o.kind)),
        ("name", Value::from(o.name.as_str())),
        ("goal", Value::from(o.goal.as_str())),
        ("rules", Value::from(o.rules.as_str())),
        ("crew_rules", Value::from(o.crew_rules.as_str())),
        ("notes", Value::from(o.notes.as_str())),
        ("hue", Value::from(f64::from(o.hue))),
        ("crew", crew),
        ("wake", wake),
        ("paused", Value::Bool(o.paused)),
        ("archived", Value::Bool(o.archived)),
        ("created", secs(&o.created_at)),
        ("updated", secs(&o.updated_at)),
        // (Its flow, in the Flow view's own shape, and its latest run.)
        ("workflow", o.workflow.as_ref().map_or(Value::Nil, workflow_value)),
        ("flow_run", o.flow_run.as_ref().map_or(Value::Nil, run_value)),
    ])
}

/// A flow as ds/orch_flow has it (the version 2 team shape).
fn workflow_value(w: &Workflow) -> Value {
    let node = |n: &FlowNode| {
        Value::record([
            ("id", Value::from(n.id.as_str())),
            ("kind", word(&n.kind)),
            ("name", Value::from(n.name.as_str())),
            ("x", Value::from(n.x)),
            ("y", Value::from(n.y)),
            ("instructions", Value::from(n.instructions.as_str())),
            ("harness", Value::from(n.harness.as_str())),
            ("model", Value::from(n.model.as_str())),
        ])
    };
    let edge = |e: &FlowEdge| {
        Value::record([
            ("from", Value::from(e.from.as_str())),
            ("to", Value::from(e.to.as_str())),
            ("condition", word(&e.condition)),
            ("max_retries", opt(e.max_retries, |n| Value::from(f64::from(n)))),
        ])
    };
    Value::record([
        ("version", Value::from(f64::from(w.version))),
        ("name", Value::from(w.name.as_str())),
        ("next_id", Value::from(w.next_id as f64)),
        ("nodes", list(&w.nodes, node)),
        ("edges", list(&w.edges, edge)),
    ])
}

/// A flow's run: `{ id, flow, task, state, cards = { [step] = card },
/// returns = { ["from>to"] = n }, at?, reason?, started, ended? }`.
fn run_value(r: &FlowRun) -> Value {
    Value::record([
        ("id", Value::from(r.id.as_str())),
        ("flow", workflow_value(&r.flow)),
        ("task", Value::from(r.task.as_str())),
        ("state", word(&r.state)),
        ("cards", Value::record(r.cards.iter().map(|(k, v)| (k.as_str(), Value::from(v.as_str()))))),
        ("returns", Value::record(r.returns.iter().map(|(k, v)| (k.as_str(), Value::from(f64::from(*v)))))),
        ("at", opt_str(r.at.as_deref())),
        ("reason", opt_str(r.reason.as_deref())),
        ("started", secs(&r.started_at)),
        ("ended", opt(r.ended_at.as_ref(), secs)),
    ])
}

fn crew_value(c: &CrewMember) -> Value {
    let report = c.last_report.as_ref().map_or(Value::Nil, |r| {
        Value::record([
            ("id", Value::from(r.id.as_str())),
            ("state", word(&r.state)),
            ("summary", Value::from(r.summary.as_str())),
            ("at", secs(&r.at)),
        ])
    });
    Value::record([
        ("chat_id", Value::from(c.chat_id.as_str())),
        ("orchestrator_id", Value::from(c.orchestrator_id.as_str())),
        ("item_id", opt_str(c.item_id.as_deref())),
        ("role", Value::from(c.role.as_str())),
        ("last_report", report),
        ("released", Value::Bool(c.released)),
        ("joined", secs(&c.joined_at)),
    ])
}

fn link_value(l: &WorkLink) -> Value {
    Value::record([
        ("kind", word(&l.kind)),
        ("label", Value::from(l.label.as_str())),
        ("target", Value::from(l.target.as_str())),
    ])
}

fn item_value(i: &WorkItem) -> Value {
    let source = i.source.as_ref().map_or(Value::Nil, |s| {
        Value::record([("dump_id", Value::from(s.dump_id.as_str())), ("route_id", Value::from(s.route_id.as_str()))])
    });
    Value::record([
        ("id", Value::from(i.id.as_str())),
        ("orchestrator_id", opt_str(i.orchestrator_id.as_deref())),
        ("title", Value::from(i.title.as_str())),
        ("detail", Value::from(i.detail.as_str())),
        ("state", word(&i.state)),
        ("priority", word(&i.priority)),
        ("progress", opt(i.progress, fraction)),
        ("threads", strings(&i.threads)),
        ("links", list(&i.links, link_value)),
        ("outcome", opt_str(i.outcome.as_deref())),
        ("source", source),
        ("order", Value::from(i.order)),
        ("created", secs(&i.created_at)),
        ("updated", secs(&i.updated_at)),
        ("done", opt(i.done_at.as_ref(), secs)),
        // (Where it starts, before it's started; why a Next card waits.)
        ("space_id", opt_str(i.space_id.as_deref())),
        ("device_id", opt_str(i.device_id.as_deref())),
        ("waiting", opt_str(i.waiting.as_deref())),
        // (Paused by the person; the board check's note on it.)
        ("paused_at", opt(i.paused_at.as_ref(), secs)),
        ("check", i.check.as_ref().map_or(Value::Nil, check_value)),
        // (Where it stands in its orchestrator's flow; the cards it waits on.)
        ("flow", i.flow.as_ref().map_or(Value::Nil, |f| {
            Value::record([
                ("run", Value::from(f.run.as_str())),
                ("node", Value::from(f.node.as_str())),
                ("kind", word(&f.kind)),
                ("flow", Value::from(f.flow.as_str())),
                ("step", Value::from(f.step.as_str())),
                ("index", Value::from(f64::from(f.index))),
                ("of", Value::from(f64::from(f.of))),
            ])
        })),
        ("depends_on", strings(&i.depends_on)),
    ])
}

/// Where a card stands, as its engine worked it out.
fn card_value(c: &BoardCard) -> Value {
    let finish = Value::record([
        ("line", word(&c.finish.line)),
        ("branch", opt_str(c.finish.branch.as_deref())),
        ("into", opt_str(c.finish.into.as_deref())),
        ("pr", opt_str(c.finish.pr.as_deref())),
    ]);
    Value::record([
        ("id", Value::from(c.id.as_str())),
        ("place", word(&c.place)),
        ("status", word(&c.status)),
        ("note", opt_str(c.note.as_deref())),
        ("until", opt(c.until.as_ref(), secs)),
        ("since", secs(&c.since)),
        ("status_since", secs(&c.status_since)),
        // (Counts its moves: one that rises is a move to show.)
        ("seq", Value::from(c.seq as f64)),
        ("from", opt(c.from.as_ref(), word)),
        ("reason", Value::from(c.reason.as_str())),
        ("cause", word(&c.cause)),
        ("by", word(&c.by)),
        ("asks", strings(&c.asks)),
        ("chip", c.chip.as_ref().map_or(Value::Nil, |chip| {
            Value::record([
                ("kind", word(&chip.kind)),
                ("label", Value::from(chip.label.as_str())),
                ("target", opt_str(chip.target.as_deref())),
            ])
        })),
        ("finish", finish),
        ("closed", c.closed.as_ref().map_or(Value::Nil, |closed| {
            Value::record([
                ("at", secs(&closed.at)),
                ("by", word(&closed.by)),
                ("how", word(&closed.how)),
                ("note", opt_str(closed.note.as_deref())),
            ])
        })),
        ("held", Value::Bool(c.held)),
        ("work_at", opt(c.work_at.as_ref(), secs)),
        ("word", c.word.as_ref().map_or(Value::Nil, |w| {
            Value::record([
                ("state", word(&w.state)),
                ("at", secs(&w.at)),
                ("from", Value::from(w.from.as_str())),
                ("summary", Value::from(w.summary.as_str())),
                ("report", opt_str(w.report.as_deref())),
            ])
        })),
        ("guests", strings(&c.guests)),
        ("checking", c.checking.as_ref().map_or(Value::Nil, |k| {
            Value::record([("since", secs(&k.since)), ("why", word(&k.why))])
        })),
        ("seen_at", opt(c.seen_at.as_ref(), secs)),
        // (The engine moved it since the person last opened it.)
        ("unseen", Value::Bool(c.unseen())),
        ("events", list(&c.events, event_value)),
        ("old_state", word(&c.old_state())),
        ("updated", secs(&c.updated_at)),
    ])
}

/// A line of a card's "What happened".
fn event_value(e: &CardEvent) -> Value {
    Value::record([
        ("at", secs(&e.at)),
        ("kind", word(&e.kind)),
        ("text", Value::from(e.text.as_str())),
        ("by", opt(e.by.as_ref(), word)),
        ("from", opt(e.from.as_ref(), word)),
        ("to", opt(e.to.as_ref(), word)),
        ("seq", opt(e.seq, |n| Value::from(n as f64))),
        ("about", opt_str(e.about.as_deref())),
    ])
}

fn check_value(c: &zeron_proto::orchestration::CardCheck) -> Value {
    Value::record([
        ("kind", word(&c.kind)),
        ("note", Value::from(c.note.as_str())),
        ("since", secs(&c.since)),
        ("nudged_at", opt(c.nudged_at.as_ref(), secs)),
    ])
}

fn score_value(s: &Score) -> Value {
    match s {
        Score::Number(n) => Value::from(*n),
        Score::Text(t) => Value::from(t.as_str()),
    }
}

fn ask_value(a: &Ask) -> Value {
    let options = list(&a.options, |o| {
        Value::record([
            ("label", Value::from(o.label.as_str())),
            // (None when it has none, as the sample's.)
            ("detail", opt_str(Some(o.detail.as_str()).filter(|d| !d.is_empty()))),
            ("images", strings(&o.images)),
            // (In the order written: a list, not a table keyed by name.)
            (
                "scores",
                list(&o.scores.0, |(name, score)| {
                    Value::record([("name", Value::from(name.as_str())), ("value", score_value(score))])
                }),
            ),
        ])
    });
    let recommendation = a.recommendation.as_ref().map(|r| {
        Value::record([
            // (1-based, as Luau counts.)
            ("option", opt(r.option, |i| Value::from((i + 1) as f64))),
            ("label", opt_str(r.label.as_deref())),
            ("why", opt_str(r.why.as_deref())),
        ])
    });
    Value::record([
        ("id", Value::from(a.id.as_str())),
        ("from_chat", Value::from(a.from_chat.as_str())),
        ("orchestrator_id", opt_str(a.orchestrator_id.as_deref())),
        ("item_id", opt_str(a.item_id.as_deref())),
        ("kind", word(&a.kind)),
        ("question", Value::from(a.question.as_str())),
        ("options", options),
        ("urgent", Value::Bool(a.urgent)),
        ("state", word(&a.state)),
        ("answer", opt_str(a.answer.as_deref())),
        ("created", secs(&a.created_at)),
        ("answered", opt(a.answered_at.as_ref(), secs)),
        // A thread's question from its turn's end, not its `ask` tool.
        ("origin", word(&a.origin)),
        ("turn_id", opt_str(a.turn_id.as_deref())),
        ("updated", opt(a.updated_at.as_ref(), secs)),
        ("escalated", opt(a.escalated_at.as_ref(), secs)),
        ("answered_via", opt(a.answered_via.as_ref(), word)),
        // Shown without a banner, and on no card ("hasn't answered").
        ("quiet", Value::Bool(a.quiet)),
        ("images", strings(&a.images)),
        ("recommendation", recommendation.unwrap_or(Value::Nil)),
        ("links", list(&a.links, link_value)),
        ("context", opt_str(a.context.as_deref())),
        // Put off until then: out of Needs you meanwhile.
        ("deferred_until", opt(a.deferred_until.as_ref(), secs)),
    ])
}

/// Where a route went: `{ kind, id? }`.
fn target_value(t: &RouteTarget) -> Value {
    let (kind, id) = match t {
        RouteTarget::Orchestrator { id } => ("orchestrator", Some(id)),
        RouteTarget::Chat { id } => ("chat", Some(id)),
        RouteTarget::NewOrchestrator { id } => ("new_orchestrator", Some(id)),
        RouteTarget::NewThread { id } => ("new_thread", Some(id)),
        RouteTarget::Item { id } => ("item", Some(id)),
        RouteTarget::Note => ("note", None),
        RouteTarget::Unsure => ("unsure", None),
        RouteTarget::Other => ("other", None),
    };
    // (A candidate "new" place has no id yet: none, not "".)
    Value::record([("kind", Value::from(kind)), ("id", opt_str(id.map(String::as_str).filter(|id| !id.is_empty())))])
}

fn direct(r: &Route) -> bool {
    use zeron_proto::orchestration::RouteIntent;
    r.intent == RouteIntent::Question
        && zeron_proto::direct_question::settle_intent(RouteIntent::Question, &r.excerpt) == RouteIntent::Question
}

fn route_value(r: &Route) -> Value {
    let span = r.span.map_or(Value::Nil, |(from, to)| {
        Value::record([("from", Value::from(f64::from(from))), ("to", Value::from(f64::from(to)))])
    });
    Value::record([
        ("id", Value::from(r.id.as_str())),
        ("excerpt", Value::from(r.excerpt.as_str())),
        ("span", span),
        ("target", target_value(&r.target)),
        ("intent", word(&r.intent)),
        ("message", Value::from(r.message.as_str())),
        ("state", word(&r.state)),
        ("candidates", list(&r.candidates, target_value)),
        ("answer", opt_str(r.answer.as_deref())),
        ("answer_full", opt_str(r.answer_full.as_deref())),
        ("answered", opt(r.answered_at.as_ref(), secs)),
        // When it was last sent to its target (a question's "Answering" is
        // a turn begun since).
        ("delivered", opt(r.delivered_at.as_ref(), secs)),
        // A question the person wants answered (the Control room's Q&A):
        // tagged so, and not work or rhetoric in their words (checked here
        // too, for routes from before the engine checked).
        ("direct", Value::from(direct(r))),
        ("item_id", opt_str(r.item_id.as_deref())),
        // The files it went with (paths where its target reads them).
        ("attachments", Value::List(r.attachments.iter().map(|p| Value::from(p.as_str())).collect())),
    ])
}

fn dump_value(d: &Dump) -> Value {
    Value::record([
        ("id", Value::from(d.id.as_str())),
        ("text", Value::from(d.text.as_str())),
        ("state", word(&d.state)),
        ("routes", list(&d.routes, route_value)),
        ("summary", opt_str(d.summary.as_deref())),
        ("device", Value::from(d.device_id.as_str())),
        ("created", secs(&d.created_at)),
        ("updated", secs(&d.updated_at)),
        // The files the person dumped with it: `{ path, name }`.
        (
            "attachments",
            Value::List(
                d.attachments
                    .iter()
                    .map(|a| Value::record([("path", Value::from(a.path.as_str())), ("name", Value::from(a.name.as_str()))]))
                    .collect(),
            ),
        ),
    ])
}

/// The devices' loads, by device id (no history).
fn loads_value(loads: &[DeviceLoad]) -> Value {
    loads_value_with(loads, &History::default())
}

/// The devices' loads, by device id, each with its CPU line.
fn loads_value_with(loads: &[DeviceLoad], history: &History) -> Value {
    Value::record(loads.iter().map(|l| {
        let bytes = |b: Option<u64>| opt(b, |b| Value::from(b as f64));
        (
            l.device_id.as_str(),
            Value::record([
                ("cpu", opt(l.cpu, fraction)),
                ("mem_used", bytes(l.memory_used_bytes)),
                ("mem_total", bytes(l.memory_total_bytes)),
                ("load1", opt(l.load1, fraction)),
                ("cores", opt(l.cores, |n| Value::from(f64::from(n)))),
                ("online", Value::Bool(l.online)),
                ("at", secs(&l.at)),
                ("history", history.value(&l.device_id)),
            ]),
        )
    }))
}

fn comm_value(c: &CommEvent) -> Value {
    Value::record([
        ("id", Value::from(c.id.as_str())),
        ("kind", word(&c.kind)),
        ("from", Value::from(c.from.as_str())),
        ("to", Value::from(c.to.as_str())),
        ("summary", Value::from(c.summary.as_str())),
        ("at", secs(&c.at)),
    ])
}

/// What a chat's agent is doing, for its session row: `{ kind, label, since }`.
pub(crate) fn activity_value(a: &ChatActivity) -> Value {
    Value::record([("kind", word(&a.kind)), ("label", Value::from(a.label.as_str())), ("since", secs(&a.since))])
}

/// How far through its todo list a chat is, for its session row:
/// `{ done, total, current? }`.
pub(crate) fn todo_value(t: &TodoProgress) -> Value {
    Value::record([
        ("done", Value::from(f64::from(t.done))),
        ("total", Value::from(f64::from(t.total))),
        ("current", opt_str(t.current.as_deref())),
    ])
}

/// A chat its provider's usage limit stopped, for its session row:
/// `{ provider, account_slot?, account_label?, window?, until_at?, since,
/// attempt, switched_to? = { account_slot, account_label, at, harness?,
/// model? } }` (times in Unix seconds; `until_at`, `until` on the wire, nil
/// while unknown: `until` is a Luau keyword).
pub(crate) fn limited_value(l: &zeron_proto::usage_limit::SessionLimit) -> Value {
    let switched = l.switched_to.as_ref().map_or(Value::Nil, |s| {
        Value::record([
            ("account_slot", Value::from(s.account_slot.as_str())),
            ("account_label", Value::from(s.account_label.as_str())),
            ("at", secs(&s.at)),
            ("harness", opt_str(s.harness.as_deref())),
            ("model", opt_str(s.model.as_deref())),
        ])
    });
    Value::record([
        ("provider", Value::from(l.provider.as_str())),
        ("account_slot", opt_str(l.account_slot.as_deref())),
        ("account_label", opt_str(l.account_label.as_deref())),
        ("window", opt_str(l.window.as_deref())),
        ("until_at", opt(l.until.as_ref(), secs)),
        ("since", secs(&l.since)),
        ("attempt", Value::from(f64::from(l.attempt))),
        ("switched_to", switched),
    ])
}

// ── Pieces ──────────────────────────────────────────────────────────────────

/// An enum's wire word in snake_case (`brainDump` → `brain_dump`).
fn word<T: Serialize>(v: &T) -> Value {
    match serde_json::to_value(v) {
        Ok(Json::String(s)) => Value::from(snake(&s).as_str()),
        _ => Value::Nil,
    }
}

fn snake(camel: &str) -> String {
    let mut out = String::with_capacity(camel.len() + 4);
    for c in camel.chars() {
        if c.is_ascii_uppercase() {
            out.push('_');
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// Unix seconds.
fn secs(t: &DateTime<Utc>) -> Value {
    Value::from(t.timestamp() as f64)
}

/// A 0–1 (or load) figure, without f32's tail (0.71, not 0.7099999785).
fn fraction(x: f32) -> Value {
    Value::from((f64::from(x) * 1e6).round() / 1e6)
}

fn opt<T>(v: Option<T>, f: impl FnOnce(T) -> Value) -> Value {
    v.map_or(Value::Nil, f)
}

fn opt_str(v: Option<&str>) -> Value {
    v.map_or(Value::Nil, Value::from)
}

fn strings(v: &[String]) -> Value {
    Value::List(v.iter().map(|s| Value::from(s.as_str())).collect())
}

fn list<T>(v: &[T], f: impl Fn(&T) -> Value) -> Value {
    Value::List(v.iter().map(f).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_sends_only_the_person_s_ops() {
        for op in PERSON_OPS {
            assert!(person_op(&json!({ "op": op })).is_ok(), "{op}");
        }
        for op in ["report", "ask", "route", "dumpDone", "somethingNew"] {
            assert_eq!(person_op(&json!({ "op": op })), Err("not yours".to_string()), "{op}");
        }
        assert_eq!(person_op(&json!({ "text": "no op" })), Err("invalid".to_string()));
        // (The person starts an Inbox or Next card from the board.)
        assert!(person_op(&json!({ "op": "startItem", "id": "it-1" })).is_ok());
        // (The person pauses a card and resumes it: the Board's drag into and
        // out of Paused, Move to, its Resume button. Refused here, they never
        // reached the engine.)
        assert!(person_op(&json!({ "op": "pauseItem", "id": "it-1" })).is_ok());
        assert!(person_op(&json!({ "op": "resumeItem", "id": "it-1", "state": "active" })).is_ok());
        assert!(person_op(&json!({ "op": "updateSettings", "crewAutoArchive": true })).is_ok());
        // (What the person does to a card on the board: mark it done or
        // drop it, open it again, keep it for later, open it.)
        assert!(person_op(&json!({ "op": "closeItem", "id": "it-1", "dropped": true })).is_ok());
        assert!(person_op(&json!({ "op": "reopenItem", "id": "it-1" })).is_ok());
        assert!(person_op(&json!({ "op": "holdItem", "id": "it-1", "held": true })).is_ok());
        assert!(person_op(&json!({ "op": "seenItem", "id": "it-1" })).is_ok());
    }

    /// A delete waits for its Undo: undone, nothing is sent; not undone, it
    /// is sent once its wait is over, and only that wait's; still waiting
    /// when the app quits (its window closed, the scripts gone), it is sent
    /// then.
    #[test]
    fn a_delete_waits_for_its_undo_and_the_quit_sends_what_still_waits() {
        // Undone: never due, and nothing left for the quit.
        let t = remove_later("it-undone");
        assert!(undo_remove("it-undone"));
        assert!(!remove_due("it-undone", t));
        assert!(!undo_remove("it-undone"), "undone once");
        // Its wait over: due once.
        let t = remove_later("it-sent");
        assert!(remove_due("it-sent", t));
        assert!(!remove_due("it-sent", t));
        // Deleted, undone, deleted again: the first wait's end sends nothing.
        let first = remove_later("it-again");
        undo_remove("it-again");
        let second = remove_later("it-again");
        assert!(!remove_due("it-again", first));
        // Still waiting as the app quits: sent then, as `removeItem`.
        let _ = second;
        remove_later("it-closing");
        assert_eq!(removing_now(), vec!["it-again".to_string(), "it-closing".to_string()]);
        assert!(removing_now().is_empty());
        let op = remove_op("it-closing");
        assert!(person_op(&op).is_ok());
        assert!(matches!(
            serde_json::from_value::<zeron_proto::orchestration::OrchestrateOp>(op).unwrap(),
            zeron_proto::orchestration::OrchestrateOp::RemoveItem { id } if id == "it-closing"
        ));
    }

    /// A delete the engine refuses is told to the window that asked, with
    /// the engine's words (the board brings the card back and says why);
    /// with the window gone, only the log hears it.
    #[test]
    fn a_refused_delete_reaches_the_window_that_asked() {
        let (told, mut heard) = tokio::sync::oneshot::channel();
        assert!(tell_removal(told, "it-1", Err("That card is starting already".into())));
        let refused = removal_reply(Ok(heard.try_recv().unwrap()));
        assert!(matches!(&refused, Err(e) if e == "That card is starting already"));
        // Sent, and undone.
        let (told, mut heard) = tokio::sync::oneshot::channel();
        assert!(tell_removal(told, "it-1", Ok(true)));
        assert!(matches!(removal_reply(Ok(heard.try_recv().unwrap())), Ok(Value::Bool(true))));
        assert!(matches!(removal_reply(Ok(Ok(false))), Ok(Value::Nil)));
        // The window gone: nobody to tell, nothing breaks.
        let (told, heard) = tokio::sync::oneshot::channel::<Result<bool, String>>();
        drop(heard);
        assert!(!tell_removal(told, "it-1", Err("refused".into())));
        // No engine to send it to (its task dropped): left for the quit.
        let (told, heard) = tokio::sync::oneshot::channel::<Result<bool, String>>();
        drop(told);
        let gone = tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(heard);
        assert!(matches!(removal_reply(gone), Ok(Value::Nil)));
    }

    /// Every op the board sends for what the person does to a card, as
    /// account/orchestration.luau builds it (ui/tests/orch_board_ops.test.luau
    /// checks the scripts build these same tables): the bridge lets it
    /// through, and the engine reads it as the op it means, with its fields.
    #[test]
    fn every_board_action_is_the_person_s_and_reads_as_the_engine_s_op() {
        use zeron_proto::orchestration::{OrchestrateOp as Op, RefKind, WorkState};
        let at = (NOW + 3600) * 1000;
        let deferred = defer_params(&Value::record([("id", Value::from("ak-1")), ("until", Value::from((at / 1000) as f64))])).unwrap();
        let answered = answer_params(&Value::record([("id", Value::from("ak-1")), ("option", Value::from(1.0))])).unwrap();
        let back = defer_params(&Value::record([("id", Value::from("ak-1"))])).unwrap();
        let cases: Vec<(Json, Box<dyn Fn(&Op) -> bool>)> = vec![
            (json!({ "op": "closeItem", "id": "it-1" }), Box::new(|op| matches!(op, Op::CloseItem { id, reason: None, dropped: false } if id == "it-1"))),
            (json!({ "op": "closeItem", "id": "it-1", "dropped": true }), Box::new(|op| matches!(op, Op::CloseItem { dropped: true, .. }))),
            (json!({ "op": "reopenItem", "id": "it-1" }), Box::new(|op| matches!(op, Op::ReopenItem { id } if id == "it-1"))),
            (json!({ "op": "holdItem", "id": "it-1", "held": true }), Box::new(|op| matches!(op, Op::HoldItem { held: true, .. }))),
            (json!({ "op": "holdItem", "id": "it-1", "held": false }), Box::new(|op| matches!(op, Op::HoldItem { held: false, .. }))),
            (json!({ "op": "seenItem", "id": "it-1" }), Box::new(|op| matches!(op, Op::SeenItem { id } if id == "it-1"))),
            (json!({ "op": "pauseItem", "id": "it-1" }), Box::new(|op| matches!(op, Op::PauseItem { .. }))),
            // (Resume into Running: the engine's `active`, its threads told to carry on.)
            (
                json!({ "op": "resumeItem", "id": "it-1", "state": "active" }),
                Box::new(|op| matches!(op, Op::ResumeItem { state: Some(WorkState::Active), .. })),
            ),
            (json!({ "op": "startItem", "id": "it-1" }), Box::new(|op| matches!(op, Op::StartItem { id } if id == "it-1"))),
            (json!({ "op": "removeItem", "id": "it-1" }), Box::new(|op| matches!(op, Op::RemoveItem { .. }))),
            (
                json!({ "op": "requestChanges", "id": "it-1", "text": "tighten it" }),
                Box::new(|op| matches!(op, Op::RequestChanges { text, .. } if text == "tighten it")),
            ),
            // (Delete's Undo: the card made again, no `state` on it.)
            (
                json!({ "op": "upsertItem", "item": { "orchestratorId": "orc-1", "title": "T", "detail": "D", "priority": "normal",
                    "threads": ["th-1"], "links": [], "order": 3 } }),
                Box::new(|op| matches!(op, Op::UpsertItem { item, ask: None } if item.id.is_none() && item.state.is_none() && item.title.as_deref() == Some("T"))),
            ),
            // (A Luau table that is empty arrives as a list.)
            (
                json!({ "op": "upsertItem", "item": { "title": "T", "detail": "", "threads": [], "links": [] } }),
                Box::new(|op| matches!(op, Op::UpsertItem { .. })),
            ),
            (
                json!({ "op": "dump", "text": "Speed up the sidebar", "refs": [{ "kind": "orchestrator", "id": "orc-ci", "label": "CI speed" }] }),
                Box::new(|op| {
                    matches!(op, Op::Dump { text, refs, .. } if text == "Speed up the sidebar" && refs.len() == 1 && refs[0].kind == RefKind::Orchestrator && refs[0].id == "orc-ci")
                }),
            ),
            (json!({ "op": "dump", "text": "Look at the logs" }), Box::new(|op| matches!(op, Op::Dump { refs, .. } if refs.is_empty()))),
            (
                deferred,
                Box::new(move |op| matches!(op, Op::DeferAsk { id, until: Some(u) } if id == "ak-1" && u.timestamp_millis() == at)),
            ),
            (back, Box::new(|op| matches!(op, Op::DeferAsk { until: None, .. }))),
            (answered, Box::new(|op| matches!(op, Op::AnswerAsk { option: Some(0), .. }))),
        ];
        for (params, is) in &cases {
            assert!(person_op(params).is_ok(), "refused by the bridge: {params}");
            let op: Op = serde_json::from_value(params.clone()).unwrap_or_else(|e| panic!("{params}: {e}"));
            assert!(is(&op), "{params} read as {op:?}");
        }
    }

    /// The ops an agent sends (through Zeron's tools) never come from the
    /// window; every other op the engine has is the person's.
    #[test]
    fn only_an_agent_s_ops_are_refused_from_the_window() {
        const AGENT_ONLY: &[&str] = &["report", "ask", "route", "dumpDone"];
        for op in AGENT_ONLY {
            assert_eq!(person_op(&json!({ "op": op })), Err("not yours".to_string()), "{op}");
            assert!(!PERSON_OPS.contains(op), "{op}");
        }
        // (Each op the engine reads, by its wire name: the person's or an agent's.)
        let source = include_str!("../../../crates/proto/src/orchestration.rs");
        let start = source.find("pub enum OrchestrateOp {").expect("OrchestrateOp");
        let body = &source[start..];
        let body = &body[..body.find("\n}\n").expect("its end")];
        let mut names = Vec::new();
        for line in body.lines() {
            let t = line.strip_prefix("    ").unwrap_or_default();
            if t.starts_with(|c: char| c.is_ascii_uppercase()) {
                let name: String = t.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
                let mut wire = name.clone();
                wire[..1].make_ascii_lowercase();
                names.push(wire);
            }
        }
        assert!(names.len() > 20, "{names:?}");
        for op in ["closeItem", "reopenItem", "holdItem", "seenItem", "pauseItem", "resumeItem", "startItem", "removeItem"] {
            assert!(names.iter().any(|n| n == op), "{op} is no op of the engine's");
        }
        for name in &names {
            assert!(
                PERSON_OPS.contains(&name.as_str()) || AGENT_ONLY.contains(&name.as_str()),
                "{name}: neither the person's (PERSON_OPS) nor listed as an agent's"
            );
        }
        for op in PERSON_OPS {
            assert!(names.iter().any(|n| n == op), "{op} in PERSON_OPS is no op of the engine's");
        }
    }

    /// A card's place as the engine worked it out reaches scripts whole, and
    /// a move of one card makes only that card's row again.
    #[test]
    fn a_card_s_place_reads_for_the_scripts_and_a_move_touches_only_it() {
        let mut wire = sample();
        wire["cards"] = json!([
            { "id": "it-titlebar", "place": "done", "status": "done", "since": iso(NOW - 26_000),
              "statusSince": iso(NOW - 26_000), "seq": 3, "from": "running", "reason": "Merged in e99f817",
              "cause": "merged", "by": "engine",
              "chip": { "kind": "merged", "label": "merged e99f817" },
              "finish": { "line": "merged", "branch": "zeron/titlebar", "into": "zeron/orchestrators" },
              "closed": { "at": iso(NOW - 26_000), "by": "engine", "how": "merged" },
              "word": { "state": "done", "at": iso(NOW - 27_000), "from": "th-titlebar", "summary": "Toggle lands" },
              "events": [
                  { "at": iso(NOW - 29_000), "kind": "move", "text": "Thread started", "by": "agent",
                    "from": "planned", "to": "running", "seq": 1 },
                  { "at": iso(NOW - 27_000), "kind": "report", "text": "Reported done: Toggle lands", "about": "rp-9" },
              ],
              "updatedAt": iso(NOW - 26_000) },
            { "id": "it-live", "place": "needsYou", "status": "asked", "since": iso(NOW - 300),
              "statusSince": iso(NOW - 300), "seq": 1, "asks": ["ak-9"], "note": "Answer in its thread",
              "until": iso(NOW + 600), "held": true, "seenAt": iso(NOW - 100), "updatedAt": iso(NOW - 300) },
        ]);
        let s1 = OrchestrationState::deserialize(&wire).unwrap();
        let v = state_value(&s1);
        let done = row(&v, "cards", "id", "it-titlebar");
        assert_eq!(text(done.field("place")).as_deref(), Some("done"));
        assert_eq!(text(done.field("from")).as_deref(), Some("running"));
        assert_eq!(text(done.field("cause")).as_deref(), Some("merged"));
        assert_eq!(text(done.field("by")).as_deref(), Some("engine"));
        assert_eq!(num(done.field("seq")), Some(3.0));
        assert_eq!(text(done.field("old_state")).as_deref(), Some("done"));
        assert_eq!(text(done.field("chip").unwrap().field("label")).as_deref(), Some("merged e99f817"));
        let finish = done.field("finish").unwrap();
        assert_eq!(text(finish.field("line")).as_deref(), Some("merged"));
        assert_eq!(text(finish.field("into")).as_deref(), Some("zeron/orchestrators"));
        assert_eq!(text(done.field("closed").unwrap().field("how")).as_deref(), Some("merged"));
        assert_eq!(text(done.field("word").unwrap().field("summary")).as_deref(), Some("Toggle lands"));
        // (Moved by the engine and never opened: its dot.)
        assert_eq!(done.field("unseen"), Some(&Value::Bool(true)));
        let events = done.field("events").unwrap().list();
        assert_eq!(events.len(), 2);
        assert_eq!(text(events[0].field("to")).as_deref(), Some("running"));
        assert_eq!(text(events[1].field("kind")).as_deref(), Some("report"));
        let asked = row(&v, "cards", "id", "it-live");
        assert_eq!(text(asked.field("place")).as_deref(), Some("needs_you"));
        assert_eq!(text(asked.field("status")).as_deref(), Some("asked"));
        assert_eq!(text(asked.field("old_state")).as_deref(), Some("blocked"));
        assert_eq!(num(asked.field("until")), Some((NOW + 600) as f64));
        assert_eq!(asked.field("held"), Some(&Value::Bool(true)));
        assert_eq!(text(asked.field("asks").unwrap().list().first()).as_deref(), Some("ak-9"));
        assert!(nil(asked.field("closed")) && nil(asked.field("chip")));

        // It is answered and its thread carries on: one card's row.
        let mut s2 = s1.clone();
        let live = s2.cards.iter_mut().find(|c| c.id == "it-live").unwrap();
        live.place = zeron_proto::orchestration::board::Place::Running;
        live.status = zeron_proto::orchestration::board::CardStatus::Working;
        live.seq = 2;
        let frames = frames(&[s1, s2.clone()]);
        assert_eq!(frames.len(), 2);
        assert!(frames[1].get("items").is_none(), "only the card's row goes");
        let o = Orchestration::default();
        o.heard_state(frames[0].clone());
        let before = o.now_value();
        o.heard_state(frames[1].clone());
        let after = o.now_value();
        assert_eq!(after, state_value(&s2));
        assert_eq!(text(row(&after, "cards", "id", "it-live").field("place")).as_deref(), Some("running"));
        assert!(!same(row(&before, "cards", "id", "it-live"), row(&after, "cards", "id", "it-live")));
        assert!(same(row(&before, "cards", "id", "it-titlebar"), row(&after, "cards", "id", "it-titlebar")));
        for list in ["orchestrators", "crew", "items", "asks", "dumps"] {
            assert!(same(before.field(list).unwrap(), after.field(list).unwrap()), "{list} kept whole");
        }
    }

    #[test]
    fn a_flow_step_s_card_and_its_orchestrator_s_run_read_for_the_scripts() {
        let item: WorkItem = serde_json::from_value(json!({
            "id": "c", "title": "QA: t", "state": "active", "priority": "normal", "createdAt": iso(NOW), "updatedAt": iso(NOW),
            "flow": { "run": "r1", "node": "6", "kind": "qa", "flow": "Plan", "step": "QA", "index": 4, "of": 4 },
            "dependsOn": ["v"],
        }))
        .unwrap();
        let v = item_value(&item);
        let f = v.field("flow").unwrap();
        assert_eq!(text(f.field("step")).as_deref(), Some("QA"));
        assert_eq!(text(f.field("kind")).as_deref(), Some("qa"));
        assert_eq!(text(v.field("depends_on").unwrap().list().first()).as_deref(), Some("v"));
        let o: Orchestrator = serde_json::from_value(json!({
            "id": "o", "kind": "standard", "name": "O", "hue": 1, "createdAt": iso(NOW), "updatedAt": iso(NOW),
            "workflow": { "version": 2, "name": "Plan", "next_id": 3, "nodes": [
                { "id": "1", "kind": "chief", "name": "Entry" }, { "id": "2", "kind": "finish", "name": "Finish" }],
                "edges": [{ "from": "1", "to": "2", "condition": "always" }] },
            "flowRun": { "id": "r1", "flow": { "version": 2, "name": "Plan", "nodes": [], "edges": [] }, "task": "T",
                "state": "blocked", "cards": { "6": "c" }, "returns": { "6>4": 2 }, "at": "6", "startedAt": iso(NOW) },
        }))
        .unwrap();
        let v = orchestrator_value(&o);
        let w = v.field("workflow").unwrap();
        // (The Flow view's own shape: its entry is "chief".)
        assert_eq!(text(w.field("nodes").unwrap().list().first().unwrap().field("kind")).as_deref(), Some("chief"));
        let run = v.field("flow_run").unwrap();
        assert_eq!(text(run.field("state")).as_deref(), Some("blocked"));
        assert_eq!(text(run.field("cards").unwrap().field("6")).as_deref(), Some("c"));
        assert_eq!(text(run.field("at")).as_deref(), Some("6"));
    }

    #[test]
    fn a_waiting_card_says_where_it_starts_and_why_it_waits() {
        let item: WorkItem = serde_json::from_value(json!({
            "id": "x", "title": "t", "state": "planned", "priority": "normal", "createdAt": iso(NOW), "updatedAt": iso(NOW),
            "spaceId": "sp-1", "deviceId": "dev-1", "waiting": "Auth's crew is full: 2 of 2 working",
        }))
        .unwrap();
        let v = item_value(&item);
        assert_eq!(text(v.field("space_id")).as_deref(), Some("sp-1"));
        assert_eq!(text(v.field("device_id")).as_deref(), Some("dev-1"));
        assert_eq!(text(v.field("waiting")).as_deref(), Some("Auth's crew is full: 2 of 2 working"));
        // (None of it: nil, as the sample's.)
        let bare: WorkItem = serde_json::from_value(json!({
            "id": "y", "title": "t", "state": "inbox", "priority": "normal", "createdAt": iso(NOW), "updatedAt": iso(NOW),
        }))
        .unwrap();
        let v = item_value(&bare);
        assert!(nil(v.field("space_id")) && nil(v.field("device_id")) && nil(v.field("waiting")));
    }

    #[test]
    fn a_preview_can_t_drive_orchestration() {
        use crate::plugins::preview;
        preview::set(Some(preview::Preview { id: "sleek".into(), name: "Sleek".into() }));
        assert!(preview::refuse("orchestration.op").is_err());
        assert!(preview::refuse("orchestration.loads_wanted").is_err());
        preview::set(None);
        assert!(preview::refuse("orchestration.op").is_ok());
    }

    const NOW: i64 = 1_794_470_400;

    fn iso(secs: i64) -> String {
        DateTime::<Utc>::from_timestamp(secs, 0).unwrap().to_rfc3339()
    }

    fn text(v: Option<&Value>) -> Option<String> {
        v.map(crate::auth::text)
    }

    fn num(v: Option<&Value>) -> Option<f64> {
        v.and_then(Value::as_f64)
    }

    fn nil(v: Option<&Value>) -> bool {
        v.is_none_or(|v| matches!(v, Value::Nil))
    }

    fn by_id<'a>(list: &'a Value, key: &str, id: &str) -> &'a Value {
        list.list().iter().find(|v| text(v.field(key)).as_deref() == Some(id)).unwrap()
    }

    /// The sample's world (ui/account/orchestration_sample.luau), a part of
    /// each kind, as the engine sends it.
    fn sample() -> Json {
        json!({
            "orchestrators": [
                { "id": "orc-brain", "kind": "brainDump", "name": "Brain dump", "goal": "Route everything.",
                  "notes": "- Ask Wing", "hue": 262, "crew": {}, "createdAt": iso(NOW - 518_400), "updatedAt": iso(NOW - 40) },
                { "id": "orc-vpn", "kind": "standard", "name": "Exnomic VPN", "goal": "Phase 2 migration, safely.",
                  "rules": "Nothing touches prod without asking.", "crewRules": "Own worktree.", "hue": 32,
                  "crew": { "devices": ["dev-desk", "dev-mac"], "maxRunning": 4, "model": "claude-opus-5-5" },
                  "wake": { "everyMinutes": 30, "at": ["09:00"], "weekdaysOnly": true, "prompt": "Check node health.",
                            "lastWokeAt": iso(NOW - 600) },
                  "paused": true, "createdAt": iso(NOW - 259_200), "updatedAt": iso(NOW - 3600) },
            ],
            "crew": [
                { "chatId": "th-vpn-moscow", "orchestratorId": "orc-vpn", "itemId": "it-moscow", "role": "reboot plan",
                  "lastReport": { "id": "rp-4", "state": "done", "summary": "Reboot plan ready", "at": iso(NOW - 3500) },
                  "joinedAt": iso(NOW - 7000) },
                { "chatId": "th-vpn-merge", "orchestratorId": "orc-vpn", "role": "merge", "joinedAt": iso(NOW - 1800) },
            ],
            "items": [
                { "id": "it-titlebar", "title": "Linux titlebar toggle", "state": "done", "priority": "normal", "progress": 0.71,
                  "threads": ["th-titlebar"], "links": [{ "kind": "pr", "label": "#1383", "target": "https://github.com/zeronsh/zeron/pull/1383" }],
                  "outcome": "PR #1383 open.", "source": { "dumpId": "dp-2", "routeId": "rt-6" }, "order": 2,
                  "createdAt": iso(NOW - 30_000), "updatedAt": iso(NOW - 26_000), "doneAt": iso(NOW - 26_000) },
                { "id": "it-live", "orchestratorId": "orc-orch", "title": "Live test", "state": "planned", "priority": "urgent",
                  "createdAt": iso(NOW - 5300), "updatedAt": iso(NOW - 5300) },
            ],
            "asks": [
                { "id": "ak-2", "fromChat": "orc-vpn", "orchestratorId": "orc-vpn", "itemId": "it-moscow", "kind": "approval",
                  "question": "Reboot the Moscow node?", "options": [{ "label": "Approve" }, { "label": "Not tonight", "detail": "Tomorrow" }],
                  "urgent": true, "state": "open", "createdAt": iso(NOW - 3400) },
                { "id": "ak-3", "fromChat": "th-ci-cache", "kind": "question", "question": "Which bucket?", "state": "answered",
                  "answer": "zeron-ci-cache-us", "createdAt": iso(NOW - 95_000), "answeredAt": iso(NOW - 91_000) },
            ],
            "dumps": [
                { "id": "dp-1", "text": "make the board show PR checks and look at why zgx eats 2gb", "state": "needsYou",
                  "summary": "1 part placed, 1 unsure.", "deviceId": "dev-mac", "createdAt": iso(NOW - 7200), "updatedAt": iso(NOW - 7100),
                  "routes": [
                      { "id": "rt-1", "excerpt": "make the board show PR checks", "span": [0, 29],
                        "target": { "kind": "orchestrator", "id": "orc-orch" }, "intent": "task", "message": "Show PR checks.",
                        "state": "delivered", "itemId": "it-checks" },
                      { "id": "rt-2", "excerpt": "look at why zgx eats 2gb", "target": { "kind": "unsure" }, "intent": "task",
                        "state": "pending",
                        "candidates": [{ "kind": "orchestrator", "id": "orc-orch" }, { "kind": "newThread", "id": "" },
                                       { "kind": "newOrchestrator", "id": "orc-new" }] },
                  ] },
            ],
        })
    }

    #[test]
    fn the_state_reads_as_the_sample_does() {
        let state = OrchestrationState::deserialize(&sample()).unwrap();
        let v = state_value(&state);
        assert_eq!(text(v.field("state")).as_deref(), Some("ready"));

        let orchestrators = v.field("orchestrators").unwrap();
        let brain = by_id(orchestrators, "id", "orc-brain");
        assert_eq!(text(brain.field("kind")).as_deref(), Some("brain_dump"));
        assert_eq!(text(brain.field("rules")).as_deref(), Some(""));
        assert!(nil(brain.field("wake")));
        assert!(nil(brain.field("crew").unwrap().field("max_running")));
        assert!(brain.field("crew").unwrap().field("devices").unwrap().list().is_empty());
        let vpn = by_id(orchestrators, "id", "orc-vpn");
        assert_eq!(text(vpn.field("kind")).as_deref(), Some("standard"));
        assert_eq!(text(vpn.field("crew_rules")).as_deref(), Some("Own worktree."));
        assert_eq!(num(vpn.field("hue")), Some(32.));
        assert_eq!(vpn.field("paused"), Some(&Value::Bool(true)));
        assert_eq!(vpn.field("archived"), Some(&Value::Bool(false)));
        assert_eq!(num(vpn.field("created")), Some((NOW - 259_200) as f64));
        assert_eq!(num(vpn.field("updated")), Some((NOW - 3600) as f64));
        let crew = vpn.field("crew").unwrap();
        assert_eq!(num(crew.field("max_running")), Some(4.));
        assert_eq!(crew.field("devices").unwrap().list().len(), 2);
        assert_eq!(text(crew.field("model")).as_deref(), Some("claude-opus-5-5"));
        assert!(nil(crew.field("harness")));
        let wake = vpn.field("wake").unwrap();
        assert_eq!(num(wake.field("every_minutes")), Some(30.));
        assert_eq!(wake.field("weekdays_only"), Some(&Value::Bool(true)));
        assert_eq!(text(wake.field("at").unwrap().list().first()).as_deref(), Some("09:00"));
        assert_eq!(num(wake.field("last_woke")), Some((NOW - 600) as f64));

        let crew = v.field("crew").unwrap();
        let moscow = by_id(crew, "chat_id", "th-vpn-moscow");
        assert_eq!(text(moscow.field("orchestrator_id")).as_deref(), Some("orc-vpn"));
        assert_eq!(text(moscow.field("item_id")).as_deref(), Some("it-moscow"));
        assert_eq!(num(moscow.field("joined")), Some((NOW - 7000) as f64));
        assert_eq!(moscow.field("released"), Some(&Value::Bool(false)));
        let report = moscow.field("last_report").unwrap();
        assert_eq!(text(report.field("state")).as_deref(), Some("done"));
        assert_eq!(text(report.field("summary")).as_deref(), Some("Reboot plan ready"));
        assert_eq!(num(report.field("at")), Some((NOW - 3500) as f64));
        let merge = by_id(crew, "chat_id", "th-vpn-merge");
        assert!(nil(merge.field("last_report")));
        assert!(nil(merge.field("item_id")));

        let items = v.field("items").unwrap();
        let titlebar = by_id(items, "id", "it-titlebar");
        assert!(nil(titlebar.field("orchestrator_id")));
        assert_eq!(text(titlebar.field("state")).as_deref(), Some("done"));
        assert_eq!(text(titlebar.field("priority")).as_deref(), Some("normal"));
        // (f32's 0.71 reads as 0.71, not 0.7099999785.)
        assert_eq!(num(titlebar.field("progress")), Some(0.71));
        assert_eq!(num(titlebar.field("order")), Some(2.));
        assert_eq!(num(titlebar.field("done")), Some((NOW - 26_000) as f64));
        assert_eq!(text(titlebar.field("detail")).as_deref(), Some(""));
        let source = titlebar.field("source").unwrap();
        assert_eq!(text(source.field("dump_id")).as_deref(), Some("dp-2"));
        assert_eq!(text(source.field("route_id")).as_deref(), Some("rt-6"));
        let link = &titlebar.field("links").unwrap().list()[0];
        assert_eq!(text(link.field("kind")).as_deref(), Some("pr"));
        assert_eq!(text(link.field("label")).as_deref(), Some("#1383"));
        assert_eq!(text(titlebar.field("threads").unwrap().list().first()).as_deref(), Some("th-titlebar"));
        let live = by_id(items, "id", "it-live");
        assert_eq!(text(live.field("priority")).as_deref(), Some("urgent"));
        for key in ["progress", "outcome", "source", "done"] {
            assert!(nil(live.field(key)), "{key}");
        }
        assert!(live.field("links").unwrap().list().is_empty());
        assert!(live.field("threads").unwrap().list().is_empty());

        let asks = v.field("asks").unwrap();
        let approval = by_id(asks, "id", "ak-2");
        assert_eq!(text(approval.field("kind")).as_deref(), Some("approval"));
        assert_eq!(text(approval.field("from_chat")).as_deref(), Some("orc-vpn"));
        assert_eq!(approval.field("urgent"), Some(&Value::Bool(true)));
        assert_eq!(text(approval.field("state")).as_deref(), Some("open"));
        assert!(nil(approval.field("answer")));
        assert!(nil(approval.field("answered")));
        let options = approval.field("options").unwrap().list();
        assert_eq!(text(options[0].field("label")).as_deref(), Some("Approve"));
        assert!(nil(options[0].field("detail")));
        assert_eq!(text(options[1].field("detail")).as_deref(), Some("Tomorrow"));
        let answered = by_id(asks, "id", "ak-3");
        assert_eq!(text(answered.field("answer")).as_deref(), Some("zeron-ci-cache-us"));
        assert_eq!(num(answered.field("answered")), Some((NOW - 91_000) as f64));
        assert!(answered.field("options").unwrap().list().is_empty());
        assert!(nil(answered.field("orchestrator_id")));

        let dump = &v.field("dumps").unwrap().list()[0];
        assert_eq!(text(dump.field("state")).as_deref(), Some("needs_you"));
        assert_eq!(text(dump.field("device")).as_deref(), Some("dev-mac"));
        assert_eq!(num(dump.field("created")), Some((NOW - 7200) as f64));
        assert_eq!(text(dump.field("summary")).as_deref(), Some("1 part placed, 1 unsure."));
        let routes = dump.field("routes").unwrap();
        let placed = by_id(routes, "id", "rt-1");
        let span = placed.field("span").unwrap();
        assert_eq!((num(span.field("from")), num(span.field("to"))), (Some(0.), Some(29.)));
        assert_eq!(text(placed.field("target").unwrap().field("kind")).as_deref(), Some("orchestrator"));
        assert_eq!(text(placed.field("target").unwrap().field("id")).as_deref(), Some("orc-orch"));
        assert_eq!(text(placed.field("intent")).as_deref(), Some("task"));
        assert_eq!(text(placed.field("state")).as_deref(), Some("delivered"));
        assert_eq!(text(placed.field("item_id")).as_deref(), Some("it-checks"));
        assert!(nil(placed.field("answer")));
        let unsure = by_id(routes, "id", "rt-2");
        assert!(nil(unsure.field("span")));
        assert_eq!(text(unsure.field("target").unwrap().field("kind")).as_deref(), Some("unsure"));
        assert!(nil(unsure.field("target").unwrap().field("id")));
        assert_eq!(text(unsure.field("message")).as_deref(), Some(""));
        assert_eq!(text(unsure.field("state")).as_deref(), Some("pending"));
        let candidates = unsure.field("candidates").unwrap().list();
        let kinds: Vec<_> = candidates.iter().map(|c| text(c.field("kind")).unwrap()).collect();
        assert_eq!(kinds, ["orchestrator", "new_thread", "new_orchestrator"]);
        // (A new place's empty id is none.)
        assert!(nil(candidates[1].field("id")));
        assert_eq!(text(candidates[2].field("id")).as_deref(), Some("orc-new"));
    }

    #[test]
    fn words_are_snake_case() {
        assert_eq!(snake("brainDump"), "brain_dump");
        assert_eq!(snake("needsYou"), "needs_you");
        assert_eq!(snake("newOrchestrator"), "new_orchestrator");
        assert_eq!(snake("other"), "other");
        // An unknown word from a newer peer reads as "other".
        let item: WorkItem = serde_json::from_value(json!({
            "id": "x", "title": "t", "state": "someday", "priority": "low", "createdAt": iso(NOW), "updatedAt": iso(NOW),
        }))
        .unwrap();
        assert_eq!(text(item_value(&item).field("state")).as_deref(), Some("other"));
    }

    #[test]
    fn a_turn_end_ask_reads_with_its_origin_and_how_it_closed() {
        let ask: Ask = serde_json::from_value(json!({
            "id": "turn-c-m", "fromChat": "c", "kind": "decision", "question": "Should I A, or B?", "state": "answered",
            "createdAt": iso(NOW - 3600), "origin": "turnEnd", "turnId": "m", "escalatedAt": iso(NOW - 1800),
            "answeredAt": iso(NOW), "answeredVia": "thread",
        }))
        .unwrap();
        let v = ask_value(&ask);
        assert_eq!(text(v.field("origin")).as_deref(), Some("turn_end"));
        assert_eq!(text(v.field("turn_id")).as_deref(), Some("m"));
        assert_eq!(num(v.field("escalated")), Some((NOW - 1800) as f64));
        assert_eq!(text(v.field("answered_via")).as_deref(), Some("thread"));
        assert!(nil(v.field("updated")));
        // One the `ask` tool raised: "tool", nothing else.
        let tool: Ask = serde_json::from_value(json!({
            "id": "a", "fromChat": "c", "kind": "question", "question": "?", "state": "open", "createdAt": iso(NOW),
        }))
        .unwrap();
        let v = ask_value(&tool);
        assert_eq!(text(v.field("origin")).as_deref(), Some("tool"));
        assert!(nil(v.field("escalated")) && nil(v.field("answered_via")));
        assert!(matches!(v.field("quiet"), Some(Value::Bool(false))));
    }

    #[test]
    fn a_decision_reads_with_its_pictures_scores_pick_and_deferral() {
        let ask: Ask = serde_json::from_value(json!({
            "id": "a", "fromChat": "c", "kind": "decision", "question": "Which?", "state": "open", "createdAt": iso(NOW),
            "options": [
                { "label": "(a) Tag", "detail": "No lines", "images": ["/u/1-tag.png"], "scores": { "Claude": 8, "risk": "low" } },
                { "label": "(b) Edge" }
            ],
            "images": ["https://x/now.png"],
            "recommendation": { "option": 0, "label": "(a) Tag", "why": "Cleaner." },
            "links": [{ "kind": "branch", "label": "orch/x", "target": "orch/x" }],
            "context": "Built on `c1e`.", "deferredUntil": iso(NOW + 3600),
        }))
        .unwrap();
        let v = ask_value(&ask);
        let options = v.field("options").unwrap().list();
        let a = &options[0];
        assert_eq!(text(a.field("images").unwrap().list().first()).as_deref(), Some("/u/1-tag.png"));
        let scores = a.field("scores").unwrap().list();
        assert_eq!(text(scores[0].field("name")).as_deref(), Some("Claude"));
        assert_eq!(num(scores[0].field("value")), Some(8.0));
        assert_eq!(text(scores[1].field("value")).as_deref(), Some("low"));
        assert!(options[1].field("images").unwrap().list().is_empty() && nil(options[1].field("detail")));
        assert_eq!(text(v.field("images").unwrap().list().first()).as_deref(), Some("https://x/now.png"));
        let pick = v.field("recommendation").unwrap();
        assert_eq!(num(pick.field("option")), Some(1.0), "1-based");
        assert_eq!(text(pick.field("why")).as_deref(), Some("Cleaner."));
        assert_eq!(text(v.field("links").unwrap().list()[0].field("target")).as_deref(), Some("orch/x"));
        assert_eq!(text(v.field("context")).as_deref(), Some("Built on `c1e`."));
        assert_eq!(num(v.field("deferred_until")), Some((NOW + 3600) as f64));
        // An old ask: empty lists, nils.
        let old: Ask = serde_json::from_value(json!({
            "id": "b", "fromChat": "c", "kind": "question", "question": "?", "state": "open", "createdAt": iso(NOW),
        }))
        .unwrap();
        let v = ask_value(&old);
        assert!(v.field("images").unwrap().list().is_empty() && v.field("links").unwrap().list().is_empty());
        assert!(nil(v.field("recommendation")) && nil(v.field("context")) && nil(v.field("deferred_until")));
    }

    #[test]
    fn answering_with_an_option_and_deferring_make_the_engines_ops() {
        let args = Value::record([("id", Value::from("a")), ("option", Value::from(2.0))]);
        assert_eq!(answer_params(&args).unwrap(), json!({ "op": "answerAsk", "id": "a", "answer": "", "option": 1 }));
        let args = Value::record([("id", Value::from("a")), ("option", Value::from(1.0)), ("text", Value::from(" and polish "))]);
        assert_eq!(answer_params(&args).unwrap(), json!({ "op": "answerAsk", "id": "a", "answer": "and polish", "option": 0 }));
        let args = Value::record([("id", Value::from("a")), ("text", Value::from("go"))]);
        assert_eq!(answer_params(&args).unwrap(), json!({ "op": "answerAsk", "id": "a", "answer": "go" }));
        assert!(answer_params(&Value::record([("id", Value::from("a"))])).is_err(), "an option or words");
        assert!(answer_params(&Value::record([("id", Value::from("a")), ("option", Value::from(0.0))])).is_err(), "1-based");
        assert!(answer_params(&Value::record([("option", Value::from(1.0))])).is_err(), "an id");
        let args = Value::record([("id", Value::from("a")), ("until", Value::from(NOW as f64))]);
        let op = defer_params(&args).unwrap();
        assert_eq!(op["op"], "deferAsk");
        let until: DateTime<Utc> = op["until"].as_str().unwrap().parse().unwrap();
        assert_eq!(until.timestamp(), NOW);
        assert_eq!(defer_params(&Value::record([("id", Value::from("a"))])).unwrap(), json!({ "op": "deferAsk", "id": "a" }));
        // What they make, the engine reads.
        for op in [answer_params(&Value::record([("id", Value::from("a")), ("option", Value::from(1.0))])).unwrap(), op] {
            assert!(serde_json::from_value::<zeron_proto::orchestration::OrchestrateOp>(op.clone()).is_ok(), "{op}");
            assert!(person_op(&op).is_ok());
        }
    }

    #[test]
    fn activity_and_todo_read_as_session_rows_carry_them() {
        let a: ChatActivity =
            serde_json::from_value(json!({ "kind": "testing", "label": "cargo test", "since": iso(NOW - 48) })).unwrap();
        let v = activity_value(&a);
        assert_eq!(text(v.field("kind")).as_deref(), Some("testing"));
        assert_eq!(text(v.field("label")).as_deref(), Some("cargo test"));
        assert_eq!(num(v.field("since")), Some((NOW - 48) as f64));
        let t = todo_value(&TodoProgress { done: 3, total: 3, current: None });
        assert_eq!((num(t.field("done")), num(t.field("total"))), (Some(3.), Some(3.)));
        assert!(nil(t.field("current")));
    }

    #[test]
    fn loads_are_keyed_by_device() {
        let loads: Vec<DeviceLoad> = serde_json::from_value(json!([
            { "deviceId": "dev-desk", "cpu": 0.71, "memoryUsedBytes": 41_200_000_000u64, "memoryTotalBytes": 62_000_000_000u64,
              "load1": 11.4, "cores": 16, "online": true, "at": iso(NOW) },
            { "deviceId": "dev-cloud", "online": false, "at": iso(NOW - 30) },
        ]))
        .unwrap();
        let v = loads_value(&loads);
        let desk = v.field("dev-desk").unwrap();
        assert_eq!(num(desk.field("cpu")), Some(0.71));
        assert_eq!(num(desk.field("mem_used")), Some(41.2e9));
        assert_eq!(num(desk.field("mem_total")), Some(62.0e9));
        assert_eq!(num(desk.field("load1")), Some(11.4));
        assert_eq!(num(desk.field("cores")), Some(16.));
        assert_eq!(desk.field("online"), Some(&Value::Bool(true)));
        assert_eq!(num(desk.field("at")), Some(NOW as f64));
        let cloud = v.field("dev-cloud").unwrap();
        for key in ["cpu", "mem_used", "mem_total", "load1", "cores"] {
            assert!(nil(cloud.field(key)), "{key}");
        }
        assert_eq!(cloud.field("online"), Some(&Value::Bool(false)));
        assert!(loads_value(&[]).field("dev-desk").is_none());
    }

    fn comm(id: &str, at: i64) -> Json {
        json!({ "id": id, "kind": "route", "from": "orc-brain", "to": "you", "summary": id, "at": iso(at) })
    }

    #[test]
    fn comms_append_replace_and_keep_the_latest_hundred() {
        let mut comms = Comms::default();
        // On subscribe, the ring whole (in any order): newest last.
        assert!(comms.heard(&json!([comm("b", NOW - 10), comm("a", NOW - 20)])).unwrap());
        let ids = |c: &Comms| c.events.iter().map(|e| e.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(&comms), ["a", "b"]);
        // The same again: nothing new.
        assert!(!comms.heard(&json!([comm("a", NOW - 20), comm("b", NOW - 10)])).unwrap());
        // One more: at the end.
        assert!(comms.heard(&comm("c", NOW)).unwrap());
        assert_eq!(ids(&comms), ["a", "b", "c"]);
        // One heard again: in its place, not twice.
        assert!(!comms.heard(&comm("c", NOW)).unwrap());
        assert_eq!(comms.events.len(), 3);
        // Never more than 100: the oldest go.
        for i in 0..150 {
            comms.heard(&comm(&format!("n{i}"), NOW + i)).unwrap();
        }
        assert_eq!(comms.events.len(), COMMS);
        assert_eq!(comms.events.front().unwrap().id, "n50");
        assert_eq!(comms.events.back().unwrap().id, "n149");
        // A whole ring again replaces it, cut to the newest 100.
        let ring: Vec<Json> = (0..120).map(|i| comm(&format!("r{i}"), NOW + 1000 + i)).collect();
        assert!(comms.heard(&Json::Array(ring)).unwrap());
        assert_eq!(comms.events.len(), COMMS);
        assert_eq!(comms.events.front().unwrap().id, "r20");
        // What scripts see: newest last, snake_case, seconds.
        let v = comms.value();
        let last = v.list().last().unwrap();
        assert_eq!(text(last.field("id")).as_deref(), Some("r119"));
        assert_eq!(text(last.field("kind")).as_deref(), Some("route"));
        assert_eq!(num(last.field("at")), Some((NOW + 1119) as f64));
        // Something unreadable: told, and the ring stands.
        assert!(comms.heard(&json!({ "id": "x" })).is_err());
        assert_eq!(comms.events.len(), COMMS);
    }

    #[test]
    fn a_dump_and_its_routes_carry_their_files_and_a_route_can_be_done() {
        let d: Dump = serde_json::from_value(json!({
            "id": "dp", "text": "look", "state": "routed", "deviceId": "dev", "createdAt": iso(NOW), "updatedAt": iso(NOW),
            "attachments": [{ "path": "/tmp/a.png", "name": "a.png" }],
            "routes": [{ "id": "rt", "excerpt": "look", "target": { "kind": "inbox" }, "intent": "task", "message": "m",
                         "state": "done", "answer": "Fixed in `abc`.", "answerFull": "Fixed.\n\nAll green.", "attachments": ["/srv/a.png"] }],
        }))
        .unwrap();
        let v = dump_value(&d);
        let file = &v.field("attachments").unwrap().list()[0];
        assert_eq!(text(file.field("name")).as_deref(), Some("a.png"));
        let route = &v.field("routes").unwrap().list()[0];
        assert_eq!(text(route.field("state")).as_deref(), Some("done"));
        assert_eq!(text(route.field("answer_full")).as_deref(), Some("Fixed.\n\nAll green."));
        assert_eq!(text(Some(&route.field("attachments").unwrap().list()[0])).as_deref(), Some("/srv/a.png"));
    }

    #[test]
    fn a_dumps_picks_become_the_brain_dumps_config() {
        let picks = Value::record([("model", Value::from("claude-code/claude-opus-5-5")), ("effort", Value::from("high"))]);
        let config = crate::chat::config_with("orc-unknown", &picks);
        assert_eq!(config["harness"], "claude-code");
        assert_eq!(config["model"], "claude-opus-5-5");
        assert_eq!(config["reasoning"], "high");
    }

    #[test]
    fn each_comms_frame_adds_to_what_came_before() {
        // The stream's frames: the backlog, then each burst since (also a
        // list). A later frame must not stand for the whole feed.
        let mut comms = Comms::default();
        let ids = |c: &Comms| c.events.iter().map(|e| e.id.clone()).collect::<Vec<_>>();
        assert!(comms.heard(&json!([comm("a", NOW - 30), comm("b", NOW - 20)])).unwrap());
        assert!(comms.heard(&json!([comm("c", NOW - 10)])).unwrap());
        assert!(comms.heard(&json!([comm("d", NOW - 5), comm("e", NOW)])).unwrap());
        assert_eq!(ids(&comms), ["a", "b", "c", "d", "e"]);
        // Fallen behind: the whole ring again, overlapping what's held.
        assert!(comms.heard(&json!([comm("c", NOW - 10), comm("d", NOW - 5), comm("e", NOW), comm("f", NOW + 1)])).unwrap());
        assert_eq!(ids(&comms), ["a", "b", "c", "d", "e", "f"]);
        // One that came late goes in its place in time.
        assert!(comms.heard(&json!([comm("z", NOW - 25)])).unwrap());
        assert_eq!(ids(&comms), ["a", "z", "b", "c", "d", "e", "f"]);
    }

    #[test]
    fn each_device_keeps_its_cpu_line() {
        let round = |at: i64, cpu: f32| -> Vec<DeviceLoad> {
            serde_json::from_value(json!([
                { "deviceId": "dev-desk", "cpu": cpu, "online": true, "at": iso(at) },
                { "deviceId": "dev-cloud", "online": false, "at": iso(at) },
            ]))
            .unwrap()
        };
        let mut history = History::default();
        history.heard(&round(NOW - 10, 0.2));
        history.heard(&round(NOW - 5, 0.5));
        // The same round again adds nothing.
        history.heard(&round(NOW - 5, 0.5));
        let v = loads_value_with(&round(NOW - 5, 0.5), &history);
        let line = v.field("dev-desk").unwrap().field("history").unwrap().list();
        assert_eq!(line.len(), 2);
        assert_eq!(num(line[0].field("at")), Some((NOW - 10) as f64));
        assert_eq!(num(line[1].field("cpu")), Some(0.5));
        // No CPU, no line.
        assert!(v.field("dev-cloud").unwrap().field("history").unwrap().list().is_empty());
        // At most six minutes of it.
        for i in 0..200 {
            history.heard(&round(NOW + i * 5, 0.1));
        }
        assert_eq!(history.by_device["dev-desk"].len(), HISTORY);
    }

    fn same(a: &Value, b: &Value) -> bool {
        match (a, b) {
            (Value::Map(a), Value::Map(b)) => Rc::ptr_eq(a, b),
            (Value::List(a), Value::List(b)) => Rc::ptr_eq(a, b),
            _ => false,
        }
    }

    fn row<'a>(v: &'a Value, list: &str, key: &str, id: &str) -> &'a Value {
        v.field(list).unwrap().list().iter().find(|r| text(r.field(key)).as_deref() == Some(id)).unwrap()
    }

    /// As the engine's watch on changes sends them: a snapshot, then deltas.
    fn frames(states: &[OrchestrationState]) -> Vec<Json> {
        let mut framer = zeron_proto::orchestration::frames::Framer::new();
        states.iter().filter_map(|s| framer.frame(s)).map(|f| serde_json::to_value(f).unwrap()).collect()
    }

    /// A delta that moves one card makes that card's row again and nothing
    /// else: every other row, and every other list, is the value scripts
    /// already have (the same Luau table, so their selectors don't run).
    #[test]
    fn a_delta_keeps_every_row_it_does_not_touch() {
        let s1 = OrchestrationState::deserialize(&sample()).unwrap();
        let mut s2 = s1.clone();
        let live = s2.items.iter_mut().find(|i| i.id == "it-live").unwrap();
        live.state = zeron_proto::orchestration::WorkState::Active;
        let wire = frames(&[s1, s2.clone()]);
        assert_eq!(wire.len(), 2);
        assert_eq!(wire[1]["frame"], "delta");
        assert!(wire[1].get("asks").is_none(), "only what changed goes");
        let o = Orchestration::default();
        o.heard_state(wire[0].clone());
        let before = o.now_value();
        o.heard_state(wire[1].clone());
        let after = o.now_value();
        assert_eq!(after, state_value(&s2), "the delta gives the state the engine has");
        assert_eq!(text(row(&after, "items", "id", "it-live").field("state")).as_deref(), Some("active"));
        assert!(!same(row(&before, "items", "id", "it-live"), row(&after, "items", "id", "it-live")));
        assert!(same(row(&before, "items", "id", "it-titlebar"), row(&after, "items", "id", "it-titlebar")));
        for list in ["orchestrators", "crew", "asks", "dumps"] {
            assert!(same(before.field(list).unwrap(), after.field(list).unwrap()), "{list} kept whole");
        }
    }

    /// What scripts see of a pushed change: a card's selector (as the
    /// Board's cards read theirs) runs again only for the card that moved,
    /// and every other card is the very Luau table it was.
    #[test]
    fn a_pushed_delta_runs_only_the_moved_card_s_selector() {
        use zgx_script::testing::TestApp;
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/orch_push_fixture.luau");
        let app = TestApp::new(|runtime| ScriptHost::new(runtime, path));
        app.frames(5);
        let shared = app.host.shared("orchestration", loading_value());
        let o = Orchestration::default();
        let push = |item: Json| {
            o.heard_state(item);
            shared.set(o.now_value());
            app.settle();
            app.frames(3);
        };
        let state = |name: &str| app.host.states().into_iter().find(|(n, _)| n == name).map(|(_, v)| v).unwrap();
        let count = |id: &str| num(state("runs").field(id)).unwrap_or(0.);
        let kept = |id: &str| matches!(state("kept").field(id), Some(Value::Bool(true)));

        let s1 = OrchestrationState::deserialize(&sample()).unwrap();
        let mut s2 = s1.clone();
        s2.items.iter_mut().find(|i| i.id == "it-live").unwrap().state = zeron_proto::orchestration::WorkState::Active;
        let mut s3 = s2.clone();
        s3.asks[0].question = "Something else?".into();
        let mut s4 = s3.clone();
        let mut fresh = s4.items[1].clone();
        fresh.id = "it-new".into();
        s4.items.push(fresh);
        let wire = frames(&[s1, s2, s3, s4]);
        assert_eq!(wire.len(), 4);

        push(wire[0].clone());
        let (titlebar, live, new) = (count("it-titlebar"), count("it-live"), count("it-new"));
        // A card moved: its selector alone runs again.
        push(wire[1].clone());
        assert_eq!(count("it-live"), live + 1.);
        assert_eq!(count("it-titlebar"), titlebar, "an unchanged card's selector didn't run");
        assert_eq!(count("it-new"), new);
        assert!(kept("it-titlebar"), "an unchanged card is the same table");
        assert!(!kept("it-live"));
        // An ask changed: no card's selector runs, every card is kept.
        push(wire[2].clone());
        assert_eq!((count("it-titlebar"), count("it-live")), (titlebar, live + 1.));
        assert!(kept("it-titlebar") && kept("it-live"));
        // A card added: its own selector runs, the others don't.
        push(wire[3].clone());
        assert_eq!(count("it-new"), new + 1.);
        assert_eq!((count("it-titlebar"), count("it-live")), (titlebar, live + 1.));
        assert!(kept("it-titlebar") && kept("it-live"));
    }

    /// A row added, one removed and the order changed, by a delta.
    #[test]
    fn a_delta_adds_removes_and_reorders_rows() {
        let s1 = OrchestrationState::deserialize(&sample()).unwrap();
        let mut s2 = s1.clone();
        let gone = s2.crew.remove(0);
        let mut newcomer = gone.clone();
        newcomer.chat_id = "th-new".into();
        s2.crew.insert(0, newcomer);
        s2.items.reverse();
        let wire = frames(&[s1, s2.clone()]);
        let o = Orchestration::default();
        o.heard_state(wire[0].clone());
        let before = o.now_value();
        o.heard_state(wire[1].clone());
        let after = o.now_value();
        assert_eq!(after, state_value(&s2));
        // Reordered rows are the same rows.
        assert!(same(row(&before, "items", "id", "it-live"), row(&after, "items", "id", "it-live")));
    }

    /// A missed frame: nothing applied, and the state is watched again.
    #[test]
    fn a_missed_frame_changes_nothing_and_watches_again() {
        let s1 = OrchestrationState::deserialize(&sample()).unwrap();
        let mut s2 = s1.clone();
        s2.items[0].title = "renamed".into();
        let mut s3 = s2.clone();
        s3.items[0].title = "renamed again".into();
        let wire = frames(&[s1, s2, s3]);
        let o = Orchestration::default();
        o.heard_state(wire[0].clone());
        let before = o.now_value();
        let resyncs = RESYNCS.with(Cell::get);
        o.heard_state(wire[2].clone());
        assert_eq!(RESYNCS.with(Cell::get), resyncs + 1);
        assert!(same(&before, &o.now_value()));
        // More of the old watch's deltas: nothing applied, no second watch.
        o.heard_state(wire[1].clone());
        assert_eq!(RESYNCS.with(Cell::get), resyncs + 1);
        assert!(same(&before, &o.now_value()));
        // The new watch's snapshot: the state as it is, untouched rows kept.
        let s3 = OrchestrationState::deserialize(&sample()).unwrap();
        let mut s3 = s3;
        s3.items[0].title = "renamed again".into();
        let again = frames(std::slice::from_ref(&s3));
        o.heard_state(again[0].clone());
        let after = o.now_value();
        assert_eq!(after, state_value(&s3));
        assert!(same(before.field("asks").unwrap(), after.field("asks").unwrap()));
        let id = s3.items[1].id.clone();
        assert!(same(row(&before, "items", "id", &id), row(&after, "items", "id", &id)));
        // And deltas apply again after it.
        let mut s4 = s3.clone();
        s4.items[1].title = "fourth".into();
        let mut framer = zeron_proto::orchestration::frames::Framer::new();
        framer.frame(&s3);
        o.heard_state(serde_json::to_value(framer.frame(&s4).unwrap()).unwrap());
        assert_eq!(o.now_value(), state_value(&s4));
    }

    /// An engine from before frames sends the whole state each time: each
    /// is taken as it comes, and rows that didn't change keep their values.
    #[test]
    fn an_older_engine_s_whole_states_keep_unchanged_rows() {
        let s1 = OrchestrationState::deserialize(&sample()).unwrap();
        let mut s2 = s1.clone();
        s2.asks[0].question = "Another question?".into();
        let o = Orchestration::default();
        o.heard_state(serde_json::to_value(&s1).unwrap());
        let before = o.now_value();
        o.heard_state(serde_json::to_value(&s2).unwrap());
        let after = o.now_value();
        assert_eq!(after, state_value(&s2));
        assert!(same(before.field("items").unwrap(), after.field("items").unwrap()));
        assert!(!same(before.field("asks").unwrap(), after.field("asks").unwrap()));
        let id = s2.asks[1].id.clone();
        assert!(same(row(&before, "asks", "id", &id), row(&after, "asks", "id", &id)));
    }

    #[test]
    fn a_state_heard_again_is_not_published_and_a_bad_one_keeps_the_last() {
        let o = Orchestration::default();
        assert_eq!(text(o.now_value().field("state")).as_deref(), Some("loading"));
        o.heard_state(sample());
        let first = o.now_value();
        assert_eq!(text(first.field("state")).as_deref(), Some("ready"));
        o.heard_state(sample());
        let again = o.now_value();
        assert!(matches!((&first, &again), (Value::Map(a), Value::Map(b)) if Rc::ptr_eq(a, b)));
        o.heard_state(json!({ "orchestrators": "nope" }));
        assert_eq!(o.now_value(), first);
    }

    /// Every op the scripts send through `orchestration.op` (by the
    /// literal `op = "…"` in each file that calls it) is one of the
    /// person's: one left out is refused by the bridge ("not yours") before
    /// it reaches the engine, which the scripts' tests, on fake handlers,
    /// never see.
    #[test]
    fn every_op_the_scripts_send_is_the_persons() {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
                if path.is_dir() {
                    if name != "tests" && name != "demos" {
                        walk(&path, out);
                    }
                } else if name.ends_with(".luau") && !name.ends_with("_fixture.luau") {
                    out.push(path);
                }
            }
        }
        let ui = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui");
        let mut files = Vec::new();
        walk(&ui, &mut files);
        let mut sent: Vec<(String, String)> = Vec::new();
        for path in files {
            let text = std::fs::read_to_string(&path).unwrap();
            let rel = path.strip_prefix(&ui).unwrap().display().to_string();
            let account = rel.replace('\\', "/") == "account/orchestration.luau";
            if !(text.contains("orchestration.op(") || (account && text.contains("M.op("))) {
                continue;
            }
            for (at, _) in text.match_indices("op = \"") {
                let rest = &text[at + 6..];
                let name: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
                if rest[name.len()..].starts_with('"') && !name.is_empty() {
                    sent.push((rel.clone(), name));
                }
            }
        }
        // (The ones the Board's Pause and Resume send among them.)
        for op in ["pauseItem", "resumeItem", "upsertItem"] {
            assert!(sent.iter().any(|(_, o)| o == op), "{op} is sent by no script: the scan missed it");
        }
        let refused: Vec<&(String, String)> = sent.iter().filter(|(_, op)| !PERSON_OPS.contains(&op.as_str())).collect();
        assert!(refused.is_empty(), "sent by the scripts, refused by the bridge (add to PERSON_OPS): {refused:?}");
    }

    #[test]
    fn a_limited_chat_and_its_policy_reach_scripts_snake_cased() {
        let waiting: zeron_proto::usage_limit::SessionLimit = serde_json::from_value(json!({
            "provider": "claude", "accountLabel": "me@work.dev", "window": "5-hour",
            "until": iso(NOW + 1800), "since": iso(NOW - 60), "attempt": 1
        }))
        .unwrap();
        let v = limited_value(&waiting);
        assert_eq!(text(v.field("provider")).as_deref(), Some("claude"));
        assert_eq!(text(v.field("account_label")).as_deref(), Some("me@work.dev"));
        assert_eq!(num(v.field("until_at")), Some((NOW + 1800) as f64));
        assert_eq!(num(v.field("since")), Some((NOW - 60) as f64));
        assert_eq!(num(v.field("attempt")), Some(1.0));
        assert!(nil(v.field("switched_to")));
        assert!(nil(v.field("account_slot")));

        let switched: zeron_proto::usage_limit::SessionLimit = serde_json::from_value(json!({
            "provider": "claude", "since": iso(NOW - 60),
            "switchedTo": { "accountSlot": "s2", "accountLabel": "me@home.dev", "at": iso(NOW - 30), "model": "opus" }
        }))
        .unwrap();
        let v = limited_value(&switched);
        assert!(nil(v.field("until_at")));
        let to = v.field("switched_to").unwrap();
        assert_eq!(text(to.field("account_slot")).as_deref(), Some("s2"));
        assert_eq!(text(to.field("account_label")).as_deref(), Some("me@home.dev"));
        assert_eq!(num(to.field("at")), Some((NOW - 30) as f64));
        assert_eq!(text(to.field("model")).as_deref(), Some("opus"));
        assert!(nil(to.field("harness")));

        let mut o: Orchestrator = serde_json::from_value(sample()["orchestrators"][1].clone()).unwrap();
        o.crew.on_limit.switch_account = true;
        let v = orchestrator_value(&o);
        let policy = v.field("crew").unwrap().field("on_limit").unwrap();
        assert_eq!(policy.field("switch_account"), Some(&Value::Bool(true)));
        assert_eq!(policy.field("switch_harness"), Some(&Value::Bool(false)));
    }
}
