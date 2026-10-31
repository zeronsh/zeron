//! Contract tests use the adapter's actual parser, not a duplicate wire schema.
#![cfg(unix)]
#[allow(dead_code)]
#[path = "../../harness/src/claude/wire.rs"]
mod wire;

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdout, Command};
use zeron_bench::probe::{ProbeEvent, ProbeSock};
use zeron_bench::replay::script::{Script, text_sha256};

static NEXT: AtomicU64 = AtomicU64::new(0);
fn scratch(label: &str) -> PathBuf {
    let root = PathBuf::from("/home/user/Documents/dev/tmp/zeron-perf/scratch/b3-replay/tests")
        .join(format!(
            "{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
    std::fs::create_dir_all(&root).unwrap();
    root
}
fn script(path: &Path, frames: &[Value], extra: &[Value], text: &str) {
    let mut lines = vec![
        json!({"zb":"meta","v":1,"name":"test","seed":42,
        "text_bytes":text.len(),"text_sha256":text_sha256(text),"frames":frames.len()})
        .to_string(),
    ];
    lines.extend(frames.iter().map(|f| json!({"dt":0,"f":f}).to_string()));
    lines.extend(extra.iter().map(Value::to_string));
    std::fs::write(path, lines.join("\n") + "\n").unwrap();
}
fn text_frames(text: &str) -> Vec<Value> {
    let mut frames = vec![
        json!({"type":"stream_event","session_id":"$SESSION_ID","event":{
        "type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}}),
    ];
    frames.extend(text.as_bytes().chunks(100).map(|s| json!({"type":"stream_event","session_id":"$SESSION_ID","event":{
        "type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":std::str::from_utf8(s).unwrap()}}})));
    frames.push(json!({"type":"stream_event","event":{"type":"content_block_stop","index":0}}));
    frames
}
struct Peer {
    child: Child,
    lines: Lines<BufReader<ChildStdout>>,
}
impl Peer {
    fn start(root: &Path, extra: &[(&str, &str)], args: &[&str]) -> Result<Self> {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_zb-replay"));
        cmd.env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", root)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("XDG_CONFIG_HOME", root.join("xdg"))
            .env("ZB_REPLAY_STATE_DIR", root.join("state"))
            .env("ZB_REPLAY_LOG_DIR", root.join("log"))
            .current_dir(root)
            .args(["--print", "--replay-user-messages"])
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .kill_on_drop(true);
        for (k, v) in extra {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn()?;
        let lines = BufReader::new(child.stdout.take().unwrap()).lines();
        Ok(Self { child, lines })
    }
    async fn send(&mut self, f: Value) -> Result<()> {
        let stdin = self.child.stdin.as_mut().context("closed stdin")?;
        stdin.write_all(format!("{f}\n").as_bytes()).await?;
        stdin.flush().await?;
        Ok(())
    }
    async fn next(&mut self) -> Result<Value> {
        let line = tokio::time::timeout(Duration::from_secs(5), self.lines.next_line())
            .await??
            .context("unexpected EOF")?;
        wire::parse_frame(&line)?;
        Ok(serde_json::from_str(&line)?)
    }
    async fn until_result(&mut self) -> Result<Vec<Value>> {
        let mut frames = Vec::new();
        loop {
            let f = self.next().await?;
            let end = f["type"] == "result";
            frames.push(f);
            if end {
                return Ok(frames);
            }
        }
    }
    async fn stop(mut self) -> Result<()> {
        self.child.stdin.take();
        ensure!(
            tokio::time::timeout(Duration::from_secs(5), self.child.wait())
                .await??
                .success(),
            "replay exited unsuccessfully"
        );
        Ok(())
    }
}
fn user(text: &str) -> Value {
    json!({"type":"user","uuid":"cmd-1","message":{"role":"user","content":text},"parent_tool_use_id":null})
}
fn deltas(frames: &[Value]) -> String {
    frames
        .iter()
        .filter(|f| {
            f["event"]["delta"]["type"] == "text_delta" && f["parent_tool_use_id"].is_null()
        })
        .filter_map(|f| f["event"]["delta"]["text"].as_str())
        .collect()
}

#[tokio::test]
async fn initialize_and_lifecycle_golden_under_50ms() -> Result<()> {
    let root = scratch("handshake");
    let mut peer = Peer::start(
        &root,
        &[
            ("ZB_REPLAY_STARTUP_MS", "200"),
            ("ZB_REPLAY_BALLAST_MIB", "300"),
        ],
        &[],
    )?;
    // Includes process startup and the large allocation racing initialize.
    let start = Instant::now();
    peer.send(serde_json::from_str(&wire::initialize_request_line(
        "probe",
    ))?)
    .await?;
    let response = peer.next().await?;
    let latency = start.elapsed();
    assert!(
        latency < Duration::from_millis(50),
        "initialize took {latency:?}"
    );
    assert_eq!(
        response,
        serde_json::from_str::<Value>(include_str!("fixtures/replay-initialize.json"))?
    );
    peer.send(user("zb:echo hello")).await?;
    let frames = peer.until_result().await?;
    let lifecycle: Vec<_> = frames
        .iter()
        .filter(|f| f["type"] == "command_lifecycle")
        .cloned()
        .collect();
    assert_eq!(
        lifecycle,
        serde_json::from_str::<Vec<Value>>(include_str!("fixtures/replay-lifecycle.json"))?
    );
    assert_eq!(deltas(&frames), "hello");
    let mut result = frames.last().unwrap().clone();
    result["session_id"] = "$SESSION_ID".into();
    assert_eq!(
        result,
        serde_json::from_str::<Value>(include_str!("fixtures/replay-result.json"))?
    );
    assert_eq!(frames.iter().filter(|f| f["subtype"] == "init").count(), 1);
    eprintln!(
        "initialize latency: {:.3} ms",
        latency.as_secs_f64() * 1000.0
    );
    peer.stop().await
}

#[tokio::test]
async fn live_fixture_frames_roundtrip_through_real_wire_parser() -> Result<()> {
    let fixtures =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../harness/tests/fixtures/claude");
    let mut files: Vec<_> = std::fs::read_dir(fixtures)?
        .flatten()
        .map(|f| f.path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("live-")
                && p.extension().is_some_and(|x| x == "jsonl")
        })
        .collect();
    files.sort();
    assert_eq!(
        files.len(),
        6,
        "update the pinned live fixture corpus intentionally"
    );
    let mut types = std::collections::BTreeSet::new();
    let mut count = 0;
    for file in files {
        let source = std::fs::read_to_string(&file)?;
        let frames: Vec<Value> = source
            .lines()
            .filter(|s| !s.trim().is_empty())
            .map(serde_json::from_str)
            .collect::<Result<_, _>>()?;
        let root = scratch("live");
        let path = root.join("live.zbr.jsonl");
        script(&path, &frames, &[], &deltas(&frames));
        let mut peer = Peer::start(&root, &[], &[])?;
        peer.send(user(&format!("zb:play {} speed=0", path.display())))
            .await?;
        peer.child.stdin.take();
        let mut emitted = Vec::new();
        while let Some(line) =
            tokio::time::timeout(Duration::from_secs(5), peer.lines.next_line()).await??
        {
            let parsed = wire::parse_frame(&line)?;
            let value: Value = serde_json::from_str(&line)?;
            // The generated envelope precedes the exact recorded frames.
            emitted.push((value, format!("{parsed:?}")));
        }
        for frame in &frames {
            let expected = wire::parse_frame(&frame.to_string())?;
            assert!(
                emitted
                    .iter()
                    .any(|(v, p)| v == frame && *p == format!("{expected:?}")),
                "lost/changed fixture frame in {}: {frame}",
                file.display()
            );
            types.insert(frame["type"].as_str().unwrap_or("").to_owned());
            count += 1;
        }
        assert_eq!(
            emitted[4..4 + frames.len()]
                .iter()
                .map(|(v, _)| v.clone())
                .collect::<Vec<_>>(),
            frames,
            "recorded frame order changed in {}",
            file.display()
        );
        assert!(peer.child.wait().await?.success());
    }
    assert_eq!(
        types.into_iter().collect::<Vec<_>>(),
        [
            "api-request-blob",
            "assistant",
            "attachment",
            "control_request",
            "rate_limit_event",
            "result",
            "stream_event",
            "system",
            "user"
        ]
    );
    eprintln!("{count} recorded frames parsed and replayed");
    Ok(())
}

#[tokio::test]
async fn resume_requires_state_and_restores_count_and_hash() -> Result<()> {
    let root = scratch("resume");
    let mut missing = Peer::start(&root, &[], &["--resume=absent"])?;
    let error = missing.next().await?;
    assert_eq!(error["type"], "result");
    assert_eq!(error["is_error"], true);
    assert!(
        error["errors"][0]
            .as_str()
            .unwrap()
            .contains("No conversation found")
    );
    assert!(!missing.child.wait().await?.success());
    let mut peer = Peer::start(&root, &[], &[])?;
    peer.send(user("zb:echo first")).await?;
    let frames = peer.until_result().await?;
    let id = frames.last().unwrap()["session_id"]
        .as_str()
        .unwrap()
        .to_owned();
    peer.stop().await?;
    let mut peer = Peer::start(&root, &[], &["--resume", &id])?;
    peer.send(user("zb:echo second")).await?;
    let frames = peer.until_result().await?;
    assert_eq!(frames.last().unwrap()["session_id"], id);
    assert_eq!(deltas(&frames), "second");
    peer.stop().await?;
    let state: Value = serde_json::from_slice(&std::fs::read(
        root.join("state").join(format!("{id}.json")),
    )?)?;
    assert_eq!(state["turns"], 2);
    assert_eq!(state["text_sha256"], text_sha256("firstsecond"));
    Ok(())
}

#[tokio::test]
async fn interrupt_responds_during_long_script_and_cancels_queued_commands() -> Result<()> {
    let root = scratch("interrupt");
    let path = root.join("slow.zbr.jsonl");
    script(&path, &[], &[json!({"dt":10000,"zb":"turn_end"})], "");
    let mut peer = Peer::start(&root, &[], &[])?;
    peer.send(user(&format!("zb:play {}", path.display())))
        .await?;
    while peer.next().await?["subtype"] != "init" {}
    let mut queued = user("zb:echo must be cancelled");
    queued["uuid"] = "cmd-queued".into();
    peer.send(queued).await?;
    while peer.next().await?["state"] != "queued" {}
    peer.send(serde_json::from_str(&wire::stop_turn_request_line("stop"))?)
        .await?;
    let frames = peer.until_result().await?;
    assert!(frames.iter().any(|f| f["response"]["request_id"] == "stop"));
    assert!(frames.iter().any(|f| f["state"] == "cancelled"));
    assert_eq!(frames.last().unwrap()["subtype"], "error_during_execution");
    let cancelled = peer.next().await?;
    assert_eq!(cancelled["command_uuid"], "cmd-queued");
    assert_eq!(cancelled["state"], "cancelled");
    peer.send(user("zb:echo after")).await?;
    assert_eq!(deltas(&peer.until_result().await?), "after");
    peer.stop().await
}

#[tokio::test]
async fn ballast_300_mib_pss_and_owned_children() -> Result<()> {
    let root = scratch("ballast");
    let mut peer = Peer::start(
        &root,
        &[
            ("ZB_REPLAY_MODE", "ballast"),
            ("ZB_REPLAY_BALLAST_MIB", "300"),
            ("ZB_REPLAY_CHILDREN", "2:16"),
        ],
        &[],
    )?;
    peer.send(user("zb:hold")).await?;
    peer.until_result().await?;
    let pid = peer.child.id().unwrap();
    let start = Instant::now();
    let mut previous = 0.0f64;
    let mut stable = 0;
    let pss = loop {
        let smaps = std::fs::read_to_string(format!("/proc/{pid}/smaps_rollup"))?;
        let pss: f64 = smaps
            .lines()
            .find(|l| l.starts_with("Pss:"))
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse::<f64>()?
            / 1024.0;
        if pss >= 300.0 && (pss - previous).abs() < 0.1 {
            stable += 1;
        } else {
            stable = 0;
        }
        previous = pss;
        if stable >= 3 {
            break pss;
        }
        ensure!(
            start.elapsed() < Duration::from_secs(10),
            "ballast did not become resident: {pss} MiB"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert!(
        (285.0..=315.0).contains(&pss),
        "300 MiB ballast PSS was {pss}"
    );
    eprintln!("300 MiB ballast: {pss:.3} MiB total PSS");
    tokio::time::sleep(Duration::from_millis(100)).await;
    let children = std::fs::read_to_string(format!("/proc/{pid}/task/{pid}/children"))?;
    let children: Vec<u32> = children
        .split_whitespace()
        .map(str::parse)
        .collect::<Result<_, _>>()?;
    assert_eq!(children.len(), 2);
    peer.stop().await?;
    assert!(
        children
            .iter()
            .all(|p| !PathBuf::from(format!("/proc/{p}")).exists()),
        "ballast children survived clean EOF"
    );
    Ok(())
}

#[tokio::test]
async fn markers_follow_flushed_text_and_hash_includes_markers() -> Result<()> {
    let root = scratch("marker");
    let path = root.join("marked.zbr.jsonl");
    let sock_path = root.join("p.sock");
    let sock = ProbeSock::bind(&sock_path)?;
    sock.set_read_timeout(Some(Duration::from_secs(1)))?;
    let text = "a [[zb:chat1234:1]]";
    let mut lines = vec![
        json!({"zb":"meta","v":1,"name":"marker","seed":0,"frames":1,"text_bytes":text.len(),"text_sha256":text_sha256(text)}),
        json!({"dt":0,"zb":"marker"}),
        json!({"dt":0,"f":{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"a"}}}}),
        json!({"dt":0,"zb":"turn_end"}),
    ];
    std::fs::write(
        &path,
        lines
            .drain(..)
            .map(|v| v.to_string() + "\n")
            .collect::<String>(),
    )?;
    let mut peer = Peer::start(
        &root,
        &[
            ("ZB_PROBE_SOCK", sock_path.to_str().unwrap()),
            ("ZB_REPLAY_CHAT", "chat1234"),
        ],
        &[],
    )?;
    peer.send(user(&format!("zb:play {} speed=0", path.display())))
        .await?;
    let frames = peer.until_result().await?;
    let received = zeron_bench::mono_now();
    assert_eq!(deltas(&frames), text);
    assert_eq!(frames.last().unwrap()["is_error"], false);
    let mut mark = None;
    for _ in 0..3 {
        if let event @ ProbeEvent::Mark { .. } = sock.recv()? {
            mark = Some(event);
            break;
        }
    }
    match mark.context("missing marker event")? {
        ProbeEvent::Mark { chat, seq, t } => {
            assert_eq!(chat, "chat1234");
            assert_eq!(seq, 1);
            assert!(t <= received);
        }
        _ => unreachable!(),
    }
    peer.stop().await
}

#[tokio::test]
async fn mitm_never_leaks_user_prompt_or_tool_response_to_recording_stub() -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let root = scratch("mitm");
    let stub = root.join("real.py");
    std::fs::write(
        &stub,
        r#"#!/usr/bin/python3
import sys,json,os
with open(os.path.join(os.environ['HOME'],'recorded.jsonl'),'w',buffering=1) as log:
 for line in sys.stdin:
  log.write(line)
  f=json.loads(line)
  req=f.get('request',{})
  payload={'mcpServers':[{'name':'stub','status':'connected'}]} if req.get('subtype')=='mcp_status' else {'commands':[],'models':[]}
  print(json.dumps({'type':'control_response','response':{'subtype':'success','request_id':f['request_id'],'response':payload}}),flush=True)
"#,
    )?;
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o700))?;
    let socket = root.join("p.sock");
    let probe = ProbeSock::bind(&socket)?;
    probe.set_read_timeout(Some(Duration::from_secs(2)))?;
    let mut peer = Peer::start(
        &root,
        &[
            ("ZB_REPLAY_MODE", "mitm"),
            ("ZB_REPLAY_REAL_CLAUDE", stub.to_str().unwrap()),
            ("ZB_PROBE_SOCK", socket.to_str().unwrap()),
        ],
        &[],
    )?;
    peer.send(json!({"type":"control_request","request_id":"engine-init","request":{"subtype":"initialize"}})).await?;
    assert_eq!(peer.next().await?["response"]["request_id"], "engine-init");
    const SECRET: &str = "NEVER_SEND_THIS_USER_PROMPT_987654321";
    peer.send(user(SECRET)).await?;
    let frames = peer.until_result().await?;
    assert_eq!(deltas(&frames), "held");
    peer.send(json!({"type":"user","uuid":"image","message":{"role":"user","content":[{"type":"text","text":SECRET},{"type":"image","source":{"data":SECRET}}]}})).await?;
    peer.until_result().await?;
    peer.send(json!({"type":"control_response","response":{"response":{"updatedInput":{"prompt":SECRET}}}})).await?;
    peer.send(json!({"type":"unknown","prompt":SECRET})).await?;
    peer.send(json!({"type":"control_request","request_id":"model","request":{"subtype":"set_model","model":"haiku","prompt":SECRET},"message":SECRET})).await?;
    while peer.next().await?["response"]["request_id"] != "model" {}
    tokio::time::sleep(Duration::from_millis(350)).await;
    let mut ready = false;
    for _ in 0..6 {
        if let ProbeEvent::McpReady {
            servers, fallback, ..
        } = probe.recv()?
        {
            assert!(!fallback);
            assert_eq!(servers[0].name, "stub");
            ready = true;
            break;
        }
    }
    assert!(ready);
    peer.stop().await?;
    let recorded = std::fs::read_to_string(root.join("recorded.jsonl"))?;
    assert!(!recorded.contains(SECRET), "prompt leaked: {recorded}");
    let recorded: Vec<Value> = recorded
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?;
    assert!(recorded.iter().all(|v| v["type"] == "control_request"));
    assert!(
        recorded
            .iter()
            .any(|v| v["request"]["subtype"] == "mcp_status")
    );
    Ok(())
}

#[test]
fn script_rejects_bad_versions_counts_delays_and_nonfinite_speed() {
    assert!(Script::parse("{\"zb\":\"meta\",\"v\":2}").is_err());
    assert!(zeron_bench::replay::script::Play::parse("zb:play foo speed=NaN").is_err());
    assert!(zeron_bench::replay::script::Play::parse("zb:play foo speed=-1").is_err());
    assert!(zeron_bench::replay::script::Play::parse("zb:play foo loop=0").is_err());
}

#[tokio::test]
async fn exec_inherits_build_env_and_reports_tool_result_and_probe() -> Result<()> {
    let root = scratch("exec");
    let socket = root.join("p.sock");
    let probe = ProbeSock::bind(&socket)?;
    probe.set_read_timeout(Some(Duration::from_secs(1)))?;
    let path = root.join("exec.zbr.jsonl");
    script(
        &path,
        &[],
        &[
            json!({"zb":"exec","dt":0,"argv":["/bin/sh","-c","printf '%s' \"$MAKEFLAGS\""],"cwd":"$CWD","tool":"Bash"}),
            json!({"zb":"turn_end","dt":0}),
        ],
        "",
    );
    let mut peer = Peer::start(
        &root,
        &[
            ("MAKEFLAGS", "inherited-bench-flags"),
            ("ZB_PROBE_SOCK", socket.to_str().unwrap()),
        ],
        &[],
    )?;
    peer.send(user(&format!("zb:play {} speed=0", path.display())))
        .await?;
    let frames = peer.until_result().await?;
    assert!(
        frames
            .iter()
            .any(|f| f["message"]["content"][0]["name"] == "Bash")
    );
    assert!(
        frames
            .iter()
            .any(|f| f["message"]["content"][0]["type"] == "tool_result"
                && f["message"]["content"][0]["content"] == "inherited-bench-flags")
    );
    let mut found = false;
    for _ in 0..3 {
        if let ProbeEvent::Exec {
            t0,
            t1,
            status,
            makeflags_present,
            ..
        } = probe.recv()?
        {
            assert!(t1 >= t0);
            assert_eq!(status, 0);
            assert!(makeflags_present);
            found = true;
            break;
        }
    }
    assert!(found);
    peer.stop().await
}

#[tokio::test]
async fn multiple_turns_keep_one_init_and_log_env_values_only_when_allowed() -> Result<()> {
    let root = scratch("turns");
    let mut peer = Peer::start(
        &root,
        &[
            ("PRIVATE_TOKEN", "secret-not-logged"),
            ("ZB_ENV_CAPTURE", "ALLOWED"),
            ("ALLOWED", "captured"),
        ],
        &[],
    )?;
    peer.send(user("zb:echo one")).await?;
    let first = peer.until_result().await?;
    peer.send(user("zb:echo two")).await?;
    let second = peer.until_result().await?;
    assert_eq!(first.iter().filter(|f| f["subtype"] == "init").count(), 1);
    assert!(!second.iter().any(|f| f["subtype"] == "init"));
    assert_eq!(
        first.last().unwrap()["session_id"],
        second.last().unwrap()["session_id"]
    );
    peer.stop().await?;
    let file = std::fs::read_dir(root.join("log"))?
        .next()
        .context("missing argv log")??
        .path();
    let bytes = std::fs::read_to_string(file)?;
    assert!(!bytes.contains("secret-not-logged"));
    let log: Value = serde_json::from_str(&bytes)?;
    assert!(
        log["env_names"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "PRIVATE_TOKEN")
    );
    assert_eq!(log["env"]["ALLOWED"], "captured");
    Ok(())
}

#[tokio::test]
async fn default_script_speed_loop_and_thinking_tool_blocks() -> Result<()> {
    let root = scratch("blocks");
    let path = root.join("blocks.zbr.jsonl");
    let frames=vec![
        json!({"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}}),
        json!({"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"consider"}}}),
        json!({"type":"stream_event","event":{"type":"content_block_stop","index":0}}),
        json!({"type":"stream_event","event":{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"$TOOL_ID","name":"Read","input":{}}}}),
        json!({"type":"stream_event","event":{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"file_path\":\"$CWD/file\"}"}}}),
        json!({"type":"stream_event","event":{"type":"content_block_stop","index":1}}),
        json!({"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"$TOOL_ID","content":"file bytes","is_error":false}]}}),
    ].into_iter().chain(text_frames("done")).collect::<Vec<_>>();
    script(&path, &frames, &[json!({"zb":"turn_end","dt":1})], "done");
    let mut peer = Peer::start(
        &root,
        &[("ZB_REPLAY_DEFAULT_SCRIPT", path.to_str().unwrap())],
        &[],
    )?;
    peer.send(user("unknown prompt uses default")).await?;
    let first = peer.until_result().await?;
    assert_eq!(deltas(&first), "done");
    assert!(
        first
            .iter()
            .any(|f| f["message"]["content"][0]["thinking"] == "consider")
    );
    assert!(
        first
            .iter()
            .any(|f| f["message"]["content"][0]["input"]["file_path"]
                == format!("{}/file", root.display()))
    );
    peer.send(user(&format!(
        "zb:play {} speed=0 loop=2 markers=1",
        path.display()
    )))
    .await?;
    let second = peer.until_result().await?;
    assert_eq!(
        deltas(&second),
        "done [[zb:00000000:1]]done [[zb:00000000:2]]"
    );
    peer.stop().await
}

#[test]
fn probe_socket_roundtrip_and_existing_bind_rejected() -> Result<()> {
    let root = scratch("probe");
    let path = root.join("p.sock");
    let receiver = ProbeSock::bind(&path)?;
    assert!(ProbeSock::bind(&path).is_err());
    let sender = std::os::unix::net::UnixDatagram::unbound()?;
    let event = ProbeEvent::Mark {
        chat: "12345678".into(),
        seq: 2,
        t: 123,
    };
    sender.send_to(&serde_json::to_vec(&event)?, &path)?;
    assert_eq!(receiver.recv()?, event);
    drop(receiver);
    assert!(!path.exists());
    Ok(())
}

#[tokio::test]
async fn interrupt_before_cold_init_still_initializes_the_next_turn() -> Result<()> {
    let root = scratch("cold-interrupt");
    let mut peer = Peer::start(&root, &[("ZB_REPLAY_STARTUP_MS", "100")], &[])?;
    peer.send(user("zb:hold")).await?;
    while peer.next().await?["state"] != "started" {}
    peer.send(
        json!({"type":"control_request","request_id":"stop","request":{"subtype":"interrupt"}}),
    )
    .await?;
    assert_eq!(peer.until_result().await?.last().unwrap()["is_error"], true);
    peer.send(user("zb:echo next")).await?;
    let frames = peer.until_result().await?;
    assert_eq!(frames.iter().filter(|f| f["subtype"] == "init").count(), 1);
    assert_eq!(deltas(&frames), "next");
    peer.stop().await
}

#[tokio::test]
async fn mitm_unsupported_status_uses_five_second_tree_stability_fallback() -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let root = scratch("fallback");
    let stub = root.join("real.py");
    std::fs::write(
        &stub,
        r#"#!/usr/bin/python3
import sys,json
for line in sys.stdin:
 f=json.loads(line)
 bad=f['request']['subtype']=='mcp_status'
 print(json.dumps({'type':'control_response','response':{'request_id':f['request_id'],'subtype':'error' if bad else 'success','error':'unsupported' if bad else None,'response':{}}}),flush=True)
"#,
    )?;
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o700))?;
    let socket = root.join("p.sock");
    let probe = ProbeSock::bind(&socket)?;
    probe.set_nonblocking(true)?;
    let peer = Peer::start(
        &root,
        &[
            ("ZB_REPLAY_MODE", "mitm"),
            ("ZB_REPLAY_REAL_CLAUDE", stub.to_str().unwrap()),
            ("ZB_PROBE_SOCK", socket.to_str().unwrap()),
        ],
        &[],
    )?;
    tokio::time::timeout(Duration::from_secs(9), async {
        loop {
            if let Ok(ProbeEvent::McpReady {
                t_init,
                t_ready,
                fallback,
                servers,
                ..
            }) = probe.recv()
            {
                assert!(fallback);
                assert!(servers.is_empty());
                assert!(t_ready - t_init >= 5_000_000_000);
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await?;
    peer.stop().await
}

#[tokio::test]
async fn mitm_refuses_argv_prompts_before_starting_real_child() -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let root = scratch("argv-prompt");
    let stub = root.join("stub");
    std::fs::write(
        &stub,
        "#!/bin/sh\ntouch \"$HOME/real-was-started\"\ncat > \"$HOME/leaked\"\n",
    )?;
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o700))?;
    for args in [
        &["-p", "TOP_SECRET_ARGV_PROMPT"][..],
        &["--system-prompt", "TOP_SECRET_ARGV_PROMPT"][..],
    ] {
        let mut peer = Peer::start(
            &root,
            &[
                ("ZB_REPLAY_MODE", "mitm"),
                ("ZB_REPLAY_REAL_CLAUDE", stub.to_str().unwrap()),
            ],
            args,
        )?;
        assert!(
            !tokio::time::timeout(Duration::from_secs(5), peer.child.wait())
                .await??
                .success()
        );
    }
    assert!(!root.join("real-was-started").exists());
    assert!(!root.join("leaked").exists());
    Ok(())
}

#[tokio::test]
async fn sigterm_with_open_stdin_reaps_owned_ballast_children() -> Result<()> {
    let root = scratch("term");
    let mut peer = Peer::start(&root, &[("ZB_REPLAY_CHILDREN", "1:1")], &[])?;
    peer.send(user("zb:hold")).await?;
    peer.until_result().await?;
    let pid = peer.child.id().unwrap();
    let children = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let children = std::fs::read_to_string(format!("/proc/{pid}/task/{pid}/children"))?;
            if !children.trim().is_empty() {
                return Ok::<_, anyhow::Error>(children);
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await??;
    let children: Vec<u32> = children
        .split_whitespace()
        .map(str::parse)
        .collect::<Result<_, _>>()?;
    assert_eq!(unsafe { libc::kill(pid as i32, libc::SIGTERM) }, 0);
    assert!(
        tokio::time::timeout(Duration::from_secs(2), peer.child.wait())
            .await??
            .success()
    );
    assert!(
        children
            .iter()
            .all(|p| !PathBuf::from(format!("/proc/{p}")).exists())
    );
    Ok(())
}

#[tokio::test]
async fn ballast_mode_still_plays_resume_marker_scripts() -> Result<()> {
    let root = scratch("ballast-play");
    let path = root.join("short.zbr.jsonl");
    script(
        &path,
        &text_frames("resumed"),
        &[json!({"zb":"turn_end","dt":0})],
        "resumed",
    );
    let mut peer = Peer::start(
        &root,
        &[
            ("ZB_REPLAY_MODE", "ballast"),
            ("ZB_REPLAY_BALLAST_MIB", "1"),
        ],
        &[],
    )?;
    peer.send(user(&format!(
        "zb:play {} markers=1 speed=0",
        path.display()
    )))
    .await?;
    let frames = peer.until_result().await?;
    assert_eq!(deltas(&frames), "resumed [[zb:00000000:1]]");
    assert_eq!(frames.last().unwrap()["is_error"], false);
    peer.stop().await
}
