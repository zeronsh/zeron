//! Scrolling up a long chat from its end passes through every message: a
//! trackpad's steps, one a refresh, with the window's spare time building rows
//! ahead of the view as Zeron.app's does, move what shows by each step and no
//! more. (Nov 10, beta.4: scrolling up from the end of one chat skipped past a
//! message in one frame, every time, until a send; scrolling back down showed
//! it. Not reproduced here: these hold the ways it was looked for.) On a long
//! working chat (ui/long_chat_fixture.luau's texts, its rows built by
//! src/chat.rs as the engine's entries are): replies of markdown, tables and
//! work far taller than the list's estimate for a row not yet measured.

use std::path::PathBuf;

use serde_json::{Value as Json, json};
use zgx_script::{ScriptHost, Value, testing::TestApp};

fn ui() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui")
}

/// Each text in the transcript's view, by its words: its top.
fn shown(app: &mut TestApp) -> Vec<(String, f32)> {
    let view = app.bounds("transcript");
    let tree = app.tree();
    let mut out = Vec::new();
    for line in tree.lines() {
        let Some(at) = line.find(" text \"") else {
            continue;
        };
        let rest = &line[at + 7..];
        let Some(end) = rest.rfind("\" ") else {
            continue;
        };
        let words = rest[..end].to_owned();
        let Some(place) = rest[end + 2..].split_whitespace().next() else {
            continue;
        };
        let Some((_, y)) = place.split_once(',') else {
            continue;
        };
        let Ok(y) = y.parse::<f32>() else { continue };
        if y >= view.y && y < view.y + view.height {
            out.push((words, y));
        }
    }
    out
}

/// How far what shows moved between two looks (the median of the texts seen
/// in both), and how many were seen in both.
fn moved(before: &[(String, f32)], after: &[(String, f32)]) -> (Option<f32>, usize) {
    let mut deltas: Vec<f32> = before
        .iter()
        .filter_map(|(words, y)| after.iter().find(|(w, _)| w == words).map(|(_, y2)| y2 - y))
        .collect();
    deltas.sort_by(f32::total_cmp);
    let n = deltas.len();
    (deltas.get(n / 2).copied(), n)
}

/// The fixture's long texts (`local NAME = [[…]]` in ui/long_chat_fixture.luau).
fn texts() -> std::collections::HashMap<String, String> {
    let source = std::fs::read_to_string(ui().join("long_chat_fixture.luau")).unwrap();
    let mut out = std::collections::HashMap::new();
    let mut rest = source.as_str();
    while let Some(at) = rest.find(" = [[") {
        let name = rest[..at].rsplit(' ').next().unwrap().to_owned();
        let body = &rest[at + 5..];
        let end = body.find("]]").unwrap();
        out.insert(name, body[..end].to_owned());
        rest = &body[end + 2..];
    }
    out
}

/// The same chat as the host builds it from the engine's entries
/// (src/chat.rs): thoughts, commands and texts as a reply's parts.
fn built_chat(turns: usize) -> Value {
    built_chat_keyed(turns, None)
}
/// `built_chat`, with prompt `rekey.0` under another id (a row the host
/// publishes again under a new one).
fn built_chat_keyed(turns: usize, rekey: Option<(usize, String)>) -> Value {
    let t = texts();
    let prompts = [
        "Go, this will fix it right?",
        "three and one sounds like the most optional option\n\nrolling back at this point is not a good idea. we've released a feature for three weeks almost",
        "ok",
        "What does the monitor say now?",
    ];
    let mut at = 1_794_000_000_000_i64;
    let mut rows = Vec::new();
    let mut user = |rows: &mut Vec<Value>, id: &str, text: &str, at: &mut i64| {
        *at += 60_000;
        rows.push(crate::chat::built_entry(&json!({ "id": id, "role": "user", "parts": [{ "id": format!("{id}p"), "kind": "text", "text": text }], "createdAt": *at }), "doc", None, Some("Opus")).unwrap());
    };
    let reply = |rows: &mut Vec<Value>, id: &str, parts: Vec<Json>, at: &mut i64| {
        *at += 60_000;
        rows.push(crate::chat::built_entry(&json!({ "id": id, "role": "assistant", "status": "complete", "parts": parts, "createdAt": *at, "durationMs": 90_000 }), "doc", None, Some("Opus")).unwrap());
    };
    let thought = |id: &str| json!({ "id": id, "kind": "reasoning", "text": "Weighing which of the three fits the timing." });
    let work = |id: &str, n: usize| -> Vec<Json> {
        (0..n).map(|k| json!({ "id": format!("{id}.{k}"), "kind": "tool", "call": { "kind": "exec", "command": format!("aws logs start-query --log-group-name /engine --start-time {k}") }, "output": "ok", "exitCode": 0 })).collect()
    };
    let text = |id: &str, name: &str| json!({ "id": id, "kind": "text", "text": t[name] });
    let bodies = ["STATE", "SUSPECTS", "TRIGGER"];
    for i in 1..=turns {
        let id = match &rekey {
            Some((n, id)) if *n == i => id.clone(),
            _ => format!("lu{i}"),
        };
        user(&mut rows, &id, prompts[(i + 1) % prompts.len()], &mut at);
        let mut parts = vec![
            thought(&format!("t{i}")),
            text(&format!("x{i}"), bodies[(i + 1) % 3]),
        ];
        parts.extend(work(&format!("w{i}"), 1 + i % 4));
        reply(&mut rows, &format!("la{i}"), parts, &mut at);
    }
    user(&mut rows, "orbit-go", prompts[0], &mut at);
    let mut parts = work("ws", 9);
    parts.push(thought("ts"));
    parts.push(text("xs", "SUSPECTS"));
    reply(&mut rows, "orbit-suspects", parts, &mut at);
    user(&mut rows, "orbit-answer", prompts[1], &mut at);
    let mut parts = vec![thought("tt"), text("xa", "APPROVED")];
    parts.extend(work("w1", 4));
    parts.push(text("xb", "RUNNING"));
    parts.extend(work("w2", 14));
    parts.push(text("xc", "TRIGGER"));
    reply(&mut rows, "orbit-trigger", parts, &mut at);
    Value::List(rows.into())
}

/// What happens to the chat as the reader scrolls (a synced chat's host
/// publishing it), besides the scroll.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Meanwhile {
    /// Nothing.
    Nothing,
    /// Published again, the same rows built anew, every other step.
    Republished,
    /// Published again every other step, a prompt far above under a new id
    /// each time.
    Rekeyed,
    /// Opened with its newest 40 rows; the rest arrive with the first step.
    HistoryOnFirstStep,
    /// Published once without its last two rows, then whole again, with the
    /// first step.
    LastRowsDroppedOnce,
}

/// Scrolled `step` points up each refresh from the chat's end, what shows moves
/// down by the step each time (a synced chat's, never more), and the person's
/// prompt between the last two replies passes through the view.
fn scroll_up_through(step: f32, meanwhile: Meanwhile) {
    let fixture = ui().join("chat_fixture.luau");
    let mut app = TestApp::with_spare_time(move |runtime| ScriptHost::new(runtime, fixture));
    app.ui.scene.borrow_mut().resize(960., 700.);
    app.frames(20);
    assert!(app.host.run_command("go_s7"));
    for _ in 0..30 {
        app.spend_spare_time();
        app.frame();
    }
    let messages = app.host.shared("chat.messages", Value::Nil);
    let whole = built_chat(150);
    if meanwhile == Meanwhile::HistoryOnFirstStep {
        let rows = whole.list();
        messages.set(Value::List(rows[rows.len() - 40..].to_vec().into()));
    } else {
        messages.set(whole);
    }
    for _ in 0..60 {
        app.spend_spare_time();
        app.frame();
    }
    let node = app.find("transcript").expect("the transcript");
    // Up a way and back down to the end, building and keeping rows, then a
    // while at the end: the window's frames stop and its lists let go of what
    // they kept (`zgx::spare::KEPT_REST`), so the next scroll is a first move.
    for dy in [-40., 40.] {
        for _ in 0..if dy < 0. { 60 } else { 90 } {
            zgx_script::driver::wheel(&mut app.ui, node, 0., dy, None);
            app.spend_spare_time();
            app.frame();
        }
    }
    for _ in 0..30 {
        app.spend_spare_time();
        app.frame();
    }
    app.ui.release_kept_rows().unwrap();
    app.frames(5);
    let mut seen_prompt = false;
    let mut jumps = Vec::new();
    // (Far enough up to pass the prompt between the last two replies.)
    for i in 0..(2400. / step) as usize {
        let before = shown(&mut app);
        seen_prompt |= before
            .iter()
            .any(|(w, _)| w.starts_with("three and one sounds like"));
        let republish = match meanwhile {
            Meanwhile::Republished => Some(built_chat(150)),
            Meanwhile::Rekeyed => Some(built_chat_keyed(
                150,
                Some((100 + i % 40, format!("rekeyed{i}"))),
            )),
            _ => None,
        };
        if let Some(chat) = republish
            && i % 2 == 1
        {
            messages.set(chat);
        }
        // A step, then the window's refresh: its spare time first, then its
        // frame.
        zgx_script::driver::wheel(&mut app.ui, node, 0., -step, None);
        if i == 0 {
            match meanwhile {
                Meanwhile::HistoryOnFirstStep => {
                    messages.set(built_chat(150));
                }
                Meanwhile::LastRowsDroppedOnce => {
                    let whole = built_chat(150);
                    let rows = whole.list();
                    messages.set(Value::List(rows[..rows.len() - 2].to_vec().into()));
                    app.spend_spare_time();
                    app.frame();
                    messages.set(whole);
                }
                _ => {}
            }
        }
        app.spend_spare_time();
        app.frame();
        let after = shown(&mut app);
        let (d, n) = moved(&before, &after);
        // A skip: what showed went out of view, or moved more than the step.
        // (With nothing else going on, exactly the step. A synced chat's
        // republish may take a step back to the end it holds: no skip.)
        let skipped = match d {
            None => true,
            Some(d) if meanwhile == Meanwhile::Nothing => (d - step).abs() > 1.,
            Some(d) => !(-1. ..=step + 1.).contains(&d),
        };
        if skipped {
            let top = after
                .iter()
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|t| t.0.clone())
                .unwrap_or_default();
            jumps.push(format!(
                "step {i}: moved {d:?} ({n} texts in both), top now {top:?}"
            ));
        }
    }
    assert_eq!(app.error(), None);
    assert!(
        jumps.is_empty(),
        "{meanwhile:?}: each step of {step} moved the view by {step}:\n{}",
        jumps.join("\n")
    );
    assert!(
        seen_prompt,
        "the prompt between the last two replies showed on the way up"
    );
}

#[test]
fn a_long_chat_scrolled_up_from_its_end_passes_through_every_message() {
    for step in [6., 24., 80.] {
        scroll_up_through(step, Meanwhile::Nothing);
    }
}

#[test]
fn a_synced_chat_scrolled_up_from_its_end_passes_through_every_message() {
    for meanwhile in [
        Meanwhile::Republished,
        Meanwhile::Rekeyed,
        Meanwhile::HistoryOnFirstStep,
        Meanwhile::LastRowsDroppedOnce,
    ] {
        scroll_up_through(24., meanwhile);
    }
}
