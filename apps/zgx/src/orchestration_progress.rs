//! How far each chat's work has got, for the scripts: the rows the engine
//! streams (`WatchProgress`), kept whole by chat id, and the one call that
//! asks for a fresh note (`Orchestrate{op: progressCheck}`). Progress is
//! read, never asked of the agent: a check never touches the chat itself.
//! docs/design/orchestrators.md ("Progress") is the contract;
//! ui/account/orchestration_sample.luau has one of each state.
//!
//! What scripts see, `orchestration.progress` (snake_case keys, times in
//! Unix seconds, a field that isn't there is nil):
//!   { [chat id] = { state, device_id, started?, last_step?, step?,
//!                   steps? = { done, total }, note?, note_at?, stalled,
//!                   tool?, tool_since?, refreshing, eta_secs?, updated } }
//! `state`: "working", "waiting" (on background work), "awaiting_input",
//! "idle", "failed", "done" or "other". The clocks tick in the scripts:
//! elapsed = now − `started`, since the last step = now − `last_step`, time
//! left = `eta_secs` − (now − `updated`).
//!
//! and call:
//!   orchestration.progress_check(chat)   (async) the chat's row with its
//!                                        note made fresh now (or on its
//!                                        way: `refreshing`), also put in
//!                                        `orchestration.progress` at once

use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    rc::Rc,
};

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Value as Json, json};
use zeron_proto::orchestration::{Progress, ProgressState};
use zgx_desktop::WindowContext;
use zgx_script::{ScriptHost, Shared, Value};

use crate::engine::{self, rpc::Watch};

/// One open window's state.
struct Window {
    id: u64,
    shared: Shared,
}

#[derive(Default)]
struct Tracker {
    windows: RefCell<Vec<Window>>,
    next_window: Cell<u64>,
    watching: Cell<bool>,
    rows: RefCell<Rows>,
    /// Whether something the engine sent couldn't be read, said once.
    complained: Cell<bool>,
    watch: RefCell<Option<Watch>>,
}

thread_local! {
    static TRACKER: Rc<Tracker> = Rc::new(Tracker::default());
}

pub fn install(cx: &mut WindowContext, host: &ScriptHost) {
    let t = TRACKER.with(Rc::clone);
    let window = t.next_window.get();
    t.next_window.set(window + 1);
    t.windows.borrow_mut().push(Window { id: window, shared: host.shared("orchestration.progress", t.rows.borrow().value()) });
    let me = Rc::downgrade(&t);
    cx.on_closed(move || {
        if let Some(t) = me.upgrade() {
            t.windows.borrow_mut().retain(|w| w.id != window);
        }
    });
    if !t.watching.replace(true) {
        let weak = Rc::downgrade(&t);
        engine::get().on_connect(move |rpc| {
            let Some(t) = weak.upgrade() else { return };
            let heard = Rc::downgrade(&t);
            let watch = rpc.watch("WatchProgress", json!({}), move |item| {
                if let Some(t) = heard.upgrade() {
                    t.heard(&item);
                }
            });
            *t.watch.borrow_mut() = Some(watch);
        });
    }

    host.async_action("orchestration.progress_check", move |chat| {
        let chat = crate::auth::text(&chat);
        async move {
            crate::plugins::preview::refuse("orchestration.progress_check")?;
            if chat.is_empty() {
                return Err("invalid".to_string());
            }
            let reply = engine::get().call("Orchestrate", json!({ "op": "progressCheck", "chatId": chat })).await?;
            let Ok(row) = Progress::deserialize(&reply) else { return Ok(Value::Nil) };
            let value = progress_value(&row);
            TRACKER.with(|t| t.checked(row));
            Ok(value)
        }
    });
}

impl Tracker {
    /// A frame of `WatchProgress`: every row, whole.
    fn heard(&self, item: &Json) {
        let Some(list) = item.as_array() else {
            return self.complain(&"not a list");
        };
        // (A row this build can't read costs that row, not the rest.)
        let rows: Vec<Progress> = list.iter().filter_map(|row| Progress::deserialize(row).ok()).collect();
        if rows.len() < list.len() {
            self.complain(&format!("{} unreadable rows", list.len() - rows.len()));
        }
        if self.rows.borrow_mut().replace(rows) {
            self.publish();
        }
    }

    /// A check's reply: its row, unless the stream already has a newer one.
    fn checked(&self, row: Progress) {
        if self.rows.borrow_mut().upsert(row) {
            self.publish();
        }
    }

    fn publish(&self) {
        let value = self.rows.borrow().value();
        // (Taken out first: setting one may run a script that calls back in.)
        let targets: Vec<Shared> = self.windows.borrow().iter().map(|w| w.shared.clone()).collect();
        for shared in targets {
            shared.set(value.clone());
        }
    }

    fn complain(&self, error: &dyn std::fmt::Display) {
        if !self.complained.replace(true) {
            eprintln!("zeron-zgx: progress: WatchProgress sent what this build can't read ({error}); keeping what it had");
        }
    }
}

/// Every chat's row, by chat id.
#[derive(Default)]
struct Rows {
    by_chat: BTreeMap<String, Progress>,
}

impl Rows {
    /// The stream's whole list in place of what was held. Whether anything
    /// changed.
    fn replace(&mut self, rows: Vec<Progress>) -> bool {
        let by_chat: BTreeMap<String, Progress> = rows.into_iter().map(|r| (r.chat_id.clone(), r)).collect();
        if by_chat == self.by_chat {
            return false;
        }
        self.by_chat = by_chat;
        true
    }

    /// One row in, unless the one held is newer. Whether anything changed.
    fn upsert(&mut self, row: Progress) -> bool {
        if let Some(held) = self.by_chat.get(&row.chat_id) {
            if *held == row || held.updated_at > row.updated_at {
                return false;
            }
        }
        self.by_chat.insert(row.chat_id.clone(), row);
        true
    }

    fn value(&self) -> Value {
        Value::record(self.by_chat.iter().map(|(id, r)| (id.as_str(), progress_value(r))))
    }
}

/// The word a state goes by in scripts.
fn state_word(s: ProgressState) -> &'static str {
    match s {
        ProgressState::Working => "working",
        ProgressState::Waiting => "waiting",
        ProgressState::AwaitingInput => "awaiting_input",
        ProgressState::Idle => "idle",
        ProgressState::Failed => "failed",
        ProgressState::Done => "done",
        ProgressState::Other => "other",
    }
}

/// One chat's row, as scripts see it.
fn progress_value(p: &Progress) -> Value {
    let secs = |t: Option<&DateTime<Utc>>| t.map_or(Value::Nil, |t| Value::from(t.timestamp() as f64));
    let text = |s: Option<&str>| s.map_or(Value::Nil, Value::from);
    let steps = p.steps.map_or(Value::Nil, |s| {
        Value::record([("done", Value::from(f64::from(s.done))), ("total", Value::from(f64::from(s.total)))])
    });
    Value::record([
        ("chat_id", Value::from(p.chat_id.as_str())),
        ("state", Value::from(state_word(p.state))),
        ("device_id", Value::from(p.device_id.as_str())),
        ("started", secs(p.started_at.as_ref())),
        ("last_step", secs(p.last_step_at.as_ref())),
        ("step", text(p.step.as_deref())),
        ("steps", steps),
        ("note", text(p.note.as_deref())),
        ("note_at", secs(p.note_at.as_ref())),
        ("stalled", Value::Bool(p.stalled)),
        ("tool", text(p.tool.as_deref())),
        ("tool_since", secs(p.tool_since.as_ref())),
        ("refreshing", Value::Bool(p.refreshing)),
        ("eta_secs", p.eta_secs.map_or(Value::Nil, |s| Value::from(f64::from(s)))),
        ("updated", secs(Some(&p.updated_at))),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn working() -> Json {
        json!({
            "chatId": "th-engine-state", "deviceId": "dev-desk", "state": "working",
            "startedAt": iso(NOW - 1260), "lastStepAt": iso(NOW - 130), "step": "WatchOrchestration stream",
            "steps": { "done": 5, "total": 8 }, "note": "Wiring the stream; ops next.", "noteAt": iso(NOW - 200),
            "tool": "cargo test -p zeron-engine", "toolSince": iso(NOW - 540), "etaSecs": 840, "updatedAt": iso(NOW - 30),
        })
    }

    #[test]
    fn a_row_reads_in_snake_case_and_seconds() {
        let row = Progress::deserialize(&working()).unwrap();
        let v = progress_value(&row);
        assert_eq!(text(v.field("chat_id")).as_deref(), Some("th-engine-state"));
        assert_eq!(text(v.field("state")).as_deref(), Some("working"));
        assert_eq!(text(v.field("device_id")).as_deref(), Some("dev-desk"));
        assert_eq!(num(v.field("started")), Some((NOW - 1260) as f64));
        assert_eq!(num(v.field("last_step")), Some((NOW - 130) as f64));
        assert_eq!(text(v.field("step")).as_deref(), Some("WatchOrchestration stream"));
        let steps = v.field("steps").unwrap();
        assert_eq!((num(steps.field("done")), num(steps.field("total"))), (Some(5.), Some(8.)));
        assert_eq!(text(v.field("note")).as_deref(), Some("Wiring the stream; ops next."));
        assert_eq!(num(v.field("note_at")), Some((NOW - 200) as f64));
        assert_eq!(v.field("stalled"), Some(&Value::Bool(false)));
        assert_eq!(text(v.field("tool")).as_deref(), Some("cargo test -p zeron-engine"));
        assert_eq!(num(v.field("tool_since")), Some((NOW - 540) as f64));
        assert_eq!(v.field("refreshing"), Some(&Value::Bool(false)));
        assert_eq!(num(v.field("eta_secs")), Some(840.));
        assert_eq!(num(v.field("updated")), Some((NOW - 30) as f64));
    }

    #[test]
    fn what_a_row_lacks_is_nil_and_states_are_words() {
        let row: Progress = serde_json::from_value(json!({
            "chatId": "c", "deviceId": "d", "state": "awaitingInput", "stalled": true, "refreshing": true, "updatedAt": iso(NOW),
        }))
        .unwrap();
        let v = progress_value(&row);
        assert_eq!(text(v.field("state")).as_deref(), Some("awaiting_input"));
        assert_eq!(v.field("stalled"), Some(&Value::Bool(true)));
        assert_eq!(v.field("refreshing"), Some(&Value::Bool(true)));
        for key in ["started", "last_step", "step", "steps", "note", "note_at", "tool", "tool_since", "eta_secs"] {
            assert!(nil(v.field(key)), "{key}");
        }
        for (wire, word) in [
            ("working", "working"),
            ("waiting", "waiting"),
            ("idle", "idle"),
            ("failed", "failed"),
            ("done", "done"),
            ("somethingNew", "other"),
        ] {
            let row: Progress = serde_json::from_value(json!({
                "chatId": "c", "deviceId": "d", "state": wire, "updatedAt": iso(NOW),
            }))
            .unwrap();
            assert_eq!(text(progress_value(&row).field("state")).as_deref(), Some(word), "{wire}");
        }
    }

    #[test]
    fn the_stream_replaces_and_a_check_upserts_unless_older() {
        let t = Tracker::default();
        let other = json!({ "chatId": "th-b", "deviceId": "dev-mac", "state": "done", "updatedAt": iso(NOW - 600) });
        t.heard(&json!([working(), other, { "chatId": "broken" }]));
        let v = t.rows.borrow().value();
        assert!(v.field("th-engine-state").is_some());
        assert_eq!(text(v.field("th-b").unwrap().field("state")).as_deref(), Some("done"));
        assert!(v.field("broken").is_none());
        // The same again: nothing new.
        assert!(!t.rows.borrow_mut().replace(vec![
            Progress::deserialize(&working()).unwrap(),
            serde_json::from_value(json!({ "chatId": "th-b", "deviceId": "dev-mac", "state": "done", "updatedAt": iso(NOW - 600) }))
                .unwrap(),
        ]));
        // A check's reply, newer: in its place, refreshing.
        let mut fresh = Progress::deserialize(&working()).unwrap();
        fresh.refreshing = true;
        fresh.updated_at = DateTime::<Utc>::from_timestamp(NOW, 0).unwrap();
        t.checked(fresh.clone());
        let v = t.rows.borrow().value();
        assert_eq!(v.field("th-engine-state").unwrap().field("refreshing"), Some(&Value::Bool(true)));
        // An older one doesn't replace it.
        let mut stale = fresh.clone();
        stale.refreshing = false;
        stale.updated_at = DateTime::<Utc>::from_timestamp(NOW - 100, 0).unwrap();
        assert!(!t.rows.borrow_mut().upsert(stale));
        // A frame without a chat: it goes (the stream is whole).
        t.heard(&json!([working()]));
        assert!(t.rows.borrow().value().field("th-b").is_none());
        // Not a list: told once, kept.
        t.heard(&json!({ "nope": 1 }));
        assert!(t.rows.borrow().value().field("th-engine-state").is_some());
    }

    #[test]
    fn a_preview_can_t_check_progress() {
        use crate::plugins::preview;
        preview::set(Some(preview::Preview { id: "sleek".into(), name: "Sleek".into() }));
        assert!(preview::refuse("orchestration.progress_check").is_err());
        preview::set(None);
        assert!(preview::refuse("orchestration.progress_check").is_ok());
    }
}
