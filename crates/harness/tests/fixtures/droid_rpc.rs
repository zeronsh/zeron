//! Synthetic Factory peer. No network, user configuration, or credentials.
use serde_json::{Value, json};
use std::{
    io::{self, BufRead, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

fn send(mut value: Value) {
    value["jsonrpc"] = "2.0".into();
    value["factoryApiVersion"] = "1.0.0".into();
    let mut out = io::stdout().lock();
    writeln!(out, "{value}").unwrap();
    out.flush().unwrap();
}
fn reply(id: &Value, result: Value) {
    send(json!({"type":"response","id":id,"result":result}));
}
fn error(id: &Value, code: i64, message: &str) {
    send(json!({"type":"response","id":id,"error":{"code":code,"message":message}}));
}
fn notify(value: Value) {
    send(
        json!({"type":"notification","method":"droid.session_notification","params":{"notification":value}}),
    );
}
fn notify_session(session: &str, value: Value) {
    send(
        json!({"type":"notification","method":"droid.session_notification","params":{"sessionId":session,"notification":value}}),
    );
}
fn text(value: &str) {
    notify(
        json!({"type":"assistant_text_delta","messageId":"assistant","blockIndex":0,"textDelta":value}),
    );
}
fn done(reason: &str) {
    notify(
        json!({"type":"agent_turn_completed","reason":reason,"turnId":"turn","tokenUsage":{"inputTokens":2,"cacheReadTokens":3,"outputTokens":4}}),
    );
}
fn models() -> Value {
    json!([{"id":"auto","displayName":"Auto","supportedReasoningEfforts":["none"]},
        {"id":"model-fast","displayName":"Fast model","supportedReasoningEfforts":["off","minimal","low","medium","high","xhigh","max","dynamic"]},
        {"id":"disabled","displayName":"Disabled","disabled":true,"supportedReasoningEfforts":[]}])
}
fn normalize_auto_reasoning(settings: &mut Value) {
    for (model, effort) in [
        ("modelId", "reasoningEffort"),
        ("specModeModelId", "specModeReasoningEffort"),
    ] {
        if settings[model] == "auto" {
            settings[effort] = "none".into();
        }
    }
}
fn tool(id: &str, name: &str, input: Value) {
    notify(json!({"type":"tool_call","toolUse":{"id":id,"name":name,"input":input}}));
}
fn permission_options(kind: &str, autonomy: &str) -> Value {
    if kind == "spec-no-options" {
        return Value::Null;
    }
    let mut options = vec![json!({"value":"cancel"})];
    if kind != "spec-unavailable" || autonomy != "off" {
        options.push(json!({"value":"proceed_once"}));
    }
    if kind.starts_with("spec") {
        for level in ["low", "medium", "high"] {
            if kind != "spec-unavailable" || level != autonomy {
                options.push(json!({"value":format!("proceed_auto_run_{level}")}));
            }
        }
    }
    json!(options)
}
fn permission(kind: &str, autonomy: &str) {
    let item = |id: &str, name: &str, confirmation: &str, details: Value| json!({"toolUse":{"id":id,"name":name,"input":{"file_path":"note.txt","content":"hello","command":"printf hello"}},"confirmationType":confirmation,"details":details});
    let create = item(
        "create",
        "Create",
        "create",
        json!({"filePath":"note.txt","content":"hello"}),
    );
    let exec = item(
        "exec",
        "Execute",
        "exec",
        json!({"fullCommand":"printf hello"}),
    );
    let uses = match kind {
        "create" => vec![create],
        "batch" => vec![create, exec],
        kind if kind.starts_with("spec") => {
            tool(
                "spec",
                "ExitSpecMode",
                json!({"plan":"Preliminary arguments"}),
            );
            notify(
                json!({"type":"tool_execution_phase_changed","toolUseId":"spec","phase":"queued"}),
            );
            let mut plan = item(
                "spec",
                "ExitSpecMode",
                "exit_spec_mode",
                json!({"plan":match kind {
                    "spec-missing" => Value::Null,
                    "spec-oversized" => json!("x".repeat(65536)),
                    _ => json!("Create note.txt"),
                }, "title":"A tiny plan"}),
            );
            plan["toolUse"]["input"] = json!({"plan":"Preliminary arguments"});
            if kind == "spec-batch" {
                vec![create, plan, exec]
            } else {
                vec![plan]
            }
        }
        _ => vec![exec],
    };
    send(
        json!({"type":"request","id":"permission","method":"droid.request_permission","params":{"toolUses":uses,"options":permission_options(kind, autonomy)}}),
    );
}
fn main() {
    if std::env::args().any(|arg| arg == "--version") {
        println!("1.1.0");
        return;
    }
    assert_eq!(
        std::env::var("FACTORY_DROID_AUTO_UPDATE_ENABLED").unwrap(),
        "false"
    );
    assert_eq!(std::env::var("DROID_DISABLE_AUTO_UPDATE").unwrap(), "true");
    let mut settings = json!({"modelId":"model-fast","reasoningEffort":"medium","specModeModelId":"other","specModeReasoningEffort":"low",
        "autonomyLevel":"off","interactionMode":"auto","availableAutonomyLevels":["off","low","medium","high"],"toolExecutionMode":"direct_only"});
    let session = "droid-fixture-session";
    let mut loaded = false;
    let active = Arc::new(AtomicBool::new(false));
    let mut prompt = String::new();
    let mut pending = false;
    let mut stubborn = false;
    for line in io::stdin().lock().lines() {
        let line = line.unwrap();
        let Ok(frame) = serde_json::from_str::<Value>(&line) else {
            error(&Value::Null, -32700, "Invalid JSON-RPC message");
            continue;
        };
        if frame["factoryApiVersion"] != "1.0.0"
            || !matches!(
                frame["type"].as_str(),
                Some("request" | "response" | "notification")
            )
            || frame.get("id").is_some_and(|id| !id.is_string())
        {
            error(&Value::Null, -32700, "Invalid JSON-RPC message");
            continue;
        }
        let id = &frame["id"];
        let params = &frame["params"];
        if frame["type"] == "response" {
            if !pending {
                continue;
            }
            pending = false;
            if id == "permission" {
                let outcome = frame["result"]["selectedOption"].as_str().unwrap();
                let kind = prompt.strip_prefix("permission:").unwrap();
                assert!(
                    outcome == "cancel"
                        || permission_options(kind, settings["autonomyLevel"].as_str().unwrap())
                            .as_array()
                            .is_some_and(|options| options.iter().any(|v| v["value"] == outcome))
                );
                if kind.starts_with("spec") && outcome != "cancel" {
                    settings["autonomyLevel"] = match outcome {
                        "proceed_once" => "off",
                        "proceed_auto_run_low" => "low",
                        "proceed_auto_run_medium" => "medium",
                        "proceed_auto_run_high" => "high",
                        _ => panic!("unexpected plan outcome: {outcome}"),
                    }
                    .into();
                    settings["interactionMode"] = "auto".into();
                    notify(json!({"type":"settings_updated","settings":settings}));
                }
                if kind == "spec-settings" {
                    text(
                        &json!({"selectedOption":outcome,"autonomyLevel":settings["autonomyLevel"],"interactionMode":settings["interactionMode"]})
                            .to_string(),
                    );
                } else {
                    text(outcome);
                }
                notify(
                    json!({"type":"tool_result","toolUseId":"create","content":outcome,"isError":outcome=="cancel"}),
                );
            } else if id == "ask" {
                text(&frame["result"].to_string());
            } else {
                text(&frame["error"]["code"].to_string());
            }
            active.store(false, Ordering::SeqCst);
            done("completed");
            continue;
        }
        match frame["method"].as_str().unwrap_or("") {
            "droid.list_models" => reply(id, json!({"models":models()})),
            "droid.initialize_session" | "droid.load_session" => {
                loaded = frame["method"] == "droid.load_session";
                if loaded && params["sessionId"] != session {
                    error(id, -32004, "Session not found");
                    continue;
                }
                if !loaded {
                    for (key, value) in params.as_object().unwrap() {
                        if settings.get(key).is_some() {
                            settings[key] = value.clone();
                        }
                    }
                }
                if let Some(servers) = params["mcpServers"].as_array() {
                    for server in servers {
                        assert!(server["env"].is_object());
                        assert!(server["args"].is_array());
                    }
                }
                if settings["modelId"] == "coerce" {
                    settings["reasoningEffort"] = "off".into();
                }
                if settings["modelId"] == "cap" {
                    settings["availableAutonomyLevels"] = json!(["low"]);
                }
                normalize_auto_reasoning(&mut settings);
                notify(json!({"type":"settings_updated","settings":settings}));
                reply(
                    id,
                    json!({"sessionId":session,"settings":settings,"availableModels":models(),"session":{"id":session,"messages":[{"role":"assistant","content":[{"type":"text","text":"NEVER REPLAY"}]}]}}),
                );
            }
            "droid.update_session_settings" => {
                assert!(
                    !active.load(Ordering::SeqCst),
                    "settings changed during active turn"
                );
                for (key, value) in params.as_object().unwrap() {
                    settings[key] = value.clone();
                }
                if settings["modelId"] == "new-default" && params.get("reasoningEffort").is_none() {
                    settings["reasoningEffort"] = "low".into();
                }
                if settings["modelId"] == "coerce" {
                    settings["reasoningEffort"] = "off".into();
                }
                if settings["modelId"] == "cap" {
                    settings["availableAutonomyLevels"] = json!(["low"]);
                }
                normalize_auto_reasoning(&mut settings);
                let request_id = if settings["modelId"] == "uncorrelated-settings" {
                    json!(format!("unrelated-{}", id.as_str().unwrap()))
                } else {
                    id.clone()
                };
                notify(
                    json!({"type":"settings_updated","requestId":request_id,"settings":settings}),
                );
                reply(id, json!({}));
            }
            "droid.interrupt_session" => {
                pending = false;
                active.store(false, Ordering::SeqCst);
                reply(id, json!({}));
                done(if prompt == "stop-completes" {
                    "completed"
                } else {
                    "cancelled"
                });
            }
            "droid.add_user_message" => {
                reply(id, json!({}));
                let next = params["text"].as_str().unwrap();
                if next == "discard-steer" {
                    notify(json!({"type":"queued_messages_discarded","requestId":id,"text":next}));
                    active.store(false, Ordering::SeqCst);
                    done("completed");
                    continue;
                }
                notify(
                    json!({"type":"create_message","requestId":id,"message":{"role":"user","content":[{"type":"text","text":next}]}}),
                );
                let was_active = active.swap(true, Ordering::SeqCst);
                prompt = next.into();
                if was_active {
                    text(&format!("steer:{next}"));
                    active.store(false, Ordering::SeqCst);
                    done("completed");
                    continue;
                }
                match next {
                    "settings" => text(&settings.to_string()),
                    "child-transcript"
                    | "session-ids"
                    | "late-append"
                    | "settle-without-tool"
                    | "settle-empty-tool"
                    | "settle-mismatched-tool" => {
                        let stamped = next == "session-ids";
                        let late_append = next == "late-append";
                        let settle_tool = match next {
                            "settle-without-tool" => None,
                            "settle-empty-tool" => Some(""),
                            "settle-mismatched-tool" => Some("different-tool"),
                            _ => Some("spawn"),
                        };
                        let emit = move |value| {
                            if stamped {
                                notify_session(session, value);
                            } else {
                                notify(value);
                            }
                        };
                        let child = "11111111-1111-4111-8111-111111111111";
                        let root = std::path::PathBuf::from(
                            std::env::var_os("FACTORY_HOME_OVERRIDE").unwrap(),
                        );
                        let project = root.join(".factory/sessions/opaque-worktree");
                        std::fs::create_dir_all(&project).unwrap();
                        let path = project.join(format!("{child}.jsonl"));
                        let mut file = std::fs::File::create(path).unwrap();
                        for record in [
                            json!({"type":"session_start","id":child,"callingSessionId":session,"callingToolUseId":"spawn"}),
                            json!({"type":"message","id":"context","message":{"role":"user","content":[{"type":"text","text":"PRIVATE CONTEXT"}]}}),
                            json!({"type":"message","id":"call","message":{"role":"assistant","content":[{"type":"thinking","encrypted_content":"OPAQUE THINKING"},{"type":"tool_use","id":"child-read","name":"Read","input":{"file_path":"fixture.txt"}}]}}),
                        ] {
                            writeln!(file, "{record}").unwrap();
                        }
                        file.flush().unwrap();
                        emit(
                            json!({"type":"tool_call","toolUse":{"id":"spawn","name":"Task","input":{"description":"fixture child"}}}),
                        );
                        notify_session(
                            child,
                            json!({"type":"assistant_text_delta","textDelta":"NOT PARENT TEXT"}),
                        );
                        emit(
                            json!({"type":"child_session_available","childSessionId":child,"toolUseId":"spawn"}),
                        );
                        emit(json!({"type":"assistant_text_delta","textDelta":"initial"}));
                        active.store(false, Ordering::SeqCst);
                        emit(json!({"type":"agent_turn_completed","reason":"completed"}));
                        std::thread::spawn(move || {
                            std::thread::sleep(Duration::from_millis(350));
                            let result = json!({"type":"message","id":"result","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"child-read","content":"fixture contents","is_error":false}]}}).to_string();
                            let (first, last) = result.split_at(result.len() / 2);
                            write!(file, "{first}").unwrap();
                            file.flush().unwrap();
                            std::thread::sleep(Duration::from_millis(300));
                            writeln!(file, "{last}").unwrap();
                            file.flush().unwrap();
                            let mut settled = json!({"type":"child_session_settled","childSessionId":child,"status":"completed"});
                            if let Some(tool) = settle_tool {
                                settled["toolUseId"] = tool.into();
                            }
                            emit(settled);
                            emit(json!({"type":"assistant_text_delta","textDelta":"autonomous"}));
                            emit(json!({"type":"agent_turn_completed","reason":"completed"}));
                            std::thread::sleep(Duration::from_millis(if late_append {
                                1500
                            } else {
                                100
                            }));
                            writeln!(file, "{}", json!({"type":"message","id":"final","message":{"role":"assistant","content":[{"type":"text","text":"child finished"}]}})).unwrap();
                            file.flush().unwrap();
                        });
                        continue;
                    }
                    "background" => {
                        notify(json!({"type":"child_session_available","childSessionId":"child"}));
                        text("initial");
                        active.store(false, Ordering::SeqCst);
                        done("completed");
                        std::thread::spawn(move || {
                            std::thread::sleep(Duration::from_millis(150));
                            notify(
                                json!({"type":"child_session_settled","childSessionId":"child","status":"completed"}),
                            );
                            text("autonomous");
                            done("completed");
                        });
                        continue;
                    }
                    "hold" | "stop-completes" => {
                        text("working");
                        continue;
                    }
                    "delayed" => {
                        text("working");
                        let active = active.clone();
                        std::thread::spawn(move || {
                            std::thread::sleep(Duration::from_millis(150));
                            active.store(false, Ordering::SeqCst);
                            done("completed");
                        });
                        continue;
                    }
                    "stubborn" => {
                        stubborn = true;
                        #[cfg(unix)]
                        unsafe {
                            libc::signal(libc::SIGTERM, libc::SIG_IGN);
                        }
                        text(&format!("pid:{}", std::process::id()));
                        continue;
                    }
                    "crash" => {
                        eprintln!("fixture crashed deliberately");
                        std::process::exit(42);
                    }
                    "require-resume" if !loaded => {
                        eprintln!("resume required");
                        std::process::exit(43);
                    }
                    "auth" => {
                        notify(json!({"type":"error","message":"No access token available"}));
                        active.store(false, Ordering::SeqCst);
                        done("model_authentication_failed");
                        continue;
                    }
                    "error" => {
                        notify(json!({"type":"error","message":"provider refused"}));
                        active.store(false, Ordering::SeqCst);
                        done("model_rate_limited");
                        continue;
                    }
                    "tools" => {
                        tool("tool", "Create", json!({}));
                        tool(
                            "tool",
                            "Create",
                            json!({"file_path":"note.txt","content":"hello"}),
                        );
                        notify(
                            json!({"type":"tool_execution_phase_changed","toolUseId":"tool","phase":"queued"}),
                        );
                        tool("tool", "Create", json!({"file_path":"WRONG"}));
                        notify(
                            json!({"type":"tool_execution_phase_changed","toolUseId":"tool","phase":"queued"}),
                        );
                        notify(
                            json!({"type":"tool_result","toolUseId":"tool","content":[{"type":"text","text":"ok"}],"isError":false}),
                        );
                    }
                    value if value.starts_with("permission:") => {
                        pending = true;
                        permission(&value[11..], settings["autonomyLevel"].as_str().unwrap());
                        continue;
                    }
                    "ask" => {
                        pending = true;
                        send(
                            json!({"type":"request","id":"ask","method":"droid.ask_user","params":{"questions":[{"index":1,"topic":"Color","question":"Pick colors","options":["Red","Blue"],"multiSelect":true}]}}),
                        );
                        continue;
                    }
                    "pick" => {
                        pending = true;
                        send(
                            json!({"type":"request","id":"pick","method":"droid.pick_options","params":{}}),
                        );
                        continue;
                    }
                    _ => {
                        notify(json!({"type":"thinking_text_delta","textDelta":"thinking"}));
                        for part in ["reply:", next, ":done"] {
                            text(part);
                        }
                        notify(
                            json!({"type":"assistant_text_complete","messageId":"assistant","blockIndex":0}),
                        );
                        notify(
                            json!({"type":"create_message","message":{"role":"assistant","content":[{"type":"text","text":"NEVER DUPLICATE"}]}}),
                        );
                    }
                }
                active.store(false, Ordering::SeqCst);
                done("completed");
                if next == "idle-crash" {
                    std::thread::spawn(|| {
                        std::thread::sleep(Duration::from_millis(100));
                        std::process::exit(0);
                    });
                }
            }
            _ => error(id, -32601, "Unknown method"),
        }
    }
    if stubborn {
        loop {
            std::thread::sleep(Duration::from_secs(60));
        }
    }
}
