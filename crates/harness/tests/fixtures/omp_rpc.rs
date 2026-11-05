//! Stateful cross-platform Oh My Pi RPC peer (protocol v1/v2, `prompt_result`
//! completion); never built into the application.
use base64::Engine;
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    io::{BufRead, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

/// OMP's physical stdout frame cap.
const MAX_FRAME: usize = 1024 * 1024;
static V2: AtomicBool = AtomicBool::new(false);
static CHUNKS: AtomicU64 = AtomicU64::new(0);

fn emit(v: Value) {
    let text = v.to_string();
    let mut out = std::io::stdout().lock();
    if text.len() < MAX_FRAME {
        writeln!(out, "{text}").unwrap();
    } else if V2.load(Ordering::SeqCst) {
        // Arbitrary byte boundaries: a chunk may split a UTF-8 character.
        let bytes = text.as_bytes();
        let parts: Vec<_> = bytes.chunks(700_000).collect();
        let id = format!("rpc-{}", CHUNKS.fetch_add(1, Ordering::SeqCst));
        for (index, part) in parts.iter().enumerate() {
            let frame = json!({"type":"rpc_chunk","chunkId":id,"index":index,"count":parts.len(),"byteLength":bytes.len(),"data":base64::engine::general_purpose::STANDARD.encode(part)});
            writeln!(out, "{frame}").unwrap();
        }
    } else {
        let frame = json!({"type":"response","id":v["id"],"command":v["command"],"success":false,"error":"RPC response exceeded the transport limit"});
        writeln!(out, "{frame}").unwrap();
    }
    out.flush().unwrap();
}
fn response(v: &Value, data: Value) {
    emit(json!({"type":"response","id":v["id"],"command":v["type"],"success":true,"data":data}));
}
fn ack(v: &Value) {
    emit(json!({"type":"response","id":v["id"],"command":v["type"],"success":true}));
}
fn failure(v: &Value, error: &str) {
    emit(json!({"type":"response","id":v["id"],"command":v["type"],"success":false,"error":error}));
}
fn prompt_result(id: &str, invoked: bool, status: &str, settled: bool) {
    let mut frame = json!({"type":"prompt_result","id":id,"agentInvoked":invoked,"status":status,"sessionSettled":settled});
    if status == "error" {
        frame["error"] = json!({"message":"mock provider failure","retryable":false});
    }
    emit(frame);
}
fn message(text: &str, stop: &str) {
    emit(
        json!({"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":text}],"stopReason":stop,"errorMessage":"mock provider failure","usage":{"input":12,"output":3}}}),
    );
}
fn reply(text: &str) {
    emit(json!({"type":"message_start","message":{"role":"assistant"}}));
    emit(
        json!({"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"reply:"}}),
    );
    message(&format!("reply:{text}"), "stop");
}
fn user(text: &str) {
    emit(
        json!({"type":"message_start","message":{"role":"user","content":[{"type":"text","text":text}]}}),
    );
}
fn queue_update(queue: &VecDeque<(String, String)>) {
    let steering: Vec<_> = queue.iter().map(|(_, text)| text).collect();
    emit(json!({"type":"queue_update","steering":steering,"followUp":[]}));
    std::fs::write(
        "pending-steers.json",
        serde_json::to_vec(&steering).unwrap(),
    )
    .unwrap();
}
fn commands() -> Value {
    json!({"commands":[
        {"name":"compact","source":"builtin","description":"Compact context","input":{"hint":"[instructions]"}},
        {"name":"new","source":"builtin","description":"Start a new session"},
        {"name":"noop","source":"extension","description":"handled"},
        {"name":"skill:probe","source":"skill","description":"skill"}
    ]})
}

struct Session {
    id: String,
    file: Option<PathBuf>,
}
impl Session {
    /// OMP writes the file at the first turn: a padded title slot, then the header.
    fn persist(&self) {
        if let Some(file) = &self.file
            && !file.exists()
        {
            let title = json!({"type":"title","v":1,"title":"","pad":" ".repeat(64)});
            let header = json!({"type":"session","version":3,"id":self.id,"cwd":std::env::current_dir().unwrap()});
            std::fs::write(file, format!("{title}\n{header}\n")).unwrap();
        }
    }
}

#[derive(Clone)]
struct Run {
    queue: Arc<Mutex<VecDeque<(String, String)>>>,
    active: Arc<AtomicBool>,
    abort: Arc<AtomicBool>,
    steering_all: Arc<AtomicBool>,
    session: Arc<Mutex<Session>>,
}
impl Run {
    fn start(self, id: String, text: String) {
        std::thread::spawn(move || {
            let mut ids = vec![id];
            emit(json!({"type":"agent_start"}));
            self.session.lock().unwrap().persist();
            user(&text);
            let mut text = text;
            loop {
                if text == "burst-start" {
                    emit(json!({"type":"message_start","message":{"role":"assistant"}}));
                    emit(
                        json!({"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"waiting"}}),
                    );
                    while !Path::new("release-burst").exists() && !self.abort.load(Ordering::SeqCst)
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
                        if self.abort.load(Ordering::SeqCst) {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(10));
                    }
                }
                if text == "retry" {
                    message("", "error");
                    // A non-yielding end completes no prompt.
                    emit(
                        json!({"type":"agent_end","messages":[],"isTerminal":false,"yielded":false}),
                    );
                    std::thread::sleep(Duration::from_millis(150));
                    emit(json!({"type":"agent_start"}));
                }
                if text == "auto-compact" {
                    emit(
                        json!({"type":"agent_end","messages":[],"isTerminal":false,"yielded":false}),
                    );
                    emit(json!({"type":"auto_compaction_start","reason":"threshold"}));
                    std::thread::sleep(Duration::from_millis(100));
                    emit(
                        json!({"type":"auto_compaction_end","result":{"summary":"s","tokensBefore":100}}),
                    );
                    emit(json!({"type":"agent_start"}));
                }
                if self.abort.load(Ordering::SeqCst) {
                    message("", "aborted");
                } else if text == "error" {
                    message("", "error");
                } else {
                    reply(&text);
                }
                // Held through session_settled so a new prompt starts a new run.
                let mut queued = self.queue.lock().unwrap();
                if !self.abort.load(Ordering::SeqCst) && !queued.is_empty() {
                    // Steering drains at the step boundary, inside the same run.
                    let count = if self.steering_all.load(Ordering::SeqCst) {
                        queued.len()
                    } else {
                        1
                    };
                    let mut batch = Vec::new();
                    for _ in 0..count {
                        let (id, next) = queued.pop_front().unwrap();
                        queue_update(&queued);
                        user(&next);
                        ids.push(id);
                        batch.push(next);
                    }
                    text = batch.join("\n");
                    continue;
                }
                emit(json!({"type":"agent_end","messages":[],"isTerminal":true,"yielded":true}));
                let status = if self.abort.load(Ordering::SeqCst) {
                    "aborted"
                } else if text == "error" {
                    "error"
                } else {
                    "completed"
                };
                let background = text == "background" && status == "completed";
                for id in &ids {
                    prompt_result(id, true, status, !background);
                }
                if background {
                    // A background job wakes the agent after the yield.
                    std::thread::sleep(Duration::from_millis(150));
                    emit(json!({"type":"agent_start"}));
                    emit(json!({"type":"message_start","message":{"role":"assistant"}}));
                    message("background done", "stop");
                    emit(
                        json!({"type":"agent_end","messages":[],"isTerminal":true,"yielded":true}),
                    );
                }
                self.active.store(false, Ordering::SeqCst);
                emit(json!({"type":"session_settled"}));
                drop(queued);
                if text == "late-notify" {
                    std::thread::spawn(|| {
                        std::thread::sleep(Duration::from_millis(30));
                        emit(
                            json!({"type":"extension_ui_request","id":"late","method":"notify","message":"background notice"}),
                        );
                    });
                }
                break;
            }
        });
    }
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    if args.iter().any(|a| a == "--version") {
        println!("omp/18.11.1");
        return;
    }
    if args.get(1).is_some_and(|a| a == "models") {
        println!(
            "{}",
            json!({"models":[
                {"provider":"mock","id":"mock","name":"Mock","kind":"chat","reasoning":true,"thinking":["low","medium","high"],"contextWindow":128000},
                {"provider":"mock","id":"mock-2","name":"Mock 2","kind":"chat","reasoning":true,"thinking":["low","medium","high","xhigh"],"contextWindow":128000},
                {"provider":"mock","id":"image","name":"Image","kind":"image","reasoning":false,"thinking":null}
            ]})
        );
        return;
    }
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(30));
        std::process::exit(99);
    });
    if args.iter().any(|a| a == "--no-themes") {
        eprintln!("error: unknown option '--no-themes'");
        std::process::exit(1);
    }
    let ephemeral = args.iter().any(|a| a == "--no-session");
    let resume = args
        .iter()
        .position(|a| a == "--resume")
        .map(|i| PathBuf::from(&args[i + 1]));
    let session = match &resume {
        Some(file) => {
            let text = std::fs::read_to_string(file).unwrap();
            let header: Value = serde_json::from_str(text.lines().nth(1).unwrap()).unwrap();
            Session {
                id: header["id"].as_str().unwrap().into(),
                file: Some(file.clone()),
            }
        }
        None if ephemeral => Session {
            id: "omp-probe".into(),
            file: None,
        },
        None => Session {
            id: "omp-fixture-session".into(),
            file: Some(
                std::env::current_dir()
                    .unwrap()
                    .join("omp-fixture-session.jsonl"),
            ),
        },
    };
    if !ephemeral {
        std::fs::write("omp.pid", std::process::id().to_string()).unwrap();
    }
    let v1_only = Path::new("v1-only").exists();
    emit(
        json!({"type":"ready","protocolVersion":1,"supportedProtocolVersions":if v1_only {json!([1])} else {json!([1,2])},"maxFrameBytes":MAX_FRAME,"maxReassembledFrameBytes":64 * 1024 * 1024}),
    );
    emit(
        json!({"type":"extension_ui_request","id":"loaded","method":"notify","message":"extension loaded"}),
    );
    let mut catalog = commands();
    catalog["type"] = json!("available_commands_update");
    emit(catalog);
    let run = Run {
        queue: Default::default(),
        active: Default::default(),
        abort: Default::default(),
        steering_all: Default::default(),
        session: Arc::new(Mutex::new(session)),
    };
    let mut dialog = None;
    let mut model = "mock".to_string();
    let mut thinking = "medium".to_string();
    for line in std::io::stdin().lock().lines() {
        let v: Value = serde_json::from_str(&line.unwrap()).unwrap();
        match v["type"].as_str().unwrap_or("") {
            "negotiate_protocol" if v1_only => failure(&v, "Unsupported protocol version"),
            "negotiate_protocol" => {
                response(&v, json!({"protocolVersion":2}));
                V2.store(true, Ordering::SeqCst);
            }
            "get_state" => {
                let session = run.session.lock().unwrap();
                let active = run.active.load(Ordering::SeqCst);
                let mut state = json!({"sessionId":session.id,"isStreaming":active,"isSettled":!active,"isCompacting":false,"model":{"id":model,"provider":"mock","contextWindow":128000},"thinkingLevel":thinking});
                if let Some(file) = &session.file {
                    state["sessionFile"] = json!(file);
                }
                if Path::new("big-state").exists() {
                    state["systemPrompt"] = json!("é".repeat(800_000));
                }
                drop(session);
                response(&v, state);
            }
            "get_available_models" => response(
                &v,
                json!({"models":[{"id":"mock","provider":"mock","name":"Mock","reasoning":true,"contextWindow":128000},{"id":"mock-2","provider":"mock","name":"Mock 2","reasoning":true,"contextWindow":128000}]}),
            ),
            "get_available_thinking_levels" => {
                response(&v, json!({"levels":["off","low","medium","high"]}))
            }
            "set_model" => {
                model = v["modelId"].as_str().unwrap().into();
                response(&v, json!({"id":model,"provider":v["provider"]}));
            }
            "set_thinking_level" => {
                thinking = v["level"].as_str().unwrap().into();
                ack(&v);
            }
            "set_steering_mode" => {
                run.steering_all.store(v["mode"] == "all", Ordering::SeqCst);
                std::fs::write("steering-mode", v["mode"].as_str().unwrap_or("")).unwrap();
                ack(&v);
            }
            "get_available_commands" => response(&v, commands()),
            // Pi commands OMP does not serve.
            "get_commands" | "clear_queue" => {
                let mut log = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open("unknown-commands")
                    .unwrap();
                writeln!(log, "{}", v["type"].as_str().unwrap()).unwrap();
                failure(
                    &v,
                    &format!("Unknown command: {}", v["type"].as_str().unwrap()),
                );
            }
            "prompt" => {
                let id = v["id"].as_str().unwrap().to_owned();
                let mut text = v["message"].as_str().unwrap_or("").to_owned();
                match text.as_str() {
                    "env" => text = format!("env:{}", std::env::var_os("CLAUDECODE").is_some()),
                    "which-model" => text = format!("{model}/{thinking}"),
                    "agent-dir" => {
                        text = format!(
                            "agent-dir:{}|{}",
                            std::env::var("PI_CODING_AGENT_DIR").unwrap_or_default(),
                            std::env::var("OMP_PROFILE").unwrap_or_default()
                        )
                    }
                    _ => {}
                }
                if text == "reject" {
                    // Failed before reaching the agent: the error response, then its result.
                    failure(&v, "preflight rejected");
                    prompt_result(&id, false, "error", true);
                    continue;
                }
                let command = text
                    .strip_prefix('/')
                    .and_then(|c| c.split_whitespace().next());
                if let Some(name) = command.filter(|c| {
                    matches!(*c, "compact" | "session" | "name" | "autocompact" | "new")
                }) {
                    // Builtins finish locally; the response is their completion.
                    if name == "new" {
                        let mut session = run.session.lock().unwrap();
                        session.id = "omp-fixture-session-2".into();
                        session.file = Some(
                            std::env::current_dir()
                                .unwrap()
                                .join("omp-fixture-session-2.jsonl"),
                        );
                    }
                    emit(json!({"type":"command_output","text":format!("ran /{name}")}));
                    response(&v, json!({"agentInvoked":false}));
                    continue;
                }
                if text == "/noop" || text == "handled" {
                    // An extension command (or input hook) admitted, then finished without a turn.
                    ack(&v);
                    prompt_result(&id, false, "completed", true);
                    continue;
                }
                if text.starts_with("/dialog") {
                    ack(&v);
                    dialog = Some(id);
                    emit(
                        json!({"type":"extension_ui_request","id":"question","method":text.split_whitespace().nth(1).unwrap_or("input"),"title":"Choose","options":["first","second"],"prefill":"initial\nvalue"}),
                    );
                    continue;
                }
                #[cfg(unix)]
                if matches!(text.as_str(), "tree" | "inherited-pipe-crash") {
                    #[allow(
                        clippy::zombie_processes,
                        reason = "the harness must reap this tool child with the process tree"
                    )]
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
                let mut queued = run.queue.lock().unwrap();
                if run.active.load(Ordering::SeqCst) {
                    if v["streamingBehavior"].is_null() {
                        failure(&v, "streamingBehavior is required while streaming");
                        continue;
                    }
                    queued.push_back((id, text));
                    queue_update(&queued);
                    ack(&v);
                    continue;
                }
                run.active.store(true, Ordering::SeqCst);
                run.abort.store(false, Ordering::SeqCst);
                drop(queued);
                ack(&v);
                run.clone().start(id, text);
            }
            "abort_and_restore_queue" => {
                let withdrawn: Vec<_> = {
                    let mut queued = run.queue.lock().unwrap();
                    let withdrawn = queued.drain(..).collect();
                    if run.active.load(Ordering::SeqCst) {
                        queue_update(&queued);
                    }
                    withdrawn
                };
                run.abort.store(true, Ordering::SeqCst);
                for (id, _) in &withdrawn {
                    prompt_result(id, true, "aborted", true);
                }
                // Responds after the abort completes.
                for _ in 0..400 {
                    if !run.active.load(Ordering::SeqCst) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                let steering: Vec<_> = withdrawn.iter().map(|(_, t)| json!({"text":t})).collect();
                response(&v, json!({"steering":steering,"followUp":[]}));
            }
            "abort" => {
                run.abort.store(true, Ordering::SeqCst);
                ack(&v);
            }
            "extension_ui_response" => {
                if let Some(id) = dialog.take() {
                    emit(
                        json!({"type":"extension_ui_request","id":"notice","method":"notify","message":v.to_string()}),
                    );
                    prompt_result(&id, false, "completed", true);
                }
            }
            _ => ack(&v),
        }
    }
}
