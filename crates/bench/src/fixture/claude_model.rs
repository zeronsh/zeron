//! Deterministic loopback Messages fixture for credential-free Claude trials.
//! This server does not forward requests. Run Claude in a loopback-only network
//! namespace as well; an endpoint override alone cannot constrain all traffic.
use std::net::SocketAddr;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

#[derive(Clone, Debug)]
pub struct ModelRequest {
    pub method: String,
    pub path: String,
    pub t_ns: u64,
}

pub struct ClaudeModel {
    pub addr: SocketAddr,
    requests: Arc<Mutex<Vec<ModelRequest>>>,
    hold: Arc<AtomicBool>,
    task: JoinHandle<()>,
}

impl ClaudeModel {
    pub async fn start() -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = requests.clone();
        let hold = Arc::new(AtomicBool::new(false));
        let held = hold.clone();
        let task = tokio::spawn(async move {
            while let Ok((stream, peer)) = listener.accept().await {
                if !peer.ip().is_loopback() {
                    continue;
                }
                let recorded = recorded.clone();
                let held = held.clone();
                tokio::spawn(async move {
                    if let Err(err) = serve(stream, recorded, held).await {
                        eprintln!("model fixture: {err:#}");
                    }
                });
            }
        });
        Ok(Self {
            addr,
            requests,
            hold,
            task,
        })
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }
    pub fn requests(&self) -> Vec<String> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .map(|event| event.path.clone())
            .collect()
    }

    pub fn events(&self) -> Vec<ModelRequest> {
        self.requests.lock().unwrap().clone()
    }

    pub fn hold_responses(&self, held: bool) {
        self.hold.store(held, Ordering::Relaxed);
    }
}

impl Drop for ClaudeModel {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve(
    mut stream: TcpStream,
    requests: Arc<Mutex<Vec<ModelRequest>>>,
    hold: Arc<AtomicBool>,
) -> Result<()> {
    let mut buf = Vec::new();
    let header_end = loop {
        ensure!(buf.len() < 65536, "model request headers too large");
        let n = stream.read_buf(&mut buf).await?;
        ensure!(n != 0, "model request closed before headers");
        if let Some(pos) = buf.windows(4).position(|b| b == b"\r\n\r\n") {
            break pos + 4;
        }
    };
    let headers = std::str::from_utf8(&buf[..header_end])?;
    let mut lines = headers.lines();
    let first = lines.next().context("empty request")?;
    let mut fields = first.split_whitespace();
    let method = fields
        .next()
        .context("model request missing method")?
        .to_string();
    let path = fields
        .next()
        .context("model request missing path")?
        .to_string();
    requests.lock().unwrap().push(ModelRequest {
        method: method.clone(),
        path: path.clone(),
        t_ns: crate::mono_now(),
    });
    ensure!(method == "POST", "model request is not POST");
    let length: usize = lines
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .context("model request missing content length")?
        .1
        .trim()
        .parse()?;
    ensure!(length <= 4 * 1024 * 1024, "model request too large");
    while buf.len() - header_end < length {
        ensure!(stream.read_buf(&mut buf).await? != 0, "short model request");
    }
    let payload: Value = serde_json::from_slice(&buf[header_end..header_end + length])?;
    while hold.load(Ordering::Relaxed) {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let endpoint = path.split('?').next().unwrap_or("");
    let (content_type, body) = if endpoint == "/v1/messages/count_tokens" {
        ("application/json", json!({"input_tokens":0}).to_string())
    } else if endpoint == "/v1/messages" {
        let model = payload["model"].as_str().unwrap_or("claude-haiku-4-5");
        let events = [
            (
                "message_start",
                json!({"type":"message_start","message":{"id":"msg_fixture","type":"message","role":"assistant","model":model,"content":[],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":0,"output_tokens":0}}}),
            ),
            (
                "content_block_start",
                json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
            ),
            (
                "content_block_delta",
                json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"fixture complete"}}),
            ),
            (
                "content_block_stop",
                json!({"type":"content_block_stop","index":0}),
            ),
            (
                "message_delta",
                json!({"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":0}}),
            ),
            ("message_stop", json!({"type":"message_stop"})),
        ];
        (
            "text/event-stream",
            events
                .iter()
                .map(|(kind, body)| format!("event: {kind}\ndata: {body}\n\n"))
                .collect(),
        )
    } else {
        bail!("unexpected model endpoint {path}");
    };
    let headers = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(headers.as_bytes()).await?;
    stream.write_all(body.as_bytes()).await?;
    stream.shutdown().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn loopback_returns_zero_usage_and_records_requests() {
        let fixture = ClaudeModel::start().await.unwrap();
        let mut stream = TcpStream::connect(fixture.addr).await.unwrap();
        let body = r#"{"model":"fixture-model"}"#;
        stream
            .write_all(
                format!(
                    "POST /v1/messages HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();
        assert!(response.contains("text/event-stream"));
        assert!(response.contains("\"input_tokens\":0"));
        assert!(response.contains("\"output_tokens\":0"));
        assert_eq!(fixture.requests(), ["/v1/messages"]);
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    #[ignore = "requires a real Claude CLI and a loopback-only network namespace"]
    async fn real_cli_uses_loopback_with_zero_usage() {
        use std::process::Stdio;
        use std::time::Duration;
        use tokio::io::{AsyncBufReadExt, BufReader};
        use tokio::process::Command;
        let interfaces = std::fs::read_to_string("/proc/net/dev").unwrap();
        let names: Vec<_> = interfaces
            .lines()
            .skip(2)
            .filter_map(|line| line.split_once(':').map(|(name, _)| name.trim()))
            .collect();
        assert_eq!(
            names,
            ["lo"],
            "run in a network namespace with only loopback"
        );
        use std::os::unix::fs::MetadataExt;
        let mapped_tmp = std::fs::metadata("/tmp").unwrap();
        let scratch_tmp = std::fs::metadata(std::env::var_os("TMPDIR").unwrap()).unwrap();
        assert_eq!(
            (mapped_tmp.dev(), mapped_tmp.ino()),
            (scratch_tmp.dev(), scratch_tmp.ino()),
            "map /tmp to the scratch TMPDIR"
        );
        for path in [
            "/etc/claude-code/managed-mcp.json",
            "/etc/claude-code/managed-settings.json",
        ] {
            assert!(
                !std::path::Path::new(path).exists(),
                "managed Claude config is unsafe"
            );
        }
        assert!(
            std::fs::read_dir("/etc/claude-code/managed-settings.d")
                .map(|mut d| d.next().is_none())
                .unwrap_or(true)
        );
        let root = std::path::PathBuf::from(std::env::var_os("TMPDIR").unwrap()).join(format!(
            "claude-loopback-{}-{}",
            std::process::id(),
            crate::mono_now()
        ));
        for name in ["home", "claude", "data", "codex", "tmp", "xdg", "project"] {
            std::fs::create_dir_all(root.join(name)).unwrap();
        }
        assert!(
            std::process::Command::new("/usr/bin/git")
                .env_clear()
                .env("HOME", root.join("home"))
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .args(["init", "-q"])
                .arg(root.join("project"))
                .status()
                .unwrap()
                .success()
        );
        let model = ClaudeModel::start().await.unwrap();
        let cli = std::env::var_os("ZERON_TEST_CLAUDE").expect("explicit real Claude binary");
        let mut cmd = Command::new(cli);
        cmd.env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", root.join("home"))
            .env("CLAUDE_CONFIG_DIR", root.join("claude"))
            .env("TMPDIR", root.join("tmp"))
            .env("XDG_CONFIG_HOME", root.join("xdg"))
            .env("XDG_DATA_HOME", root.join("data"))
            .env("XDG_CACHE_HOME", root.join("tmp"))
            .env("ZERON_DATA_DIR", root.join("data"))
            .env("CODEX_HOME", root.join("codex"))
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("ANTHROPIC_API_KEY", "invalid-zeron-zero-token-verification")
            .env("ANTHROPIC_BASE_URL", model.url())
            .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
            .env("CLAUDE_CODE_AUTO_CONNECT_IDE", "0")
            .env("CLAUDE_CODE_IDE_SKIP_AUTO_INSTALL", "1")
            .current_dir(root.join("project"))
            .args([
                "--print",
                "--input-format",
                "stream-json",
                "--output-format",
                "stream-json",
                "--verbose",
                "--include-partial-messages",
                "--replay-user-messages",
                "--permission-prompt-tool",
                "stdio",
                "--model",
                "haiku",
                "--settings",
                "{\"disableAllHooks\":true}",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = cmd.spawn().unwrap();
        let mut input = child.stdin.take().unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap()).lines();
        input.write_all(b"{\"type\":\"control_request\",\"request_id\":\"init\",\"request\":{\"subtype\":\"initialize\"}}\n").await.unwrap();
        let result = tokio::time::timeout(Duration::from_secs(45), async {
            let mut sent = false;
            while let Some(line) = output.next_line().await.unwrap() {
                let Ok(frame) = serde_json::from_str::<Value>(&line) else { continue };
                if frame["response"]["request_id"] == "init" && !sent {
                    input.write_all(b"{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"offline fixture\"}}\n").await.unwrap();
                    sent = true;
                }
                if frame["type"] == "result" { return frame; }
            }
            panic!("Claude exited before result");
        }).await.expect("real Claude test timed out");
        child.start_kill().ok();
        child.wait().await.unwrap();
        assert_ne!(result["is_error"], true, "{result}");
        assert_eq!(result["usage"]["input_tokens"], 0, "{result}");
        assert_eq!(result["usage"]["output_tokens"], 0, "{result}");
        assert_eq!(result["total_cost_usd"].as_f64(), Some(0.0), "{result}");
        assert!(
            model
                .requests()
                .iter()
                .any(|p| p.split('?').next() == Some("/v1/messages"))
        );
        assert!(model.requests().iter().all(|p| matches!(
            p.split('?').next(),
            Some("/v1/messages" | "/v1/messages/count_tokens")
        )));
        std::fs::remove_dir_all(root).unwrap();
    }
}
