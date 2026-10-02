//! Two-device e2e for agent-spawned chats (`scripts/e2e-agent-chats.sh` runs it).
//!
//! Drives real `zeron mcp` stdio servers — the process an agent's harness
//! spawns — against two running headless engines (A and B, one user, synced
//! through a real edge). The "agent" is the coordinator chat on A: its MCP
//! server carries `ZERON_CHAT_ID`, exactly as the engine injects it.
//!
//! 1. A and B see each other as online execution hosts (`list_devices`);
//! 2. the coordinator (on A) spawns a TOP-LEVEL chat on B with a prompt and
//!    waits for B's mock harness to answer;
//! 3. it spawns a SIDE chat on B the same way;
//! 4. `list_chats { spawned_by }` and `read_chat` agree from BOTH engines;
//! 5. follow-up `send_message … wait`, `interrupt_chat`, validation errors;
//! 6. the top-level chat on B spawns further chats until the depth guard;
//! 7. optionally (`ZERON_E2E_AGENT_MODEL`), a real harness in a chat on A
//!    makes the create_chat call itself through the engine-injected server.
//!
//! Usage: agent_chats_e2e <zeron-bin> <a-ipc-port> <b-ipc-port> [evidence.json]
//! Prints `PASS`/`FAIL` lines; exits nonzero on failure.

use std::process::Stdio;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

const MOCK_TEXT: &str = "Mock harness reporting in.";
const STEP_TIMEOUT: Duration = Duration::from_secs(120);

fn fail(message: &str) -> ! {
    eprintln!("FAIL: {message}");
    std::process::exit(1);
}

fn pass(message: &str) {
    println!("PASS: {message}");
}

/// One `zeron mcp` child, spoken to over newline-delimited JSON-RPC.
struct Mcp {
    label: String,
    _child: Child,
    stdin: ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
    next_id: u64,
}

impl Mcp {
    async fn spawn(bin: &str, port: u16, chat: Option<&str>, label: &str) -> Self {
        let mut command = Command::new(bin);
        command
            .arg("mcp")
            .env("ZERON_IPC_PORT", port.to_string())
            .env_remove("ZERON_CHAT_ID")
            .env_remove("ZERON_DEVICE_ID")
            .env("RUST_LOG", "warn")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        if let Some(chat) = chat {
            command.env("ZERON_CHAT_ID", chat);
        }
        let mut child = command
            .spawn()
            .unwrap_or_else(|e| fail(&format!("{label}: spawn {bin} mcp: {e}")));
        let stdin = child.stdin.take().unwrap();
        let lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let mut mcp = Self {
            label: label.to_owned(),
            _child: child,
            stdin,
            lines,
            next_id: 1,
        };
        let init = mcp
            .request(
                "initialize",
                json!({ "protocolVersion": "2025-06-18", "capabilities": {},
                        "clientInfo": { "name": "agent_chats_e2e", "version": "1" } }),
            )
            .await;
        if init["result"]["serverInfo"]["name"] != "zeron" {
            fail(&format!("{label}: bad initialize reply {init}"));
        }
        mcp
    }

    async fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let line = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        self.stdin
            .write_all(format!("{line}\n").as_bytes())
            .await
            .unwrap_or_else(|e| fail(&format!("{}: write: {e}", self.label)));
        self.stdin.flush().await.ok();
        loop {
            let next = tokio::time::timeout(Duration::from_secs(900), self.lines.next_line()).await;
            let line = match next {
                Ok(Ok(Some(line))) => line,
                other => fail(&format!("{}: no reply to {method}: {other:?}", self.label)),
            };
            let Ok(reply) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if reply["id"] == json!(id) {
                return reply;
            }
        }
    }

    /// `Ok(structuredContent)` or `Err(error text)` (tool-level `isError`).
    async fn call(&mut self, tool: &str, args: Value) -> Result<Value, String> {
        let reply = self
            .request("tools/call", json!({ "name": tool, "arguments": args }))
            .await;
        let result = &reply["result"];
        if result["isError"] == true {
            return Err(result["content"][0]["text"]
                .as_str()
                .unwrap_or_default()
                .to_owned());
        }
        if result.is_null() {
            return Err(format!("protocol error: {reply}"));
        }
        Ok(result["structuredContent"].clone())
    }

    async fn ok(&mut self, tool: &str, args: Value) -> Value {
        match self.call(tool, args.clone()).await {
            Ok(value) => value,
            Err(err) => fail(&format!("{}: {tool} {args}: {err}", self.label)),
        }
    }
}

/// Call `tool` until `check` accepts the outcome, or fail after
/// [`STEP_TIMEOUT`] (rows and transcripts arrive through sync, not at once).
async fn poll<T>(
    mcp: &mut Mcp,
    what: &str,
    tool: &str,
    args: Value,
    check: impl Fn(Result<Value, String>) -> Option<T>,
) -> T {
    let deadline = Instant::now() + STEP_TIMEOUT;
    loop {
        let outcome = mcp.call(tool, args.clone()).await;
        let last = format!("{outcome:?}");
        if let Some(value) = check(outcome) {
            return value;
        }
        if Instant::now() > deadline {
            fail(&format!("{}: timed out waiting for {what}; last: {last}", mcp.label));
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

fn replies_text(turn: &Value) -> String {
    turn["replies"]
        .as_array()
        .map(|r| {
            r.iter()
                .filter_map(|m| m["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

fn chat_in<'a>(listed: &'a Value, id: &str) -> Option<&'a Value> {
    listed["chats"].as_array()?.iter().find(|c| c["id"] == id)
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        fail("usage: agent_chats_e2e <zeron-bin> <a-ipc-port> <b-ipc-port> [evidence.json]");
    }
    let bin = args[1].clone();
    let a_port: u16 = args[2].parse().unwrap_or_else(|_| fail("bad A port"));
    let b_port: u16 = args[3].parse().unwrap_or_else(|_| fail("bad B port"));
    let evidence_path = args.get(4).cloned();
    let mut evidence = serde_json::Map::new();

    // ── 0. Plain (unattributed) servers on both engines ────────────────────────
    let mut a = Mcp::spawn(&bin, a_port, None, "A").await;
    let mut b = Mcp::spawn(&bin, b_port, None, "B").await;
    let a_dev = a.ok("whoami", json!({})).await["localDeviceId"]
        .as_str()
        .unwrap()
        .to_owned();
    let b_dev = b.ok("whoami", json!({})).await["localDeviceId"]
        .as_str()
        .unwrap()
        .to_owned();
    if a_dev == b_dev {
        fail("A and B share a device id");
    }

    // ── 1. Each engine sees the other as an online execution host ─────────────
    for (mcp, other) in [(&mut a, &b_dev), (&mut b, &a_dev)] {
        let listed = poll(mcp, "peer online", "list_devices", json!({}), |r| {
            let listed = r.ok()?;
            let peer = listed["devices"]
                .as_array()?
                .iter()
                .find(|d| d["id"] == other.as_str())?
                .clone();
            (peer["online"] == true && peer["executionHost"] == true).then_some(listed)
        })
        .await;
        evidence.insert(format!("list_devices_from_{}", mcp.label), listed);
    }
    pass("both engines list the other as an online execution host");

    // Chats below ask for the `mock` harness explicitly: deterministic, and
    // honoured by the MCP although the test rig is hidden from pickers.
    let remote = a
        .ok("list_harnesses", json!({ "device": b_dev.as_str() }))
        .await;
    if remote["deviceId"] != b_dev.as_str() {
        fail(&format!("list_harnesses did not route to B: {remote}"));
    }
    pass("list_harnesses { device: B } answers from B through targetDeviceId");

    // ── 2. The coordinator: a user-style top-level chat on A ──────────────────
    let coordinator = a
        .ok(
            "create_chat",
            json!({ "kind": "chat", "harness": "mock", "title": "Coordinator" }),
        )
        .await;
    let coordinator_id = coordinator["chatId"].as_str().unwrap().to_owned();
    if !coordinator["spawnedByChatId"].is_null() {
        fail("an unattributed create must not record provenance");
    }
    // Everything below speaks AS the coordinator's agent.
    let mut agent = Mcp::spawn(&bin, a_port, Some(&coordinator_id), "agent@A").await;
    let who = poll(&mut agent, "coordinator row", "whoami", json!({}), |r| {
        r.ok().filter(|w| w["spawning"]["spawnDepth"].is_number())
    })
    .await;
    if who["spawning"]["spawnDepth"] != 0 || who["spawning"]["canCreateChats"] != true {
        fail(&format!("coordinator spawning info: {who}"));
    }

    // ── 3. A top-level chat on B, run to completion ───────────────────────────
    let worker = agent
        .ok(
            "create_chat",
            json!({ "kind": "chat", "device": b_dev.as_str(), "harness": "mock",
                    "title": "Worker on B", "prompt": "Run the job on device B",
                    "wait": true, "timeout_secs": 120 }),
        )
        .await;
    let worker_id = worker["chatId"].as_str().unwrap().to_owned();
    if worker["kind"] != "chat"
        || worker["deviceId"] != b_dev.as_str()
        || !worker["parentChatId"].is_null()
        || worker["spawnedByChatId"] != coordinator_id.as_str()
    {
        fail(&format!("top-level worker shape: {worker}"));
    }
    if worker["turn"]["outcome"] != "completed" || !replies_text(&worker["turn"]).contains(MOCK_TEXT)
    {
        fail(&format!("top-level worker turn: {}", worker["turn"]));
    }
    evidence.insert("create_chat_top_level_on_B".into(), worker.clone());
    pass("agent on A spawned a top-level chat on B; B's engine ran the prompt");

    // ── 4. A side chat on B ────────────────────────────────────────────────────
    let side = agent
        .ok(
            "create_chat",
            json!({ "kind": "side", "device": b_dev.as_str(), "harness": "mock",
                    "title": "Side check on B", "prompt": "Quick side check",
                    "wait": true, "timeout_secs": 120 }),
        )
        .await;
    let side_id = side["chatId"].as_str().unwrap().to_owned();
    if side["kind"] != "side"
        || side["parentChatId"] != coordinator_id.as_str()
        || side["spawnedByChatId"] != coordinator_id.as_str()
        || side["turn"]["outcome"] != "completed"
    {
        fail(&format!("side chat shape: {side}"));
    }
    evidence.insert("create_chat_side_on_B".into(), side.clone());
    pass("agent on A spawned a side chat on B; B's engine ran the prompt");

    // ── 5. Both engines agree: list_chats + read_chat ─────────────────────────
    for mcp in [&mut a, &mut b] {
        let label = mcp.label.clone();
        let args = json!({ "spawned_by": coordinator_id.as_str() });
        let listed = poll(mcp, "spawned chats synced", "list_chats", args, |r| {
            let listed = r.ok()?;
            let top = chat_in(&listed, &worker_id)?;
            let side = chat_in(&listed, &side_id)?;
            (listed["total"] == 2
                && top["kind"] == "chat"
                && top["deviceId"] == b_dev.as_str()
                && side["kind"] == "side"
                && side["parentChatId"] == coordinator_id.as_str())
            .then_some(listed.clone())
        })
        .await;
        evidence.insert(format!("list_chats_spawned_by_from_{label}"), listed);
        for id in [&worker_id, &side_id] {
            let args = json!({ "chat": id.as_str() });
            let read = poll(mcp, "transcript synced", "read_chat", args, |r| {
                let read = r.ok()?;
                read["messages"]
                    .to_string()
                    .contains(MOCK_TEXT)
                    .then_some(read)
            })
            .await;
            if id == &worker_id {
                evidence.insert(format!("read_chat_worker_from_{label}"), read);
            }
        }
        pass(&format!(
            "engine {label}: list_chats {{spawned_by}} + read_chat see both spawned chats"
        ));
    }

    // ── 6. Running them afterwards: send/wait, interrupt, get_chat ────────────
    let follow = agent
        .ok(
            "send_message",
            json!({ "chat": worker_id.as_str(), "text": "One more pass, please",
                    "wait": true, "timeout_secs": 120 }),
        )
        .await;
    if follow["turn"]["outcome"] != "completed" {
        fail(&format!("follow-up turn: {follow}"));
    }
    let transcript = agent
        .ok("read_chat", json!({ "chat": worker_id.as_str() }))
        .await;
    if !transcript["messages"]
        .to_string()
        .contains("[Message from Zeron chat Coordinator")
    {
        fail("follow-up was not attributed to the coordinator");
    }
    evidence.insert("send_message_follow_up".into(), follow);
    pass("send_message … wait round-trips to B, attributed to the coordinator");
    let interrupted = agent
        .ok("interrupt_chat", json!({ "chat": worker_id.as_str() }))
        .await;
    if interrupted["commandId"].as_str().is_none_or(str::is_empty) {
        fail(&format!("interrupt: {interrupted}"));
    }
    let waited = agent
        .ok(
            "wait_for_turn",
            json!({ "chat": side_id.as_str(), "timeout_secs": 30 }),
        )
        .await;
    if waited["turn"]["outcome"] != "completed" {
        fail(&format!("wait_for_turn on the idle side chat: {waited}"));
    }
    pass("interrupt_chat queues on the remote chat; wait_for_turn reads B's session row");

    // ── 7. Validation: bad targets name the valid hosts ───────────────────────
    let err = agent
        .call("create_chat", json!({ "kind": "chat", "device": "no-such-device" }))
        .await
        .expect_err("unknown device must fail");
    if !err.contains("no device matches") || !err.contains(&b_dev) {
        fail(&format!("unknown-device error: {err}"));
    }
    let err = agent
        .call(
            "create_chat",
            json!({ "kind": "chat", "parent": coordinator_id.as_str() }),
        )
        .await
        .expect_err("parent with kind chat must fail");
    evidence.insert("validation_errors".into(), json!([err]));
    pass("invalid targets and kind/parent combinations are refused with guidance");

    // ── 8. A top-level spawned chat on B orchestrates further, up to the limit ─
    let mut from_worker = Mcp::spawn(&bin, b_port, Some(&worker_id), "agent@B").await;
    let grandchild = from_worker
        .ok(
            "create_chat",
            json!({ "kind": "chat", "device": a_dev.as_str(), "harness": "mock",
                    "title": "Depth 2 on A" }),
        )
        .await;
    let depth2 = grandchild["chatId"].as_str().unwrap().to_owned();
    let side_from_b = from_worker
        .ok(
            "create_chat",
            json!({ "harness": "mock", "title": "Side of the B worker" }),
        )
        .await;
    if side_from_b["kind"] != "side" || side_from_b["parentChatId"] != worker_id.as_str() {
        fail(&format!("B worker's side chat: {side_from_b}"));
    }
    let mut from_depth2 = Mcp::spawn(&bin, a_port, Some(&depth2), "agent@A(depth2)").await;
    let args = json!({ "kind": "chat", "harness": "mock", "title": "Depth 3" });
    let depth3 = poll(&mut from_depth2, "depth-2 spawn", "create_chat", args, |r| r.ok()).await;
    let depth3_id = depth3["chatId"].as_str().unwrap().to_owned();
    let mut from_depth3 = Mcp::spawn(&bin, a_port, Some(&depth3_id), "agent@A(depth3)").await;
    // The depth-3 row must have synced before its agent can be judged; a
    // "no chat matches" is that race, a successful create would be the bug.
    let args = json!({ "kind": "chat", "harness": "mock" });
    let err = poll(&mut from_depth3, "depth-3 refusal", "create_chat", args, |r| match r {
        Err(err) if err.contains("agent spawns deep") => Some(err),
        Ok(created) => fail(&format!("depth 3 created a chat: {created}")),
        Err(_) => None,
    })
    .await;
    evidence.insert("depth_guard_error".into(), json!(err));
    pass("spawned top-level chats spawn further chats across devices until depth 3");

    // ── 9. Optional: a real agent does the spawning ───────────────────────────
    // ZERON_E2E_AGENT_MODEL (e.g. opencode/big-pickle) runs a real harness in a
    // chat on A; the engine injects its Zeron MCP server into that run, and the
    // model itself must call create_chat to start a top-level chat on B.
    if let Ok(model) = std::env::var("ZERON_E2E_AGENT_MODEL") {
        let harness =
            std::env::var("ZERON_E2E_AGENT_HARNESS").unwrap_or_else(|_| "opencode".into());
        let prompt = format!(
            "You are testing Zeron orchestration. Call the Zeron MCP tool create_chat \
             (it may be exposed as zeron_create_chat) exactly once with these arguments: \
             kind \"chat\", device \"{b_dev}\", harness \"mock\", \
             title \"Spawned by a real agent\", prompt \"Say hello from device B\", \
             wait true. Then reply with only the chatId it returned."
        );
        let real = a
            .ok(
                "create_chat",
                json!({ "kind": "chat", "harness": harness, "model": model,
                        "title": "Real agent on A", "prompt": prompt,
                        "wait": true, "timeout_secs": 600 }),
            )
            .await;
        let real_id = real["chatId"].as_str().unwrap().to_owned();
        let args = json!({ "spawned_by": real_id.as_str() });
        let spawned = poll(&mut b, "the real agent's chat on B", "list_chats", args, |r| {
            let listed = r.ok()?;
            listed["chats"]
                .as_array()?
                .iter()
                .find(|c| c["kind"] == "chat" && c["deviceId"] == b_dev.as_str())
                .cloned()
        })
        .await;
        let spawned_id = spawned["id"].as_str().unwrap().to_owned();
        let args = json!({ "chat": spawned_id.as_str() });
        poll(&mut b, "the spawned chat's reply on B", "read_chat", args, |r| {
            r.ok()?.to_string().contains(MOCK_TEXT).then_some(())
        })
        .await;
        // The spawner's doc keeps the ids its create call made (the fold's
        // `created_chat_ids`), so its transcript links to the chat exactly.
        let args = json!({ "chat": real_id.as_str(), "include_tools": true });
        let agent_transcript = poll(&mut a, "created ids in the transcript", "read_chat", args, |r| {
            let read = r.ok()?;
            read["messages"]
                .to_string()
                .contains(&format!("[created: {spawned_id}]"))
                .then_some(read)
        })
        .await;
        evidence.insert("real_agent_turn".into(), real["turn"].clone());
        evidence.insert("real_agent_transcript".into(), agent_transcript);
        evidence.insert("real_agent_spawned_chat_on_B".into(), spawned);
        pass(&format!(
            "a real {harness} agent ({model}) on A spawned a top-level chat on B that ran"
        ));
        for id in [&real_id, &spawned_id] {
            a.call("archive_chat", json!({ "chat": id.as_str() })).await.ok();
        }
    }

    // Tidy: archive what we spawned (the side chats go with the listing).
    for id in [&worker_id, &side_id, &depth2, &depth3_id] {
        agent
            .call("archive_chat", json!({ "chat": id.as_str() }))
            .await
            .ok();
    }
    if let Some(path) = evidence_path {
        std::fs::write(&path, serde_json::to_string_pretty(&Value::Object(evidence)).unwrap())
            .unwrap_or_else(|e| fail(&format!("write {path}: {e}")));
        println!("evidence: {path}");
    }
    println!("PASS: agent-spawned chats across two devices");
}
