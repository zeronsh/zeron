//! Dictation uses the native editor, with deterministic transcription events.
use serde_json::{Value as Json, json};
use std::{cell::RefCell, path::PathBuf, rc::Rc};
use zgx_script::{ScriptHost, Value, testing::TestApp};
type Heard = Rc<RefCell<Vec<Json>>>;
const ULTRA: &str = "parakeet-ultra-q8";
/// Settings → Voice's switch, as saved (prefs.dictation).
const DICTATION_ON: &str = "screens/settings/prefs.luau::settings_dictation";
/// The models `status` lists (crates/voice/models.json's), the default first,
/// each as it is on disk.
fn models(installed: &Rc<RefCell<Vec<String>>>) -> Json {
    let here = |id: &str| installed.borrow().iter().any(|i| i == id);
    json!({
        "supported": true,
        "models": [
            {"id":ULTRA, "name":"Parakeet Ultra", "size":740363168, "memory":1150000000,
                "installed":here(ULTRA), "present":here(ULTRA), "marks":["nvidia", "moondream"],
                "languages":vec![""; 25]},
            {"id":"cohere-transcribe-q5", "name":"Cohere Transcribe", "size":1796963840u64, "memory":2800000000u64,
                "installed":here("cohere-transcribe-q5"), "present":here("cohere-transcribe-q5"),
                "slow":true, "marks":["cohere"], "languages":vec![""; 14]},
        ],
    })
}
/// `dictation.call` as the host answers it, heard and never opening a
/// microphone; `installed`, the models downloaded (a test adds one as its
/// download arrives).
fn played(host: &ScriptHost, installed: Rc<RefCell<Vec<String>>>) -> Heard {
    let heard: Heard = Rc::default();
    let said = heard.clone();
    let shared = host.shared("dictation", Value::from_json(r#"{"phase":"idle"}"#).unwrap());
    host.async_action("dictation.call", move |v: Value| {
        let args: Json = serde_json::from_str(&v.to_json()).unwrap();
        said.borrow_mut().push(args.clone());
        let state = |s: Json| shared.set(Value::from_json(&s.to_string()).unwrap());
        let answer = match args["op"].as_str() {
            Some("start") => {
                let s = json!({"phase":"starting", "serial":1, "target":args["target"], "hold":args["hold"]});
                state(s.clone());
                s
            }
            Some("stop") => {
                state(json!({"phase":"finalizing", "serial":1}));
                Json::Null
            }
            Some("cancel") => {
                state(json!({"phase":"cancelled", "serial":1}));
                Json::Null
            }
            Some("download") => {
                state(json!({"phase":"downloading", "bytes":201000000, "total":740363168}));
                Json::Null
            }
            Some("remove") => {
                installed.borrow_mut().retain(|id| args["model"] != id.as_str());
                Json::Null
            }
            Some("status") => models(&installed),
            Some("inputs") => json!([{"id":"usb", "name":"USB microphone"}]),
            _ => Json::Null,
        };
        async move { Ok(Value::from_json(&answer.to_string()).unwrap()) }
    });
    heard
}
fn at(fixture: &str, installed: Rc<RefCell<Vec<String>>>) -> (TestApp, Heard) {
    let fixture = fixture.to_string();
    let heard: Rc<RefCell<Option<Heard>>> = Rc::default();
    let keep = heard.clone();
    let mut app = TestApp::new(move |r| {
        let host = ScriptHost::new(r, PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui").join(&fixture));
        *keep.borrow_mut() = Some(played(&host, installed.clone()));
        host.action("dictation.guard", |_| {});
        host
    });
    app.ui.scene.borrow_mut().resize(960., 700.);
    app.frames(30);
    assert_eq!(app.error(), None, "{}", app.tree());
    let heard = heard.borrow().clone().unwrap();
    (app, heard)
}
/// The default model downloaded.
fn ultra() -> Rc<RefCell<Vec<String>>> {
    Rc::new(RefCell::new(vec![ULTRA.to_string()]))
}
fn app() -> TestApp {
    let (mut app, _) = at("dictation_fixture.luau", ultra());
    app.click("role:textbox \"Message\"");
    app.frames(2);
    app
}
fn ops(heard: &Heard, op: &str) -> usize {
    heard.borrow().iter().filter(|a| a["op"] == op).count()
}
/// The last `op` the host was asked for, as asked.
fn last(heard: &Heard, op: &str) -> Json {
    heard.borrow().iter().rev().find(|a| a["op"] == op).cloned().unwrap_or_default()
}
fn flag(app: &TestApp, key: &str) -> Option<bool> {
    app.host.states().into_iter().find_map(|(k, v)| match v {
        Value::Bool(b) if k == key => Some(b),
        _ => None,
    })
}
fn state(app: &TestApp, key: &str) -> String {
    app.host
        .states()
        .into_iter()
        .find_map(|(k, v)| {
            if k == key {
                if let Value::Str(s) = v {
                    Some(s.to_string())
                } else {
                    None
                }
            } else {
                None
            }
        })
        .unwrap_or_default()
}
fn command(app: &mut TestApp, key: &str) {
    assert!(app.host.run_command(key));
    app.frames(10);
    assert_eq!(app.error(), None);
}
#[test]
fn dictation_replaces_unicode_selection_as_one_undo_step() {
    let mut a = app();
    a.type_text("role:textbox \"Message\"", "hola mundo");
    a.frames(3);
    a.ui.focused_editor().unwrap().set_selection(5, 10);
    a.frames(2);
    command(&mut a, "begin");
    command(&mut a, "final");
    assert_eq!(state(&a, "dictation_draft"), "hola 你好 🌎");
    a.keys("cmd+z");
    a.frames(5);
    assert_eq!(state(&a, "dictation_draft"), "hola mundo");
}
#[test]
fn dictation_discards_stale_cancelled_and_failed_results() {
    for op in ["move", "cancel", "fail", "send_other"] {
        let mut a = app();
        a.type_text("role:textbox \"Message\"", "draft");
        a.frames(3);
        command(&mut a, "begin");
        command(&mut a, op);
        command(&mut a, "final");
        assert_eq!(state(&a, "dictation_draft"), "draft", "{op}");
        assert_eq!(state(&a, "dictation_sent"), "");
    }
}
#[test]
fn dictation_typing_on_while_it_listens_lands_the_words_after_what_was_typed() {
    let mut a = app();
    command(&mut a, "begin");
    a.type_text("role:textbox \"Message\"", "edited ");
    a.frames(3);
    command(&mut a, "final");
    assert_eq!(state(&a, "dictation_draft"), "edited 你好 🌎");
}
#[test]
fn dictation_send_waits_for_final_and_empty_speech_never_sends() {
    let mut a = app();
    command(&mut a, "begin");
    a.keys("Enter");
    a.frames(3);
    assert_eq!(state(&a, "dictation_sent"), "");
    command(&mut a, "final");
    assert_eq!(
        state(&a, "dictation_sent"),
        "你好 🌎",
        "states: {:?} tree: {}",
        a.host.states(),
        a.tree()
    );
    let mut a = app();
    a.type_text("role:textbox \"Message\"", "draft");
    a.frames(3);
    command(&mut a, "begin");
    a.keys("Enter");
    a.frames(3);
    command(&mut a, "empty");
    assert_eq!(state(&a, "dictation_sent"), "");
    assert_eq!(state(&a, "dictation_draft"), "draft");
}

#[test]
fn dictation_send_button_preserves_the_draft_until_finalization() {
    let mut a = app();
    a.type_text("role:textbox \"Message\"", "draft ");
    a.frames(3);
    command(&mut a, "begin");
    a.click("role:button \"Send message\"");
    a.frames(3);
    assert_eq!(state(&a, "dictation_sent"), "");
    assert_eq!(state(&a, "dictation_draft"), "draft ");
    command(&mut a, "final");
    assert_eq!(state(&a, "dictation_sent"), "draft 你好 🌎");
}

#[test]
fn dictation_lands_at_the_caret_moved_meanwhile_and_leaving_the_editor_drops_it() {
    let mut a = app();
    a.type_text("role:textbox \"Message\"", "draft");
    a.frames(3);
    command(&mut a, "begin");
    a.keys("ArrowLeft");
    a.frames(2);
    command(&mut a, "final");
    assert_eq!(state(&a, "dictation_draft"), "draf你好 🌎t");
    command(&mut a, "begin");
    a.click("role:button \"Model\"");
    a.frames(3);
    command(&mut a, "final");
    assert_eq!(state(&a, "dictation_draft"), "draf你好 🌎t");
    assert_eq!(state(&a, "dictation_sent"), "");
}

#[test]
fn dictation_can_finalize_a_queue_send_while_the_agent_is_running() {
    let mut a = app();
    a.host
        .set_state("dictation_running", Value::Bool(true))
        .unwrap();
    command(&mut a, "begin");
    assert!(a.find("role:button \"Queue message\"").is_some());
    a.keys("Enter");
    a.frames(3);
    command(&mut a, "final");
    assert_eq!(state(&a, "dictation_sent"), "你好 🌎");
}

#[test]
fn dictation_holds_from_the_shortcut_and_a_repeat_starts_nothing() {
    let (mut a, heard) = at("dictation_fixture.luau", ultra());
    a.click("role:textbox \"Message\"");
    a.frames(2);
    let mut held = zgx::input::Modifiers::default();
    if cfg!(target_os = "macos") {
        held.meta = true;
    } else {
        held.control = true;
    }
    zgx::input::set_held_modifiers(&a.ui.runtime, held);
    a.frames(2);
    assert!(a.host.run_command("shell.dictate"));
    assert!(a.wait_until(|_| ops(&heard, "start") == 1));
    assert!(a.host.run_command("shell.dictate"));
    a.frames(3);
    assert_eq!(ops(&heard, "start"), 1, "key repeat must not start another session");
    // Letting go at once is a tap: nothing is transcribed.
    zgx::input::set_held_modifiers(&a.ui.runtime, zgx::input::Modifiers::default());
    assert!(a.wait_until(|_| ops(&heard, "cancel") >= 1));
    assert_eq!(ops(&heard, "stop"), 0);
    assert_eq!(a.error(), None);
}
#[test]
fn dictation_stops_when_the_microphone_is_released_outside_it() {
    let (mut a, heard) = at("dictation_fixture.luau", ultra());
    a.click("role:textbox \"Message\"");
    a.frames(2);
    let b = a.bounds("#dictation-microphone");
    let (x, y) = (b.x + b.width / 2., b.y + b.height / 2.);
    a.ui.dispatch(zgx::input::InputEvent::PointerDown { x, y, button: zgx::input::PointerButton::Primary });
    assert!(a.wait_until(|_| ops(&heard, "start") == 1));
    assert_eq!(heard.borrow().iter().find(|o| o["op"] == "start").unwrap()["target"], "main");
    a.ui.dispatch(zgx::input::InputEvent::PointerMove { x: 500., y: 50. });
    a.ui.dispatch(zgx::input::InputEvent::PointerUp { x: 500., y: 50., button: zgx::input::PointerButton::Primary });
    assert!(a.wait_until(|_| ops(&heard, "cancel") + ops(&heard, "stop") >= 1), "release outside stops capture");
    assert_eq!(a.error(), None);
}
#[test]
fn voice_page_downloads_the_default_model_on_first_turning_on_and_removes_it() {
    let installed: Rc<RefCell<Vec<String>>> = Rc::default();
    let (mut a, heard) = at("voice_fixture.luau", installed.clone());
    assert!(a.wait_until(|_| ops(&heard, "status") >= 1));
    a.frames(5);
    assert!(a.tree().contains("downloads a 740 MB speech model"), "{}", a.tree());
    assert!(a.find("role:button \"Remove\"").is_none(), "{}", a.tree());
    a.click("role:switch \"Dictation\"");
    assert!(a.wait_until(|_| ops(&heard, "download") == 1));
    assert_eq!(last(&heard, "download")["model"], ULTRA);
    a.frames(5);
    assert!(a.tree().contains("Downloading · 201 MB of 740 MB"), "{}", a.tree());
    // It arrives: on, the model in use, and Remove on its row.
    installed.borrow_mut().push(ULTRA.into());
    a.host.shared("dictation", Value::Nil).set(Value::from_json(r#"{"phase":"ready"}"#).unwrap());
    assert!(a.wait_until(|a| a.tree().contains("In use")), "{}", a.tree());
    assert_eq!(flag(&a, DICTATION_ON), Some(true));
    a.click("role:button \"Remove\"");
    assert!(a.wait_until(|_| ops(&heard, "remove") == 1));
    assert_eq!(last(&heard, "remove")["model"], ULTRA);
    a.frames(10);
    assert!(a.tree().contains("downloads a 740 MB speech model"), "{}", a.tree());
    assert!(!a.tree().contains("In use"), "{}", a.tree());
    assert_eq!(flag(&a, DICTATION_ON), Some(false));
    assert_eq!(a.error(), None);
}
#[test]
fn voice_page_turned_off_mid_download_cancels_it() {
    let (mut a, heard) = at("voice_fixture.luau", Rc::default());
    assert!(a.wait_until(|_| ops(&heard, "status") >= 1));
    a.frames(5);
    a.click("role:switch \"Dictation\"");
    assert!(a.wait_until(|_| ops(&heard, "download") == 1));
    a.frames(5);
    a.click("role:switch \"Dictation\"");
    assert!(a.wait_until(|_| ops(&heard, "cancel") == 1));
    a.frames(5);
    assert!(a.tree().contains("downloads a 740 MB speech model"), "{}", a.tree());
    assert_eq!(flag(&a, DICTATION_ON), Some(false));
    assert_eq!(a.error(), None);
}
/// The voice bar's waveform while listening, and what a slot costs: scripts
/// run as the host's levels change, not between them (only the track's
/// clock, its `ui.every(500)`, ticks there), and not at all when nothing is
/// being dictated. Prints the numbers (`--nocapture`).
#[test]
fn dictation_waveform_follows_levels_with_no_script_between_slots() {
    let (mut a, _) = at("dictation_fixture.luau", ultra());
    let levels = a.host.shared("dictation_levels", Value::Nil);
    // Not dictating: no track, and frames run no scripts.
    let before = a.calls();
    a.frames(30);
    assert_eq!(a.calls() - before, 0, "idle frames call scripts");
    assert!(a.find("#voice-wave").is_none());
    command(&mut a, "listen");
    a.frames(30);
    assert!(a.find("#voice-bar-1").is_some(), "{}", a.tree());
    assert_eq!(a.bounds("#voice-bar-1").height, 3.0);
    // A slot: the newest at the trailing edge, older ones scrolling left.
    let mut bars: Vec<f64> = Vec::new();
    let (mut per_slot, mut times, mut between) = (Vec::new(), Vec::new(), 0);
    for n in 0..60u32 {
        bars.insert(0, if n % 4 == 0 { 1.0 } else { 0.25 });
        bars.truncate(128);
        let value = Value::from_json(&serde_json::to_string(&bars).unwrap()).unwrap();
        let calls = a.calls();
        let start = std::time::Instant::now();
        levels.set(value);
        a.frame();
        times.push(start.elapsed());
        per_slot.push(a.calls() - calls);
        // The rest of the slot (60 ms: three more frames).
        let calls = a.calls();
        a.frames(3);
        between += a.calls() - calls;
    }
    // 60 slots of 4 frames: 4 s, 8 ticks of the clock at most.
    assert!(between <= 9, "frames between slots call scripts: {between}");
    let newest = a.bounds("#voice-bar-1");
    let older = a.bounds("#voice-bar-2");
    // 0.25: 0.08 + 0.92 × 0.25 of 16 points.
    assert!((newest.height - (0.08 + 0.92 * 0.25) * 16.0).abs() < 0.01, "{newest:?}");
    assert!(older.x + 6.0 - newest.x < 0.01, "6 points a slot: {older:?} {newest:?}");
    assert_eq!(a.bounds("#voice-bar-4").height, 16.0);
    per_slot.sort();
    times.sort();
    eprintln!(
        "voice waveform: per slot {} script calls (median, max {}), {:?} median / {:?} max for the update and its frame; {between} calls in the 180 frames between slots (the clock)",
        per_slot[per_slot.len() / 2],
        per_slot[per_slot.len() - 1],
        times[times.len() / 2],
        times[times.len() - 1]
    );
    // Finalizing: the bars hold; frames run no scripts (the sweep is the
    // engine's).
    command(&mut a, "finalizing");
    let held = a.bounds("#voice-bar-4").height;
    let calls = a.calls();
    a.frames(30);
    let sweep = a.calls() - calls;
    assert!(sweep <= 2, "the sweep calls scripts: {sweep}");
    assert_eq!(a.bounds("#voice-bar-4").height, held);
    // Starting: the breathing baseline alone, no bars.
    command(&mut a, "starting");
    assert!(a.find("#voice-bar-1").is_none());
    let calls = a.calls();
    a.frames(30);
    let breath = a.calls() - calls;
    assert!(breath <= 2, "the breath calls scripts: {breath}");
    eprintln!("voice waveform: 30 frames finalizing {sweep} calls, 30 frames starting {breath} calls (the clock)");
    assert_eq!(a.error(), None);
}
