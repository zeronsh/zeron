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
//!                           paused, archived, created, updated } },
//!       crew = { { chat_id, orchestrator_id, item_id?, role,
//!                  last_report? = { id, state, summary, at }, released, joined } },
//!       items = { { id, orchestrator_id?, title, detail, state, priority, progress?,
//!                   threads, links = { { kind, label, target } }, outcome?,
//!                   source? = { dump_id, route_id }, order, created, updated, done? } },
//!       asks = { { id, from_chat, orchestrator_id?, item_id?, kind, question,
//!                  options = { { label, detail? } }, urgent, state, answer?,
//!                  created, answered? } },
//!       dumps = { { id, text, state, routes = { { id, excerpt, span? = { from, to },
//!                   target = { kind, id? }, intent, message, state,
//!                   candidates = { target }, answer?, item_id? } },
//!                   summary?, device, created, updated } } (newest first) }
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
//!   orchestration.loads_wanted(on) (an action) a screen showing loads mounts
//!                                  (true) or goes (false): they're sampled only
//!                                  while one wants them

use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    rc::Rc,
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use zeron_proto::orchestration::{
    Ask, ChatActivity, CommEvent, CrewMember, DeviceLoad, Dump, OrchestrationState, Orchestrator, Route, RouteTarget,
    TodoProgress, WorkItem, WorkLink,
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
    /// The state as the engine last sent it, and as scripts were given it
    /// (none yet: loading).
    heard: RefCell<Option<Json>>,
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
            let heard = Rc::downgrade(&o);
            let state = rpc.watch("WatchOrchestration", json!({}), move |item| {
                if let Some(o) = heard.upgrade() {
                    o.heard_state(item);
                }
            });
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
        let params = crate::plugins::value_json(&op);
        async move {
            crate::plugins::preview::refuse("orchestration.op")?;
            person_op(&params)?;
            let reply = engine::get().call("Orchestrate", params).await?;
            Ok(if reply.is_null() { Value::Nil } else { crate::plugins::json_value(&reply) })
        }
    });
    let a = o.clone();
    host.action("orchestration.loads_wanted", move |on| {
        if crate::plugins::preview::refuse("orchestration.loads_wanted").is_err() {
            return;
        }
        a.want_loads(window, on.truthy())
    });
}

/// The ops the person sends from the window. The rest (`report`, `ask`,
/// `route`, `dumpDone`) are an agent's, through Zeron's tools, and never come
/// from here.
const PERSON_OPS: &[&str] = &[
    "createOrchestrator",
    "promoteChat",
    "updateOrchestrator",
    "addToCrew",
    "releaseFromCrew",
    "upsertItem",
    "answerAsk",
    "dismissAsk",
    "dump",
    "reroute",
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

    fn heard_state(&self, item: Json) {
        // (The same again: nothing to give.)
        if self.heard.borrow().as_ref() == Some(&item) {
            return;
        }
        match OrchestrationState::deserialize(&item) {
            Ok(state) => {
                *self.now.borrow_mut() = Some(state_value(&state));
                *self.heard.borrow_mut() = Some(item);
                let value = self.now_value();
                for shared in self.targets(|w| &w.now) {
                    shared.set(value.clone());
                }
            }
            Err(error) => self.complain("WatchOrchestration", &error),
        }
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

/// The whole state, as scripts see it.
fn state_value(s: &OrchestrationState) -> Value {
    Value::record([
        ("state", Value::from("ready")),
        ("orchestrators", list(&s.orchestrators, orchestrator_value)),
        ("crew", list(&s.crew, crew_value)),
        ("items", list(&s.items, item_value)),
        ("asks", list(&s.asks, ask_value)),
        ("dumps", list(&s.dumps, dump_value)),
    ])
}

fn orchestrator_value(o: &Orchestrator) -> Value {
    let crew = Value::record([
        ("devices", strings(&o.crew.devices)),
        ("max_running", opt(o.crew.max_running, |n| Value::from(f64::from(n)))),
        ("harness", opt_str(o.crew.harness.as_deref())),
        ("model", opt_str(o.crew.model.as_deref())),
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
    ])
}

fn ask_value(a: &Ask) -> Value {
    let options = list(&a.options, |o| {
        Value::record([
            ("label", Value::from(o.label.as_str())),
            // (None when it has none, as the sample's.)
            ("detail", opt_str(Some(o.detail.as_str()).filter(|d| !d.is_empty()))),
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
        ("item_id", opt_str(r.item_id.as_deref())),
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
}
