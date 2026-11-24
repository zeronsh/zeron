//! The Control Room's Scratchpad, for the scripts: the document and its
//! history as the engine streams them (`WatchScratchpad`, whole on every
//! change), and the person's edits (`Scratchpad` ops). Nothing here is work:
//! no edit routes or tells an agent anything. crates/proto/src/scratchpad.rs
//! is the contract; ui/account/orchestration_sample.luau has a whole one.
//!
//! What scripts see, `scratchpad` (snake_case keys, times in Unix seconds, a
//! field that isn't there is nil):
//!   { host, missing?, loaded, text, rev, updated_at?, updated_by, device_id?,
//!     shapes = { { id, kind, x, y, w, h, points = { { x, y } }, text,
//!                  color, stroke?, z, ... } },
//!     history = { { id, at, edited_at, by, device_id?, restored_from?,
//!                   chars, shape_count, preview } } }
//! `host`: always true (the scripts' own default has it false: an app
//! without this module). `missing`: "engine" when the engine refused the
//! watch (one without the Scratchpad), until a frame comes.
//! `loaded`: false until the engine's first frame. `updated_by` and `by`:
//! "person", "brain_dump" or "other". History is newest first; its first is
//! the document as it is now. A shape's fields this build doesn't know ride
//! along and are written back as they came.
//!
//! and call:
//!   scratchpad.set_text(text)       (async) the text replaced: its `rev`
//!   scratchpad.set_shapes(shapes)   (async) the drawing replaced: its `rev`
//!   scratchpad.restore(id)          (async) a snapshot put back: its `rev`
//!   scratchpad.revision(id)         (async) one snapshot, whole:
//!                                   { id, at, edited_at, by, text, shapes,
//!                                     restored_from? }

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Value as Json, json};
use zeron_proto::scratchpad::{Editor, Revision, RevisionInfo, ScratchpadState, Shape};
use zgx_script::{ScriptHost, Value};

use crate::engine::{self, rpc::{Rpc, Watch}};
use crate::orchestrators::Key;

/// One open window's state.
struct Window {
    id: u64,
    shared: Key,
}

#[derive(Default)]
struct Tracker {
    windows: RefCell<Vec<Window>>,
    next_window: Cell<u64>,
    /// The last frame, as heard (nil before the first).
    state: RefCell<Option<ScratchpadState>>,
    /// Whether something the engine sent couldn't be read, said once.
    complained: Cell<bool>,
    /// The engine refused the watch: it has no Scratchpad.
    missing: Cell<bool>,
    watch: RefCell<Option<Watch>>,
}

thread_local! {
    static TRACKER: Rc<Tracker> = Rc::new(Tracker::default());
}

/// `host`'s window, its document given now while Orchestrators is on
/// (src/orchestrators.rs opens the stream).
pub(crate) fn add(host: &ScriptHost) -> u64 {
    let t = TRACKER.with(Rc::clone);
    let window = t.next_window.get();
    t.next_window.set(window + 1);
    let shared = Key::new(host, "scratchpad");
    if crate::orchestrators::on() {
        shared.set(t.value());
    }
    t.windows.borrow_mut().push(Window { id: window, shared });
    window
}

/// `window` closed.
pub(crate) fn remove(window: u64) {
    TRACKER.with(|t| t.windows.borrow_mut().retain(|w| w.id != window));
}

/// Orchestrators on: each window given the document as it is.
pub(crate) fn show() {
    TRACKER.with(|t| t.publish());
}

/// `WatchScratchpad` on `rpc` (a connection, Orchestrators on).
pub(crate) fn open(rpc: &Rpc) {
    let t = TRACKER.with(Rc::clone);
    let heard = Rc::downgrade(&t);
    let ended = Rc::downgrade(&t);
    let watch = rpc.watch_ending(
        "WatchScratchpad",
        json!({}),
        move |item| {
            if let Some(t) = heard.upgrade() {
                t.heard(&item);
            }
        },
        move |why| {
            if let Some(t) = ended.upgrade() {
                t.ended(&why);
            }
        },
    );
    *t.watch.borrow_mut() = Some(watch);
}

/// Orchestrators off: the stream closed, the document let go of, and each
/// window's nil.
pub(crate) fn close() {
    let t = TRACKER.with(Rc::clone);
    let watch = t.watch.borrow_mut().take();
    drop(watch);
    *t.state.borrow_mut() = None;
    t.missing.set(false);
    let keys: Vec<Key> = t.windows.borrow().iter().map(|w| w.shared.clone()).collect();
    for key in keys {
        key.clear();
    }
}

/// The window's actions: refused while Orchestrators is off.
pub(crate) fn actions(host: &ScriptHost) {
    host.async_action("scratchpad.set_text", move |text| {
        let text = crate::auth::text(&text);
        async move {
            crate::orchestrators::refuse()?;
            crate::plugins::preview::refuse("scratchpad.set_text")?;
            rev_of(engine::get().call("Scratchpad", json!({ "op": "setText", "text": text })).await?)
        }
    });
    host.async_action("scratchpad.set_shapes", move |shapes| {
        let shapes = shapes_json(&shapes);
        async move {
            crate::orchestrators::refuse()?;
            crate::plugins::preview::refuse("scratchpad.set_shapes")?;
            let shapes = shapes?;
            rev_of(engine::get().call("Scratchpad", json!({ "op": "setShapes", "shapes": shapes })).await?)
        }
    });
    host.async_action("scratchpad.restore", move |id| {
        let id = crate::auth::text(&id);
        async move {
            crate::orchestrators::refuse()?;
            crate::plugins::preview::refuse("scratchpad.restore")?;
            if id.is_empty() {
                return Err("invalid".to_string());
            }
            rev_of(engine::get().call("Scratchpad", json!({ "op": "restore", "revisionId": id })).await?)
        }
    });
    host.async_action("scratchpad.revision", move |id| {
        let id = crate::auth::text(&id);
        // (Read, not written: a preview's as much as the watch is.)
        async move {
            crate::orchestrators::refuse()?;
            if id.is_empty() {
                return Err("invalid".to_string());
            }
            let reply = engine::get().call("Scratchpad", json!({ "op": "revision", "id": id })).await?;
            let revision = Revision::deserialize(&reply).map_err(|_| "unreadable".to_string())?;
            Ok(revision_value(&revision))
        }
    });
}

impl Tracker {
    /// A frame of `WatchScratchpad`: the document and its history, whole.
    fn heard(&self, item: &Json) {
        let state = match ScratchpadState::deserialize(item) {
            Ok(state) => state,
            Err(e) => return self.complain(&e),
        };
        let found = self.missing.replace(false);
        if self.state.borrow().as_ref() == Some(&state) && !found {
            return;
        }
        *self.state.borrow_mut() = Some(state);
        self.publish();
    }

    /// The watch ended before its handle dropped: refused (an engine
    /// without the Scratchpad), or the connection gone (the next one
    /// watches again).
    fn ended(&self, why: &str) {
        if is_refusal(why) && !self.missing.replace(true) {
            eprintln!("zeron-zgx: scratchpad: the engine has no Scratchpad ({why}); edits stay unsaved");
            self.publish();
        }
    }

    fn value(&self) -> Value {
        state_value(self.state.borrow().as_ref(), self.missing.get())
    }

    fn publish(&self) {
        let value = self.value();
        // (Taken out first: setting one may run a script that calls back in.)
        let targets: Vec<Key> = self.windows.borrow().iter().map(|w| w.shared.clone()).collect();
        for shared in targets {
            shared.set(value.clone());
        }
    }

    fn complain(&self, error: &dyn std::fmt::Display) {
        if !self.complained.replace(true) {
            eprintln!("zeron-zgx: scratchpad: WatchScratchpad sent what this build can't read ({error}); keeping what it had");
        }
    }
}

/// An op's `{ rev }` reply: the rev.
fn rev_of(reply: Json) -> Result<Value, String> {
    Ok(reply["rev"].as_str().map_or(Value::Nil, Value::from))
}

/// The word an editor goes by in scripts.
fn editor_word(e: Editor) -> &'static str {
    match e {
        Editor::Person => "person",
        Editor::BrainDump => "brain_dump",
        Editor::Other => "other",
    }
}

fn secs(t: &DateTime<Utc>) -> Value {
    Value::from(t.timestamp() as f64)
}

fn text_or_nil(s: Option<&str>) -> Value {
    s.map_or(Value::Nil, Value::from)
}

/// Whether a watch's end is the engine refusing it, not the connection
/// going ("closed") or the stream ending ("done").
fn is_refusal(why: &str) -> bool {
    !matches!(why, "closed" | "done")
}

/// What scripts see: the document and its history (`loaded` false before
/// the first frame; `missing` while the engine has no Scratchpad).
fn state_value(state: Option<&ScratchpadState>, missing: bool) -> Value {
    let missing = if missing { Value::from("engine") } else { Value::Nil };
    let Some(state) = state else {
        return Value::record([
            ("host", Value::Bool(true)),
            ("missing", missing),
            ("loaded", Value::Bool(false)),
            ("text", Value::from("")),
            ("rev", Value::from("")),
            ("updated_by", Value::from("person")),
            ("shapes", Value::List(Vec::new().into())),
            ("history", Value::List(Vec::new().into())),
        ]);
    };
    let pad = &state.pad;
    Value::record([
        ("host", Value::Bool(true)),
        ("missing", missing),
        ("loaded", Value::Bool(true)),
        ("text", Value::from(pad.text.as_str())),
        ("rev", Value::from(pad.rev.as_str())),
        ("updated_at", pad.updated_at.as_ref().map_or(Value::Nil, secs)),
        ("updated_by", Value::from(editor_word(pad.updated_by))),
        ("device_id", text_or_nil(pad.device_id.as_deref())),
        ("shapes", shapes_value(&pad.shapes)),
        ("history", Value::List(state.history.iter().map(info_value).collect())),
    ])
}

fn info_value(r: &RevisionInfo) -> Value {
    Value::record([
        ("id", Value::from(r.id.as_str())),
        ("at", secs(&r.at)),
        ("edited_at", secs(&r.edited_at)),
        ("by", Value::from(editor_word(r.by))),
        ("device_id", text_or_nil(r.device_id.as_deref())),
        ("restored_from", text_or_nil(r.restored_from.as_deref())),
        ("chars", Value::from(r.chars as f64)),
        ("shape_count", Value::from(r.shape_count as f64)),
        ("preview", Value::from(r.preview.as_str())),
    ])
}

fn revision_value(r: &Revision) -> Value {
    Value::record([
        ("id", Value::from(r.id.as_str())),
        ("at", secs(&r.at)),
        ("edited_at", secs(&r.edited_at)),
        ("by", Value::from(editor_word(r.by))),
        ("device_id", text_or_nil(r.device_id.as_deref())),
        ("restored_from", text_or_nil(r.restored_from.as_deref())),
        ("text", Value::from(r.text.as_str())),
        ("shapes", shapes_value(&r.shapes)),
    ])
}

fn shapes_value(shapes: &[Shape]) -> Value {
    Value::List(shapes.iter().map(shape_value).collect())
}

/// One shape as scripts see it: its points as `{ x, y }`, and any field this
/// build doesn't know as it came.
fn shape_value(s: &Shape) -> Value {
    let points = Value::List(
        s.points.iter().map(|p| Value::record([("x", Value::from(p[0])), ("y", Value::from(p[1]))])).collect(),
    );
    let mut fields: Vec<(String, Value)> = vec![
        ("id".into(), Value::from(s.id.as_str())),
        ("kind".into(), Value::from(s.kind.as_str())),
        ("x".into(), Value::from(s.x)),
        ("y".into(), Value::from(s.y)),
        ("w".into(), Value::from(s.w)),
        ("h".into(), Value::from(s.h)),
        ("points".into(), points),
        ("text".into(), Value::from(s.text.as_str())),
        ("color".into(), Value::from(s.color.as_str())),
        ("stroke".into(), s.stroke.map_or(Value::Nil, Value::from)),
        ("z".into(), Value::from(s.z as f64)),
    ];
    for (k, v) in &s.extra {
        fields.push((k.clone(), crate::plugins::json_value(v)));
    }
    Value::record(fields)
}

/// Shapes from a script, as the wire has them: points as `[x, y]` pairs,
/// each read as a [`Shape`] (an unreadable one refuses the whole call, so a
/// half drawing is never written).
fn shapes_json(shapes: &Value) -> Result<Vec<Shape>, String> {
    shapes
        .list()
        .iter()
        .map(|s| {
            let mut json = crate::plugins::value_json(s);
            let Some(obj) = json.as_object_mut() else { return Err("invalid".to_string()) };
            let points: Vec<Json> = obj
                .get("points")
                .and_then(Json::as_array)
                .map(|list| {
                    list.iter()
                        .filter_map(|p| Some(json!([p["x"].as_f64()?, p["y"].as_f64()?])))
                        .collect()
                })
                .unwrap_or_default();
            obj.insert("points".into(), Json::Array(points));
            // (Nil fields read as absent, not null.)
            obj.retain(|_, v| !v.is_null());
            Shape::deserialize(&json).map_err(|_| "invalid".to_string())
        })
        .collect()
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

    fn frame() -> Json {
        json!({
            "pad": {
                "text": "# Sync\n\n- one", "rev": "r7", "updatedAt": iso(NOW - 60), "updatedBy": "brainDump", "deviceId": "dev-desk",
                "shapes": [
                    { "id": "a", "kind": "rect", "x": 10, "y": 20, "w": 100, "h": 40, "color": "accent", "z": 0, "roundness": 8 },
                    { "id": "b", "kind": "arrow", "points": [[110, 40], [200.5, 40]], "z": 1, "stroke": 2 },
                ],
            },
            "history": [
                { "id": "h2", "at": iso(NOW - 120), "editedAt": iso(NOW - 60), "by": "brainDump", "chars": 14, "shapeCount": 2, "preview": "Sync" },
                { "id": "h1", "at": iso(NOW - 900), "editedAt": iso(NOW - 880), "by": "person", "restoredFrom": "h0",
                  "deviceId": "dev-mac", "chars": 3, "shapeCount": 0, "preview": "old" },
            ],
        })
    }

    #[test]
    fn a_frame_reads_in_snake_case_and_seconds() {
        let state = ScratchpadState::deserialize(&frame()).unwrap();
        let v = state_value(Some(&state), false);
        assert_eq!(v.field("loaded"), Some(&Value::Bool(true)));
        assert_eq!(text(v.field("text")).as_deref(), Some("# Sync\n\n- one"));
        assert_eq!(text(v.field("rev")).as_deref(), Some("r7"));
        assert_eq!(num(v.field("updated_at")), Some((NOW - 60) as f64));
        assert_eq!(text(v.field("updated_by")).as_deref(), Some("brain_dump"));
        assert_eq!(text(v.field("device_id")).as_deref(), Some("dev-desk"));
        let shapes = v.field("shapes").unwrap().list();
        assert_eq!(shapes.len(), 2);
        let a = &shapes[0];
        assert_eq!(text(a.field("kind")).as_deref(), Some("rect"));
        assert_eq!((num(a.field("x")), num(a.field("w"))), (Some(10.), Some(100.)));
        assert_eq!(text(a.field("color")).as_deref(), Some("accent"));
        assert!(nil(a.field("stroke")));
        // A renderer's own field, kept.
        assert_eq!(num(a.field("roundness")), Some(8.));
        let b = &shapes[1];
        assert_eq!(text(b.field("color")).as_deref(), Some("fg"));
        assert_eq!(num(b.field("stroke")), Some(2.));
        let p = b.field("points").unwrap().list();
        assert_eq!((num(p[1].field("x")), num(p[1].field("y"))), (Some(200.5), Some(40.)));
        let history = v.field("history").unwrap().list();
        assert_eq!(history.len(), 2);
        assert_eq!(text(history[0].field("by")).as_deref(), Some("brain_dump"));
        assert_eq!(num(history[0].field("at")), Some((NOW - 120) as f64));
        assert_eq!(num(history[0].field("edited_at")), Some((NOW - 60) as f64));
        assert_eq!(num(history[0].field("shape_count")), Some(2.));
        assert!(nil(history[0].field("restored_from")));
        assert_eq!(text(history[1].field("restored_from")).as_deref(), Some("h0"));
        assert_eq!(text(history[1].field("device_id")).as_deref(), Some("dev-mac"));
        assert_eq!(text(history[1].field("preview")).as_deref(), Some("old"));
    }

    #[test]
    fn before_the_first_frame_it_is_empty_and_not_loaded() {
        let v = state_value(None, false);
        assert_eq!(v.field("loaded"), Some(&Value::Bool(false)));
        assert_eq!(text(v.field("text")).as_deref(), Some(""));
        assert!(v.field("shapes").unwrap().list().is_empty());
        assert!(v.field("history").unwrap().list().is_empty());
        assert!(nil(v.field("updated_at")));
        assert_eq!(v.field("host"), Some(&Value::Bool(true)));
        assert!(nil(v.field("missing")));
    }

    #[test]
    fn an_engine_without_the_scratchpad_is_said() {
        let v = state_value(None, true);
        assert_eq!(text(v.field("missing")).as_deref(), Some("engine"));
        assert_eq!(v.field("loaded"), Some(&Value::Bool(false)));
        // A refusal, not the connection going or the stream ending.
        assert!(is_refusal("unknown method: WatchScratchpad"));
        assert!(!is_refusal("closed") && !is_refusal("done"));
        // A frame clears it.
        let t = Tracker::default();
        t.ended("unknown method: WatchScratchpad");
        assert!(t.missing.get());
        t.heard(&serde_json::to_value(ScratchpadState::default()).unwrap());
        assert!(!t.missing.get());
        assert!(nil(t.value().field("missing")));
        // The connection going doesn't make it missing.
        t.ended("closed");
        assert!(!t.missing.get());
    }

    #[test]
    fn editors_are_words() {
        for (wire, word) in [("person", "person"), ("brainDump", "brain_dump"), ("somethingNew", "other")] {
            let e: Editor = serde_json::from_value(json!(wire)).unwrap();
            assert_eq!(editor_word(e), word, "{wire}");
        }
    }

    #[test]
    fn the_stream_publishes_only_what_changed_and_keeps_what_it_had() {
        let t = Tracker::default();
        t.heard(&frame());
        assert_eq!(text(t.value().field("rev")).as_deref(), Some("r7"));
        // Unreadable: told once, kept.
        t.heard(&json!({ "pad": 3 }));
        assert_eq!(text(t.value().field("rev")).as_deref(), Some("r7"));
        let mut next = frame();
        next["pad"]["rev"] = json!("r8");
        next["pad"]["text"] = json!("new");
        t.heard(&next);
        assert_eq!(text(t.value().field("text")).as_deref(), Some("new"));
    }

    #[test]
    fn shapes_go_back_as_the_wire_has_them_unknown_fields_kept() {
        let state = ScratchpadState::deserialize(&frame()).unwrap();
        let v = state_value(Some(&state), false);
        let back = shapes_json(v.field("shapes").unwrap()).unwrap();
        assert_eq!(back, state.pad.shapes);
        let wire = serde_json::to_value(&back).unwrap();
        assert_eq!(wire[0]["roundness"], json!(8));
        assert_eq!(wire[1]["points"], json!([[110.0, 40.0], [200.5, 40.0]]));
        // None at all, and one that isn't a shape.
        assert!(shapes_json(&Value::List(Vec::new().into())).unwrap().is_empty());
        assert!(shapes_json(&Value::List(vec![Value::from("nope")].into())).is_err());
    }

    #[test]
    fn a_revision_reads_whole() {
        let r: Revision = serde_json::from_value(json!({
            "id": "h1", "at": iso(NOW - 900), "editedAt": iso(NOW - 880), "by": "person", "restoredFrom": "h0",
            "text": "old", "shapes": [{ "id": "s", "kind": "text", "text": "API", "z": 3 }],
        }))
        .unwrap();
        let v = revision_value(&r);
        assert_eq!(text(v.field("text")).as_deref(), Some("old"));
        assert_eq!(text(v.field("restored_from")).as_deref(), Some("h0"));
        assert_eq!(num(v.field("edited_at")), Some((NOW - 880) as f64));
        let s = &v.field("shapes").unwrap().list()[0];
        assert_eq!((text(s.field("text")).as_deref(), num(s.field("z"))), (Some("API"), Some(3.)));
        assert!(v.field("history").is_none());
        assert_eq!(rev_of(json!({ "rev": "r9" })).unwrap(), Value::from("r9"));
    }

    #[test]
    fn a_preview_can_t_edit_the_scratchpad() {
        use crate::plugins::preview;
        preview::set(Some(preview::Preview { id: "sleek".into(), name: "Sleek".into() }));
        assert!(preview::refuse("scratchpad.set_text").is_err());
        assert!(preview::refuse("scratchpad.set_shapes").is_err());
        assert!(preview::refuse("scratchpad.restore").is_err());
        // (Reading it is a preview's as much as the watch is. Named apart:
        // preview's own test reads every `refuse("…")` as a blocked one.)
        let read = "scratchpad.revision";
        assert!(preview::refuse(read).is_ok());
        preview::set(None);
        assert!(preview::refuse("scratchpad.set_text").is_ok());
    }
}
