//! Stateful cross-platform Pi RPC peer; never built into the application.
use serde_json::{Value, json};
use std::{
    io::{BufRead, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
fn emit(v: Value) {
    let mut out = std::io::stdout().lock();
    writeln!(out, "{v}").unwrap();
    out.flush().unwrap();
}
fn response(v: &Value, data: Value) {
    emit(json!({"type":"response","id":v["id"],"command":v["type"],"success":true,"data":data}));
}
fn append_message(file: &str, message: Value) -> String {
    let parent = std::fs::read_to_string(file)
        .unwrap()
        .lines()
        .last()
        .and_then(|line| serde_json::from_str::<Value>(line).ok())
        .and_then(|v| {
            (v["type"] != "session")
                .then(|| v["id"].as_str().map(str::to_owned))
                .flatten()
        });
    let id = uuid::Uuid::new_v4().to_string();
    let mut native = std::fs::OpenOptions::new().append(true).open(file).unwrap();
    writeln!(native,"{}",json!({"type":"message","id":id,"parentId":parent,"timestamp":"2026-10-01T00:00:00Z","message":message})).unwrap();
    id
}
fn message(file: &str, session: &str, text: &str, stop: &str) {
    let message = json!({"role":"assistant","content":[{"type":"text","text":text}],"stopReason":stop,"errorMessage":"mock provider failure","usage":{"input":12,"output":3}});
    emit(json!({"type":"message_end","message":message}));
    let entry = append_message(file, message);
    if stop == "stop" {
        emit(
            json!({"type":"extension_ui_request","method":"notify","message":format!("zeron-native-fork-v1:{}",json!({"sessionId":session,"entryId":entry}))}),
        );
    }
}
fn queue_update(queue: &std::collections::VecDeque<String>) {
    emit(json!({"type":"queue_update","steering":queue,"followUp":[]}));
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    if args.iter().any(|a| a == "--version") {
        println!("0.85.1");
        return;
    }
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(30));
        std::process::exit(99);
    });
    let file = args
        .iter()
        .position(|a| a == "--session")
        .map(|i| args[i + 1].clone())
        .unwrap_or_else(|| {
            std::env::current_dir()
                .unwrap()
                .join("fixture-session.jsonl")
                .display()
                .to_string()
        });
    let session = std::fs::read_to_string(&file)
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(s.lines().next()?).ok())
        .and_then(|v| v["id"].as_str().map(str::to_owned))
        .unwrap_or_else(|| "pi-fixture-session".into());
    if !args.iter().any(|a| a == "--no-session") && !std::path::Path::new(&file).exists() {
        std::fs::write(
            &file,
            format!(
                "{}\n",
                json!({"type":"session","version":3,"id":session,"cwd":std::env::current_dir().unwrap()})
            ),
        )
        .unwrap();
    }
    if !args.iter().any(|a| a == "--no-session") {
        std::fs::write("pi.pid", std::process::id().to_string()).unwrap();
    }
    let queue = Arc::new(Mutex::new(std::collections::VecDeque::<String>::new()));
    let active = Arc::new(AtomicBool::new(false));
    let abort = Arc::new(AtomicBool::new(false));
    let steering_all = Arc::new(AtomicBool::new(false));
    let mut dialog = None;
    let mut model = "mock".to_string();
    let mut thinking = "medium".to_string();
    for line in std::io::stdin().lock().lines() {
        let v: Value = serde_json::from_str(&line.unwrap()).unwrap();
        match v["type"].as_str().unwrap_or("") {
            "get_state" => response(
                &v,
                json!({"sessionId":session,"sessionFile":file,"isStreaming":active.load(Ordering::SeqCst),"isCompacting":false,"model":{"id":model,"provider":"mock","contextWindow":128000},"thinkingLevel":thinking}),
            ),
            "get_available_models" => response(
                &v,
                json!({"models":[{"id":"mock","provider":"mock","name":"Mock","reasoning":true,"contextWindow":128000},{"id":"mock-2","provider":"mock","name":"Mock 2","reasoning":true,"contextWindow":128000}]}),
            ),
            "get_available_thinking_levels" => {
                response(&v, json!({"levels":["off","low","medium","high"]}))
            }
            "set_model" => {
                model = v["modelId"].as_str().unwrap().into();
                response(&v, json!({"id":model}));
            }
            "set_thinking_level" => {
                thinking = v["level"].as_str().unwrap().into();
                response(&v, json!({}));
            }
            "set_steering_mode" => {
                steering_all.store(v["mode"] == "all", Ordering::SeqCst);
                // Recorded so tests can prove when Zeron leaves the mode alone.
                std::fs::write("steering-mode", v["mode"].as_str().unwrap_or("")).unwrap();
                response(&v, json!({}));
            }
            "get_commands" => response(
                &v,
                json!({"commands":[{"name":"noop","description":"handled","source":"extension"},{"name":"skill:probe","description":"skill","source":"skill"}]}),
            ),
            "prompt" => {
                let mut text = v["message"].as_str().unwrap_or("").to_owned();
                if text == "env" {
                    text = format!("env:{}", std::env::var_os("CLAUDECODE").is_some());
                }
                if text == "which-model" {
                    text = format!("{model}/{thinking}");
                }
                if text == "reject" {
                    emit(
                        json!({"type":"response","id":v["id"],"command":"prompt","success":false,"error":"preflight rejected"}),
                    );
                    continue;
                }
                if text == "/noop" || text == "handled" {
                    response(&v, json!({}));
                    continue;
                }
                if text.starts_with("/dialog") {
                    dialog = Some(v.clone());
                    emit(
                        json!({"type":"extension_ui_request","id":"question","method":text.split_whitespace().nth(1).unwrap_or("input"),"title":"Choose","options":["first","second"],"prefill":"initial\nvalue"}),
                    );
                    continue;
                }
                #[cfg(unix)]
                if matches!(text.as_str(), "tree" | "inherited-pipe-crash") {
                    let child = std::process::Command::new("sh")
                        .args(["-c", "sleep 30"])
                        .spawn()
                        .unwrap();
                    std::fs::write("tool.pid", child.id().to_string()).unwrap();
                    if text == "inherited-pipe-crash" {
                        eprintln!("inherited pipe diagnostic");
                        std::process::exit(7);
                    }
                }
                if text == "crash" {
                    eprintln!("mock crash diagnostic");
                    std::process::exit(7);
                }
                let mut queued = queue.lock().unwrap();
                if active.load(Ordering::SeqCst) {
                    queued.push_back(text);
                    queue_update(&queued);
                    response(&v, json!({}));
                    std::fs::write("pending-steers.json", serde_json::to_vec(&*queued).unwrap())
                        .unwrap();
                    continue;
                }
                active.store(true, Ordering::SeqCst);
                drop(queued);
                abort.store(false, Ordering::SeqCst);
                response(&v, json!({}));
                emit(json!({"type":"agent_start"}));
                emit(
                    json!({"type":"message_start","message":{"role":"user","content":[{"type":"text","text":text}]}}),
                );
                append_message(&file, json!({"role":"user","content":text,"timestamp":1}));
                let active = active.clone();
                let abort = abort.clone();
                let queue = queue.clone();
                let steering_all = steering_all.clone();
                let file = file.clone();
                let session = session.clone();
                std::thread::spawn(move || {
                    let mut text = text;
                    loop {
                        if text == "burst-start" {
                            emit(json!({"type":"message_start","message":{"role":"assistant"}}));
                            emit(
                                json!({"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"waiting"}}),
                            );
                            while !std::path::Path::new("release-burst").exists()
                                && !abort.load(Ordering::SeqCst)
                            {
                                std::thread::sleep(Duration::from_millis(5));
                            }
                        }
                        if text == "tree" {
                            emit(
                                json!({"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"tree ready"}}),
                            );
                        }
                        if text == "slow" || text == "tree" {
                            for _ in 0..100 {
                                if abort.load(Ordering::SeqCst) {
                                    break;
                                }
                                std::thread::sleep(Duration::from_millis(10));
                            }
                        }
                        if text == "retry" {
                            message(&file, &session, "", "error");
                            emit(json!({"type":"agent_end","willRetry":true}));
                            std::thread::sleep(Duration::from_millis(150));
                            emit(json!({"type":"agent_start"}));
                        }
                        if text == "compact" {
                            emit(json!({"type":"agent_end"}));
                            emit(json!({"type":"compaction_start"}));
                            std::thread::sleep(Duration::from_millis(100));
                            emit(
                                json!({"type":"compaction_end","result":{"estimatedTokensAfter":9}}),
                            );
                        }
                        if abort.load(Ordering::SeqCst) {
                            message(&file, &session, "", "aborted");
                        } else if text == "error" {
                            message(&file, &session, "", "error");
                        } else {
                            emit(json!({"type":"message_start","message":{"role":"assistant"}}));
                            emit(
                                json!({"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"reply:"}}),
                            );
                            message(&file, &session, &format!("reply:{text}"), "stop");
                        }
                        emit(json!({"type":"agent_end"}));
                        let mut queued = queue.lock().unwrap();
                        if !abort.load(Ordering::SeqCst) && !queued.is_empty() {
                            let count = if steering_all.load(Ordering::SeqCst) {
                                queued.len()
                            } else {
                                1
                            };
                            let mut batch = Vec::new();
                            for _ in 0..count {
                                let next = queued.pop_front().unwrap();
                                queue_update(&queued);
                                emit(
                                    json!({"type":"message_start","message":{"role":"user","content":[{"type":"text","text":next}]}}),
                                );
                                append_message(
                                    &file,
                                    json!({"role":"user","content":next,"timestamp":1}),
                                );
                                batch.push(next);
                            }
                            text = batch.join("\n");
                            continue;
                        }
                        active.store(false, Ordering::SeqCst);
                        emit(json!({"type":"agent_settled"}));
                        if text == "late-notify" {
                            std::thread::spawn(|| {
                                std::thread::sleep(Duration::from_millis(30));
                                emit(
                                    json!({"type":"extension_ui_request","method":"notify","message":"background notice"}),
                                );
                            });
                        }
                        break;
                    }
                });
            }
            "steer" => {
                response(&v, json!({}));
                emit(
                    json!({"type":"message_start","message":{"role":"user","content":[{"type":"text","text":v["message"]}]}}),
                );
            }
            "clear_queue" => {
                queue.lock().unwrap().clear();
                response(&v, json!({"steering":[],"followUp":[]}));
            }
            "abort" => {
                abort.store(true, Ordering::SeqCst);
                response(&v, json!({}));
            }
            "extension_ui_response" => {
                if let Some(prompt) = dialog.take() {
                    emit(
                        json!({"type":"extension_ui_request","id":"notice","method":"notify","message":v.to_string()}),
                    );
                    response(&prompt, json!({}));
                }
            }
            _ => response(&v, json!({})),
        }
    }
}
