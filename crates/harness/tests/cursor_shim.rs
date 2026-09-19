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
    ) -> (
        tokio::process::Child,
        tokio::process::ChildStdin,
        tokio::io::Lines<tokio::io::BufReader<tokio::process::ChildStdout>>,
    ) {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
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
        let lines = tokio::io::BufReader::new(child.stdout.take().unwrap()).lines();
        let frame = serde_json::json!({"op":"run","prompt":prompt,"cwd":self.dir.path(),"resume":resume.then_some("agent-fixture"),"modelOptions":if prompt == "verify-mode:plan" {serde_json::json!({"agentMode":"plan"})} else if prompt == "verify-mode:agent" {serde_json::json!({"agentMode":"default"})} else {serde_json::json!({})}});
        stdin
            .write_all(format!("{frame}\n").as_bytes())
            .await
            .unwrap();
        (child, stdin, lines)
    }
}
async fn frame(
    lines: &mut tokio::io::Lines<tokio::io::BufReader<tokio::process::ChildStdout>>,
) -> serde_json::Value {
    let line = tokio::time::timeout(std::time::Duration::from_secs(5), lines.next_line())
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
async fn native_plan_mode_is_sent_on_create_and_reset_on_resume() {
    let fixture = SessionFixture::new();
    for (prompt, resume) in [("verify-mode:plan", false), ("verify-mode:agent", true)] {
        let (mut child, stdin, mut lines) = fixture.start(prompt, resume).await;
        assert_eq!(frame(&mut lines).await["ev"], "ready");
        assert_eq!(frame(&mut lines).await["ev"], "text");
        assert_eq!(frame(&mut lines).await["status"], "finished");
        finish(&mut child, stdin).await;
    }
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
    for round in 0..100 {
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
        201,
        "recovery must not send any unrequested prompts"
    );
    println!(
        "stress: 100 crashed/disconnected/cancelled/errored sessions, 100 successful same-session recoveries, zero history loss"
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
    use tokio::io::AsyncBufReadExt;
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
    let mut lines = tokio::io::BufReader::new(engine.stdout.take().unwrap()).lines();
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
