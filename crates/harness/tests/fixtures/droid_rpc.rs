//! Synthetic Factory 1.10.0 peer. Frame fields follow the recorded live wire
//! described in src/droid/PROTOCOL.md, including a separate settings barrier.
use serde_json::{Value, json};
use std::io::{self, BufRead, Write};

fn write(frame: Value) {
    println!("{frame}");
    io::stdout().flush().unwrap();
}
fn response(id: &Value, result: Value) {
    write(
        json!({"jsonrpc":"2.0","factoryApiVersion":"1.0.0","type":"response","id":id,"result":result}),
    );
}
fn notification(value: Value) {
    write(
        json!({"jsonrpc":"2.0","factoryApiVersion":"1.0.0","type":"notification","method":"droid.session_notification","params":{"sessionId":"droid-fixture-session","notification":value}}),
    );
}
fn completed(id: &str, reason: &str) {
    notification(json!({"type":"agent_turn_completed","turnId":id,"reason":reason}));
}
fn request(id: &str, method: &str, params: Value) {
    write(
        json!({"jsonrpc":"2.0","factoryApiVersion":"1.0.0","type":"request","id":id,"method":method,"params":params}),
    );
}

fn title_requests(stage: &str) {
    request(
        &format!("title-{stage}-permission"),
        "droid.request_permission",
        json!({"toolUses":[{"toolUse":{"id":"forbidden","name":"Read","input":{"file_path":"secret"}}}],"options":[{"value":"proceed_once"},{"value":"cancel"}]}),
    );
    request(
        &format!("title-{stage}-ask"),
        "droid.ask_user",
        json!({"toolCallId":"ask","questions":[{"index":1,"topic":"Forbidden","question":"Do not ask","options":["Yes","No"]}]}),
    );
    request(
        &format!("title-{stage}-unknown"),
        "droid.unknown_title_method",
        json!({}),
    );
}

const LIVE_CHILD: &str = "11111111-1111-4111-8111-111111111111";
const FAILED_CHILD: &str = "22222222-2222-4222-8222-222222222222";

fn message(file: &mut std::fs::File, id: &str, role: &str, content: Value) {
    writeln!(
        file,
        "{}",
        json!({"type":"message","id":id,"message":{"role":role,"content":content}})
    )
    .unwrap();
    file.flush().unwrap();
}

fn subagents() {
    let home = std::path::PathBuf::from(std::env::var_os("FACTORY_HOME_OVERRIDE").unwrap());
    let project = home.join(".factory/sessions/child-worktree");
    std::fs::create_dir_all(&project).unwrap();
    let mut files = vec![];
    for (id, tool, description) in [
        (LIVE_CHILD, "task-live", "Inspect live"),
        (FAILED_CHILD, "task-failed", "Inspect failure"),
    ] {
        notification(json!({"type":"tool_call","toolUse":{"id":tool,"name":"Task","input":{}}}));
        notification(
            json!({"type":"tool_call","toolUse":{"id":tool,"name":"Task","input":{"description":description,"prompt":"Read fixture.txt"}}}),
        );
        let mut file = std::fs::File::create(project.join(format!("{id}.jsonl"))).unwrap();
        writeln!(file, "{}", json!({"type":"session_start","id":id,"callingSessionId":"droid-fixture-session","callingToolUseId":tool})).unwrap();
        message(
            &mut file,
            "context",
            "user",
            json!([{"type":"text","text":"PRIVATE_CONTEXT"}]),
        );
        message(
            &mut file,
            "thinking",
            "assistant",
            json!([{"type":"thinking","thinking":"Inspecting"},{"type":"redacted_thinking","data":"PRIVATE_ENCRYPTED"}]),
        );
        for _ in 0..2 {
            message(
                &mut file,
                "read",
                "assistant",
                json!([{"type":"text","text":"Checking"},{"type":"tool_use","id":"read-1","name":"Read","input":{"file_path":"fixture.txt"}}]),
            );
        }
        for _ in 0..2 {
            notification(
                json!({"type":"child_session_available","childSessionId":id,"toolUseId":tool,"description":description,"timestamp":1}),
            );
        }
        // The Task result acknowledges launch, not the child's completion.
        notification(
            json!({"type":"tool_result","toolUseId":tool,"content":"Task launched","isError":false}),
        );
        files.push((id, tool, file));
    }
    // This independent binding must never steal the first child's transcript.
    notification(
        json!({"type":"child_session_available","childSessionId":LIVE_CHILD,"toolUseId":"task-failed","timestamp":2}),
    );
    std::thread::spawn(move || {
        while !home.join("release").exists() {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        for (id, tool, mut file) in files {
            let failed = id == FAILED_CHILD;
            message(
                &mut file,
                "result",
                "user",
                json!([{"type":"tool_result","tool_use_id":"read-1","content":"fixture contents","is_error":failed}]),
            );
            for _ in 0..2 {
                notification(
                    json!({"type":"child_session_settled","childSessionId":id,"status":if failed {"failed"} else {"completed"},"timestamp":3}),
                );
            }
            // Settlement can beat the final append, including a torn line.
            write!(file, "{{\"type\":\"message\",\"id\":\"final\",").unwrap();
            file.flush().unwrap();
            std::thread::sleep(std::time::Duration::from_millis(50));
            writeln!(file, "\"message\":{{\"role\":\"assistant\",\"content\":[{{\"type\":\"text\",\"text\":\"Final {tool}\"}}]}}}}").unwrap();
            file.flush().unwrap();
        }
    });
}

fn main() {
    if std::env::args().any(|arg| arg == "--version") {
        println!("1.10.0");
        return;
    }
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(60));
        std::process::exit(99);
    });
    let mut settings = json!({
        "modelId":"fixture-mini", "reasoningEffort":"low", "autonomyLevel":"off", "interactionMode":"auto",
        "toolExecutionMode":"direct_only", "availableAutonomyLevels":["off","low","medium","high"]
    });
    let mut active = String::new();
    let mut ongoing = false;
    let mut children_started = false;
    let mut title = false;
    let mut configured = false;
    let scenario = std::env::var("DROID_FIXTURE_SCENARIO").unwrap_or_default();
    for line in io::stdin().lock().lines() {
        let line = line.unwrap();
        if let Ok(log) = std::env::var("DROID_FIXTURE_LOG") {
            writeln!(
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(log)
                    .unwrap(),
                "{line}"
            )
            .unwrap();
        }
        let frame: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(frame["jsonrpc"], "2.0");
        assert_eq!(frame["factoryApiVersion"], "1.0.0");
        assert!(frame["id"].is_string());
        if frame["type"] == "response" {
            if let Some(id) = frame["id"].as_str().filter(|id| id.starts_with("title-")) {
                if id.ends_with("-permission") {
                    assert_eq!(frame["result"]["selectedOption"], "cancel");
                } else if id.ends_with("-ask") {
                    assert_eq!(frame["result"], json!({"cancelled":true,"answers":[]}));
                } else {
                    assert_eq!(frame["error"]["code"], -32601);
                }
                if id == "title-turn-unknown" {
                    completed(&active, "cancelled");
                }
                continue;
            }
            let cancelled = frame["result"]["selectedOption"] == "cancel"
                || frame["result"]["cancelled"] == true;
            completed(&active, if cancelled { "cancelled" } else { "completed" });
            continue;
        }
        assert_eq!(frame["type"], "request");
        let id = &frame["id"];
        let params = &frame["params"];
        match frame["method"].as_str().unwrap() {
            "droid.list_models" => response(
                id,
                json!({"models":[
                    {"id":"auto","displayName":"Auto Model","supportedReasoningEfforts":["none"]},
                    {"id":"fixture-mini","displayName":"Fixture Mini","supportedReasoningEfforts":["none","low","high"]},
                    {"id":"fixture-mini-fast","supportedReasoningEfforts":["minimal","xhigh","max"]},
                    {"id":"disabled","disabled":true}
                ]}),
            ),
            "droid.load_session" => match scenario.as_str() {
                "missing" => write(
                    json!({"jsonrpc":"2.0","type":"response","factoryApiVersion":"1.0.0","id":id,"error":{"code":-32004,"message":"Session not found"}}),
                ),
                "load-error" => write(
                    json!({"jsonrpc":"2.0","type":"response","factoryApiVersion":"1.0.0","id":id,"error":{"code":-32001,"message":"Authentication required"}}),
                ),
                _ => response(
                    id,
                    json!({"settings":settings,"cwd":std::env::current_dir().unwrap()}),
                ),
            },
            "droid.initialize_session" => {
                title = params.get("systemPrompt").is_some();
                if title {
                    assert_eq!(params["systemPrompt"], zeron_harness::TITLE_INSTRUCTIONS);
                    assert_eq!(params["restrictToolIds"], json!(["__no-tools__"]));
                    for key in [
                        "disableBuiltinSkills",
                        "autoRejectPermissionRequests",
                        "blockOnStartupLoad",
                        "blockOnMcpLoad",
                    ] {
                        assert_eq!(params[key], true, "{key}");
                    }
                    assert_eq!(params["autonomyLevel"], "off");
                    assert_eq!(params["interactionMode"], "auto");
                    assert_eq!(params["privacyLevel"], "private");
                    for key in ["toolExecutionMode", "mcpServers", "sessionId", "worktree"] {
                        assert!(params.get(key).is_none(), "{key}");
                    }
                    settings["systemPrompt"] = params["systemPrompt"].clone();
                    if scenario == "title-init-system" {
                        settings["systemPrompt"] = json!("Coding prompt");
                    }
                    if scenario == "title-requests" {
                        title_requests("startup");
                    }
                } else {
                    assert_eq!(params["toolExecutionMode"], "direct_only");
                }
                assert_eq!(
                    std::env::var("FACTORY_DROID_AUTO_UPDATE_ENABLED").unwrap(),
                    "false"
                );
                response(
                    id,
                    json!({"sessionId":"droid-fixture-session", "settings":settings}),
                );
            }
            "droid.update_session_settings" => {
                configured = true;
                for (key, value) in params.as_object().unwrap() {
                    settings[key] = value.clone();
                }
                if scenario == "coerced" {
                    settings["autonomyLevel"] = json!("high");
                }
                if scenario == "title-requests" {
                    title_requests("settings");
                }
                response(id, json!({}));
                notification(
                    json!({"type":"settings_updated","requestId":"unrelated","settings":params}),
                );
                if children_started && scenario == "children-at-settings" {
                    notification(
                        json!({"type":"child_session_settled","childSessionId":"child-1","status":"completed","timestamp":3}),
                    );
                    children_started = false;
                }
                if scenario != "no-barrier" {
                    notification(
                        json!({"type":"settings_updated","requestId":id,"settings":settings}),
                    );
                }
            }
            "droid.list_tools" => {
                assert!(title);
                assert!(configured, "verify after the model/settings barrier");
                assert_eq!(
                    params,
                    &json!({}),
                    "query actual session, not hypothetical overrides"
                );
                if scenario == "title-requests" {
                    title_requests("tools");
                }
                let result = match scenario.as_str() {
                    "title-tools-enabled" => {
                        json!({"tools":[{"id":"Read","currentlyAllowed":true}]})
                    }
                    "title-tools-missing" => json!({}),
                    "title-tools-missing-allowed" => json!({"tools":[{"id":"Read"}]}),
                    _ => json!({"tools":[
                        {"id":"read-cli","currentlyAllowed":false},
                        {"id":"script-cli","currentlyAllowed":false},
                        {"id":"mcp_global","currentlyAllowed":false}
                    ]}),
                };
                response(id, result);
            }
            "droid.add_user_message" => {
                response(id, json!({}));
                let message_id = format!("turn-{}", id.as_str().unwrap());
                if !ongoing {
                    active = message_id.clone();
                }
                if matches!(params["text"].as_str(), Some("ack-steer" | "discard-steer")) {
                    notification(json!({"type":"assistant_text_delta","textDelta":"queued"}));
                    if params["text"] == "discard-steer" {
                        notification(
                            json!({"type":"queued_messages_discarded","requestId":id,"text":params["text"]}),
                        );
                        completed(&active, "completed");
                        ongoing = false;
                    }
                    continue;
                }
                notification(
                    json!({"type":"create_message","requestId":id,"message":{"id":message_id,"role":"user","content":[{"type":"text","text":params["text"]}]}}),
                );
                if title {
                    assert!(params.get("imagePaths").is_none());
                    if scenario == "title-requests" {
                        title_requests("turn");
                    } else {
                        notification(
                            json!({"type":"assistant_text_delta","textDelta":"Fixture Thread Title"}),
                        );
                        completed(&active, "completed");
                    }
                    continue;
                }
                match params["text"].as_str().unwrap() {
                    "subagents-live" => {
                        subagents();
                        notification(
                            json!({"type":"assistant_text_delta","textDelta":"parent-complete"}),
                        );
                        completed(&active, "completed");
                        ongoing = false;
                    }
                    "children" => {
                        ongoing = true;
                        children_started = true;
                        notification(
                            json!({"type":"tool_call","toolUse":{"id":"task-1","name":"Task","input":{"description":"Compute"}}}),
                        );
                        notification(
                            json!({"type":"child_session_available","childSessionId":"child-1","toolUseId":"task-1","description":"Compute","timestamp":1}),
                        );
                        notification(
                            json!({"type":"child_session_available","childSessionId":"child-1","toolUseId":"task-1","timestamp":2}),
                        );
                        notification(
                            json!({"type":"assistant_text_delta","textDelta":"child-running"}),
                        );
                    }
                    "settle" => {
                        for text in ["17. Confirm", "17. Confirmed."] {
                            notification(
                                json!({"type":"tool_progress_update","toolUseId":"output-1","toolName":"TaskOutput","update":{"type":"message","text":text,"subagentSessionId":"child-1","timestamp":3}}),
                            );
                        }
                        for _ in 0..2 {
                            notification(
                                json!({"type":"child_session_settled","childSessionId":"child-1","toolUseId":"task-1","status":"completed","timestamp":4}),
                            );
                        }
                        notification(
                            json!({"type":"assistant_text_delta","textDelta":"child-settled"}),
                        );
                        completed(&active, "completed");
                        ongoing = false;
                    }
                    text if text.starts_with("plan") => {
                        notification(
                            json!({"type":"assistant_text_delta","textDelta":"Here is the plan."}),
                        );
                        notification(
                            json!({"type":"tool_call","toolUse":{"type":"tool_use","id":"spec-tool","name":"ExitSpecMode","input":{"plan":"1. Make the change."}}}),
                        );
                        let mut options = vec![
                            "proceed_once",
                            "proceed_auto_run_medium",
                            "proceed_auto_run_high",
                            "cancel",
                        ];
                        if text == "plan-missing-option" {
                            options.retain(|v| *v != "proceed_auto_run_high");
                        }
                        let plan = match text {
                            "plan-empty" => "".into(),
                            "plan-huge" => "x".repeat(65537),
                            _ => "1. Make the change.".into(),
                        };
                        request(
                            "permission-1",
                            "droid.request_permission",
                            json!({
                                "toolUses":[{"toolUse":{"type":"tool_use","id":"spec-tool","name":"ExitSpecMode","input":{}},"confirmationType":"exit_spec_mode","details":{"type":"exit_spec_mode","title":"Fixture plan","plan":plan}}],
                                "options":options.into_iter().map(|value| json!({"value":value,"label":value})).collect::<Vec<_>>()
                            }),
                        );
                    }
                    "permission-edit" | "permission-mixed" => {
                        let mut tools = vec![
                            json!({"toolUse":{"type":"tool_use","id":"edit-1","name":"Edit","input":{"file_path":"a.txt"}},"confirmationType":"edit","details":{"type":"edit","filePath":"a.txt"}}),
                        ];
                        if params["text"] == "permission-mixed" {
                            tools.push(json!({"toolUse":{"type":"tool_use","id":"exec-1","name":"Execute","input":{"command":"cargo test"}},"confirmationType":"execute","details":{"type":"execute","command":"cargo test"}}));
                        }
                        request(
                            "permission-1",
                            "droid.request_permission",
                            json!({"toolUses":tools,"options":[{"value":"proceed_once"},{"value":"proceed_always"},{"value":"cancel"}]}),
                        );
                    }
                    "question" => {
                        request(
                            "question-1",
                            "droid.ask_user",
                            json!({"toolCallId":"ask-1","questions":[
                                {"index":7,"topic":"Colors","question":"Choose colors","options":["Red","Blue"],"multiSelect":true},
                                {"index":9,"topic":"Name","question":"What name?","options":[]}
                            ]}),
                        );
                    }
                    "eof" => return,
                    "ack-only" | "wait" => {
                        ongoing = true;
                        notification(json!({"type":"assistant_text_delta","textDelta":"waiting"}));
                        notification(
                            json!({"type":"droid_working_state_changed","newState":"idle"}),
                        );
                    }
                    "tools" => {
                        notification(
                            json!({"type":"tool_call","toolUse":{"type":"tool_use","id":"tool-1","name":"Execute","input":{}}}),
                        );
                        notification(
                            json!({"type":"tool_call","toolUse":{"type":"tool_use","id":"tool-1","name":"Execute","input":{"command":"printf hello"}}}),
                        );
                        notification(
                            json!({"type":"tool_result","toolUseId":"tool-1","content":"hello","isError":false}),
                        );
                        notification(json!({"type":"thinking_text_delta","textDelta":"thinking"}));
                        notification(
                            json!({"type":"session_token_usage_changed","tokenUsage":{"inputTokens":10,"outputTokens":4}}),
                        );
                        notification(json!({"type":"assistant_text_delta","textDelta":"done"}));
                        completed(&active, "completed");
                    }
                    "auth-terminal" => completed(&active, "model_authentication_failed"),
                    "author-error" => {
                        notification(json!({"type":"error","message":"Could not find the author"}));
                        completed(&active, "error");
                    }
                    text => {
                        notification(json!({"type":"assistant_text_delta","textDelta":text}));
                        completed(&active, "completed");
                        ongoing = false;
                    }
                }
            }
            "droid.interrupt_session" => {
                completed(&active, "cancelled");
                ongoing = false;
                response(id, json!({}));
            }
            other => panic!("unexpected method {other}"),
        }
    }
}
