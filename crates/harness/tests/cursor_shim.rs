//! Execute the production JavaScript shim with a synthetic SDK. A shell shim
//! cannot catch Node dropping buffered stdout during process.exit().
use std::process::Stdio;

async fn run_shim(sdk: &str, mode: &str) -> std::process::Output {
    let dir = tempfile::tempdir().unwrap();
    let package = dir.path().join("node_modules/@cursor/sdk");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        package.join("package.json"),
        r#"{"type":"module","exports":"./index.mjs"}"#,
    )
    .unwrap();
    std::fs::write(package.join("index.mjs"), sdk).unwrap();
    let shim = dir.path().join("shim.mjs");
    std::fs::write(&shim, include_str!("../src/cursor/shim.mjs")).unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio::process::Command::new("node")
            .arg(shim)
            .arg(mode)
            .arg(dir.path().join("auth.json"))
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .expect("shim must exit despite SDK background handles")
    .expect("Node is required to exercise the Cursor shim")
}

#[tokio::test]
async fn large_catalog_is_fully_flushed_before_exit() {
    let output = run_shim(
        r#"
        setInterval(() => {}, 1000);
        export const Cursor = { models: { list: async () => Array.from(
          {length: 4096}, (_, i) => ({id: `model-${i}`, displayName: '模型 ' + i,
          description: 'x'.repeat(1024), parameters: [], variants: []})
        ) } };
        "#,
        "models",
    )
    .await;
    assert!(output.status.success());
    assert!(output.stdout.len() > 4 * 1024 * 1024);
    let frame: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(frame["ev"], "models");
    assert_eq!(frame["items"].as_array().unwrap().len(), 4096);
    assert_eq!(frame["items"][4095]["id"], "model-4095");
}

#[tokio::test]
async fn large_fatal_frame_is_fully_flushed_and_exits_unsuccessfully() {
    let output = run_shim(
        r#"export const Cursor = { models: { list: async () => {
          throw new Error('x'.repeat(1024 * 1024));
        } } };"#,
        "models",
    )
    .await;
    assert!(!output.status.success());
    let frame: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(frame["ev"], "fatal");
    assert!(frame["message"].as_str().unwrap().len() > 1024 * 1024);
}

#[tokio::test]
async fn login_frames_are_flushed_before_exit() {
    let output = run_shim(
        r#"
        export class FileCredentialStore { constructor(path) {} }
        export const Cursor = { auth: { login: async ({onLoginUrl}) => {
          onLoginUrl('https://example.test/' + 'x'.repeat(1024 * 1024));
          return {email: 'test@example.test'};
        } } };
        "#,
        "login",
    )
    .await;
    assert!(output.status.success());
    let frames: Vec<serde_json::Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0]["ev"], "auth-url");
    assert_eq!(frames[1]["ev"], "logged-in");
}

struct SessionFixture {
    dir: tempfile::TempDir,
}
impl SessionFixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let package = dir.path().join("node_modules/@cursor/sdk");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(
            package.join("package.json"),
            r#"{"type":"module","exports":"./index.mjs"}"#,
        )
        .unwrap();
        std::fs::write(
            package.join("index.mjs"),
            include_str!("fixtures/fake-cursor-sdk.mjs"),
        )
        .unwrap();
        let source = std::env::var_os("ZERON_CURSOR_TEST_SHIM")
            .map(|path| std::fs::read_to_string(path).unwrap())
            .unwrap_or_else(|| include_str!("../src/cursor/shim.mjs").to_owned());
        std::fs::write(dir.path().join("shim.mjs"), source).unwrap();
        Self { dir }
    }
    async fn start(
        &self,
        prompt: &str,
        resume: bool,
    ) -> (tokio::process::Child, tokio::process::ChildStdin, Frames) {
        self.start_with_mcp(prompt, resume, serde_json::Value::Null)
            .await
    }
    async fn start_with_mcp(
        &self,
        prompt: &str,
        resume: bool,
        mcp: serde_json::Value,
    ) -> (tokio::process::Child, tokio::process::ChildStdin, Frames) {
        self.spawn(prompt, resume, mcp, false).await
    }
    /// The SDK's native steering path alone, without text preemption.
    async fn start_native(
        &self,
        prompt: &str,
    ) -> (tokio::process::Child, tokio::process::ChildStdin, Frames) {
        self.spawn(prompt, false, serde_json::Value::Null, true)
            .await
    }
    async fn spawn(
        &self,
        prompt: &str,
        resume: bool,
        mcp: serde_json::Value,
        native_only: bool,
    ) -> (tokio::process::Child, tokio::process::ChildStdin, Frames) {
        use tokio::io::AsyncWriteExt;
        let mut child = tokio::process::Command::new("node")
            .arg(self.dir.path().join("shim.mjs"))
            .env("ZERON_CURSOR_STATE_DIR", self.dir.path().join("state"))
            .env(
                "ZERON_CURSOR_NATIVE_STEER_ONLY",
                if native_only { "1" } else { "0" },
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let lines = Frames::new(child.stdout.take().unwrap());
        let frame = serde_json::json!({"op":"run","prompt":prompt,"cwd":self.dir.path(),"resume":resume.then_some("agent-fixture"),"mcp":mcp});
        stdin
            .write_all(format!("{frame}\n").as_bytes())
            .await
            .unwrap();
        (child, stdin, lines)
    }
}
/// A shim's stdout, one JSON frame per line.
struct Frames {
    lines: tokio::io::Lines<tokio::io::BufReader<tokio::process::ChildStdout>>,
    started: bool,
}
impl Frames {
    fn new(stdout: tokio::process::ChildStdout) -> Self {
        use tokio::io::AsyncBufReadExt;
        Self {
            lines: tokio::io::BufReader::new(stdout).lines(),
            started: false,
        }
    }
    async fn next_line(&mut self) -> std::io::Result<Option<String>> {
        self.lines.next_line().await
    }
}
/// The next frame, within 5 s. A shim's first frame also waits for `node`
/// to start, which alone has taken over 5 s on Windows CI runners.
async fn frame(lines: &mut Frames) -> serde_json::Value {
    let bound = if lines.started { 5 } else { 30 };
    lines.started = true;
    let line = tokio::time::timeout(std::time::Duration::from_secs(bound), lines.next_line())
        .await
        .expect("bounded frame")
        .unwrap()
        .expect("shim closed early");
    serde_json::from_str(&line).unwrap()
}
async fn finish(child: &mut tokio::process::Child, stdin: tokio::process::ChildStdin) {
    drop(stdin);
    tokio::time::timeout(std::time::Duration::from_secs(4), child.wait())
        .await
        .expect("bounded teardown")
        .unwrap();
}

#[tokio::test]
async fn stress_100_interrupted_sessions_recover_without_losing_history_or_replaying() {
    use tokio::io::AsyncWriteExt;
    let fixture = SessionFixture::new();
    let (mut child, stdin, mut lines) = fixture.start("normal", false).await;
    assert_eq!(frame(&mut lines).await["ev"], "ready");
    assert_eq!(
        frame(&mut lines).await["text"],
        "retained-conversation-history"
    );
    assert_eq!(frame(&mut lines).await["status"], "finished");
    finish(&mut child, stdin).await;
    // Pull request CI runs two cycles of the seven failure modes
    // (ZERON_STRESS_ROUNDS=14); the nightly run and local runs do 100.
    let rounds: usize = std::env::var("ZERON_STRESS_ROUNDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(100);
    for round in 0..rounds {
        let mode = round % 7;
        let prompt = match mode {
            3 => "send-error",
            4 => "wait-error",
            5 => "auth-error",
            6 => "hung-cancel",
            _ => "hang",
        };
        let (mut child, mut stdin, mut lines) = fixture.start(prompt, true).await;
        assert_eq!(frame(&mut lines).await["ev"], "ready");
        if mode != 3 {
            assert_eq!(
                frame(&mut lines).await["text"],
                "retained-conversation-history"
            );
        }
        match mode {
            0 => {
                child.kill().await.unwrap();
                drop(stdin);
            }
            2 => {
                stdin.write_all(b"{\"op\":\"interrupt\"}\n").await.unwrap();
                assert_eq!(frame(&mut lines).await["status"], "cancelled");
                finish(&mut child, stdin).await;
            }
            3..=5 => {
                assert_eq!(frame(&mut lines).await["status"], "error");
                finish(&mut child, stdin).await;
            }
            _ => finish(&mut child, stdin).await,
        }
        let (mut child, stdin, mut lines) = fixture.start("normal", true).await;
        assert_eq!(frame(&mut lines).await["agentId"], "agent-fixture");
        assert_eq!(
            frame(&mut lines).await["text"],
            "retained-conversation-history"
        );
        assert_eq!(frame(&mut lines).await["status"], "finished");
        finish(&mut child, stdin).await;
    }
    let store =
        std::fs::read_to_string(fixture.dir.path().join("state/by-agent/agent-fixture")).unwrap();
    assert_eq!(
        std::fs::read_to_string(std::path::Path::new(store.trim()).join("prompts.ndjson"))
            .unwrap()
            .lines()
            .count(),
        2 * rounds + 1,
        "recovery must not send any unrequested prompts"
    );
    println!(
        "stress: {rounds} crashed/disconnected/cancelled/errored sessions, {rounds} successful same-session recoveries, zero history loss"
    );
}

#[tokio::test]
async fn recovery_refuses_to_modify_a_store_with_a_live_owner() {
    let fixture = SessionFixture::new();
    let (mut first, stdin, mut lines) = fixture.start("hang", false).await;
    assert_eq!(frame(&mut lines).await["ev"], "ready");
    assert_eq!(frame(&mut lines).await["ev"], "text");
    let (mut second, second_stdin, mut second_lines) = fixture.start("normal", true).await;
    let rejected = frame(&mut second_lines).await;
    assert_eq!(rejected["ev"], "fatal");
    assert!(
        rejected["message"]
            .as_str()
            .unwrap()
            .contains("still running in another process")
    );
    finish(&mut second, second_stdin).await;
    finish(&mut first, stdin).await;
    let (mut recovered, stdin, mut lines) = fixture.start("normal", true).await;
    assert_eq!(frame(&mut lines).await["ev"], "ready");
    assert_eq!(
        frame(&mut lines).await["text"],
        "retained-conversation-history"
    );
    assert_eq!(frame(&mut lines).await["status"], "finished");
    finish(&mut recovered, stdin).await;
}

#[tokio::test]
async fn engine_death_does_not_leave_an_orphan_owning_the_conversation() {
    let fixture = SessionFixture::new();
    let parent = fixture.dir.path().join("parent.mjs");
    std::fs::write(
        &parent,
        r#"
      import {spawn} from 'node:child_process';
      const child=spawn(process.execPath,[process.argv[2]],{stdio:['pipe','inherit','inherit']});
      child.stdin.write(JSON.stringify({op:'run',prompt:'hang',cwd:process.cwd()})+'\n');
      // Keep another writer open after the parent dies: EOF alone cannot heal this.
      spawn(process.execPath,['-e','setTimeout(()=>{},10000)'],{stdio:['ignore','ignore','ignore',child.stdin]}).unref();
      setInterval(()=>{},1000);
    "#,
    )
    .unwrap();
    let mut engine = tokio::process::Command::new("node")
        .arg(parent)
        .arg(fixture.dir.path().join("shim.mjs"))
        .env("ZERON_CURSOR_STATE_DIR", fixture.dir.path().join("state"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut lines = Frames::new(engine.stdout.take().unwrap());
    assert_eq!(frame(&mut lines).await["ev"], "ready");
    assert_eq!(frame(&mut lines).await["ev"], "text");
    engine.kill().await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while lines.next_line().await.unwrap().is_some() {}
    })
    .await
    .expect("orphan must close its pipe");
    let (mut recovered, stdin, mut lines) = fixture.start("normal", true).await;
    assert_eq!(frame(&mut lines).await["ev"], "ready");
    assert_eq!(
        frame(&mut lines).await["text"],
        "retained-conversation-history"
    );
    assert_eq!(frame(&mut lines).await["status"], "finished");
    finish(&mut recovered, stdin).await;
}

#[tokio::test]
async fn reported_auth_error_preserves_diagnostics_and_never_replays_the_turn() {
    for prompt in ["incident-auth", "background-auth", "background-throw"] {
        let fixture = SessionFixture::new();
        let (mut child, stdin, mut lines) = fixture.start(prompt, false).await;
        assert_eq!(frame(&mut lines).await["ev"], "ready");
        assert_eq!(
            frame(&mut lines).await["text"],
            "retained-conversation-history"
        );
        let failure = frame(&mut lines).await;
        let message = if prompt.starts_with("background-") {
            assert_eq!(failure["ev"], "fatal");
            failure["message"].as_str().unwrap()
        } else {
            assert_eq!(failure["status"], "error");
            failure["error"].as_str().unwrap()
        };
        assert!(message.contains("Authentication error"), "{message}");
        assert!(message.contains("code=unauthenticated"), "{message}");
        assert!(message.contains("requestId=request-"), "{message}");
        assert!(!message.contains("DO-NOT-LOG"));
        finish(&mut child, stdin).await;
        let (mut child, stdin, mut lines) = fixture.start("normal", true).await;
        assert_eq!(frame(&mut lines).await["agentId"], "agent-fixture");
        assert_eq!(
            frame(&mut lines).await["text"],
            "retained-conversation-history"
        );
        assert_eq!(frame(&mut lines).await["status"], "finished");
        finish(&mut child, stdin).await;
        let store =
            std::fs::read_to_string(fixture.dir.path().join("state/by-agent/agent-fixture"))
                .unwrap();
        let prompts =
            std::fs::read_to_string(std::path::Path::new(store.trim()).join("prompts.ndjson"))
                .unwrap();
        let prompts: Vec<String> = prompts
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(prompts, vec![prompt, "normal"]);
    }
}

#[tokio::test]
async fn uncheckpointed_user_text_survives_resume_without_replaying_the_turn() {
    let fixture = SessionFixture::new();
    let original = "uncheckpointed-remember TOKEN and append once to a file";
    let (mut child, stdin, mut lines) = fixture.start(original, false).await;
    assert_eq!(frame(&mut lines).await["ev"], "ready");
    assert_eq!(frame(&mut lines).await["status"], "error");
    finish(&mut child, stdin).await;
    let (mut child, stdin, mut lines) = fixture.start("what was my last token?", true).await;
    assert_eq!(frame(&mut lines).await["ev"], "ready");
    assert_eq!(frame(&mut lines).await["ev"], "text");
    assert_eq!(frame(&mut lines).await["status"], "finished");
    finish(&mut child, stdin).await;
    let (mut child, stdin, mut lines) = fixture.start("next explicit message", true).await;
    assert_eq!(frame(&mut lines).await["ev"], "ready");
    assert_eq!(frame(&mut lines).await["ev"], "text");
    assert_eq!(frame(&mut lines).await["status"], "finished");
    finish(&mut child, stdin).await;
    let store =
        std::fs::read_to_string(fixture.dir.path().join("state/by-agent/agent-fixture")).unwrap();
    let log =
        std::fs::read_to_string(std::path::Path::new(store.trim()).join("prompts.ndjson")).unwrap();
    let prompts: Vec<String> = log
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(prompts.len(), 3, "no automatic prompt replay");
    assert_eq!(prompts[0], original);
    assert!(prompts[1].contains(original));
    assert!(prompts[1].contains("do not rerun their tools or side effects"));
    assert!(prompts[1].contains("what was my last token?"));
    assert_eq!(
        prompts[2], "next explicit message",
        "already checkpointed context must not be duplicated"
    );
}

#[tokio::test]
async fn repeated_startup_failures_retain_all_user_messages_without_nesting_or_duplicates() {
    let fixture = SessionFixture::new();
    for round in 0..20 {
        let (mut child, stdin, mut lines) = fixture
            .start(&format!("uncheckpointed-{round}"), round > 0)
            .await;
        assert_eq!(frame(&mut lines).await["ev"], "ready");
        assert_eq!(frame(&mut lines).await["status"], "error");
        finish(&mut child, stdin).await;
    }
    let (mut child, stdin, mut lines) = fixture.start("continue explicitly", true).await;
    assert_eq!(frame(&mut lines).await["ev"], "ready");
    assert_eq!(frame(&mut lines).await["ev"], "text");
    assert_eq!(frame(&mut lines).await["status"], "finished");
    finish(&mut child, stdin).await;
    let store =
        std::fs::read_to_string(fixture.dir.path().join("state/by-agent/agent-fixture")).unwrap();
    let receipt: serde_json::Value = serde_json::from_slice(
        &std::fs::read(std::path::Path::new(store.trim()).join(".zeron-user-receipt.json"))
            .unwrap(),
    )
    .unwrap();
    let messages = receipt["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 21);
    for (i, message) in messages.iter().take(20).enumerate() {
        assert_eq!(message, &format!("uncheckpointed-{i}"));
    }
    assert_eq!(messages[20], "continue explicitly");
    let log =
        std::fs::read_to_string(std::path::Path::new(store.trim()).join("prompts.ndjson")).unwrap();
    assert_eq!(log.lines().count(), 21, "no automatic retries");
}

#[tokio::test]
async fn corrupt_interrupted_receipt_fails_before_sending_a_contextless_prompt() {
    let fixture = SessionFixture::new();
    let (mut child, stdin, mut lines) = fixture.start("normal", false).await;
    assert_eq!(frame(&mut lines).await["ev"], "ready");
    assert_eq!(frame(&mut lines).await["ev"], "text");
    assert_eq!(frame(&mut lines).await["status"], "finished");
    finish(&mut child, stdin).await;
    let store =
        std::fs::read_to_string(fixture.dir.path().join("state/by-agent/agent-fixture")).unwrap();
    let root = std::path::Path::new(store.trim());
    std::fs::write(root.join(".zeron-user-receipt.json"), "{\"version\":999}").unwrap();
    let (mut child, stdin, mut lines) = fixture.start("must not send", true).await;
    let error = frame(&mut lines).await;
    assert_eq!(error["status"], "error");
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains("receipt is invalid")
    );
    finish(&mut child, stdin).await;
    assert_eq!(
        std::fs::read_to_string(root.join("prompts.ndjson"))
            .unwrap()
            .lines()
            .count(),
        1
    );
}

#[tokio::test]
async fn startup_catalog_rate_limit_retries_before_sending_but_auth_does_not() {
    for (message, succeeds) in [
        (
            "You have exceeded the rate limit of 30 requests per minute for the get_models endpoint",
            true,
        ),
        ("Authentication error", false),
    ] {
        let fixture = SessionFixture::new();
        let marker = fixture.dir.path().join("startup-limit.json");
        std::fs::write(
            &marker,
            serde_json::json!({"attempts":0,"failures":1,"message":message}).to_string(),
        )
        .unwrap();
        let (mut child, stdin, mut lines) = fixture.start("normal", false).await;
        if succeeds {
            assert_eq!(frame(&mut lines).await["ev"], "ready");
            assert_eq!(frame(&mut lines).await["ev"], "text");
            assert_eq!(frame(&mut lines).await["status"], "finished");
        } else {
            assert_eq!(frame(&mut lines).await["ev"], "fatal");
        }
        finish(&mut child, stdin).await;
        let state: serde_json::Value =
            serde_json::from_slice(&std::fs::read(marker).unwrap()).unwrap();
        assert_eq!(state["attempts"], if succeeds { 2 } else { 1 });
    }
}

#[tokio::test]
async fn mcp_injection_reaches_sdk_on_create_and_resume_with_fresh_identity() {
    let fixture = SessionFixture::new();
    for (resume, chat) in [(false, "first"), (true, "second")] {
        let (mut child, stdin, mut lines) = fixture
            .start_with_mcp(
                "normal",
                resume,
                serde_json::json!({
                    "name": "zeron", "command": "/path with spaces/zeron", "args": ["mcp"],
                    "env": {"ZERON_CHAT_ID": chat, "ZERON_IPC_PORT": "27699"},
                }),
            )
            .await;
        assert_eq!(frame(&mut lines).await["ev"], "ready");
        assert_eq!(frame(&mut lines).await["ev"], "text");
        assert_eq!(frame(&mut lines).await["status"], "finished");
        finish(&mut child, stdin).await;
        let options: serde_json::Value = serde_json::from_slice(
            &std::fs::read(fixture.dir.path().join("mcp-options.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(options["mcpServers"]["zeron"]["type"], "stdio");
        assert_eq!(
            options["mcpServers"]["zeron"]["command"],
            "/path with spaces/zeron"
        );
        assert_eq!(
            options["mcpServers"]["zeron"]["args"],
            serde_json::json!(["mcp"])
        );
        assert_eq!(options["mcpServers"]["zeron"]["env"]["ZERON_CHAT_ID"], chat);
        assert_eq!(
            options["mcpServers"]["zeron"]["env"]["ZERON_IPC_PORT"],
            "27699"
        );
        // Never "project"/"all": the SDK skips MCP approvals, so a repo's
        // .cursor/mcp.json would run unprompted.
        assert_eq!(
            options["settingSources"],
            serde_json::json!(["user", "team", "mdm", "plugins"])
        );
    }
}

#[tokio::test]
async fn rapid_steers_are_acknowledged_inside_the_active_sdk_run() {
    use tokio::io::AsyncWriteExt;
    let fixture = SessionFixture::new();
    let (mut child, mut stdin, mut lines) = fixture.start_native("native-steer").await;
    while frame(&mut lines).await["ev"] != "text" {}
    for i in 0..3 {
        stdin
            .write_all(format!("{{\"op\":\"steer\",\"prompt\":\"message-{i}\"}}\n").as_bytes())
            .await
            .unwrap();
    }
    let mut acknowledgments = 0;
    let mut messages = Vec::new();
    loop {
        let event = frame(&mut lines).await;
        match event["ev"].as_str().unwrap() {
            "steered" => acknowledgments += 1,
            "text" => messages.push(event["text"].as_str().unwrap().to_owned()),
            "turn" => {
                assert_eq!(event["status"], "finished");
                break;
            }
            other => panic!("unexpected frame {other}: {event}"),
        }
    }
    assert_eq!(
        acknowledgments, 3,
        "all steers must enter before the active turn ends"
    );
    assert_eq!(
        messages,
        ["NATIVE:message-0", "NATIVE:message-1", "NATIVE:message-2"]
    );
    finish(&mut child, stdin).await;
}

#[tokio::test]
async fn acknowledged_native_steer_survives_a_crash_before_checkpoint() {
    use tokio::io::AsyncWriteExt;
    let fixture = SessionFixture::new();
    let (mut child, mut stdin, mut lines) = fixture.start_native("native-steer").await;
    while frame(&mut lines).await["ev"] != "text" {}
    stdin
        .write_all(b"{\"op\":\"steer\",\"prompt\":\"uncheckpointed-steer-important\"}\n")
        .await
        .unwrap();
    while frame(&mut lines).await["ev"] != "steered" {}
    child.kill().await.unwrap();
    drop(stdin);
    let (mut child, stdin, mut lines) = fixture.start("normal", true).await;
    while frame(&mut lines).await["ev"] != "turn" {}
    finish(&mut child, stdin).await;
    let store =
        std::fs::read_to_string(fixture.dir.path().join("state/by-agent/agent-fixture")).unwrap();
    let prompts =
        std::fs::read_to_string(std::path::Path::new(store.trim()).join("prompts.ndjson")).unwrap();
    let last: String = serde_json::from_str(prompts.lines().last().unwrap()).unwrap();
    assert!(
        last.contains("uncheckpointed-steer-important"),
        "acknowledged steering was lost before its checkpoint"
    );
    assert!(last.contains("currentUserMessage\":\"normal"));
}

#[tokio::test]
async fn turn_end_race_coalesces_all_reverted_inputs_and_announces_boundary_before_text() {
    use tokio::io::AsyncWriteExt;
    let fixture = SessionFixture::new();
    let (mut child, mut stdin, mut lines) = fixture.start_native("native-revert").await;
    while frame(&mut lines).await["ev"] != "text" {}
    let batch = (0..3)
        .map(|i| format!("{{\"op\":\"steer\",\"prompt\":\"raced-{i}\"}}\n"))
        .collect::<String>();
    stdin.write_all(batch.as_bytes()).await.unwrap();
    assert_eq!(frame(&mut lines).await["ev"], "turn");
    for _ in 0..3 {
        assert_eq!(frame(&mut lines).await["ev"], "steered");
    }
    assert_eq!(frame(&mut lines).await["ev"], "text");
    assert_eq!(frame(&mut lines).await["status"], "finished");
    finish(&mut child, stdin).await;
    let store =
        std::fs::read_to_string(fixture.dir.path().join("state/by-agent/agent-fixture")).unwrap();
    let prompts =
        std::fs::read_to_string(std::path::Path::new(store.trim()).join("prompts.ndjson")).unwrap();
    assert_eq!(
        prompts.lines().count(),
        2,
        "a boundary race must not produce one turn per message"
    );
    for i in 0..3 {
        assert!(
            prompts
                .lines()
                .last()
                .unwrap()
                .contains(&format!("raced-{i}"))
        );
    }
}

#[tokio::test]
async fn native_steering_waits_for_tool_completion_but_not_turn_completion() {
    use tokio::io::AsyncWriteExt;
    let fixture = SessionFixture::new();
    let (mut child, mut stdin, mut lines) = fixture.start("native-tool", false).await;
    while frame(&mut lines).await["phase"] != "start" {}
    stdin
        .write_all(b"{\"op\":\"steer\",\"prompt\":\"keep-child-alive\"}\n")
        .await
        .unwrap();
    assert_eq!(frame(&mut lines).await["phase"], "end");
    assert_eq!(frame(&mut lines).await["text"], "NATIVE:keep-child-alive");
    assert_eq!(frame(&mut lines).await["ev"], "steered");
    assert_eq!(frame(&mut lines).await["status"], "finished");
    finish(&mut child, stdin).await;
}

#[tokio::test]
async fn newer_steers_submit_while_earlier_delivery_is_unacknowledged() {
    use tokio::io::AsyncWriteExt;
    let fixture = SessionFixture::new();
    let (mut child, mut stdin, mut lines) = fixture.start_native("native-concurrent").await;
    while frame(&mut lines).await["ev"] != "text" {}
    // Wait for each submission before sending the next. This is not a single
    // stdin burst: none of the delivery promises resolves until all three arrive.
    for i in 0..3 {
        stdin
            .write_all(format!("{{\"op\":\"steer\",\"prompt\":\"message-{i}\"}}\n").as_bytes())
            .await
            .unwrap();
        let submitted = frame(&mut lines).await;
        assert_eq!(submitted["ev"], "thinking");
        assert_eq!(submitted["text"], format!("submitted:message-{i}"));
    }
    let mut responses = Vec::new();
    let mut acknowledgments = 0;
    loop {
        let event = frame(&mut lines).await;
        match event["ev"].as_str().unwrap() {
            "text" => responses.push(event["text"].as_str().unwrap().to_owned()),
            "steered" => acknowledgments += 1,
            "turn" => break,
            other => panic!("unexpected {other}: {event}"),
        }
    }
    assert_eq!(responses, ["NATIVE:message-2"]);
    assert_eq!(acknowledgments, 3);
    finish(&mut child, stdin).await;
}

#[tokio::test]
async fn concurrent_boundary_race_retries_only_undelivered_input() {
    use tokio::io::AsyncWriteExt;
    let fixture = SessionFixture::new();
    let (mut child, mut stdin, mut lines) = fixture.start_native("native-mixed").await;
    while frame(&mut lines).await["ev"] != "text" {}
    for i in 0..3 {
        stdin
            .write_all(format!("{{\"op\":\"steer\",\"prompt\":\"mixed-{i}\"}}\n").as_bytes())
            .await
            .unwrap();
    }
    let mut turns = 0;
    let mut acknowledgments = 0;
    while turns < 2 {
        let event = frame(&mut lines).await;
        match event["ev"].as_str().unwrap() {
            "turn" => turns += 1,
            "steered" => acknowledgments += 1,
            "text" | "thinking" => {}
            other => panic!("unexpected {other}: {event}"),
        }
    }
    assert_eq!(acknowledgments, 3);
    finish(&mut child, stdin).await;
    let store =
        std::fs::read_to_string(fixture.dir.path().join("state/by-agent/agent-fixture")).unwrap();
    let prompts =
        std::fs::read_to_string(std::path::Path::new(store.trim()).join("prompts.ndjson")).unwrap();
    assert_eq!(prompts.lines().count(), 4);
    assert_eq!(prompts.lines().last().unwrap(), r#""mixed-0""#);
}

/// Cursor's native steer lands only at a step boundary, so a plain text
/// answer would finish first. With no tool running, a steer cancels the
/// streaming run and continues as the next turn in the same process: the
/// cancelled run reports no turn end and its late output never leaks.
#[tokio::test]
async fn steer_during_text_preempts_the_run_and_continues_immediately() {
    use tokio::io::AsyncWriteExt;
    let fixture = SessionFixture::new();
    let (mut child, mut stdin, mut lines) = fixture.start("hang", false).await;
    while frame(&mut lines).await["ev"] != "text" {}
    stdin
        .write_all(b"{\"op\":\"steer\",\"prompt\":\"preempt-me\"}\n")
        .await
        .unwrap();
    assert_eq!(frame(&mut lines).await["ev"], "steered");
    assert_eq!(frame(&mut lines).await["ev"], "text");
    let end = frame(&mut lines).await;
    assert_eq!(end["ev"], "turn");
    assert_eq!(end["status"], "finished");
    finish(&mut child, stdin).await;
    let store =
        std::fs::read_to_string(fixture.dir.path().join("state/by-agent/agent-fixture")).unwrap();
    let prompts =
        std::fs::read_to_string(std::path::Path::new(store.trim()).join("prompts.ndjson")).unwrap();
    assert_eq!(prompts.lines().count(), 2, "{prompts}");
    assert!(prompts.lines().last().unwrap().contains("preempt-me"));
}

/// A model switch runs from its own message's send on the live agent: the
/// messages sent before it never ride the new model, and those after it do.
#[tokio::test]
async fn a_model_switch_names_its_model_from_its_own_send_on() {
    use tokio::io::AsyncWriteExt;
    let fixture = SessionFixture::new();
    let (mut child, mut stdin, mut lines) = fixture.start("first", false).await;
    while frame(&mut lines).await["ev"] != "turn" {}
    stdin
        .write_all(
            concat!(
                "{\"op\":\"steer\",\"prompt\":\"plain-before\"}\n",
                "{\"op\":\"steer\",\"prompt\":\"on-two\",\"reconfigure\":true,\"model\":\"m-2\",\"modelOptions\":{}}\n",
                "{\"op\":\"steer\",\"prompt\":\"after\"}\n",
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let mut steered = 0;
    loop {
        let frame = frame(&mut lines).await;
        if frame["ev"] == "steered" {
            steered += 1;
        }
        if frame["ev"] == "turn" && steered == 3 {
            break;
        }
    }
    finish(&mut child, stdin).await;
    let store =
        std::fs::read_to_string(fixture.dir.path().join("state/by-agent/agent-fixture")).unwrap();
    let dir = std::path::Path::new(store.trim());
    let prompts = std::fs::read_to_string(dir.join("prompts.ndjson")).unwrap();
    let models = std::fs::read_to_string(dir.join("models.ndjson")).unwrap_or_default();
    let named: Vec<serde_json::Value> = models
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(named.iter().all(|send| send["model"] == "m-2"), "{models}");
    assert!(
        named
            .iter()
            .any(|send| send["prompt"].as_str().unwrap().contains("on-two")),
        "the switching message ran on its model: {models}"
    );
    assert!(
        named
            .iter()
            .all(|send| !send["prompt"].as_str().unwrap().contains("plain-before")),
        "an earlier message rode the new model: {models}"
    );
    assert!(
        prompts
            .lines()
            .any(|p| p.contains("plain-before") && !p.contains("on-two")),
        "an earlier message was merged into the switch: {prompts}"
    );
}

impl SessionFixture {
    /// [`Self::start`] with extra fields on the first frame.
    async fn start_frame(
        &self,
        prompt: &str,
        extra: serde_json::Value,
    ) -> (tokio::process::Child, tokio::process::ChildStdin, Frames) {
        use tokio::io::AsyncWriteExt;
        let mut child = tokio::process::Command::new("node")
            .arg(self.dir.path().join("shim.mjs"))
            .env("ZERON_CURSOR_STATE_DIR", self.dir.path().join("state"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let lines = Frames::new(child.stdout.take().unwrap());
        let mut frame = serde_json::json!({"op":"run","prompt":prompt,"cwd":self.dir.path()});
        for (key, value) in extra.as_object().unwrap() {
            frame[key] = value.clone();
        }
        stdin
            .write_all(format!("{frame}\n").as_bytes())
            .await
            .unwrap();
        (child, stdin, lines)
    }
    fn prompts(&self) -> Vec<String> {
        let Ok(store) =
            std::fs::read_to_string(self.dir.path().join("state/by-agent/agent-fixture"))
        else {
            return Vec::new();
        };
        std::fs::read_to_string(std::path::Path::new(store.trim()).join("prompts.ndjson"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}

/// Frames up to and including the turn's terminal one.
async fn turn_frames(lines: &mut Frames) -> Vec<serde_json::Value> {
    let mut frames = Vec::new();
    loop {
        let next = frame(lines).await;
        let end = next["ev"] == "turn" || next["ev"] == "fatal";
        frames.push(next);
        if end {
            return frames;
        }
    }
}

#[tokio::test]
async fn known_sdk_internal_rejections_do_not_kill_the_chat() {
    let fixture = SessionFixture::new();
    let (mut child, stdin, mut lines) = fixture.start("harmless-rejections", false).await;
    let frames = turn_frames(&mut lines).await;
    assert_eq!(frames.last().unwrap()["status"], "finished", "{frames:?}");
    finish(&mut child, stdin).await;
}

#[tokio::test]
async fn a_reply_that_is_only_a_transport_dump_fails_the_turn() {
    let fixture = SessionFixture::new();
    let (mut child, stdin, mut lines) = fixture.start("transport-dump", false).await;
    let frames = turn_frames(&mut lines).await;
    let last = frames.last().unwrap();
    assert_eq!(last["status"], "error", "{frames:?}");
    assert_eq!(
        last["error"],
        "Error: ConnectError: [unavailable] upstream connect error"
    );
    finish(&mut child, stdin).await;
    // Prose quoting the same diagnostic is a real answer.
    let fixture = SessionFixture::new();
    let (mut child, stdin, mut lines) = fixture.start("transport-quoted", false).await;
    let frames = turn_frames(&mut lines).await;
    assert_eq!(frames.last().unwrap()["status"], "finished", "{frames:?}");
    finish(&mut child, stdin).await;
}

#[tokio::test]
async fn a_settled_reply_that_never_streamed_is_still_shown() {
    let fixture = SessionFixture::new();
    let (mut child, stdin, mut lines) = fixture.start("result-only", false).await;
    let frames = turn_frames(&mut lines).await;
    assert!(
        frames
            .iter()
            .any(|f| f["ev"] == "text" && f["text"] == "settled reply"),
        "{frames:?}"
    );
    assert_eq!(frames.last().unwrap()["status"], "finished");
    finish(&mut child, stdin).await;
}

#[tokio::test]
async fn a_stale_active_run_is_expired_once_and_the_send_retried() {
    let fixture = SessionFixture::new();
    let (mut child, stdin, mut lines) = fixture.start("stale-active", false).await;
    let frames = turn_frames(&mut lines).await;
    assert_eq!(frames.last().unwrap()["status"], "finished", "{frames:?}");
    finish(&mut child, stdin).await;
    assert_eq!(fixture.prompts(), ["stale-active", "stale-active"]);
}

#[tokio::test]
async fn images_ride_the_send_with_the_text() {
    let fixture = SessionFixture::new();
    let images =
        serde_json::json!([{"type":"image","data":"aGk=","mimeType":"image/png"}, {"bogus":true}]);
    let (mut child, stdin, mut lines) = fixture
        .start_frame("image", serde_json::json!({"images": images}))
        .await;
    let frames = turn_frames(&mut lines).await;
    assert!(
        frames.iter().any(|f| f["text"] == " images:image/png/aGk="),
        "{frames:?}"
    );
    finish(&mut child, stdin).await;
}

/// A stop while startup waits out the model-catalog rate limit ends the turn
/// once, cancelled, without retrying or sending the prompt.
#[tokio::test]
async fn a_stop_during_startup_retries_is_not_lost() {
    use tokio::io::AsyncWriteExt;
    let fixture = SessionFixture::new();
    let marker = fixture.dir.path().join("startup-limit.json");
    std::fs::write(
        &marker,
        serde_json::json!({"attempts":0,"failures":3,
            "message":"You have exceeded the rate limit of 30 requests per minute for the get_models endpoint"})
        .to_string(),
    )
    .unwrap();
    let (mut child, mut stdin, mut lines) = fixture.start("normal", false).await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    stdin.write_all(b"{\"op\":\"stop\"}\n").await.unwrap();
    assert_eq!(
        frame(&mut lines).await,
        serde_json::json!({"ev":"turn","status":"cancelled"})
    );
    let rest = tokio::time::timeout(std::time::Duration::from_secs(5), lines.next_line())
        .await
        .expect("the shim exits")
        .unwrap();
    assert!(rest.is_none(), "{rest:?}");
    finish(&mut child, stdin).await;
    let state: serde_json::Value = serde_json::from_slice(&std::fs::read(marker).unwrap()).unwrap();
    assert_eq!(state["attempts"], 1);
    assert!(fixture.prompts().is_empty());
}

/// A stop while the agent is still being created settles the first turn
/// once, and its prompt never reaches the SDK.
#[tokio::test]
async fn a_stop_during_agent_creation_settles_once() {
    use tokio::io::AsyncWriteExt;
    let fixture = SessionFixture::new();
    std::fs::write(
        fixture.dir.path().join("startup-limit.json"),
        serde_json::json!({"attempts":0,"failures":0,"message":"","delayMs":400}).to_string(),
    )
    .unwrap();
    let (mut child, mut stdin, mut lines) = fixture.start("normal", false).await;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    stdin.write_all(b"{\"op\":\"stop\"}\n").await.unwrap();
    assert_eq!(frame(&mut lines).await["ev"], "ready");
    assert_eq!(
        frame(&mut lines).await,
        serde_json::json!({"ev":"turn","status":"cancelled"})
    );
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(500), lines.next_line())
            .await
            .is_err(),
        "a second terminal frame followed the stop"
    );
    finish(&mut child, stdin).await;
    assert!(fixture.prompts().is_empty());
}

/// A send the SDK never answers with a single update ends the turn errored
/// at the silence bound, whether the send itself or its run went quiet.
#[tokio::test]
async fn a_send_with_no_update_at_all_is_abandoned_at_the_bound() {
    for prompt in ["silent", "silent-send"] {
        let fixture = SessionFixture::new();
        let (mut child, stdin, mut lines) = fixture
            .start_frame(prompt, serde_json::json!({"silenceTimeoutMs": 300}))
            .await;
        let frames = turn_frames(&mut lines).await;
        let last = frames.last().unwrap();
        assert_eq!(last["status"], "error", "{prompt}: {frames:?}");
        assert!(
            last["error"]
                .as_str()
                .unwrap()
                .contains("sent nothing back"),
            "{prompt}: {frames:?}"
        );
        finish(&mut child, stdin).await;
    }
}

/// A send that comes back after its turn was abandoned is heard no more: the
/// engine already has the turn's end, and the process stays usable.
#[tokio::test]
async fn an_abandoned_send_that_comes_back_late_stays_silent() {
    let fixture = SessionFixture::new();
    let (mut child, stdin, mut lines) = fixture
        .start_frame("silent-late", serde_json::json!({"silenceTimeoutMs": 300}))
        .await;
    let frames = turn_frames(&mut lines).await;
    assert_eq!(frames.last().unwrap()["status"], "error", "{frames:?}");
    let late =
        tokio::time::timeout(std::time::Duration::from_millis(1200), lines.next_line()).await;
    assert!(late.is_err(), "output after the abandoned turn: {late:?}");
    finish(&mut child, stdin).await;
}

/// Updates keep a turn alive past the bound: only total silence trips it.
#[tokio::test]
async fn a_turn_that_streamed_outlives_the_silence_bound() {
    let fixture = SessionFixture::new();
    let (mut child, stdin, mut lines) = fixture
        .start_frame("hang", serde_json::json!({"silenceTimeoutMs": 200}))
        .await;
    assert_eq!(frame(&mut lines).await["ev"], "ready");
    assert_eq!(frame(&mut lines).await["ev"], "text");
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(600), lines.next_line())
            .await
            .is_err(),
        "a turn that streamed was abandoned"
    );
    finish(&mut child, stdin).await;
}

/// Images batched into one send stay under the engine's inline budget.
#[tokio::test]
async fn one_send_inlines_images_only_up_to_the_budget() {
    let fixture = SessionFixture::new();
    let image = serde_json::json!({"type":"image","data":"aGk=","mimeType":"image/png"});
    let (mut child, stdin, mut lines) = fixture
        .start_frame(
            "image",
            serde_json::json!({"images": [image, image], "imageBudget": 3}),
        )
        .await;
    let frames = turn_frames(&mut lines).await;
    assert!(
        frames.iter().any(|f| f["text"] == " images:image/png/aGk="),
        "{frames:?}"
    );
    finish(&mut child, stdin).await;
}
